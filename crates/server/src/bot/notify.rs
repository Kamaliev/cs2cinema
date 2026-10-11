//! Тик уведомлений: отдаёт доставке задачи, которые ядро довело до `done` (`tg_jobs.delivery`: `none` → `pending`),
//! держит одно сообщение статуса на задачу и говорит «задача отменена». Отчёты о сбоях — план 07-09.
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
use crate::users::User;
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
    let Some(mid) = tj.status_message_id else {
        return;
    };
    let text = format!("Задача #{} отменена", tj.job_id);
    match bot.tg().edit_message_text(tj.chat_id, mid, &text) {
        Ok(()) => {}
        Err(e) if e.is_not_modified() || e.is_message_missing() => {}
        Err(e) => on_error(bot, tj, &e, bot.now_ms()),
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
