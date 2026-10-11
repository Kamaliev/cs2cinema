//! `FakeTelegram` — подставной Telegram для тестов бота: сценарий обновлений, документы на настоящем диске,
//! журнал вызовов и инъекция ошибок. Модуль публичный (не `cfg(test)`): им пользуются интеграционные тесты
//! крейтов `server` и `runner`. Состояние под одним мьютексом, отравление игнорируется.

use super::{Chat, Document, InputFile, Me, Message, Sent, Tg, TgError, TgFile, TgUser, Update};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Запись журнала вызовов: метод, чат (если есть) и текст или путь.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: String,
    pub chat: Option<i64>,
    pub detail: String,
}

/// Отправленный файл (ролик или документ), как его увидел бы Telegram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentFile {
    pub chat: i64,
    /// Путь на диске после разбора URI (для `InputFile::Id` пусто).
    pub path: PathBuf,
    pub size: u64,
    pub caption: String,
    /// `send_video` или `send_document`.
    pub method: String,
    pub message_id: i64,
}

#[derive(Default)]
struct State {
    files_dir: PathBuf,
    updates: VecDeque<Update>,
    next_update_id: i64,
    next_message_id: i64,
    next_doc: u64,
    next_vid: u64,
    /// `file_id` → описание файла для `getFile`.
    documents: HashMap<String, TgFile>,
    /// Сообщения бота: чат, номер, текущий текст — в порядке отправки.
    messages: Vec<(i64, i64, String)>,
    videos: Vec<SentFile>,
    calls: Vec<Call>,
    failures: HashMap<String, VecDeque<TgError>>,
    last_offset: i64,
    get_file_calls: usize,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// Подставной Telegram. Копии делят одно состояние.
#[derive(Clone)]
pub struct FakeTelegram {
    shared: Arc<Shared>,
}

impl FakeTelegram {
    pub fn new(files_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&files_dir);
        let state = State {
            files_dir,
            next_update_id: 1000,
            next_message_id: 500,
            ..Default::default()
        };
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                wake: Condvar::new(),
            }),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn files_dir(&self) -> PathBuf {
        self.st().files_dir.clone()
    }

    /// Ставит обновление в очередь `getUpdates`.
    pub fn push_update(&self, u: Update) {
        self.st().updates.push_back(u);
        self.shared.wake.notify_all();
    }

    /// Разбирает JSON обновления тем же `serde`, что и настоящий клиент, и ставит его в очередь.
    pub fn push_json(&self, json: &str) {
        let u: Update = serde_json::from_str(json).expect("JSON обновления");
        self.push_update(u);
    }

    fn next_ids(&self) -> (i64, i64) {
        let mut st = self.st();
        st.next_update_id += 1;
        st.next_message_id += 1;
        (st.next_update_id, st.next_message_id)
    }

    /// Обновление «пользователь `from` прислал в чат `chat` документ»; `update_id` и `message_id` выдаёт фейк.
    #[allow(clippy::too_many_arguments)]
    pub fn document_update(
        &self,
        from: i64,
        chat: i64,
        file_id: &str,
        file_unique_id: &str,
        file_name: &str,
        size: u64,
        caption: Option<&str>,
    ) -> Update {
        let (update_id, message_id) = self.next_ids();
        Update {
            update_id,
            message: Some(Message {
                message_id,
                chat: Chat {
                    id: chat,
                    kind: "private".to_owned(),
                },
                from: Some(user(from)),
                text: None,
                caption: caption.map(str::to_owned),
                document: Some(Document {
                    file_id: file_id.to_owned(),
                    file_unique_id: file_unique_id.to_owned(),
                    file_name: Some(file_name.to_owned()),
                    file_size: Some(size),
                    mime_type: None,
                }),
                forward_origin: None,
            }),
        }
    }

    /// Обновление «пользователь `from` написал в чат `chat` текст».
    pub fn text_update(&self, from: i64, chat: i64, text: &str) -> Update {
        let (update_id, message_id) = self.next_ids();
        Update {
            update_id,
            message: Some(Message {
                message_id,
                chat: Chat {
                    id: chat,
                    kind: "private".to_owned(),
                },
                from: Some(user(from)),
                text: Some(text.to_owned()),
                caption: None,
                document: None,
                forward_origin: None,
            }),
        }
    }

    /// Кладёт байты документа на диск и регистрирует `file_id` для `getFile`.
    pub fn put_document(&self, file_id: &str, file_unique_id: &str, bytes: &[u8]) -> PathBuf {
        let mut st = self.st();
        st.next_doc += 1;
        let dir = st.files_dir.join("documents");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("file_{}.dem", st.next_doc));
        std::fs::write(&path, bytes).expect("запись документа фейка");
        st.documents.insert(
            file_id.to_owned(),
            TgFile {
                file_id: file_id.to_owned(),
                file_unique_id: file_unique_id.to_owned(),
                file_size: Some(bytes.len() as u64),
                file_path: Some(path.to_string_lossy().into_owned()),
            },
        );
        path
    }

    /// Подменяет `file_path`, который `getFile` вернёт для `file_id` (проверки пути в тестах). `file_id` должен быть
    /// уже зарегистрирован через `put_document`.
    pub fn set_file_path(&self, file_id: &str, path: &str) {
        let mut st = self.st();
        let doc = st
            .documents
            .get_mut(file_id)
            .expect("документ зарегистрирован через put_document");
        doc.file_path = Some(path.to_owned());
    }

    /// Следующий вызов метода `method` вернёт `err` (очередь по имени метода).
    pub fn fail_next(&self, method: &str, err: TgError) {
        self.st()
            .failures
            .entry(method.to_owned())
            .or_default()
            .push_back(err);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.st().calls.clone()
    }

    /// Сколько раз бот вызывал `getFile`.
    pub fn get_file_calls(&self) -> usize {
        self.st().get_file_calls
    }

    /// Тексты бота в чат: отправленные, с учётом правок (текущее содержимое сообщений), в порядке отправки.
    pub fn texts_to(&self, chat: i64) -> Vec<String> {
        self.st()
            .messages
            .iter()
            .filter(|(c, _, _)| *c == chat)
            .map(|(_, _, t)| t.clone())
            .collect()
    }

    /// Номера сообщений бота в чат в порядке отправки.
    pub fn message_ids_to(&self, chat: i64) -> Vec<i64> {
        self.st()
            .messages
            .iter()
            .filter(|(c, _, _)| *c == chat)
            .map(|(_, m, _)| *m)
            .collect()
    }

    pub fn videos(&self) -> Vec<SentFile> {
        self.st().videos.clone()
    }

    /// Смещение, с которым бот в последний раз звал `getUpdates`.
    pub fn last_offset(&self) -> i64 {
        self.st().last_offset
    }

    /// Достаёт готовую ошибку для метода, если она заказана; пишет вызов в журнал.
    fn enter(&self, method: &str, chat: Option<i64>, detail: &str) -> Result<(), TgError> {
        let mut st = self.st();
        st.calls.push(Call {
            method: method.to_owned(),
            chat,
            detail: detail.to_owned(),
        });
        match st.failures.get_mut(method).and_then(|q| q.pop_front()) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn send_file(
        &self,
        method: &str,
        chat: i64,
        file: &InputFile,
        caption: &str,
    ) -> Result<Sent, TgError> {
        let detail = match file {
            InputFile::Local(uri) => uri.clone(),
            InputFile::Id(id) => format!("id:{id}"),
        };
        let injected = self.enter(method, Some(chat), &detail);
        // Заказанная ошибка «возможно доставлено»: ролик сначала записывается, потом приходит ошибка.
        if let Err(e) = &injected
            && !e.maybe_delivered()
        {
            return Err(e.clone());
        }
        let (path, size) = match file {
            InputFile::Local(uri) => {
                let path = path_from_uri(uri).ok_or_else(|| TgError::Api {
                    code: 400,
                    description: "Bad Request: wrong file URI".to_owned(),
                    retry_after: None,
                })?;
                let size = std::fs::metadata(&path)
                    .map_err(|_| TgError::Api {
                        code: 400,
                        description: "Bad Request: failed to open file".to_owned(),
                        retry_after: None,
                    })?
                    .len();
                (path, size)
            }
            InputFile::Id(_) => (PathBuf::new(), 0),
        };
        let mut st = self.st();
        st.next_message_id += 1;
        st.next_vid += 1;
        let message_id = st.next_message_id;
        let vid = st.next_vid;
        st.videos.push(SentFile {
            chat,
            path,
            size,
            caption: caption.to_owned(),
            method: method.to_owned(),
            message_id,
        });
        drop(st);
        injected?;
        Ok(Sent {
            message_id,
            file_id: Some(format!("vid-{vid}")),
        })
    }
}

fn user(id: i64) -> TgUser {
    TgUser {
        id,
        is_bot: false,
        first_name: format!("user{id}"),
        username: None,
    }
}

/// Путь на диске по URI `file:///…`: процентное декодирование; на Windows `/` перед буквой диска отрезается.
pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = rest.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let mut s = String::from_utf8(out).ok()?;
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        s.remove(0);
    }
    Some(PathBuf::from(s))
}

impl Tg for FakeTelegram {
    fn get_me(&self) -> Result<Me, TgError> {
        self.enter("get_me", None, "")?;
        Ok(Me {
            id: 424_242,
            username: Some("cs2cinema_test_bot".to_owned()),
        })
    }

    fn get_updates(&self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, TgError> {
        self.enter("get_updates", None, &offset.to_string())?;
        let mut st = self.st();
        st.last_offset = offset;
        st.updates.retain(|u| u.update_id >= offset);
        if st.updates.is_empty() && timeout_secs > 0 {
            let wait = Duration::from_secs(timeout_secs).min(Duration::from_millis(200));
            st = self
                .shared
                .wake
                .wait_timeout(st, wait)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            st.updates.retain(|u| u.update_id >= offset);
        }
        Ok(st.updates.iter().cloned().collect())
    }

    fn send_message(&self, chat_id: i64, text: &str) -> Result<i64, TgError> {
        self.enter("send_message", Some(chat_id), text)?;
        let mut st = self.st();
        st.next_message_id += 1;
        let id = st.next_message_id;
        st.messages.push((chat_id, id, text.to_owned()));
        Ok(id)
    }

    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), TgError> {
        self.enter("edit_message_text", Some(chat_id), text)?;
        let mut st = self.st();
        let Some(m) = st
            .messages
            .iter_mut()
            .find(|(c, m, _)| *c == chat_id && *m == message_id)
        else {
            return Err(TgError::Api {
                code: 400,
                description: "Bad Request: message to edit not found".to_owned(),
                retry_after: None,
            });
        };
        if m.2 == text {
            return Err(TgError::Api {
                code: 400,
                description: "Bad Request: message is not modified".to_owned(),
                retry_after: None,
            });
        }
        m.2 = text.to_owned();
        Ok(())
    }

    fn get_file(&self, file_id: &str) -> Result<TgFile, TgError> {
        self.st().get_file_calls += 1;
        self.enter("get_file", None, file_id)?;
        self.st()
            .documents
            .get(file_id)
            .cloned()
            .ok_or_else(|| TgError::Api {
                code: 400,
                description: "Bad Request: wrong file_id".to_owned(),
                retry_after: None,
            })
    }

    fn send_video(&self, chat_id: i64, video: &InputFile, caption: &str) -> Result<Sent, TgError> {
        self.send_file("send_video", chat_id, video, caption)
    }

    fn send_document(&self, chat_id: i64, doc: &InputFile, caption: &str) -> Result<Sent, TgError> {
        self.send_file("send_document", chat_id, doc, caption)
    }

    fn set_my_commands(&self, commands: &[(String, String)]) -> Result<(), TgError> {
        let names: Vec<&str> = commands.iter().map(|(c, _)| c.as_str()).collect();
        self.enter("set_my_commands", None, &names.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_is_decoded_back_to_the_path() {
        assert_eq!(
            path_from_uri("file:///srv/data/a%20b.mp4"),
            Some(PathBuf::from("/srv/data/a b.mp4"))
        );
        assert_eq!(
            path_from_uri("file:///C:/data/x%20y/a.mp4"),
            Some(PathBuf::from("C:/data/x y/a.mp4"))
        );
        assert_eq!(path_from_uri("http://x"), None);
    }

    #[test]
    fn updates_below_the_offset_are_dropped_and_edits_change_the_stored_text() {
        let dir = std::env::temp_dir().join(format!("cs2fake-{}", std::process::id()));
        let tg = FakeTelegram::new(dir.clone());
        let a = tg.text_update(1, 1, "a");
        let b = tg.text_update(1, 1, "b");
        let (ida, idb) = (a.update_id, b.update_id);
        tg.push_update(a);
        tg.push_update(b);
        assert_eq!(tg.get_updates(0, 0).unwrap().len(), 2);
        assert_eq!(tg.get_updates(idb, 0).unwrap().len(), 1);
        assert!(ida < idb);
        let m = tg.send_message(1, "x").unwrap();
        tg.edit_message_text(1, m, "y").unwrap();
        assert!(
            tg.edit_message_text(1, m, "y")
                .unwrap_err()
                .is_not_modified()
        );
        assert!(
            tg.edit_message_text(1, m + 99, "z")
                .unwrap_err()
                .is_message_missing()
        );
        assert_eq!(tg.texts_to(1), vec!["y".to_owned()]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
