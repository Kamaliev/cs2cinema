//! Аренда, эпоха и `hello` через настоящий HTTP на localhost (D-05). Время двигают ручные часы сервера:
//! тесты не ждут, а `sweep` вызывается рукой (фоновый поток раз в 5 с на ручных часах ничего не меняет,
//! пока часы стоят, поэтому проверки смотрят на состояние задачи, а не на ответ `sweep`).

mod common;

use common::{TestOptions, TestServer, fake_demo};
use server::clock::ManualClock;
use std::time::Duration;
use wire::{CancelMode, JobState, Phase, ProgressReply, ProgressRequest, Take};

const T0: i64 = 1_800_000_000_000;

fn clocked() -> TestServer {
    TestServer::start_with(TestOptions {
        clock: Some(ManualClock::new(T0)),
        ..Default::default()
    })
}

fn req(epoch: i64, seq: u64, phase: Phase) -> ProgressRequest {
    ProgressRequest {
        epoch,
        seq,
        phase,
        take: None,
        status: String::new(),
    }
}

fn runner_id(srv: &TestServer, name: &str) -> i64 {
    server::auth::list_runners(srv.running().core().db())
        .unwrap()
        .into_iter()
        .find(|r| r.name == name)
        .unwrap()
        .id
}

fn reply(body: &str) -> ProgressReply {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[test]
fn lease_loss_requeues_and_the_same_runner_takes_the_job_back() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("a");
    let a = runner_id(&srv, "a");

    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let assignment = got.unwrap();
    assert_eq!((assignment.job_id, assignment.epoch), (id, 1));
    assert_eq!(srv.job(id).state, JobState::Assigned);

    // fetch не начинает работу, запись с дублем переводит в running.
    let (st, body) = srv.progress(&token, id, &req(1, 1, Phase::Fetch));
    assert_eq!(st, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Assigned);
    let with_take = ProgressRequest {
        take: Some(Take { n: 1, m: 3 }),
        ..req(1, 2, Phase::Record)
    };
    let (st, body) = srv.progress(&token, id, &with_take);
    assert_eq!(st, 200, "{body}");
    let r = reply(&body);
    assert_eq!(r.cancel, CancelMode::NotRequested);
    assert_eq!(r.lease_until_ms, T0 + 45_000);
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Running);
    assert_eq!((job.take_n, job.take_m), (Some(1), Some(3)));
    assert_eq!(job.lease_until, Some(T0 + 45_000));

    // Раннер замолчал: аренда истекла, задача в очереди, эпоха и выдача целы.
    clock.advance(Duration::from_secs(46));
    srv.sweep();
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert!(job.lease_lost);
    assert_eq!((job.epoch, job.grants, job.runner_id), (1, 1, Some(a)));

    // Тот же раннер с той же эпохой возвращает задачу себе, останавливаться ему не говорят.
    let (st, body) = srv.progress(&token, id, &req(1, 3, Phase::Record));
    assert_eq!(st, 200, "{body}");
    assert_eq!(reply(&body).cancel, CancelMode::NotRequested);
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Running);
    assert!(!job.lease_lost);
    assert_eq!((job.epoch, job.grants), (1, 1));
    assert_eq!(job.lease_until, Some(T0 + 46_000 + 45_000));
    let log = srv.sink.lines().join("\n");
    assert!(log.contains("вернул себе задачу"), "{log}");
}

#[test]
fn progress_keeps_the_lease_alive() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("a");
    srv.claim(&token, 0);
    for seq in 1..=10u64 {
        let (st, body) = srv.progress(&token, id, &req(1, seq, Phase::Record));
        assert_eq!(st, 200, "{body}");
        clock.advance(Duration::from_secs(10));
        srv.sweep();
        assert_eq!(srv.job(id).state, JobState::Running, "шаг {seq}");
    }
    let job = srv.job(id);
    assert!(!job.lease_lost);
    assert_eq!(job.last_seq, 10);
}

#[test]
fn an_old_seq_extends_the_lease_but_changes_nothing_else() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("a");
    srv.claim(&token, 0);
    let first = ProgressRequest {
        take: Some(Take { n: 2, m: 3 }),
        ..req(1, 5, Phase::Record)
    };
    assert_eq!(srv.progress(&token, id, &first).0, 200);
    clock.advance(Duration::from_secs(20));

    // Опоздавший дубль: старый seq и другая фаза.
    let late = ProgressRequest {
        take: Some(Take { n: 1, m: 3 }),
        ..req(1, 5, Phase::Assemble)
    };
    let (st, body) = srv.progress(&token, id, &late);
    assert_eq!(st, 200, "{body}");
    let job = srv.job(id);
    assert_eq!(job.phase, "record");
    assert_eq!((job.take_n, job.take_m), (Some(2), Some(3)));
    assert_eq!(job.last_seq, 5);
    assert_eq!(
        job.lease_until,
        Some(T0 + 20_000 + 45_000),
        "аренда продлена"
    );
}
