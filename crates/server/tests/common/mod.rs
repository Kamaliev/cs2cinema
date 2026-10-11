//! Общий стенд тестов протокола: настоящий сервер на localhost, настоящий HTTP-клиент.
#![allow(dead_code)]

use server::clock::{Clock, ManualClock, SystemClock};
use server::config::Config;
use server::core::{JobRow, Submit};
use server::log::MemorySink;
use server::{Running, auth};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wire::{Assignment, ProgressRequest, ReleaseRequest};

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
}

impl Default for TestOptions {
    fn default() -> Self {
        Self {
            threads: 4,
            longpoll_secs: 2,
            clock: None,
        }
    }
}

/// Ответ сервера, как его видит тестовый клиент.
pub struct Resp {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<String>,
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
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Resp {
        status,
        body,
        retry_after,
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
