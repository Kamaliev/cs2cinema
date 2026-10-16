//! Переходы и аренды ядра на виртуальных часах без HTTP (D-05, SRV-03, SRV-04, SRV-07).
//!
//! Время двигает `ManualClock`, раннеры — строки в базе, а `Core` вызывается напрямую. Свой маленький `TestDir`
//! вместо `tests/common`: этот тестовый бинарник не тянет HTTP-стенд.

use server::auth;
use server::clock::ManualClock;
use server::core::{Core, CoreOptions, JobRow, Limits, Reject, Submit};
use server::rules::{self, Event, FinishKind, RECEIVING_STALE_MS};
use server::secret::Redactor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wire::{
    CancelMode, Failure, FinishRequest, HelloRequest, Holding, JobState, Outcome, Phase,
    ProgressReply, ProgressRequest, ReleaseRequest, codes,
};

const T0: i64 = 1_800_000_000_000;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("cs2srv-states-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core_with_clock(limits: Limits) -> (Core, ManualClock, TestDir) {
    let dir = TestDir::new();
    let clock = ManualClock::new(T0);
    let core = Core::open(CoreOptions {
        data_dir: dir.0.join("data"),
        limits,
        clock: Arc::new(clock.clone()),
        redactor: Redactor::default(),
    })
    .unwrap();
    (core, clock, dir)
}

fn submit(core: &Core, dir: &TestDir, player: &str) -> i64 {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let src = dir.0.join(format!("in-{n}.dem"));
    let mut bytes = b"PBDEMS2\0".to_vec();
    bytes.resize(2048, 9);
    std::fs::write(&src, bytes).unwrap();
    let s = Submit {
        player: player.to_owned(),
        requested_by: "оператор".to_owned(),
        orig_name: "m.dem".to_owned(),
        ..Default::default()
    };
    core.submit(&src, &s).unwrap().0
}

fn runner(core: &Core, name: &str) -> i64 {
    auth::add_runner(core.db(), name, T0).unwrap().0.id
}

fn progress(
    core: &Core,
    runner: i64,
    job: i64,
    epoch: i64,
    seq: u64,
    phase: Phase,
) -> Result<(ProgressReply, bool), Reject> {
    core.progress(
        runner,
        job,
        &ProgressRequest {
            epoch,
            seq,
            phase,
            take: None,
            status: String::new(),
        },
    )
}

fn state(core: &Core, id: i64) -> JobState {
    core.job(id).unwrap().unwrap().state
}

fn secs(clock: &ManualClock, n: u64) {
    clock.advance(Duration::from_secs(n));
}

#[test]
fn claim_hands_out_the_oldest_job_with_a_new_epoch_and_grant() {
    let (core, _clock, dir) = core_with_clock(Limits::default());
    let first = submit(&core, &dir, "one");
    let second = submit(&core, &dir, "two");
    let a = runner(&core, "a");

    let got = core.claim(a).unwrap().expect("задача выдана");
    assert_eq!((got.job_id, got.epoch), (first, 1));
    assert_eq!(got.lease_until_ms, T0 + 45_000);
    let job = core.job(first).unwrap().unwrap();
    assert_eq!(job.state, JobState::Assigned);
    assert_eq!((job.epoch, job.grants, job.runner_id), (1, 1, Some(a)));
    assert_eq!(state(&core, second), JobState::Queued, "вторая ждёт");
}

#[test]
fn expired_lease_requeues_without_a_new_epoch_or_grant() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "a");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();

    secs(&clock, 44);
    assert!(
        core.sweep().unwrap().requeued.is_empty(),
        "срок ещё не вышел"
    );
    assert_eq!(state(&core, id), JobState::Running);

    secs(&clock, 2);
    let out = core.sweep().unwrap();
    assert_eq!(out.requeued, vec![id]);
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Queued);
    assert!(job.lease_lost && job.lease_until.is_none());
    assert_eq!((job.epoch, job.grants, job.runner_id), (1, 1, Some(a)));
    assert_eq!(job.fail_code, None, "потеря аренды — не сбой");
}

#[test]
fn same_runner_reattaches_a_lost_job_by_its_epoch() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "a");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();
    secs(&clock, 60);
    core.sweep().unwrap();

    let (reply, reattached) = progress(&core, a, id, 1, 2, Phase::Record).unwrap();
    assert!(reattached);
    assert_eq!(
        reply.cancel,
        CancelMode::NotRequested,
        "запись не останавливают"
    );
    assert_eq!(reply.lease_until_ms, T0 + 60_000 + 45_000);
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Running);
    assert!(!job.lease_lost);
    assert_eq!((job.epoch, job.grants), (1, 1));
}

#[test]
fn claim_after_loss_bumps_the_epoch_and_fences_the_old_runner() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "a");
    let b = runner(&core, "b");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();
    secs(&clock, 46);

    // Ленивое истечение внутри `claim`: потока `sweep` в тесте нет.
    let got = core.claim(b).unwrap().expect("задачу забрал второй раннер");
    assert_eq!((got.job_id, got.epoch), (id, 2));
    let job = core.job(id).unwrap().unwrap();
    assert_eq!((job.grants, job.runner_id), (2, Some(b)));
    assert_eq!(
        (job.last_seq, job.phase.as_str()),
        (0, ""),
        "счёт сообщений начат заново"
    );

    let e = progress(&core, a, id, 1, 2, Phase::Record).unwrap_err();
    assert_eq!(e.code, codes::STALE_EPOCH);
    let e = progress(&core, b, id, 1, 1, Phase::Record).unwrap_err();
    assert_eq!(
        e.code,
        codes::STALE_EPOCH,
        "эпоха старая и у нового раннера"
    );
    progress(&core, b, id, 2, 1, Phase::Fetch).unwrap();
}

#[test]
fn assigned_job_is_redelivered_to_the_same_runner_without_a_new_grant() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let first = submit(&core, &dir, "one");
    let _second = submit(&core, &dir, "two");
    let a = runner(&core, "a");
    let one = core.claim(a).unwrap().unwrap();
    secs(&clock, 10);
    let again = core.claim(a).unwrap().unwrap();
    assert_eq!((again.job_id, again.epoch), (first, 1));
    assert_eq!(again.job_id, one.job_id);
    assert_eq!(again.lease_until_ms, T0 + 10_000 + 45_000, "срок обновлён");
    let job = core.job(first).unwrap().unwrap();
    assert_eq!((job.grants, job.epoch), (1, 1));
}

#[test]
fn runner_with_a_running_job_cannot_claim_another() {
    let (core, _clock, dir) = core_with_clock(Limits::default());
    let first = submit(&core, &dir, "one");
    let second = submit(&core, &dir, "two");
    let a = runner(&core, "a");
    core.claim(a).unwrap();
    progress(&core, a, first, 1, 1, Phase::Record).unwrap();

    let e = core.claim(a).unwrap_err();
    assert_eq!(e.code, codes::ALREADY_HOLDING);
    assert!(e.message.contains(&format!("#{first}")), "{}", e.message);
    assert_eq!(state(&core, second), JobState::Queued);
    let b = runner(&core, "b");
    assert_eq!(core.claim(b).unwrap().unwrap().job_id, second);
}

#[test]
fn job_granted_max_times_fails_with_too_many_grants() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let poison = submit(&core, &dir, "poison");
    let a = runner(&core, "a");
    for round in 1..=3 {
        let got = core.claim(a).unwrap().expect("выдача");
        assert_eq!((got.job_id, got.epoch), (poison, round));
        secs(&clock, 46);
        core.sweep().unwrap();
        assert_eq!(state(&core, poison), JobState::Queued);
    }
    let next = submit(&core, &dir, "next");

    // Четвёртая выдача: «ядовитая» задача падает, очередь идёт дальше.
    let report = core.claim_report(a).unwrap();
    assert_eq!(report.failed, vec![poison]);
    assert_eq!(report.assignment.expect("следующая задача").job_id, next);
    let job = core.job(poison).unwrap().unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("TOO_MANY_GRANTS"));
    assert!(job.fail_message.unwrap().contains("3 раз"));
    assert!(job.finished_at.is_some());
    assert_eq!(job.grants, 3);
}

/// Задача, трижды выданная раннеру `a`; после третьей выдачи аренда потеряна (`lease_lost`), задача в очереди.
fn job_lost_on_the_last_grant(core: &Core, clock: &ManualClock, dir: &TestDir, a: i64) -> i64 {
    let id = submit(core, dir, "poison");
    for round in 1..=3 {
        let got = core.claim(a).unwrap().expect("выдача");
        assert_eq!((got.job_id, got.epoch), (id, round));
        secs(clock, 46);
        core.sweep().unwrap();
    }
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(
        job.state,
        JobState::Queued,
        "после третьей потери задача в очереди"
    );
    assert_eq!(job.grants, 3);
    assert!(job.lease_lost, "аренда потеряна, а не снята");
    assert_eq!(job.runner_id, Some(a), "раннер-владелец помнится");
    id
}

#[test]
fn lost_lease_on_the_last_grant_waits_for_its_runner() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let b = runner(&core, "b");
    let id = job_lost_on_the_last_grant(&core, &clock, &dir, a);

    // Чужой раннер просит задачу: задача ждёт своего раннера, а не падает.
    let report = core.claim_report(b).unwrap();
    assert!(
        report.failed.is_empty(),
        "чужой claim не роняет задачу: {:?}",
        report.failed
    );
    assert!(report.assignment.is_none(), "выдавать больше нечего");
    assert_eq!(state(&core, id), JobState::Queued);

    // Владелец вернулся через hello и получил задачу обратно.
    let hello = core
        .hello(
            a,
            &HelloRequest {
                protocol: wire::PROTOCOL_VERSION,
                name: "a".to_owned(),
                app_version: String::new(),
                status: String::new(),
                holding: vec![Holding {
                    job_id: id,
                    epoch: 3,
                }],
            },
        )
        .unwrap();
    assert_eq!(hello.directives[0].job_id, id);
    assert_eq!(hello.directives[0].action, wire::DirectiveAction::Keep);
    let job = core.job(id).unwrap().unwrap();
    assert!(!job.lease_lost, "аренда возвращена");
    assert_eq!((job.epoch, job.grants), (3, 3));
    assert_eq!(job.runner_id, Some(a));
    assert!(
        job.state.is_leased(),
        "задача снова за раннером: {:?}",
        job.state
    );
}

#[test]
fn lost_lease_on_the_last_grant_fails_after_the_return_window() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let b = runner(&core, "b");
    let id = job_lost_on_the_last_grant(&core, &clock, &dir, a);

    // Окно возврата — 45 + 90 с; через 136 с раннер уже не вернётся.
    secs(&clock, 136);
    let report = core.claim_report(b).unwrap();
    assert_eq!(report.failed, vec![id]);
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("TOO_MANY_GRANTS"));
    assert!(job.fail_message.unwrap().contains("3 раз"));
}

#[test]
fn release_returns_the_job_and_can_refund_the_grant() {
    let (core, _clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "a");
    let b = runner(&core, "b");
    let release = |runner, epoch, counts| {
        core.release(
            runner,
            id,
            &ReleaseRequest {
                epoch,
                reason: "окружение".to_owned(),
                counts,
            },
        )
    };

    core.claim(a).unwrap();
    assert_eq!(release(b, 1, true).unwrap_err().code, codes::STALE_EPOCH);
    assert_eq!(release(a, 7, true).unwrap_err().code, codes::STALE_EPOCH);
    release(a, 1, true).unwrap();
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(
        (job.runner_id, job.lease_lost, job.grants),
        (None, false, 1)
    );
    assert_eq!(
        release(a, 1, true).unwrap_err().code,
        codes::STALE_EPOCH,
        "повторный возврат — уже не её задача"
    );

    let got = core.claim(b).unwrap().unwrap();
    assert_eq!((got.epoch, core.job(id).unwrap().unwrap().grants), (2, 2));
    release(b, 2, false).unwrap();
    assert_eq!(
        core.job(id).unwrap().unwrap().grants,
        1,
        "выдача возвращена"
    );
    assert_eq!(core.job(id).unwrap().unwrap().epoch, 2, "эпоха не меняется");
}

fn open_on(dir: &TestDir, clock: &ManualClock) -> Core {
    Core::open(CoreOptions {
        data_dir: dir.0.join("data"),
        limits: Limits::default(),
        clock: Arc::new(clock.clone()),
        redactor: Redactor::default(),
    })
    .unwrap()
}

fn clock_now(clock: &ManualClock) -> i64 {
    use server::clock::Clock;
    clock.now_ms()
}

#[test]
fn startup_grace_keeps_leases_after_a_server_restart() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "a");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();
    secs(&clock, 40);
    drop(core);

    // Новый процесс на той же папке данных: аренда живого раннера продлевается на льготный срок.
    let core = open_on(&dir, &clock);
    let rec = core.recover().unwrap();
    assert_eq!(rec.grace_extended, 1);
    assert!(rec.receive_failed.is_empty());
    let job = core.job(id).unwrap().unwrap();
    assert!(job.lease_until.unwrap() >= T0 + 40_000 + 90_000);

    secs(&clock, 60);
    assert!(core.sweep().unwrap().requeued.is_empty());
    assert_eq!(
        state(&core, id),
        JobState::Running,
        "раннер успел вернуться"
    );
    progress(&core, a, id, 1, 2, Phase::Record).unwrap();

    // Если раннер так и не вернулся, по окончании срока аренда истекает как обычно.
    secs(&clock, 46);
    assert_eq!(core.sweep().unwrap().requeued, vec![id]);
}

#[test]
fn stale_receiving_job_fails_on_recover_and_on_sweep() {
    let (core, clock, _dir) = core_with_clock(Limits::default());
    let begin = |player: &str| {
        core.begin_receive(&Submit {
            player: player.to_owned(),
            ..Default::default()
        })
        .unwrap()
    };
    let hung = begin("one");
    secs(&clock, 31 * 60);
    let fresh = begin("two");
    secs(&clock, 60);

    let rec = core.recover().unwrap();
    assert_eq!(rec.receive_failed, vec![hung]);
    let job = core.job(hung).unwrap().unwrap();
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("RECEIVE_FAILED"));
    assert!(job.finished_at.is_some());
    assert_eq!(state(&core, fresh), JobState::Receiving, "свежий приём цел");

    // Тот же приём под `sweep`: зависшим он становится, когда прошло больше 30 минут.
    secs(&clock, 31 * 60);
    let out = core.sweep().unwrap();
    assert_eq!(out.receive_failed, vec![fresh]);
    assert_eq!(state(&core, fresh), JobState::Failed);
}

#[test]
fn revoked_runner_loses_its_leases() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let id = submit(&core, &dir, "one");
    let a = runner(&core, "main");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();
    secs(&clock, 5);

    auth::revoke_runner(core.db(), "main", clock_now(&clock)).unwrap();
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(
        job.lease_until,
        Some(T0 + 5_000),
        "аренда заканчивается сейчас"
    );
    assert_eq!(job.state, JobState::Running);

    assert_eq!(core.sweep().unwrap().requeued, vec![id]);
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(
        (job.epoch, job.grants),
        (1, 1),
        "возврат — не сбой и не новая выдача"
    );
}

// ---------------------------------------------------------------------------------------------------------
// Итог задачи, отмена и вся таблица переходов (05-04, SRV-03)
// ---------------------------------------------------------------------------------------------------------

fn failed_request(epoch: i64) -> FinishRequest {
    FinishRequest {
        epoch,
        outcome: Outcome::Failed(Failure {
            code: "HLAE_ERROR".to_owned(),
            state: "take".to_owned(),
            message: "boom".to_owned(),
            elapsed_ms: 10,
            ..Default::default()
        }),
    }
}

fn cancelled_request(epoch: i64) -> FinishRequest {
    FinishRequest {
        epoch,
        outcome: Outcome::Cancelled,
    }
}

fn demo_source(dir: &TestDir, name: &str) -> PathBuf {
    let src = dir.0.join(name);
    let mut bytes = b"PBDEMS2\0".to_vec();
    bytes.resize(1024, 5);
    std::fs::write(&src, bytes).unwrap();
    src
}

/// Кладёт куски результата в папку задачи: `*.part` и готовый файл.
fn put_parts(core: &Core, id: i64) -> PathBuf {
    let dir = core.blobs().results_dir(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("highlights.mp4.part"), b"part").unwrap();
    dir
}

#[test]
fn the_happy_path_walks_receiving_to_done_on_virtual_time() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let mut seen: Vec<JobState> = Vec::new();
    let mut note = |s: JobState| {
        if !seen.contains(&s) {
            seen.push(s);
        }
    };

    // receiving -> queued
    let id = core
        .begin_receive(&Submit {
            player: "s1mple".to_owned(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(state(&core, id), JobState::Receiving);
    note(state(&core, id));
    secs(&clock, 1);
    core.finish_receive(id, &demo_source(&dir, "happy.dem"))
        .unwrap();
    assert_eq!(state(&core, id), JobState::Queued);
    note(state(&core, id));

    // queued -> assigned -> running -> uploading
    let a = runner(&core, "a");
    secs(&clock, 1);
    let got = core.claim(a).unwrap().expect("выдача");
    assert_eq!((got.job_id, got.epoch), (id, 1));
    note(state(&core, id));
    progress(&core, a, id, 1, 1, Phase::Fetch).unwrap();
    assert_eq!(state(&core, id), JobState::Assigned, "fetch ещё не работа");
    secs(&clock, 2);
    progress(&core, a, id, 1, 2, Phase::Record).unwrap();
    assert_eq!(state(&core, id), JobState::Running);
    note(state(&core, id));
    secs(&clock, 2);
    progress(&core, a, id, 1, 3, Phase::Upload).unwrap();
    assert_eq!(state(&core, id), JobState::Uploading);
    note(state(&core, id));

    // uploading -> done: загрузку результата по HTTP делает 05-05, поэтому итог `done` проверяется автоматом.
    let before = core.job(id).unwrap().unwrap();
    secs(&clock, 3);
    let now = T0 + 9_000;
    let ev = Event::Finish {
        runner_id: a,
        epoch: 1,
        kind: FinishKind::Done,
    };
    let done = rules::decide(&before, &ev, now, &Limits::default()).unwrap();
    assert_eq!(done.job.state, JobState::Done);
    assert_eq!(done.job.finished_at, Some(now));
    assert!(done.job.lease_until.is_none() && done.discard_parts);
    note(done.job.state);

    // failed и cancelled у других задач.
    // Настоящий итог той же задачи — сбой раннера; третья задача отменяется оператором.
    core.finish(a, id, &failed_request(1)).unwrap();
    note(state(&core, id));
    let third = submit(&core, &dir, "three");
    core.cancel(third, false).unwrap();
    note(state(&core, third));

    for s in JobState::ALL {
        assert!(seen.contains(&s), "состояние {s:?} не встретилось");
    }
}

#[derive(Clone, Copy)]
struct Who {
    runner: i64,
    epoch: i64,
}

const OWN: Who = Who {
    runner: 1,
    epoch: 3,
};
const FOREIGN: Who = Who {
    runner: 2,
    epoch: 3,
};

/// Порядок состояний в строках таблицы.
const STATES: [&str; 9] = [
    "receiving",
    "queued",
    "queued+lost",
    "assigned",
    "running",
    "uploading",
    "done",
    "failed",
    "cancelled",
];

/// Строка задачи в нужном состоянии: у выданной, потерявшей аренду и окончательной задачи раннер 1 и эпоха 3.
fn row(kind: &str) -> JobRow {
    let (state, lost) = match kind {
        "receiving" => (JobState::Receiving, false),
        "queued" => (JobState::Queued, false),
        "queued+lost" => (JobState::Queued, true),
        "assigned" => (JobState::Assigned, false),
        "running" => (JobState::Running, false),
        "uploading" => (JobState::Uploading, false),
        "done" => (JobState::Done, false),
        "failed" => (JobState::Failed, false),
        "cancelled" => (JobState::Cancelled, false),
        other => panic!("неизвестное состояние {other}"),
    };
    let has_runner = state.is_leased() || lost || state.is_terminal();
    JobRow {
        id: 1,
        user_id: None,
        requested_by: String::new(),
        state,
        cancel_requested: CancelMode::NotRequested,
        epoch: if has_runner { 3 } else { 0 },
        grants: i64::from(has_runner),
        runner_id: has_runner.then_some(1),
        lease_until: state.is_leased().then_some(T0 + 1_000),
        lease_lost: lost,
        last_seq: 0,
        source_kind: "file".to_owned(),
        orig_name: String::new(),
        faceit_link: None,
        player_steam: None,
        player_name: "s1mple".to_owned(),
        phase: String::new(),
        take_n: None,
        take_m: None,
        fail_code: None,
        fail_class: None,
        fail_state: None,
        fail_hint: None,
        fail_message: None,
        fail_tail: None,
        fail_elapsed_ms: None,
        created_at: T0 - 10,
        updated_at: T0 - 10,
        finished_at: state.is_terminal().then_some(T0 - 5),
    }
}

fn plain(_: &mut JobRow) {}

fn overdue(j: &mut JobRow) {
    if j.lease_until.is_some() {
        j.lease_until = Some(T0 - 1);
    }
}

fn hung_receiving(j: &mut JobRow) {
    j.updated_at = T0 - RECEIVING_STALE_MS - 1;
}

fn progress_event(who: Who) -> Event {
    Event::Progress {
        runner_id: who.runner,
        epoch: who.epoch,
        seq: 1,
        phase: Phase::Record,
        take: None,
    }
}

fn release_event(who: Who) -> Event {
    Event::Release {
        runner_id: who.runner,
        epoch: who.epoch,
        counts: true,
    }
}

fn reattach_event(who: Who) -> Event {
    Event::Reattach {
        runner_id: who.runner,
        epoch: who.epoch,
    }
}

fn finish_event(who: Who, kind: FinishKind) -> Event {
    Event::Finish {
        runner_id: who.runner,
        epoch: who.epoch,
        kind,
    }
}

fn failed_kind() -> FinishKind {
    FinishKind::Failed(Failure {
        code: "E".to_owned(),
        ..Default::default()
    })
}

/// Ожидание: `=` без изменений; `S:{состояние}` — Ok и новое состояние; `C:{флаг}` — Ok, состояние то же, флаг отмены;
/// `!{код}` — отказ с этим кодом.
fn expect_pair(label: &str, kind: &str, row_in: &JobRow, ev: &Event, want: &str) {
    let got = rules::decide(row_in, ev, T0, &Limits::default());
    let ctx = format!("{kind} × {label}");
    match (want, got) {
        ("=", Ok(d)) => assert_eq!(&d.job, row_in, "{ctx}: ждали «без изменений»"),
        (w, Ok(d)) if w.starts_with("S:") => {
            assert_eq!(d.job.state.as_str(), &w[2..], "{ctx}: новое состояние")
        }
        (w, Ok(d)) if w.starts_with("C:") => {
            assert_eq!(d.job.state, row_in.state, "{ctx}: состояние то же");
            assert_eq!(d.job.cancel_requested.as_str(), &w[2..], "{ctx}: флаг");
        }
        (w, Err(r)) if w.starts_with('!') => assert_eq!(r.code, &w[1..], "{ctx}: код отказа"),
        (w, other) => panic!("{ctx}: ждали «{w}», получили {other:?}"),
    }
    if row_in.state.is_terminal()
        && let Ok(d) = rules::decide(row_in, ev, T0, &Limits::default())
    {
        assert_eq!(
            d.job.state, row_in.state,
            "{ctx}: окончательное не меняется"
        );
    }
}

#[test]
fn every_state_and_event_pair_matches_the_transition_table() {
    // Порядок столбцов — STATES: receiving, queued, queued+lost, assigned, running, uploading, done, failed, cancelled.
    // Короткие коды отказов: !stale_epoch и !job_terminal; `!internal` — событие вызвано не на своём состоянии.
    let cells = |s: &str| -> [String; 9] {
        let v: Vec<String> = s.split_whitespace().map(str::to_owned).collect();
        v.try_into().expect("в строке таблицы ровно девять ячеек")
    };
    #[allow(clippy::type_complexity)]
    let table: Vec<(&str, Event, fn(&mut JobRow), [String; 9])> = vec![
        (
            "progress своего",
            progress_event(OWN),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:running S:running S:running S:uploading !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "progress чужого",
            progress_event(FOREIGN),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "expire со сроком в будущем",
            Event::Expire,
            plain,
            cells("= = = = = = = = ="),
        ),
        (
            "expire просроченной аренды",
            Event::Expire,
            overdue,
            cells("= = = S:queued S:queued S:queued = = ="),
        ),
        (
            "release своего",
            release_event(OWN),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:queued S:queued S:queued S:queued !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "release чужого",
            release_event(FOREIGN),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "reattach своего",
            reattach_event(OWN),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:assigned S:assigned S:running S:uploading !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "reattach чужого",
            reattach_event(FOREIGN),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "chunk своего",
            Event::Chunk {
                runner_id: OWN.runner,
                epoch: OWN.epoch,
            },
            plain,
            cells(
                "!stale_epoch !stale_epoch S:uploading S:uploading S:uploading S:uploading !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "chunk чужого",
            Event::Chunk {
                runner_id: FOREIGN.runner,
                epoch: FOREIGN.epoch,
            },
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "touch своего",
            Event::Touch {
                runner_id: OWN.runner,
                epoch: OWN.epoch,
            },
            plain,
            cells(
                "!stale_epoch !stale_epoch S:assigned S:assigned S:running S:uploading !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "touch чужого",
            Event::Touch {
                runner_id: FOREIGN.runner,
                epoch: FOREIGN.epoch,
            },
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "recover",
            Event::Recover {
                grace_until: T0 + 90_000,
            },
            plain,
            cells("= = = S:assigned S:running S:uploading = = ="),
        ),
        (
            "receive_stale зависшего приёма",
            Event::ReceiveStale,
            hung_receiving,
            cells("S:failed = = = = = = = ="),
        ),
        (
            "receive_stale свежего приёма",
            Event::ReceiveStale,
            plain,
            cells("= = = = = = = = ="),
        ),
        (
            "finish failed своего",
            finish_event(OWN, failed_kind()),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:failed S:failed S:failed S:failed !job_terminal = !job_terminal",
            ),
        ),
        (
            "finish failed чужого",
            finish_event(FOREIGN, failed_kind()),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "finish cancelled своего",
            finish_event(OWN, FinishKind::Cancelled),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:cancelled S:cancelled S:cancelled S:cancelled !job_terminal !job_terminal =",
            ),
        ),
        (
            "finish cancelled чужого",
            finish_event(FOREIGN, FinishKind::Cancelled),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "finish done своего",
            finish_event(OWN, FinishKind::Done),
            plain,
            cells(
                "!stale_epoch !stale_epoch S:done !offset_mismatch !offset_mismatch S:done = !job_terminal !job_terminal",
            ),
        ),
        (
            "finish done чужого",
            finish_event(FOREIGN, FinishKind::Done),
            plain,
            cells(
                "!stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !stale_epoch !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "cancel мягкая",
            Event::Cancel { force: false },
            plain,
            cells(
                "S:cancelled S:cancelled S:cancelled C:soft C:soft C:soft !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "cancel жёсткая",
            Event::Cancel { force: true },
            plain,
            cells(
                "S:cancelled S:cancelled S:cancelled C:force C:force C:force !job_terminal !job_terminal !job_terminal",
            ),
        ),
        (
            "received",
            Event::Received,
            plain,
            cells(
                "S:queued !internal !internal !internal !internal !internal !internal !internal !internal",
            ),
        ),
        (
            "receive_failed",
            Event::ReceiveFailed {
                message: "диск полон".to_owned(),
            },
            plain,
            cells(
                "S:failed !internal !internal !internal !internal !internal !internal !internal !internal",
            ),
        ),
    ];

    let mut pairs = 0;
    for (label, ev, tweak, want) in &table {
        for (kind, want) in STATES.iter().zip(want) {
            let mut r = row(kind);
            tweak(&mut r);
            expect_pair(label, kind, &r, ev, want);
            pairs += 1;
        }
    }
    assert_eq!(pairs, 25 * 9, "таблица охватывает все пары");
}

#[test]
fn failed_and_cancelled_outcomes_are_terminal_and_sticky() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let b = runner(&core, "b");

    // Сбой раннера.
    let failed = submit(&core, &dir, "one");
    core.claim(a).unwrap();
    progress(&core, a, failed, 1, 1, Phase::Record).unwrap();
    secs(&clock, 3);
    let reply = core.finish(a, failed, &failed_request(1)).unwrap();
    assert_eq!(reply.state, JobState::Failed);
    let after = core.job(failed).unwrap().unwrap();
    assert_eq!(after.fail_code.as_deref(), Some("HLAE_ERROR"));
    assert_eq!(after.finished_at, Some(T0 + 3_000));
    assert!(after.lease_until.is_none() && !after.lease_lost);

    // Любое дальнейшее событие — job_terminal; строка не меняется.
    secs(&clock, 100);
    let code = |e: Reject| e.code;
    assert_eq!(
        code(core.finish(a, failed, &cancelled_request(1)).unwrap_err()),
        codes::JOB_TERMINAL
    );
    assert_eq!(
        code(progress(&core, a, failed, 1, 2, Phase::Record).unwrap_err()),
        codes::JOB_TERMINAL
    );
    let release = ReleaseRequest {
        epoch: 1,
        reason: String::new(),
        counts: true,
    };
    assert_eq!(
        code(core.release(a, failed, &release).unwrap_err()),
        codes::JOB_TERMINAL
    );
    assert_eq!(
        core.cancel(failed, true).unwrap_err(),
        "задача уже завершена"
    );
    assert_eq!(
        code(core.finish(b, failed, &failed_request(1)).unwrap_err()),
        codes::JOB_TERMINAL,
        "чужой раннер на окончательной задаче тоже получает job_terminal"
    );
    core.sweep().unwrap();
    assert!(core.claim(b).unwrap().is_none(), "окончательную не выдают");
    assert_eq!(core.job(failed).unwrap().unwrap(), after);

    // Отмена раннером, затем сбой — отказ.
    let cancelled = submit(&core, &dir, "two");
    core.claim(a).unwrap();
    let reply = core
        .finish(a, cancelled, &cancelled_request(2))
        .unwrap_err();
    assert_eq!(
        reply.code,
        codes::STALE_EPOCH,
        "эпоха второй задачи — 1, не 2"
    );
    let reply = core.finish(a, cancelled, &cancelled_request(1)).unwrap();
    assert_eq!(reply.state, JobState::Cancelled);
    let frozen = core.job(cancelled).unwrap().unwrap();
    assert_eq!(
        code(core.finish(a, cancelled, &failed_request(1)).unwrap_err()),
        codes::JOB_TERMINAL
    );
    assert_eq!(core.job(cancelled).unwrap().unwrap(), frozen);
}

#[test]
fn repeating_the_same_finish_is_idempotent() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let id = submit(&core, &dir, "one");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();

    let first = core.finish(a, id, &failed_request(1)).unwrap();
    let row_after = core.job(id).unwrap().unwrap();
    secs(&clock, 30);
    let again = core.finish(a, id, &failed_request(1)).unwrap();
    assert_eq!(first, again);
    assert_eq!(
        core.job(id).unwrap().unwrap(),
        row_after,
        "ни finished_at, ни updated_at не сдвинулись"
    );

    let other = submit(&core, &dir, "two");
    let got = core.claim(a).unwrap().unwrap();
    assert_eq!(got.job_id, other);
    core.finish(a, other, &cancelled_request(1)).unwrap();
    let frozen = core.job(other).unwrap().unwrap();
    secs(&clock, 30);
    assert_eq!(
        core.finish(a, other, &cancelled_request(1)).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(core.job(other).unwrap().unwrap(), frozen);
}

#[test]
fn cancel_of_a_queued_job_is_immediate_and_of_a_leased_job_is_a_flag() {
    let (core, _clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let b = runner(&core, "b");

    // queued и receiving — сразу.
    let queued = submit(&core, &dir, "one");
    assert_eq!(core.cancel(queued, false).unwrap(), JobState::Cancelled);
    let job = core.job(queued).unwrap().unwrap();
    assert!(job.finished_at.is_some() && job.lease_until.is_none());
    let receiving = core
        .begin_receive(&Submit {
            player: "two".to_owned(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(core.cancel(receiving, false).unwrap(), JobState::Cancelled);

    // Выданная — флаг: мягкий, затем жёсткий.
    let leased = submit(&core, &dir, "three");
    assert_eq!(core.claim(a).unwrap().unwrap().job_id, leased);
    progress(&core, a, leased, 1, 1, Phase::Record).unwrap();
    assert_eq!(core.cancel(leased, false).unwrap(), JobState::Running);
    assert_eq!(
        core.job(leased).unwrap().unwrap().cancel_requested,
        CancelMode::Soft
    );
    let (reply, _) = progress(&core, a, leased, 1, 2, Phase::Record).unwrap();
    assert_eq!(reply.cancel, CancelMode::Soft);
    assert_eq!(core.cancel(leased, false).unwrap(), JobState::Running);
    assert_eq!(
        core.job(leased).unwrap().unwrap().cancel_requested,
        CancelMode::Force,
        "повторная отмена ужесточает"
    );
    let (reply, _) = progress(&core, a, leased, 1, 3, Phase::Record).unwrap();
    assert_eq!(reply.cancel, CancelMode::Force);

    // С `force` сразу жёсткий.
    let second = submit(&core, &dir, "four");
    assert_eq!(core.claim(b).unwrap().unwrap().job_id, second);
    assert_eq!(core.cancel(second, true).unwrap(), JobState::Assigned);
    assert_eq!(
        core.job(second).unwrap().unwrap().cancel_requested,
        CancelMode::Force
    );

    // hello выданной раннеру отменённой задачи — директива cancel.
    let hello = core
        .hello(
            a,
            &HelloRequest {
                protocol: wire::PROTOCOL_VERSION,
                name: "a".to_owned(),
                app_version: String::new(),
                status: String::new(),
                holding: vec![Holding {
                    job_id: leased,
                    epoch: 1,
                }],
            },
        )
        .unwrap();
    assert_eq!(hello.directives[0].action, wire::DirectiveAction::Cancel);

    // Окончательная и несуществующая.
    assert_eq!(
        core.cancel(queued, false).unwrap_err(),
        "задача уже завершена"
    );
    assert_eq!(core.cancel(999, false).unwrap_err(), "нет задачи #999");
}

#[test]
fn cancel_requested_job_with_an_expired_lease_becomes_cancelled() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let id = submit(&core, &dir, "one");
    core.claim(a).unwrap();
    progress(&core, a, id, 1, 1, Phase::Record).unwrap();
    let parts = put_parts(&core, id);
    core.cancel(id, false).unwrap();

    secs(&clock, 46);
    let out = core.sweep().unwrap();
    assert_eq!(out.cancelled, vec![id]);
    assert!(out.requeued.is_empty());
    let job = core.job(id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Cancelled, "а не queued");
    assert!(job.finished_at.is_some() && job.lease_until.is_none());
    assert!(!parts.join("highlights.mp4.part").exists(), "куски убраны");
}

#[test]
fn recover_discards_parts_of_finished_jobs_only() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let a = runner(&core, "a");
    let b = runner(&core, "b");
    let finished = submit(&core, &dir, "one");
    let live = submit(&core, &dir, "two");
    core.claim(a).unwrap();
    core.claim(b).unwrap();
    progress(&core, b, live, 1, 1, Phase::Upload).unwrap();
    core.finish(a, finished, &failed_request(1)).unwrap();

    // Остаток после сбоя процесса: куски окончательной задачи и живой выдачи.
    let dead = put_parts(&core, finished);
    std::fs::write(dead.join("highlights.mp4"), b"done").unwrap();
    let alive = put_parts(&core, live);
    drop(core);

    let core = open_on(&dir, &clock);
    let rec = core.recover().unwrap();
    assert_eq!(rec.parts_discarded, 1);
    assert!(!dead.join("highlights.mp4.part").exists());
    assert!(dead.join("highlights.mp4").exists(), "готовый файл цел");
    assert!(
        alive.join("highlights.mp4.part").exists(),
        "у живой выдачи куски остаются"
    );
}

/// Линейный конгруэнтный генератор: ни нового крейта, ни зависимости от времени.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// Держатель задачи: раннер, которому она выдана (в том числе с потерянной арендой).
fn held_by(jobs: &[JobRow], runner: i64) -> Option<&JobRow> {
    jobs.iter().find(|j| {
        j.runner_id == Some(runner)
            && (j.state.is_leased() || (j.state == JobState::Queued && j.lease_lost))
    })
}

fn check_invariants(prev: &[JobRow], cur: &[JobRow], runners: &[i64], max_grants: i64, ctx: &str) {
    assert_eq!(prev.len(), cur.len());
    for (p, c) in prev.iter().zip(cur) {
        assert!(c.epoch >= p.epoch, "{ctx}: эпоха задачи #{} убыла", c.id);
        if p.state.is_terminal() {
            assert_eq!(
                c.state, p.state,
                "{ctx}: окончательная #{} сменила состояние",
                c.id
            );
            assert_eq!(
                c.finished_at, p.finished_at,
                "{ctx}: finished_at #{} сдвинулось",
                c.id
            );
        }
        if c.state.is_terminal() {
            assert!(
                c.finished_at.is_some(),
                "{ctx}: у окончательной #{} нет finished_at",
                c.id
            );
            assert!(
                c.lease_until.is_none(),
                "{ctx}: у окончательной #{} осталась аренда",
                c.id
            );
        }
        if c.state.is_leased() {
            assert!(
                c.runner_id.is_some(),
                "{ctx}: у выданной #{} нет раннера",
                c.id
            );
            assert!(
                c.lease_until.is_some(),
                "{ctx}: у выданной #{} нет срока",
                c.id
            );
        }
        if c.state == JobState::Queued {
            assert!(
                c.lease_until.is_none(),
                "{ctx}: у queued #{} есть срок аренды",
                c.id
            );
        }
        assert!(
            c.grants <= max_grants,
            "{ctx}: выдач #{} больше потолка",
            c.id
        );
    }
    for r in runners {
        let n = cur
            .iter()
            .filter(|j| j.state.is_leased() && j.runner_id == Some(*r))
            .count();
        assert!(n <= 1, "{ctx}: у раннера {r} выданных задач: {n}");
    }
}

#[test]
fn random_sequences_keep_the_invariants() {
    let limits = Limits::default();
    let max_grants = limits.max_grants;
    for seed in 1..=5u64 {
        let (core, clock, dir) = core_with_clock(limits.clone());
        let a = runner(&core, "a");
        let b = runner(&core, "b");
        let runners = [a, b];
        for name in ["one", "two", "three"] {
            submit(&core, &dir, name);
        }
        let mut rng = Lcg(seed);
        // Что раннер помнит по последнему ответу claim; может быть устаревшим.
        let mut remembered: [Option<(i64, i64)>; 2] = [None, None];
        let mut prev = core.jobs().unwrap();

        for step in 0..600 {
            let jobs = core.jobs().unwrap();
            let who = rng.below(2) as usize;
            let r = runners[who];
            let op = rng.below(9);
            let ctx = format!("зерно {seed}, шаг {step}, операция {op}");
            match op {
                0 => match core.claim_report(r) {
                    Ok(c) => {
                        if let Some(asg) = c.assignment {
                            remembered[who] = Some((asg.job_id, asg.epoch));
                        }
                    }
                    Err(e) => assert_eq!(e.code, codes::ALREADY_HOLDING, "{ctx}: {}", e.message),
                },
                1 => {
                    if let Some(j) = held_by(&jobs, r) {
                        let phase =
                            [Phase::Fetch, Phase::Record, Phase::Upload][rng.below(3) as usize];
                        progress(&core, r, j.id, j.epoch, j.last_seq as u64 + 1, phase)
                            .unwrap_or_else(|e| panic!("{ctx}: прогресс держателя: {}", e.message));
                    }
                }
                2 => {
                    // Прогресс с чужой эпохой отсекается и ничего не меняет.
                    let j = &jobs[rng.below(jobs.len() as u64) as usize];
                    let epoch = if rng.below(2) == 0 {
                        j.epoch - 1
                    } else {
                        j.epoch + 1
                    };
                    let e = progress(&core, r, j.id, epoch, 99, Phase::Record)
                        .expect_err("прогресс с чужой эпохой должен быть отказан");
                    assert!(
                        e.code == codes::STALE_EPOCH || e.code == codes::JOB_TERMINAL,
                        "{ctx}: код {}",
                        e.code
                    );
                    assert_eq!(
                        &core.job(j.id).unwrap().unwrap(),
                        j,
                        "{ctx}: отказ изменил строку"
                    );
                }
                3 => secs(&clock, 5 + rng.below(56)),
                4 => {
                    core.sweep().unwrap();
                }
                5 => {
                    let j = &jobs[rng.below(jobs.len() as u64) as usize];
                    let force = rng.below(2) == 0;
                    if let Err(msg) = core.cancel(j.id, force) {
                        assert_eq!(msg, "задача уже завершена", "{ctx}");
                        assert!(j.state.is_terminal(), "{ctx}");
                    }
                }
                6 => {
                    if let Some(j) = held_by(&jobs, r) {
                        let req = if rng.below(2) == 0 {
                            failed_request(j.epoch)
                        } else {
                            cancelled_request(j.epoch)
                        };
                        core.finish(r, j.id, &req)
                            .unwrap_or_else(|e| panic!("{ctx}: итог держателя: {}", e.message));
                    }
                }
                7 => {
                    if let Some(j) = held_by(&jobs, r) {
                        core.release(
                            r,
                            j.id,
                            &ReleaseRequest {
                                epoch: j.epoch,
                                reason: String::new(),
                                counts: rng.below(2) == 0,
                            },
                        )
                        .unwrap_or_else(|e| panic!("{ctx}: возврат держателя: {}", e.message));
                    }
                }
                _ => {
                    let holding: Vec<Holding> = remembered[who]
                        .into_iter()
                        .map(|(job_id, epoch)| Holding { job_id, epoch })
                        .collect();
                    core.hello(
                        r,
                        &HelloRequest {
                            protocol: wire::PROTOCOL_VERSION,
                            name: String::new(),
                            app_version: String::new(),
                            status: String::new(),
                            holding,
                        },
                    )
                    .unwrap_or_else(|e| panic!("{ctx}: hello: {}", e.message));
                }
            }
            let cur = core.jobs().unwrap();
            check_invariants(&prev, &cur, &runners, max_grants, &ctx);
            prev = cur;
        }

        // Финал: все аренды истекают, раннер A забирает и роняет всё, что выдаётся.
        secs(&clock, 120);
        core.sweep().unwrap();
        for _ in 0..20 {
            let c = match core.claim_report(a) {
                Ok(c) => c,
                Err(e) => panic!("зерно {seed}: финальная выдача: {}", e.message),
            };
            let Some(asg) = c.assignment else { break };
            core.finish(a, asg.job_id, &failed_request(asg.epoch))
                .unwrap_or_else(|e| panic!("зерно {seed}: финальный итог: {}", e.message));
        }
        let end = core.jobs().unwrap();
        check_invariants(
            &prev,
            &end,
            &runners,
            max_grants,
            &format!("зерно {seed}, финал"),
        );
        for j in &end {
            assert!(
                j.state.is_terminal(),
                "зерно {seed}: задача #{} осталась {:?}",
                j.id,
                j.state
            );
        }
    }
}

#[test]
fn runner_that_claims_again_drops_its_own_lost_lease() {
    let (core, clock, dir) = core_with_clock(Limits::default());
    let y = runner(&core, "y");
    let x = runner(&core, "x");
    let first = submit(&core, &dir, "one");
    let second = submit(&core, &dir, "two");
    assert_eq!(core.claim(y).unwrap().unwrap().job_id, first);
    assert_eq!(core.claim(x).unwrap().unwrap().job_id, second);
    progress(&core, x, second, 1, 1, Phase::Record).unwrap();
    secs(&clock, 46);
    // Аренда второй задачи у X истекла, а Y вернул первую: в очереди обе, вторая с потерянной арендой X.
    core.release(
        y,
        first,
        &ReleaseRequest {
            epoch: 1,
            reason: String::new(),
            counts: true,
        },
    )
    .unwrap();
    core.sweep().unwrap();
    assert!(core.job(second).unwrap().unwrap().lease_lost);

    // X просит новую задачу и получает первую: вторая ему больше не принадлежит.
    assert_eq!(core.claim(x).unwrap().unwrap().job_id, first);
    let lost = core.job(second).unwrap().unwrap();
    assert!(!lost.lease_lost && lost.runner_id.is_none());
    let e = progress(&core, x, second, 1, 2, Phase::Record).unwrap_err();
    assert_eq!(
        e.code,
        codes::STALE_EPOCH,
        "двух задач у раннера быть не может"
    );
    assert_eq!(
        core.claim(y).unwrap().unwrap().job_id,
        second,
        "её берёт другой раннер"
    );
}
