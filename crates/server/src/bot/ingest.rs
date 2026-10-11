//! Приём входящих: решение «брать ли» (`precheck`, только чтение), запись строки `tg_inbox` внутри транзакции
//! опроса (`accept`) и скачивание с постановкой в очередь (`ingest_once`, вне опроса).
//!
//! `getFile` блокируется до конца скачивания, поэтому вызывается только из `ingest_once`, а не из опроса.
//! Путь из `getFile` содержит токен бота: он не попадает ни в журнал, ни в чат, ни в `reason`.
//! Новые виды входа (ссылки FACEIT) добавляют сюда, не трогая `dispatch`.
//!
//! Защита приёма (план 07-06): размер и имя проверяются до `getFile`; лимиты D-03 (`admit`) считают и ещё не
//! скачанные строки `tg_inbox`; повтор узнаётся по `file_unique_id` до `getFile` и по SHA-256 после копирования;
//! путь из `getFile` принимается только внутри корня файлов bot-api как обычный файл заявленного размера.

use super::{Bot, BotSettings, store};
use crate::Result;
use crate::core::{DedupKey, JobRow, Submit, player_key};
use crate::db::db_err;
use crate::tg::Message;
use crate::users::{self, User};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::{Component, Path, PathBuf};
use wire::JobState;

const MIB: u64 = 1024 * 1024;

/// Состояния «задача ещё не закончена» (как в ядре).
const ACTIVE_STATES: &str = "('receiving','queued','assigned','running','uploading')";
/// Состояния, в которых задача считается дублем: активные и готовая.
const DEDUP_STATES: &str = "('receiving','queued','assigned','running','uploading','done')";

/// Что бот принимает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeKind {
    Document {
        file_id: String,
        file_unique_id: String,
        file_size: Option<u64>,
        file_name: String,
    },
}

/// Решение «принять» с выбранным игроком (D-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intake {
    pub kind: IntakeKind,
    pub player: String,
}

pub enum Precheck {
    /// Ответить текстом, строку во входящих не создавать.
    Reply(String),
    Accept(Intake),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    /// Новая строка `tg_inbox` с этим id.
    Row(i64),
    /// Отказ с текстом для пользователя.
    Refused(String),
    /// То же сообщение уже принято раньше (повтор обновления): молча пропускаем.
    Already,
}

const NO_PLAYER: &str =
    "Сначала задайте игрока: /player ник или SteamID64 — или напишите ник в подписи к файлу.";
const HINT: &str = "Пришлите демку CS2 файлом (.dem или .dem.zst). Ник игрока напишите в подписи к файлу. Справка: /help";
const NO_SIZE: &str =
    "Не вижу размера файла — пришлите демку файлом (как документ), а не ссылкой или пересылкой.";
const BAD_NAME: &str = "Нужен файл .dem или .dem.zst.";
/// Путь от Telegram не прошёл проверку: ни путей, ни причин в тексте.
const PATH_REJECTED: &str = "не удалось принять файл: проверка пути не пройдена";
const PATH_REJECTED_TEXT: &str =
    "Не удалось принять файл: он лежит не там, где ожидалось. Попробуйте прислать его ещё раз.";

/// Игрок задачи (D-02): непустая подпись переопределяет сохранённого игрока.
fn choose_player(bot: &Bot, user: &User, caption: Option<&str>) -> Option<String> {
    let from_caption = caption
        .map(|c| bot.core().redactor().clean(c.trim(), 64))
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    from_caption.or_else(|| {
        user.player_steam
            .clone()
            .or_else(|| user.player_name.clone())
            .filter(|p| !p.trim().is_empty())
    })
}

/// Имя похоже на демку: `.dem` или `.dem.zst` без учёта регистра. Только дешёвый фильтр до скачивания:
/// истину решает содержимое (`sniff_demo` в `Core::submit`).
fn demo_name_ok(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    n.ends_with(".dem") || n.ends_with(".dem.zst")
}

/// Склонение «активная задача» для числа `n`.
fn active_tasks(n: i64) -> String {
    let word = match (n % 10, n % 100) {
        (_, 11..=14) => "активных задач",
        (1, _) => "активная задача",
        (2..=4, _) => "активные задачи",
        _ => "активных задач",
    };
    format!("{n} {word}")
}

/// Ответ про повтор: задача в очереди или готовый ролик.
fn duplicate_text(job: &JobRow, player: &str) -> String {
    if job.state == JobState::Done {
        format!(
            "Ролик по этой демке для игрока {player} уже готов: /resend {}",
            job.id
        )
    } else {
        format!(
            "Эта демка для игрока {player} уже в очереди: задача #{}",
            job.id
        )
    }
}

/// Строка `tg_files` по `file_unique_id`: хеш и задача-источник.
struct KnownFile {
    sha256: String,
    job_id: i64,
}

fn known_file(bot: &Bot, unique: &str) -> Result<Option<KnownFile>> {
    if unique.is_empty() {
        return Ok(None);
    }
    bot.core().db().read(|c| {
        c.query_row(
            "SELECT sha256, job_id FROM tg_files WHERE file_unique_id = ?1",
            [unique],
            |r| {
                Ok(KnownFile {
                    sha256: r.get(0)?,
                    job_id: r.get(1)?,
                })
            },
        )
        .optional()
        .map_err(db_err)
    })
}

/// Решает, что делать с сообщением друга. Только чтение, ничего не пишет.
pub fn precheck(bot: &Bot, user: &User, msg: &Message) -> Precheck {
    let Some(doc) = &msg.document else {
        return Precheck::Reply(HINT.to_owned());
    };
    let Some(player) = choose_player(bot, user, msg.caption.as_deref()) else {
        return Precheck::Reply(NO_PLAYER.to_owned());
    };
    // Размер и имя — до любых обращений к Telegram за файлом (BOT-04, BOT-11).
    let Some(size) = doc.file_size else {
        return Precheck::Reply(NO_SIZE.to_owned());
    };
    let max = bot.core().limits().max_demo_bytes;
    if size > max {
        return Precheck::Reply(format!(
            "Файл слишком большой: {} МиБ, а лимит {} МиБ.",
            size.div_ceil(MIB),
            max / MIB
        ));
    }
    if !demo_name_ok(doc.file_name.as_deref().unwrap_or("")) {
        return Precheck::Reply(BAD_NAME.to_owned());
    }
    // Повтор уже принятого сообщения (смещение потеряно): ответ про дубль не шлём, `accept` пропустит его молча.
    let replay = bot
        .core()
        .db()
        .read(|c| {
            count(
                c,
                "SELECT EXISTS(SELECT 1 FROM tg_inbox WHERE chat_id = ?1 AND message_id = ?2)",
                params![msg.chat.id, msg.message_id],
            )
        })
        .map(|n| n != 0)
        .unwrap_or(false);
    // Слой 1 против повторов: известный файл и тот же игрок — отвечаем без скачивания.
    match if replay {
        Ok(None)
    } else {
        known_file(bot, &doc.file_unique_id)
    } {
        Ok(Some(known)) => {
            let key = DedupKey::File {
                sha256: known.sha256,
                player_key: player_key(&player),
            };
            match bot.core().find_duplicate(&key) {
                Ok(Some(dup)) => return Precheck::Reply(duplicate_text(&dup, &player)),
                Ok(None) => {}
                Err(_) => bot.log().warn("бот: не удалось проверить повтор файла"),
            }
        }
        Ok(None) => {}
        Err(_) => bot.log().warn("бот: не удалось проверить известные файлы"),
    }
    Precheck::Accept(Intake {
        kind: IntakeKind::Document {
            file_id: doc.file_id.clone(),
            file_unique_id: doc.file_unique_id.clone(),
            file_size: doc.file_size,
            file_name: doc.file_name.clone().unwrap_or_default(),
        },
        player,
    })
}

fn count(
    c: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> std::result::Result<i64, String> {
    c.query_row(sql, args, |r| r.get(0)).map_err(db_err)
}

/// Лимиты D-03 (BOT-11): у друга меньше 2 активных задач (у админа 5), в общей очереди меньше 10. В счёт идут и
/// ещё не скачанные строки `tg_inbox` (`pending`/`downloading`). Только SQL на переданной транзакции: звать в
/// той же `Db::write`, что и вставку строки, чтобы пять файлов подряд не проскочили (Pitfall 4). `Err` — текст
/// отказа для пользователя.
pub fn admit(bot: &Bot, tx: &Transaction<'_>, user: &User) -> std::result::Result<(), String> {
    let s = bot.settings();
    let limit = if user.role == "admin" {
        s.admin_max_active
    } else {
        s.user_max_active
    };
    let busy = "state IN ('pending','downloading')";
    let mine = count(
        tx,
        &format!("SELECT COUNT(*) FROM jobs WHERE user_id = ?1 AND state IN {ACTIVE_STATES}"),
        [user.id],
    )? + count(
        tx,
        &format!("SELECT COUNT(*) FROM tg_inbox WHERE user_id = ?1 AND {busy}"),
        [user.id],
    )?;
    if mine >= limit {
        return Err(format!(
            "У вас уже {} — дождитесь окончания или отмените /cancel.",
            active_tasks(mine)
        ));
    }
    let total = count(
        tx,
        &format!("SELECT COUNT(*) FROM jobs WHERE state IN {ACTIVE_STATES}"),
        [],
    )? + count(
        tx,
        &format!("SELECT COUNT(*) FROM tg_inbox WHERE {busy}"),
        [],
    )?;
    if total >= s.queue_max_active {
        return Err(format!(
            "Сейчас общая очередь заполнена ({}) — попробуйте позже.",
            active_tasks(total)
        ));
    }
    Ok(())
}

/// Записывает входящее в транзакции опроса (одной с новым смещением). Только SQL на `tx`:
/// методы `Core` и `Db::read`/`write` здесь звать нельзя — мьютекс базы не реентерабелен.
pub fn accept(
    bot: &Bot,
    tx: &Transaction<'_>,
    user: &User,
    msg: &Message,
    intake: &Intake,
) -> std::result::Result<Accepted, String> {
    // Повтор уже принятого сообщения: молча пропускаем, лимиты к нему не применяются.
    let seen = count(
        tx,
        "SELECT EXISTS(SELECT 1 FROM tg_inbox WHERE chat_id = ?1 AND message_id = ?2)",
        params![msg.chat.id, msg.message_id],
    )?;
    if seen != 0 {
        return Ok(Accepted::Already);
    }
    if let Err(text) = admit(bot, tx, user) {
        return Ok(Accepted::Refused(text));
    }
    let now = bot.now_ms();
    let n = match &intake.kind {
        IntakeKind::Document {
            file_id,
            file_unique_id,
            file_size,
            file_name,
        } => {
            let name = bot.core().redactor().clean(file_name, 120);
            tx.execute(
                "INSERT OR IGNORE INTO tg_inbox(chat_id, message_id, user_id, kind, file_id, file_unique_id, file_size,
                    file_name, player, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'document', ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?9)",
                params![
                    msg.chat.id,
                    msg.message_id,
                    user.id,
                    file_id,
                    file_unique_id,
                    file_size.map(|s| s as i64),
                    name,
                    intake.player,
                    now
                ],
            )
            .map_err(db_err)?
        }
    };
    if n == 0 {
        return Ok(Accepted::Already);
    }
    Ok(Accepted::Row(tx.last_insert_rowid()))
}

/// Сколько задач впереди ещё не скачанной строки: все активные задачи плюс более ранние строки `tg_inbox` в
/// `pending`/`downloading`. Ошибка чтения считается нулём (подтверждение не должно из-за неё пропасть).
fn ahead_of_row(bot: &Bot, row_id: i64) -> i64 {
    bot.core()
        .db()
        .read(|c| {
            Ok::<i64, String>(count(
                c,
                &format!("SELECT COUNT(*) FROM jobs WHERE state IN {ACTIVE_STATES}"),
                [],
            )? + count(
                c,
                "SELECT COUNT(*) FROM tg_inbox WHERE state IN ('pending','downloading') AND id < ?1",
                [row_id],
            )?)
        })
        .unwrap_or(0)
}

/// «(перед вами K)» или «(очередь пуста)».
fn ahead_phrase(ahead: i64) -> String {
    if ahead > 0 {
        format!("перед вами {ahead}")
    } else {
        "очередь пуста".to_owned()
    }
}

/// Текст подтверждения приёма: имя файла, игрок и сколько задач впереди (BOT-06).
pub fn ack_text(bot: &Bot, row_id: i64) -> String {
    let ahead = ahead_phrase(ahead_of_row(bot, row_id));
    match store::inbox_row(bot.core().db(), row_id) {
        Ok(Some(row)) if !row.file_name.is_empty() => format!(
            "Принял «{}» для игрока {}. Скачиваю… ({ahead})",
            row.file_name, row.player
        ),
        Ok(Some(row)) => format!("Принял файл для игрока {}. Скачиваю… ({ahead})", row.player),
        _ => "Принял файл. Скачиваю…".to_owned(),
    }
}

/// Проверка пути из `getFile` (T-07-25). Путь обязан быть абсолютным, лежать внутри `files_root` по компонентам
/// без `..`, после замены корня на `files_root_local` указывать на обычный файл (не ссылку) заявленного размера
/// и не выходить за корень через ссылки в каталогах. `Err` — только класс ошибки, без пути.
fn safe_local_path(
    settings: &BotSettings,
    file_path: &str,
    expected_size: u64,
) -> std::result::Result<PathBuf, &'static str> {
    let p = Path::new(file_path);
    if !p.is_absolute() {
        return Err("путь не абсолютный");
    }
    let rel = p
        .strip_prefix(&settings.files_root)
        .map_err(|_| "путь вне корня")?;
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("недопустимые компоненты пути");
    }
    let local = settings.files_root_local.join(rel);
    let meta = std::fs::symlink_metadata(&local).map_err(|_| "файл недоступен")?;
    if !meta.file_type().is_file() {
        return Err("не обычный файл");
    }
    if meta.len() != expected_size {
        return Err("размер не совпал");
    }
    let real = std::fs::canonicalize(&local).map_err(|_| "файл недоступен")?;
    let root =
        std::fs::canonicalize(&settings.files_root_local).map_err(|_| "корень недоступен")?;
    if !real.starts_with(&root) {
        return Err("файл за пределами корня");
    }
    Ok(local)
}

/// Класс ошибки постановки в очередь: без путей и подробностей.
fn submit_error_class(e: &str) -> &'static str {
    if e.contains("лимита") {
        "файл больше допустимого размера"
    } else if e.contains("не похоже на демку") {
        "это не демка CS2: пришлите файл .dem или .dem.zst из матча"
    } else {
        "сервер не смог принять файл"
    }
}

/// Отказ после взятия строки: причина в `tg_inbox.reason`, другу — отдельный текст.
fn reject_with(bot: &Bot, row: &store::InboxRow, reason: &str, text: &str) -> Result<bool> {
    let reason = bot.core().redactor().clean(reason, 200);
    store::reject(bot.core().db(), row.id, &reason, bot.now_ms())?;
    bot.say(row.chat_id, text);
    Ok(true)
}

fn reject(bot: &Bot, row: &store::InboxRow, reason: &str) -> Result<bool> {
    let clean = bot.core().redactor().clean(reason, 200);
    reject_with(
        bot,
        row,
        &clean,
        &format!("Не получилось принять файл: {clean}."),
    )
}

/// Ответ в сообщение статуса (правка), а если его нет — новым сообщением.
fn status(bot: &Bot, row: &store::InboxRow, text: &str) {
    match row.status_message_id {
        Some(mid) => bot.edit(row.chat_id, mid, text),
        None => {
            bot.say(row.chat_id, text);
        }
    }
}

/// Повтор найден: строка закрывается, другу уходит ответ про уже существующую задачу.
fn reject_duplicate(bot: &Bot, row: &store::InboxRow, dup: &JobRow) -> Result<bool> {
    let reason = format!("дубль задачи #{}", dup.id);
    store::reject(bot.core().db(), row.id, &reason, bot.now_ms())?;
    status(bot, row, &duplicate_text(dup, &row.player));
    Ok(true)
}

/// Запоминает `file_unique_id` → хеш демки задачи, чтобы повтор узнавался без `getFile`.
fn remember_file(bot: &Bot, unique: Option<&str>, job_id: i64) -> Result<()> {
    let Some(unique) = unique.filter(|u| !u.is_empty()) else {
        return Ok(());
    };
    let Some(demo) = bot.core().demo_file(job_id)? else {
        return Ok(());
    };
    let now = bot.now_ms();
    bot.core().db().write(|tx| {
        tx.execute(
            "INSERT OR IGNORE INTO tg_files(file_unique_id, size, sha256, job_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![unique, demo.size as i64, demo.sha256, job_id, now],
        )
        .map(|_| ())
        .map_err(db_err)
    })
}

/// Задача создана: привязать к строке, запомнить файл и поправить подтверждение на «Задача #N в очереди».
fn queue_row(bot: &Bot, row: &store::InboxRow, job_id: i64) -> Result<bool> {
    remember_file(bot, row.file_unique_id.as_deref(), job_id)?;
    store::mark_queued(bot.core().db(), row, job_id, bot.now_ms())?;
    bot.log()
        .info(&format!("бот: демка из чата принята как задача #{job_id}"));
    let ahead = bot.core().jobs_ahead(job_id).unwrap_or(0);
    let text = format!(
        "Задача #{job_id} в очереди (игрок {}), {}.",
        row.player,
        ahead_phrase(ahead)
    );
    status(bot, row, &text);
    Ok(true)
}

/// Восстановление после падения между `Core::submit` и записью `queued` (Pitfall 3): у строки уже есть своя
/// задача — либо `job_id` остался, либо находится задача того же пользователя, игрока, имени и размера, которую
/// не держит ни одна строка `tg_inbox` и ни одна `tg_jobs`. Привязываемся к ней, а не ставим вторую.
fn own_job(bot: &Bot, row: &store::InboxRow) -> Result<Option<i64>> {
    if let Some(id) = row.job_id
        && bot.core().job(id)?.is_some()
    {
        return Ok(Some(id));
    }
    let Some(size) = row.file_size else {
        return Ok(None);
    };
    let name = bot.core().redactor().clean(&row.file_name, 120);
    let ids: Vec<i64> = bot.core().db().read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT j.id FROM jobs j
                 WHERE j.user_id = ?1 AND j.source_kind = 'file' AND j.orig_name = ?2 AND j.created_at >= ?3
                   AND j.state IN {ACTIVE_STATES}
                   AND NOT EXISTS (SELECT 1 FROM tg_jobs t WHERE t.job_id = j.id)
                   AND NOT EXISTS (SELECT 1 FROM tg_inbox i WHERE i.job_id = j.id)
                   AND EXISTS (SELECT 1 FROM files f WHERE f.job_id = j.id AND f.kind = 'demo' AND f.size = ?4)
                 ORDER BY j.id"
            ))
            .map_err(db_err)?;
        let rows = st
            .query_map(params![row.user_id, name, row.created_at, size], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })?;
    let key = player_key(&row.player);
    for id in ids {
        if let Some(job) = bot.core().job(id)?
            && player_key(&job.player_name) == key
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Другая задача с тем же хешем демки и игроком (активная или готовая), кроме `exclude` — только что созданной.
fn other_duplicate(bot: &Bot, sha256: &str, key: &str, exclude: i64) -> Result<Option<JobRow>> {
    let ids: Vec<i64> = bot.core().db().read(|c| {
        let mut st = c
            .prepare(&format!(
                "SELECT j.id FROM jobs j JOIN files f ON f.job_id = j.id AND f.kind = 'demo'
                 WHERE f.sha256 = ?1 AND j.id <> ?2 AND j.state IN {DEDUP_STATES} ORDER BY j.id DESC"
            ))
            .map_err(db_err)?;
        let rows = st
            .query_map(params![sha256, exclude], |r| r.get::<_, i64>(0))
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })?;
    for id in ids {
        if let Some(job) = bot.core().job(id)?
            && player_key(&job.player_name) == key
        {
            return Ok(Some(job));
        }
    }
    Ok(None)
}

/// Берёт самую старую строку `pending`, скачивает файл (`getFile`) и ставит задачу через `Core::submit`.
pub fn ingest_once(bot: &Bot) -> Result<bool> {
    let db = bot.core().db();
    let Some(mut row) = store::claim_pending(db, bot.now_ms())? else {
        return Ok(false);
    };
    // Подтверждение могло не уйти (падение между записью и ответом): отправляем его до `getFile`.
    if row.status_message_id.is_none() {
        let text = ack_text(bot, row.id);
        if let Some(mid) = bot.say(row.chat_id, &text) {
            store::set_inbox_status_message(db, row.id, mid)?;
            row.status_message_id = Some(mid);
        }
    }
    let Some(user) = users::find_user_by_id(db, row.user_id)? else {
        return reject(bot, &row, "пользователь не найден");
    };
    let sub = Submit {
        player: row.player.clone(),
        user_id: Some(user.id),
        requested_by: user.name.clone(),
        orig_name: row.file_name.clone(),
    };
    let key = player_key(&row.player);

    // Падение после `Core::submit`: задача уже есть.
    if let Some(job_id) = own_job(bot, &row)? {
        return queue_row(bot, &row, job_id);
    }

    // Слой 1: файл уже приходил (тот же `file_unique_id`) — берём демку той задачи без `getFile`.
    if let Some(known) = known_file(bot, row.file_unique_id.as_deref().unwrap_or(""))?
        && bot.core().demo_file(known.job_id)?.is_some()
    {
        // Между приёмом строки и этим шагом та же пара могла стать задачей (две строки подряд).
        let dup_key = DedupKey::File {
            sha256: known.sha256.clone(),
            player_key: key.clone(),
        };
        if let Some(dup) = bot.core().find_duplicate(&dup_key)? {
            return reject_duplicate(bot, &row, &dup);
        }
        match bot.core().submit_from(known.job_id, &sub) {
            Ok(job_id) => return queue_row(bot, &row, job_id),
            Err(_) => bot
                .log()
                .info("бот: демка известного файла недоступна — скачиваю заново"),
        }
    }

    let Some(file_id) = row.file_id.clone() else {
        return reject(bot, &row, "во входящем нет файла");
    };
    let Some(size) = row.file_size.and_then(|s| u64::try_from(s).ok()) else {
        return reject(bot, &row, "не указан размер файла");
    };
    let got = match bot.tg().get_file(&file_id) {
        Ok(f) => f,
        Err(e) => return reject(bot, &row, &format!("Telegram не отдал файл ({e})")),
    };
    let local = match got
        .file_path
        .as_deref()
        .ok_or("нет пути")
        .and_then(|p| safe_local_path(bot.settings(), p, size))
    {
        Ok(p) => p,
        Err(class) => {
            // Причина (класс без пути) — в журнал; другу и в `reason` — общий текст.
            bot.log()
                .warn(&format!("бот: путь из getFile не принят ({class})"));
            return reject_with(bot, &row, PATH_REJECTED, PATH_REJECTED_TEXT);
        }
    };
    let (job_id, ingested) = match bot.core().submit(&local, &sub) {
        Ok(v) => v,
        Err(e) => {
            let class = submit_error_class(&e);
            if e.contains("не похоже на демку") {
                // Содержимое отвергнуто: оригинал в папке bot-api больше не нужен.
                let _ = std::fs::remove_file(&local);
            }
            return reject(bot, &row, class);
        }
    };
    // Копия уже на сервере: оригинал bot-api больше не нужен. Ошибка удаления не фатальна.
    if let Err(e) = std::fs::remove_file(&local) {
        bot.log().warn(&format!(
            "бот: не удалось удалить исходный файл задачи #{job_id} из папки bot-api: {:?}",
            e.kind()
        ));
    }
    // Слой 2: тот же хеш и игрок, но под другим `file_unique_id` — новая копия лишняя.
    if let Some(dup) = other_duplicate(bot, &ingested.sha256, &key, job_id)? {
        remember_file(bot, row.file_unique_id.as_deref(), dup.id)?;
        if let Err(e) = bot.core().cancel(job_id, true) {
            bot.log().warn(&format!(
                "бот: не удалось отменить лишнюю копию задачи #{job_id}: {e}"
            ));
        }
        return reject_duplicate(bot, &row, &dup);
    }
    queue_row(bot, &row, job_id)
}
