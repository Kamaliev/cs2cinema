//! Стенд тестов режима раннера: настоящий `server::serve` в процессе теста, `FakeHost` вместо игры и ffprobe,
//! настоящий поток очереди приложения и поток раннера. Собран по образцу стенда тестов крейта `runner`
//! (общий крейт не выделяется).

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use pipeline::{
    Progress, RunError,
    host::{
        CmdOutput, Host,
        fake::{FakeHost, Scenario},
    },
    inspect::InspectLimits,
    job::{JobConfig, JobSpec, Outcome},
};
use runner::{Config, Control, InspectSettings, PipelineExecutor, Token};
use server::{
    Running, auth,
    clock::SystemClock,
    config::Config as ServerConfig,
    core::{Core, JobRow, Submit},
    log::MemorySink,
};
use store::{Job, Store};

use super::hub::{ExecFactory, RemoteExecutor, RemoteHub, queue_runner};
use crate::{
    app::Reporter,
    queue::{self, Queue, QueueConfig},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Ответ дочернего `cs2-cli inspect` для годной демки.
pub const OK_REPLY: &str = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"moments":3,"utility":1,"peak_mem_bytes":1048576}"#;

/// Ответ ffprobe для годного ролика: H.264, yuv420p, звук, mp4, длительность 5 с.
pub const GOOD_PROBE: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"5.000000"}}"#;

/// Временная папка теста; удаляется вместе со значением.
pub struct TestDir {
    pub path: PathBuf,
}

impl TestDir {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("cs2app-{tag}-{}-{n}", std::process::id()));
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
        let dir = TestDir::new("stand");
        let cfg = ServerConfig { bind: "127.0.0.1:0".to_owned(), data_dir: dir.path.join("data"), threads: 4, longpoll_secs: 1, ..Default::default() };
        let running = server::serve(&cfg, Arc::new(SystemClock), Arc::new(MemorySink::default())).expect("сервер должен запуститься");
        let base = format!("http://{}", running.addr());
        let core = running.core();
        let (_, token) = auth::add_runner(core.db(), "tester", core.clock().now_ms()).unwrap();
        Stand { dir, running: Some(running), base, token }
    }

    fn running(&self) -> &Running {
        self.running.as_ref().unwrap()
    }

    /// Копия ядра сервера: тест может смотреть и менять задачи напрямую.
    pub fn core(&self) -> Core {
        self.running().core().clone()
    }

    /// Ставит демку в очередь сервера (как `cs2-server submit`) и возвращает номер задачи. Имя файла враждебное:
    /// в путях раннера оно участвовать не должно.
    pub fn submit(&self, bytes: &[u8], player: &str) -> i64 {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let src = self.dir.path.join(format!("in-{n}.dem"));
        std::fs::write(&src, bytes).unwrap();
        let sub =
            Submit {
                player: player.to_owned(), requested_by: "оператор".to_owned(), orig_name: "..\\..\\evil name.dem".to_owned(), ..Default::default()
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
        self.dir.path.join("data").join("blobs").join("results").join(id.to_string())
    }

    /// Рабочая папка раннера.
    pub fn runner_dir(&self) -> PathBuf {
        self.dir.path.join("runner")
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        if let Some(r) = self.running.take() {
            r.stop();
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

/// Бокс MP4 верхнего уровня: размер, тип, полезная нагрузка.
fn mp4_box(kind: &[u8; 4], payload: usize) -> Vec<u8> {
    let mut b = ((8 + payload) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend(std::iter::repeat_n(0xAB, payload));
    b
}

/// Ролик с `moov` перед `mdat` (faststart): проверка ролика его пропускает.
pub fn faststart_mp4() -> Vec<u8> {
    [b"ftyp", b"moov", b"mdat"].iter().flat_map(|k| mp4_box(k, 64)).collect()
}

fn exit0() -> Result<CmdOutput, pipeline::host::HostError> {
    Ok(CmdOutput { code: Some(0), stderr_tail: Vec::new(), elapsed: Duration::ZERO })
}

/// Хост, в котором дочерняя предпроверка отвечает «демка годна», а ffprobe — «ролик годен».
pub fn probing_host() -> FakeHost {
    FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()).program("ffprobe", &[GOOD_PROBE], exit0()))
}

/// Сколько раз хост запускал дочернюю предпроверку `cs2-cli inspect`.
pub fn inspect_runs(host: &FakeHost) -> usize {
    host.runs().iter().filter(|(_, program, args)| program.contains("cs2-cli") && args.first().is_some_and(|a| a == "inspect")).count()
}

/// Конфиг задачи с пустыми временными папками: до CS2 дело не доходит или он целиком в `FakeHost`.
fn healthy_config(tools: &TestDir) -> JobConfig {
    for f in ["hlae/HLAE.exe", "hlae/x64/AfxHookSource2.dll", "hlae/ffmpeg/bin/ffmpeg.exe", "cs2/game/bin/win64/cs2.exe"] {
        let path = tools.path.join("tools").join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
    JobConfig { hlae_dir: tools.path.join("tools/hlae"), cs2_dir: tools.path.join("tools/cs2"), out_root: tools.path.join("tools/out"), ..JobConfig::default() }
}

/// Фабрика исполнителей карточки: настоящий `PipelineExecutor` на `FakeHost`, вместо `run_job` — `job_fn`.
pub fn pipeline_factory(
    host: &FakeHost,
    tools: &TestDir,
    runner_dir: PathBuf,
    job_fn: impl Fn(&dyn Host, &JobSpec, &dyn Progress) -> Result<Outcome, RunError> + Send + Sync + 'static,
) -> ExecFactory {
    let host = host.clone();
    let cfg = JobConfig { out_root: runner_dir.join("out"), ..healthy_config(tools) };
    let job_fn = Arc::new(job_fn);
    Arc::new(move || {
        let f = job_fn.clone();
        let inspect = InspectSettings { exe: PathBuf::from("cs2-cli"), limits: InspectLimits::default() };
        let exec = PipelineExecutor::new(Arc::new(host.clone()), cfg.clone(), inspect).with_job_fn(move |h, spec, p| f(h, spec, p));
        Ok(Arc::new(exec) as Arc<dyn runner::Executor>)
    })
}

/// Очередь в тестах: такт 10 мс, ролик не открывается, пауза авто-повтора 50 мс.
pub fn queue_cfg() -> QueueConfig {
    QueueConfig { tick: Duration::from_millis(10), open_results: false, retry_pause: Duration::from_millis(50) }
}

/// Ждёт, пока условие не станет истинным; `false` — не дождались за `secs` секунд.
pub fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cond()
}

/// База, хаб, настоящий поток очереди и поток раннера на одном стенде.
pub(crate) struct Rig {
    pub store: Store,
    pub hub: Arc<RemoteHub>,
    pub queue: Queue,
    control: Control,
    thread: Option<JoinHandle<()>>,
}

impl Rig {
    /// `local` — исполнитель локальных задач очереди (и закрытых карточек), `factory` — исполнитель удалённых.
    pub fn start(stand: &Stand, factory: ExecFactory, local: queue::Runner) -> Rig {
        let store = Store::in_memory().unwrap();
        let hub = RemoteHub::new(store.clone());
        let host: Arc<dyn Host> = Arc::new(FakeHost::new(Scenario::happy(1)));
        let queue =
            queue::start_with_hook(store.clone(), host, queue_runner(local, hub.clone(), factory), Reporter::detached(), None, queue_cfg(), Some(hub.clone()));

        let mut cfg = Config::new(stand.base.clone(), Token::new(stand.token.clone()), stand.runner_dir());
        cfg.name = "tester".to_owned();
        cfg.poll_secs = Some(0);
        cfg.heartbeat = Some(Duration::from_millis(200));
        cfg.debounce = Duration::from_millis(50);
        cfg.release_cooldown = Duration::ZERO;
        let mut runner = runner::Runner::new(cfg, Arc::new(RemoteExecutor::new(hub.clone()))).expect("раннер должен создаться");
        let control = runner.control();
        let thread = std::thread::Builder::new()
            .name("cs2app-runner".into())
            .spawn(move || {
                let _ = runner.run();
            })
            .expect("поток раннера");
        Rig { store, hub, queue, control, thread: Some(thread) }
    }

    /// Карточка выдачи задачи сервера с этим номером (по `external_ref = srv:{id}:…`).
    pub fn card_for(&self, job_id: i64) -> Option<Job> {
        let prefix = format!("srv:{job_id}:");
        self.store.jobs().unwrap().into_iter().find(|j| j.external_ref.as_deref().is_some_and(|r| r.starts_with(&prefix)))
    }

    /// Останавливает поток раннера и ждёт его; `true` — поток завершился без паники.
    pub fn stop(&mut self) -> bool {
        self.control.stop();
        self.thread.take().is_none_or(|t| t.join().is_ok())
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop();
    }
}
