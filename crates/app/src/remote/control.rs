//! Поток раннера приложения: замок рабочей папки, токен, ворота допуска по виду очереди и наблюдатель, который ведёт
//! состояние для окна.
//!
//! Приложение — единственный хозяин потока раннера (`cs2cinema-runner`). Адрес сервера и токен применяются перезапуском
//! потока, но не во время открытой карточки сервера: остановка раннера при идущей карточке отменила бы её, а задача друга
//! должна дожить до конца. Без адреса потока нет и сетевых запросов нет, очередь ведёт локальные задачи как в v1.0.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use pipeline::{host::Host, inspect::InspectLimits};
use runner::{Admission, Config, Control, Gate, InspectSettings, Observer, PipelineExecutor, RunnerEvent};
use store::Store;

use super::{RemoteExecutor, RemoteHub, external_ref, hub::ExecFactory, token};
use crate::{
    jobs,
    queue::{Queue, QueueView},
    settings::{self, Settings},
};

/// Файл замка рабочей папки: тот же, что у `cs2-cli runner`, поэтому приложение и консольный раннер не работают вместе.
const LOCK_FILE: &str = "runner.lock";

/// Состояние раннера для окна и строки статуса.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteStatus {
    /// Адрес сервера не задан.
    Off,
    /// Нет файла токена.
    NoToken,
    /// Файл токена не читается или в нём не токен.
    BadToken(String),
    /// Нет `cs2-cli.exe` рядом с приложением (путь, где его искали).
    NoCli(PathBuf),
    /// Папку раннера уже держит другой процесс.
    LockBusy,
    /// Адрес не принят (`Link::new`, создание папки).
    BadAddress(String),
    /// Поток раннера запущен, сервер ещё не ответил.
    Starting,
    /// На связи, задач нет.
    Online { server_version: String },
    /// На связи, но новые задачи не берутся; причина по-русски.
    Gated { server_version: String, reason: String },
    /// Выполняется задача сервера.
    Working { job_id: i64 },
    /// Нет связи с сервером.
    NetError { message: String, retry_secs: u64 },
    /// Раннер на паузе после повторяющихся неисправностей.
    Paused { code: String, message: String },
    /// Сервер не принял токен.
    TokenRejected,
    /// Версия протокола не совпала с сервером.
    ProtocolMismatch(String),
    /// Раннер остановлен.
    Stopped,
}

/// Как показывать состояние: цвет значка и текста в окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    /// Режим выключен или раннер остановлен.
    Off,
    /// Всё работает.
    Ok,
    /// Раннер ждёт: нужно внимание, но не сбой.
    Warn,
    /// Раннер не работает по причине, которую исправляет автор.
    Bad,
}

impl RemoteStatus {
    /// Полное описание для страницы настроек (заглушка до реализации).
    pub fn text(&self) -> String {
        String::new()
    }

    /// Короткая строка для боковой панели (заглушка до реализации).
    pub fn short(&self) -> String {
        String::new()
    }

    /// Уровень состояния (заглушка до реализации).
    pub fn level(&self) -> StatusLevel {
        StatusLevel::Off
    }
}

/// Настройки интервалов: в тестах короткие, у приложения — по умолчанию.
#[derive(Debug, Clone)]
pub struct RemoteTuning {
    pub poll_secs: Option<u64>,
    pub heartbeat: Option<Duration>,
    pub debounce: Duration,
    pub release_cooldown: Duration,
}

impl Default for RemoteTuning {
    fn default() -> Self {
        RemoteTuning { poll_secs: Some(5), heartbeat: None, debounce: Duration::from_secs(2), release_cooldown: runner::RELEASE_COOLDOWN }
    }
}

/// Всё, что нужно раннеру приложения снаружи.
#[derive(Clone)]
pub struct RemoteDeps {
    pub store: Store,
    pub host: Arc<dyn Host>,
    pub queue: Queue,
    pub hub: Arc<RemoteHub>,
    /// Рабочая папка раннера: токен, замок, демки и результаты задач сервера.
    pub dir: PathBuf,
    /// `Ok` — путь к `cs2-cli`, `Err` — где его искали и не нашли.
    pub inspect_exe: Result<PathBuf, PathBuf>,
    /// Имя раннера для журнала сервера.
    pub name: String,
    pub settings: Arc<dyn Fn() -> Settings + Send + Sync>,
    /// Перерисовать окно: состояние изменилось.
    pub repaint: Arc<dyn Fn() + Send + Sync>,
    pub tuning: RemoteTuning,
}

/// Рабочая папка раннера: та же, что у `cs2-cli runner` по умолчанию.
pub fn runner_dir() -> PathBuf {
    settings::data_dir().join("runner")
}

/// `cs2-cli` рядом с исполняемым файлом приложения: `Ok` — найден, `Err` — путь, где его искали.
pub fn inspect_exe() -> Result<PathBuf, PathBuf> {
    let name = format!("cs2-cli{}", std::env::consts::EXE_SUFFIX);
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let exe = dir.join(name);
    if exe.is_file() { Ok(exe) } else { Err(exe) }
}

/// Фабрика исполнителей удалённых карточек: настоящий `PipelineExecutor`, настройки читаются заново на каждый запуск.
pub fn app_factory(settings: Arc<dyn Fn() -> Settings + Send + Sync>, host: Arc<dyn Host>, dir: PathBuf, exe: Result<PathBuf, PathBuf>) -> ExecFactory {
    Arc::new(move || {
        let exe = exe.clone().map_err(|p| format!("не найден cs2-cli.exe рядом с приложением: {}", p.display()))?;
        let cfg = pipeline::job::JobConfig { out_root: dir.join("out"), ..jobs::job_config(&settings()) };
        let inspect = InspectSettings { exe, limits: InspectLimits::default() };
        Ok(Arc::new(PipelineExecutor::new(host.clone(), cfg, inspect)) as Arc<dyn runner::Executor>)
    })
}

/// Ворота раннера по виду очереди приложения. Чистая функция: причины по-русски и без точки, проверка по порядку перечисления.
pub fn admission(view: &QueueView, local_waiting: bool, remote_open: bool, env_ready: bool) -> Admission {
    let why = if view.running.is_some() {
        "идёт задача приложения"
    } else if view.paused.is_some() {
        "очередь приложения на паузе"
    } else if view.hold.is_some() {
        "очередь ждёт авто-повтора"
    } else if local_waiting {
        "в очереди приложения есть задачи"
    } else if remote_open {
        "задача с сервера ещё в работе"
    } else if !env_ready {
        "в настройках не заданы папки HLAE и CS2"
    } else {
        return Admission::Open;
    };
    Admission::Closed(why.to_owned())
}

/// Состояние, общее с потоком раннера. Поколение отсекает запоздавшие события остановленного потока.
struct Shared {
    status: Mutex<(u64, RemoteStatus)>,
    version: Mutex<String>,
}

impl Shared {
    /// Новое поколение с начальным состоянием; возвращает его номер.
    fn begin(&self, status: RemoteStatus) -> u64 {
        let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
        s.0 += 1;
        s.1 = status;
        s.0
    }

    fn is_current(&self, generation: u64) -> bool {
        self.status.lock().is_ok_and(|s| s.0 == generation)
    }

    /// Записывает состояние, если поколение ещё актуально.
    fn set(&self, generation: u64, status: RemoteStatus) {
        let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if s.0 == generation {
            s.1 = status;
        }
    }

    fn get(&self) -> RemoteStatus {
        self.status.lock().map(|s| s.1.clone()).unwrap_or(RemoteStatus::Stopped)
    }

    fn version(&self) -> String {
        self.version.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

/// Поток раннера и ручки его остановки.
struct Handle {
    control: Arc<Mutex<Option<Control>>>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Handle {
    /// Просит поток остановиться: сам раннер (если уже создан) и флаг для потока, который ещё готовится.
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(c) = self.control.lock() {
            if let Some(c) = c.as_ref() {
                c.stop();
            }
        }
    }

    fn finished(&self) -> bool {
        self.thread.is_finished()
    }
}

/// Раннер приложения: владеет потоком `cs2cinema-runner`.
pub struct RemoteRunner {
    deps: RemoteDeps,
    shared: Arc<Shared>,
    handle: Option<Handle>,
    /// Адрес, с которым раннер сейчас запущен (обрезанный).
    server: String,
}

impl RemoteRunner {
    pub fn new(deps: RemoteDeps) -> RemoteRunner {
        let shared = Arc::new(Shared { status: Mutex::new((0, RemoteStatus::Off)), version: Mutex::new(String::new()) });
        RemoteRunner { deps, shared, handle: None, server: String::new() }
    }

    /// Применить адрес сервера: пусто — раннер выключен, иначе поток раннера (пере)запускается.
    pub fn apply(&mut self, server: &str) {
        self.start(server.trim().to_owned());
    }

    /// Раз в кадр окна. Сюда же приходит применение отложенного адреса.
    pub fn tick(&mut self) {
        if self.handle.as_ref().is_some_and(|h| h.finished()) && self.server.is_empty() {
            self.handle = None;
        }
    }

    pub fn status(&self) -> RemoteStatus {
        self.shared.get()
    }

    /// Жив ли поток раннера (в том числе уже остановленный, но ещё не вышедший).
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.finished())
    }

    /// Есть ли открытая карточка сервера: пока она есть, раннер не останавливается.
    pub fn busy(&self) -> bool {
        self.deps.hub.has_slots()
    }

    /// Есть ли отложенная смена адреса или токена (заглушка до реализации).
    pub fn pending(&self) -> bool {
        false
    }

    /// Снять паузу раннера (заглушка до реализации).
    pub fn resume_paused(&self) {}

    /// Переподключиться с текущим адресом (заглушка до реализации).
    pub fn restart(&mut self) {}

    /// Сохранить токен и переподключиться (заглушка до реализации).
    pub fn save_token(&mut self, _value: &str) -> Result<(), String> {
        Err("не реализовано".to_owned())
    }

    /// Удалить токен и переподключиться (заглушка до реализации).
    pub fn clear_token(&mut self) -> Result<(), String> {
        Err("не реализовано".to_owned())
    }

    /// Есть ли файл токена (заглушка до реализации).
    pub fn has_token(&self) -> bool {
        false
    }

    pub fn hub(&self) -> &Arc<RemoteHub> {
        &self.deps.hub
    }

    pub fn dir(&self) -> &Path {
        &self.deps.dir
    }

    fn start(&mut self, server: String) {
        let off = server.is_empty();
        let generation = self.shared.begin(if off { RemoteStatus::Off } else { RemoteStatus::Starting });
        self.server = server.clone();
        let previous = self.handle.take();
        if let Some(h) = &previous {
            h.stop();
        }
        if off {
            self.deps.hub.set_held_ref(None);
            self.handle = previous;
            (self.deps.repaint)();
            return;
        }
        let control = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let args = ThreadArgs {
            deps: self.deps.clone(),
            shared: self.shared.clone(),
            generation,
            server,
            previous: previous.map(|h| h.thread),
            control: control.clone(),
            stop: stop.clone(),
        };
        match std::thread::Builder::new().name("cs2cinema-runner".into()).spawn(move || run_thread(args)) {
            Ok(thread) => self.handle = Some(Handle { control, stop, thread }),
            Err(e) => self.shared.set(generation, RemoteStatus::BadAddress(format!("поток раннера не запущен: {e}"))),
        }
        (self.deps.repaint)();
    }
}

/// Аргументы потока раннера.
struct ThreadArgs {
    deps: RemoteDeps,
    shared: Arc<Shared>,
    generation: u64,
    server: String,
    previous: Option<JoinHandle<()>>,
    control: Arc<Mutex<Option<Control>>>,
    stop: Arc<AtomicBool>,
}

/// Замок рабочей папки; `None` — папку держит другой процесс.
fn lock_dir(dir: &Path) -> Result<Option<fs::File>, String> {
    fs::create_dir_all(dir).map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
    let path = dir.join(LOCK_FILE);
    let file = fs::File::create(&path).map_err(|e| format!("не удалось открыть {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(format!("не удалось занять {}: {e}", path.display())),
    }
}

/// Ворота допуска: вид очереди приложения, ждущие локальные задачи, открытая карточка сервера, папки HLAE и CS2.
fn gate(deps: &RemoteDeps) -> Gate {
    let (store, queue, hub, settings) = (deps.store.clone(), deps.queue.clone(), deps.hub.clone(), deps.settings.clone());
    Arc::new(move || {
        let local_waiting = match store.next_queued() {
            // удалённая карточка уже в хабе — не «локальная задача», её причину называет `remote_open`
            Ok(job) => job.is_some_and(|j| !hub.is_held(&j)),
            Err(_) => return Admission::Closed("база приложения недоступна".to_owned()),
        };
        let s = settings();
        let env_ready = !s.hlae_dir.trim().is_empty() && !s.cs2_dir.trim().is_empty();
        admission(&queue.view(), local_waiting, hub.has_slots(), env_ready)
    })
}

/// Наблюдатель раннера: ведёт состояние, ссылку удерживаемой карточки и перерисовку окна.
fn observer(deps: &RemoteDeps, shared: Arc<Shared>, generation: u64) -> Observer {
    let (hub, repaint) = (deps.hub.clone(), deps.repaint.clone());
    Arc::new(move |event, holding| {
        if !shared.is_current(generation) {
            return;
        }
        hub.set_held_ref(holding.map(|h| external_ref(h.job_id, h.epoch)));
        let status = match event {
            RunnerEvent::Connected { server_version } => {
                if let Ok(mut v) = shared.version.lock() {
                    *v = server_version.clone();
                }
                RemoteStatus::Online { server_version: server_version.clone() }
            }
            RunnerEvent::Idle | RunnerEvent::Settled { .. } | RunnerEvent::Released { .. } | RunnerEvent::Dropped { .. } => {
                RemoteStatus::Online { server_version: shared.version() }
            }
            RunnerEvent::Gated(reason) => RemoteStatus::Gated { server_version: shared.version(), reason: reason.clone() },
            RunnerEvent::Working { job_id, .. } => RemoteStatus::Working { job_id: *job_id },
            RunnerEvent::Paused(reason) => RemoteStatus::Paused { code: reason.code.clone(), message: reason.message.clone() },
            RunnerEvent::NetError { message, retry_secs } => RemoteStatus::NetError { message: message.clone(), retry_secs: *retry_secs },
            RunnerEvent::Fatal(e) if e == runner::tee::TOKEN_REJECTED => RemoteStatus::TokenRejected,
            RunnerEvent::Fatal(e) => RemoteStatus::ProtocolMismatch(e.clone()),
        };
        shared.set(generation, status);
        repaint();
    })
}

/// Тело потока раннера: дождаться прежнего потока, занять замок, собрать раннер и крутить его до остановки.
fn run_thread(a: ThreadArgs) {
    if let Some(previous) = a.previous {
        let _ = previous.join();
    }
    if a.stop.load(Ordering::SeqCst) {
        return;
    }
    let refuse = |status: RemoteStatus| {
        a.shared.set(a.generation, status);
        (a.deps.repaint)();
    };
    // файл замка живёт до конца потока
    let _lock = match lock_dir(&a.deps.dir) {
        Ok(Some(f)) => f,
        Ok(None) => return refuse(RemoteStatus::Stopped),
        Err(_) => return refuse(RemoteStatus::Stopped),
    };
    if a.deps.inspect_exe.is_err() {
        return refuse(RemoteStatus::Stopped);
    }
    let token = match token::load(&a.deps.dir) {
        Ok(Some(t)) => t,
        _ => return refuse(RemoteStatus::Stopped),
    };
    let mut cfg = Config::new(a.server.clone(), token, a.deps.dir.clone());
    cfg.name = a.deps.name.clone();
    cfg.poll_secs = a.deps.tuning.poll_secs;
    cfg.heartbeat = a.deps.tuning.heartbeat;
    cfg.debounce = a.deps.tuning.debounce;
    cfg.release_cooldown = a.deps.tuning.release_cooldown;
    cfg.admission = Some(gate(&a.deps));
    cfg.observer = Some(observer(&a.deps, a.shared.clone(), a.generation));
    let mut runner = match runner::Runner::new(cfg, Arc::new(RemoteExecutor::new(a.deps.hub.clone()))) {
        Ok(r) => r,
        Err(_) => return refuse(RemoteStatus::Stopped),
    };
    if let Ok(mut c) = a.control.lock() {
        *c = Some(runner.control());
    }
    // остановка могла прийти, пока поток готовился
    if a.stop.load(Ordering::SeqCst) {
        runner.control().stop();
    }
    if a.shared.is_current(a.generation) {
        a.deps.hub.set_held_ref(runner.holding().map(|h| external_ref(h.job_id, h.epoch)));
    }
    (a.deps.repaint)();
    if runner.run().is_ok() {
        a.shared.set(a.generation, RemoteStatus::Stopped);
        (a.deps.repaint)();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use pipeline::{FailCode, Progress, RecordFailure, RecordState, RunError, Stage, host::fake::FakeHost, job::Outcome};
    use store::{JobKind, JobOutcome, JobSource, JobStatus, NewJob};
    use wire::JobState;

    use super::*;
    use crate::{
        jobs::CancelToken,
        queue::{self, PauseReason},
        remote::testkit::{RigOpts, Stand, TestDir, fake_demo, faststart_mp4, pipeline_factory, probing_host, start_queue, wait_until},
    };

    const MIB: usize = 1024 * 1024;

    fn settings_with_paths() -> Settings {
        Settings { hlae_dir: "C:/hlae".to_owned(), cs2_dir: "C:/cs2".to_owned(), ..Settings::default() }
    }

    fn local_job() -> NewJob {
        NewJob {
            kind: JobKind::Record,
            source: JobSource::File("local.dem".into()),
            label: "своя".into(),
            hero_name: String::new(),
            hero_steam: 0,
            map: String::new(),
        }
    }

    /// Локальный исполнитель: «готово» сразу.
    fn instant_local() -> queue::Runner {
        Arc::new(|job, _| Ok(JobOutcome { out_dir: "out/local".into(), result: format!("out/local/{}.mp4", job.id).into(), shots: 1, secs: 1.0 }))
    }

    /// Локальный исполнитель, который держит очередь занятой, пока не поднят `release` (не дольше 30 с).
    fn gated_local(entered: Arc<AtomicBool>, release: Arc<AtomicBool>) -> queue::Runner {
        Arc::new(move |job, _| {
            entered.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(30);
            while !release.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(JobOutcome { out_dir: "out/local".into(), result: format!("out/local/{}.mp4", job.id).into(), shots: 1, secs: 1.0 })
        })
    }

    /// Исполнитель удалённой карточки: этап записи, ожидание `release` (не дольше 15 с), годный ролик.
    fn waiting_job_fn(
        runner_dir: PathBuf,
        release: Arc<AtomicBool>,
    ) -> impl Fn(&dyn Host, &pipeline::job::JobSpec, &dyn Progress) -> Result<Outcome, RunError> + Send + Sync + 'static {
        move |_, spec, p| {
            p.stage(Stage::Record);
            let until = Instant::now() + Duration::from_secs(15);
            while !release.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            let out_dir = runner_dir.join("out").join(format!("srv{}", spec.job_id));
            std::fs::create_dir_all(&out_dir).unwrap();
            let result = out_dir.join("highlights.mp4");
            std::fs::write(&result, faststart_mp4()).unwrap();
            Ok(Outcome { out_dir, result, shots: 1, secs: 5.0 })
        }
    }

    /// Очередь приложения и всё для `RemoteRunner` на стенде. Токен в папке раннера записан, если `with_token`.
    struct Fixture {
        host: FakeHost,
        _tools: TestDir,
        store: Store,
        hub: Arc<RemoteHub>,
        queue: Queue,
        dir: PathBuf,
        exe: Result<PathBuf, PathBuf>,
    }

    impl Fixture {
        fn new(
            stand: &Stand,
            with_token: bool,
            local: queue::Runner,
            job_fn: impl Fn(&dyn Host, &pipeline::job::JobSpec, &dyn Progress) -> Result<Outcome, RunError> + Send + Sync + 'static,
        ) -> Fixture {
            let tools = TestDir::new("tools");
            let host = probing_host();
            let factory = pipeline_factory(&host, &tools, stand.runner_dir(), job_fn);
            let (store, hub, queue) = start_queue(factory, RigOpts { local: Some(local), ..RigOpts::default() });
            let dir = stand.runner_dir();
            if with_token {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(token::TOKEN_FILE), &stand.token).unwrap();
            }
            Fixture { host, _tools: tools, store, hub, queue, dir, exe: Ok(PathBuf::from("cs2-cli")) }
        }

        fn deps(&self) -> RemoteDeps {
            let settings = settings_with_paths();
            RemoteDeps {
                store: self.store.clone(),
                host: Arc::new(self.host.clone()),
                queue: self.queue.clone(),
                hub: self.hub.clone(),
                dir: self.dir.clone(),
                inspect_exe: self.exe.clone(),
                name: "tester".to_owned(),
                settings: Arc::new(move || settings.clone()),
                repaint: Arc::new(|| {}),
                tuning: RemoteTuning {
                    poll_secs: Some(0),
                    heartbeat: Some(Duration::from_millis(200)),
                    debounce: Duration::from_millis(50),
                    release_cooldown: Duration::ZERO,
                },
            }
        }
    }

    fn view(running: bool, paused: bool, hold: bool) -> QueueView {
        QueueView {
            running: running.then(|| (1, CancelToken::default())),
            paused: paused.then_some(PauseReason::Cancelled),
            hold: hold.then(|| (1, Instant::now())),
        }
    }

    #[test]
    fn admission_closes_for_every_busy_queue_state() {
        let closed = |a: Admission| match a {
            Admission::Closed(why) => why,
            Admission::Open => panic!("ворота должны быть закрыты"),
        };
        assert_eq!(admission(&view(false, false, false), false, false, true), Admission::Open, "свободная очередь и пути есть");
        assert_eq!(closed(admission(&view(true, false, false), false, false, true)), "идёт задача приложения");
        assert_eq!(closed(admission(&view(false, true, false), false, false, true)), "очередь приложения на паузе");
        assert_eq!(closed(admission(&view(false, false, true), false, false, true)), "очередь ждёт авто-повтора");
        assert_eq!(closed(admission(&view(false, false, false), true, false, true)), "в очереди приложения есть задачи");
        assert_eq!(closed(admission(&view(false, false, false), false, true, true)), "задача с сервера ещё в работе");
        assert_eq!(closed(admission(&view(false, false, false), false, false, false)), "в настройках не заданы папки HLAE и CS2");
        assert_eq!(closed(admission(&view(true, true, true), true, true, false)), "идёт задача приложения", "при нескольких причинах называется первая");
    }

    #[test]
    fn address_and_token_make_the_app_take_a_server_job() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let release = Arc::new(AtomicBool::new(false));
        let fx = Fixture::new(&stand, true, instant_local(), waiting_job_fn(stand.runner_dir(), release.clone()));
        let mut remote = RemoteRunner::new(fx.deps());
        assert_eq!(remote.status(), RemoteStatus::Off);
        assert!(!remote.is_running(), "без адреса потока нет");

        remote.apply(&stand.base);
        assert!(remote.is_running(), "с адресом и токеном поток раннера запущен");
        let working = wait_until(20, || matches!(remote.status(), RemoteStatus::Working { job_id } if job_id == id));
        assert!(working, "за 20 с раннер не взялся за задачу #{id}: {:?}", remote.status());
        assert!(wait_until(15, || remote.busy()), "пока карточка идёт, запись в хабе открыта");
        release.store(true, Ordering::SeqCst);

        let done = wait_until(20, || stand.job(id).state == JobState::Done);
        assert!(done, "за 20 с задача сервера не стала done: {:?}", stand.job(id).state);
        let card_done = wait_until(10, || fx.store.jobs().unwrap().iter().any(|j| j.external_ref.is_some() && j.status == JobStatus::Done));
        assert!(card_done, "карточка не стала Done");
        assert!(wait_until(10, || matches!(remote.status(), RemoteStatus::Online { .. })), "после задачи раннер снова на связи: {:?}", remote.status());

        remote.apply("");
        assert!(wait_until(10, || !remote.is_running()), "поток раннера должен завершиться");
        assert_eq!(remote.status(), RemoteStatus::Off);
    }

    #[test]
    fn empty_address_starts_no_runner_and_the_queue_works_as_before() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let cancellable: queue::Runner = Arc::new(|_, rep| {
            let until = Instant::now() + Duration::from_secs(15);
            while Instant::now() < until {
                if rep.cancelled() {
                    return Err(RunError::from(pipeline::CANCELLED));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(RunError::from("отмены за 15 с не дождались"))
        });
        let fx = Fixture::new(&stand, true, cancellable, |_, _, _| Err(RunError::from("удалённая карточка в этом тесте не ожидалась")));
        let mut remote = RemoteRunner::new(fx.deps());
        for empty in ["", "   "] {
            remote.apply(empty);
            assert!(!remote.is_running(), "по адресу {empty:?} поток раннера не создаётся");
            assert_eq!(remote.status(), RemoteStatus::Off);
        }
        assert!(!remote.busy());
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!((stand.job(id).state, stand.job(id).grants), (JobState::Queued, 0), "без адреса сервер приложение не слышит");

        // локальная задача идёт как в v1.0: отмена идущей ставит паузу
        let local = fx.store.add_job(&local_job()).unwrap();
        assert!(wait_until(10, || fx.queue.view().running.is_some_and(|(running, _)| running == local)), "локальная задача не началась");
        let (_, token) = fx.queue.view().running.unwrap();
        token.cancel();
        fx.store.set_cancelling(local).unwrap();
        assert!(wait_until(10, || fx.store.job(local).unwrap().unwrap().status == JobStatus::Cancelled), "локальная задача не отменилась");
        assert!(wait_until(5, || fx.queue.view().paused == Some(PauseReason::Cancelled)), "отмена автором ставит очередь на паузу, как в v1.0");
    }

    #[test]
    fn local_job_that_finishes_reaches_done_with_the_hub_connected() {
        let stand = Stand::start();
        let fx = Fixture::new(&stand, false, instant_local(), |_, _, _| Err(RunError::from("удалённая карточка в этом тесте не ожидалась")));
        let local = fx.store.add_job(&local_job()).unwrap();
        assert!(wait_until(10, || fx.store.job(local).unwrap().unwrap().status == JobStatus::Done), "локальная задача не дошла до Done");
        assert_eq!(fx.queue.view().paused, None);
    }

    #[test]
    fn local_job_closes_the_gate_and_the_server_job_waits() {
        let stand = Stand::start();
        let (entered, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let remote_release = Arc::new(AtomicBool::new(true));
        let fx = Fixture::new(&stand, true, gated_local(entered.clone(), release.clone()), waiting_job_fn(stand.runner_dir(), remote_release));
        fx.store.add_job(&local_job()).unwrap();
        assert!(wait_until(10, || entered.load(Ordering::SeqCst)), "локальная задача не заняла очередь");
        let id = stand.submit(&fake_demo(MIB), "s1mple");

        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        let gated = wait_until(10, || matches!(remote.status(), RemoteStatus::Gated { ref reason, .. } if reason == "идёт задача приложения"));
        assert!(gated, "ворота должны назвать причину: {:?}", remote.status());
        std::thread::sleep(Duration::from_secs(2));
        let job = stand.job(id);
        assert_eq!((job.state, job.grants), (JobState::Queued, 0), "пока идёт локальная задача, сервер задачу не выдаёт");

        release.store(true, Ordering::SeqCst);
        let done = wait_until(30, || stand.job(id).state == JobState::Done);
        assert!(done, "после локальной задачи задача сервера не дошла до done: {:?}", stand.job(id).state);
    }
    #[test]
    fn refusals_before_start_name_their_reason() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let mut fx = Fixture::new(&stand, false, instant_local(), |_, _, _| Err(RunError::from("карточка в этом тесте не ожидалась")));

        // нет файла токена
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert!(wait_until(5, || !remote.is_running()), "без токена поток раннера не работает");
        assert_eq!(remote.status(), RemoteStatus::NoToken);

        // содержимое файла токена не годится
        std::fs::create_dir_all(&fx.dir).unwrap();
        std::fs::write(fx.dir.join(token::TOKEN_FILE), "ab cd").unwrap();
        remote.apply(&stand.base);
        assert!(wait_until(5, || !remote.is_running()));
        assert!(matches!(remote.status(), RemoteStatus::BadToken(ref why) if !why.contains("ab cd")), "{:?}", remote.status());

        // публичный адрес по http
        std::fs::write(fx.dir.join(token::TOKEN_FILE), &stand.token).unwrap();
        remote.apply("http://8.8.8.8:1");
        assert!(wait_until(5, || !remote.is_running() && matches!(remote.status(), RemoteStatus::BadAddress(_))), "{:?}", remote.status());

        // нет cs2-cli.exe рядом с приложением
        let missing = PathBuf::from("C:/app/cs2-cli.exe");
        fx.exe = Err(missing.clone());
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert!(wait_until(5, || !remote.is_running()));
        assert_eq!(remote.status(), RemoteStatus::NoCli(missing));

        let job = stand.job(id);
        assert_eq!((job.state, job.grants), (JobState::Queued, 0), "ни одного запроса за задачей к серверу");
    }

    #[test]
    fn saving_a_token_restarts_the_runner() {
        let stand = Stand::start();
        let fx = Fixture::new(&stand, false, instant_local(), |_, _, _| Err(RunError::from("карточка в этом тесте не ожидалась")));
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert_eq!(remote.status(), RemoteStatus::NoToken);
        assert!(!remote.has_token());

        remote.save_token(&stand.token).expect("токен должен сохраниться");
        assert!(remote.has_token());
        let online = wait_until(10, || matches!(remote.status(), RemoteStatus::Online { .. } | RemoteStatus::Working { .. }));
        assert!(online, "после сохранения токена раннер должен выйти на связь: {:?}", remote.status());

        remote.clear_token().expect("токен должен удалиться");
        assert!(!remote.has_token());
        assert!(wait_until(10, || !remote.is_running()), "без токена поток раннера завершается");
        assert_eq!(remote.status(), RemoteStatus::NoToken);
    }

    #[test]
    fn second_runner_in_the_same_folder_is_refused() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let fx = Fixture::new(&stand, true, instant_local(), |_, _, _| Err(RunError::from("карточка в этом тесте не ожидалась")));
        // другой процесс (например, `cs2-cli runner`) держит замок папки
        let other = std::fs::File::create(fx.dir.join(LOCK_FILE)).unwrap();
        other.try_lock().expect("замок свободен");

        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert!(wait_until(5, || !remote.is_running()), "при занятом замке поток раннера завершается");
        assert_eq!(remote.status(), RemoteStatus::LockBusy);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(stand.job(id).state, JobState::Queued, "задача на сервере остаётся в очереди");
    }

    #[test]
    fn revoked_token_shows_token_rejected() {
        let stand = Stand::start();
        let fx = Fixture::new(&stand, true, instant_local(), |_, _, _| Err(RunError::from("карточка в этом тесте не ожидалась")));
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert!(wait_until(10, || matches!(remote.status(), RemoteStatus::Online { .. })), "{:?}", remote.status());

        let core = stand.core();
        server::auth::revoke_runner(core.db(), "tester", core.clock().now_ms()).unwrap();
        assert!(wait_until(15, || remote.status() == RemoteStatus::TokenRejected), "{:?}", remote.status());
        assert!(wait_until(5, || !remote.is_running()), "при отозванном токене поток раннера завершается");
    }

    #[test]
    fn address_change_waits_for_the_server_card() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let release = Arc::new(AtomicBool::new(false));
        let fx = Fixture::new(&stand, true, instant_local(), waiting_job_fn(stand.runner_dir(), release.clone()));
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);
        assert!(wait_until(20, || remote.busy()), "карточка сервера не появилась: {:?}", remote.status());

        remote.apply("http://127.0.0.1:1");
        assert!(remote.pending(), "смена адреса отложена до закрытия карточки");
        assert!(remote.is_running(), "раннер не остановлен");
        remote.tick();
        assert!(remote.pending(), "пока карточка идёт, адрес не применяется");
        let card = fx.store.jobs().unwrap().into_iter().find(|j| j.external_ref.is_some()).expect("карточка есть");
        assert_ne!(card.status, JobStatus::Cancelled, "карточка не отменена");
        assert_ne!(stand.job(id).state, JobState::Cancelled);

        release.store(true, Ordering::SeqCst);
        assert!(wait_until(20, || stand.job(id).state == JobState::Done), "задача на сервере не стала done: {:?}", stand.job(id).state);
        assert!(wait_until(10, || !remote.busy()), "запись карточки закрыта");
        assert!(
            wait_until(10, || {
                remote.tick();
                !remote.pending()
            }),
            "отложенный адрес применяется после закрытия карточки"
        );
        assert!(wait_until(10, || matches!(remote.status(), RemoteStatus::Starting | RemoteStatus::NetError { .. })), "{:?}", remote.status());
        assert!(wait_until(10, || fx.store.job(card.id).unwrap().unwrap().status == JobStatus::Done), "карточка дошла до Done");
    }

    #[test]
    fn server_outcome_is_written_into_the_card_journal() {
        let stand = Stand::start();
        let id = stand.submit(&fake_demo(MIB), "s1mple");
        let calls = Arc::new(AtomicUsize::new(0));
        let runner_dir = stand.runner_dir();
        let job_fn = {
            let calls = calls.clone();
            let ok = waiting_job_fn(runner_dir, Arc::new(AtomicBool::new(true)));
            move |host: &dyn Host, spec: &pipeline::job::JobSpec, p: &dyn Progress| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let f = RecordFailure::new(FailCode::PrevGameStuck, RecordState::Precheck, "прежняя игра ещё закрывается".to_owned(), Duration::ZERO);
                    return Err(RunError { message: f.message.clone(), fail: Some(f) });
                }
                ok(host, spec, p)
            }
        };
        let fx = Fixture::new(&stand, true, instant_local(), job_fn);
        let mut remote = RemoteRunner::new(fx.deps());
        remote.apply(&stand.base);

        let done = wait_until(30, || stand.job(id).state == JobState::Done);
        assert!(done, "после возврата задача должна дойти до done: {:?}, вызовов {}", stand.job(id).state, calls.load(Ordering::SeqCst));
        let logs = |fx: &Fixture| fx.store.jobs().unwrap().into_iter().filter(|j| j.external_ref.is_some()).map(|j| j.log).collect::<Vec<_>>().join("\n---\n");
        assert!(wait_until(10, || logs(&fx).contains("результат принят сервером")), "{}", logs(&fx));
        let all = logs(&fx);
        assert!(all.contains("локальная копия"), "{all}");
        assert!(all.contains("возвращена в очередь сервера"), "{all}");
        assert_eq!(fx.queue.view().paused, None, "один сбой окружения очередь на паузу не ставит");
    }

    #[test]
    fn status_texts_are_russian_and_short() {
        use RemoteStatus::*;
        let all = [
            Off,
            NoToken,
            BadToken("ошибка".into()),
            NoCli(PathBuf::from("C:/app/cs2-cli.exe")),
            LockBusy,
            BadAddress("ошибка".into()),
            Starting,
            Online { server_version: "0.1.0".into() },
            Gated { server_version: "0.1.0".into(), reason: "идёт задача приложения".into() },
            Working { job_id: 7 },
            NetError { message: "нет связи".into(), retry_secs: 4 },
            Paused { code: "CONSOLE_NOT_OPEN".into(), message: "консоль не открылась".into() },
            TokenRejected,
            ProtocolMismatch("версия".into()),
            Stopped,
        ];
        for s in &all {
            let (text, short) = (s.text(), s.short());
            let first = text.chars().next().expect("текст состояния не пуст");
            assert!(('А'..='Я').contains(&first) || first == 'Ё', "{s:?}: текст начинается не с заглавной русской буквы: {text}");
            assert!(short.starts_with("Сервер:") && short.chars().count() <= 32, "{s:?}: короткая строка «{short}»");
        }
        let level = |s: RemoteStatus| s.level();
        for bad in [NoCli(PathBuf::new()), LockBusy, BadAddress(String::new()), BadToken(String::new()), TokenRejected, ProtocolMismatch(String::new())] {
            assert_eq!(level(bad.clone()), StatusLevel::Bad, "{bad:?}");
        }
        for warn in [
            NoToken,
            NetError { message: String::new(), retry_secs: 1 },
            Paused { code: String::new(), message: String::new() },
            Gated { server_version: String::new(), reason: String::new() },
        ] {
            assert_eq!(level(warn.clone()), StatusLevel::Warn, "{warn:?}");
        }
        for ok in [Online { server_version: String::new() }, Working { job_id: 1 }, Starting] {
            assert_eq!(level(ok.clone()), StatusLevel::Ok, "{ok:?}");
        }
        assert_eq!((Off.level(), Stopped.level()), (StatusLevel::Off, StatusLevel::Off));
        assert!(Working { job_id: 7 }.text().contains("#7") && Working { job_id: 7 }.short().contains("#7"));
        assert!(NoCli(PathBuf::from("C:/app/cs2-cli.exe")).text().contains("cs2-cli.exe"));
    }
}
