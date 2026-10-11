//! Очередь задач в своём потоке: окно может быть скрыто в трей и не перерисовываться, а задачи всё равно идут по одной
//! (CS2 пишет только один экземпляр). Паника задачи не вешает очередь: задача получает статус «ошибка», очередь идёт дальше.
//!
//! Окно очередью не управляет, а только читает общий вид (`Queue::view`) и шлёт команды (`wake`, `resume`).
//! Отмена выполняемой задачи по-прежнему идёт через токен из вида: кнопка действует сразу, не дожидаясь потока.
//!
//! Политика очереди без присмотра (`QueuePolicy`) чистая: ни часов, ни ввода-вывода. Она решает, что делать после
//! задачи: идти дальше, повторить сбой запуска один раз или встать на паузу (DIF-03, DIF-04).

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use pipeline::{
    FailCode, RecordState, RunError,
    host::Host,
    record::failure::{FailClass, classify},
};
use store::{
    Job, JobOutcome, Store,
    journal::{self, StartKind},
};

use crate::{
    app::{Msg, Reporter},
    jobs::{self, CancelToken, JobReporter},
    ui,
};

/// Пауза перед авто-повтором: игра и HLAE успевают отпустить порт, окно и файлы (DC-4). Полная уборка прошлой попытки
/// уже сделана самой записью; здесь только время.
pub const AUTO_RETRY_PAUSE: Duration = Duration::from_secs(20);

/// Сколько авто-повторов получает задача. Счётчик `auto_retries` живёт в базе и обнуляется только ручным запуском (T-04-21).
pub const AUTO_RETRY_BUDGET: u32 = 1;

/// Команда потоку очереди. Новые задачи отдельной команды не требуют: постановка идёт через `Queue::resume`, а иначе
/// поток находит их на очередном такте.
pub enum Cmd {
    /// Снять паузу и не ждать следующего такта (обрабатывается потоком по порядку, поэтому не теряется, если пришла во время задачи).
    Resume,
}

/// Причина паузы очереди (D-16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseReason {
    /// Пользователь отменил выполняемую задачу.
    Cancelled,
    /// При запуске приложения нашлась задача, прерванная его закрытием.
    Interrupted,
}

/// Что окно видит в очереди: выполняемая задача с токеном отмены, пауза с причиной и «пауза для машины» перед авто-повтором.
#[derive(Clone, Default)]
pub struct QueueView {
    pub running: Option<(i64, CancelToken)>,
    pub paused: Option<PauseReason>,
    /// Задача, которая ждёт авто-повтора, и момент, до которого очередь не берёт задач (DC-4).
    pub hold: Option<(i64, Instant)>,
}

/// Настройки потока очереди (в тестах такт короткий и ролик не открывается).
pub struct QueueConfig {
    /// Как часто поток смотрит в базу, если никто не разбудил.
    pub tick: Duration,
    /// Открывать готовый ролик в плеере после удачной задачи.
    pub open_results: bool,
    /// Пауза перед авто-повтором сбоя запуска.
    pub retry_pause: Duration,
}

impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig { tick: Duration::from_millis(500), open_results: true, retry_pause: AUTO_RETRY_PAUSE }
    }
}

/// Итог задачи для политики: только то, что она должна знать. Код берётся из результата исполнителя до любых `requeue_*`,
/// потому что они стирают `fail_*` в базе.
#[derive(Debug, Clone, PartialEq)]
pub enum Finished {
    Done,
    Cancelled,
    Failed { id: i64, code: Option<FailCode>, state: Option<RecordState>, message: String, hint: String },
}

impl Finished {
    fn of(id: i64, result: &Result<JobOutcome, RunError>) -> Finished {
        match result {
            Ok(_) => Finished::Done,
            Err(e) if e.message == pipeline::CANCELLED => Finished::Cancelled,
            Err(RunError { fail: Some(f), .. }) => {
                Finished::Failed { id, code: Some(f.code), state: Some(f.state), message: f.message.clone(), hint: f.hint.clone() }
            }
            Err(e) => Finished::Failed { id, code: None, state: None, message: e.message.clone(), hint: String::new() },
        }
    }
}

/// Решение политики после задачи.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Идти к следующей задаче.
    Next,
    /// Поставить эту же задачу «с записи» после паузы `retry_pause`.
    AutoRetry,
    /// Остановить очередь до «Продолжить».
    Pause(PauseReason),
}

/// Политика очереди без присмотра (DIF-03, DIF-04). Чистая: решает по итогам задач, ни часов, ни базы.
#[derive(Default)]
pub struct QueuePolicy;

impl QueuePolicy {
    /// Что делать после задачи. `auto_retries_done` — сколько авто-повторов эта задача уже получила до текущей попытки.
    pub fn after(&mut self, auto_retries_done: u32, f: &Finished) -> Verdict {
        match f {
            Finished::Done => Verdict::Next,
            Finished::Cancelled => Verdict::Pause(PauseReason::Cancelled),
            Finished::Failed { code: Some(code), state: Some(state), .. }
                if classify(*code, *state) == FailClass::Launch && auto_retries_done < AUTO_RETRY_BUDGET =>
            {
                Verdict::AutoRetry
            }
            Finished::Failed { .. } => Verdict::Next,
        }
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

/// Всё, что нужно потоку очереди для одной задачи.
struct Worker {
    store: Store,
    host: Arc<dyn Host>,
    runner: Runner,
    ui: Reporter,
    cfg: QueueConfig,
    shared: Arc<Mutex<QueueView>>,
    policy: QueuePolicy,
}

impl Worker {
    fn set(&self, f: &dyn Fn(&mut QueueView)) {
        if let Ok(mut v) = self.shared.lock() {
            f(&mut v);
        }
    }
}

/// Запустить поток очереди. `paused` — пауза с самого начала (после падения приложения: задача «прервана»).
/// `host` нужен для уведомлений в трее.
pub fn start(store: Store, host: Arc<dyn Host>, runner: Runner, ui: Reporter, paused: Option<PauseReason>, cfg: QueueConfig) -> Queue {
    let (tx, rx) = channel();
    let view = Arc::new(Mutex::new(QueueView { running: None, paused, hold: None }));
    let mut worker = Worker { store, host, runner, ui: ui.clone(), cfg, shared: view.clone(), policy: QueuePolicy::default() };
    let spawned = std::thread::Builder::new().name("cs2cinema-queue".into()).spawn(move || {
        let mut last_error = String::new();
        loop {
            match rx.recv_timeout(worker.cfg.tick) {
                Ok(Cmd::Resume) => worker.set(&|v| v.paused = None),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if worker.shared.lock().map_or(true, |v| v.paused.is_some()) {
                continue;
            }
            // «пауза для машины» перед авто-повтором: задач не берём, пока она не истекла (DC-4)
            let hold = worker.shared.lock().ok().and_then(|v| v.hold);
            if let Some((_, until)) = hold {
                if Instant::now() < until {
                    continue;
                }
                worker.set(&|v| v.hold = None);
            }
            let job = match worker.store.next_queued() {
                Ok(Some(job)) => job,
                Ok(None) => continue,
                Err(e) => {
                    note_error(&worker.ui, &mut last_error, &e);
                    continue;
                }
            };
            let attempt = match worker.store.start_job(job.id) {
                Ok(n) => n,
                Err(e) => {
                    note_error(&worker.ui, &mut last_error, &e);
                    continue;
                }
            };
            last_error.clear();
            worker.run_one(job, attempt);
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

impl Worker {
    /// Одна задача от «выполняется» до итога. Решение политики применяется до отправки `JobDone`, чтобы окно в
    /// этом же сообщении видело и паузу, и новый статус задачи.
    fn run_one(&mut self, job: Job, attempt: u32) {
        // журнал не стирается: каждая попытка открывается разделителем с номером, датой и способом запуска (D-10);
        // авто-повтор остаётся «авто-повтором» и после перезапуска приложения: его выдают `resume` и счётчик в базе
        let kind = if job.resume && job.auto_retries > 0 {
            StartKind::Auto
        } else if job.resume {
            StartKind::Retry
        } else if attempt <= 1 {
            StartKind::First
        } else {
            StartKind::Redo
        };
        let when = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let _ = self.store.append_log(job.id, &journal::separator(attempt, &when, kind));
        let cancel = CancelToken::default();
        let running = Some((job.id, cancel.clone()));
        self.set(&|v| v.running = running.clone());
        self.ui.send(Msg::JobStarted { title: format!("Задача #{}: {} · {}", job.id, job.label, job.kind.title()) });
        let rep = JobReporter::new(self.store.clone(), job.id, self.ui.clone(), cancel, attempt);
        let runner = &self.runner;
        let result = match catch_unwind(AssertUnwindSafe(|| runner(&job, &rep))) {
            Ok(r) => r,
            Err(payload) => Err(RunError::from(format!("внутренняя ошибка: {}", panic_text(&*payload)))),
        };
        // длительность последнего этапа — до итога попытки (успех, сбой, отмена и паника)
        rep.close_stage();
        let _ = jobs::finish(&self.store, job.id, &result);
        // готовый ролик открываем в плеере здесь, а не в `run_job`: тесты `pipeline` плеер не запускают
        if self.cfg.open_results {
            if let Ok(o) = &result {
                if o.result.extension().is_some_and(|e| e == "mp4") {
                    ui::open_path(&o.result);
                }
            }
        }
        // итог берётся до любых `requeue_*`: они стирают `fail_*` в базе
        let finished = Finished::of(job.id, &result);
        match self.policy.after(job.auto_retries, &finished) {
            Verdict::AutoRetry => self.auto_retry(job.id, &finished),
            Verdict::Pause(reason) => self.set(&|v| v.paused = Some(reason.clone())),
            Verdict::Next => {}
        }
        self.set(&|v| v.running = None);
        self.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result).map_err(|e| e.message) });
    }

    /// Поставить задачу в очередь «с записи» и задержать очередь на `retry_pause`. Причина и счётчик остаются в журнале
    /// и в базе: приёмка отличает авто-повтор от ручного «Повторить» по сохранённым данным (T-04-23).
    fn auto_retry(&self, id: i64, f: &Finished) {
        let Finished::Failed { code, state, .. } = f else { return };
        let code = code.map_or("?", |c| c.as_str());
        let state = state.map_or("?", |s| s.title());
        let line = format!(
            "авто-повтор: сбой {code} на этапе «{state}» (класс «{}»); пауза {} с, затем запись заново по record.json",
            FailClass::Launch.title(),
            self.cfg.retry_pause.as_secs()
        );
        let _ = self.store.append_log(id, &line);
        self.ui.send(Msg::Log(line));
        match self.store.requeue_auto(id) {
            Ok(()) => {
                let until = Instant::now() + self.cfg.retry_pause;
                self.set(&|v| v.hold = Some((id, until)));
            }
            Err(e) => self.ui.send(Msg::Log(format!("авто-повтор не поставлен: {e}"))),
        }
    }
}

/// Текст паники из полезной нагрузки: `&str` или `String`.
fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "без текста".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    use pipeline::{
        RecordFailure,
        host::{
            CmdOutput,
            fake::{FakeHost, Scenario},
        },
        manifest::{Manifest, RecordJson, TakeSpec},
        record::verify::TakeFacts,
    };
    use renderer::TakePlan;
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
        QueueConfig { tick: Duration::from_millis(10), open_results: false, retry_pause: Duration::from_millis(50) }
    }

    /// Хост для тестов, которым игра не нужна: исполнитель — заглушка.
    fn host() -> Arc<dyn Host> {
        Arc::new(FakeHost::new(Scenario::happy(1)))
    }

    /// Типизированный сбой записи, как его возвращает исполнитель.
    fn failure(code: FailCode, state: RecordState, message: &str) -> RunError {
        let f = RecordFailure::new(code, state, message.into(), Duration::from_secs(3));
        RunError { message: f.message.clone(), fail: Some(f) }
    }

    fn failed(id: i64, code: FailCode, state: RecordState) -> Finished {
        Finished::Failed { id, code: Some(code), state: Some(state), message: "сообщение".into(), hint: "подсказка".into() }
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

    /// Пока очередь на паузе, вторая задача остаётся в очереди не меньше пяти тактов.
    fn stays_queued(store: &Store, id: i64) {
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(store.job(id).unwrap().unwrap().status, JobStatus::Queued, "на паузе очередь задач не берёт");
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
        let q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

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
        let q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

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
        let _q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Failed);
        wait_for(&store, b, JobStatus::Failed);
        for id in [a, b] {
            let j = store.job(id).unwrap().unwrap();
            assert_eq!(j.fail_code.as_deref(), Some("RETRY_NO_DATA"), "{}", j.log);
        }
    }

    #[test]
    fn cancel_pauses_the_queue_until_resume() {
        let store = Store::in_memory().unwrap();
        let (a, b) = (add(&store, "первая"), add(&store, "вторая"));
        let cancelled: Behavior = Box::new(|| Err(RunError::from(pipeline::CANCELLED)));
        let (runner, _) = stub(vec![("первая", cancelled), ("вторая", ok())]);
        let q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Cancelled);
        stays_queued(&store, b);
        assert_eq!(q.view().paused, Some(PauseReason::Cancelled));
        q.resume();
        wait_for(&store, b, JobStatus::Done);
        assert_eq!(q.view().paused, None);
    }

    #[test]
    fn interrupted_start_waits_for_resume() {
        let store = Store::in_memory().unwrap();
        let a = add(&store, "первая");
        let (runner, started) = stub(vec![("первая", ok())]);
        let q = start(store.clone(), host(), runner, Reporter::detached(), Some(PauseReason::Interrupted), cfg());

        stays_queued(&store, a);
        assert!(started.lock().unwrap().is_empty());
        assert_eq!(q.view().paused, Some(PauseReason::Interrupted));
        q.resume();
        wait_for(&store, a, JobStatus::Done);
    }

    #[test]
    fn resume_clears_any_pause() {
        let store = Store::in_memory().unwrap();
        let a = add(&store, "первая");
        let cancelled: Behavior = Box::new(|| Err(RunError::from(pipeline::CANCELLED)));
        let (runner, _) = stub(vec![("первая", cancelled)]);
        let q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Cancelled);
        let begin = Instant::now();
        while q.view().paused.is_none() && begin.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(q.view().paused, Some(PauseReason::Cancelled));
        q.resume();
        assert_eq!(q.view().paused, None, "«Продолжить», новая задача и «Повторить» снимают паузу сразу");
    }

    #[test]
    fn policy_retries_a_launch_failure_once_and_leaves_the_rest() {
        let mut p = QueuePolicy::default();
        let console = failed(1, FailCode::ConsoleNotOpen, RecordState::Console);
        assert_eq!(p.after(0, &console), Verdict::AutoRetry);
        assert_ne!(p.after(1, &console), Verdict::AutoRetry, "бюджет авто-повтора — один");
        assert_eq!(p.after(0, &failed(1, FailCode::GameExited, RecordState::Take)), Verdict::Next, "смерть игры посреди дубля не повторяется");
        assert_eq!(p.after(0, &failed(1, FailCode::GameExited, RecordState::Demo)), Verdict::AutoRetry, "игра умерла до первого дубля");
        assert_eq!(p.after(0, &failed(1, FailCode::TakeInvalid, RecordState::Take)), Verdict::Next);
        let untyped = Finished::Failed { id: 1, code: None, state: None, message: "внутренняя ошибка".into(), hint: String::new() };
        assert_eq!(p.after(0, &untyped), Verdict::Next, "нетипизированный сбой не повторяется");
        assert_eq!(p.after(0, &Finished::Done), Verdict::Next);
        assert_eq!(p.after(0, &Finished::Cancelled), Verdict::Pause(PauseReason::Cancelled));
    }

    #[test]
    fn launch_failure_is_retried_once_after_the_pause_and_logged_as_auto() {
        let store = Store::in_memory().unwrap();
        let a = add(&store, "первая");
        let starts = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let ends = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let (s, e) = (starts.clone(), ends.clone());
        let runner: Runner = Arc::new(move |_job, _rep| {
            let n = {
                let mut s = s.lock().unwrap();
                s.push(Instant::now());
                s.len()
            };
            let result = if n == 1 {
                Err(failure(FailCode::ConsoleNotOpen, RecordState::Console, "консольный порт не открылся"))
            } else {
                Ok(JobOutcome { out_dir: "out".into(), result: "out/highlights.mp4".into(), shots: 1, secs: 1.0 })
            };
            e.lock().unwrap().push(Instant::now());
            result
        });
        let _q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Done);
        let j = store.job(a).unwrap().unwrap();
        assert_eq!((j.attempt, j.auto_retries), (2, 1), "{}", j.log);
        assert_eq!(journal::attempts(&j.log), vec![(1, StartKind::First), (2, StartKind::Auto)], "{}", j.log);
        assert!(j.log.contains("авто-повтор: сбой CONSOLE_NOT_OPEN"), "{}", j.log);
        let (starts, ends) = (starts.lock().unwrap(), ends.lock().unwrap());
        assert!(starts[1].duration_since(ends[0]) >= Duration::from_millis(50), "вторая попытка раньше паузы");
    }

    #[test]
    fn second_launch_failure_is_not_retried() {
        let store = Store::in_memory().unwrap();
        let a = add(&store, "первая");
        let calls = Arc::new(Mutex::new(0u32));
        let c = calls.clone();
        let runner: Runner = Arc::new(move |_job, _rep| {
            *c.lock().unwrap() += 1;
            Err(failure(FailCode::ConsoleNotOpen, RecordState::Console, "консольный порт не открылся"))
        });
        let _q = start(store.clone(), host(), runner, Reporter::detached(), None, cfg());

        let begin = Instant::now();
        while *calls.lock().unwrap() < 2 && begin.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for(&store, a, JobStatus::Failed);
        // времени на третью попытку (пауза 50 мс, такт 10 мс) с запасом
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*calls.lock().unwrap(), 2, "ровно две попытки");
        let j = store.job(a).unwrap().unwrap();
        assert_eq!((j.status, j.auto_retries, j.attempt), (JobStatus::Failed, 1, 2), "{}", j.log);
    }

    /// Дерево HLAE и CS2 во временной папке, где предпроверка находит всё нужное; CS2 — в Steam-раскладке со сборкой.
    fn tree_settings(tag: &str) -> (Settings, PathBuf) {
        let root = std::env::temp_dir().join(format!("cs2cinema-queue-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("lib/steamapps/common/Counter-Strike Global Offensive");
        for f in ["hlae/HLAE.exe", "hlae/x64/AfxHookSource2.dll", "hlae/ffmpeg/bin/ffmpeg.exe"] {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let exe = game.join("game/bin/win64/cs2.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(exe, b"x").unwrap();
        std::fs::write(
            root.join("lib/steamapps/appmanifest_730.acf"),
            "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n\t\"TargetBuildID\"\t\t\"0\"\n}\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        let s = Settings {
            hlae_dir: root.join("hlae").to_string_lossy().into_owned(),
            cs2_dir: game.to_string_lossy().into_owned(),
            out_root: root.join("out").to_string_lossy().into_owned(),
            ..Settings::default()
        };
        (s, root)
    }

    /// Папка задачи, как её оставляет запись: скрипты, `record.json`, дубль 1 проверен, дубль 2 негодный.
    fn job_dir_with_invalid_take(root: &Path) -> PathBuf {
        let dir = root.join("out/match_hero");
        let nonce = "ab12cd34";
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("highlights.cfg"), format!("echo CS2C {nonce} SCRIPT\nexec cs2c_01\n")).unwrap();
        std::fs::write(dir.join("cs2c_01.cfg"), format!("echo CS2C {nonce} TAKE_END 0\n")).unwrap();
        let hash = pipeline::job::plan_hash(&dir, nonce);
        let takes: Vec<TakePlan> = (0..2u32).map(|index| TakePlan { index, shot: index as usize, game_secs: 5.0, capture_fps: 60 }).collect();
        let demo = dir.join("match.dem");
        std::fs::write(&demo, b"PBDEMS2\0demo").unwrap();
        let mut args: Vec<String> = vec!["-y".into()];
        for t in &takes {
            for f in TakePlan::FILES {
                args.extend(["-i".to_owned(), format!("clips/{}/{f}", t.dir_name())]);
            }
        }
        args.extend(["-map", "[v]", "highlights.mp4"].map(String::from));
        RecordJson {
            version: 1,
            nonce: nonce.into(),
            demo_path: demo,
            demo_name: "cs2cinema_match_hero".into(),
            demo_len: 12,
            frame: (1280, 720),
            fps: 60,
            take_nvenc: false,
            takes: takes.iter().map(TakeSpec::from).collect(),
            ffmpeg_args: args,
            video_secs: 10.0,
            shots: takes.len(),
            secs: 10.0,
            plan_hash: hash.clone(),
            created_unix: 1,
        }
        .save(&dir)
        .unwrap();
        let clips = dir.join("clips");
        let mut m = Manifest::new(hash, nonce, (1280, 720), &takes);
        let facts = TakeFacts { video_secs: 5.0, audio_secs: 5.0, width: 1280, height: 720, video_bytes: 10, audio_bytes: 4 };
        for t in &takes {
            let d = clips.join(t.dir_name());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("video.mp4"), [b'v'; 10]).unwrap();
            std::fs::write(d.join("audio.wav"), [b'a'; 4]).unwrap();
            if t.index == 0 {
                m.mark_validated(t.index, &facts, 1);
            } else {
                m.mark_invalid(t.index, "обрезан", 1);
            }
        }
        m.save(&clips).unwrap();
        // поддельный ffmpeg склейки файлов не пишет: временный ролик кладёт сам тест
        std::fs::write(dir.join("highlights.part.mp4"), b"new").unwrap();
        dir
    }

    #[test]
    fn auto_retry_runs_through_the_real_resume_path() {
        let (settings, root) = tree_settings("auto");
        let dir = job_dir_with_invalid_take(&root);
        let store = Store::in_memory().unwrap();
        let a = add(&store, "первая");
        store.set_out_dir(a, &dir).unwrap();
        // «Повторить» вручную: с записи по record.json
        store.requeue_resume(a).unwrap();

        let ok = CmdOutput { code: Some(0), stderr_tail: vec![], elapsed: Duration::ZERO };
        let scenario = |s: Scenario| {
            s.clips_dir(dir.join("clips"))
                .program_for("ffmpeg", "-version", &["ffmpeg version 7.1.1 Copyright (c) the FFmpeg developers"], Ok(ok.clone()))
                .program_for("ffprobe", "-version", &["ffprobe version 7.1.1 Copyright (c) the FFmpeg developers"], Ok(ok.clone()))
        };
        let fake = Arc::new(FakeHost::sequence(vec![scenario(Scenario::happy(2).console_never_up()), scenario(Scenario::happy(2))]));
        let runner = jobs::runner(Arc::new(move || settings.clone()), fake.clone());
        let _q = start(store.clone(), fake.clone(), runner, Reporter::detached(), None, cfg());

        wait_for(&store, a, JobStatus::Done);
        let j = store.job(a).unwrap().unwrap();
        assert_eq!((j.attempt, j.auto_retries), (2, 1), "{}", j.log);
        assert_eq!(journal::attempts(&j.log), vec![(1, StartKind::Retry), (2, StartKind::Auto)], "{}", j.log);
        assert!(j.log.contains("авто-повтор: сбой CONSOLE_NOT_OPEN"), "{}", j.log);
        assert_eq!(fake.launches(), 2, "игра запускалась дважды: сбой и повтор");
        assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"new", "ролик склеен");
        assert!(!dir.join("installed.json").exists(), "папка игры убрана после записи");
        let _ = std::fs::remove_dir_all(root);
    }
}
