//! Стенд тестов раннера: настоящий `server::serve` в процессе теста и исполнитель-заглушка.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipeline::job::Outcome;
use pipeline::record::Event;
use pipeline::{FailCode, Progress, RecordFailure, RecordState, RunError};
use runner::{Config, Executor, Task, Token};
use server::clock::{Clock, ManualClock, SystemClock};
use server::config::Config as ServerConfig;
use server::core::{Core, JobRow, Submit};
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
    /// Ручные часы сервера, если стенд запущен с ними.
    pub clock: Option<ManualClock>,
}

impl Stand {
    pub fn start() -> Stand {
        Stand::start_with(Arc::new(SystemClock), None)
    }

    /// Стенд с ручными часами сервера: аренды истекают, когда скажет тест.
    pub fn start_manual() -> Stand {
        let clock = ManualClock::new(SystemClock.now_ms());
        Stand::start_with(Arc::new(clock.clone()), Some(clock))
    }

    fn start_with(clock: Arc<dyn Clock>, manual: Option<ManualClock>) -> Stand {
        let dir = TestDir::new("flow");
        let cfg = ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            data_dir: dir.path.join("data"),
            threads: 4,
            longpoll_secs: 1,
            ..Default::default()
        };
        let running = server::serve(&cfg, clock, Arc::new(MemorySink::default()))
            .expect("сервер должен запуститься");
        let base = format!("http://{}", running.addr());
        let core = running.core();
        let (_, token) = auth::add_runner(core.db(), "tester", core.clock().now_ms()).unwrap();
        Stand {
            dir,
            running: Some(running),
            base,
            token,
            clock: manual,
        }
    }

    /// Копия ядра сервера: заглушка исполнителя может отменять задачу сама.
    pub fn core(&self) -> Core {
        self.running().core().clone()
    }

    /// Выдаёт ещё один токен раннера.
    pub fn second_token(&self) -> String {
        let core = self.running().core();
        auth::add_runner(core.db(), "second", core.clock().now_ms())
            .unwrap()
            .1
    }

    /// Двигает ручные часы сервера.
    pub fn advance(&self, d: Duration) {
        self.clock.as_ref().expect("нужен start_manual").advance(d);
    }

    /// Возвращает в очередь задачи с истёкшей арендой.
    pub fn sweep(&self) {
        self.running().core().sweep().unwrap();
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

    /// Отмена оператором, как `cs2-server cancel`.
    pub fn cancel(&self, id: i64, force: bool) {
        self.running().core().cancel(id, force).unwrap();
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
            upload_chunk_limit: None,
            heartbeat: None,
            debounce: Duration::from_secs(2),
            force_after: runner::FORCE_AFTER,
            release_cooldown: Duration::ZERO,
            max_demo_bytes: runner::MAX_DEMO_BYTES,
            admission: None,
            observer: None,
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

/// Ждёт, пока условие не станет истинным; `false` — не дождались за `secs` секунд.
pub fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    cond()
}

fn cancelled_error() -> RunError {
    RunError {
        message: pipeline::CANCELLED.to_owned(),
        fail: None,
    }
}

/// Исполнитель-заглушка: ставит этап `record` и «дубль 2 из 3», ждёт отмены (не дольше 10 с) и сообщает, когда
/// увидел мягкую и жёсткую отмену.
pub struct WaitForCancel {
    /// Пауза до этапа `record`: сервер успевает показать `precheck`.
    pub delay: Duration,
    pub calls: AtomicUsize,
    pub cancel_seen: Mutex<Option<Instant>>,
    pub force_seen: Mutex<Option<Instant>>,
}

impl WaitForCancel {
    pub fn new(delay: Duration) -> Arc<WaitForCancel> {
        Arc::new(WaitForCancel {
            delay,
            calls: AtomicUsize::new(0),
            cancel_seen: Mutex::new(None),
            force_seen: Mutex::new(None),
        })
    }
}

impl Executor for WaitForCancel {
    fn run(&self, _task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        p.stage(pipeline::Stage::Record);
        p.event(&Event::State {
            state: RecordState::Take,
            take: Some((2, 3)),
        });
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if p.force_cancelled() {
                self.force_seen
                    .lock()
                    .unwrap()
                    .get_or_insert(Instant::now());
            }
            if p.cancelled() {
                self.cancel_seen
                    .lock()
                    .unwrap()
                    .get_or_insert(Instant::now());
                return Err(cancelled_error());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(RunError {
            message: "заглушка не дождалась отмены".to_owned(),
            fail: None,
        })
    }
}

/// Исполнитель-заглушка, который не замечает мягкой отмены: ждёт только жёсткой.
pub struct IgnoreSoftCancel {
    pub soft_seen: Mutex<Option<Instant>>,
    pub force_seen: Mutex<Option<Instant>>,
}

impl IgnoreSoftCancel {
    pub fn new() -> Arc<IgnoreSoftCancel> {
        Arc::new(IgnoreSoftCancel {
            soft_seen: Mutex::new(None),
            force_seen: Mutex::new(None),
        })
    }
}

impl Executor for IgnoreSoftCancel {
    fn run(&self, _task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        p.stage(pipeline::Stage::Record);
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if p.cancelled() {
                self.soft_seen.lock().unwrap().get_or_insert(Instant::now());
            }
            if p.force_cancelled() {
                *self.force_seen.lock().unwrap() = Some(Instant::now());
                return Err(cancelled_error());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(RunError {
            message: "заглушка не дождалась жёсткой отмены".to_owned(),
            fail: None,
        })
    }
}

/// Исполнитель-заглушка, у которого флаг остановки раннера поднимается посреди работы.
pub struct StopMidway {
    pub flag: Mutex<Option<Arc<AtomicBool>>>,
    pub calls: AtomicUsize,
    pub force_seen: AtomicBool,
}

impl StopMidway {
    pub fn new() -> Arc<StopMidway> {
        Arc::new(StopMidway {
            flag: Mutex::new(None),
            calls: AtomicUsize::new(0),
            force_seen: AtomicBool::new(false),
        })
    }
}

impl Executor for StopMidway {
    fn run(&self, _task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        p.stage(pipeline::Stage::Record);
        let flag = self.flag.lock().unwrap().clone().expect("флаг не задан");
        flag.store(true, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until && !p.cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.force_seen.store(p.force_cancelled(), Ordering::SeqCst);
        Err(cancelled_error())
    }
}

/// Исполнитель-заглушка: отдаёт готовый результат, но перед этим отменяет задачу на сервере и ждёт, пока раннер
/// узнает об отмене, — загрузка начинается уже при отмене.
pub struct FinishThenCancel {
    dir: PathBuf,
    core: Core,
    size: usize,
}

impl FinishThenCancel {
    pub fn new(dir: PathBuf, core: Core, size: usize) -> Arc<FinishThenCancel> {
        Arc::new(FinishThenCancel { dir, core, size })
    }
}

impl Executor for FinishThenCancel {
    fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        p.stage(pipeline::Stage::Assemble);
        let out_dir = self.dir.join("out").join(format!("srv{}", task.job_id));
        std::fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        std::fs::write(&result, result_bytes(self.size)).unwrap();
        self.core.cancel(task.job_id, false).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !p.cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(Outcome {
            out_dir,
            result,
            shots: 1,
            secs: 1.0,
        })
    }
}
