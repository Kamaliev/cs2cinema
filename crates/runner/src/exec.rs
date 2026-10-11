//! Шов к исполнению задачи: раннер знает только `Task` и `Executor`.
//!
//! Приложение фазы 8 реализует `Executor` поверх своей очереди; `PipelineExecutor` — исполнитель раннера на машине
//! автора: дочерняя предпроверка чужой демки, `pipeline::job::run_job` под защитой от паники, проверка ролика и
//! уборка после падения. В тестах вместо него стоит заглушка.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pipeline::assemble::verify_result;
use pipeline::host::Host;
use pipeline::inspect::{InspectArgs, InspectLimits, preflight};
use pipeline::job::{DemoSource, JobConfig, JobSpec, Outcome, run_job};
use pipeline::record::failure::{msg_result_invalid, msg_unsupported_source};
use pipeline::record::log::one_line;
use pipeline::{CANCELLED, FailCode, Progress, RecordFailure, RecordState, RunError};

/// Сколько ждать ffprobe при проверке готового ролика.
const VERIFY_DEADLINE: Duration = Duration::from_secs(60);

/// Задача раннера: всё, что исполнителю нужно знать о выданной сервером работе. Пути строит сам раннер из
/// номера задачи; имя файла и параметры от пользователя сюда не попадают.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Номер задачи на сервере: уникален в папке раннера и служит `JobSpec.job_id`.
    pub job_id: i64,
    /// Номер попытки с единицы.
    pub attempt: u32,
    /// Скачанная и проверенная демка: `remote/srv{job_id}.dem`.
    pub demo: PathBuf,
    /// Ссылка FACEIT, если задача пришла с ней.
    pub faceit: Option<String>,
    /// Steam64 героя или 0, если он неизвестен.
    pub hero_steam: u64,
    pub hero_name: String,
    /// Повтор по `record.json` из `out_dir`, а не заново.
    pub resume: bool,
    /// Папка результата прошлой попытки, если она известна.
    pub out_dir: Option<PathBuf>,
}

/// Исполнитель задачи.
pub trait Executor: Send + Sync {
    /// Делает ролик по задаче; `p` получает этапы и строки журнала и сообщает об отмене.
    fn run(
        &self,
        task: &Task,
        p: &dyn pipeline::Progress,
    ) -> Result<pipeline::job::Outcome, pipeline::RunError>;

    /// Убирает за прошлым запуском после падения; возвращает строки для журнала. По умолчанию ничего не делает.
    fn recover(&self, _out_dir: &Path) -> Vec<String> {
        Vec::new()
    }
}

/// Как раннер зовёт дочерний разбор демки: какой `cs2-cli` и с какими потолками.
#[derive(Debug, Clone)]
pub struct InspectSettings {
    /// Исполняемый файл с подкомандой `inspect`: у раннера это он сам (`std::env::current_exe()`).
    pub exe: PathBuf,
    pub limits: InspectLimits,
}

/// Подмена `run_job` в тестах.
type JobFn = dyn Fn(&dyn Host, &JobSpec, &dyn Progress) -> Result<Outcome, RunError> + Send + Sync;

/// Исполнитель раннера поверх `pipeline`: чужая демка до `run_job` проходит дочерний разбор, `run_job` идёт под
/// `catch_unwind` (паника не роняет раннер), а после падения раннера свои осиротевшие процессы завершаются раньше,
/// чем убирается папка игры.
pub struct PipelineExecutor {
    host: Arc<dyn Host>,
    cfg: JobConfig,
    inspect: InspectSettings,
    job_fn: Arc<JobFn>,
}

impl PipelineExecutor {
    pub fn new(host: Arc<dyn Host>, cfg: JobConfig, inspect: InspectSettings) -> Self {
        PipelineExecutor {
            host,
            cfg,
            inspect,
            job_fn: Arc::new(run_job),
        }
    }

    /// Подменяет `run_job` (только для тестов).
    #[doc(hidden)]
    pub fn with_job_fn(
        mut self,
        f: impl Fn(&dyn Host, &JobSpec, &dyn Progress) -> Result<Outcome, RunError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.job_fn = Arc::new(f);
        self
    }

    /// Дочерняя предпроверка демки: мусор, бомба, неверный игрок закрывают задачу своим кодом до запуска CS2.
    fn inspect_demo(&self, task: &Task, p: &dyn Progress) -> Result<(), RunError> {
        p.log("проверяю демку…");
        let args = InspectArgs {
            demo: task.demo.clone(),
            steam: task.hero_steam,
            nick: task.hero_name.clone(),
            flybys: self.cfg.flybys,
            limits: self.inspect.limits,
        };
        let ok = preflight(&*self.host, &self.inspect.exe, &args, &|| p.cancelled())?;
        p.log(&format!(
            "демка проверена: игрок {}, моментов {}, пик памяти разбора {} МиБ",
            ok.player_name,
            ok.moments,
            ok.peak_mem_bytes >> 20
        ));
        Ok(())
    }

    /// Проверка готового ролика до загрузки (RUN-06): негодный не уходит на сервер и остаётся у раннера уликой.
    fn verify(&self, outcome: Outcome, p: &dyn Progress) -> Result<Outcome, RunError> {
        let started = self.host.now();
        let checked = verify_result(
            &*self.host,
            &self.cfg.ffprobe,
            &outcome.result,
            VERIFY_DEADLINE,
            &|| p.cancelled(),
        );
        match checked {
            Ok(facts) => {
                p.log(&format!(
                    "ролик проверен: H.264, {}x{}, {:.1} с, faststart",
                    facts.width, facts.height, facts.duration
                ));
                Ok(outcome)
            }
            Err(reason) if reason == CANCELLED => Err(RunError::from(CANCELLED)),
            Err(reason) => {
                let f = RecordFailure::new(
                    FailCode::ResultInvalid,
                    RecordState::Done,
                    msg_result_invalid(&reason),
                    self.host.now().saturating_sub(started),
                );
                Err(RunError {
                    message: f.message.clone(),
                    fail: Some(f),
                })
            }
        }
    }

    /// Один вызов `run_job` под защитой от паники.
    fn call_job(&self, task: &Task, resume: bool, p: &dyn Progress) -> Result<Outcome, RunError> {
        let spec = JobSpec {
            job_id: task.job_id,
            attempt: task.attempt,
            record: true,
            resume,
            source: DemoSource::File(task.demo.clone()),
            hero_steam: task.hero_steam,
            hero_name: task.hero_name.clone(),
            known_demo: None,
            last_good: None,
            out_dir: if resume { task.out_dir.clone() } else { None },
            cfg: self.cfg.clone(),
        };
        let job_fn = &self.job_fn;
        match catch_unwind(AssertUnwindSafe(|| job_fn(&*self.host, &spec, p))) {
            Ok(result) => result,
            Err(payload) => {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "причина неизвестна".to_owned());
                Err(RunError::from(format!(
                    "исполнитель задачи упал: {}",
                    one_line(&text)
                )))
            }
        }
    }
}

/// Сбой «источник не поддерживается»: раннер берёт только файл демки.
fn unsupported_source() -> RunError {
    let f = RecordFailure::new(
        FailCode::UnsupportedSource,
        RecordState::Precheck,
        msg_unsupported_source(),
        Duration::ZERO,
    );
    RunError {
        message: f.message.clone(),
        fail: Some(f),
    }
}

/// Сбой `RETRY_NO_DATA`: продолжать запись нечем.
fn is_retry_no_data(e: &RunError) -> bool {
    e.fail
        .as_ref()
        .is_some_and(|f| f.code == FailCode::RetryNoData)
}

impl Executor for PipelineExecutor {
    fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        if task.faceit.is_some() && task.demo.as_os_str().is_empty() {
            return Err(unsupported_source());
        }
        // Продолжение: демка прошла проверку при первом запуске, повторять её незачем (D-01).
        if !task.resume {
            self.inspect_demo(task, p)?;
        }
        let outcome = match self.call_job(task, task.resume, p) {
            Err(e) if task.resume && is_retry_no_data(&e) => {
                p.log(&format!(
                    "продолжить нельзя ({}) — начинаю задачу заново",
                    one_line(&e.message)
                ));
                self.inspect_demo(task, p)?;
                self.call_job(task, false, p)
            }
            other => other,
        }?;
        self.verify(outcome, p)
    }

    /// Порядок обязателен (Pitfall 5): живая игра держит копию демки, и уборка без её завершения не удалит файлы.
    fn recover(&self, out_dir: &Path) -> Vec<String> {
        let mut notes = pipeline::gamedir::kill_orphans(&*self.host, out_dir);
        notes.extend(pipeline::gamedir::recover(&self.cfg.cs2_dir, out_dir));
        notes
    }
}
