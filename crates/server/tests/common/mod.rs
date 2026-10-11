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

// ---------------------------------------------------------------------------------------------------------
// Стенд бота Telegram (07-02): настоящий сервер на ручных часах, FakeTelegram и Bot поверх ядра
// ---------------------------------------------------------------------------------------------------------

use server::bot::store::TgJob;
use server::bot::{Bot, BotSettings};
use server::tg::fake::FakeTelegram;

/// Начальное время стенда бота, unix-мс.
pub const BOT_T0: i64 = 1_800_000_000_000;

pub struct BotStand {
    pub server: TestServer,
    pub tg: FakeTelegram,
    pub bot: Bot,
    pub tg_dir: TestDir,
    runner: std::sync::Mutex<Option<String>>,
}

impl BotStand {
    pub fn start() -> Self {
        let server = TestServer::start_with(TestOptions {
            clock: Some(ManualClock::new(BOT_T0)),
            ..Default::default()
        });
        let tg_dir = TestDir::new("tg");
        let tg = FakeTelegram::new(tg_dir.path.clone());
        let data_dir = server.dir.path.join("data");
        let bot = Bot::new(
            server.running().core().clone(),
            Arc::new(tg.clone()),
            server.running().log().clone(),
            BotSettings::new(&data_dir, tg.files_dir()),
        );
        Self {
            server,
            tg,
            bot,
            tg_dir,
            runner: std::sync::Mutex::new(None),
        }
    }

    pub fn clock(&self) -> ManualClock {
        self.server.clock()
    }

    /// Друг в белом списке: id строки `users`.
    pub fn add_friend(&self, tg_id: i64, name: &str) -> i64 {
        let core = self.server.running().core();
        server::users::add_user(core.db(), name, Some(tg_id), false, core.clock().now_ms())
            .unwrap()
            .id
    }

    pub fn add_admin(&self, tg_id: i64, name: &str) -> i64 {
        let core = self.server.running().core();
        server::users::add_user(core.db(), name, Some(tg_id), true, core.clock().now_ms())
            .unwrap()
            .id
    }

    /// Кладёт `fake_demo` на диск фейка и толкает обновление «друг прислал файл»; пустая подпись — без подписи.
    /// Возвращает `file_id`.
    pub fn send_demo(&self, from: i64, caption: &str) -> String {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let file_id = format!("file-{n}");
        let unique = format!("uniq-{n}");
        let bytes = fake_demo(4096);
        self.tg.put_document(&file_id, &unique, &bytes);
        let caption = (!caption.is_empty()).then_some(caption);
        let upd = self.tg.document_update(
            from,
            from,
            &file_id,
            &unique,
            "match.dem",
            bytes.len() as u64,
            caption,
        );
        self.tg.push_update(upd);
        file_id
    }

    pub fn step(&self) -> server::bot::StepReport {
        self.bot.step().expect("шаг бота")
    }

    /// Токен единственного фейкового раннера стенда (создаётся при первом обращении).
    pub fn runner_token(&self) -> String {
        let mut slot = self.runner.lock().unwrap();
        slot.get_or_insert_with(|| self.server.add_runner("bot-runner"))
            .clone()
    }

    /// Раннер берёт задачу `job_id`: эпоха выдачи и первый прогресс `record`.
    fn claim_and_record(&self, job_id: i64, token: &str) -> i64 {
        let (status, got) = self.server.claim(token, 0);
        assert_eq!(status, 200, "раннер должен получить задачу");
        let a = got.unwrap();
        assert_eq!(a.job_id, job_id, "выдана не та задача");
        let req = ProgressRequest {
            epoch: a.epoch,
            seq: 1,
            phase: wire::Phase::Record,
            take: None,
            status: String::new(),
        };
        assert_eq!(self.server.progress(token, job_id, &req).0, 200);
        a.epoch
    }

    /// Фейковый раннер доводит задачу до `done`: claim, прогресс, кусок результата, finish. Возвращает размер.
    pub fn finish_done(&self, job_id: i64, token: &str) -> u64 {
        let epoch = self.claim_and_record(job_id, token);
        let data = result_bytes(64 * 1024);
        let (status, _, body) = self.server.put_chunk(token, job_id, epoch, 0, &data);
        assert_eq!(status, 204, "{body}");
        let (status, body) =
            self.server
                .finish_done(token, job_id, epoch, data.len() as u64, &sha256_hex(&data));
        assert_eq!(status, 200, "{body}");
        data.len() as u64
    }

    /// Фейковый раннер завершает задачу сбоем с кодом `code`.
    pub fn finish_failed(&self, job_id: i64, token: &str, code: &str) {
        let epoch = self.claim_and_record(job_id, token);
        let req = FinishRequest {
            epoch,
            outcome: wire::Outcome::Failed(wire::Failure {
                code: code.to_owned(),
                class: "other".to_owned(),
                state: "recording".to_owned(),
                hint: String::new(),
                message: "сбой для теста".to_owned(),
                tail: String::new(),
                elapsed_ms: 1000,
            }),
        };
        let (status, body) = self.server.finish(token, job_id, &req);
        assert_eq!(status, 200, "{body}");
    }

    pub fn tg_job(&self, job_id: i64) -> TgJob {
        server::bot::store::tg_job(self.server.running().core().db(), job_id)
            .unwrap()
            .expect("строка tg_jobs")
    }
}

// ---------------------------------------------------------------------------------------------------------
// Поддельный Bot API (07-05): настоящий HTTP-сервер на loopback вместо локального telegram-bot-api
// ---------------------------------------------------------------------------------------------------------

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

/// Запрос, дошедший до поддельного Bot API.
#[derive(Debug, Clone)]
pub struct BotApiRequest {
    pub method: String,
    /// В пути стоял ожидаемый токен.
    pub token_ok: bool,
    pub body: serde_json::Value,
}

#[derive(Default)]
struct BotApiState {
    log: Vec<BotApiRequest>,
    replies: HashMap<String, VecDeque<serde_json::Value>>,
    silent: HashSet<String>,
}

pub struct FakeBotApi {
    addr: std::net::SocketAddr,
    state: Arc<Mutex<BotApiState>>,
    stop: Arc<AtomicBool>,
    main: Option<std::thread::JoinHandle<()>>,
}

impl FakeBotApi {
    /// Поднимает сервер на `127.0.0.1:0`. Без сценарных ответов: `getMe` и `setMyCommands` отвечают сами,
    /// `getUpdates` ждёт `min(timeout, 1 с)` и возвращает пустой список (как длинный опрос настоящего bot-api).
    pub fn start(token: &str) -> FakeBotApi {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("поддельный Bot API"));
        let addr = server
            .server_addr()
            .to_ip()
            .expect("адрес поддельного Bot API");
        let state = Arc::new(Mutex::new(BotApiState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let expected = token.to_owned();
        let main = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok(Some(rq)) = server.recv_timeout(Duration::from_millis(50)) else {
                        continue;
                    };
                    let (state, stop, expected) = (state.clone(), stop.clone(), expected.clone());
                    // Каждый запрос в своём потоке: долгий опрос не должен задерживать остальные вызовы.
                    std::thread::spawn(move || bot_api_handle(rq, &state, &stop, &expected));
                }
            })
        };
        FakeBotApi {
            addr,
            state,
            stop,
            main: Some(main),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn requests(&self) -> Vec<BotApiRequest> {
        self.state.lock().unwrap().log.clone()
    }

    /// Запросы к одному методу.
    pub fn requests_to(&self, method: &str) -> Vec<BotApiRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method)
            .collect()
    }

    /// Следующий ответ на метод; если у ответа `ok:false`, HTTP-статус берётся из `error_code`.
    pub fn reply(&self, method: &str, json: serde_json::Value) {
        self.state
            .lock()
            .unwrap()
            .replies
            .entry(method.to_owned())
            .or_default()
            .push_back(json);
    }

    /// Следующий запрос к методу остаётся без ответа до остановки поддельного сервера.
    pub fn silence(&self, method: &str) {
        self.state.lock().unwrap().silent.insert(method.to_owned());
    }
}

impl Drop for FakeBotApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.main.take() {
            let _ = h.join();
        }
    }
}

fn bot_api_handle(
    mut rq: tiny_http::Request,
    state: &Mutex<BotApiState>,
    stop: &AtomicBool,
    expected: &str,
) {
    let mut raw = String::new();
    let _ = rq.as_reader().read_to_string(&mut raw);
    let body = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    // Путь: /bot<токен>/<метод>
    let path = rq.url().to_owned();
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let token = parts
        .next()
        .and_then(|s| s.strip_prefix("bot"))
        .unwrap_or("")
        .to_owned();
    let method = parts.next().unwrap_or("").to_owned();
    let token_ok = token == expected;
    let (scripted, silent) = {
        let mut st = state.lock().unwrap();
        st.log.push(BotApiRequest {
            method: method.clone(),
            token_ok,
            body: body.clone(),
        });
        let silent = st.silent.remove(&method);
        let scripted = st.replies.get_mut(&method).and_then(VecDeque::pop_front);
        (scripted, silent)
    };
    let wait = |d: Duration| {
        let until = std::time::Instant::now() + d;
        while std::time::Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    if silent {
        // Держим запрос без ответа, пока не остановят сервер.
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    let reply = if !token_ok {
        serde_json::json!({"ok": false, "error_code": 401, "description": "Unauthorized"})
    } else if let Some(r) = scripted {
        r
    } else {
        match method.as_str() {
            "getMe" => {
                serde_json::json!({"ok": true, "result": {"id": 1, "username": "cs2cinema_test_bot"}})
            }
            "setMyCommands" => serde_json::json!({"ok": true, "result": true}),
            "getUpdates" => {
                let secs = body["timeout"].as_u64().unwrap_or(0).min(1);
                wait(Duration::from_secs(secs));
                serde_json::json!({"ok": true, "result": []})
            }
            _ => serde_json::json!({"ok": false, "error_code": 404, "description": "Not Found"}),
        }
    };
    let status = if reply["ok"].as_bool() == Some(true) {
        200
    } else {
        reply["error_code"].as_u64().unwrap_or(500) as u16
    };
    let header =
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = rq.respond(
        tiny_http::Response::from_string(reply.to_string())
            .with_status_code(status)
            .with_header(header),
    );
}
