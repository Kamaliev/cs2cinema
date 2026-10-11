//! Очередь задач в своём потоке: окно может быть скрыто в трей и не перерисовываться, а задачи всё равно идут по одной
//! (CS2 пишет только один экземпляр). Паника задачи не вешает очередь: задача получает статус «ошибка», очередь идёт дальше.
//!
//! Окно очередью не управляет, а только читает общий вид (`Queue::view`) и шлёт команды (`wake`, `resume`).
//! Отмена выполняемой задачи по-прежнему идёт через токен из вида: кнопка действует сразу, не дожидаясь потока.

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::Duration,
};

use pipeline::RunError;
use store::{
    Job, JobOutcome, Store,
    journal::{self, StartKind},
};

use crate::{
    app::{Msg, Reporter},
    jobs::{self, CancelToken, JobReporter},
    ui,
};

/// Команда потоку очереди. Новые задачи отдельной команды не требуют: постановка идёт через `Queue::resume`, а иначе
/// поток находит их на очередном такте.
pub enum Cmd {
    /// Снять паузу и не ждать следующего такта (обрабатывается потоком по порядку, поэтому не теряется, если пришла во время задачи).
    Resume,
}

/// Причина паузы очереди (D-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// Пользователь отменил выполняемую задачу.
    Cancelled,
    /// При запуске приложения нашлась задача, прерванная его закрытием.
    Interrupted,
}

/// Что окно видит в очереди: выполняемая задача с токеном отмены и пауза с причиной.
#[derive(Clone, Default)]
pub struct QueueView {
    pub running: Option<(i64, CancelToken)>,
    pub paused: Option<PauseReason>,
}

/// Настройки потока очереди (в тестах такт короткий и ролик не открывается).
pub struct QueueConfig {
    /// Как часто поток смотрит в базу, если никто не разбудил.
    pub tick: Duration,
    /// Открывать готовый ролик в плеере после удачной задачи.
    pub open_results: bool,
}

impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig { tick: Duration::from_millis(500), open_results: true }
    }
}

/// Исполнитель одной задачи: настоящий — `jobs::runner`, в тестах — заглушка.
pub type Runner = Arc<dyn Fn(&Job, &JobReporter) -> Result<JobOutcome, RunError> + Send + Sync>;

/// Ручка очереди для окна.
pub struct Queue {
    tx: Sender<Cmd>,
    view: Arc<Mutex<QueueView>>,
}

impl Queue {
    /// Снять паузу. Её снимают «Продолжить», постановка новой задачи, «Повторить» и «Заново» — и только они.
    /// Вид меняется сразу, чтобы баннер исчез в этом же кадре; поток подтверждает это своей командой по порядку,
    /// поэтому пауза от задачи, которая ещё заканчивается, не переживёт команду.
    pub fn resume(&self) {
        self.view_mut(|v| v.paused = None);
        let _ = self.tx.send(Cmd::Resume);
    }

    /// Снимок вида. Отравленный замок — вид по умолчанию, без паники.
    pub fn view(&self) -> QueueView {
        self.view.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn view_mut(&self, f: impl FnOnce(&mut QueueView)) {
        if let Ok(mut v) = self.view.lock() {
            f(&mut v);
        }
    }
}

/// Запустить поток очереди. `paused` — пауза с самого начала (после падения приложения: задача «прервана»).
pub fn start(store: Store, runner: Runner, ui: Reporter, paused: Option<PauseReason>, cfg: QueueConfig) -> Queue {
    let (tx, rx) = channel();
    let view = Arc::new(Mutex::new(QueueView { running: None, paused }));
    let shared = view.clone();
    let ui_thread = ui.clone();
    let spawned = std::thread::Builder::new().name("cs2cinema-queue".into()).spawn(move || {
        let ui = ui_thread;
        let set = |f: &dyn Fn(&mut QueueView)| {
            if let Ok(mut v) = shared.lock() {
                f(&mut v);
            }
        };
        let mut last_error = String::new();
        loop {
            match rx.recv_timeout(cfg.tick) {
                Ok(Cmd::Resume) => set(&|v| v.paused = None),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if shared.lock().map_or(true, |v| v.paused.is_some()) {
                continue;
            }
            let job = match store.next_queued() {
                Ok(Some(job)) => job,
                Ok(None) => continue,
                Err(e) => {
                    note_error(&ui, &mut last_error, &e);
                    continue;
                }
            };
            let attempt = match store.start_job(job.id) {
                Ok(n) => n,
                Err(e) => {
                    note_error(&ui, &mut last_error, &e);
                    continue;
                }
            };
            last_error.clear();
            run_one(&store, &runner, &ui, &cfg, &set, job, attempt);
        }
    });
    if let Err(e) = spawned {
        ui.send(Msg::Log(format!("очередь не запустилась: {e}")));
    }
    Queue { tx, view }
}

/// Ошибка базы в окно один раз, а не на каждом такте.
fn note_error(ui: &Reporter, last: &mut String, e: &str) {
    if last != e {
        ui.send(Msg::Log(format!("очередь: {e}")));
        *last = e.to_owned();
    }
}

/// Одна задача от «выполняется» до итога. Осознанная отмена ставит очередь на паузу до отправки `JobDone`.
fn run_one(store: &Store, runner: &Runner, ui: &Reporter, cfg: &QueueConfig, set: &dyn Fn(&dyn Fn(&mut QueueView)), job: Job, attempt: u32) {
    // журнал не стирается: каждая попытка открывается разделителем с номером, датой и способом запуска (D-10)
    let kind = if job.resume {
        StartKind::Retry
    } else if attempt <= 1 {
        StartKind::First
    } else {
        StartKind::Redo
    };
    let _ = store.append_log(job.id, &journal::separator(attempt, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), kind));
    let cancel = CancelToken::default();
    let running = Some((job.id, cancel.clone()));
    set(&|v| v.running = running.clone());
    ui.send(Msg::JobStarted { title: format!("Задача #{}: {} · {}", job.id, job.label, job.kind.title()) });
    let rep = JobReporter::new(store.clone(), job.id, ui.clone(), cancel, attempt);
    let result = match catch_unwind(AssertUnwindSafe(|| runner(&job, &rep))) {
        Ok(r) => r,
        Err(payload) => Err(RunError::from(format!("внутренняя ошибка: {}", panic_text(&*payload)))),
    };
    // длительность последнего этапа — до итога попытки (успех, сбой, отмена и паника)
    rep.close_stage();
    let _ = jobs::finish(store, job.id, &result);
    // готовый ролик открываем в плеере здесь, а не в `run_job`: тесты `pipeline` плеер не запускают
    if cfg.open_results {
        if let Ok(o) = &result {
            if o.result.extension().is_some_and(|e| e == "mp4") {
                ui::open_path(&o.result);
            }
        }
    }
    // только осознанная отмена останавливает очередь, сбой и успех — нет (D-13)
    let cancelled = matches!(&result, Err(e) if e.message == pipeline::CANCELLED);
    set(&|v| {
        v.running = None;
        if cancelled {
            v.paused = Some(PauseReason::Cancelled);
        }
    });
    ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result).map_err(|e| e.message) });
}

/// Текст паники из полезной нагрузки: `&str` или `String`.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "без текста".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, time::Instant};

    use pipeline::{
        FailCode, RecordFailure, RecordState,
        host::fake::{FakeHost, Scenario},
    };
    use store::{JobKind, JobSource, JobStatus};

    use crate::settings::Settings;

    type Behavior = Box<dyn FnMut() -> Result<JobOutcome, RunError> + Send>;

    fn ok() -> Behavior {
        Box::new(|| Ok(JobOutcome { out_dir: "out".into(), result: "out/highlights.mp4".into(), shots: 1, secs: 1.0 }))
    }

    /// Исполнитель-заглушка: поведение по подписи задачи; порядок стартов пишется в `started`.
    fn stub(behaviors: Vec<(&str, Behavior)>) -> (Runner, Arc<Mutex<Vec<String>>>) {
        let map: Mutex<HashMap<String, Behavior>> = Mutex::new(behaviors.into_iter().map(|(k, b)| (k.to_owned(), b)).collect());
        let started = Arc::new(Mutex::new(Vec::new()));
        let log = started.clone();
        let runner: Runner = Arc::new(move |job, _rep| {
            log.lock().unwrap().push(job.label.clone());
            // замок отпускается до вызова: паника поведения не отравляет таблицу
            let mut f = map.lock().unwrap().remove(&job.label).expect("поведение задачи не задано");
            f()
        });
        (runner, started)
    }

    fn add(store: &Store, label: &str) -> i64 {
        store
            .add_job(&store::NewJob {
                kind: JobKind::Record,
                source: JobSource::Faceit("1-abc".into()),
                label: label.into(),
                hero_name: "me".into(),
                hero_steam: 42,
                map: "de_nuke".into(),
            })
            .unwrap()
    }

    fn cfg() -> QueueConfig {
        QueueConfig { tick: Duration::from_millis(10), open_results: false }
    }

    fn wait_for(store: &Store, id: i64, status: JobStatus) {
        let begin = Instant::now();
        while begin.elapsed() < Duration::from_secs(5) {
            if store.job(id).unwrap().unwrap().status == status {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let j = store.job(id).unwrap().unwrap();
        panic!("задача #{id} не стала {status:?} за 5 с: {:?}\n{}", j.status, j.log);
    }

    #[test]
    fn a_failed_job_does_not_stop_the_queue() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (add(&store, "первая"), add(&store, "вторая"));
        let failing: Behavior = Box::new(|| {
            let f = RecordFailure::new(FailCode::TakeTimeout, RecordState::Take, "дубль не записался".into(), Duration::from_secs(3));
            Err(RunError { message: f.message.clone(), fail: Some(f) })
        });
        let (runner, started) = stub(vec![("первая", failing), ("вторая", ok())]);
        let q = start(store.clone(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, b, JobStatus::Done);
        let first = store.job(a).unwrap().unwrap();
        assert_eq!(first.status, JobStatus::Failed);
        assert_eq!(first.fail_code.as_deref(), Some("TAKE_TIMEOUT"));
        assert!(first.log.contains("итог попытки 1: сбой TAKE_TIMEOUT"), "{}", first.log);
        assert_eq!(*started.lock().unwrap(), ["первая", "вторая"]);
        assert_eq!(q.view().paused, None, "сбой очередь не останавливает");
    }

    #[test]
    fn a_panicking_job_fails_and_the_queue_goes_on() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (add(&store, "первая"), add(&store, "вторая"));
        let panicking: Behavior = Box::new(|| panic!("рассыпалось"));
        let (runner, _) = stub(vec![("первая", panicking), ("вторая", ok())]);
        let q = start(store.clone(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, b, JobStatus::Done);
        let first = store.job(a).unwrap().unwrap();
        assert_eq!(first.status, JobStatus::Failed);
        let error = first.error.unwrap();
        assert!(error.starts_with("внутренняя ошибка:") && error.contains("рассыпалось"), "{error}");
        assert_eq!(q.view().running.map(|(id, _)| id), None);
        assert_eq!(q.view().paused, None, "паника очередь не останавливает");
    }

    #[test]
    fn real_runner_failure_reaches_the_row_and_the_queue_goes_on() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (add(&store, "первая"), add(&store, "вторая"));
        // «Повторить» без папки задачи: повторять нечего — типизированный сбой RETRY_NO_DATA
        store.requeue_resume(a).unwrap();
        store.requeue_resume(b).unwrap();
        let runner = jobs::runner(Arc::new(Settings::default), Arc::new(FakeHost::new(Scenario::happy(1))));
        let _q = start(store.clone(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Failed);
        wait_for(&store, b, JobStatus::Failed);
        for id in [a, b] {
            let j = store.job(id).unwrap().unwrap();
            assert_eq!(j.fail_code.as_deref(), Some("RETRY_NO_DATA"), "{}", j.log);
        }
    }
}
