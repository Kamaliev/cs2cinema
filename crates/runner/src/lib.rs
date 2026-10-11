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
pub mod state;
pub mod tee;
pub mod transfer;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipeline::record::failure::classify;
use pipeline::record::log::one_line;
use pipeline::{FailCode, RunError};
use wire::{
    Assignment, DirectiveAction, Failure, FinishRequest, HelloRequest, Holding, JobState, Outcome,
    Phase,
};

pub use exec::{Executor, Task};
pub use link::{Link, LinkError, Token};
pub use state::{HeldJob, ResultFile, RunnerState, Stage};
pub use tee::{FORCE_AFTER, SeqSource, Shared, Tee, Uplink};
pub use transfer::{StopWhy, TransferEnd, sha256_file};

use transfer::{Verdict, judge};

/// Код сбоя исполнителя без собственного кода. Позже заменится на `wire::fail::JOB_ERROR`.
const JOB_ERROR: &str = "JOB_ERROR";
/// Код «демка не сошлась по SHA-256». Позже заменится на `wire::fail::BAD_DEMO`.
const BAD_DEMO: &str = "BAD_DEMO";

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

/// Итог для сервера по ошибке исполнителя: отмена — `Cancelled`, сбой записи — `Failed` со своим кодом, классом и
/// состоянием автомата, ошибка без описания — `JOB_ERROR`. Весь текст проходит `one_line`.
fn outcome_of(e: &RunError, elapsed_ms: u64) -> Outcome {
    let cancelled = e.message == pipeline::CANCELLED
        || e.fail
            .as_ref()
            .is_some_and(|f| f.code == FailCode::Cancelled);
    if cancelled {
        return Outcome::Cancelled;
    }
    Outcome::Failed(match &e.fail {
        Some(f) => Failure {
            code: f.code.as_str().to_owned(),
            class: classify(f.code, f.state).title().to_owned(),
            state: f.state.code().to_owned(),
            hint: one_line(&f.hint),
            message: one_line(&f.message),
            tail: f
                .console_tail
                .iter()
                .map(|line| one_line(line))
                .collect::<Vec<_>>()
                .join("\n"),
            elapsed_ms,
        },
        None => Failure {
            code: JOB_ERROR.to_owned(),
            message: one_line(&e.message),
            elapsed_ms,
            ..Default::default()
        },
    })
}

/// Ручка остановки раннера из другого потока (поток консоли, Ctrl+C). Остановка не отмена: задача остаётся в
/// состоянии и продолжится после перезапуска (D-01).
#[derive(Clone)]
pub struct Control {
    stop: Arc<AtomicBool>,
}

impl Control {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
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
        }
    }

    /// Один шаг: представиться серверу (в первый раз, с удерживаемой задачей), довести удерживаемую задачу до
    /// конца или спросить новую и довести её.
    pub fn tick(&mut self) -> Result<Tick, String> {
        if self.poll_secs.is_none() {
            self.recover_executor();
            self.hello()?;
        }
        if self.state.job.is_some() {
            return self.drive();
        }
        let poll = self.cfg.poll_secs.or(self.poll_secs).unwrap_or(0);
        match self.link.claim(poll).map_err(link_error)? {
            None => Ok(Tick::Idle),
            Some(a) => self.accept(a),
        }
    }

    /// Цикл до флага остановки; ошибки связи — пауза 1, 2, 4 … 30 с.
    pub fn run(&mut self) {
        let mut pause = 0u64;
        while !self.stop.load(Ordering::SeqCst) {
            match self.tick() {
                Ok(Tick::Settled { .. } | Tick::Dropped { .. }) => pause = 0,
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
                Err(e) if e == tee::TOKEN_REJECTED => {
                    self.log.line(&format!(
                        "ошибка: {e}; раннер остановлен, задача остаётся в состоянии"
                    ));
                    self.stop.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    self.log.line(&format!("ошибка: {e}"));
                    pause = (pause * 2).clamp(1, 30);
                    self.sleep(pause);
                }
            }
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
                return Err(format!(
                    "версия протокола сервера не совпадает с раннером: {message}"
                ));
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
        let held = self.held();
        held.pending = Some(Outcome::Cancelled);
        held.stage = Stage::Reporting;
        self.persist()
    }

    /// Скачивает демку; не сошлась по SHA-256 — итог задачи `BAD_DEMO`.
    fn fetch(&mut self) -> Result<Option<Tick>, String> {
        let job = self.state.job.clone().expect("задача удерживается");
        let end = {
            let log = |m: &str| self.log.line(m);
            let stop = self.stopper();
            transfer::download_demo(&self.link, &job, &stop, &log)?
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
                let held = self.held();
                held.pending = Some(Outcome::Failed(Failure {
                    code: BAD_DEMO.to_owned(),
                    message: reason,
                    ..Default::default()
                }));
                held.stage = Stage::Reporting;
                self.persist()?;
                Ok(None)
            }
        }
    }

    /// Запускает исполнитель и записывает его итог в outbox. `Some` — задача на этом тике закончена.
    fn execute(&mut self, started: Instant) -> Result<Option<Tick>, String> {
        // Демки на месте нет (диск почистили): сначала скачать заново.
        if !self.held().demo_path.is_file() {
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
        let task = Task {
            job_id: held.server_job_id,
            attempt,
            demo: held.demo_path.clone(),
            faceit: held.faceit.clone(),
            hero_steam: held.hero_steam,
            hero_name: held.hero_name.clone(),
            resume: resumed_dir.is_some(),
            out_dir: resumed_dir,
        };
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

        // Задача потеряна, пока шёл исполнитель: ничего не закрываем (D-03).
        if self.shared.is_lost() {
            return self.lose().map(Some);
        }
        if let Some(dir) = self.shared.out_dir() {
            self.held().out_dir = Some(dir);
        }
        let server_cancel = self.shared.server_cancel();
        let local_stop = self.stop.load(Ordering::SeqCst);
        match run {
            Ok(outcome) => match sha256_file(&outcome.result) {
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
                        code: JOB_ERROR.to_owned(),
                        message,
                        elapsed_ms,
                        ..Default::default()
                    }));
                    job.stage = Stage::Reporting;
                }
            },
            Err(e) => {
                let outcome = outcome_of(&e, elapsed_ms);
                // Остановка раннера не отмена: задача остаётся в состоянии `Executing` и продолжится (D-01).
                if matches!(outcome, Outcome::Cancelled) && local_stop && !server_cancel {
                    self.log.line(&format!(
                        "задача #{id}: раннер остановлен, задача останется для продолжения"
                    ));
                    self.persist()?;
                    return Ok(Some(Tick::Stopped));
                }
                self.log
                    .line(&format!("задача #{id} не удалась: {}", e.message));
                let job = self.held();
                job.pending = Some(outcome);
                job.stage = Stage::Reporting;
            }
        }
        self.persist()?;
        Ok(None)
    }

    /// Загружает результат со смещения сервера и закрывает задачу `Done`. `None` — отмена на сервере прервала
    /// загрузку, дальше нужно сообщить итог `Cancelled`.
    fn upload(&mut self) -> Result<Option<Tick>, String> {
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

    /// Сообщает серверу сохранённый итог (`Failed` или `Cancelled`); сеть повторяется следующим `tick`.
    fn report(&mut self) -> Result<Tick, String> {
        let held = self.state.job.clone().expect("задача удерживается");
        let id = held.server_job_id;
        let outcome = held.pending.clone().unwrap_or_else(|| {
            Outcome::Failed(Failure {
                code: JOB_ERROR.to_owned(),
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
                Verdict::Retry => Err(format!("задача #{id}: finish: {e}")),
            },
        }
    }
}
