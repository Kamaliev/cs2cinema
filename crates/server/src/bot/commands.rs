//! Команды бота: `/start`, `/help`, `/player` и управление белым списком (`/adduser`, `/removeuser`, `/users`).
//! Команды статуса, отмены и повтора добавляют следующие планы фазы.
//!
//! Роль берётся только из базы (`User::role`). Команды админа проверяют её до любого изменения; `/adduser`
//! создаёт только участника, поэтому повысить права через чат нельзя.

use super::Bot;
use crate::core::is_steam64;
use crate::users::{self, User};

const ADMIN_ONLY: &str = "Эта команда только для администратора.";
const ADDUSER_FORMAT: &str =
    "Формат: /adduser Telegram-ID [имя]. Например: /adduser 123456789 петя";
const REMOVEUSER_FORMAT: &str = "Формат: /removeuser Telegram-ID. Например: /removeuser 123456789";
const PLAYER_FORMAT: &str = "Формат: /player ник или SteamID64 (17 цифр). Например: /player s1mple";

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
        ("adduser", "Добавить друга (админ)"),
        ("removeuser", "Убрать друга (админ)"),
        ("users", "Список друзей (админ)"),
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
         /start — как пользоваться\n\
         /help — эта справка",
    );
    if is_admin(user) {
        t.push_str(
            "\n\nУправление списком:\n\
             /adduser Telegram-ID [имя] — добавить друга\n\
             /removeuser Telegram-ID — убрать друга\n\
             /users — показать список",
        );
    }
    t
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

/// Выполняет команду и отвечает в чат.
pub fn handle(bot: &Bot, user: &User, chat_id: i64, text: &str) {
    let (cmd, args) = parse(text);
    let reply = match cmd.as_str() {
        "start" => start_text(user),
        "help" => help_text(user),
        "player" => player_cmd(bot, user, &args),
        "adduser" | "removeuser" | "users" if !is_admin(user) => ADMIN_ONLY.to_owned(),
        "adduser" => adduser_cmd(bot, &args),
        "removeuser" => removeuser_cmd(bot, &args),
        "users" => users_cmd(bot),
        _ => "Не знаю такой команды. Список команд и порядок работы: /help".to_owned(),
    };
    bot.say(chat_id, &reply);
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
    fn telegram_id_is_digits_only_and_positive() {
        assert_eq!(parse_tg_id("123"), Some(123));
        assert_eq!(parse_tg_id("+5"), None);
        assert_eq!(parse_tg_id("-5"), None);
        assert_eq!(parse_tg_id("0"), None);
        assert_eq!(parse_tg_id(""), None);
        assert_eq!(parse_tg_id("99999999999999999999"), None);
    }
}
