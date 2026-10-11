//! Общий стенд тестов протокола: настоящий сервер на localhost, настоящий HTTP-клиент.
#![allow(dead_code)]

use server::clock::{Clock, ManualClock, SystemClock};
use server::config::Config;
use server::core::{JobRow, Submit};
use server::log::MemorySink;
use server::secret::Secret;
use server::{Running, auth};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wire::{Assignment, FinishRequest, HelloRequest, ProgressRequest, ReleaseRequest};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Временная папка теста; удаляется вместе со значением.
pub struct TestDir {
    pub path: PathBuf,
}

impl TestDir {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("cs2srv-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Настройки стенда, которые тесты меняют чаще всего.
pub struct TestOptions {
    pub threads: usize,
    pub longpoll_secs: u64,
    pub clock: Option<ManualClock>,
    /// Токен бота Telegram в настройках стенда (для проверок SEC-04).
    pub bot_token: Option<String>,
}

impl Default for TestOptions {
    fn default() -> Self {
        Self {
            threads: 4,
            longpoll_secs: 2,
            clock: None,
            bot_token: None,
        }
    }
}

/// Ответ сервера, как его видит тестовый клиент.
pub struct Resp {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<String>,
    /// Все заголовки ответа: имя и значение.
    pub headers: Vec<(String, String)>,
}

impl Resp {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn error_code(&self) -> String {
        self.json()["error"].as_str().unwrap_or("").to_owned()
    }
}

pub struct TestServer {
    pub dir: TestDir,
    running: Option<Running>,
    pub base: String,
    pub sink: Arc<MemorySink>,
    clock: Option<ManualClock>,
}

impl TestServer {
    pub fn start() -> Self {
        Self::start_with(TestOptions::default())
    }

    pub fn start_with(opts: TestOptions) -> Self {
        let dir = TestDir::new("proto");
        let cfg = Config {
            bind: "127.0.0.1:0".to_owned(),
            data_dir: dir.path.join("data"),
            threads: opts.threads,
            longpoll_secs: opts.longpoll_secs,
            telegram_bot_token: opts.bot_token.map(Secret::new),
            ..Default::default()
        };
        let manual = opts.clock.clone();
        let clock: Arc<dyn Clock> = match opts.clock {
            Some(c) => Arc::new(c),
            None => Arc::new(SystemClock),
        };
        let sink = Arc::new(MemorySink::default());
        let running = server::serve(&cfg, clock, sink.clone()).expect("сервер должен запуститься");
        let base = format!("http://{}", running.addr());
        Self {
            dir,
            running: Some(running),
            base,
            sink,
            clock: manual,
        }
    }

    /// Ручные часы стенда; паника, если стенд запущен на настоящих.
    pub fn clock(&self) -> ManualClock {
        self.clock.clone().expect("стенд запущен без ручных часов")
    }

    pub fn running(&self) -> &Running {
        self.running.as_ref().unwrap()
    }

    /// Останавливает сервер раньше конца теста.
    pub fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.stop();
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Выдаёт раннеру токен через ядро (как `cs2-server runner add`).
    pub fn add_runner(&self, name: &str) -> String {
        let core = self.running().core();
        let (_, token) = auth::add_runner(core.db(), name, core.clock().now_ms()).unwrap();
        token
    }

    /// Ставит демку в очередь (как `cs2-server submit`) и возвращает id задачи.
    pub fn submit(&self, bytes: &[u8], player: &str) -> i64 {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let src = self.dir.path.join(format!("in-{n}.dem"));
        std::fs::write(&src, bytes).unwrap();
        let sub = Submit {
            player: player.to_owned(),
            requested_by: "оператор".to_owned(),
            orig_name: "match.dem".to_owned(),
            ..Default::default()
        };
        let (id, _) = self.running().core().submit(&src, &sub).unwrap();
        id
    }

    pub fn job(&self, id: i64) -> JobRow {
        self.running().core().job(id).unwrap().unwrap()
    }

    /// Запрос к серверу с необязательным токеном и телом (POST, если тело задано, иначе GET).
    pub fn send(&self, path: &str, authorization: Option<&str>, body: Option<&str>) -> Resp {
        send(&self.url(path), authorization, body)
    }

    /// `POST /v1/runner/claim?wait=N` с телом `{"status":"idle"}`.
    pub fn claim(&self, token: &str, wait_secs: u64) -> (u16, Option<Assignment>) {
        let r = self.send(
            &format!("{}?wait={wait_secs}", wire::paths::CLAIM),
            Some(&format!("Bearer {token}")),
            Some(r#"{"status":"idle"}"#),
        );
        let a = (r.status == 200).then(|| serde_json::from_str(&r.body).unwrap());
        (r.status, a)
    }
}

impl TestServer {
    /// `POST /v1/jobs/{id}/progress`: статус и тело ответа.
    pub fn progress(&self, token: &str, job_id: i64, req: &ProgressRequest) -> (u16, String) {
        let r = self.send(
            &wire::paths::job(job_id, "progress"),
            Some(&format!("Bearer {token}")),
            Some(&serde_json::to_string(req).unwrap()),
        );
        (r.status, r.body)
    }

    /// `POST /v1/jobs/{id}/release`: статус и тело ответа.
    pub fn release(&self, token: &str, job_id: i64, req: &ReleaseRequest) -> (u16, String) {
        let r = self.send(
            &wire::paths::job(job_id, "release"),
            Some(&format!("Bearer {token}")),
            Some(&serde_json::to_string(req).unwrap()),
        );
        (r.status, r.body)
    }

    /// `POST /v1/jobs/{id}/finish`: статус и тело ответа.
    pub fn finish(&self, token: &str, job_id: i64, req: &FinishRequest) -> (u16, String) {
        let r = self.send(
            &wire::paths::job(job_id, "finish"),
            Some(&format!("Bearer {token}")),
            Some(&serde_json::to_string(req).unwrap()),
        );
        (r.status, r.body)
    }

    /// Байты `server.db`, `server.db-wal` и `server.db-shm` (какие есть), склеенные подряд.
    pub fn db_bytes(&self) -> Vec<u8> {
        let base = self.dir.path.join("data").join("server.db");
        let mut out = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut name = base.as_os_str().to_owned();
            name.push(suffix);
            if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(name)) {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }

    /// Все строки журнала сервера одним текстом.
    pub fn sink_text(&self) -> String {
        self.sink.lines().join("\n")
    }

    /// `POST /v1/runner/hello`: статус и тело ответа.
    pub fn hello(&self, token: &str, req: &HelloRequest) -> (u16, String) {
        let r = self.send(
            wire::paths::HELLO,
            Some(&format!("Bearer {token}")),
            Some(&serde_json::to_string(req).unwrap()),
        );
        (r.status, r.body)
    }

    /// Один проход `sweep` в ядре (как сделал бы фоновый поток).
    pub fn sweep(&self) -> server::core::Sweep {
        self.running().core().sweep().unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// HTTP-клиент тестов: статусы 4xx и 5xx не ошибка (Pitfall 6), общий срок — 60 с.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into()
}

pub fn send(url: &str, authorization: Option<&str>, body: Option<&str>) -> Resp {
    let agent = agent();
    let result = match body {
        Some(body) => {
            let mut rq = agent.post(url);
            if let Some(a) = authorization {
                rq = rq.header("Authorization", a);
            }
            rq.header("Content-Type", "application/json").send(body)
        }
        None => {
            let mut rq = agent.get(url);
            if let Some(a) = authorization {
                rq = rq.header("Authorization", a);
            }
            rq.call()
        }
    };
    let mut resp = result.expect("запрос к серверу");
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Resp {
        status,
        body,
        retry_after,
        headers,
    }
}

/// Демка из `size` детерминированных байтов с магией `PBDEMS2\0`.
pub fn fake_demo(size: usize) -> Vec<u8> {
    let mut bytes = b"PBDEMS2\0".to_vec();
    let mut x = 12345u32;
    while bytes.len() < size {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bytes.push((x >> 24) as u8);
    }
    bytes
}

// ---------------------------------------------------------------------------------------------------------
// Передача файлов кусками (05-05)
// ---------------------------------------------------------------------------------------------------------

/// Ответ на скачивание куска демки: статус, заголовки и байты тела.
pub struct RangeResp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RangeResp {
    /// Значение заголовка без учёта регистра имени.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// SHA-256 строчными шестнадцатеричными знаками.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    server::hex(&Sha256::digest(bytes))
}

/// Детерминированные байты результата: не повторяются на границах кусков.
pub fn result_bytes(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    let mut x = 0x9E37_79B9u32;
    while out.len() < size {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((x >> 24) as u8);
    }
    out
}

fn upload_offset_of(resp: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    resp.headers()
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

impl TestServer {
    /// `PUT /v1/jobs/{id}/result` на готовом клиенте (соединение можно переиспользовать):
    /// статус, заголовок `Upload-Offset` и тело ответа.
    pub fn put_chunk_on(
        &self,
        agent: &ureq::Agent,
        token: &str,
        job_id: i64,
        epoch: i64,
        offset: u64,
        bytes: &[u8],
    ) -> (u16, Option<u64>, String) {
        let mut resp = agent
            .put(&self.url(&wire::paths::job(job_id, "result")))
            .header("Authorization", &format!("Bearer {token}"))
            .header(wire::headers::EPOCH, &epoch.to_string())
            .header(wire::headers::UPLOAD_OFFSET, &offset.to_string())
            .send(bytes)
            .expect("PUT куска");
        let status = resp.status().as_u16();
        let off = upload_offset_of(&resp);
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        (status, off, body)
    }

    pub fn put_chunk(
        &self,
        token: &str,
        job_id: i64,
        epoch: i64,
        offset: u64,
        bytes: &[u8],
    ) -> (u16, Option<u64>, String) {
        self.put_chunk_on(&agent(), token, job_id, epoch, offset, bytes)
    }

    /// `HEAD /v1/jobs/{id}/result` на готовом клиенте: статус и `Upload-Offset`.
    pub fn head_offset_on(
        &self,
        agent: &ureq::Agent,
        token: &str,
        job_id: i64,
        epoch: i64,
    ) -> (u16, Option<u64>) {
        let resp = agent
            .head(&self.url(&wire::paths::job(job_id, "result")))
            .header("Authorization", &format!("Bearer {token}"))
            .header(wire::headers::EPOCH, &epoch.to_string())
            .call()
            .expect("HEAD результата");
        (resp.status().as_u16(), upload_offset_of(&resp))
    }

    pub fn head_offset(&self, token: &str, job_id: i64, epoch: i64) -> (u16, Option<u64>) {
        self.head_offset_on(&agent(), token, job_id, epoch)
    }

    /// `GET /v1/jobs/{id}/demo` с необязательным `Range`.
    pub fn get_range(
        &self,
        token: &str,
        job_id: i64,
        epoch: i64,
        range: Option<&str>,
    ) -> RangeResp {
        let mut rq = agent()
            .get(&self.url(&wire::paths::job(job_id, "demo")))
            .header("Authorization", &format!("Bearer {token}"))
            .header(wire::headers::EPOCH, &epoch.to_string());
        if let Some(r) = range {
            rq = rq.header("Range", r);
        }
        let mut resp = rq.call().expect("GET демки");
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        let body = resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .unwrap_or_default();
        RangeResp {
            status,
            headers,
            body,
        }
    }

    /// `HEAD /v1/jobs/{id}/demo`: статус и заголовки.
    pub fn head_demo(&self, token: &str, job_id: i64, epoch: i64) -> (u16, Vec<(String, String)>) {
        let resp = agent()
            .head(&self.url(&wire::paths::job(job_id, "demo")))
            .header("Authorization", &format!("Bearer {token}"))
            .header(wire::headers::EPOCH, &epoch.to_string())
            .call()
            .expect("HEAD демки");
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        (resp.status().as_u16(), headers)
    }

    /// Итог `done` с размером и SHA-256: статус и тело ответа.
    pub fn finish_done(
        &self,
        token: &str,
        job_id: i64,
        epoch: i64,
        size: u64,
        sha256: &str,
    ) -> (u16, String) {
        self.finish(
            token,
            job_id,
            &FinishRequest {
                epoch,
                outcome: wire::Outcome::Done {
                    size,
                    sha256: sha256.to_owned(),
                },
            },
        )
    }

    /// Адрес сервера для сырого соединения.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.running().addr()
    }

    /// Папка результата задачи на диске.
    pub fn results_dir(&self, job_id: i64) -> PathBuf {
        self.dir
            .path
            .join("data")
            .join("blobs")
            .join("results")
            .join(job_id.to_string())
    }
}

/// Сырое TCP-соединение с таймаутом чтения 10 с: для запросов, которые `ureq` не умеет (обрыв, `Expect`).
pub fn raw_request(addr: std::net::SocketAddr) -> std::net::TcpStream {
    let s = std::net::TcpStream::connect(addr).expect("соединение с сервером");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}

/// Читает строку до `\n` побайтово, чтобы не забрать лишнего из потока; `\r` в конце отбрасывается.
pub fn read_line(s: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    while let Ok(1) = s.read(&mut b) {
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
    }
    String::from_utf8_lossy(&line)
        .trim_end_matches('\r')
        .to_owned()
}

/// Первая строка ответа, например `HTTP/1.1 204 No Content`.
pub fn read_status_line(s: &mut std::net::TcpStream) -> String {
    read_line(s)
}

/// Читает заголовки ответа до пустой строки (после строки статуса) в виде пар «имя, значение».
pub fn read_headers(s: &mut std::net::TcpStream) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        let line = read_line(s);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    out
}

/// Заголовки сырого `PUT` куска результата; `extra` — готовые строки с `\r\n` на конце.
pub fn raw_put_head(token: &str, job_id: i64, epoch: i64, offset: u64, extra: &str) -> String {
    format!(
        "PUT /v1/jobs/{job_id}/result HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {token}\r\nX-Epoch: {epoch}\r\nUpload-Offset: {offset}\r\n{extra}\r\n"
    )
}

/// Ответ, прочитанный из сырого соединения: строка статуса, заголовки и тело по `Content-Length`.
pub struct RawReply {
    pub status_line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawReply {
    pub fn status(&self) -> u16 {
        self.status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Читает полный ответ; для `HEAD` тело не читается.
pub fn read_reply(s: &mut std::net::TcpStream, head_request: bool) -> RawReply {
    let status_line = read_status_line(s);
    let headers = read_headers(s);
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; if head_request { 0 } else { len }];
    std::io::Read::read_exact(s, &mut body).expect("тело ответа");
    RawReply {
        status_line,
        headers,
        body,
    }
}
