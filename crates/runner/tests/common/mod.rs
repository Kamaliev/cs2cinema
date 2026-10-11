//! Стенд тестов раннера: настоящий `server::serve` в процессе теста и исполнитель-заглушка.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pipeline::job::Outcome;
use pipeline::{FailCode, Progress, RecordFailure, RecordState, RunError};
use runner::{Config, Executor, Task, Token};
use server::clock::SystemClock;
use server::config::Config as ServerConfig;
use server::core::{JobRow, Submit};
use server::log::MemorySink;
use server::{Running, auth};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Временная папка теста; удаляется вместе со значением.
pub struct TestDir {
    pub path: PathBuf,
}

impl TestDir {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("cs2run-{tag}-{}-{n}", std::process::id()));
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

/// Настоящий сервер на localhost с выданным раннеру токеном.
pub struct Stand {
    pub dir: TestDir,
    running: Option<Running>,
    pub base: String,
    pub token: String,
}

impl Stand {
    pub fn start() -> Stand {
        let dir = TestDir::new("flow");
        let cfg = ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            data_dir: dir.path.join("data"),
            threads: 4,
            longpoll_secs: 1,
            ..Default::default()
        };
        let running = server::serve(&cfg, Arc::new(SystemClock), Arc::new(MemorySink::default()))
            .expect("сервер должен запуститься");
        let base = format!("http://{}", running.addr());
        let core = running.core();
        let (_, token) = auth::add_runner(core.db(), "tester", core.clock().now_ms()).unwrap();
        Stand {
            dir,
            running: Some(running),
            base,
            token,
        }
    }

    fn running(&self) -> &Running {
        self.running.as_ref().unwrap()
    }

    /// Ставит демку в очередь (как `cs2-server submit`) и возвращает номер задачи.
    /// Имя файла намеренно враждебное: в путях раннера оно участвовать не должно.
    pub fn submit(&self, bytes: &[u8], player: &str) -> i64 {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let src = self.dir.path.join(format!("in-{n}.dem"));
        std::fs::write(&src, bytes).unwrap();
        let sub = Submit {
            player: player.to_owned(),
            requested_by: "оператор".to_owned(),
            orig_name: "..\\..\\evil name.dem".to_owned(),
            ..Default::default()
        };
        let (id, _) = self.running().core().submit(&src, &sub).unwrap();
        id
    }

    pub fn job(&self, id: i64) -> JobRow {
        self.running().core().job(id).unwrap().unwrap()
    }

    /// Папка принятого результата задачи на сервере.
    pub fn results_dir(&self, id: i64) -> PathBuf {
        self.dir
            .path
            .join("data")
            .join("blobs")
            .join("results")
            .join(id.to_string())
    }

    /// Рабочая папка раннера.
    pub fn runner_dir(&self) -> PathBuf {
        self.dir.path.join("runner")
    }

    /// Настройки раннера: без ожидания в long-poll.
    pub fn runner_config(&self) -> Config {
        Config {
            server: self.base.clone(),
            token: Token::new(self.token.clone()),
            dir: self.runner_dir(),
            name: "tester".to_owned(),
            poll_secs: Some(0),
        }
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            r.stop();
        }
    }
}

/// Что исполнитель-заглушка делает при очередном вызове.
#[derive(Clone)]
pub enum Scenario {
    /// Записывает в `out/srv{id}/highlights.mp4` столько детерминированных байт.
    Result(usize),
    /// Сбой записи с кодом и состоянием автомата и двумя строками хвоста консоли.
    Fail { code: FailCode, state: RecordState },
    /// Ошибка без типизированного описания.
    Plain(String),
    /// Исполнитель остановился по отмене.
    Cancel,
}

/// Исполнитель-заглушка: очередь сценариев, расходуемая по вызову (последний повторяется).
pub struct ScriptedExecutor {
    dir: PathBuf,
    scenarios: Mutex<VecDeque<Scenario>>,
    pub calls: AtomicUsize,
    pub last: Mutex<Option<Task>>,
}

impl ScriptedExecutor {
    pub fn new(dir: PathBuf, scenarios: Vec<Scenario>) -> Arc<ScriptedExecutor> {
        Arc::new(ScriptedExecutor {
            dir,
            scenarios: Mutex::new(scenarios.into()),
            calls: AtomicUsize::new(0),
            last: Mutex::new(None),
        })
    }

    pub fn last_task(&self) -> Task {
        self.last
            .lock()
            .unwrap()
            .clone()
            .expect("исполнитель не вызывался")
    }

    fn next(&self) -> Scenario {
        let mut q = self.scenarios.lock().unwrap();
        if q.len() > 1 {
            q.pop_front().unwrap()
        } else {
            q.front().cloned().expect("нет сценариев")
        }
    }
}

impl Executor for ScriptedExecutor {
    fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some(task.clone());
        p.log("заглушка исполнителя");
        match self.next() {
            Scenario::Fail { code, state } => {
                let mut fail = RecordFailure::new(
                    code,
                    state,
                    "дубль не записан".to_owned(),
                    Duration::from_millis(1500),
                );
                fail.console_tail = vec![
                    "строка консоли\nс переводом".to_owned(),
                    "вторая".to_owned(),
                ];
                Err(RunError {
                    message: "дубль не записан".to_owned(),
                    fail: Some(fail),
                })
            }
            Scenario::Plain(message) => Err(RunError {
                message,
                fail: None,
            }),
            Scenario::Cancel => Err(RunError {
                message: pipeline::CANCELLED.to_owned(),
                fail: None,
            }),
            Scenario::Result(n) => {
                let out_dir = self.dir.join("out").join(format!("srv{}", task.job_id));
                std::fs::create_dir_all(&out_dir).unwrap();
                let result = out_dir.join("highlights.mp4");
                std::fs::write(&result, result_bytes(n)).unwrap();
                Ok(Outcome {
                    out_dir,
                    result,
                    shots: 1,
                    secs: 1.0,
                })
            }
        }
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

/// SHA-256 строчными шестнадцатеричными знаками.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    server::hex(&Sha256::digest(bytes))
}
