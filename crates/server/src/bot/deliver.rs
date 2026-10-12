//! Доставка готового ролика новым сообщением. Состояния `tg_jobs.delivery`:
//! `pending` → `sending` (пишется ДО вызова Telegram) → `sent` | `uncertain` | `failed`.
//!
//! Слепого повтора нет: если запрос мог дойти (тайм-аут или обрыв после начала отправки), доставка
//! `uncertain`, и решает человек (`/resend`). После перезапуска `sending` тоже становится `uncertain`
//! (`Bot::recover`). Повторяется сам только запрос, который точно не ушёл (`429`, ошибка до отправки), и не
//! больше пяти раз; `403` заглушает чат; ролик больше `SEND_LIMIT_BYTES` не отправляется.

use super::store::{self, TgJob};
use super::{Bot, SendAs};
use crate::Result;
use crate::tg::{InputFile, Sent, TgError};
use crate::users::User;
use std::path::Path;
use wire::JobState;

const NOT_YOURS: &str = "Это не ваша задача.";

/// Самый большой ролик, который бот пробует отправить: 1950 МиБ. Telegram принимает до 2000 МБ через локальный
/// Bot API, запас защищает от расхождения в единицах и накладных расходах запроса (допущение A5 исследования).
pub const SEND_LIMIT_BYTES: u64 = 1950 * 1024 * 1024;

/// Сколько раз пробуем отправить ролик, прежде чем признать доставку несостоявшейся.
const MAX_ATTEMPTS: i64 = 5;

/// Пауза перед повтором, если запрос до Telegram не дошёл (нет соединения, тайм-аут до начала отправки).
const UNSENT_RETRY_MS: i64 = 30_000;

/// Меньше этого после `429` не ждём: нулевой `retry_after` не должен крутить цикл доставки.
const MIN_RETRY_MS: i64 = 1_000;

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
    let player = bot
        .core()
        .job(id)?
        .map(|j| j.player_name)
        .unwrap_or_default();
    // Повторная отправка (`/resend` доставленного) идёт по сохранённому `file_id` без загрузки файла. Если Telegram
    // его не принял (`400`), запрос точно не сработал, и ниже ролик уходит с файла (WR-08).
    let mut began = false;
    if let Some(file_id) = tj.result_file_id.clone() {
        if !store::begin_sending(db, id, now)? {
            return Ok(true);
        }
        began = true;
        let caption = caption_for(bot, id, &player);
        match send(bot, tj.chat_id, &InputFile::Id(file_id.clone()), &caption) {
            Ok(s) => {
                let new_id = s.file_id.as_deref().unwrap_or(&file_id);
                store::mark_sent(db, id, s.message_id, Some(new_id), bot.now_ms())?;
                bot.log()
                    .info(&format!("бот: ролик задачи #{id} отправлен повторно по file_id"));
                return Ok(true);
            }
            Err(TgError::Api { code: 400, .. }) => {}
            Err(e) => {
                let attempts = tj.delivery_attempts + 1;
                fail_delivery(bot, &tj, &e, attempts, bot.now_ms())?;
                return Ok(true);
            }
        }
    }
    let on_disk = bot
        .core()
        .data_dir()
        .join("blobs")
        .join("results")
        .join(id.to_string())
        .join("highlights.mp4");
    let Some(file) = bot.core().result_file(id)?.filter(|_| on_disk.is_file()) else {
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
        tell(
            bot,
            &tj,
            &format!("Ролик задачи #{id} не найден на сервере — автор уже получил подробности."),
        );
        return Ok(true);
    };
    // Telegram не примет файл больше своего предела: не грузим его полчаса впустую.
    if file.size > SEND_LIMIT_BYTES {
        store::mark_delivery(
            db,
            id,
            "failed",
            &format!(
                "ролик {} МиБ больше лимита Telegram",
                file.size / (1024 * 1024)
            ),
            now,
        )?;
        bot.log().warn(&format!(
            "бот: ролик задачи #{id} ({} МиБ) больше лимита Telegram — не отправляю",
            file.size / (1024 * 1024)
        ));
        tell(
            bot,
            &tj,
            &format!(
                "Ролик задачи #{id} больше лимита Telegram (≈2 ГБ); автор передаст его иначе."
            ),
        );
        return Ok(true);
    }
    if !began && !store::begin_sending(db, id, now)? {
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
            let attempts = tj.delivery_attempts + 1;
            fail_delivery(bot, &tj, &e, attempts, done_at)?;
        }
    }
    Ok(true)
}

/// Разбор неудачной отправки. Повторяется только то, что точно не ушло (T-07-35):
/// - запрос мог дойти → `uncertain` без автоповтора, друг решает `/resend`;
/// - `403` → `failed`, чат заглушён, друг недоступен;
/// - `429` → `pending` не раньше `retry_after`; ошибка до отправки запроса → `pending` через 30 с;
///   после `MAX_ATTEMPTS` попыток → `failed`;
/// - прочий ответ API → `failed`.
fn fail_delivery(bot: &Bot, tj: &TgJob, e: &TgError, attempts: i64, now: i64) -> Result<()> {
    let db = bot.core().db();
    let id = tj.job_id;
    let text = bot.core().redactor().clean(&e.to_string(), 500);
    if e.maybe_delivered() {
        store::mark_delivery(db, id, "uncertain", &text, now)?;
        bot.log().warn(&format!(
            "бот: отправка ролика задачи #{id} — uncertain: {text}"
        ));
        tell(bot, tj, &uncertain_text(id));
        return Ok(());
    }
    if e.is_forbidden() {
        store::mark_delivery(db, id, "failed", &text, now)?;
        store::mute_chat(db, tj.chat_id, now)?;
        bot.log().warn(&format!(
            "бот: чат задачи #{id} закрыт для бота (403) — ролик не доставлен, чат заглушён"
        ));
        return Ok(());
    }
    let wait = match e {
        TgError::Api { code: 429, .. } => Some(
            e.retry_after_ms()
                .unwrap_or(UNSENT_RETRY_MS)
                .max(MIN_RETRY_MS),
        ),
        TgError::Timeout { .. } | TgError::Io { .. } => Some(UNSENT_RETRY_MS),
        TgError::Api { .. } => None,
    };
    match wait {
        Some(wait) if attempts < MAX_ATTEMPTS => {
            store::retry_delivery(db, id, now + wait, &text, now)?;
            bot.log().warn(&format!(
                "бот: отправка ролика задачи #{id} не удалась (попытка {attempts} из {MAX_ATTEMPTS}), повтор через {} с: {text}",
                wait / 1000
            ));
        }
        _ => {
            store::mark_delivery(db, id, "failed", &text, now)?;
            bot.log().warn(&format!(
                "бот: отправка ролика задачи #{id} — failed: {text}"
            ));
            tell(
                bot,
                tj,
                &format!(
                    "Не получилось отправить ролик задачи #{id} в Telegram. Попробуйте позже: /resend {id}"
                ),
            );
        }
    }
    Ok(())
}

/// Сообщение другу про неясную отправку: ролик мог не дойти, решает `/resend`.
fn uncertain_text(id: i64) -> String {
    format!("Не уверен, что ролик задачи #{id} дошёл. Если его нет в чате — отправьте /resend {id}")
}

/// После перезапуска: отправка, прерванная падением процесса, стала `uncertain` без слова другу. Говорим то же,
/// что и в обычном пути (WR-02). Ошибки только в журнал: запуск бота они не останавливают.
pub(super) fn tell_uncertain_after_restart(bot: &Bot, job_id: i64) {
    match store::tg_job(bot.core().db(), job_id) {
        Ok(Some(tj)) => tell(bot, &tj, &uncertain_text(job_id)),
        Ok(None) => {}
        Err(e) => bot.log().warn(&format!(
            "бот: не прочитал доставку задачи #{job_id} для сообщения после перезапуска: {e}"
        )),
    }
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

/// `/resend N`: отправляет ролик своей готовой задачи ещё раз и возвращает ответ для чата.
///
/// Сама отправка всегда идёт в `deliver_once` в потоке доставки, а не в потоке опроса: передача может занять до часа
/// (WR-08). Здесь доставка только возвращается в `pending`:
/// - доставленный (`sent`) уходит по сохранённому `file_id` Telegram без повторной загрузки файла (`deliver_once`
///   сам берёт `result_file_id` и при отказе Telegram шлёт с файла);
/// - неясный или не ушедший (`uncertain`, `failed`) возвращается с нуля; дубль возможен, о чём сказано заранее;
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
    let was_sent = match tj.delivery.as_str() {
        "sent" => true,
        "uncertain" | "failed" => false,
        _ => return format!("Ролик задачи #{id} уже отправляется."),
    };
    match store::requeue_delivery(bot.core().db(), id, bot.now_ms()) {
        Ok(true) if was_sent => format!("Отправляю ролик задачи #{id} ещё раз."),
        Ok(true) => {
            format!("Отправляю ещё раз; если ролик уже пришёл, дубль — нормально. Задача #{id}.")
        }
        Ok(false) => format!("Ролик задачи #{id} уже отправляется."),
        Err(e) => format!("Не удалось поставить отправку заново: {e}"),
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
