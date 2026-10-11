//! Раннер: исходящий цикл «claim → демка → исполнитель → результат → finish»; одна задача за раз.
//!
//! Раннер сам ходит к серверу, сервер к нему никогда не подключается. Всё, что делает задачу роликом, прячется
//! за `Executor`; здесь только транспорт и порядок шагов. Токен живёт в `Token` и в журнал не попадает.
//!
//! Задача, пока она у раннера, записана в `runner_state.json` (outbox, модуль `state`): после перезапуска раннер
//! здоровается с `holding`, по указанию сервера продолжает с сохранённого этапа или бросает задачу. Пока в
//! состоянии есть задача, новая не берётся: одна запись и одна загрузка за раз.

pub mod exec;
pub mod link;
pub mod policy;
pub mod state;
pub mod tee;
pub mod transfer;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipeline::record::failure;
use pipeline::record::log::one_line;
use pipeline::{FailCode, RecordFailure, RecordState, RunError};
use wire::{
    Assignment, DirectiveAction, Failure, FinishRequest, HelloRequest, Holding, JobState, Outcome,
    Phase, ReleaseRequest, fail,
};

pub use exec::{Executor, InspectSettings, PipelineExecutor, Task};
pub use link::{Link, LinkError, Token};
pub use policy::{Disposition, Flags, PausePolicy, PauseReason, dispose, failure_of};
pub use state::{HeldJob, ResultFile, RunnerState, Stage};
pub use tee::{FORCE_AFTER, SeqSource, Shared, Tee, Uplink};
pub use transfer::{StopWhy, TransferEnd, sha256_file};

use transfer::{Verdict, judge};

/// Сколько раннер ждёт после `release` перед следующим `claim`: кратковременная неисправность (игра ещё
/// закрывается) успевает пройти.
pub const RELEASE_COOLDOWN: Duration = Duration::from_secs(15);
/// Потолок размера демки по умолчанию: 2000 МиБ, как у дочернего разбора (`InspectLimits`).
pub const MAX_DEMO_BYTES: u64 = 2000 << 20;
/// Начало ошибки `hello` при несовпадении версии протокола: по нему `run` понимает, что повторы не помогут.
pub const PROTOCOL_MISMATCH: &str = "версия протокола сервера не совпадает с раннером";
/// Как часто раннер на паузе напоминает серверу о себе.
const PAUSE_HELLO_EVERY: Duration = Duration::from_secs(30);

/// Настройки раннера.
#[derive(Clone)]
pub struct Config {
    /// Адрес сервера: `http://хост:порт`.
    pub server: String,
    pub token: Token,
    /// Рабочая папка: `remote/` (демки), `out/` (результаты), `runner.log`, `runner_state.json`.
    pub dir: PathBuf,
    /// Имя раннера для журнала сервера.
    pub name: String,
    /// Замена `poll_secs` сервера (для тестов).
    pub poll_secs: Option<u64>,
    /// Остановиться после стольких принятых кусков результата, оставив его в outbox. Только для тестов и учения
    /// «обрыв загрузки».
    #[doc(hidden)]
    pub upload_chunk_limit: Option<u32>,
    /// Замена интервала пульса `min(heartbeat_secs из hello, 5)` с (для тестов).
    pub heartbeat: Option<Duration>,
    /// Не чаще этого уходит внеочередной пульс при смене этапа или дубля (по умолчанию 2 с).
    pub debounce: Duration,
    /// Через сколько мягкая отмена сервера становится жёсткой (по умолчанию `FORCE_AFTER`).
    pub force_after: Duration,
    /// Сколько ждать после `release` перед следующим `claim` (по умолчанию `RELEASE_COOLDOWN`, в тестах ноль).
    pub release_cooldown: Duration,
    /// Демка больше этого размера (по выдаче) не скачивается: задача закрывается `DEMO_TOO_LARGE`.
    pub max_demo_bytes: u64,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("server", &self.server)
            .field("token", &"***")
            .field("dir", &self.dir)
            .field("name", &self.name)
            .field("poll_secs", &self.poll_secs)
            .field("upload_chunk_limit", &self.upload_chunk_limit)
            .field("heartbeat", &self.heartbeat)
            .field("debounce", &self.debounce)
            .field("force_after", &self.force_after)
            .field("release_cooldown", &self.release_cooldown)
            .field("max_demo_bytes", &self.max_demo_bytes)
            .finish()
    }
}

/// Итог одного шага цикла.
#[derive(Debug, Clone, PartialEq)]
pub enum Tick {
    /// Задач нет.
    Idle,
    /// Задача выдана и закрыта на сервере.
    Settled { job_id: i64, state: JobState },
    /// Сервер окончательно отказал в задаче: файлы убраны из outbox, причина в `runner.log`.
    Dropped { job_id: i64 },
    /// Остановлено флагом или лимитом кусков; задача осталась в outbox и продолжится при следующем запуске.
    Stopped,
    /// Сбой окружения или запуска: задача возвращена в очередь сервера через `release`, выдача засчитана.
    Released { job_id: i64 },
    /// Раннер на паузе и задач не берёт, пока его не продолжат (`Control::resume`).
    Paused(PauseReason),
}

/// Журнал `runner.log`: строки `{время} {текст}`, значение токена заменено на `***`.
struct Log {
    file: Mutex<File>,
    secret: String,
}

impl Log {
    fn open(path: &Path, secret: &str) -> Result<Log, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("не удалось открыть {}: {e}", path.display()))?;
        Ok(Log {
            file: Mutex::new(file),
            secret: secret.to_owned(),
        })
    }

    fn line(&self, text: &str) {
        let text = if self.secret.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.secret, "***")
        };
        let line = format!(
            "{} {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            one_line(&text)
        );
        eprintln!("{line}");
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Ход задачи для исполнителя: этапы и строки идут в `runner.log` и stderr.
struct RunnerProgress {
    log: Arc<Log>,
}

impl pipeline::Progress for RunnerProgress {
    fn stage(&self, s: pipeline::Stage) {
        self.log.line(&format!("этап: {}", s.title()));
    }

    fn log(&self, msg: &str) {
        self.log.line(msg);
    }
}

/// Текст ошибки связи; 401 — отдельное сообщение, по нему `run` останавливает раннер.
fn link_error(e: LinkError) -> String {
    match e {
        LinkError::Status { status: 401, .. } => tee::TOKEN_REJECTED.to_owned(),
        e => e.to_string(),
    }
}

/// Нужно ли качать демку с сервера: у ссылки FACEIT без файла (`demo.size == 0`) качать нечего.
fn needs_download(held: &HeldJob) -> bool {
    !(held.faceit.is_some() && held.demo_size == 0)
}

/// Задача для исполнителя из удерживаемой: путь демки всегда `remote/srv{id}.dem`; у ссылки FACEIT без файла
/// на сервере исполнитель сам скачивает в него демку, а ссылка остаётся в `faceit`.
fn task_of(held: &HeldJob, attempt: u32, resumed_dir: Option<PathBuf>) -> Task {
    Task {
        job_id: held.server_job_id,
        attempt,
        demo: held.demo_path.clone(),
        faceit: held.faceit.clone(),
        hero_steam: held.hero_steam,
        hero_name: held.hero_name.clone(),
        resume: resumed_dir.is_some(),
        out_dir: resumed_dir,
    }
}

/// Итог для сервера со своим кодом, подсказкой и классом, как у сбоя исполнителя.
fn typed_failure(code: FailCode, state: RecordState, message: String) -> Failure {
    let f = RecordFailure::new(code, state, message.clone(), Duration::ZERO);
    policy::failure_of(
        &RunError {
            message,
            fail: Some(f),
        },
        0,
    )
}

/// Ручка остановки раннера из другого потока (поток консоли, Ctrl+C). Остановка не отмена: задача остаётся в
/// состоянии и продолжится после перезапуска (D-01).
#[derive(Clone)]
pub struct Control {
    stop: Arc<AtomicBool>,
    resume: Arc<AtomicBool>,
}

impl Control {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Снять паузу раннера: следующий `tick` снова берёт задачи. Без паузы ничего не меняет.
    pub fn resume(&self) {
        self.resume.store(true, Ordering::SeqCst);
    }
}

pub struct Runner {
    cfg: Config,
    link: Link,
    exec: Arc<dyn Executor>,
    log: Arc<Log>,
    stop: Arc<AtomicBool>,
    poll_secs: Option<u64>,
    /// `heartbeat_secs` из ответа `hello`.
    heartbeat_secs: u64,
    /// Источник `seq` прогресса: общий для всех пульсов раннера, начинается с `RunnerState.last_seq`.
    seq: Arc<SeqSource>,
    /// Доска текущей задачи: этап, дубль, отмены; новая на каждое удержание задачи.
    shared: Arc<Shared>,
    /// Содержимое `runner_state.json`: единственная удерживаемая задача.
    state: RunnerState,
    /// Серия одинаковых неисправностей подряд.
    pause: PausePolicy,
    /// Раннер на паузе: пока `Some`, задачи не берутся.
    paused: Option<PauseReason>,
    /// Флаг `Control::resume`.
    resume: Arc<AtomicBool>,
    /// Причина, по которой `run` обязан вернуть ошибку: токен отозван, протокол не совпал.
    fatal: Option<String>,
    /// Когда раннер последний раз сообщил серверу о паузе.
    pause_hello: Option<Instant>,
    /// Раньше этого момента новый `claim` не делается (пауза после `release`).
    cooldown_until: Option<Instant>,
}

impl Runner {
    pub fn new(cfg: Config, exec: Arc<dyn Executor>) -> Result<Runner, String> {
        for sub in ["remote", "out"] {
            let dir = cfg.dir.join(sub);
            fs::create_dir_all(&dir)
                .map_err(|e| format!("не удалось создать {}: {e}", dir.display()))?;
        }
        let log = Arc::new(Log::open(&cfg.dir.join("runner.log"), cfg.token.secret())?);
        let link = Link::new(&cfg.server, cfg.token.clone())?;
        let (state, warning) = RunnerState::load(&cfg.dir);
        if let Some(w) = warning {
            log.line(&format!("состояние раннера: {w}"));
        }
        Ok(Runner {
            cfg,
            link,
            exec,
            log,
            stop: Arc::new(AtomicBool::new(false)),
            poll_secs: None,
            heartbeat_secs: 10,
            seq: Arc::new(SeqSource::new(state.last_seq)),
            shared: Arc::new(Shared::new(Phase::Fetch)),
            state,
            pause: PausePolicy::default(),
            paused: None,
            resume: Arc::new(AtomicBool::new(false)),
            pause_hello: None,
            cooldown_until: None,
            fatal: None,
        })
    }

    /// Флаг остановки `run`: поднимите его из другого потока.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Ручка остановки для другого потока.
    pub fn control(&self) -> Control {
        Control {
            stop: self.stop.clone(),
            resume: self.resume.clone(),
        }
    }

    /// Один шаг: представиться серверу (в первый раз, с удерживаемой задачей), довести удерживаемую задачу до
    /// конца или спросить новую и довести её. На паузе задач не берёт.
    pub fn tick(&mut self) -> Result<Tick, String> {
        if self.poll_secs.is_none() {
            self.recover_executor();
            self.hello()?;
        }
        if self.resume.swap(false, Ordering::SeqCst) && self.paused.take().is_some() {
            self.pause_hello = None;
            self.log.line("пауза снята: раннер снова берёт задачи");
        }
        let tick = if self.state.job.is_some() {
            self.drive()?
        } else if let Some(reason) = self.paused.clone() {
            self.pause_ping(&reason)?;
            return Ok(Tick::Paused(reason));
        } else {
            self.wait_cooldown();
            let poll = self.cfg.poll_secs.or(self.poll_secs).unwrap_or(0);
            match self.link.claim(poll).map_err(link_error)? {
                None => Tick::Idle,
                Some(a) => self.accept(a)?,
            }
        };
        // Вторая неисправность подряд: задача уже возвращена, раннер встаёт на паузу.
        Ok(match (tick, &self.paused) {
            (Tick::Released { .. }, Some(reason)) => Tick::Paused(reason.clone()),
            (tick, _) => tick,
        })
    }

    /// Ждёт `release_cooldown` после `release`, проверяя флаг остановки.
    fn wait_cooldown(&mut self) {
        let Some(until) = self.cooldown_until.take() else {
            return;
        };
        while Instant::now() < until && !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
        }
    }

    /// Раннер на паузе раз в 30 с говорит серверу «пауза: {код}» с пустым `holding`.
    fn pause_ping(&mut self, reason: &PauseReason) -> Result<(), String> {
        if self
            .pause_hello
            .is_some_and(|t| t.elapsed() < PAUSE_HELLO_EVERY)
        {
            return Ok(());
        }
        let req = HelloRequest {
            protocol: wire::PROTOCOL_VERSION,
            name: self.cfg.name.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            status: format!("пауза: {}", reason.code),
            holding: Vec::new(),
        };
        self.link.hello(&req).map_err(link_error)?;
        self.pause_hello = Some(Instant::now());
        Ok(())
    }

    /// Раннер встаёт на паузу: строка в `runner.log` и stderr.
    fn start_pause(&mut self, reason: PauseReason) {
        self.log.line(&reason.to_string());
        self.pause_hello = None;
        // Enter, нажатый до паузы, не считается её снятием (иначе защита от круга «неисправность - повтор» обходится)
        self.resume.store(false, Ordering::SeqCst);
        self.paused = Some(reason);
    }

    /// Цикл до флага остановки; ошибки связи — пауза 1, 2, 4 … 30 с. `Err` — раннер остановило то, что повторами
    /// не лечится: сервер не принял токен или версия протокола не совпала; задача при этом остаётся в состоянии.
    pub fn run(&mut self) -> Result<(), String> {
        let mut pause = 0u64;
        while !self.stop.load(Ordering::SeqCst) {
            match self.tick() {
                Ok(Tick::Settled { .. } | Tick::Dropped { .. } | Tick::Released { .. }) => {
                    pause = 0
                }
                Ok(Tick::Paused(_)) => {
                    pause = 0;
                    self.sleep(1);
                }
                Ok(Tick::Stopped) => {
                    self.stop.store(true, Ordering::SeqCst);
                }
                Ok(Tick::Idle) => {
                    pause = 0;
                    // Long-poll уже ждал на сервере; без него (poll_secs = 0) не крутимся вхолостую.
                    if self.cfg.poll_secs == Some(0) {
                        self.sleep(1);
                    }
                }
                Err(e) if e == tee::TOKEN_REJECTED || e.starts_with(PROTOCOL_MISMATCH) => {
                    self.log.line(&format!(
                        "ошибка: {e}; раннер остановлен, задача остаётся в состоянии"
                    ));
                    self.fatal = Some(e);
                    self.stop.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    self.log.line(&format!("ошибка: {e}"));
                    pause = (pause * 2).clamp(1, 30);
                    self.sleep(pause);
                }
            }
        }
        match self.fatal.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Пауза с проверкой флага остановки каждые 200 мс.
    fn sleep(&self, secs: u64) {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until && !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Записывает `runner_state.json`.
    fn persist(&self) -> Result<(), String> {
        self.state
            .save(&self.cfg.dir)
            .map_err(|e| format!("не удалось записать {}: {e}", state::STATE_FILE))
    }

    fn held(&mut self) -> &mut HeldJob {
        self.state
            .job
            .as_mut()
            .expect("вызывается только при удерживаемой задаче")
    }

    /// Убирает за прошлым запуском исполнителя после падения: строки уходят в журнал.
    fn recover_executor(&self) {
        let Some(out_dir) = self.state.job.as_ref().and_then(|j| j.out_dir.as_ref()) else {
            return;
        };
        for line in self.exec.recover(out_dir) {
            self.log.line(&line);
        }
    }

    fn hello(&mut self) -> Result<(), String> {
        let holding: Vec<Holding> = self
            .state
            .job
            .iter()
            .map(|j| Holding {
                job_id: j.server_job_id,
                epoch: j.epoch,
            })
            .collect();
        let req = HelloRequest {
            protocol: wire::PROTOCOL_VERSION,
            name: self.cfg.name.clone(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            status: if holding.is_empty() { "idle" } else { "busy" }.to_owned(),
            holding,
        };
        let reply = match self.link.hello(&req) {
            Ok(reply) => reply,
            Err(LinkError::Status {
                status: 426,
                message,
                ..
            }) => {
                return Err(format!("{PROTOCOL_MISMATCH}: {message}"));
            }
            Err(e) => return Err(link_error(e)),
        };
        self.log.line(&format!(
            "на связи с сервером {} (протокол {})",
            reply.server_version,
            wire::PROTOCOL_VERSION
        ));
        self.poll_secs = Some(reply.poll_secs);
        self.heartbeat_secs = reply.heartbeat_secs;

        let Some(id) = self.state.job.as_ref().map(|j| j.server_job_id) else {
            return Ok(());
        };
        let action = reply
            .directives
            .iter()
            .find(|d| d.job_id == id)
            .map(|d| d.action);
        match action {
            Some(DirectiveAction::Keep) => {
                let stage = self.held().stage;
                self.log.line(&format!(
                    "задача #{id}: сервер подтвердил выдачу, продолжаю с этапа {stage:?}"
                ));
            }
            // Отмена: итог `Cancelled` сохраняется и уходит как обычный `finish`, файлы убираются после ответа.
            Some(DirectiveAction::Cancel) => {
                self.log
                    .line(&format!("задача #{id}: сервер запросил отмену"));
                let held = self.held();
                held.pending = Some(Outcome::Cancelled);
                held.pending_release = None;
                held.stage = Stage::Reporting;
                self.persist()?;
            }
            _ => {
                let why = match action {
                    Some(DirectiveAction::Abandon) => "сервер велел бросить задачу",
                    _ => "сервер не знает этой задачи",
                };
                self.drop_job(why)?;
            }
        }
        Ok(())
    }

    /// Бросает удерживаемую задачу: файлы убираются, причина и номер задачи пишутся в журнал, состояние очищается.
    fn drop_job(&mut self, reason: &str) -> Result<(), String> {
        let Some(job) = self.state.job.take() else {
            return Ok(());
        };
        let id = job.server_job_id;
        self.log.line(&format!(
            "задача #{id}: {reason}; файлы задачи удалены из outbox"
        ));
        self.remove_files(&job);
        self.persist()
    }

    /// Удаляет скачанную демку (и недокачанную) внутри рабочей папки раннера: сервер хранит её сам.
    fn remove_demo(&self, job: &HeldJob) {
        for p in [job.demo_path.clone(), job.demo_path.with_extension("dem.part")] {
            if p.starts_with(&self.cfg.dir) {
                let _ = fs::remove_file(&p);
            }
        }
        self.remove_faceit_dir(job);
    }

    /// Временная папка скачивания FACEIT (`srv{id}.faceit`) убирается вместе с демкой, только внутри рабочей папки.
    fn remove_faceit_dir(&self, job: &HeldJob) {
        let dir = job.demo_path.with_extension("faceit");
        if dir.starts_with(&self.cfg.dir) && dir != self.cfg.dir {
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// Задача закрыта сервером с принятым результатом: демка, ролик и папка с сырыми дублями больше не нужны,
    /// иначе автономный раннер заполнит диск. Папка удаляется только внутри рабочей папки раннера.
    fn remove_finished(&self, job: &HeldJob) {
        self.remove_files(job);
        if let Some(dir) = &job.out_dir
            && dir.starts_with(&self.cfg.dir)
            && dir != &self.cfg.dir
        {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// Удаляет демку и результат задачи, но только внутри рабочей папки раннера.
    fn remove_files(&self, job: &HeldJob) {
        let mut paths = vec![
            job.demo_path.clone(),
            job.demo_path.with_extension("dem.part"),
        ];
        if let Some(r) = &job.result {
            paths.push(r.path.clone());
        }
        for p in paths {
            if p.starts_with(&self.cfg.dir) {
                let _ = fs::remove_file(&p);
            }
        }
        self.remove_faceit_dir(job);
    }

    /// Выданная задача: записать в outbox и довести.
    fn accept(&mut self, a: Assignment) -> Result<Tick, String> {
        let id = a.job_id;
        self.log
            .line(&format!("задача #{id} выдана (эпоха {})", a.epoch));
        // Имя файла строится только из номера задачи: `orig_name` от пользователя в путях не участвует.
        let demo = self.cfg.dir.join("remote").join(format!("srv{id}.dem"));
        self.state.job = Some(HeldJob::claimed(&a, demo));
        self.persist()?;
        self.drive()
    }

    /// Ведёт удерживаемую задачу по этапам с того, на котором она записана. Пульс (`Uplink`) идёт на всё время
    /// удержания задачи; после него `seq` попадает в состояние (Pitfall 2).
    fn drive(&mut self) -> Result<Tick, String> {
        let mut uplink = None;
        let result = self.drive_stages(&mut uplink);
        self.stop_uplink(&mut uplink);
        result
    }

    /// Останавливает пульс и запоминает последний `seq`.
    fn stop_uplink(&mut self, uplink: &mut Option<Uplink>) {
        if let Some(u) = uplink.take() {
            let last = u.stop();
            if last > self.state.last_seq {
                self.state.last_seq = last;
                if let Err(e) = self.persist() {
                    self.log.line(&e);
                }
            }
        }
    }

    /// Запускает пульс для удерживаемой задачи: новая доска, начальный этап по этапу задачи.
    fn start_uplink(&mut self) -> Uplink {
        let (id, epoch, stage) = {
            let held = self.held();
            (held.server_job_id, held.epoch, held.stage)
        };
        let phase = match stage {
            Stage::Claimed => Phase::Fetch,
            Stage::Fetched | Stage::Executing => Phase::Precheck,
            Stage::Finished | Stage::Reporting => Phase::Upload,
        };
        self.shared = Arc::new(Shared::new(phase));
        let interval = self
            .cfg
            .heartbeat
            .unwrap_or_else(|| Duration::from_secs(self.heartbeat_secs.clamp(1, 5)));
        let log = self.log.clone();
        Uplink::start(
            self.link.clone(),
            id,
            epoch,
            self.shared.clone(),
            self.seq.clone(),
            tee::Timing {
                interval,
                debounce: self.cfg.debounce,
            },
            Arc::new(move |m: &str| log.line(m)),
        )
    }

    fn drive_stages(&mut self, uplink: &mut Option<Uplink>) -> Result<Tick, String> {
        let started = Instant::now();
        if self.held().stage != Stage::Reporting {
            *uplink = Some(self.start_uplink());
        }
        loop {
            let Some(stage) = self.state.job.as_ref().map(|j| j.stage) else {
                return Ok(Tick::Idle);
            };
            match stage {
                Stage::Claimed => {
                    if let Some(t) = self.fetch()? {
                        return Ok(t);
                    }
                }
                Stage::Fetched | Stage::Executing => {
                    if let Some(t) = self.execute(started)? {
                        return Ok(t);
                    }
                }
                Stage::Finished => {
                    self.shared.set_phase(Phase::Upload);
                    if let Some(t) = self.upload()? {
                        return Ok(t);
                    }
                }
                Stage::Reporting => {
                    self.stop_uplink(uplink);
                    return self.report();
                }
            }
        }
    }

    /// Функция остановки передач: потеря задачи, отмена сервера, локальная остановка.
    fn stopper(&self) -> impl Fn() -> Option<StopWhy> + use<> {
        let (shared, stop) = (self.shared.clone(), self.stop.clone());
        move || shared.stop_reason(&stop)
    }

    /// Задача потеряна: токен отозван — раннер останавливается, задача остаётся; иначе сброс без `finish` (D-03).
    fn lose(&mut self) -> Result<Tick, String> {
        let id = self.held().server_job_id;
        if self.shared.token_revoked() {
            self.stop.store(true, Ordering::SeqCst);
            self.log.line(&format!(
                "задача #{id}: {}; раннер остановлен, задача остаётся в состоянии",
                tee::TOKEN_REJECTED
            ));
            self.fatal = Some(tee::TOKEN_REJECTED.to_owned());
            return Ok(Tick::Stopped);
        }
        let reason = self.shared.lost_reason();
        self.drop_job(&format!("задача потеряна — {reason}"))?;
        Ok(Tick::Dropped { job_id: id })
    }

    /// Отмена сервера: итог `Cancelled` сохраняется и уходит как обычный `finish`, файлы убираются после ответа.
    fn cancel_job(&mut self) -> Result<(), String> {
        let id = self.held().server_job_id;
        self.log
            .line(&format!("задача #{id}: сервер запросил отмену"));
        self.pause.after(&Disposition::Cancel);
        let held = self.held();
        held.pending = Some(Outcome::Cancelled);
        held.pending_release = None;
        held.stage = Stage::Reporting;
        self.persist()
    }

    /// Закрывает удерживаемую задачу сбоем: итог пишется в outbox и уходит серверу этапом `Reporting`. Контентный
    /// сбой обрывает серию неисправностей. Файлы задачи не удаляются: негодный результат остаётся уликой.
    fn fail_job(&mut self, failure: Failure) -> Result<(), String> {
        self.pause.after(&Disposition::Fail(failure.clone()));
        let held = self.held();
        held.pending = Some(Outcome::Failed(failure));
        held.stage = Stage::Reporting;
        self.persist()
    }

    /// Скачивает демку; не сошлась по SHA-256 — итог задачи `BAD_DEMO`.
    fn fetch(&mut self) -> Result<Option<Tick>, String> {
        let job = self.state.job.clone().expect("задача удерживается");
        // Ссылка FACEIT без файла: качать нечего, отказ даст исполнитель (`UNSUPPORTED_SOURCE`).
        if !needs_download(&job) {
            self.log.line(&format!(
                "задача #{}: демки на сервере нет, скачивание пропущено",
                job.server_job_id
            ));
            self.held().stage = Stage::Fetched;
            self.persist()?;
            return Ok(None);
        }
        // Огромная демка не скачивается вовсе: диск раннера не зависит от чужого файла (T-06-29).
        if job.demo_size > self.cfg.max_demo_bytes {
            let reason = format!(
                "{} МиБ при потолке {} МиБ",
                job.demo_size.div_ceil(1 << 20),
                self.cfg.max_demo_bytes >> 20
            );
            let failure = typed_failure(
                FailCode::DemoTooLarge,
                RecordState::Precheck,
                failure::msg_demo_too_large(&reason),
            );
            self.log.line(&format!(
                "задача #{}: {}",
                job.server_job_id, failure.message
            ));
            self.fail_job(failure)?;
            return Ok(None);
        }
        let end = {
            let log = |m: &str| self.log.line(m);
            let stop = self.stopper();
            match transfer::download_demo(&self.link, &job, &stop, &log) {
                Ok(end) => end,
                // Диск не пишется: сеть тут ни при чём, повтор не поможет. Задача закрывается понятной причиной.
                Err(e) if e.starts_with(transfer::DISK_ERROR) => {
                    let failure = Failure {
                        code: fail::JOB_ERROR.to_owned(),
                        message: format!("{e} (проверьте свободное место на диске раннера)"),
                        ..Default::default()
                    };
                    self.log.line(&format!(
                        "задача #{}: {}",
                        job.server_job_id, failure.message
                    ));
                    self.fail_job(failure)?;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        };
        match end {
            TransferEnd::Done(()) => {
                self.held().stage = Stage::Fetched;
                self.persist()?;
                Ok(None)
            }
            TransferEnd::Stopped => Ok(Some(Tick::Stopped)),
            TransferEnd::Cancelled => {
                self.cancel_job()?;
                Ok(None)
            }
            TransferEnd::Lost => self.lose().map(Some),
            TransferEnd::Dropped(reason) => {
                let failure = Failure {
                    code: fail::BAD_DEMO.to_owned(),
                    message: reason,
                    ..Default::default()
                };
                // Контентный сбой обрывает серию неисправностей.
                self.pause.after(&Disposition::Fail(failure.clone()));
                let held = self.held();
                held.pending = Some(Outcome::Failed(failure));
                held.stage = Stage::Reporting;
                self.persist()?;
                Ok(None)
            }
        }
    }

    /// Запускает исполнитель и записывает его итог в outbox. `Some` — задача на этом тике закончена.
    fn execute(&mut self, started: Instant) -> Result<Option<Tick>, String> {
        // Демки на месте нет (диск почистили): сначала скачать заново.
        if needs_download(self.held()) && !self.held().demo_path.is_file() {
            let id = self.held().server_job_id;
            self.log
                .line(&format!("задача #{id}: файла демки нет, скачиваю заново"));
            self.held().stage = Stage::Claimed;
            self.persist()?;
            return Ok(None);
        }
        let planned = self
            .cfg
            .dir
            .join("out")
            .join(format!("srv{}", self.held().server_job_id));
        // Перезапуск посреди записи (этап уже `Executing`): исполнитель идёт с `resume` по папке прошлой попытки.
        let (resumed_dir, attempt) = {
            let held = self.held();
            let resumed_dir = if held.stage == Stage::Executing {
                held.out_dir.clone()
            } else {
                None
            };
            held.stage = Stage::Executing;
            held.attempt += 1;
            // Папка известна заранее: если раннер упадёт посреди записи, после перезапуска исполнитель найдёт её.
            held.out_dir.get_or_insert(planned);
            (resumed_dir, held.attempt)
        };
        self.persist()?;
        let held = self.state.job.clone().expect("задача удерживается");
        let task = task_of(&held, attempt, resumed_dir);
        let progress = RunnerProgress {
            log: self.log.clone(),
        };
        // Папку задачи, которую назвал пайплайн, пишем в outbox сразу: после падения она нужна для продолжения.
        let persist_out_dir = {
            let (dir, seq, snapshot) = (self.cfg.dir.clone(), self.seq.clone(), held.clone());
            let log = self.log.clone();
            move |p: &Path| {
                let mut job = snapshot.clone();
                job.out_dir = Some(p.to_path_buf());
                let state = RunnerState {
                    version: state::VERSION,
                    job: Some(job),
                    last_seq: seq.last(),
                };
                if let Err(e) = state.save(&dir) {
                    log.line(&format!("не удалось записать {}: {e}", state::STATE_FILE));
                }
            }
        };
        self.shared.set_phase(Phase::Precheck);
        let tee = Tee {
            inner: &progress,
            shared: self.shared.clone(),
            stop: self.stop.clone(),
            force_after: self.cfg.force_after,
            persist_out_dir: &persist_out_dir,
        };
        let run = self.exec.run(&task, &tee);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let id = held.server_job_id;

        let flags = Flags {
            server_cancel: self.shared.cancel_mode(),
            local_stop: self.stop.load(Ordering::SeqCst),
            abandoned: self.shared.is_lost(),
            elapsed_ms,
        };
        let disposition = policy::dispose(&run, flags);
        if let Some(dir) = self.shared.out_dir() {
            self.held().out_dir = Some(dir);
        }
        if let Some(reason) = self.pause.after(&disposition) {
            self.start_pause(reason);
        }
        match disposition {
            Disposition::Done(outcome) => match sha256_file(&outcome.result) {
                Ok((size, sha256)) => {
                    let job = self.held();
                    job.out_dir = Some(outcome.out_dir);
                    job.result = Some(ResultFile {
                        path: outcome.result,
                        size,
                        sha256,
                    });
                    job.stage = Stage::Finished;
                }
                Err(e) => {
                    let message = format!("результат не читается: {e}");
                    self.log
                        .line(&format!("задача #{id} не удалась: {message}"));
                    let job = self.held();
                    job.pending = Some(Outcome::Failed(Failure {
                        code: fail::JOB_ERROR.to_owned(),
                        message,
                        elapsed_ms,
                        ..Default::default()
                    }));
                    job.stage = Stage::Reporting;
                }
            },
            // Остановка раннера не отмена: задача остаётся в состоянии `Executing` и продолжится (D-01).
            Disposition::Keep => {
                self.log.line(&format!(
                    "задача #{id}: раннер остановлен, задача останется для продолжения"
                ));
                self.persist()?;
                return Ok(Some(Tick::Stopped));
            }
            Disposition::Fail(failure) => {
                self.log.line(&format!(
                    "задача #{id} не удалась: {} ({})",
                    failure.message, failure.code
                ));
                let job = self.held();
                job.pending = Some(Outcome::Failed(failure));
                job.stage = Stage::Reporting;
            }
            Disposition::Cancel => {
                self.log
                    .line(&format!("задача #{id}: отменена, итог — отмена"));
                let job = self.held();
                job.pending = Some(Outcome::Cancelled);
                job.stage = Stage::Reporting;
            }
            // Неисправность окружения или запуска не вина демки: решение записывается до запроса, чтобы
            // перезапуск повторил `release`, а не гонял исполнителя заново.
            Disposition::Release {
                reason,
                code,
                class,
                ..
            } => {
                self.log.line(&format!(
                    "задача #{id}: неисправность «{}» ({code}), возвращаю в очередь",
                    class.title()
                ));
                let job = self.held();
                job.pending = None;
                job.pending_release = Some(reason);
                job.stage = Stage::Reporting;
            }
            // Задача потеряна, пока шёл исполнитель: ничего не закрываем (D-03).
            Disposition::Abandon => return self.lose().map(Some),
        }
        self.persist()?;
        Ok(None)
    }

    /// Загружает результат со смещения сервера и закрывает задачу `Done`. `None` — отмена на сервере прервала
    /// загрузку, дальше нужно сообщить итог `Cancelled`.
    fn upload(&mut self) -> Result<Option<Tick>, String> {
        // Файл мог измениться после проверки: на сервер уходит только тот, что проверен (T-06-28). Сверка идёт
        // перед первой загрузкой; после отказа сервера по SHA-256 (`sha_retries > 0`) решает сам сервер.
        let first_upload = self.held().sha_retries == 0;
        if first_upload && let Some(result) = self.held().result.clone() {
            let same = matches!(
                sha256_file(&result.path),
                Ok((size, sha256)) if size == result.size && sha256 == result.sha256
            );
            if !same {
                let failure = typed_failure(
                    FailCode::ResultInvalid,
                    RecordState::Done,
                    failure::msg_result_invalid("файл результата изменился после проверки"),
                );
                let id = self.held().server_job_id;
                self.log.line(&format!("задача #{id}: {}", failure.message));
                self.fail_job(failure)?;
                return Ok(None);
            }
        }
        let mut held = self.state.job.take().expect("задача удерживается");
        let id = held.server_job_id;
        let end = {
            let dir = self.cfg.dir.clone();
            let last_seq = self.seq.last();
            let log = |m: &str| self.log.line(m);
            let mut save = |h: &HeldJob| {
                let snapshot = RunnerState {
                    version: state::VERSION,
                    job: Some(h.clone()),
                    last_seq,
                };
                if let Err(e) = snapshot.save(&dir) {
                    self.log
                        .line(&format!("не удалось записать {}: {e}", state::STATE_FILE));
                }
            };
            let stop = self.stopper();
            transfer::upload_result(
                &self.link,
                &mut held,
                &mut save,
                &stop,
                self.cfg.upload_chunk_limit,
                &log,
            )
        };
        match end {
            TransferEnd::Done(state) => {
                self.log.line(&format!("задача #{id}: {}", state.as_str()));
                self.remove_finished(&held);
                self.persist()?;
                Ok(Some(Tick::Settled { job_id: id, state }))
            }
            TransferEnd::Stopped => {
                self.state.job = Some(held);
                self.persist()?;
                Ok(Some(Tick::Stopped))
            }
            TransferEnd::Cancelled => {
                self.state.job = Some(held);
                self.log.line(&format!(
                    "задача #{id}: отмена на сервере прервала загрузку"
                ));
                self.cancel_job()?;
                Ok(None)
            }
            TransferEnd::Lost => {
                self.state.job = Some(held);
                self.lose().map(Some)
            }
            TransferEnd::Dropped(reason) => {
                self.state.job = Some(held);
                self.drop_job(&format!("сервер не принял результат — {reason}"))?;
                Ok(Some(Tick::Dropped { job_id: id }))
            }
        }
    }

    /// Возвращает задачу в очередь сервера: выдача засчитывается всегда (D-02 буквально). Потолок держит сервер:
    /// после `max_grants` выдач он сам закроет задачу `TOO_MANY_GRANTS`.
    fn release(&mut self, held: HeldJob, reason: String) -> Result<Tick, String> {
        let id = held.server_job_id;
        let req = ReleaseRequest {
            epoch: held.epoch,
            reason,
            counts: true,
        };
        match self.link.release(id, &req) {
            Ok(()) => {
                self.log
                    .line(&format!("задача #{id}: возвращена в очередь сервера"));
                self.remove_files(&held);
                self.state.job = None;
                self.persist()?;
                if !self.cfg.release_cooldown.is_zero() {
                    self.cooldown_until = Some(Instant::now() + self.cfg.release_cooldown);
                }
                Ok(Tick::Released { job_id: id })
            }
            // Чужая эпоха или закрытая задача: возвращать нечего, дело сделано.
            Err(e) => match judge(&e) {
                Verdict::Drop(why) => {
                    self.drop_job(&format!("сервер не принял release — {why}"))?;
                    Ok(Tick::Dropped { job_id: id })
                }
                // 401 повторами не лечится: ровно `TOKEN_REJECTED`, чтобы `run` остановил раннер.
                Verdict::Retry => Err(match e {
                    LinkError::Status { status: 401, .. } => tee::TOKEN_REJECTED.to_owned(),
                    e => format!("задача #{id}: release: {e}"),
                }),
            },
        }
    }

    /// Сообщает серверу сохранённый итог (`Failed` или `Cancelled`); сеть повторяется следующим `tick`.
    fn report(&mut self) -> Result<Tick, String> {
        let held = self.state.job.clone().expect("задача удерживается");
        let id = held.server_job_id;
        // Итог важнее возврата: отмена, пришедшая после решения о `release`, закрывает задачу (WR-01 сервера).
        if held.pending.is_none()
            && let Some(reason) = held.pending_release.clone()
        {
            return self.release(held, reason);
        }
        let outcome = held.pending.clone().unwrap_or_else(|| {
            Outcome::Failed(Failure {
                code: fail::JOB_ERROR.to_owned(),
                message: "итог задачи не сохранён".to_owned(),
                ..Default::default()
            })
        });
        match self.link.finish(
            id,
            &FinishRequest {
                epoch: held.epoch,
                outcome,
            },
        ) {
            Ok(reply) => {
                self.log
                    .line(&format!("задача #{id}: {}", reply.state.as_str()));
                if matches!(held.pending, Some(Outcome::Cancelled)) {
                    self.log.line(&format!(
                        "задача #{id}: отменена, файлы задачи удалены из outbox"
                    ));
                    self.remove_files(&held);
                } else {
                    // Сбой: ролик и папка дублей остаются уликой, демка скачивается с сервера заново.
                    self.remove_demo(&held);
                }
                self.state.job = None;
                self.persist()?;
                Ok(Tick::Settled {
                    job_id: id,
                    state: reply.state,
                })
            }
            Err(e) => match judge(&e) {
                Verdict::Drop(reason) => {
                    self.drop_job(&format!("сервер не принял итог — {reason}"))?;
                    Ok(Tick::Dropped { job_id: id })
                }
                Verdict::Retry => Err(match e {
                    LinkError::Status { status: 401, .. } => tee::TOKEN_REJECTED.to_owned(),
                    e => format!("задача #{id}: finish: {e}"),
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(faceit: Option<&str>, size: u64) -> Assignment {
        Assignment {
            job_id: 5,
            epoch: 1,
            lease_until_ms: 0,
            demo: wire::DemoInfo {
                size,
                ..Default::default()
            },
            faceit_link: faceit.map(str::to_owned),
            player: wire::Player {
                steam64: None,
                name: "s1mple".to_owned(),
            },
            requested_by: String::new(),
            options: Default::default(),
        }
    }

    #[test]
    fn faceit_link_is_not_downloaded_from_the_server_and_keeps_the_demo_path() {
        let link = "https://www.faceit.com/ru/cs2/room/1-abc";
        let held = HeldJob::claimed(&assignment(Some(link), 0), PathBuf::from("remote/srv5.dem"));
        assert!(!needs_download(&held));
        let task = task_of(&held, 1, None);
        assert_eq!(task.faceit.as_deref(), Some(link));
        assert_eq!(task.demo, PathBuf::from("remote/srv5.dem"));

        // Ссылка вместе с файлом демки и обычная задача без ссылки качаются как раньше.
        let with_file = HeldJob::claimed(
            &assignment(Some(link), 10),
            PathBuf::from("remote/srv5.dem"),
        );
        assert!(needs_download(&with_file));
        assert_eq!(
            task_of(&with_file, 1, None).demo,
            PathBuf::from("remote/srv5.dem")
        );
        let plain = HeldJob::claimed(&assignment(None, 0), PathBuf::from("remote/srv5.dem"));
        assert!(needs_download(&plain));
    }
}
