//! SQL таблиц бота: смещение `getUpdates` (`tg_kv`), входящие (`tg_inbox`) и задачи в чатах (`tg_jobs`).
//!
//! Правила: мьютекс базы не держат во время вызовов `Tg` и копирования файлов; функции с `tx` зовут только
//! внутри `Db::write` и только SQL на переданной транзакции (мьютекс не реентерабелен).

use super::ingest::ACTIVE_STATES as ACTIVE;
use crate::Result;
use crate::db::{Db, db_err};
use rusqlite::{OptionalExtension, Row, Transaction, params};

/// Строка `tg_inbox`.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxRow {
    pub id: i64,
    pub chat_id: i64,
    pub message_id: i64,
    pub user_id: i64,
    pub kind: String,
    pub file_id: Option<String>,
    pub file_unique_id: Option<String>,
    pub file_size: Option<i64>,
    pub file_name: String,
    pub faceit_link: Option<String>,
    pub player: String,
    pub state: String,
    pub job_id: Option<i64>,
    pub reason: Option<String>,
    pub status_message_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) const INBOX_COLUMNS: &str = "id, chat_id, message_id, user_id, kind, file_id, file_unique_id, file_size, \
    file_name, faceit_link, player, state, job_id, reason, status_message_id, created_at, updated_at";

fn inbox_from_row(r: &Row<'_>) -> rusqlite::Result<InboxRow> {
    Ok(InboxRow {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        message_id: r.get(2)?,
        user_id: r.get(3)?,
        kind: r.get(4)?,
        file_id: r.get(5)?,
        file_unique_id: r.get(6)?,
        file_size: r.get(7)?,
        file_name: r.get(8)?,
        faceit_link: r.get(9)?,
        player: r.get(10)?,
        state: r.get(11)?,
        job_id: r.get(12)?,
        reason: r.get(13)?,
        status_message_id: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

/// Строка `tg_jobs`: чат задачи, сообщение статуса и состояние доставки ролика.
#[derive(Debug, Clone, PartialEq)]
pub struct TgJob {
    pub job_id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub status_message_id: Option<i64>,
    pub status_text: String,
    pub next_edit_ms: i64,
    pub muted: bool,
    pub final_reported_ms: Option<i64>,
    pub admin_reported_ms: Option<i64>,
    /// `none`, `pending`, `sending`, `sent`, `uncertain` или `failed`.
    pub delivery: String,
    pub delivery_attempts: i64,
    pub next_try_ms: i64,
    pub result_message_id: Option<i64>,
    pub result_file_id: Option<String>,
    pub delivery_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(crate) const TG_JOB_COLUMNS: &str = "job_id, chat_id, user_id, status_message_id, status_text, next_edit_ms, muted, \
    final_reported_ms, admin_reported_ms, delivery, delivery_attempts, next_try_ms, result_message_id, result_file_id, \
    delivery_error, created_at, updated_at";

fn tg_job_from_row(r: &Row<'_>) -> rusqlite::Result<TgJob> {
    Ok(TgJob {
        job_id: r.get(0)?,
        chat_id: r.get(1)?,
        user_id: r.get(2)?,
        status_message_id: r.get(3)?,
        status_text: r.get(4)?,
        next_edit_ms: r.get(5)?,
        muted: r.get::<_, i64>(6)? != 0,
        final_reported_ms: r.get(7)?,
        admin_reported_ms: r.get(8)?,
        delivery: r.get(9)?,
        delivery_attempts: r.get(10)?,
        next_try_ms: r.get(11)?,
        result_message_id: r.get(12)?,
        result_file_id: r.get(13)?,
        delivery_error: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

/// Смещение для `getUpdates` (0, пока ничего не принято).
pub fn offset(db: &Db) -> Result<i64> {
    db.read(|c| {
        let v: Option<String> = c
            .query_row("SELECT value FROM tg_kv WHERE key = 'offset'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_err)?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    })
}

/// Записывает новое смещение; зовётся в той же транзакции, что и приём сообщения.
pub fn set_offset(tx: &Transaction<'_>, value: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO tg_kv(key, value) VALUES ('offset', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [value.to_string()],
    )
    .map(|_| ())
    .map_err(db_err)
}

/// Все входящие по возрастанию id.
pub fn inbox(db: &Db) -> Result<Vec<InboxRow>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!("SELECT {INBOX_COLUMNS} FROM tg_inbox ORDER BY id"))
            .map_err(db_err)?;
        let rows = st.query_map([], inbox_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

pub fn inbox_row(db: &Db, id: i64) -> Result<Option<InboxRow>> {
    db.read(|c| {
        c.query_row(
            &format!("SELECT {INBOX_COLUMNS} FROM tg_inbox WHERE id = ?1"),
            [id],
            inbox_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

/// Берёт самую старую строку `pending` и одной транзакцией переводит её в `downloading`.
pub fn claim_pending(db: &Db, now_ms: i64) -> Result<Option<InboxRow>> {
    db.write(|tx| {
        let row = tx
            .query_row(
                &format!(
                    "SELECT {INBOX_COLUMNS} FROM tg_inbox WHERE state = 'pending' AND next_try_ms <= ?1 ORDER BY id LIMIT 1"
                ),
                [now_ms],
                inbox_from_row,
            )
            .optional()
            .map_err(db_err)?;
        let Some(mut row) = row else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE tg_inbox SET state = 'downloading', updated_at = ?2 WHERE id = ?1",
            params![row.id, now_ms],
        )
        .map_err(db_err)?;
        row.state = "downloading".to_owned();
        row.updated_at = now_ms;
        Ok(Some(row))
    })
}

/// Сколько раз приём этой строки уже откладывался (WR-05, WR-07).
pub fn inbox_attempts(db: &Db, row_id: i64) -> Result<i64> {
    db.read(|c| {
        c.query_row("SELECT attempts FROM tg_inbox WHERE id = ?1", [row_id], |r| {
            r.get(0)
        })
        .map_err(db_err)
    })
}

/// Строка возвращается в `pending` до `next_try_ms`: временный сбой, повторную отправку файла друг делать не должен.
pub fn defer_inbox(db: &Db, row_id: i64, next_try_ms: i64, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_inbox SET state = 'pending', attempts = attempts + 1, next_try_ms = ?2, updated_at = ?3
             WHERE id = ?1 AND state = 'downloading'",
            params![row_id, next_try_ms, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

pub fn set_inbox_status_message(db: &Db, row_id: i64, message_id: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_inbox SET status_message_id = ?2 WHERE id = ?1",
            params![row_id, message_id],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Строка `rejected` с причиной (текст без путей и токенов).
pub fn reject(db: &Db, row_id: i64, reason: &str, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_inbox SET state = 'rejected', reason = ?2, updated_at = ?3 WHERE id = ?1",
            params![row_id, reason, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Задача принята ядром: строка `queued` с `job_id` и новая строка `tg_jobs` — одной транзакцией.
pub fn mark_queued(db: &Db, row: &InboxRow, job_id: i64, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_inbox SET state = 'queued', job_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![row.id, job_id, now_ms],
        )
        .map_err(db_err)?;
        tx.execute(
            "INSERT INTO tg_jobs(job_id, chat_id, user_id, status_message_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![job_id, row.chat_id, row.user_id, row.status_message_id, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

pub fn tg_job(db: &Db, job_id: i64) -> Result<Option<TgJob>> {
    db.read(|c| {
        c.query_row(
            &format!("SELECT {TG_JOB_COLUMNS} FROM tg_jobs WHERE job_id = ?1"),
            [job_id],
            tg_job_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

pub fn tg_jobs(db: &Db) -> Result<Vec<TgJob>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT {TG_JOB_COLUMNS} FROM tg_jobs ORDER BY job_id"
            ))
            .map_err(db_err)?;
        let rows = st.query_map([], tg_job_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

/// Строки `tg_jobs`, которым может понадобиться сообщение статуса: чат не заглушён, а задача либо не окончена,
/// либо отменена и об отмене ещё не сказано.
pub fn status_candidates(db: &Db) -> Result<Vec<TgJob>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT {TG_JOB_COLUMNS} FROM tg_jobs
                 WHERE muted = 0 AND (
                     job_id IN (SELECT id FROM jobs WHERE state IN {ACTIVE})
                     OR (final_reported_ms IS NULL AND job_id IN (SELECT id FROM jobs WHERE state = 'cancelled')))
                 ORDER BY job_id"
            ))
            .map_err(db_err)?;
        let rows = st.query_map([], tg_job_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

/// Строки `tg_jobs` упавших задач, о сбое которых ещё не сказано: другу (чат не заглушён) или админам.
pub fn failure_candidates(db: &Db) -> Result<Vec<TgJob>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT {TG_JOB_COLUMNS} FROM tg_jobs
                 WHERE job_id IN (SELECT id FROM jobs WHERE state = 'failed')
                   AND (admin_reported_ms IS NULL OR (final_reported_ms IS NULL AND muted = 0))
                 ORDER BY job_id"
            ))
            .map_err(db_err)?;
        let rows = st.query_map([], tg_job_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

/// Занимает право отправить админам отчёт о сбое: `true` только первому, кто успел (`admin_reported_ms`).
pub fn claim_admin_report(db: &Db, job_id: i64, now_ms: i64) -> Result<bool> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE tg_jobs SET admin_reported_ms = ?2, updated_at = ?2
                 WHERE job_id = ?1 AND admin_reported_ms IS NULL",
                params![job_id, now_ms],
            )
            .map_err(db_err)?;
        Ok(n == 1)
    })
}

/// Запоминает текст сообщения статуса и время, раньше которого его не правят.
pub fn note_status(db: &Db, job_id: i64, text: &str, next_edit_ms: i64, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET status_text = ?2, next_edit_ms = ?3, updated_at = ?4 WHERE job_id = ?1",
            params![job_id, text, next_edit_ms, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Откладывает следующую правку (ответ 429 или неясный сбой), текст не трогает.
pub fn defer_edit(db: &Db, job_id: i64, next_edit_ms: i64, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET next_edit_ms = ?2, updated_at = ?3 WHERE job_id = ?1",
            params![job_id, next_edit_ms, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Новое сообщение статуса вместо потерянного (или так и не отправленного): номер, текст и срок следующей правки.
pub fn set_status_message(
    db: &Db,
    job_id: i64,
    message_id: i64,
    text: &str,
    next_edit_ms: i64,
    now_ms: i64,
) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET status_message_id = ?2, status_text = ?3, next_edit_ms = ?4, updated_at = ?5
             WHERE job_id = ?1",
            params![job_id, message_id, text, next_edit_ms, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Telegram ответил `403`: чат заглушается целиком (все его строки), правки и отчёты в него прекращаются.
pub fn mute_chat(db: &Db, chat_id: i64, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET muted = 1, updated_at = ?2 WHERE chat_id = ?1",
            params![chat_id, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Занимает право сказать «задача отменена»: `true` только первому, кто успел (отметка `final_reported_ms`).
pub fn claim_final(db: &Db, job_id: i64, now_ms: i64) -> Result<bool> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE tg_jobs SET final_reported_ms = ?2, updated_at = ?2
                 WHERE job_id = ?1 AND final_reported_ms IS NULL",
                params![job_id, now_ms],
            )
            .map_err(db_err)?;
        Ok(n == 1)
    })
}

/// Снимает отметку, если сообщение об отмене точно не ушло: тик повторит его позже.
pub fn unclaim_final(db: &Db, job_id: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET final_reported_ms = NULL WHERE job_id = ?1",
            [job_id],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Новая строка `tg_jobs` для задачи, поставленной командой (`/retry`): сообщение статуса уже отправлено.
pub struct NewTgJob<'a> {
    pub job_id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub status_message_id: Option<i64>,
    pub status_text: &'a str,
    pub next_edit_ms: i64,
}

pub fn insert_tg_job(db: &Db, n: &NewTgJob<'_>, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "INSERT INTO tg_jobs(job_id, chat_id, user_id, status_message_id, status_text, next_edit_ms, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                n.job_id,
                n.chat_id,
                n.user_id,
                n.status_message_id,
                n.status_text,
                n.next_edit_ms,
                now_ms
            ],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Номера задач пользователя в заданных состояниях (`states` — готовый список SQL вида `('failed','cancelled')`,
/// только из констант кода), от свежих к старым, не больше `limit`.
pub fn user_job_ids(db: &Db, user_id: i64, states: &str, limit: i64) -> Result<Vec<i64>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT id FROM jobs WHERE user_id = ?1 AND state IN {states} ORDER BY id DESC LIMIT ?2"
            ))
            .map_err(db_err)?;
        let rows = st
            .query_map(params![user_id, limit], |r| r.get::<_, i64>(0))
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

/// Сколько задач сейчас у раннера (выданы, ведутся или загружают результат).
pub fn working_count(db: &Db) -> Result<i64> {
    db.read(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('assigned','running','uploading')",
            [],
            |r| r.get(0),
        )
        .map_err(db_err)
    })
}

/// Первая строка `tg_jobs` в `pending`, которой пора слать ролик.
pub fn next_pending_delivery(db: &Db, now_ms: i64) -> Result<Option<TgJob>> {
    db.read(|c| {
        c.query_row(
            &format!(
                "SELECT {TG_JOB_COLUMNS} FROM tg_jobs WHERE delivery = 'pending' AND next_try_ms <= ?1 ORDER BY job_id LIMIT 1"
            ),
            [now_ms],
            tg_job_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

/// Намерение отправки: `pending` → `sending` и `delivery_attempts + 1` до вызова Telegram.
/// `false` — строка уже не `pending` (кто-то успел раньше).
pub fn begin_sending(db: &Db, job_id: i64, now_ms: i64) -> Result<bool> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE tg_jobs SET delivery = 'sending', delivery_attempts = delivery_attempts + 1, updated_at = ?2
                 WHERE job_id = ?1 AND delivery = 'pending'",
                params![job_id, now_ms],
            )
            .map_err(db_err)?;
        Ok(n == 1)
    })
}

/// Итог отправки: `sent` с номером сообщения и `file_id`.
pub fn mark_sent(
    db: &Db,
    job_id: i64,
    message_id: i64,
    file_id: Option<&str>,
    now_ms: i64,
) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET delivery = 'sent', result_message_id = ?2, result_file_id = ?3, delivery_error = NULL, updated_at = ?4
             WHERE job_id = ?1",
            params![job_id, message_id, file_id, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Запрос точно не ушёл или Telegram просит подождать: `sending` → `pending`, следующая попытка не раньше
/// `next_try_ms`. Число попыток остаётся как есть (его увеличил `begin_sending`).
pub fn retry_delivery(
    db: &Db,
    job_id: i64,
    next_try_ms: i64,
    error: &str,
    now_ms: i64,
) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET delivery = 'pending', next_try_ms = ?2, delivery_error = ?3, updated_at = ?4
             WHERE job_id = ?1 AND delivery = 'sending'",
            params![job_id, next_try_ms, error, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Итог без успеха: `uncertain` или `failed` с причиной.
pub fn mark_delivery(db: &Db, job_id: i64, delivery: &str, error: &str, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET delivery = ?2, delivery_error = ?3, updated_at = ?4 WHERE job_id = ?1",
            params![job_id, delivery, error, now_ms],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// `none` → `pending` для задач, которые ядро довело до `done`. Возвращает число переведённых строк.
pub fn promote_done(db: &Db, now_ms: i64) -> Result<usize> {
    db.write(|tx| {
        tx.execute(
            "UPDATE tg_jobs SET delivery = 'pending', updated_at = ?1
             WHERE delivery = 'none' AND job_id IN (SELECT id FROM jobs WHERE state = 'done')",
            [now_ms],
        )
        .map_err(db_err)
    })
}

/// Доставка снова в очередь по просьбе владельца (`/resend`): `uncertain`, `failed` или `sent` → `pending`,
/// счётчик попыток с нуля, чат разглушается (человек только что написал боту, значит чат открыт), отметка
/// «другу сказано» снимается, чтобы исход новой отправки получил своё сообщение. `false` — доставка в другом
/// состоянии (отправляется прямо сейчас), ничего не менялось.
pub fn requeue_delivery(db: &Db, job_id: i64, now_ms: i64) -> Result<bool> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE tg_jobs SET delivery = 'pending', delivery_attempts = 0, next_try_ms = 0, muted = 0,
                 final_reported_ms = NULL, delivery_error = NULL, updated_at = ?2
                 WHERE job_id = ?1 AND delivery IN ('uncertain','failed','sent')",
                params![job_id, now_ms],
            )
            .map_err(db_err)?;
        if n == 1 {
            tx.execute(
                "UPDATE tg_jobs SET muted = 0, updated_at = ?2
                 WHERE chat_id = (SELECT chat_id FROM tg_jobs WHERE job_id = ?1)",
                params![job_id, now_ms],
            )
            .map_err(db_err)?;
        }
        Ok(n == 1)
    })
}

/// Восстановление после перезапуска: `downloading` → `pending`, `sending` → `uncertain`.
/// Возвращает числа переведённых строк входящих и доставок.
pub fn recover(db: &Db, now_ms: i64) -> Result<(usize, usize)> {
    db.write(|tx| {
        let inbox = tx
            .execute(
                "UPDATE tg_inbox SET state = 'pending', updated_at = ?1 WHERE state = 'downloading'",
                [now_ms],
            )
            .map_err(db_err)?;
        let sending = tx
            .execute(
                "UPDATE tg_jobs SET delivery = 'uncertain', delivery_error = 'сервер перезапустился во время отправки', updated_at = ?1
                 WHERE delivery = 'sending'",
                [now_ms],
            )
            .map_err(db_err)?;
        Ok((inbox, sending))
    })
}
