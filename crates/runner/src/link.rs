//! Клиент протокола раннера: только исходящие запросы; токен уходит только в заголовке `Authorization`.
//!
//! Перенаправления запрещены (токен не должен уйти на чужой адрес), общего таймаута у агента нет: срок ответа
//! задаётся на каждый запрос отдельно, потому что `claim` ждёт задачу десятки секунд, а кусок файла — минуты.

use std::io::{Read, Write};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::Body;
use ureq::http::Response;
use wire::{
    ApiError, Assignment, ClaimRequest, FinishReply, FinishRequest, HelloReply, HelloRequest,
    ProgressReply, ProgressRequest, ReleaseRequest, headers, paths,
};

/// Срок установления соединения.
const CONNECT_SECS: u64 = 10;
/// Запас сверх `wait` на ответ `claim`.
const CLAIM_SLACK_SECS: u64 = 15;
/// Срок на один кусок `PUT`.
const PUT_SECS: u64 = 180;
/// Срок на один кусок `GET`.
const GET_SECS: u64 = 120;
/// Срок на остальные запросы.
const SHORT_SECS: u64 = 30;
/// Сколько байт тела ошибки читается: этого хватает на `ApiError`.
const ERROR_BODY_LIMIT: u64 = 64 * 1024;

/// Токен раннера. `Debug` печатает `***`, `Display` нет вовсе: значение нельзя случайно напечатать.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(value: impl Into<String>) -> Token {
        Token(value.into())
    }

    /// Значение заголовка `Authorization`.
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }

    /// Само значение: нужно только для вычёркивания из журнала.
    pub(crate) fn secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(***)")
    }
}

/// Ошибка запроса к серверу.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkError {
    /// Сети нет, соединение оборвано или ответ не разобрался.
    Net(String),
    /// Сервер ответил не 2xx.
    Status {
        status: u16,
        /// ASCII-код из `wire::codes` (пусто, если тела нет, как у `HEAD`).
        error: String,
        /// Текст сервера для человека.
        message: String,
        /// Принятое сервером смещение (`offset_mismatch`).
        offset: Option<u64>,
        /// `Retry-After` в секундах.
        retry_after: Option<u64>,
    },
}

impl LinkError {
    /// Стоит ли повторить запрос позже: сеть, 5xx, `overloaded` и `upload_busy`.
    pub fn is_transient(&self) -> bool {
        match self {
            LinkError::Net(_) => true,
            LinkError::Status { status, error, .. } => {
                *status >= 500
                    || error == wire::codes::OVERLOADED
                    || error == wire::codes::UPLOAD_BUSY
            }
        }
    }

    /// Код ошибки сервера, если он есть.
    pub fn code(&self) -> Option<&str> {
        match self {
            LinkError::Status { error, .. } if !error.is_empty() => Some(error),
            _ => None,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::Net(e) => write!(f, "нет связи с сервером: {e}"),
            LinkError::Status {
                status,
                error,
                message,
                ..
            } => {
                write!(f, "сервер ответил {status}")?;
                if !error.is_empty() {
                    write!(f, " ({error})")?;
                }
                if !message.is_empty() {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
        }
    }
}

/// Соединение раннера с сервером. Копия делит агент `ureq`; поток пульса держит свою.
#[derive(Clone)]
pub struct Link {
    base: String,
    token: Token,
    agent: ureq::Agent,
}

/// Можно ли слать токен по открытому `http://`: только на этот же компьютер или в частную сеть. Имя хоста,
/// кроме `localhost`, не проверить без DNS, поэтому для него нужен `https://`.
fn plain_http_allowed(base: &str) -> bool {
    let rest = base.strip_prefix("http://").unwrap_or(base);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // после логина `user@` и до порта: `[::1]:80` и `127.0.0.1:80`
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback() || a.is_private() || a.is_link_local(),
        Ok(std::net::IpAddr::V6(a)) => {
            a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

impl Link {
    pub fn new(server: &str, token: Token) -> Result<Link, String> {
        let base = server.trim().trim_end_matches('/').to_owned();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(format!(
                "адрес сервера должен начинаться с http:// или https://: {}",
                pipeline::record::log::one_line(&base)
            ));
        }
        if base.starts_with("http://") && !plain_http_allowed(&base) {
            return Err(format!(
                "токен нельзя слать по открытому http:// на внешний адрес: используйте https:// ({})",
                pipeline::record::log::one_line(&base)
            ));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(Duration::from_secs(CONNECT_SECS)))
            // системное хранилище сертификатов, как в faceit: работает и за корпоративными прокси
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Ok(Link { base, token, agent })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Текст сетевой ошибки: одна строка, без токена.
    fn net(&self, e: impl std::fmt::Display) -> LinkError {
        let text = e.to_string().replace(self.token.secret(), "***");
        LinkError::Net(pipeline::record::log::one_line(&text))
    }

    /// 2xx — ответ как есть; иначе разбор тела в `LinkError::Status`.
    fn ok(&self, r: Result<Response<Body>, ureq::Error>) -> Result<Response<Body>, LinkError> {
        let mut resp = r.map_err(|e| self.net(e))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let number = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
        };
        let retry_after = number(headers::RETRY_AFTER);
        let header_offset = number(headers::UPLOAD_OFFSET);
        let body = resp
            .body_mut()
            .with_config()
            .limit(ERROR_BODY_LIMIT)
            .read_to_string()
            .unwrap_or_default();
        let api: Option<ApiError> = serde_json::from_str(&body).ok();
        let clean =
            |s: &str| pipeline::record::log::one_line(&s.replace(self.token.secret(), "***"));
        Err(match api {
            Some(a) => LinkError::Status {
                status,
                error: clean(&a.error),
                message: clean(&a.message),
                offset: a.offset.or(header_offset),
                retry_after,
            },
            None => LinkError::Status {
                status,
                error: String::new(),
                message: String::new(),
                offset: header_offset,
                retry_after,
            },
        })
    }

    fn json<T: DeserializeOwned>(&self, resp: &mut Response<Body>) -> Result<T, LinkError> {
        resp.body_mut()
            .read_json::<T>()
            .map_err(|e| self.net(format!("ответ сервера не разобран: {e}")))
    }

    fn post<B: Serialize>(
        &self,
        path: &str,
        body: &B,
        secs: u64,
    ) -> Result<Response<Body>, LinkError> {
        let rq = self
            .agent
            .post(&self.url(path))
            .header("Authorization", self.token.bearer().as_str())
            .config()
            .timeout_global(Some(Duration::from_secs(secs)))
            .build();
        self.ok(rq.send_json(body))
    }

    /// `POST /v1/runner/hello`.
    pub fn hello(&self, req: &HelloRequest) -> Result<HelloReply, LinkError> {
        let mut resp = self.post(paths::HELLO, req, SHORT_SECS)?;
        self.json(&mut resp)
    }

    /// `POST /v1/runner/claim?wait=N`: 204 — задач нет.
    pub fn claim(&self, wait_secs: u64) -> Result<Option<Assignment>, LinkError> {
        let path = format!("{}?wait={wait_secs}", paths::CLAIM);
        let req = ClaimRequest {
            status: "idle".to_owned(),
        };
        let mut resp = self.post(&path, &req, wait_secs + CLAIM_SLACK_SECS)?;
        if resp.status().as_u16() == 204 {
            return Ok(None);
        }
        self.json(&mut resp).map(Some)
    }

    /// `POST /v1/jobs/{id}/progress`.
    pub fn progress(&self, job_id: i64, req: &ProgressRequest) -> Result<ProgressReply, LinkError> {
        let mut resp = self.post(&paths::job(job_id, "progress"), req, SHORT_SECS)?;
        self.json(&mut resp)
    }

    /// `POST /v1/jobs/{id}/release`.
    pub fn release(&self, job_id: i64, req: &ReleaseRequest) -> Result<(), LinkError> {
        self.post(&paths::job(job_id, "release"), req, SHORT_SECS)
            .map(|_| ())
    }

    /// `POST /v1/jobs/{id}/finish`.
    pub fn finish(&self, job_id: i64, req: &FinishRequest) -> Result<FinishReply, LinkError> {
        let mut resp = self.post(&paths::job(job_id, "finish"), req, SHORT_SECS)?;
        self.json(&mut resp)
    }

    fn head(&self, job_id: i64, what: &str, epoch: i64) -> Result<Response<Body>, LinkError> {
        let rq = self
            .agent
            .head(&self.url(&paths::job(job_id, what)))
            .header("Authorization", self.token.bearer().as_str())
            .header(headers::EPOCH, epoch.to_string().as_str())
            .config()
            .timeout_global(Some(Duration::from_secs(SHORT_SECS)))
            .build();
        self.ok(rq.call())
    }

    /// `HEAD /v1/jobs/{id}/demo`: размер демки из `Content-Length`.
    pub fn demo_head(&self, job_id: i64, epoch: i64) -> Result<u64, LinkError> {
        let resp = self.head(job_id, "demo", epoch)?;
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .ok_or_else(|| self.net("в ответе HEAD нет Content-Length"))
    }

    /// `GET /v1/jobs/{id}/demo` с `Range: bytes={offset}-`: тело потоком в `out`, возвращает число байт.
    /// Читается не больше `max` байт: сервер или прокси, приславший лишнее, не заполнит диск.
    pub fn demo_range(
        &self,
        job_id: i64,
        epoch: i64,
        offset: u64,
        max: u64,
        out: &mut dyn Write,
    ) -> Result<u64, LinkError> {
        let rq = self
            .agent
            .get(&self.url(&paths::job(job_id, "demo")))
            .header("Authorization", self.token.bearer().as_str())
            .header(headers::EPOCH, epoch.to_string().as_str())
            .header("Range", format!("bytes={offset}-").as_str())
            .config()
            .timeout_global(Some(Duration::from_secs(GET_SECS)))
            .build();
        let mut resp = self.ok(rq.call())?;
        if resp.status().as_u16() != 206 {
            return Err(self.net(format!(
                "ожидался ответ 206 на Range, пришёл {}",
                resp.status().as_u16()
            )));
        }
        let mut reader = resp.body_mut().as_reader().take(max);
        std::io::copy(&mut reader, out).map_err(|e| self.net(format!("чтение демки: {e}")))
    }

    /// `HEAD /v1/jobs/{id}/result`: сколько байт результата сервер уже принял.
    pub fn result_head(&self, job_id: i64, epoch: i64) -> Result<u64, LinkError> {
        let resp = self.head(job_id, "result", epoch)?;
        upload_offset(&resp).ok_or_else(|| self.net("в ответе HEAD нет Upload-Offset"))
    }

    /// `PUT /v1/jobs/{id}/result`: один кусок с заданного смещения; возвращает новое смещение сервера.
    pub fn result_put(
        &self,
        job_id: i64,
        epoch: i64,
        offset: u64,
        body: &[u8],
    ) -> Result<u64, LinkError> {
        let rq = self
            .agent
            .put(&self.url(&paths::job(job_id, "result")))
            .header("Authorization", self.token.bearer().as_str())
            .header(headers::EPOCH, epoch.to_string().as_str())
            .header(headers::UPLOAD_OFFSET, offset.to_string().as_str())
            .config()
            .timeout_global(Some(Duration::from_secs(PUT_SECS)))
            .build();
        let resp = self.ok(rq.send(body))?;
        upload_offset(&resp).ok_or_else(|| self.net("в ответе на PUT нет Upload-Offset"))
    }
}

fn upload_offset(resp: &Response<Body>) -> Option<u64> {
    resp.headers()
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_debug_hides_the_secret() {
        let token = Token::new("cs2r_super_secret_value");
        let shown = format!("{token:?}");
        assert!(!shown.contains("super_secret"), "{shown}");
        assert!(shown.contains("***"), "{shown}");
        assert_eq!(token.bearer(), "Bearer cs2r_super_secret_value");
    }

    #[test]
    fn transient_errors_are_network_5xx_overloaded_and_busy() {
        let st = |status: u16, error: &str| LinkError::Status {
            status,
            error: error.to_owned(),
            message: String::new(),
            offset: None,
            retry_after: None,
        };
        assert!(LinkError::Net("обрыв".to_owned()).is_transient());
        assert!(st(500, "internal").is_transient());
        assert!(st(503, wire::codes::OVERLOADED).is_transient());
        assert!(st(409, wire::codes::UPLOAD_BUSY).is_transient());
        assert!(!st(409, wire::codes::STALE_EPOCH).is_transient());
        assert!(!st(401, wire::codes::UNAUTHORIZED).is_transient());
    }

    #[test]
    fn server_address_must_be_http() {
        assert!(Link::new("example.org", Token::new("t")).is_err());
        assert!(Link::new("http://127.0.0.1:8080/", Token::new("t")).is_ok());
    }

    #[test]
    fn plain_http_is_allowed_only_for_loopback_and_private_addresses() {
        for ok in [
            "http://localhost:8080",
            "http://127.0.0.1:8080/",
            "http://[::1]:8080",
            "http://192.168.1.5:8080",
            "http://10.0.0.2",
            "http://172.16.3.4:80/v1",
            "https://example.org",
        ] {
            assert!(Link::new(ok, Token::new("t")).is_ok(), "{ok}");
        }
        for bad in [
            "http://example.org",
            "http://8.8.8.8:8080",
            "http://172.32.0.1",
            "http://127.0.0.1@evil.example",
            "http://evil.example/127.0.0.1",
        ] {
            assert!(Link::new(bad, Token::new("t")).is_err(), "{bad}");
        }
    }
}
