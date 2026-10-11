//! Мост между пайплайном и сервером: этап и дубль уходят пульсом, отмена сервера и локальная остановка приходят
//! в `Progress`. Прогресс теряемый по замыслу (следующий пульс заменит пропавший), итоги надёжные и лежат в
//! outbox (`state`).
//!
//! `Shared` — общая доска: поток пульса (`Uplink`) пишет на неё ответы сервера, `Tee` читает их как
//! `cancelled()`/`force_cancelled()`, а пайплайн пишет на неё этап и дубль, которые пульс отправляет серверу.
//! Мягкая отмена сервера, после которой задача не закончилась за `FORCE_AFTER`, становится жёсткой: игра,
//! которая не отвечает на `quit`, закрывается без него.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pipeline::record::Event;
use pipeline::record::log::one_line;
use pipeline::{Artifact, RecordState};
use wire::codes::{JOB_TERMINAL, NOT_FOUND, STALE_EPOCH};
use wire::{CancelMode, Phase, ProgressRequest, Take};

use crate::link::{Link, LinkError};
use crate::transfer::StopWhy;

/// Мягкая отмена сервера, после которой задача не закончилась за это время, становится жёсткой. 10 с больше пути
/// `quit_wait` 5 с + `kill_confirm` 3 с, поэтому исправная мягкая отмена успевает закрыть игру сама.
pub const FORCE_AFTER: Duration = Duration::from_secs(10);

/// Длина текста состояния, уходящего на сервер.
const STATUS_LIMIT: usize = 120;

/// Сообщение об отозванном токене: раннер останавливается, задача остаётся в состоянии.
pub const TOKEN_REJECTED: &str = "токен раннера не принят сервером";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Этап пайплайна в этап протокола; `Done` ничего не меняет.
pub fn phase_of(s: pipeline::Stage) -> Option<Phase> {
    use pipeline::Stage;
    Some(match s {
        Stage::Download => Phase::Download,
        Stage::Parse => Phase::Parse,
        Stage::Positions => Phase::Positions,
        Stage::Geometry => Phase::Geometry,
        Stage::Plan => Phase::Plan,
        Stage::Record => Phase::Record,
        Stage::Assemble => Phase::Assemble,
        Stage::Done => return None,
    })
}

/// Источник `seq` прогресса: `max(предыдущий + 1, unix-мс)`, потолок `i64::MAX` (сервер хранит `seq` в SQLite).
/// Начальное значение — `RunnerState.last_seq`, поэтому после перезапуска в той же эпохе `seq` не откатывается.
pub struct SeqSource(AtomicU64);

impl SeqSource {
    pub fn new(last: u64) -> SeqSource {
        SeqSource(AtomicU64::new(last))
    }

    pub fn next(&self) -> u64 {
        loop {
            let prev = self.0.load(Ordering::SeqCst);
            let next = (prev + 1).max(unix_ms()).min(i64::MAX as u64);
            if self
                .0
                .compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return next;
            }
        }
    }

    /// Последнее выданное значение.
    pub fn last(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct Snapshot {
    phase: Phase,
    take: Option<Take>,
    status: String,
}

struct Board {
    snap: Snapshot,
    /// Этап или дубль изменились после прошлой отправки.
    dirty: bool,
    /// Поток пульса должен выйти.
    stop: bool,
}

const SOFT: u8 = 1;
const FORCE: u8 = 2;

/// Общая доска задачи: этап, дубль, текст состояния, запрошенная отмена, потеря задачи.
pub struct Shared {
    board: Mutex<Board>,
    wake: Condvar,
    /// 0 — нет, 1 — мягкая, 2 — жёсткая.
    cancel: AtomicU8,
    /// Когда сервер впервые запросил отмену.
    soft_since: Mutex<Option<Instant>>,
    /// Строка об эскалации уже записана в журнал.
    escalation_logged: AtomicBool,
    lost: AtomicBool,
    lost_reason: Mutex<String>,
    token_revoked: AtomicBool,
    out_dir: Mutex<Option<PathBuf>>,
}

impl Shared {
    pub fn new(phase: Phase) -> Shared {
        Shared {
            board: Mutex::new(Board {
                snap: Snapshot {
                    phase,
                    take: None,
                    status: String::new(),
                },
                dirty: false,
                stop: false,
            }),
            wake: Condvar::new(),
            cancel: AtomicU8::new(0),
            soft_since: Mutex::new(None),
            escalation_logged: AtomicBool::new(false),
            lost: AtomicBool::new(false),
            lost_reason: Mutex::new(String::new()),
            token_revoked: AtomicBool::new(false),
            out_dir: Mutex::new(None),
        }
    }

    /// Новый этап: дубль сбрасывается, пульс уходит сразу (с задержкой `debounce`).
    pub fn set_phase(&self, phase: Phase) {
        let mut b = lock(&self.board);
        if b.snap.phase != phase {
            b.snap.phase = phase;
            b.snap.take = None;
            b.dirty = true;
            self.wake.notify_all();
        }
    }

    fn set_take(&self, take: Take) {
        let mut b = lock(&self.board);
        if b.snap.take != Some(take) {
            b.snap.take = Some(take);
            b.dirty = true;
            self.wake.notify_all();
        }
    }

    /// Короткий текст состояния для людей: уходит со следующим пульсом.
    pub fn set_status(&self, text: &str) {
        let text: String = one_line(text).chars().take(STATUS_LIMIT).collect();
        lock(&self.board).snap.status = text;
    }

    /// Этап и дубль, которые сейчас на доске.
    pub fn current(&self) -> (Phase, Option<Take>) {
        let b = lock(&self.board);
        (b.snap.phase, b.snap.take)
    }

    /// Ответ сервера о запрошенной отмене; уровень только растёт. Неизвестное значение читается как мягкая отмена.
    pub fn apply_cancel(&self, mode: CancelMode) {
        let level = match mode {
            CancelMode::NotRequested => 0,
            CancelMode::Soft | CancelMode::Unknown => SOFT,
            CancelMode::Force => FORCE,
        };
        if level == 0 {
            return;
        }
        if self.cancel.fetch_max(level, Ordering::SeqCst) == 0 {
            *lock(&self.soft_since) = Some(Instant::now());
        }
    }

    /// Сервер запросил отмену (мягкую или жёсткую).
    pub fn server_cancel(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) >= SOFT
    }

    fn server_force(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) >= FORCE
    }

    fn soft_age(&self) -> Option<Duration> {
        lock(&self.soft_since).map(|t| t.elapsed())
    }

    /// Задача потеряна: сервер отозвал выдачу, закрыл задачу или не принял токен.
    pub fn mark_lost(&self, reason: &str, token_revoked: bool) {
        *lock(&self.lost_reason) = reason.to_owned();
        if token_revoked {
            self.token_revoked.store(true, Ordering::SeqCst);
        }
        self.lost.store(true, Ordering::SeqCst);
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    pub fn lost_reason(&self) -> String {
        lock(&self.lost_reason).clone()
    }

    /// Токен отозван (401): раннер останавливается, задача остаётся в состоянии.
    pub fn token_revoked(&self) -> bool {
        self.token_revoked.load(Ordering::SeqCst)
    }

    /// Причина прервать передачу файла: потеря задачи важнее отмены сервера, отмена — локальной остановки.
    pub fn stop_reason(&self, local: &AtomicBool) -> Option<StopWhy> {
        if self.is_lost() {
            Some(StopWhy::Lost)
        } else if self.server_cancel() {
            Some(StopWhy::Cancelled)
        } else if local.load(Ordering::SeqCst) {
            Some(StopWhy::Local)
        } else {
            None
        }
    }

    fn set_out_dir(&self, p: &Path) {
        *lock(&self.out_dir) = Some(p.to_path_buf());
    }

    /// Папка задачи, которую назвал пайплайн.
    pub fn out_dir(&self) -> Option<PathBuf> {
        lock(&self.out_dir).clone()
    }
}

/// `Progress` для исполнителя: передаёт всё во внутренний прогресс, пишет этап и дубль на доску и сливает отмены
/// (своя, сервера, локальная остановка, потеря задачи) в `cancelled`/`force_cancelled`.
pub struct Tee<'a> {
    pub inner: &'a dyn pipeline::Progress,
    pub shared: Arc<Shared>,
    /// Локальная остановка раннера: это не отмена, задача останется для продолжения.
    pub stop: Arc<AtomicBool>,
    /// Через сколько мягкая отмена сервера становится жёсткой.
    pub force_after: Duration,
    /// Сохраняет папку задачи в outbox сразу, как только пайплайн её назвал (D-01).
    pub persist_out_dir: &'a (dyn Fn(&Path) + Sync),
}

impl Tee<'_> {
    /// Мягкая отмена сервера длится не меньше `force_after`; одна строка в журнал при первом срабатывании.
    fn escalated(&self) -> bool {
        let Some(age) = self.shared.soft_age() else {
            return false;
        };
        if age < self.force_after {
            return false;
        }
        if !self.shared.escalation_logged.swap(true, Ordering::SeqCst) {
            self.inner.log(&format!(
                "мягкая отмена не завершила задачу за {} — закрываю игру принудительно",
                secs_text(self.force_after)
            ));
        }
        true
    }
}

/// «10 с», «0,3 с».
fn secs_text(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{} с", d.as_secs())
    } else {
        format!("{:.1} с", d.as_secs_f32()).replace('.', ",")
    }
}

impl pipeline::Progress for Tee<'_> {
    fn stage(&self, s: pipeline::Stage) {
        self.inner.stage(s);
        if let Some(phase) = phase_of(s) {
            self.shared.set_phase(phase);
            self.shared.set_status(s.title());
        }
    }

    fn log(&self, msg: &str) {
        self.inner.log(msg);
    }

    fn event(&self, e: &Event) {
        self.inner.event(e);
        match e {
            Event::State {
                state: RecordState::Take,
                take: Some((n, m)),
            }
            | Event::TakeDone { n, m } => self.shared.set_take(Take { n: *n, m: *m }),
            _ => {}
        }
        self.shared.set_status(&e.to_string());
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled()
            || self.stop.load(Ordering::SeqCst)
            || self.shared.server_cancel()
            || self.shared.is_lost()
    }

    fn force_cancelled(&self) -> bool {
        self.inner.force_cancelled()
            || self.shared.server_force()
            || self.shared.is_lost()
            || self.escalated()
    }

    fn artifact(&self, a: &Artifact) {
        self.inner.artifact(a);
        if let Artifact::OutDir(p) = a {
            self.shared.set_out_dir(p);
            (self.persist_out_dir)(p);
        }
    }
}

/// Сроки пульса.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Пульс не реже этого.
    pub interval: Duration,
    /// Немедленная отправка при смене этапа или дубля не чаще этого.
    pub debounce: Duration,
}

/// Поток пульса: раз в `interval` и сразу (не чаще раза в `debounce`) при смене этапа или дубля шлёт прогресс.
/// Ответ с отменой попадает на доску, потеря задачи помечается там же.
pub struct Uplink {
    shared: Arc<Shared>,
    seq: Arc<SeqSource>,
    handle: Option<JoinHandle<()>>,
}

impl Uplink {
    pub fn start(
        link: Link,
        job_id: i64,
        epoch: i64,
        shared: Arc<Shared>,
        seq: Arc<SeqSource>,
        timing: Timing,
        log: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Uplink {
        let (sh, sq) = (shared.clone(), seq.clone());
        // Первый пульс несёт этап на момент запуска (не на момент старта потока): сервер увидит `fetch`.
        let first = lock(&shared.board).snap.clone();
        let handle = std::thread::Builder::new()
            .name("cs2c-uplink".to_owned())
            .spawn(move || {
                Pulse {
                    link,
                    job_id,
                    epoch,
                    shared: sh,
                    seq: sq,
                    interval: timing.interval,
                    debounce: timing.debounce,
                    log,
                    first,
                }
                .run()
            })
            .expect("поток пульса должен запуститься");
        Uplink {
            shared,
            seq,
            handle: Some(handle),
        }
    }

    /// Останавливает поток и возвращает последний `seq` для `RunnerState.last_seq`.
    pub fn stop(mut self) -> u64 {
        self.halt();
        self.seq.last()
    }

    fn halt(&mut self) {
        let Some(h) = self.handle.take() else {
            return;
        };
        lock(&self.shared.board).stop = true;
        self.shared.wake.notify_all();
        let _ = h.join();
    }
}

impl Drop for Uplink {
    fn drop(&mut self) {
        self.halt();
    }
}

struct Pulse {
    link: Link,
    job_id: i64,
    epoch: i64,
    shared: Arc<Shared>,
    seq: Arc<SeqSource>,
    interval: Duration,
    debounce: Duration,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    /// Состояние на момент запуска: уходит первым пульсом.
    first: Snapshot,
}

impl Pulse {
    /// Ждёт срока очередной отправки; `None` — поток должен выйти.
    fn wait_due(&self, last: Option<Instant>) -> Option<Snapshot> {
        let mut b = lock(&self.shared.board);
        if last.is_none() {
            if b.stop {
                return None;
            }
            // Этап изменился, пока поток стартовал: следующий пульс уйдёт через `debounce`.
            b.dirty = b.snap.phase != self.first.phase || b.snap.take != self.first.take;
            return Some(self.first.clone());
        }
        loop {
            if b.stop {
                return None;
            }
            let wait = match last.map(|t| t.elapsed()) {
                None => break,
                Some(since) if since >= self.interval => break,
                Some(since) if b.dirty && since >= self.debounce => break,
                Some(since) => {
                    let mut wait = self.interval - since;
                    if b.dirty {
                        wait = wait.min(self.debounce - since);
                    }
                    wait
                }
            };
            b = self
                .shared
                .wake
                .wait_timeout(b, wait.max(Duration::from_millis(1)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        b.dirty = false;
        Some(b.snap.clone())
    }

    fn run(self) {
        let mut last: Option<Instant> = None;
        let mut failing = false;
        while let Some(snap) = self.wait_due(last) {
            last = Some(Instant::now());
            let req = ProgressRequest {
                epoch: self.epoch,
                seq: self.seq.next(),
                phase: snap.phase,
                take: snap.take,
                status: snap.status,
            };
            match self.link.progress(self.job_id, &req) {
                Ok(reply) => {
                    if failing {
                        failing = false;
                        (self.log)("связь с сервером восстановилась, прогресс уходит");
                    }
                    self.shared.apply_cancel(reply.cancel);
                }
                Err(LinkError::Status { status: 401, .. }) => {
                    self.lose(TOKEN_REJECTED, true);
                    return;
                }
                Err(LinkError::Status { error, status, .. })
                    if error == STALE_EPOCH || (status == 409 && error.is_empty()) =>
                {
                    self.lose(
                        &format!(
                            "сервер отозвал выдачу задачи или выдал её заново ({STALE_EPOCH})"
                        ),
                        false,
                    );
                    return;
                }
                Err(LinkError::Status { error, .. }) if error == JOB_TERMINAL => {
                    self.lose(
                        &format!("задача уже закрыта на сервере ({JOB_TERMINAL})"),
                        false,
                    );
                    return;
                }
                Err(LinkError::Status { error, status, .. })
                    if error == NOT_FOUND || status == 404 =>
                {
                    self.lose(
                        &format!("сервер не знает такой задачи ({NOT_FOUND})"),
                        false,
                    );
                    return;
                }
                Err(e) => {
                    if !failing {
                        failing = true;
                        (self.log)(&format!(
                            "задача #{}: прогресс не дошёл до сервера: {e}; повтор со следующим пульсом",
                            self.job_id
                        ));
                    }
                }
            }
        }
    }

    fn lose(&self, reason: &str, token_revoked: bool) {
        (self.log)(&format!("задача #{} потеряна: {reason}", self.job_id));
        self.shared.mark_lost(reason, token_revoked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::{Progress, Quiet};

    fn tee<'a>(
        shared: &Arc<Shared>,
        stop: &Arc<AtomicBool>,
        force_after: Duration,
        persist: &'a (dyn Fn(&Path) + Sync),
    ) -> Tee<'a> {
        Tee {
            inner: &Quiet,
            shared: shared.clone(),
            stop: stop.clone(),
            force_after,
            persist_out_dir: persist,
        }
    }

    #[test]
    fn escalation_needs_a_soft_cancel_and_the_bound() {
        assert_eq!(FORCE_AFTER, Duration::from_secs(10));
        let nothing = |_: &Path| {};
        let stop = Arc::new(AtomicBool::new(false));

        // Без отмены сервера и при одной локальной остановке жёсткой отмены нет сколько ни жди.
        let shared = Arc::new(Shared::new(Phase::Record));
        let t = tee(&shared, &stop, Duration::from_millis(60), &nothing);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!t.force_cancelled());
        stop.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.cancelled(), "локальная остановка видна как cancelled");
        assert!(!t.force_cancelled(), "локальная остановка не эскалирует");
        stop.store(false, Ordering::SeqCst);

        // Мягкая отмена: ложно до границы, истинно после.
        shared.apply_cancel(CancelMode::Soft);
        assert!(t.cancelled());
        assert!(!t.force_cancelled(), "граница ещё не прошла");
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.force_cancelled(), "граница прошла");
        assert!(t.force_cancelled(), "и остаётся истинной");

        // Жёсткая отмена сервера действует сразу.
        let shared = Arc::new(Shared::new(Phase::Record));
        let t = tee(&shared, &stop, FORCE_AFTER, &nothing);
        shared.apply_cancel(CancelMode::Force);
        assert!(t.force_cancelled());

        // Потеря задачи останавливает исполнителя жёстко.
        let shared = Arc::new(Shared::new(Phase::Record));
        let t = tee(&shared, &stop, FORCE_AFTER, &nothing);
        assert!(!t.cancelled());
        shared.mark_lost("тест", false);
        assert!(t.cancelled() && t.force_cancelled());
    }

    #[test]
    fn tee_puts_stage_and_take_on_the_board_and_remembers_the_out_dir() {
        use std::sync::Mutex;
        let saved: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        let persist = |p: &Path| saved.lock().unwrap().push(p.to_path_buf());
        let shared = Arc::new(Shared::new(Phase::Precheck));
        let stop = Arc::new(AtomicBool::new(false));
        let t = tee(&shared, &stop, FORCE_AFTER, &persist);

        t.stage(pipeline::Stage::Record);
        t.event(&Event::State {
            state: RecordState::Take,
            take: Some((2, 3)),
        });
        assert_eq!(shared.current(), (Phase::Record, Some(Take { n: 2, m: 3 })));
        t.stage(pipeline::Stage::Done);
        assert_eq!(shared.current().0, Phase::Record, "Done этап не меняет");
        t.stage(pipeline::Stage::Assemble);
        assert_eq!(shared.current(), (Phase::Assemble, None), "дубль сброшен");

        t.artifact(&Artifact::OutDir(PathBuf::from("out/x")));
        assert_eq!(shared.out_dir(), Some(PathBuf::from("out/x")));
        assert_eq!(*saved.lock().unwrap(), vec![PathBuf::from("out/x")]);
    }

    #[test]
    fn phases_map_one_to_one_and_seq_only_grows() {
        use pipeline::Stage;
        assert_eq!(phase_of(Stage::Record), Some(Phase::Record));
        assert_eq!(phase_of(Stage::Download), Some(Phase::Download));
        assert_eq!(phase_of(Stage::Done), None);

        let s = SeqSource::new(unix_ms() + 1_000_000);
        let first = s.last();
        let (a, b) = (s.next(), s.next());
        assert_eq!(a, first + 1);
        assert!(b > a);
        assert_eq!(SeqSource::new(i64::MAX as u64).next(), i64::MAX as u64);
    }
}
