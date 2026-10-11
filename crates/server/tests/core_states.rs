//! Переходы и аренды ядра на виртуальных часах без HTTP (D-05, SRV-03, SRV-04, SRV-07).
//!
//! Время двигает `ManualClock`, раннеры — строки в базе, а `Core` вызывается напрямую. Свой маленький `TestDir`
//! вместо `tests/common`: этот тестовый бинарник не тянет HTTP-стенд.

use server::auth;
use server::clock::ManualClock;
use server::core::{Core, CoreOptions, Limits, Reject, Submit};
use server::secret::Redactor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wire::{CancelMode, JobState, Phase, ProgressReply, ProgressRequest, ReleaseRequest, codes};

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
