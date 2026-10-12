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
use pipeline::record::failure::{msg_faceit_fetch_failed, msg_result_invalid, msg_unsupported_source};
use pipeline::record::log::one_line;
use pipeline::{CANCELLED, FailCode, Progress, RecordFailure, RecordState, RunError, Stage};

/// Сколько ждать ffprobe при проверке готового ролика.
const VERIFY_DEADLINE: Duration = Duration::from_secs(60);

/// Задача раннера: всё, что исполнителю нужно знать о выданной сервером работе. Пути строит сам раннер из
/// номера задачи; имя файла и параметры от пользователя сюда не попадают.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Номер задачи на сервере: уникален в папке раннера и служит `JobSpec.job_id`.
    pub job_id: i64,
    /// Эпоха выдачи: вместе с `job_id` называет одну выдачу задачи.
    pub epoch: i64,
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

/// Подмена скачивания демки FACEIT в тестах: `(match_id, ключ, папка)` -> путь скачанного файла. Ошибка — текст
/// `faceit` как есть; наружу из раннера идёт только фиксированная фраза (`fetch_phrase`).
type FetchFn = dyn Fn(&str, &str, &Path) -> Result<PathBuf, String> + Send + Sync;

/// Исполнитель раннера поверх `pipeline`: чужая демка до `run_job` проходит дочерний разбор, `run_job` идёт под
/// `catch_unwind` (паника не роняет раннер), а после падения раннера свои осиротевшие процессы завершаются раньше,
/// чем убирается папка игры.
pub struct PipelineExecutor {
    host: Arc<dyn Host>,
    cfg: JobConfig,
    inspect: InspectSettings,
    job_fn: Arc<JobFn>,
    fetch_fn: Arc<FetchFn>,
}

impl PipelineExecutor {
    pub fn new(host: Arc<dyn Host>, cfg: JobConfig, inspect: InspectSettings) -> Self {
        PipelineExecutor {
            host,
            cfg,
            inspect,
            job_fn: Arc::new(run_job),
            fetch_fn: Arc::new(fetch_demo),
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

    /// Подменяет скачивание демки FACEIT (только для тестов).
    #[doc(hidden)]
    pub fn with_fetch_fn(
        mut self,
        f: impl Fn(&str, &str, &Path) -> Result<PathBuf, String> + Send + Sync + 'static,
    ) -> Self {
        self.fetch_fn = Arc::new(f);
        self
    }

    /// Демка FACEIT на месте: если файла задачи нет, раннер скачивает его своим ключом по `match_id` из ссылки
    /// (первая демка матча). Скачивание идёт во временную папку рядом, затем файл переименовывается в
    /// `task.demo`; папка убирается при любом исходе. Сервер в этом не участвует (D-01).
    fn ensure_faceit_demo(&self, task: &Task, p: &dyn Progress) -> Result<(), RunError> {
        let Some(link) = task.faceit.as_deref() else {
            return Ok(());
        };
        if task.demo.is_file() {
            return Ok(());
        }
        pipeline::check(p)?;
        p.stage(Stage::Download);
        // Ссылка проверяется заново тем же строгим разбором, что и на сервере: в клиент FACEIT идёт только id.
        let Some(match_id) = wire::faceit::room_match_id(link) else {
            return Err(unsupported_source());
        };
        let key = self.cfg.faceit_key.trim();
        if key.is_empty() {
            return Err(fetch_failed(
                "у раннера не задан ключ FACEIT (переменная FACEIT_API_KEY)",
            ));
        }
        p.log("качаю демку FACEIT…");
        let tmp = task.demo.with_extension("faceit");
        let _ = std::fs::remove_dir_all(&tmp);
        let fetched = (self.fetch_fn)(&match_id, key, &tmp);
        let moved = match fetched {
            Ok(path) => place_demo(&path, &task.demo),
            Err(e) => {
                // Исходный текст ошибки остаётся в журнале раннера (его сервер не получает), без ключа и адресов.
                p.log(&format!(
                    "скачивание демки FACEIT не удалось: {}",
                    scrub_error(&e, key)
                ));
                Err(fetch_phrase(&e))
            }
        };
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(reason) = moved {
            return Err(fetch_failed(&reason));
        }
        pipeline::check(p)?;
        Ok(())
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
            // потолки чтения демки в run_job те же, что у дочерней предпроверки (флаги раннера)
            cfg: JobConfig { demo_limits: self.inspect.limits.demo, ..self.cfg.clone() },
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

/// Сбой скачивания демки FACEIT с фиксированной фразой (без адресов и ключа).
fn fetch_failed(reason: &str) -> RunError {
    let f = RecordFailure::new(
        FailCode::FaceitFetchFailed,
        RecordState::Precheck,
        msg_faceit_fetch_failed(reason),
        Duration::ZERO,
    );
    RunError {
        message: f.message.clone(),
        fail: Some(f),
    }
}

/// Фраза для друга и сервера по тексту ошибки клиента FACEIT: исходный текст может нести подписанный адрес CDN.
fn fetch_phrase(err: &str) -> String {
    if err.contains("нет демки") {
        "у матча FACEIT нет демки: матч ещё не доигран или демка удалена"
    } else if err.contains("отказал в доступе") {
        "FACEIT отказал в доступе к демке: ключу автора нужен доступ к Downloads API"
    } else if err.contains("не найдено") {
        "матч FACEIT не найден"
    } else {
        "не удалось скачать демку FACEIT: ошибка сети или FACEIT API"
    }
    .to_owned()
}

/// Текст ошибки для журнала раннера: одна строка, значение ключа заменено на `***`, адреса — на `<адрес>`.
fn scrub_error(err: &str, key: &str) -> String {
    let text = if key.is_empty() {
        err.to_owned()
    } else {
        err.replace(key, "***")
    };
    one_line(&text)
        .split_whitespace()
        .map(|w| {
            let l = w.to_ascii_lowercase();
            if l.contains("http://") || l.contains("https://") {
                "<адрес>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Скачанный файл становится демкой задачи (`remote/srv{id}.dem`): формат решает сигнатура, а не расширение.
fn place_demo(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| "не удалось подготовить папку для демки FACEIT".to_owned())?;
    }
    std::fs::rename(from, to).map_err(|_| "не удалось сохранить скачанную демку FACEIT".to_owned())
}

/// Настоящее скачивание: первая демка матча (в bo3 их несколько, A9) через `faceit::Client` ключом автора.
fn fetch_demo(match_id: &str, key: &str, dir: &Path) -> Result<PathBuf, String> {
    let client = faceit::Client::new(key);
    let urls = client.demo_urls(match_id).map_err(|e| e.to_string())?;
    let url = urls.first().ok_or("у матча нет демки")?;
    client.download(url, dir).map_err(|e| e.to_string())
}

/// Сбой «источник не поддерживается»: ссылка в задаче не прошла строгий шаблон комнаты матча FACEIT.
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
        // Без пути для файла демки скачивать некуда: раннер всегда задаёт `remote/srv{id}.dem`.
        if task.faceit.is_some() && task.demo.as_os_str().is_empty() {
            return Err(unsupported_source());
        }
        // Продолжение: демка прошла проверку при первом запуске, повторять её незачем (D-01).
        if !task.resume {
            self.ensure_faceit_demo(task, p)?;
            self.inspect_demo(task, p)?;
        }
        let outcome = match self.call_job(task, task.resume, p) {
            Err(e) if task.resume && is_retry_no_data(&e) => {
                p.log(&format!(
                    "продолжить нельзя ({}) — начинаю задачу заново",
                    one_line(&e.message)
                ));
                self.ensure_faceit_demo(task, p)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_text_hides_the_key_and_addresses() {
        let t = scrub_error(
            "io: reset https://cdn.example/d.dem.zst?token=abc ключ k-1\nвторая строка",
            "k-1",
        );
        assert!(!t.contains("k-1") && !t.contains("cdn.example") && !t.contains("token=abc"), "{t}");
        assert!(t.contains("***") && t.contains("<адрес>"), "{t}");
        assert!(!t.contains('\n'));
    }

    #[test]
    fn phrases_are_fixed_per_error_class() {
        assert!(fetch_phrase("у матча нет демки (x)").contains("нет демки"));
        assert!(fetch_phrase("FACEIT отказал в доступе (y)").contains("отказал в доступе"));
        assert!(fetch_phrase("FACEIT: не найдено (матч)").contains("не найден"));
        assert!(fetch_phrase("что угодно https://a/b").starts_with("не удалось скачать"));
    }
}
