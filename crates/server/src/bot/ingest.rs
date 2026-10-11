//! Приём входящих: решение «брать ли» (`precheck`, только чтение), запись строки `tg_inbox` внутри транзакции
//! опроса (`accept`) и скачивание с постановкой в очередь (`ingest_once`, вне опроса).
//!
//! `getFile` блокируется до конца скачивания, поэтому вызывается только из `ingest_once`, а не из опроса.
//! Путь из `getFile` содержит токен бота: он не попадает ни в журнал, ни в чат, ни в `reason`.
//! Новые виды входа (проверки размера и дублей, ссылки FACEIT) добавляют сюда, не трогая `dispatch`.

use super::{Bot, BotSettings, store};
use crate::Result;
use crate::core::Submit;
use crate::tg::Message;
use crate::users::{self, User};
use rusqlite::{Transaction, params};
use std::path::{Component, Path, PathBuf};

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

/// Решает, что делать с сообщением друга. Только чтение, ничего не пишет.
pub fn precheck(bot: &Bot, user: &User, msg: &Message) -> Precheck {
    let Some(doc) = &msg.document else {
        return Precheck::Reply(HINT.to_owned());
    };
    let Some(player) = choose_player(bot, user, msg.caption.as_deref()) else {
        return Precheck::Reply(NO_PLAYER.to_owned());
    };
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

/// Записывает входящее в транзакции опроса (одной с новым смещением). Только SQL на `tx`:
/// методы `Core` и `Db::read`/`write` здесь звать нельзя — мьютекс базы не реентерабелен.
pub fn accept(
    bot: &Bot,
    tx: &Transaction<'_>,
    user: &User,
    msg: &Message,
    intake: &Intake,
) -> std::result::Result<Accepted, String> {
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
            .map_err(crate::db::db_err)?
        }
    };
    if n == 0 {
        return Ok(Accepted::Already);
    }
    Ok(Accepted::Row(tx.last_insert_rowid()))
}

/// Текст подтверждения приёма (место в очереди добавляет план 07-06).
pub fn ack_text(bot: &Bot, row_id: i64) -> String {
    match store::inbox_row(bot.core().db(), row_id) {
        Ok(Some(row)) if !row.file_name.is_empty() => format!(
            "Принял «{}» для игрока {}. Скачиваю…",
            row.file_name, row.player
        ),
        Ok(Some(row)) => format!("Принял файл для игрока {}. Скачиваю…", row.player),
        _ => "Принял файл. Скачиваю…".to_owned(),
    }
}

/// Путь файла на стороне сервера по пути из `getFile`: он обязан лежать внутри `files_root` (по компонентам).
fn local_path(settings: &BotSettings, file_path: &str) -> Option<PathBuf> {
    let rel = Path::new(file_path)
        .strip_prefix(&settings.files_root)
        .ok()?;
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(settings.files_root_local.join(rel))
}

/// Класс ошибки постановки в очередь: без путей и подробностей.
fn submit_error_class(e: &str) -> &'static str {
    if e.contains("лимита") {
        "файл больше допустимого размера"
    } else if e.contains("не похоже на демку") {
        "это не демка CS2 (нужен .dem или .dem.zst)"
    } else {
        "сервер не смог принять файл"
    }
}

fn reject(bot: &Bot, row: &store::InboxRow, reason: &str) -> Result<bool> {
    let reason = bot.core().redactor().clean(reason, 200);
    store::reject(bot.core().db(), row.id, &reason, bot.now_ms())?;
    bot.say(
        row.chat_id,
        &format!("Не получилось принять файл: {reason}."),
    );
    Ok(true)
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
    let Some(file_id) = row.file_id.clone() else {
        return reject(bot, &row, "во входящем нет файла");
    };
    let got = match bot.tg().get_file(&file_id) {
        Ok(f) => f,
        Err(e) => return reject(bot, &row, &format!("Telegram не отдал файл ({e})")),
    };
    let Some(local) = got
        .file_path
        .as_deref()
        .and_then(|p| local_path(bot.settings(), p))
    else {
        return reject(bot, &row, "файл лежит вне рабочей папки бота");
    };
    let Some(user) = users::find_user_by_id(db, row.user_id)? else {
        return reject(bot, &row, "пользователь не найден");
    };
    let sub = Submit {
        player: row.player.clone(),
        user_id: Some(user.id),
        requested_by: user.name.clone(),
        orig_name: row.file_name.clone(),
    };
    let (job_id, _) = match bot.core().submit(&local, &sub) {
        Ok(v) => v,
        Err(e) => return reject(bot, &row, submit_error_class(&e)),
    };
    // Копия уже на сервере: оригинал bot-api больше не нужен. Ошибка удаления не фатальна.
    if std::fs::remove_file(&local).is_err() {
        bot.log()
            .warn("бот: оригинал файла в папке bot-api не удалился");
    }
    store::mark_queued(db, &row, job_id, bot.now_ms())?;
    bot.log()
        .info(&format!("бот: демка из чата принята как задача #{job_id}"));
    let text = format!("Задача #{job_id} в очереди (игрок {}).", row.player);
    match row.status_message_id {
        Some(mid) => bot.edit(row.chat_id, mid, &text),
        None => {
            bot.say(row.chat_id, &text);
        }
    }
    Ok(true)
}
