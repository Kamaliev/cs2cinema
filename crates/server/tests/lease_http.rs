//! Аренда, эпоха и `hello` через настоящий HTTP на localhost (D-05). Время двигают ручные часы сервера:
//! тесты не ждут, а `sweep` вызывается рукой (фоновый поток раз в 5 с на ручных часах ничего не меняет,
//! пока часы стоят, поэтому проверки смотрят на состояние задачи, а не на ответ `sweep`).

mod common;

use common::{TestOptions, TestServer, fake_demo};
use server::clock::ManualClock;
use std::time::Duration;
use wire::{
    CancelMode, DirectiveAction, Failure, FinishReply, FinishRequest, HelloReply, HelloRequest,
    Holding, JobState, Outcome, Phase, ProgressReply, ProgressRequest, ReleaseRequest, Take,
};

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

#[test]
fn stale_epoch_progress_is_refused_over_http() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let old = srv.add_runner("old");
    let new = srv.add_runner("new");
    srv.claim(&old, 0);
    assert_eq!(srv.progress(&old, id, &req(1, 1, Phase::Record)).0, 200);

    clock.advance(Duration::from_secs(46));
    srv.sweep();
    let (status, got) = srv.claim(&new, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 2, "новая выдача — новая эпоха");

    let (status, body) = srv.progress(&old, id, &req(1, 2, Phase::Record));
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("stale_epoch"), "{body}");
    assert_eq!(srv.job(id).last_seq, 0, "старый раннер ничего не записал");
    assert_eq!(srv.progress(&new, id, &req(2, 1, Phase::Record)).0, 200);
}

#[test]
fn poison_demo_fails_after_max_grants_over_http() {
    let srv = clocked();
    let clock = srv.clock();
    let poison = srv.submit(&fake_demo(4096), "poison");
    let token = srv.add_runner("a");
    for round in 1..=3 {
        let (status, got) = srv.claim(&token, 0);
        assert_eq!(status, 200, "выдача {round}");
        assert_eq!(got.unwrap().epoch, round);
        clock.advance(Duration::from_secs(46));
        srv.sweep();
    }
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 204, "четвёртой выдачи нет");
    assert!(got.is_none());
    let job = srv.job(poison);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("TOO_MANY_GRANTS"));
    let log = srv.sink.lines().join("\n");
    assert!(log.contains("превышено число выдач"), "{log}");
}

#[test]
fn lost_lease_on_the_last_grant_survives_another_runners_claim_over_http() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "poison");
    let token_a = srv.add_runner("a");
    let token_b = srv.add_runner("b");
    for round in 1..=3 {
        let (status, got) = srv.claim(&token_a, 0);
        assert_eq!(status, 200, "выдача {round}");
        assert_eq!(got.unwrap().epoch, round);
        clock.advance(Duration::from_secs(46));
        srv.sweep();
    }
    assert_eq!(srv.job(id).state, JobState::Queued);

    // Чужой раннер просит задачу: задача на последней выдаче ждёт своего владельца.
    let (status, got) = srv.claim(&token_b, 0);
    assert_eq!(status, 204, "выдавать нечего");
    assert!(got.is_none());
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Queued, "чужой claim не роняет задачу");
    assert!(job.lease_lost);

    // Владелец вернулся через hello и получил задачу обратно.
    let held = vec![Holding {
        job_id: id,
        epoch: 3,
    }];
    let (status, body) = srv.hello(&token_a, &hello_req(held));
    assert_eq!(status, 200, "{body}");
    assert_eq!(action_of(&hello_reply(&body), id), DirectiveAction::Keep);
    let job = srv.job(id);
    assert!(!job.lease_lost);
    assert_eq!((job.epoch, job.grants), (3, 3));
}

#[test]
fn release_over_http_requeues_the_job() {
    let srv = clocked();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("a");
    srv.claim(&token, 0);

    let bad = ReleaseRequest {
        epoch: 5,
        reason: String::new(),
        counts: true,
    };
    let (status, body) = srv.release(&token, id, &bad);
    assert_eq!(status, 409, "{body}");

    let ok = ReleaseRequest {
        epoch: 1,
        reason: "окружение не готово".to_owned(),
        counts: true,
    };
    let (status, body) = srv.release(&token, id, &ok);
    assert_eq!(status, 204, "{body}");
    assert_eq!(srv.job(id).state, JobState::Queued);
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 2);
    let log = srv.sink.lines().join("\n");
    assert!(
        log.contains("вернул задачу") && log.contains("окружение не готово"),
        "{log}"
    );

    // Задача, которой нет, и кривой id.
    let (status, _) = srv.release(&token, 999, &ok);
    assert_eq!(status, 404);
    let r = srv.send(
        "/v1/jobs/abc/release",
        Some(&format!("Bearer {token}")),
        Some("{}"),
    );
    assert_eq!(r.status, 404);
}

fn hello_req(holding: Vec<Holding>) -> HelloRequest {
    HelloRequest {
        protocol: 1,
        name: "main".to_owned(),
        app_version: "0.5.0".to_owned(),
        status: "idle".to_owned(),
        holding,
    }
}

fn hello_reply(body: &str) -> HelloReply {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

fn action_of(reply: &HelloReply, job_id: i64) -> DirectiveAction {
    reply
        .directives
        .iter()
        .find(|d| d.job_id == job_id)
        .unwrap_or_else(|| panic!("нет указания по задаче #{job_id}"))
        .action
}

fn runner_version(srv: &TestServer) -> Option<String> {
    srv.running()
        .core()
        .db()
        .read(|c| {
            c.query_row("SELECT version FROM runners WHERE name = 'main'", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())
        })
        .unwrap()
}

#[test]
fn hello_reconciles_held_jobs() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("main");
    srv.claim(&token, 0);
    assert_eq!(srv.progress(&token, id, &req(1, 1, Phase::Record)).0, 200);
    clock.advance(Duration::from_secs(20));

    let held = vec![
        Holding {
            job_id: id,
            epoch: 1,
        },
        Holding {
            job_id: id + 100,
            epoch: 1,
        },
    ];
    let (status, body) = srv.hello(&token, &hello_req(held));
    assert_eq!(status, 200, "{body}");
    let reply = hello_reply(&body);
    assert_eq!(action_of(&reply, id), DirectiveAction::Keep);
    assert_eq!(
        action_of(&reply, id + 100),
        DirectiveAction::Abandon,
        "задачи нет"
    );
    assert_eq!(
        srv.job(id).lease_until,
        Some(T0 + 20_000 + 45_000),
        "аренда продлена"
    );
    assert_eq!(reply.poll_secs, 2, "longpoll стенда");
    assert_eq!(reply.heartbeat_secs, 10);
    assert_eq!(reply.chunk_size, 8 * 1024 * 1024);
    assert_eq!(reply.server_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(reply.server_time_ms, T0 + 20_000);
    assert_eq!(runner_version(&srv), Some("0.5.0".to_owned()));

    // Эпоха 0 — «бросить».
    let wrong = vec![Holding {
        job_id: id,
        epoch: 0,
    }];
    let (_, body) = srv.hello(&token, &hello_req(wrong));
    assert_eq!(action_of(&hello_reply(&body), id), DirectiveAction::Abandon);
    let log = srv.sink.lines().join("\n");
    assert!(log.contains("раннер main на связи (0.5.0)"), "{log}");
}

#[test]
fn hello_reattaches_a_lost_job() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("main");
    srv.claim(&token, 0);
    assert_eq!(srv.progress(&token, id, &req(1, 1, Phase::Record)).0, 200);
    clock.advance(Duration::from_secs(60));
    srv.sweep();
    assert_eq!(srv.job(id).state, JobState::Queued);

    let held = vec![Holding {
        job_id: id,
        epoch: 1,
    }];
    let (status, body) = srv.hello(&token, &hello_req(held));
    assert_eq!(status, 200, "{body}");
    assert_eq!(action_of(&hello_reply(&body), id), DirectiveAction::Keep);
    let job = srv.job(id);
    assert_eq!(
        job.state,
        JobState::Running,
        "этап взят из сохранённой фазы"
    );
    assert!(!job.lease_lost);
    assert_eq!((job.epoch, job.grants), (1, 1));
    assert_eq!(job.lease_until, Some(T0 + 60_000 + 45_000));
}

#[test]
fn hello_with_another_protocol_is_refused_with_426() {
    let srv = clocked();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("main");
    srv.claim(&token, 0);
    let before = srv.job(id);

    let mut bad = hello_req(vec![Holding {
        job_id: id,
        epoch: 1,
    }]);
    bad.protocol = 2;
    let r = srv.send(
        wire::paths::HELLO,
        Some(&format!("Bearer {token}")),
        Some(&serde_json::to_string(&bad).unwrap()),
    );
    assert_eq!(r.status, 426, "{}", r.body);
    assert_eq!(r.error_code(), "protocol_mismatch");
    assert!(r.body.contains("обновите раннер"), "{}", r.body);
    assert_eq!(srv.job(id), before, "задача не тронута");
    assert_eq!(runner_version(&srv), None, "версия раннера не записана");
}

#[test]
fn revoked_runner_is_refused_and_its_job_goes_to_another_runner() {
    let srv = clocked();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let main = srv.add_runner("main");
    let other = srv.add_runner("other");
    srv.claim(&main, 0);
    assert_eq!(srv.progress(&main, id, &req(1, 1, Phase::Record)).0, 200);

    let core = srv.running().core();
    server::auth::revoke_runner(core.db(), "main", core.clock().now_ms()).unwrap();
    assert_eq!(srv.progress(&main, id, &req(1, 2, Phase::Record)).0, 401);
    let held = vec![Holding {
        job_id: id,
        epoch: 1,
    }];
    let (status, _) = srv.hello(&main, &hello_req(held));
    assert_eq!(status, 401);

    srv.sweep();
    assert_eq!(srv.job(id).state, JobState::Queued);
    let (status, got) = srv.claim(&other, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 2, "новая выдача — новая эпоха");
    assert_eq!(srv.job(id).grants, 2);
}

// ---------------------------------------------------------------------------------------------------------
// Итог задачи и отмена (05-04, SRV-03)
// ---------------------------------------------------------------------------------------------------------

fn failed_body(epoch: i64) -> FinishRequest {
    FinishRequest {
        epoch,
        outcome: Outcome::Failed(Failure {
            code: "HLAE_ERROR".to_owned(),
            message: "boom".to_owned(),
            ..Default::default()
        }),
    }
}

fn running_job(srv: &TestServer, token: &str) -> i64 {
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let (status, got) = srv.claim(token, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().job_id, id);
    let (st, body) = srv.progress(token, id, &req(1, 1, Phase::Record));
    assert_eq!(st, 200, "{body}");
    id
}

#[test]
fn cancel_reaches_the_runner_in_the_progress_reply() {
    let srv = clocked();
    let token = srv.add_runner("a");
    let id = running_job(&srv, &token);
    let core = srv.running().core();

    core.cancel(id, false).unwrap();
    let (st, body) = srv.progress(&token, id, &req(1, 2, Phase::Record));
    assert_eq!(st, 200, "{body}");
    assert_eq!(reply(&body).cancel, CancelMode::Soft);
    assert_eq!(srv.job(id).state, JobState::Running, "состояние то же");

    core.cancel(id, false).unwrap();
    let (_, body) = srv.progress(&token, id, &req(1, 3, Phase::Record));
    assert_eq!(reply(&body).cancel, CancelMode::Force);

    // Раннер остановился и сообщает отмену.
    let cancelled = FinishRequest {
        epoch: 1,
        outcome: Outcome::Cancelled,
    };
    let (st, body) = srv.finish(&token, id, &cancelled);
    assert_eq!(st, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<FinishReply>(&body).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(srv.job(id).state, JobState::Cancelled);

    let (st, body) = srv.progress(&token, id, &req(1, 4, Phase::Record));
    assert_eq!(st, 409, "{body}");
    assert!(body.contains("job_terminal"), "{body}");
}

#[test]
fn hello_tells_the_runner_to_cancel() {
    let srv = clocked();
    let token = srv.add_runner("a");
    let id = running_job(&srv, &token);
    srv.running().core().cancel(id, false).unwrap();

    let hello = HelloRequest {
        protocol: wire::PROTOCOL_VERSION,
        name: "a".to_owned(),
        app_version: "0.1.0".to_owned(),
        status: "busy".to_owned(),
        holding: vec![Holding {
            job_id: id,
            epoch: 1,
        }],
    };
    let (st, body) = srv.hello(&token, &hello);
    assert_eq!(st, 200, "{body}");
    let got: HelloReply = serde_json::from_str(&body).unwrap();
    assert_eq!(got.directives.len(), 1);
    assert_eq!(got.directives[0].action, DirectiveAction::Cancel);
}

#[test]
fn stale_runner_cannot_finish() {
    let srv = clocked();
    let clock = srv.clock();
    let a = srv.add_runner("a");
    let b = srv.add_runner("b");
    let id = running_job(&srv, &a);

    clock.advance(Duration::from_secs(46));
    srv.sweep();
    let (status, got) = srv.claim(&b, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 2);

    // Старый раннер с эпохой 1 и с чужой эпохой 2 отсекается; задача не тронута.
    let before = srv.job(id);
    for epoch in [1, 2] {
        let (st, body) = srv.finish(&a, id, &failed_body(epoch));
        assert_eq!(st, 409, "{body}");
        assert!(body.contains("stale_epoch"), "{body}");
    }
    assert_eq!(srv.job(id), before);
    let (st, body) = srv.finish(&b, id, &failed_body(1));
    assert_eq!(st, 409, "эпоха первой выдачи у нового раннера: {body}");
    let (st, body) = srv.finish(&b, id, &failed_body(2));
    assert_eq!(st, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Failed);
}

#[test]
fn finish_validates_the_outcome_and_cleans_the_failure_fields() {
    let srv = clocked();
    let token = srv.add_runner("a");
    let id = running_job(&srv, &token);
    let bearer = format!("Bearer {token}");
    let path = wire::paths::job(id, "finish");

    // Итог от более новой стороны протокола не принимается.
    let r = srv.send(
        &path,
        Some(&bearer),
        Some(r#"{"epoch":1,"outcome":{"kind":"something_new"}}"#),
    );
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("неизвестный итог задачи"), "{}", r.body);
    assert_eq!(srv.job(id).state, JobState::Running);

    // Без токена, чужая задача, мусорный JSON.
    assert_eq!(srv.send(&path, None, Some("{}")).status, 401);
    let r = srv.send(
        &wire::paths::job(999, "finish"),
        Some(&bearer),
        Some(r#"{"epoch":1,"outcome":{"kind":"cancelled"}}"#),
    );
    assert_eq!(r.status, 404);
    assert_eq!(srv.send(&path, Some(&bearer), Some("не json")).status, 400);

    // Код не из [A-Z0-9_] сохраняется как UNKNOWN, длинное сообщение обрезается, управляющие символы уходят.
    let long = "я".repeat(3000);
    let body = serde_json::json!({
        "epoch": 1,
        "outcome": {"kind": "failed", "code": "hlae error!", "message": format!("a\u{7}b{long}"), "tail": "x\u{1b}y"}
    });
    let r = srv.send(&path, Some(&bearer), Some(&body.to_string()));
    assert_eq!(r.status, 200, "{}", r.body);
    let job = srv.job(id);
    assert_eq!(job.fail_code.as_deref(), Some("UNKNOWN"));
    let message = job.fail_message.unwrap();
    assert_eq!(message.chars().count(), 2000);
    assert!(message.starts_with("ab"), "управляющий символ удалён");
    assert_eq!(job.fail_tail.as_deref(), Some("xy"));
}

#[test]
fn cancelled_leased_job_released_by_the_runner_is_not_issued_again() {
    let srv = clocked();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let first = srv.add_runner("a");
    let second = srv.add_runner("b");
    assert_eq!(srv.claim(&first, 0).0, 200);
    let grants = srv.job(id).grants;

    // Оператор отменяет выданную задачу: она остаётся у раннера с флагом отмены.
    srv.running().core().cancel(id, false).unwrap();
    assert_eq!(srv.job(id).state, JobState::Assigned);

    // Раннер возвращает её без списания выдачи: задача окончена, а не снова в очереди.
    let rel = ReleaseRequest {
        epoch: 1,
        reason: "окружение не готово".to_owned(),
        counts: false,
    };
    let (status, body) = srv.release(&first, id, &rel);
    assert_eq!(status, 204, "{body}");
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.grants, grants, "число выдач не меняется");

    assert_eq!(srv.claim(&first, 0).0, 204);
    assert_eq!(srv.claim(&second, 0).0, 204);
    assert_eq!(srv.job(id).grants, grants);
}
