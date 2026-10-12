//! Тик уведомлений: отдаёт доставке задачи, которые ядро довело до `done` (`tg_jobs.delivery`: `none` → `pending`),
//! держит одно сообщение статуса на задачу, говорит «задача отменена» и присылает отчёты о сбоях: другу — короткую
//! фразу по коду, админам — код, этап, подсказку и число выдач раннеру (D-04).
//!
//! Сообщение статуса — то же, что подтверждение приёма (`tg_jobs.status_message_id`). Правка идёт только при
//! изменении текста и не чаще `status_edit_ms` (D-04); время берётся из часов ядра, поэтому тесты двигают его рукой.
//! Ошибка одного чата не прерывает остальные: `429` откладывает правку этого чата, `403` глушит его, пропавшее
//! сообщение заменяется новым (Pitfall 5). Мьютекс базы во время вызовов Telegram не держится.

use super::Bot;
use super::ingest::{ACTIVE_STATES, ahead_phrase};
use super::store::{self, TgJob};
use crate::Result;
use crate::core::JobRow;
use crate::tg::TgError;
use crate::users::{self, User};
use wire::{CancelMode, JobState};

/// Честная строка про раннер: бот не знает, когда он вернётся, и не обещает срока.
pub const OFFLINE_LINE: &str = "Раннер сейчас не в сети — задача подождёт его.";

/// Сколько недавно завершённых задач показывает `/status`.
const RECENT_FINISHED: i64 = 3;

/// Возвращает число задач, переведённых в ожидание доставки.
pub fn tick(bot: &Bot) -> Result<usize> {
    let promoted = store::promote_done(bot.core().db(), bot.now_ms())?;
    match store::status_candidates(bot.core().db()) {
        Ok(rows) => {
            for tj in rows {
                // Один чат не должен останавливать остальные.
                if let Err(e) = update_status(bot, &tj) {
                    bot.log().warn(&format!(
                        "бот: статус задачи #{} не обновлён: {}",
                        tj.job_id,
                        bot.core().redactor().clean(&e, 200)
                    ));
                }
            }
        }
        Err(e) => bot
            .log()
            .warn(&format!("бот: не прочитал задачи для статусов: {e}")),
    }
    match store::failure_candidates(bot.core().db()) {
        Ok(rows) => {
            for tj in rows {
                report_failed(bot, &tj);
            }
        }
        Err(e) => bot
            .log()
            .warn(&format!("бот: не прочитал упавшие задачи: {e}")),
    }
    Ok(promoted)
}

/// Этап задачи по-русски: из `jobs.phase`, а пустой или незнакомый — по состоянию задачи.
fn stage_title(job: &JobRow) -> &'static str {
    match job.phase.as_str() {
        "fetch" => "получение демки",
        "precheck" => "проверка демки",
        "download" => "скачивание демки",
        "parse" => "разбор демки",
        "positions" | "geometry" | "plan" => "подготовка камер",
        "record" => "запись",
        "assemble" => "склейка",
        "upload" => "отправка ролика на сервер",
        _ if job.state == JobState::Assigned => "выдана раннеру",
        _ => "в работе",
    }
}

/// «, дубль k из n», если раннер сообщил номер дубля.
fn take_part(job: &JobRow) -> String {
    match (job.take_n, job.take_m) {
        (Some(n), Some(m)) if n > 0 && m > 0 => format!(", дубль {n} из {m}"),
        _ => String::new(),
    }
}

fn is_active(job: &JobRow) -> bool {
    matches!(
        job.state,
        JobState::Receiving
            | JobState::Queued
            | JobState::Assigned
            | JobState::Running
            | JobState::Uploading
    )
}

fn is_working(job: &JobRow) -> bool {
    matches!(
        job.state,
        JobState::Assigned | JobState::Running | JobState::Uploading
    )
}

/// Раннер не в сети. Ошибка чтения считается «в сети»: ложное «не в сети» хуже молчания.
fn runner_offline(bot: &Bot) -> bool {
    matches!(bot.core().runner_online(), Ok(false))
}

/// Текст сообщения статуса задачи.
pub fn status_text(bot: &Bot, job: &JobRow) -> String {
    let id = job.id;
    let player = bot.core().redactor().clean(&job.player_name, 64);
    let mut text = match job.state {
        JobState::Queued => {
            let ahead = bot.core().jobs_ahead(id).unwrap_or(0);
            format!(
                "Задача #{id} в очереди (игрок {player}), {}.",
                ahead_phrase(ahead)
            )
        }
        JobState::Receiving => format!("Задача #{id} (игрок {player}): принимается демка"),
        JobState::Assigned | JobState::Running | JobState::Uploading => format!(
            "Задача #{id} (игрок {player}): {}{}",
            stage_title(job),
            take_part(job)
        ),
        JobState::Done => format!("Задача #{id} (игрок {player}): готова"),
        JobState::Failed => format!("Задача #{id} (игрок {player}): завершилась сбоем"),
        JobState::Cancelled => format!("Задача #{id} отменена"),
        JobState::Unknown => format!("Задача #{id} (игрок {player})"),
    };
    if is_active(job) {
        if job.cancel_requested != CancelMode::NotRequested {
            text.push_str("\nОтмена запрошена — жду остановки раннера.");
        }
        if runner_offline(bot) {
            text.push('\n');
            text.push_str(OFFLINE_LINE);
        }
    }
    text
}

/// Что делать после неудачи вызова Telegram по сообщению статуса: `403` глушит чат, `429` откладывает правку на
/// `retry_after`, прочее — обычный интервал.
fn on_error(bot: &Bot, tj: &TgJob, e: &TgError, now: i64) {
    let db = bot.core().db();
    if e.is_forbidden() {
        bot.log().warn(&format!(
            "бот: чат задачи #{} закрыт для бота (403) — статусы в него прекращены",
            tj.job_id
        ));
        if let Err(err) = store::mute_chat(db, tj.chat_id, now) {
            bot.log()
                .warn(&format!("бот: не удалось заглушить чат: {err}"));
        }
        return;
    }
    let wait = e.retry_after_ms().unwrap_or(bot.settings().status_edit_ms);
    bot.log().warn(&format!(
        "бот: статус задачи #{} не доставлен: {}",
        tj.job_id,
        bot.core().redactor().clean(&e.to_string(), 200)
    ));
    if let Err(err) = store::defer_edit(db, tj.job_id, now + wait, now) {
        bot.log()
            .warn(&format!("бот: не удалось отложить правку: {err}"));
    }
}

fn warn_db(bot: &Bot, r: Result<()>) {
    if let Err(e) = r {
        bot.log()
            .warn(&format!("бот: не удалось записать статус: {e}"));
    }
}

/// Новое сообщение статуса (прежнего нет или оно пропало).
fn send_new(bot: &Bot, tj: &TgJob, text: &str, now: i64) {
    let next = now + bot.settings().status_edit_ms;
    match bot.tg().send_message(tj.chat_id, text) {
        Ok(mid) => warn_db(
            bot,
            store::set_status_message(bot.core().db(), tj.job_id, mid, text, next, now),
        ),
        Err(e) => on_error(bot, tj, &e, now),
    }
}

/// Показывает `text` в чате: правит сообщение статуса или, если его нет, шлёт новое. `text` уже очищен.
fn push_status(bot: &Bot, tj: &TgJob, text: &str, now: i64) {
    let next = now + bot.settings().status_edit_ms;
    let Some(mid) = tj.status_message_id else {
        return send_new(bot, tj, text, now);
    };
    match bot.tg().edit_message_text(tj.chat_id, mid, text) {
        Ok(()) => warn_db(
            bot,
            store::note_status(bot.core().db(), tj.job_id, text, next, now),
        ),
        // Текст в Telegram уже такой: это успех.
        Err(e) if e.is_not_modified() => warn_db(
            bot,
            store::note_status(bot.core().db(), tj.job_id, text, next, now),
        ),
        Err(e) if e.is_message_missing() => send_new(bot, tj, text, now),
        Err(e) => on_error(bot, tj, &e, now),
    }
}

/// Один шаг по строке `tg_jobs`: отмена — финальное сообщение, иначе правка статуса, если текст изменился и пора.
fn update_status(bot: &Bot, tj: &TgJob) -> Result<()> {
    let Some(job) = bot.core().job(tj.job_id)? else {
        return Ok(());
    };
    if job.state == JobState::Cancelled {
        report_cancelled(bot, tj);
        return Ok(());
    }
    let now = bot.now_ms();
    let text = bot.core().redactor().clean(&status_text(bot, &job), 4000);
    if tj.status_message_id.is_some() && text == tj.status_text {
        return Ok(());
    }
    if now < tj.next_edit_ms {
        return Ok(());
    }
    push_status(bot, tj, &text, now);
    Ok(())
}

/// Первое сообщение статуса сразу после постановки задачи: правка подтверждения приёма, учитываемая в частоте.
pub(super) fn push_now(bot: &Bot, job_id: i64) {
    let run = || -> Result<()> {
        let Some(tj) = store::tg_job(bot.core().db(), job_id)? else {
            return Ok(());
        };
        let Some(job) = bot.core().job(job_id)? else {
            return Ok(());
        };
        if tj.muted {
            return Ok(());
        }
        let text = bot.core().redactor().clean(&status_text(bot, &job), 4000);
        push_status(bot, &tj, &text, bot.now_ms());
        Ok(())
    };
    if let Err(e) = run() {
        bot.log().warn(&format!(
            "бот: статус задачи #{job_id} не обновлён: {}",
            bot.core().redactor().clean(&e, 200)
        ));
    }
}

/// Задача отменена: новое сообщение «Задача #N отменена.» ровно один раз (отметка `final_reported_ms`), а
/// сообщение статуса правится на «Задача #N отменена».
fn report_cancelled(bot: &Bot, tj: &TgJob) {
    let db = bot.core().db();
    let now = bot.now_ms();
    match store::claim_final(db, tj.job_id, now) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            bot.log()
                .warn(&format!("бот: не удалось занять отчёт об отмене: {e}"));
            return;
        }
    }
    match bot
        .tg()
        .send_message(tj.chat_id, &format!("Задача #{} отменена.", tj.job_id))
    {
        Ok(_) => {}
        Err(e) if e.is_forbidden() => {
            on_error(bot, tj, &e, now);
            return;
        }
        Err(e) => {
            // Не дошло точно — повторим на следующем тике; могло дойти — не дублируем.
            if !e.maybe_delivered() {
                let _ = store::unclaim_final(db, tj.job_id);
            }
            bot.log().warn(&format!(
                "бот: сообщение об отмене задачи #{} не отправлено: {}",
                tj.job_id,
                bot.core().redactor().clean(&e.to_string(), 200)
            ));
            return;
        }
    }
    mark_status_cancelled(bot, tj);
}

/// Правит сообщение статуса на «Задача #N отменена» (без ожидания интервала: это конец истории задачи).
pub fn mark_status_cancelled(bot: &Bot, tj: &TgJob) {
    mark_status_final(bot, tj, &format!("Задача #{} отменена", tj.job_id));
}

/// Последняя правка сообщения статуса (отмена, сбой): без ожидания интервала, это конец истории задачи.
fn mark_status_final(bot: &Bot, tj: &TgJob, text: &str) {
    let Some(mid) = tj.status_message_id else {
        return;
    };
    match bot.tg().edit_message_text(tj.chat_id, mid, text) {
        Ok(()) => {}
        Err(e) if e.is_not_modified() || e.is_message_missing() => {}
        Err(e) => on_error(bot, tj, &e, bot.now_ms()),
    }
}

/// Общая фраза для кода сбоя, которого нет в таблице (в том числе `UNKNOWN`).
const GENERIC_PHRASE: &str = "что-то пошло не так; автор уже получил подробности";

/// Фраза для друга по коду сбоя. У каждого кода `wire::fail::KNOWN` своя строка; пути, коды и тексты ошибок
/// раннера в ней недопустимы (T-07-36): друг видит только причину по-человечески и что делать.
pub fn phrase_for(code: &str) -> Option<&'static str> {
    use wire::fail;
    Some(match code {
        // Запись на машине автора: друг ничего исправить не может, задачу можно повторить.
        fail::LAUNCHER_FAILED => {
            "на машине автора не запустился лаунчер записи; автор уже знает, задачу можно повторить"
        }
        fail::GAME_NOT_STARTED => {
            "на машине автора не запустилась игра; автор уже знает, задачу можно повторить"
        }
        fail::CONSOLE_NOT_OPEN => {
            "игра запустилась, но консоль не открылась; сбой записи на машине автора, задачу можно повторить"
        }
        fail::DEMO_NOT_LOADED => {
            "игра не смогла открыть демку; сбой записи на машине автора, задачу можно повторить"
        }
        fail::SCRIPT_NOT_RUN => {
            "сценарий записи не запустился в игре; сбой записи на машине автора, задачу можно повторить"
        }
        fail::TAKE_TIMEOUT => {
            "один из дублей записывался слишком долго; сбой записи на машине автора, задачу можно повторить"
        }
        fail::GAME_EXITED => {
            "игра закрылась посреди записи; сбой записи на машине автора, задачу можно повторить"
        }
        fail::HLAE_ERROR => {
            "сбой программы записи на машине автора; автор уже знает, задачу можно повторить"
        }
        fail::PRECHECK_FAILED => {
            "проверка окружения на машине автора не прошла; автор уже знает, задачу можно повторить позже"
        }
        fail::FOREIGN_GAME_RUNNING => {
            "на машине автора сейчас идёт своя игра, запись ей не мешает и подождёт; повторите задачу позже"
        }
        fail::PREV_GAME_STUCK => {
            "на машине автора зависла предыдущая игра; автор уже знает, задачу можно повторить"
        }
        fail::TAKE_INVALID => {
            "записанный дубль оказался повреждённым; сбой записи на машине автора, задачу можно повторить"
        }
        fail::RETRY_NO_DATA => {
            "для повтора на машине автора не осталось записанных данных; пришлите демку заново"
        }
        fail::RESULT_INVALID => {
            "готовый ролик не прошёл проверку; автор уже знает, задачу можно повторить"
        }
        fail::JOB_ERROR => {
            "на машине автора задача упала неожиданно; автор уже получил подробности"
        }
        fail::UNSUPPORTED_SOURCE => {
            "раннер автора пока не умеет брать такой источник демки; пришлите файл демки"
        }
        // Причина во входных данных: другу есть что поправить.
        fail::PLAYER_NOT_FOUND => {
            "игрока с таким ником нет в этой демке — проверьте ник (/player) и повторите"
        }
        fail::PLAYER_AMBIGUOUS => {
            "ник подходит нескольким игрокам — укажите его точнее или SteamID64"
        }
        fail::NO_HIGHLIGHTS => "у игрока в этом матче нет моментов для ролика",
        fail::BAD_DEMO => "демка повреждена или это не демка CS2",
        fail::DEMO_TOO_LARGE => "демка слишком большая — пришлите демку одной карты",
        fail::FACEIT_FETCH_FAILED => {
            "не удалось скачать демку с FACEIT — матч мог ещё не закончиться; пришлите файл или повторите позже"
        }
        // Сбои сервера.
        fail::TOO_MANY_GRANTS => {
            "задача несколько раз срывалась на машине автора — похоже, демка мешает записи"
        }
        fail::RECEIVE_FAILED => "сервер не смог сохранить демку — пришлите её ещё раз",
        fail::CANCELLED => "задача отменена",
        _ => return None,
    })
}

/// Фраза для друга: из таблицы, а для неизвестного кода (и `UNKNOWN`) общая.
pub fn friend_phrase(code: &str) -> &'static str {
    phrase_for(code).unwrap_or(GENERIC_PHRASE)
}

/// Сообщение другу о сбое: короткая фраза без путей и кода и подсказка, как повторить.
fn failure_text_for_friend(job: &JobRow) -> String {
    let phrase = friend_phrase(job.fail_code.as_deref().unwrap_or(""));
    format!(
        "Задача #{id} не получилась: {phrase}. Повторить: /retry {id}",
        id = job.id
    )
}

/// Подробный отчёт админу: код, этап, подсказка, число выдач раннеру и сообщение раннера. Всё, что пришло от
/// раннера, проходит `Redactor::clean`; хвост консоли в чат не идёт, он остаётся в `job show`.
fn failure_text_for_admin(bot: &Bot, job: &JobRow) -> String {
    let red = bot.core().redactor();
    let or_dash = |v: &Option<String>, max: usize| match v.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => red.clean(s, max),
        _ => "—".to_owned(),
    };
    let phase = match job.phase.trim() {
        "" => String::new(),
        p => format!(", фаза {}", red.clean(p, 40)),
    };
    let text = format!(
        "Сбой задачи #{id} (пользователь {user}, игрок {player}): код {code}; этап {state}{phase}; \
         подсказка: {hint}; выдач раннеру: {grants}; сообщение: {message}",
        id = job.id,
        user = red.clean(&job.requested_by, 64),
        player = red.clean(&job.player_name, 64),
        code = or_dash(&job.fail_code, 64),
        state = or_dash(&job.fail_state, 64),
        hint = or_dash(&job.fail_hint, 300),
        grants = job.grants,
        message = or_dash(&job.fail_message, 500),
    );
    red.clean(&text, 4000)
}

/// Отчёты о сбое задачи: другу новым сообщением (и правка сообщения статуса) и каждому админу отдельным
/// сообщением. Право на каждое занимает своя отметка (`final_reported_ms`, `admin_reported_ms`), поэтому
/// повторный тик ничего не дублирует.
fn report_failed(bot: &Bot, tj: &TgJob) {
    let job = match bot.core().job(tj.job_id) {
        Ok(Some(j)) if j.state == JobState::Failed => j,
        _ => return,
    };
    if tj.final_reported_ms.is_none() && !tj.muted {
        tell_friend_failure(bot, tj, &job);
    }
    if tj.admin_reported_ms.is_none() {
        tell_admins_failure(bot, tj, &job);
    }
}

fn tell_friend_failure(bot: &Bot, tj: &TgJob, job: &JobRow) {
    let db = bot.core().db();
    let now = bot.now_ms();
    match store::claim_final(db, tj.job_id, now) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            bot.log()
                .warn(&format!("бот: не удалось занять отчёт о сбое: {e}"));
            return;
        }
    }
    let text = bot
        .core()
        .redactor()
        .clean(&failure_text_for_friend(job), 4000);
    match bot.tg().send_message(tj.chat_id, &text) {
        Ok(_) => mark_status_final(bot, tj, &format!("Задача #{}: сбой", tj.job_id)),
        Err(e) if e.is_forbidden() => on_error(bot, tj, &e, now),
        Err(e) => {
            // Не дошло точно — повторим на следующем тике; могло дойти — не дублируем.
            if !e.maybe_delivered() {
                let _ = store::unclaim_final(db, tj.job_id);
            }
            bot.log().warn(&format!(
                "бот: сообщение о сбое задачи #{} не отправлено: {}",
                tj.job_id,
                bot.core().redactor().clean(&e.to_string(), 200)
            ));
        }
    }
}

fn tell_admins_failure(bot: &Bot, tj: &TgJob, job: &JobRow) {
    let db = bot.core().db();
    match store::claim_admin_report(db, tj.job_id, bot.now_ms()) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            bot.log()
                .warn(&format!("бот: не удалось занять отчёт админам: {e}"));
            return;
        }
    }
    let admins = match users::list_users(db) {
        Ok(list) => list,
        Err(e) => {
            // Отчёт не отправлен: право на него возвращается, тик повторит (WR-09).
            let _ = store::unclaim_admin_report(db, tj.job_id);
            bot.log()
                .warn(&format!("бот: не прочитал список админов: {e}"));
            return;
        }
    };
    let text = failure_text_for_admin(bot, job);
    let mut recipients = 0;
    // Сообщение дошло или могло дойти, либо адресат закрыт: повторять нечего.
    let mut settled = 0;
    for admin in admins.iter().filter(|u| u.role == "admin" && u.is_active()) {
        let Some(chat) = admin.tg_id else { continue };
        recipients += 1;
        // Ошибка одного админа не мешает остальным; в журнал — только очищенный текст.
        match bot.tg().send_message(chat, &text) {
            Ok(_) => settled += 1,
            Err(e) => {
                if e.maybe_delivered() || e.is_forbidden() {
                    settled += 1;
                }
                bot.log().warn(&format!(
                    "бот: отчёт о сбое задачи #{} админу не отправлен: {}",
                    tj.job_id,
                    bot.core().redactor().clean(&e.to_string(), 200)
                ));
            }
        }
    }
    if recipients == 0 {
        // Повтор ничего не изменит, пока оператор не заведёт админа с Telegram-id: отметка остаётся.
        bot.log().warn(&format!(
            "бот: отчёт о сбое задачи #{} некому отправить: нет активного админа с Telegram-id",
            tj.job_id
        ));
    } else if settled == 0 {
        // Ни одно сообщение точно не ушло (сеть, 429): отчёт не теряем, тик повторит (WR-09).
        let _ = store::unclaim_admin_report(db, tj.job_id);
    }
}

/// Строка задачи для `/status`.
fn status_line(bot: &Bot, job: &JobRow) -> String {
    let id = job.id;
    let player = bot.core().redactor().clean(&job.player_name, 64);
    let mut line = match job.state {
        JobState::Queued => {
            let ahead = bot.core().jobs_ahead(id).unwrap_or(0);
            format!("#{id} (игрок {player}): в очереди, перед вами {ahead}")
        }
        JobState::Receiving => format!("#{id} (игрок {player}): принимается демка"),
        JobState::Assigned | JobState::Running | JobState::Uploading => format!(
            "#{id} (игрок {player}): {}{}",
            stage_title(job),
            take_part(job)
        ),
        JobState::Done => format!("#{id} (игрок {player}): готова"),
        JobState::Failed => format!("#{id} (игрок {player}): сбой — повторить: /retry {id}"),
        JobState::Cancelled => format!("#{id} (игрок {player}): отменена — повторить: /retry {id}"),
        JobState::Unknown => format!("#{id} (игрок {player})"),
    };
    if is_active(job) && job.cancel_requested != CancelMode::NotRequested {
        line.push_str(" (отмена запрошена)");
    }
    line
}

/// Задачи пользователя из списка номеров; пропавшая строка пропускается.
fn jobs_of(bot: &Bot, ids: Vec<i64>) -> Vec<JobRow> {
    ids.into_iter()
        .filter_map(|id| bot.core().job(id).ok().flatten())
        .collect()
}

/// Активные задачи пользователя по возрастанию номера.
fn active_jobs(bot: &Bot, user: &User) -> Result<Vec<JobRow>> {
    let mut ids = store::user_job_ids(bot.core().db(), user.id, ACTIVE_STATES, 100)?;
    ids.reverse();
    Ok(jobs_of(bot, ids))
}

/// Ответ на `/status`: только задачи этого пользователя — состояние, этап, дубль, сколько впереди.
pub fn status_reply(bot: &Bot, user: &User) -> String {
    let active = match active_jobs(bot, user) {
        Ok(a) => a,
        Err(e) => return format!("Не удалось получить задачи: {e}"),
    };
    let finished = store::user_job_ids(
        bot.core().db(),
        user.id,
        "('done','failed','cancelled')",
        RECENT_FINISHED,
    )
    .map(|ids| jobs_of(bot, ids))
    .unwrap_or_default();
    let mut lines = Vec::new();
    if active.is_empty() {
        lines.push("У вас нет активных задач.".to_owned());
    } else {
        lines.push("Ваши задачи:".to_owned());
        lines.extend(active.iter().map(|j| status_line(bot, j)));
    }
    if !finished.is_empty() {
        lines.push("Недавние:".to_owned());
        lines.extend(finished.iter().map(|j| status_line(bot, j)));
    }
    if !active.is_empty() && runner_offline(bot) {
        lines.push(OFFLINE_LINE.to_owned());
    }
    lines.join("\n")
}

/// Ответ на `/queue`: сколько задач в общей очереди и в работе и места задач этого пользователя.
pub fn queue_reply(bot: &Bot, user: &User) -> String {
    let counts = match bot.core().active_counts(user.id) {
        Ok(c) => c,
        Err(e) => return format!("Не удалось получить очередь: {e}"),
    };
    let working = store::working_count(bot.core().db()).unwrap_or(0);
    let waiting = (counts.total - working).max(0);
    let mut lines = vec![format!("В очереди {waiting}, в работе {working}.")];
    match active_jobs(bot, user) {
        Ok(mine) if mine.is_empty() => lines.push("У вас нет активных задач.".to_owned()),
        Ok(mine) => {
            lines.push("Ваши места:".to_owned());
            for job in mine {
                let id = job.id;
                lines.push(if is_working(&job) {
                    format!("#{id} — в работе")
                } else {
                    let ahead = bot.core().jobs_ahead(id).unwrap_or(0);
                    format!("#{id} — впереди {ahead}")
                });
            }
        }
        Err(e) => lines.push(format!("Ваши задачи узнать не удалось: {e}")),
    }
    if runner_offline(bot) {
        lines.push(OFFLINE_LINE.to_owned());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_code_has_a_friend_phrase_without_paths() {
        const FORBIDDEN: [&str; 9] = [
            "\\", ":\\", "/home", "/var", "/tmp", "/mnt", "/srv", "Users", "AppData",
        ];
        for code in wire::fail::KNOWN {
            let phrase = phrase_for(code).unwrap_or_else(|| panic!("нет фразы для {code}"));
            assert!(
                phrase.chars().count() <= 300,
                "{code}: фраза длиннее 300 символов"
            );
            for bad in FORBIDDEN.iter().copied().chain([".dem", *code]) {
                assert!(!phrase.contains(bad), "{code}: во фразе {bad:?}: {phrase}");
            }
            assert!(!phrase.trim().is_empty(), "{code}: пустая фраза");
        }
        assert_eq!(friend_phrase("UNKNOWN"), GENERIC_PHRASE);
        assert_eq!(friend_phrase("XYZ"), GENERIC_PHRASE);
        assert_eq!(friend_phrase(""), GENERIC_PHRASE);
        assert_ne!(friend_phrase("BAD_DEMO"), GENERIC_PHRASE);
    }
}
