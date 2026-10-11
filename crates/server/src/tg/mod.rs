//! Шов к Telegram Bot API: трейт `Tg`, ошибки и типы обновлений.
//!
//! Бот (`crate::bot`) говорит с Telegram только через `Arc<dyn Tg>`. Настоящий клиент (`HttpTg` на `ureq`,
//! план 07-05) держит токен бота в себе и нигде его не показывает; в тестах стоит `fake::FakeTelegram`.
//! Типы обновлений разбираются `serde`: лишние поля игнорируются, необязательные имеют значения по умолчанию.

pub mod fake;
pub mod http;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Файл для отправки: локальный путь для процесса `telegram-bot-api` (URI `file:///…`) или готовый `file_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputFile {
    Local(String),
    Id(String),
}

/// Итог отправки файла: номер сообщения и `file_id` ролика на серверах Telegram, если Telegram его вернул.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub message_id: i64,
    pub file_id: Option<String>,
}

/// Ошибка вызова Bot API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TgError {
    /// Telegram ответил `ok: false`.
    Api {
        code: i64,
        description: String,
        retry_after: Option<u64>,
    },
    /// Ответа не дождались; `sent` — тело запроса уже уходило, запрос мог дойти.
    Timeout { sent: bool },
    /// Сбой соединения; `sent` — как у `Timeout`.
    Io { sent: bool, detail: String },
}

impl TgError {
    /// Запрос мог дойти до Telegram и сработать: слепой повтор опасен (дубль).
    pub fn maybe_delivered(&self) -> bool {
        matches!(
            self,
            TgError::Timeout { sent: true } | TgError::Io { sent: true, .. }
        )
    }

    /// `400 message is not modified`: текст уже такой, правка не нужна.
    pub fn is_not_modified(&self) -> bool {
        matches!(self, TgError::Api { code: 400, description, .. } if description.contains("message is not modified"))
    }

    /// `400 message to edit not found`: сообщение удалено, нужно новое.
    pub fn is_message_missing(&self) -> bool {
        matches!(self, TgError::Api { code: 400, description, .. }
            if description.contains("message to edit not found") || description.contains("message not found"))
    }

    /// `403`: пользователь заблокировал бота или чат недоступен.
    pub fn is_forbidden(&self) -> bool {
        matches!(self, TgError::Api { code: 403, .. })
    }

    /// Сколько миллисекунд Telegram просит подождать (`429 retry_after`).
    pub fn retry_after_ms(&self) -> Option<i64> {
        match self {
            TgError::Api {
                retry_after: Some(s),
                ..
            } => Some((*s as i64).saturating_mul(1000)),
            _ => None,
        }
    }
}

/// Текст для человека: без адресов, путей и токена.
impl fmt::Display for TgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TgError::Api {
                code, description, ..
            } => write!(f, "Telegram ответил ошибкой {code}: {description}"),
            TgError::Timeout { sent } => {
                if *sent {
                    f.write_str("Telegram не ответил вовремя после начала отправки")
                } else {
                    f.write_str("Telegram не ответил вовремя")
                }
            }
            TgError::Io { sent, .. } => {
                if *sent {
                    f.write_str("связь с Telegram оборвалась после начала отправки")
                } else {
                    f.write_str("нет связи с Telegram")
                }
            }
        }
    }
}

impl std::error::Error for TgError {}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Message {
    pub message_id: i64,
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<TgUser>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub document: Option<Document>,
    #[serde(default)]
    pub forward_origin: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Chat {
    pub id: i64,
    /// `private`, `group`, `supergroup` или `channel`.
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TgUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Document {
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Ответ `getFile`. `file_path` — абсолютный путь в пространстве имён процесса `telegram-bot-api`
/// (в нём может быть токен бота: не писать ни в журнал, ни в чат).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TgFile {
    pub file_id: String,
    #[serde(default)]
    pub file_unique_id: String,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Me {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
}

/// Вызовы Bot API, которые нужны боту. Все методы блокирующие; `getFile` возвращается после скачивания файла.
pub trait Tg: Send + Sync {
    fn get_me(&self) -> Result<Me, TgError>;
    /// Long polling: ждёт обновления не дольше `timeout_secs`, подтверждает все с `update_id < offset`.
    fn get_updates(&self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, TgError>;
    /// Возвращает `message_id` отправленного сообщения. Текст без `parse_mode`.
    fn send_message(&self, chat_id: i64, text: &str) -> Result<i64, TgError>;
    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), TgError>;
    fn get_file(&self, file_id: &str) -> Result<TgFile, TgError>;
    fn send_video(&self, chat_id: i64, video: &InputFile, caption: &str) -> Result<Sent, TgError>;
    fn send_document(&self, chat_id: i64, doc: &InputFile, caption: &str) -> Result<Sent, TgError>;
    /// Команды меню бота: пары «команда, описание».
    fn set_my_commands(&self, commands: &[(String, String)]) -> Result<(), TgError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_is_parsed_and_unknown_fields_are_ignored() {
        let u: Update = serde_json::from_str(
            r#"{"update_id":7,"message":{"message_id":3,"date":1,"chat":{"id":5,"type":"private","first_name":"x"},
            "from":{"id":5,"is_bot":false,"first_name":"Вася","language_code":"ru"},
            "caption":"s1mple","document":{"file_id":"f","file_unique_id":"u","file_name":"a.dem","file_size":10}}}"#,
        )
        .unwrap();
        let m = u.message.unwrap();
        assert_eq!(m.chat.kind, "private");
        assert_eq!(m.document.unwrap().file_size, Some(10));
        assert_eq!(m.caption.as_deref(), Some("s1mple"));
    }

    #[test]
    fn error_helpers_classify_bot_api_answers() {
        let nm = TgError::Api {
            code: 400,
            description: "Bad Request: message is not modified".into(),
            retry_after: None,
        };
        assert!(nm.is_not_modified() && !nm.is_message_missing());
        let rate = TgError::Api {
            code: 429,
            description: "Too Many Requests".into(),
            retry_after: Some(30),
        };
        assert_eq!(rate.retry_after_ms(), Some(30_000));
        assert!(TgError::Timeout { sent: true }.maybe_delivered());
        assert!(!TgError::Timeout { sent: false }.maybe_delivered());
        let text = TgError::Io {
            sent: true,
            detail: "http://secret/bot123:AAA".into(),
        }
        .to_string();
        assert!(!text.contains("secret"), "{text}");
    }
}
