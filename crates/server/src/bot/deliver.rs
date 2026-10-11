//! Доставка готового ролика новым сообщением. Состояния `tg_jobs.delivery`:
//! `pending` → `sending` (пишется ДО вызова Telegram) → `sent` | `uncertain` | `failed`.
//!
//! Слепого повтора нет: если запрос мог дойти (тайм-аут или обрыв после начала отправки), доставка
//! `uncertain`, и решает человек. После перезапуска `sending` тоже становится `uncertain` (`Bot::recover`).

use super::store::{self, TgJob};
use super::{Bot, SendAs};
use crate::Result;
use crate::tg::{InputFile, Sent, TgError};
use crate::users::User;
use std::path::Path;
use wire::JobState;

const NOT_YOURS: &str = "Это не ваша задача.";

/// URI `file:///…` для процесса `telegram-bot-api`: `\` → `/`, у пути без начального `/` (диск Windows)
/// добавляется `/`, всё вне `[A-Za-z0-9-_.~/:]` кодируется как `%XX` (остаток после `file:/` декодируется).
pub fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/');
    }
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Отправляет один ролик из `pending`, которому пора. `false` — отправлять нечего.
pub fn deliver_once(bot: &Bot) -> Result<bool> {
    let db = bot.core().db();
    let now = bot.now_ms();
    let Some(tj) = store::next_pending_delivery(db, now)? else {
        return Ok(false);
    };
    let id = tj.job_id;
    let on_disk = bot
        .core()
        .data_dir()
        .join("blobs")
        .join("results")
        .join(id.to_string())
        .join("highlights.mp4");
    if bot.core().result_file(id)?.is_none() || !on_disk.is_file() {
        store::mark_delivery(
            db,
            id,
            "failed",
            "файл результата не найден на сервере",
            now,
        )?;
        bot.log().warn(&format!(
            "бот: у задачи #{id} нет файла результата для отправки"
        ));
        return Ok(true);
    }
    let player = bot
        .core()
        .job(id)?
        .map(|j| j.player_name)
        .unwrap_or_default();
    if !store::begin_sending(db, id, now)? {
        return Ok(true);
    }
    // Путь строится только из числового id и префикса оператора.
    let uri = file_uri(
        &bot.settings()
            .results_prefix
            .join(id.to_string())
            .join("highlights.mp4"),
    );
    let video = InputFile::Local(uri);
    let caption = caption_for(bot, id, &player);
    let sent = send(bot, tj.chat_id, &video, &caption);
    let done_at = bot.now_ms();
    match sent {
        Ok(s) => {
            store::mark_sent(db, id, s.message_id, s.file_id.as_deref(), done_at)?;
            bot.log()
                .info(&format!("бот: ролик задачи #{id} отправлен"));
        }
        Err(e) => {
            let state = if e.maybe_delivered() {
                "uncertain"
            } else {
                "failed"
            };
            let text = bot.core().redactor().clean(&e.to_string(), 500);
            store::mark_delivery(db, id, state, &text, done_at)?;
            bot.log().warn(&format!(
                "бот: отправка ролика задачи #{id} — {state}: {text}"
            ));
            // Запрос мог дойти: слепого повтора нет, решает друг (T-07-35).
            if e.maybe_delivered() {
                tell(
                    bot,
                    &tj,
                    &format!(
                        "Не уверен, что ролик задачи #{id} дошёл. Если его нет в чате — отправьте /resend {id}"
                    ),
                );
            }
        }
    }
    Ok(true)
}

/// Подпись под роликом: номер задачи и игрок.
fn caption_for(bot: &Bot, id: i64, player: &str) -> String {
    bot.core()
        .redactor()
        .clean(&format!("Задача #{id} — игрок {player}"), 1000)
}

/// Отправка роликом или документом по настройке оператора.
fn send(
    bot: &Bot,
    chat_id: i64,
    file: &InputFile,
    caption: &str,
) -> std::result::Result<Sent, TgError> {
    match bot.settings().send_as {
        SendAs::Video => bot.tg().send_video(chat_id, file, caption),
        SendAs::Document => bot.tg().send_document(chat_id, file, caption),
    }
}

/// Одно сообщение другу про исход доставки. Право его отправить занимает отметка `final_reported_ms` (как у
/// «отменена»): повторный вызов молчит. Если сообщение точно не ушло, отметка снимается и можно повторить;
/// заглушённому чату не пишем, `403` глушит его.
fn tell(bot: &Bot, tj: &TgJob, text: &str) {
    if tj.muted {
        return;
    }
    let db = bot.core().db();
    match store::claim_final(db, tj.job_id, bot.now_ms()) {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            bot.log()
                .warn(&format!("бот: не удалось занять сообщение о доставке: {e}"));
            return;
        }
    }
    let clean = bot.core().redactor().clean(text, 4000);
    match bot.tg().send_message(tj.chat_id, &clean) {
        Ok(_) => {}
        Err(e) => {
            if e.is_forbidden() {
                let _ = store::mute_chat(db, tj.chat_id, bot.now_ms());
            } else if !e.maybe_delivered() {
                let _ = store::unclaim_final(db, tj.job_id);
            }
            bot.log().warn(&format!(
                "бот: сообщение о доставке задачи #{} не отправлено: {}",
                tj.job_id,
                bot.core().redactor().clean(&e.to_string(), 200)
            ));
        }
    }
}

/// `/resend N`: отправляет ролик своей готовой задачи ещё раз и возвращает ответ для чата (пустой — отвечать
/// нечего, сам ролик и есть ответ).
///
/// - доставленный (`sent`) уходит сразу по `file_id` Telegram, без повторной загрузки файла (вызов быстрый, поэтому
///   его можно сделать в потоке опроса);
/// - неясный или не ушедший (`uncertain`, `failed`) возвращается в `pending` с нуля, и отправку с файла делает
///   `deliver_once` вне потока опроса; дубль возможен, о чём сказано заранее;
/// - ещё отправляемый ролик не трогаем.
pub fn resend(bot: &Bot, user: &User, job_id: i64) -> String {
    let job = match bot.core().job(job_id) {
        Ok(Some(j)) => j,
        // Несуществующий номер отвечает так же, как чужой: наличие чужих задач не раскрываем.
        Ok(None) => return NOT_YOURS.to_owned(),
        Err(e) => return format!("Не удалось получить задачу: {e}"),
    };
    let id = job.id;
    if job.user_id != Some(user.id) {
        return NOT_YOURS.to_owned();
    }
    match job.state {
        JobState::Done => {}
        JobState::Failed | JobState::Cancelled => {
            return format!("У задачи #{id} нет ролика. Повторить задачу: /retry {id}");
        }
        _ => return format!("Ролик задачи #{id} ещё не готов."),
    }
    let tj = match store::tg_job(bot.core().db(), id) {
        Ok(Some(tj)) => tj,
        Ok(None) => {
            return format!("Задача #{id} поставлена не через бота — ролик отсюда не отправить.");
        }
        Err(e) => return format!("Не удалось получить доставку: {e}"),
    };
    match tj.delivery.as_str() {
        "sent" => {
            if let Some(file_id) = tj.result_file_id.clone() {
                match resend_by_id(bot, &tj, &job.player_name, &file_id) {
                    ByIdOutcome::Reply(text) => return text,
                    // Telegram не принял `file_id`: отправляем с файла.
                    ByIdOutcome::FromFile => {}
                }
            }
        }
        "uncertain" | "failed" => {}
        _ => return format!("Ролик задачи #{id} уже отправляется."),
    }
    match store::requeue_delivery(bot.core().db(), id, bot.now_ms()) {
        Ok(true) => {
            format!("Отправляю ещё раз; если ролик уже пришёл, дубль — нормально. Задача #{id}.")
        }
        Ok(false) => format!("Ролик задачи #{id} уже отправляется."),
        Err(e) => format!("Не удалось поставить отправку заново: {e}"),
    }
}

enum ByIdOutcome {
    Reply(String),
    FromFile,
}

/// Повторная отправка по `file_id`: новое сообщение, `result_message_id` обновляется.
fn resend_by_id(bot: &Bot, tj: &TgJob, player: &str, file_id: &str) -> ByIdOutcome {
    let id = tj.job_id;
    let caption = caption_for(bot, id, player);
    match send(
        bot,
        tj.chat_id,
        &InputFile::Id(file_id.to_owned()),
        &caption,
    ) {
        Ok(s) => {
            let new_id = s.file_id.as_deref().unwrap_or(file_id);
            if let Err(e) = store::mark_sent(
                bot.core().db(),
                id,
                s.message_id,
                Some(new_id),
                bot.now_ms(),
            ) {
                bot.log().warn(&format!(
                    "бот: повторная отправка задачи #{id} не записана: {e}"
                ));
            }
            bot.log().info(&format!(
                "бот: ролик задачи #{id} отправлен повторно по file_id"
            ));
            ByIdOutcome::Reply(String::new())
        }
        Err(TgError::Api { code: 400, .. }) => ByIdOutcome::FromFile,
        Err(e) if e.maybe_delivered() => ByIdOutcome::Reply(format!(
            "Не уверен, что ролик задачи #{id} дошёл ещё раз. Если его нет в чате — отправьте /resend {id} снова."
        )),
        Err(e) => {
            if e.is_forbidden() {
                let _ = store::mute_chat(bot.core().db(), tj.chat_id, bot.now_ms());
            }
            bot.log().warn(&format!(
                "бот: повторная отправка задачи #{id} не удалась: {}",
                bot.core().redactor().clean(&e.to_string(), 200)
            ));
            ByIdOutcome::Reply(format!(
                "Не получилось отправить ролик задачи #{id} ещё раз ({}). Попробуйте позже: /resend {id}",
                bot.core().redactor().clean(&e.to_string(), 200)
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_percent_encodes_unsafe_bytes() {
        assert_eq!(
            file_uri(Path::new("/srv/data/blobs/results/7/highlights.mp4")),
            "file:///srv/data/blobs/results/7/highlights.mp4"
        );
        assert_eq!(
            file_uri(Path::new("/srv/my data/100%/a#b?c/видео.mp4")),
            "file:///srv/my%20data/100%25/a%23b%3Fc/%D0%B2%D0%B8%D0%B4%D0%B5%D0%BE.mp4"
        );
        assert_eq!(
            file_uri(Path::new(r"C:\data\x y\a.mp4")),
            "file:///C:/data/x%20y/a.mp4"
        );
    }
}
