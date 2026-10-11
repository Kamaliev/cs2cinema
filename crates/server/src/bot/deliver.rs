//! Доставка готового ролика новым сообщением. Состояния `tg_jobs.delivery`:
//! `pending` → `sending` (пишется ДО вызова Telegram) → `sent` | `uncertain` | `failed`.
//!
//! Слепого повтора нет: если запрос мог дойти (тайм-аут или обрыв после начала отправки), доставка
//! `uncertain`, и решает человек. После перезапуска `sending` тоже становится `uncertain` (`Bot::recover`).

use super::{Bot, SendAs, store};
use crate::Result;
use crate::tg::InputFile;
use std::path::Path;

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
    let caption = bot
        .core()
        .redactor()
        .clean(&format!("Задача #{id} — игрок {player}"), 1000);
    let sent = match bot.settings().send_as {
        SendAs::Video => bot.tg().send_video(tj.chat_id, &video, &caption),
        SendAs::Document => bot.tg().send_document(tj.chat_id, &video, &caption),
    };
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
        }
    }
    Ok(true)
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
