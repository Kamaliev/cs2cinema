//! Хаб удалённых карточек: шов между потоком раннера и потоком очереди приложения.
//!
//! Поток раннера зовёт `RemoteExecutor::run`: тот создаёт (или усыновляет) карточку задачи сервера в базе, заводит
//! запись в хабе и ждёт итога по каналу, пересылая раннеру этапы, строки журнала и события карточки. Поток очереди
//! находит карточку на очередном такте и исполняет её в `queue_runner` настоящим исполнителем раннера
//! (`PipelineExecutor`: дочерняя предпроверка чужой демки, `run_job` под `catch_unwind`, проверка ролика). Итог
//! уходит раннеру через `SettleHook` после решения `QueuePolicy` и только если это решение не авто-повтор.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    },
    time::{Duration, Instant},
};

use pipeline::{Artifact, Event, Progress, RunError, Stage, job::Outcome};
use runner::{Executor, Task};
use store::{JobKind, JobOutcome, JobSource, JobStatus, NewJob, Store};

use super::{card_label, external_ref};
use crate::{
    jobs::JobReporter,
    queue::{self, Advice, Finished, SettleHook},
};

/// Как часто поток раннера просыпается, пока ждёт итога карточки.
const WAIT_STEP: Duration = Duration::from_millis(200);

/// Как часто поток раннера смотрит в строку карточки: отмена или удаление ждущей карточки автором должны дойти до сервера.
const ROW_STEP: Duration = Duration::from_secs(1);

/// Исполнитель карточки создаётся заново на каждый запуск: приложение читает настройки на каждую задачу.
pub type ExecFactory = Arc<dyn Fn() -> Result<Arc<dyn Executor>, String> + Send + Sync>;

/// Что поток очереди присылает потоку раннера.
pub(super) enum HubMsg {
    Stage(Stage),
    Log(String),
    Event(Event),
    Artifact(Artifact),
    /// Окончательный итог карточки.
    Settled(Result<Outcome, RunError>),
}

/// Запись карточки в хабе: пока она есть, выдача сервера открыта и итог карточки нужен раннеру.
struct Slot {
    card_id: i64,
    task: Task,
    tx: Sender<HubMsg>,
    /// Сервер отменил задачу (мягко / жёстко): карточка видит это через `Progress`.
    server_cancel: AtomicBool,
    server_force: AtomicBool,
    /// Карточку исполняет поток очереди: ставится при взятии (под замком хаба) и снимается, когда попытка кончилась сбоем,
    /// после которого карточка может вернуться в очередь авто-повтором. Пока признак снят, карточка «ждёт», и отмену ждущей
    /// карточки поток раннера закрывает сам, не запуская её.
    started: AtomicBool,
}

impl Slot {
    fn send(&self, m: HubMsg) {
        // закрытый канал значит, что раннер уже не ждёт: молча игнорируем
        let _ = self.tx.send(m);
    }
}

/// Записи карточек удалённых задач по `external_ref`.
pub struct RemoteHub {
    store: Store,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl RemoteHub {
    pub fn new(store: Store) -> Arc<RemoteHub> {
        Arc::new(RemoteHub { store, slots: Mutex::new(HashMap::new()) })
    }

    /// Есть ли открытая запись карточки.
    pub fn has_slots(&self) -> bool {
        self.slots.lock().map_or(false, |s| !s.is_empty())
    }

    /// Номера карточек с открытой записью.
    pub fn slot_cards(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self.slots.lock().map(|s| s.values().map(|slot| slot.card_id).collect()).unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    fn slot(&self, external_ref: &str) -> Option<Arc<Slot>> {
        self.slots.lock().ok()?.get(external_ref).cloned()
    }

    /// Карточку берёт поток очереди: запись находится и помечается взятой под одним замком хаба. Проверка `started` в
    /// `RemoteExecutor` идёт под тем же замком, поэтому «очередь взяла карточку в момент отмены» решается в одну сторону.
    fn take(&self, external_ref: &str) -> Option<Arc<Slot>> {
        let slots = self.slots.lock().ok()?;
        let slot = slots.get(external_ref)?.clone();
        slot.started.store(true, Ordering::SeqCst);
        Some(slot)
    }

    fn remove(&self, external_ref: &str) {
        if let Ok(mut s) = self.slots.lock() {
            s.remove(external_ref);
        }
    }
}

/// Итог карточки раннеру: `RunError` не `Clone`, копия собирается из сообщения и описания сбоя.
fn copy_result(r: &Result<JobOutcome, RunError>) -> Result<Outcome, RunError> {
    match r {
        Ok(o) => Ok(Outcome { out_dir: o.out_dir.clone(), result: o.result.clone(), shots: o.shots, secs: o.secs }),
        Err(e) => Err(RunError { message: e.message.clone(), fail: e.fail.clone() }),
    }
}

impl SettleHook for RemoteHub {
    fn on_settled(&self, job: &store::Job, finished: &Finished, result: &Result<JobOutcome, RunError>) -> Advice {
        let Some(slot) = job.external_ref.as_deref().and_then(|r| self.slot(r)) else {
            return Advice::default();
        };
        slot.send(HubMsg::Settled(copy_result(result)));
        Advice { suppress_pause: slot.server_cancel.load(Ordering::SeqCst) && *finished == Finished::Cancelled }
    }
}

/// Исполнитель раннера, который отдаёт задачу очереди приложения: карточка + ожидание итога.
pub struct RemoteExecutor {
    hub: Arc<RemoteHub>,
}

impl RemoteExecutor {
    pub fn new(hub: Arc<RemoteHub>) -> RemoteExecutor {
        RemoteExecutor { hub }
    }

    /// Карточка и запись хаба под одним замком хаба: поток очереди не может увидеть карточку в `queued` без записи.
    fn open(&self, task: &Task) -> Result<(String, Receiver<HubMsg>), RunError> {
        let reference = external_ref(task.job_id, task.epoch);
        let new_job = NewJob {
            kind: JobKind::Record,
            source: JobSource::File(task.demo.clone()),
            label: card_label(task.job_id),
            // ник пришёл от друга через сервер: управляющие символы и длина
            hero_name: cs2::text::clean_text(&task.hero_name, cs2::text::NAME_MAX),
            hero_steam: task.hero_steam,
            map: String::new(),
        };
        let mut slots = self.hub.slots.lock().map_err(|_| RunError::from("хаб удалённых задач недоступен"))?;
        let card_id = self
            .hub
            .store
            .adopt_remote(&new_job, &reference, task.resume, task.out_dir.as_deref())
            .map_err(|e| RunError::from(format!("карточка задачи не создана: {e}")))?;
        let (tx, rx) = channel();
        let slot = Slot {
            card_id,
            task: task.clone(),
            tx,
            server_cancel: AtomicBool::new(false),
            server_force: AtomicBool::new(false),
            started: AtomicBool::new(false),
        };
        slots.insert(reference.clone(), Arc::new(slot));
        drop(slots);
        let line = if task.resume {
            format!("{}: продолжение после перезапуска по record.json", card_label(task.job_id))
        } else {
            format!("{} (эпоха {}): в очереди приложения", card_label(task.job_id), task.epoch)
        };
        let _ = self.hub.store.append_log(card_id, &line);
        Ok((reference, rx))
    }
}

impl RemoteExecutor {
    /// Отмена, пришедшая в поток раннера (сервер отменил задачу, выдача потеряна или раннер останавливается): очередь приложения
    /// на паузу не встаёт. Ждущая карточка закрывается, не запускаясь (`Some` — итог для раннера); идущая получает отмену через
    /// `Progress` и показывает «Отменяется…».
    fn on_server_cancel(&self, reference: &str, slot: &Slot) -> Option<Result<Outcome, RunError>> {
        slot.server_cancel.store(true, Ordering::SeqCst);
        let _ = self.hub.store.append_log(slot.card_id, "задача с сервера снята: отмена на сервере, потеря выдачи или остановка раннера");
        {
            let mut slots = self.hub.slots.lock().ok()?;
            if !slot.started.load(Ordering::SeqCst) && self.hub.store.cancel_if_queued(slot.card_id).unwrap_or(false) {
                slots.remove(reference);
                return Some(Err(RunError::from(pipeline::CANCELLED)));
            }
        }
        // карточка уже идёт (или очередь как раз её взяла): отмена дойдёт через `CardProgress::cancelled`
        let _ = self.hub.store.set_cancelling(slot.card_id);
        None
    }

    /// Строка карточки: ждущую карточку автор отменил или удалил — запись закрывается без запуска, раннер сообщает серверу отмену.
    fn on_row_closed(&self, reference: &str, slot: &Slot, p: &dyn Progress) -> Option<Result<Outcome, RunError>> {
        let closed = match self.hub.store.job(slot.card_id) {
            Ok(None) => true,
            Ok(Some(job)) => job.status == JobStatus::Cancelled,
            Err(_) => false,
        };
        if !closed {
            return None;
        }
        let mut slots = self.hub.slots.lock().ok()?;
        if slot.started.load(Ordering::SeqCst) {
            return None;
        }
        slots.remove(reference);
        drop(slots);
        p.log("задача отменена в приложении");
        Some(Err(RunError::from(pipeline::CANCELLED)))
    }
}

impl Executor for RemoteExecutor {
    fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        let (reference, rx) = self.open(task)?;
        let Some(slot) = self.hub.slot(&reference) else {
            return Err(RunError::from("запись карточки пропала из хаба"));
        };
        let (mut cancel_seen, mut force_seen) = (false, false);
        let mut row_checked = Instant::now();
        loop {
            match rx.recv_timeout(WAIT_STEP) {
                Ok(HubMsg::Stage(s)) => p.stage(s),
                Ok(HubMsg::Log(l)) => p.log(&l),
                Ok(HubMsg::Event(e)) => p.event(&e),
                // по OutDir `Tee` пишет папку задачи в runner_state.json: без неё после убийства нечего продолжать
                Ok(HubMsg::Artifact(a)) => p.artifact(&a),
                Ok(HubMsg::Settled(r)) => {
                    self.hub.remove(&reference);
                    return r;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.hub.remove(&reference);
                    return Err(RunError::from("очередь приложения не вернула итог карточки"));
                }
            }
            if !cancel_seen && p.cancelled() {
                cancel_seen = true;
                if let Some(done) = self.on_server_cancel(&reference, &slot) {
                    return done;
                }
            }
            if !force_seen && p.force_cancelled() {
                force_seen = true;
                slot.server_force.store(true, Ordering::SeqCst);
            }
            if row_checked.elapsed() >= ROW_STEP {
                row_checked = Instant::now();
                if let Some(done) = self.on_row_closed(&reference, &slot, p) {
                    return done;
                }
            }
        }
    }
}

/// `Progress` потока очереди для удалённой карточки: всё идёт в отчёт карточки (журнал, этап, окно) и копией в канал
/// записи; отмена карточки — это отмена автора или сервера.
struct CardProgress<'a> {
    rep: &'a JobReporter,
    slot: &'a Slot,
}

impl Progress for CardProgress<'_> {
    fn stage(&self, s: Stage) {
        self.rep.stage(s);
        self.slot.send(HubMsg::Stage(s));
    }
    fn log(&self, msg: &str) {
        self.rep.log(msg);
        self.slot.send(HubMsg::Log(msg.to_owned()));
    }
    fn cancelled(&self) -> bool {
        self.rep.cancelled() || self.slot.server_cancel.load(Ordering::SeqCst)
    }
    fn force_cancelled(&self) -> bool {
        self.rep.force_cancelled() || self.slot.server_force.load(Ordering::SeqCst)
    }
    fn event(&self, e: &Event) {
        self.rep.event(e);
        self.slot.send(HubMsg::Event(e.clone()));
    }
    fn artifact(&self, a: &Artifact) {
        self.rep.artifact(a);
        self.slot.send(HubMsg::Artifact(a.clone()));
    }
}

/// Итог общей задачи `pipeline` в строку базы: поля перекладываются как есть.
fn job_outcome(o: Outcome) -> JobOutcome {
    JobOutcome { out_dir: o.out_dir, result: o.result, shots: o.shots, secs: o.secs }
}

/// Ветка удалённой карточки над прежним исполнителем очереди. Строка без `external_ref` идёт в `local` без изменений.
/// Карточка с открытой записью исполняется только `PipelineExecutor`-ом из фабрики (предпроверка чужой демки обязательна).
/// Карточка закрытой выдачи («Повторить» после закрытия на сервере) продолжается локально по `record.json` без отправки.
pub fn queue_runner(local: queue::Runner, hub: Arc<RemoteHub>, factory: ExecFactory) -> queue::Runner {
    Arc::new(move |job: &store::Job, rep: &JobReporter| {
        let Some(reference) = job.external_ref.as_deref() else {
            return local(job, rep);
        };
        let Some(slot) = hub.take(reference) else {
            if job.resume {
                rep.log("результат не отправляется: задача на сервере закрыта");
                return local(job, rep);
            }
            return Err(RunError::from("задача с сервера уже закрыта — начать её заново нельзя; «Повторить» продолжит по record.json"));
        };
        if slot.server_cancel.load(Ordering::SeqCst) {
            return Err(RunError::from(pipeline::CANCELLED));
        }
        let result = factory().map_err(RunError::from).and_then(|exec| {
            let task = Task {
                attempt: rep.attempt,
                resume: slot.task.resume || job.resume,
                out_dir: job.out_dir.clone().or_else(|| slot.task.out_dir.clone()),
                ..slot.task.clone()
            };
            exec.run(&task, &CardProgress { rep, slot: &slot }).map(job_outcome)
        });
        // сбой (не отмена) может кончиться авто-повтором: до него карточка снова ждёт, и отмену автора ловит поток раннера
        if result.as_ref().is_err_and(|e| e.message != pipeline::CANCELLED) {
            slot.started.store(false, Ordering::SeqCst);
        }
        result
    })
}
