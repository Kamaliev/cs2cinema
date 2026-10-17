//! Боевой клиент Bot API для локального `telegram-bot-api` (`HttpTg`, реализация `Tg` на `ureq`).
//!
//! Правила модуля:
//! - токен бота живёт только здесь и только в URL запроса `{база}/bot{токен}/{метод}`; любой текст ошибки, который
//!   уходит наружу, проходит `scrub` (точный токен, его форма с `~` и `Redactor`). Обработчик фасада `log` в сервере
//!   не ставится: `ureq` пишет адреса с токеном на уровнях debug и trace;
//! - адрес — только `http://` на loopback (`127.0.0.1`, `localhost`, `[::1]`) с портом: облачный `api.telegram.org`
//!   режет файлы до 50 МБ и получил бы токен по сети, поэтому он отвергается при создании клиента;
//! - тело файла через клиент не идёт: ролик уходит полем `file:///…`, его читает сам процесс `telegram-bot-api`;
//! - текст отправляется без `parse_mode`: пользовательские строки не должны ничего разметкой ломать.

use super::{Me, Sent, Tg, TgError, TgFile, Update};
use crate::secret::{Redactor, Secret};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;
use ureq::Agent;
use ureq::Timeout;

/// Сроки вызовов. `control` — обычный вызов, `poll_extra` — запас сверх срока длинного опроса,
/// `transfer` — `getFile` и отправка ролика (bot-api отвечает, когда закончит работу с файлом).
#[derive(Debug, Clone, Copy)]
pub struct HttpTimeouts {
    pub control: Duration,
    pub poll_extra: Duration,
    pub transfer: Duration,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            control: Duration::from_secs(30),
            poll_extra: Duration::from_secs(15),
            transfer: Duration::from_secs(60 * 60),
        }
    }
}

/// Время на соединение с локальным процессом: он либо живёт рядом, либо не живёт.
const CONNECT: Duration = Duration::from_secs(5);

/// Проверка адреса `telegram-bot-api` из настроек: только `http://` на loopback с портом.
/// Её же зовёт проверка конфига сервера. Сам адрес в тексте ошибок не повторяется.
pub fn check_base_url(url: &str) -> Result<(), String> {
    parse_base_url(url).map(|_| ())
}

/// Разобранный адрес без хвостового `/`: `http://хост:порт`.
fn parse_base_url(url: &str) -> Result<String, String> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(
            "адрес telegram-bot-api должен начинаться с http:// (облачный адрес и https не поддерживаются)"
                .to_owned(),
        );
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains(['/', '@', '?', '#', ' ', '\\']) {
        return Err(
            "адрес telegram-bot-api должен быть вида http://127.0.0.1:порт без пути и логина"
                .to_owned(),
        );
    }
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let Some((inner, tail)) = after.split_once(']') else {
            return Err("адрес telegram-bot-api: не закрыта скобка в IPv6".to_owned());
        };
        let Some(port) = tail.strip_prefix(':') else {
            return Err("в адресе telegram-bot-api нужен порт".to_owned());
        };
        (format!("[{inner}]"), port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), port),
            None => return Err("в адресе telegram-bot-api нужен порт".to_owned()),
        }
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return Err(
            "telegram-bot-api должен работать на этом же компьютере: допустимы 127.0.0.1, localhost и [::1]"
                .to_owned(),
        );
    }
    match port.parse::<u16>() {
        Ok(p) if p != 0 && port.bytes().all(|b| b.is_ascii_digit()) => {}
        _ => {
            return Err(
                "в адресе telegram-bot-api порт должен быть числом от 1 до 65535".to_owned(),
            );
        }
    }
    Ok(format!("http://{host}:{port}"))
}

pub struct HttpTg {
    base: String,
    token: Secret,
    redactor: Redactor,
    timeouts: HttpTimeouts,
    control: Agent,
    poll: Agent,
    transfer: Agent,
}

impl std::fmt::Debug for HttpTg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpTg { .. }")
    }
}

impl HttpTg {
    pub fn new(
        base_url: &str,
        token: &Secret,
        redactor: Redactor,
        timeouts: HttpTimeouts,
    ) -> Result<HttpTg, String> {
        let base = parse_base_url(base_url)?;
        // Прокси из окружения отключён: токен не должен уходить никуда, кроме локального процесса.
        // Статусы 4xx/5xx не ошибка: `ok:false` разбирает сам клиент. Перенаправления не нужны.
        let build = |cfg: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>| -> Agent {
            cfg.http_status_as_error(false)
                .proxy(None)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .build()
                .into()
        };
        let control = build(
            Agent::config_builder()
                .timeout_connect(Some(CONNECT))
                .timeout_global(Some(timeouts.control)),
        );
        // Срок опроса зависит от вызова и ставится на каждый запрос отдельно (`call`).
        let poll = build(Agent::config_builder().timeout_connect(Some(CONNECT)));
        let transfer = build(
            Agent::config_builder()
                .timeout_connect(Some(CONNECT))
                .timeout_recv_response(Some(timeouts.transfer)),
        );
        Ok(HttpTg {
            base,
            token: token.clone(),
            redactor,
            timeouts,
            control,
            poll,
            transfer,
        })
    }

    /// Маскирует токен во всех его формах в произвольном тексте.
    fn scrub(&self, text: &str) -> String {
        let token = self.token.expose();
        let mut out = text.to_owned();
        if !token.is_empty() {
            out = out.replace(token, "***");
            if token.contains(':') {
                out = out.replace(&token.replace(':', "~"), "***");
            }
        }
        self.redactor.redact(&out)
    }

    /// Сводит ошибку `ureq` к `TgError`. Решает один вопрос: мог ли запрос дойти до bot-api.
    ///
    /// - `Timeout(Resolve | Connect)`, `HostNotFound`, `ConnectionFailed`, `BadUri`, `Http`, `InvalidProxyUrl`,
    ///   `Io` с `ConnectionRefused`, `AddrNotAvailable` или `NetworkUnreachable`: соединения не было, `sent: false`;
    /// - любой другой `Timeout` (отправка заголовков и тела, ожидание ответа) — `Timeout { sent: true }`;
    /// - прочие `Io`, `Protocol`, `LargeResponseHeader` и остальное — `Io { sent: true }`: запрос мог сработать.
    fn classify(&self, e: ureq::Error) -> TgError {
        use std::io::ErrorKind;
        match e {
            ureq::Error::Timeout(Timeout::Resolve | Timeout::Connect) => {
                TgError::Timeout { sent: false }
            }
            ureq::Error::Timeout(_) => TgError::Timeout { sent: true },
            ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::BadUri(_)
            | ureq::Error::Http(_)
            | ureq::Error::InvalidProxyUrl => TgError::Io {
                sent: false,
                detail: self.scrub(&e.to_string()),
            },
            ureq::Error::Io(io)
                if matches!(
                    io.kind(),
                    ErrorKind::ConnectionRefused
                        | ErrorKind::AddrNotAvailable
                        | ErrorKind::NetworkUnreachable
                        | ErrorKind::HostUnreachable
                        | ErrorKind::NetworkDown
                ) =>
            {
                TgError::Io {
                    sent: false,
                    detail: self.scrub(&io.to_string()),
                }
            }
            other => TgError::Io {
                sent: true,
                detail: self.scrub(&other.to_string()),
            },
        }
    }

    /// `POST {база}/bot{токен}/{метод}` с телом JSON и разбором `Reply<T>`. `global` — срок на весь вызов
    /// поверх настроек агента (нужен длинному опросу).
    fn call<T: DeserializeOwned>(
        &self,
        agent: &Agent,
        global: Option<Duration>,
        method: &str,
        body: Value,
    ) -> Result<T, TgError> {
        // Токен — только в URL, и только здесь.
        let url = format!("{}/bot{}/{method}", self.base, self.token.expose());
        let mut rq = agent.post(&url).header("Content-Type", "application/json");
        if let Some(g) = global {
            rq = rq.config().timeout_global(Some(g)).build();
        }
        let mut resp = rq.send(body.to_string()).map_err(|e| self.classify(e))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| self.classify(e))?;
        let reply: Reply<T> = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(_) => {
                return Err(TgError::Io {
                    sent: true,
                    detail: format!("ответ bot-api не разобран (HTTP {status})"),
                });
            }
        };
        if !reply.ok {
            return Err(TgError::Api {
                code: reply.error_code.unwrap_or(i64::from(status)),
                description: self.scrub(reply.description.as_deref().unwrap_or("")),
                retry_after: reply.parameters.and_then(|p| p.retry_after),
            });
        }
        reply.result.ok_or_else(|| TgError::Io {
            sent: true,
            detail: "ответ bot-api без поля result".to_owned(),
        })
    }
}

#[derive(Deserialize)]
struct Reply<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<Params>,
}

#[derive(Deserialize)]
struct Params {
    #[serde(default)]
    retry_after: Option<u64>,
}

#[derive(Deserialize)]
struct FileRef {
    file_id: String,
}

/// Отправленное сообщение: нужен номер и `file_id` вложения из первого присутствующего поля.
#[derive(Deserialize)]
struct SentMessage {
    message_id: i64,
    #[serde(default)]
    video: Option<FileRef>,
    #[serde(default)]
    document: Option<FileRef>,
    #[serde(default)]
    animation: Option<FileRef>,
}

impl SentMessage {
    fn into_sent(self) -> Sent {
        let file_id = self
            .video
            .or(self.document)
            .or(self.animation)
            .map(|f| f.file_id);
        Sent {
            message_id: self.message_id,
            file_id,
        }
    }
}

#[derive(Deserialize)]
struct MessageId {
    message_id: i64,
}

fn input_str(f: &super::InputFile) -> &str {
    match f {
        super::InputFile::Local(s) | super::InputFile::Id(s) => s,
    }
}

impl Tg for HttpTg {
    fn get_me(&self) -> Result<Me, TgError> {
        self.call(&self.control, None, "getMe", json!({}))
    }

    fn get_updates(&self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, TgError> {
        // Только личные сообщения: остальные виды обновлений боту не нужны и лишний раз не приходят.
        let body = json!({
            "offset": offset,
            "timeout": timeout_secs,
            "limit": 100,
            "allowed_updates": ["message"],
        });
        let global = Duration::from_secs(timeout_secs) + self.timeouts.poll_extra;
        self.call(&self.poll, Some(global), "getUpdates", body)
    }

    fn send_message(&self, chat_id: i64, text: &str) -> Result<i64, TgError> {
        let m: MessageId = self.call(
            &self.control,
            None,
            "sendMessage",
            json!({"chat_id": chat_id, "text": text}),
        )?;
        Ok(m.message_id)
    }

    fn send_web_app_button(&self, chat_id: i64, text: &str, button: &str, url: &str) -> Result<i64, TgError> {
        let m: MessageId = self.call(
            &self.control,
            None,
            "sendMessage",
            json!({
                "chat_id": chat_id,
                "text": text,
                "reply_markup": {"inline_keyboard": [[{"text": button, "web_app": {"url": url}}]]}
            }),
        )?;
        Ok(m.message_id)
    }

    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), TgError> {
        let _: Value = self.call(
            &self.control,
            None,
            "editMessageText",
            json!({"chat_id": chat_id, "message_id": message_id, "text": text}),
        )?;
        Ok(())
    }

    fn get_file(&self, file_id: &str) -> Result<TgFile, TgError> {
        self.call(&self.transfer, None, "getFile", json!({"file_id": file_id}))
    }

    fn send_video(
        &self,
        chat_id: i64,
        video: &super::InputFile,
        caption: &str,
    ) -> Result<Sent, TgError> {
        let m: SentMessage = self.call(
            &self.transfer,
            None,
            "sendVideo",
            json!({
                "chat_id": chat_id,
                "video": input_str(video),
                "caption": caption,
                "supports_streaming": true,
            }),
        )?;
        Ok(m.into_sent())
    }

    fn send_document(
        &self,
        chat_id: i64,
        doc: &super::InputFile,
        caption: &str,
    ) -> Result<Sent, TgError> {
        let m: SentMessage = self.call(
            &self.transfer,
            None,
            "sendDocument",
            json!({
                "chat_id": chat_id,
                "document": input_str(doc),
                "caption": caption,
            }),
        )?;
        Ok(m.into_sent())
    }

    fn set_my_commands(&self, commands: &[(String, String)]) -> Result<(), TgError> {
        let list: Vec<Value> = commands
            .iter()
            .map(|(c, d)| json!({"command": c, "description": d}))
            .collect();
        let _: Value = self.call(
            &self.control,
            None,
            "setMyCommands",
            json!({"commands": list}),
        )?;
        Ok(())
    }
}
