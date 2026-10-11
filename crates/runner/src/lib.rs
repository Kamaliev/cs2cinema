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
};

pub use exec::{Executor, Task};
pub use link::{Link, LinkError, Token};
pub use state::{HeldJob, ResultFile, RunnerState, Stage};
pub use transfer::{TransferEnd, sha256_file};

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

pub struct Runner {
    cfg: Config,
    link: Link,
    exec: Arc<dyn Executor>,
    log: Arc<Log>,
    stop: Arc<AtomicBool>,
    poll_secs: Option<u64>,
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
            state,
        })
    }

    /// Флаг остановки `run`: поднимите его из другого потока.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
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
        match self.link.claim(poll).map_err(|e| e.to_string())? {
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
            Err(e) => return Err(e.to_string()),
        };
        self.log.line(&format!(
            "на связи с сервером {} (протокол {})",
            reply.server_version,
            wire::PROTOCOL_VERSION
        ));
        self.poll_secs = Some(reply.poll_secs);

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
            _ => {
                let why = match action {
                    Some(DirectiveAction::Cancel) => "сервер отменил задачу",
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

    /// Ведёт удерживаемую задачу по этапам с того, на котором она записана.
    fn drive(&mut self) -> Result<Tick, String> {
        let started = Instant::now();
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
                Stage::Fetched | Stage::Executing => self.execute(started)?,
                Stage::Finished => return self.upload(),
                Stage::Reporting => return self.report(),
            }
        }
    }

    /// Скачивает демку; не сошлась по SHA-256 — итог задачи `BAD_DEMO`.
    fn fetch(&mut self) -> Result<Option<Tick>, String> {
        let job = self.state.job.clone().expect("задача удерживается");
        let log = |m: &str| self.log.line(m);
        match transfer::download_demo(&self.link, &job, &self.stop, &log)? {
            TransferEnd::Done(()) => {
                self.held().stage = Stage::Fetched;
                self.persist()?;
                Ok(None)
            }
            TransferEnd::Stopped => Ok(Some(Tick::Stopped)),
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

    /// Запускает исполнитель и записывает его итог в outbox.
    fn execute(&mut self, started: Instant) -> Result<(), String> {
        {
            let held = self.held();
            held.stage = Stage::Executing;
            held.attempt += 1;
        }
        self.persist()?;
        let held = self.state.job.clone().expect("задача удерживается");
        let task = Task {
            job_id: held.server_job_id,
            attempt: held.attempt,
            demo: held.demo_path.clone(),
            faceit: held.faceit.clone(),
            hero_steam: held.hero_steam,
            hero_name: held.hero_name.clone(),
            resume: false,
            out_dir: None,
        };
        let progress = RunnerProgress {
            log: self.log.clone(),
        };
        let run = self.exec.run(&task, &progress);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let id = held.server_job_id;
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
                self.log
                    .line(&format!("задача #{id} не удалась: {}", e.message));
                let job = self.held();
                job.pending = Some(outcome_of(&e, elapsed_ms));
                job.stage = Stage::Reporting;
            }
        }
        self.persist()
    }

    /// Загружает результат со смещения сервера и закрывает задачу `Done`.
    fn upload(&mut self) -> Result<Tick, String> {
        let mut held = self.state.job.take().expect("задача удерживается");
        let id = held.server_job_id;
        let end = {
            let dir = self.cfg.dir.clone();
            let last_seq = self.state.last_seq;
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
            transfer::upload_result(
                &self.link,
                &mut held,
                &mut save,
                &self.stop,
                self.cfg.upload_chunk_limit,
                &log,
            )
        };
        match end {
            TransferEnd::Done(state) => {
                self.log.line(&format!("задача #{id}: {}", state.as_str()));
                self.persist()?;
                Ok(Tick::Settled { job_id: id, state })
            }
            TransferEnd::Stopped => {
                self.state.job = Some(held);
                self.persist()?;
                Ok(Tick::Stopped)
            }
            TransferEnd::Dropped(reason) => {
                self.state.job = Some(held);
                self.drop_job(&format!("сервер не принял результат — {reason}"))?;
                Ok(Tick::Dropped { job_id: id })
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
