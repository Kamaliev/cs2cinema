//! Команды бота: `/start`, `/help`, `/player`, ход задач (`/status`, `/queue`), управление ими (`/cancel`, `/retry`, `/resend`),
//! белый список (`/adduser`, `/removeuser`, `/users`) и раннеры (`/runner`, `/runners`).
//!
//! Роль берётся только из базы (`User::role`). Команды админа проверяют её до любого изменения; `/adduser`
//! создаёт только участника, поэтому повысить права через чат нельзя. Токен раннера через чат не проходит:
//! `/runner код` лишь одобряет заявку, в которой серверу известен только хеш токена.

use super::ingest::{self, ACTIVE_STATES, ahead_phrase};
use super::store::{self, NewTgJob};
use super::{Bot, deliver, notify};
use crate::auth;
use crate::core::{DedupKey, is_steam64, player_key};
use crate::users::{self, User};
use wire::{CancelMode, JobState};

const ADMIN_ONLY: &str = "Эта команда только для администратора.";
const RUNNER_FORMAT: &str = "Формат: /runner код [имя] — подключить раннер по коду из приложения (Настройки → «Подключить через Telegram»); \
                             /runner revoke имя — отозвать токен. Список: /runners";
const ADDUSER_FORMAT: &str =
    "Формат: /adduser Telegram-ID [имя]. Например: /adduser 123456789 петя";
const REMOVEUSER_FORMAT: &str = "Формат: /removeuser Telegram-ID. Например: /removeuser 123456789";
const PLAYER_FORMAT: &str = "Формат: /player ник или SteamID64 (17 цифр). Например: /player s1mple";
const CANCEL_FORMAT: &str = "Формат: /cancel [номер задачи]. Например: /cancel 12";
const RETRY_FORMAT: &str = "Формат: /retry [номер задачи]. Например: /retry 12";
const RESEND_FORMAT: &str = "Формат: /resend [номер задачи]. Например: /resend 12";
const NOT_YOURS: &str = "Это не ваша задача.";

/// Разбирает `/команда@имя_бота аргументы` в пару «команда без слеша в нижнем регистре, аргументы».
pub fn parse(text: &str) -> (String, String) {
    let text = text.trim();
    let body = text.strip_prefix('/').unwrap_or(text);
    let (head, args) = match body.split_once(char::is_whitespace) {
        Some((h, a)) => (h, a.trim()),
        None => (body, ""),
    };
    let cmd = head.split('@').next().unwrap_or(head).to_lowercase();
    (cmd, args.to_owned())
}

/// Меню команд для `setMyCommands`. Одно на всех: Telegram не умеет прятать команды по ролям, а доступ к
/// командам админа проверяется при выполнении.
pub fn menu() -> Vec<(String, String)> {
    [
        ("start", "Как пользоваться ботом"),
        ("help", "Справка"),
        ("player", "Игрок по умолчанию"),
        ("settings", "Настройки ролика"),
        ("status", "Мои задачи"),
        ("queue", "Очередь"),
        ("cancel", "Отменить задачу"),
        ("retry", "Повторить задачу"),
        ("resend", "Прислать ролик ещё раз"),
        ("adduser", "Добавить друга (админ)"),
        ("removeuser", "Убрать друга (админ)"),
        ("users", "Список друзей (админ)"),
        ("runner", "Подключить раннер по коду (админ)"),
        ("runners", "Список раннеров (админ)"),
    ]
    .into_iter()
    .map(|(c, d)| (c.to_owned(), d.to_owned()))
    .collect()
}

/// Символы, которым не место в нике и имени: управляющие и те, что меняют направление текста или
/// переносят строку (U+200E/F, U+202A-E, U+2066-9, U+2028/9, U+FEFF).
fn forbidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{FEFF}'
        )
}

/// Проверка ника игрока и имени друга: 1-64 символа без управляющих и bidi. Возвращает строку без краевых
/// пробелов или причину отказа для пользователя.
pub fn valid_player(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    let n = s.chars().count();
    if n == 0 {
        return Err("пусто".to_owned());
    }
    if n > 64 {
        return Err(format!("слишком длинно: {n} символов, допустимо до 64"));
    }
    if s.chars().any(forbidden_char) {
        return Err("в тексте управляющие символы или символы смены направления".to_owned());
    }
    Ok(s.to_owned())
}

fn is_admin(user: &User) -> bool {
    user.role == "admin"
}

fn player_label(user: &User) -> String {
    user.player_steam
        .clone()
        .or_else(|| user.player_name.clone())
        .unwrap_or_else(|| "не задан".to_owned())
}

fn start_text(user: &User) -> String {
    format!(
        "Я делаю ролик с лучшими моментами игрока из демки CS2.\n\n\
         1. Задайте игрока: /player ник или SteamID64. Он запоминается.\n\
         2. Пришлите демку файлом (.dem или .dem.zst) или ссылкой на комнату матча FACEIT.\n\
         3. Ник в подписи к файлу действует только на эту задачу.\n\n\
         Я напишу номер задачи, а готовый ролик пришлю отдельным сообщением.\n\
         Сейчас игрок: {}.\n\
         Все команды: /help",
        player_label(user)
    )
}

fn help_text(user: &User) -> String {
    let mut t = String::from(
        "Пришлите демку CS2 файлом (.dem или .dem.zst) — я смонтирую ролик с лучшими моментами игрока.\n\
         Ник или SteamID64 игрока напишите в подписи к файлу. Если подписи нет, берётся сохранённый игрок.\n\n\
         /player — показать игрока по умолчанию\n\
         /player ник — запомнить игрока (ник или SteamID64)\n\
         /status — мои задачи и их этапы\n\
         /queue — очередь и моё место в ней\n\
         /cancel [номер] — отменить задачу\n\
         /retry [номер] — повторить упавшую или отменённую задачу\n\
         /resend [номер] — прислать готовый ролик ещё раз\n\
         /start — как пользоваться\n\
         /help — эта справка",
    );
    if is_admin(user) {
        t.push_str(
            "\n\nУправление списком:\n\
             /adduser Telegram-ID [имя] — добавить друга\n\
             /removeuser Telegram-ID — убрать друга\n\
             /users — показать список\n\n\
             Раннеры (компьютеры, которые пишут ролики):\n\
             /runner код [имя] — подключить раннер по коду из приложения\n\
             /runner revoke имя — отозвать токен раннера\n\
             /runners — список раннеров",
        );
    }
    t
}

/// Сколько прошло с момента `ms` назад: по-русски, грубо.
fn ago(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    match secs {
        0..60 => "меньше минуты назад".to_owned(),
        60..3600 => format!("{} мин назад", secs / 60),
        3600..86_400 => format!("{} ч назад", secs / 3600),
        _ => format!("{} дн назад", secs / 86_400),
    }
}

/// `/runner код [имя]` одобряет заявку приложения (раннер создаётся с хешем из заявки), `/runner revoke имя`
/// отзывает токен. Сам токен в чат не попадает и серверу неизвестен.
fn runner_cmd(bot: &Bot, args: &str) -> String {
    let (head, rest) = match args.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (args, ""),
    };
    if head.is_empty() {
        return RUNNER_FORMAT.to_owned();
    }
    let db = bot.core().db();
    if head.eq_ignore_ascii_case("revoke") {
        if rest.is_empty() {
            return RUNNER_FORMAT.to_owned();
        }
        return match auth::revoke_runner(db, rest, bot.now_ms()) {
            Ok(()) => {
                bot.log()
                    .info(&format!("бот: токен раннера «{rest}» отозван админом"));
                format!(
                    "Токен раннера «{rest}» отозван: его задачи вернутся в очередь, а приложению понадобится новый код."
                )
            }
            Err(e) => format!("Не отозвал: {e}"),
        };
    }
    let name = (!rest.is_empty()).then_some(rest);
    match auth::approve_pairing(db, head, name, bot.now_ms()) {
        Ok(r) => {
            bot.log().info(&format!(
                "бот: раннер «{}» подключён по коду (отпечаток {})",
                r.name, r.fingerprint
            ));
            format!(
                "Раннер «{}» подключён (отпечаток {}). Приложение выйдет на связь само в течение минуты.",
                r.name, r.fingerprint
            )
        }
        Err(e) => format!("Не подключил: {e}"),
    }
}

fn runners_cmd(bot: &Bot) -> String {
    let list = match auth::list_runners(bot.core().db()) {
        Ok(l) => l,
        Err(e) => return format!("Не удалось получить список: {e}"),
    };
    if list.is_empty() {
        return "Раннеров нет. Подключить: в приложении «Подключить через Telegram», затем /runner код.".to_owned();
    }
    let now = bot.now_ms();
    let lines: Vec<String> = list
        .iter()
        .map(|r| {
            let state = if r.revoked_at.is_some() {
                "отозван".to_owned()
            } else {
                match r.last_seen_at {
                    Some(t) => format!("на связи {}", ago(now - t)),
                    None => "ещё не входил".to_owned(),
                }
            };
            format!("{} — отпечаток {} — {state}", r.name, r.fingerprint)
        })
        .collect();
    format!("Раннеры ({}):\n{}", list.len(), lines.join("\n"))
}

/// `/player`: без аргумента показывает игрока, с аргументом запоминает.
fn player_cmd(bot: &Bot, user: &User, args: &str) -> String {
    if args.is_empty() {
        return match (&user.player_steam, &user.player_name) {
            (None, None) => format!("Игрок не задан. {PLAYER_FORMAT}"),
            _ => format!("Игрок: {}", player_label(user)),
        };
    }
    let player = match valid_player(args) {
        Ok(p) => p,
        Err(why) => return format!("Не принял игрока: {why}. {PLAYER_FORMAT}"),
    };
    let steam = is_steam64(&player).then_some(player.as_str());
    match users::set_player(bot.core().db(), user.id, steam, &player) {
        Ok(()) => format!("Игрок: {player}"),
        Err(e) => format!("Не удалось сохранить игрока: {e}"),
    }
}

/// Telegram-ID: только десятичные цифры, положительное `i64`.
fn parse_tg_id(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().filter(|id| *id > 0)
}

fn adduser_cmd(bot: &Bot, args: &str) -> String {
    let (id_part, rest) = match args.split_once(char::is_whitespace) {
        Some((i, r)) => (i, r.trim()),
        None => (args, ""),
    };
    let Some(tg_id) = parse_tg_id(id_part) else {
        return ADDUSER_FORMAT.to_owned();
    };
    // Флаги вроде `--admin` не поддерживаются: админов заводит оператор на сервере.
    if rest.split_whitespace().any(|t| t.starts_with('-')) {
        return format!(
            "Флаги не поддерживаются, админа назначает оператор на сервере. {ADDUSER_FORMAT}"
        );
    }
    let name = if rest.is_empty() {
        format!("tg{tg_id}")
    } else {
        match valid_player(rest) {
            Ok(n) => n,
            Err(why) => return format!("Не принял имя: {why}. {ADDUSER_FORMAT}"),
        }
    };
    match users::add_member_by_tg(bot.core().db(), tg_id, &name, bot.now_ms()) {
        Ok((u, true)) => format!("Вернул доступ {} ({tg_id})", u.name),
        Ok((u, false)) => format!("Добавил {} ({tg_id})", u.name),
        Err(e) => format!("Не добавил: {e}"),
    }
}

fn removeuser_cmd(bot: &Bot, args: &str) -> String {
    let Some(tg_id) = parse_tg_id(args) else {
        return REMOVEUSER_FORMAT.to_owned();
    };
    let db = bot.core().db();
    let user = match users::remove_user(db, tg_id, bot.now_ms()) {
        Ok(u) => u,
        Err(e) => return format!("Не убрал: {e}"),
    };
    let tail = match users::active_jobs_of(db, user.id) {
        Ok(0) => "Активных задач у него нет.".to_owned(),
        Ok(n) => format!("Активных задач: {n} — они закончатся, ролики будут доставлены."),
        Err(e) => format!("Число активных задач узнать не удалось: {e}"),
    };
    format!(
        "Убрал {} ({tg_id}). Новые сообщения от него бот не принимает. {tail}",
        user.name
    )
}

fn users_cmd(bot: &Bot) -> String {
    let list = match users::list_users(bot.core().db()) {
        Ok(l) => l,
        Err(e) => return format!("Не удалось получить список: {e}"),
    };
    if list.is_empty() {
        return "Список пуст.".to_owned();
    }
    let lines: Vec<String> = list
        .iter()
        .map(|u| {
            let tg = u
                .tg_id
                .map_or_else(|| "без Telegram".to_owned(), |t| t.to_string());
            let state = if u.is_active() {
                "активен"
            } else {
                "убран"
            };
            format!(
                "{} — {tg} — {} — игрок: {} — {state}",
                u.name,
                u.role,
                player_label(u)
            )
        })
        .collect();
    format!("Пользователи ({}):\n{}", list.len(), lines.join("\n"))
}

/// Номер задачи из аргумента команды: только десятичные цифры, положительное `i64`.
fn parse_job_no(s: &str) -> Option<i64> {
    parse_tg_id(s)
}

/// Задача по аргументу команды или, если аргумента нет, самая свежая задача пользователя в `states`.
/// `Err` — готовый ответ пользователю.
fn target_job(
    bot: &Bot,
    user: &User,
    args: &str,
    states: &str,
    none_text: &str,
    format_text: &str,
) -> Result<crate::core::JobRow, String> {
    let id = if args.is_empty() {
        match store::user_job_ids(bot.core().db(), user.id, states, 1) {
            Ok(ids) => match ids.first() {
                Some(id) => *id,
                None => return Err(none_text.to_owned()),
            },
            Err(e) => return Err(format!("Не удалось получить задачи: {e}")),
        }
    } else {
        parse_job_no(args).ok_or_else(|| format_text.to_owned())?
    };
    match bot.core().job(id) {
        Ok(Some(job)) => Ok(job),
        // Несуществующий номер отвечает так же, как чужой: наличие чужих задач не раскрываем.
        Ok(None) => Err(NOT_YOURS.to_owned()),
        Err(e) => Err(format!("Не удалось получить задачу: {e}")),
    }
}

/// `/cancel [N]`: своя задача (админ — любая). В очереди отменяется сразу, у раннера — флагом; повторная отмена
/// жёсткая. Пустой ответ — сказать нечего: «отменена» уже говорит тик уведомлений.
fn cancel_cmd(bot: &Bot, user: &User, chat_id: i64, args: &str) -> String {
    let job = match target_job(
        bot,
        user,
        args,
        ACTIVE_STATES,
        "У вас нет активных задач — отменять нечего.",
        CANCEL_FORMAT,
    ) {
        Ok(j) => j,
        Err(text) => return text,
    };
    let id = job.id;
    if job.user_id != Some(user.id) && !is_admin(user) {
        return NOT_YOURS.to_owned();
    }
    if job.state.is_terminal() {
        return format!("Задача #{id} уже завершена, отменять нечего.");
    }
    let force = job.cancel_requested != CancelMode::NotRequested;
    match bot.core().cancel(id, force) {
        Ok(JobState::Cancelled) => cancelled_now(bot, id, chat_id),
        Ok(_) if force => format!("Останавливаю задачу #{id} жёстко — раннер оборвёт работу."),
        Ok(_) => format!("Отменяю задачу #{id} — раннер остановит запись."),
        Err(e) => format!("Не отменил задачу #{id}: {e}"),
    }
}

/// Задача отменена сразу (была в очереди). Если это чат её владельца, отчёт «отменена» занимаем здесь, чтобы тик
/// не прислал его вторым сообщением; чужую задачу (отмена админом) владельцу объявит тик.
fn cancelled_now(bot: &Bot, id: i64, chat_id: i64) -> String {
    let text = format!("Задача #{id} отменена.");
    let db = bot.core().db();
    let Ok(Some(tj)) = store::tg_job(db, id) else {
        return text;
    };
    if tj.chat_id != chat_id {
        return text;
    }
    match store::claim_final(db, id, bot.now_ms()) {
        Ok(true) => {
            notify::mark_status_cancelled(bot, &tj);
            text
        }
        // Тик успел раньше и уже сказал сам.
        Ok(false) => String::new(),
        Err(_) => text,
    }
}

/// `/retry [N]`: ставит свою упавшую или отменённую задачу заново по сохранённой демке (`getFile` не нужен) в
/// пределах тех же лимитов D-03. Успех отвечает сообщением статуса новой задачи, поэтому возвращает пустой ответ.
fn retry_cmd(bot: &Bot, user: &User, chat_id: i64, args: &str) -> String {
    let job = match target_job(
        bot,
        user,
        args,
        "('failed','cancelled')",
        "У вас нет упавших или отменённых задач — повторять нечего.",
        RETRY_FORMAT,
    ) {
        Ok(j) => j,
        Err(text) => return text,
    };
    let id = job.id;
    // Повтор занимает лимиты, поэтому и админ повторяет только свои задачи.
    if job.user_id != Some(user.id) {
        return NOT_YOURS.to_owned();
    }
    match job.state {
        JobState::Failed | JobState::Cancelled => {}
        JobState::Done => {
            return format!("Задача #{id} уже готова. Прислать ролик ещё раз: /resend {id}");
        }
        _ => return format!("Задача #{id} ещё идёт — повторять нечего."),
    }
    // Повтор не ставится второй раз: тот же матч или демка и тот же игрок уже в очереди или готовы (WR-03).
    let key = if job.source_kind == "faceit" {
        job.faceit_link.clone().map(|link| DedupKey::Faceit {
            link,
            player_key: player_key(&job.player_name),
        })
    } else {
        match bot.core().demo_file(id) {
            Ok(file) => file.map(|f| DedupKey::File {
                sha256: f.sha256,
                player_key: player_key(&job.player_name),
            }),
            Err(_) => None,
        }
    };
    if let Some(key) = key {
        match bot.core().find_duplicate_for(&key, user.id) {
            Ok(Some(dup)) => return ingest::duplicate_text(&dup, &job.player_name, user.id),
            Ok(None) => {}
            Err(e) => return format!("Не удалось проверить повтор задачи #{id}: {e}"),
        }
    }
    // Лимиты D-03 проверяются до постановки (T-07-34).
    match bot
        .core()
        .db()
        .write(|tx| Ok::<_, String>(ingest::admit(bot, tx, user)))
    {
        Ok(Ok(())) => {}
        Ok(Err(refusal)) => return refusal,
        Err(e) => return format!("Не удалось проверить лимиты: {e}"),
    }
    let new_id = match bot.core().resubmit(id, &user.name) {
        Ok(n) => n,
        Err(e) => return format!("Не удалось повторить задачу #{id}: {e}"),
    };
    let now = bot.now_ms();
    // Строка `tg_jobs` создаётся ДО любых вызовов Telegram: без неё доставка и уведомления задачу не видят, и
    // остановка процесса в окне сетевого вызова оставила бы готовый ролик без получателя (CR-02).
    let row = NewTgJob {
        job_id: new_id,
        chat_id,
        user_id: user.id,
        status_message_id: None,
        status_text: "",
        // Пока уходит подтверждение, тик не шлёт своё сообщение статуса.
        next_edit_ms: now + bot.settings().status_edit_ms,
    };
    if let Err(e) = store::insert_tg_job(bot.core().db(), &row, now) {
        // Без `tg_jobs` результат никому не придёт: задачу лучше не держать в очереди.
        if let Err(c) = bot.core().cancel(new_id, true) {
            bot.log().warn(&format!(
                "бот: не удалось снять задачу #{new_id} после сбоя записи: {c}"
            ));
        }
        bot.log().warn(&format!(
            "бот: не удалось записать задачу #{new_id} повтора: {e}"
        ));
        return format!("Не получилось поставить задачу заново: {e}. Попробуйте ещё раз.");
    }
    let ahead = bot.core().jobs_ahead(new_id).unwrap_or(0);
    let text = format!(
        "Задача #{new_id} — повтор задачи #{id}, {}.",
        ahead_phrase(ahead)
    );
    // Если сообщение не ушло, `status_message_id` остаётся пустым и тик пришлёт статус сам.
    if let Some(mid) = bot.say(chat_id, &text) {
        let at = bot.now_ms();
        if let Err(e) = store::set_status_message(
            bot.core().db(),
            new_id,
            mid,
            &text,
            at + bot.settings().status_edit_ms,
            at,
        ) {
            bot.log().warn(&format!(
                "бот: не удалось запомнить сообщение статуса задачи #{new_id}: {e}"
            ));
        }
    }
    bot.log().info(&format!(
        "бот: задача #{id} повторена как #{new_id} по сохранённой демке"
    ));
    String::new()
}

/// `/resend [N]`: прислать ролик своей готовой задачи ещё раз; без номера — последней готовой. Остальное решает
/// `deliver::resend`: владелец, состояние доставки, предупреждение о возможном дубле.
fn resend_cmd(bot: &Bot, user: &User, args: &str) -> String {
    let job = match target_job(
        bot,
        user,
        args,
        "('done')",
        "У вас нет готовых задач — отправлять нечего.",
        RESEND_FORMAT,
    ) {
        Ok(j) => j,
        Err(text) => return text,
    };
    deliver::resend(bot, user, job.id)
}

/// `/settings`: кнопка, открывающая экран настроек (Mini App). Пока у сервера нет публичного https-адреса
/// (`public_url`), кнопки нет — бот показывает текущие значения текстом и говорит, чего не хватает.
fn settings_cmd(bot: &Bot, user: &User, chat_id: i64) -> String {
    let values = match users::user_settings(bot.core().db(), user.id) {
        Ok(v) => v,
        Err(e) => return format!("Не смог прочитать настройки: {e}"),
    };
    let current = wire::settings::describe(&values).join("\n");
    let Some(base) = &bot.settings().public_url else {
        return format!(
            "Ваши настройки ролика:\n{current}\n\nЭкран с галками пока недоступен: администратору нужно задать public_url (https-адрес сервера)."
        );
    };
    let text = format!("Ваши настройки ролика:\n{current}\n\nИзменить можно на экране настроек — они применятся к новым задачам.");
    if let Err(e) = bot.tg().send_web_app_button(chat_id, &text, "Открыть настройки", &format!("{base}/app/settings")) {
        bot.log().warn(&format!("бот: не удалось отправить кнопку настроек: {e}"));
        return "Не удалось отправить кнопку настроек, попробуйте ещё раз.".to_owned();
    }
    String::new()
}

/// Выполняет команду и отвечает в чат.
pub fn handle(bot: &Bot, user: &User, chat_id: i64, text: &str) {
    let (cmd, args) = parse(text);
    let reply = match cmd.as_str() {
        "start" => start_text(user),
        "help" => help_text(user),
        "player" => player_cmd(bot, user, &args),
        "settings" => settings_cmd(bot, user, chat_id),
        "status" => notify::status_reply(bot, user),
        "queue" => notify::queue_reply(bot, user),
        "cancel" => cancel_cmd(bot, user, chat_id, &args),
        "retry" => retry_cmd(bot, user, chat_id, &args),
        "resend" => resend_cmd(bot, user, &args),
        "adduser" | "removeuser" | "users" | "runner" | "runners" if !is_admin(user) => {
            ADMIN_ONLY.to_owned()
        }
        "adduser" => adduser_cmd(bot, &args),
        "removeuser" => removeuser_cmd(bot, &args),
        "users" => users_cmd(bot),
        "runner" => runner_cmd(bot, &args),
        "runners" => runners_cmd(bot),
        _ => "Не знаю такой команды. Список команд и порядок работы: /help".to_owned(),
    };
    // Пустой ответ — команда уже ответила сама (сообщением статуса новой задачи) или тик скажет вместо неё.
    if !reply.is_empty() {
        bot.say(chat_id, &reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_split_from_arguments_and_bot_name() {
        assert_eq!(
            parse("/Player@my_bot  s1mple "),
            ("player".into(), "s1mple".into())
        );
        assert_eq!(parse("/help"), ("help".into(), String::new()));
    }

    #[test]
    fn player_name_is_checked_for_length_and_hidden_characters() {
        assert_eq!(valid_player("  s1mple "), Ok("s1mple".to_owned()));
        assert_eq!(valid_player("Иван Петров").unwrap(), "Иван Петров");
        assert!(valid_player("").is_err());
        assert!(valid_player(&"я".repeat(64)).is_ok());
        assert!(valid_player(&"я".repeat(65)).is_err());
        for bad in [
            "a\u{202E}b",
            "a\u{200F}b",
            "a\u{2066}b",
            "a\u{2028}b",
            "a\u{2029}b",
            "a\u{FEFF}b",
            "a\u{7}b",
            "a\nb",
        ] {
            assert!(valid_player(bad).is_err(), "{bad:?} должен отклоняться");
        }
    }

    #[test]
    fn ago_is_coarse_and_never_negative() {
        assert_eq!(ago(-5_000), "меньше минуты назад");
        assert_eq!(ago(59_000), "меньше минуты назад");
        assert_eq!(ago(5 * 60_000), "5 мин назад");
        assert_eq!(ago(3 * 3_600_000), "3 ч назад");
        assert_eq!(ago(2 * 86_400_000), "2 дн назад");
    }

    #[test]
    fn telegram_id_is_digits_only_and_positive() {
        assert_eq!(parse_tg_id("123"), Some(123));
        assert_eq!(parse_tg_id("+5"), None);
        assert_eq!(parse_tg_id("-5"), None);
        assert_eq!(parse_tg_id("0"), None);
        assert_eq!(parse_tg_id(""), None);
        assert_eq!(parse_tg_id("99999999999999999999"), None);
    }
}
