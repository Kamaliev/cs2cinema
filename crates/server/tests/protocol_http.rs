//! Протокол сервера и раннера через настоящий HTTP на localhost (D-05): сервер поднимается в процессе теста,
//! клиент — `ureq`. Ожидание long-poll идёт по реальному времени, аренда — по часам сервера.

mod common;

use common::{TestOptions, TestServer, fake_demo};
use server::clock::SystemClock;
use server::core::{Core, CoreOptions, Limits};
use server::log::{Log, MemorySink};
use server::secret::Redactor;
use sha2::{Digest, Sha256};
use std::net::TcpListener;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};
use wire::{Assignment, JobState};

const MIB: usize = 1024 * 1024;

type Claimed = (u16, Option<Assignment>);

/// Все раннеры стартуют за общим барьером и одновременно просят задачу без ожидания.
fn claim_together(srv: &TestServer, tokens: &[String]) -> Vec<Claimed> {
    let gate = Barrier::new(tokens.len());
    std::thread::scope(|s| {
        let hs: Vec<_> = tokens
            .iter()
            .map(|t| {
                let gate = &gate;
                s.spawn(move || {
                    gate.wait();
                    srv.claim(t, 0)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

#[test]
fn submitted_demo_is_claimed_over_http_once() {
    let srv = TestServer::start();
    let bytes = fake_demo(3 * MIB);
    let id = srv.submit(&bytes, "s1mple");
    let token = srv.add_runner("main");
    let before = srv.running().core().clock().now_ms();

    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let a = got.expect("задача выдана");
    assert_eq!(a.job_id, id);
    assert_eq!(a.epoch, 1);
    assert_eq!(a.demo.size, (3 * MIB) as u64);
    assert_eq!(a.demo.sha256, server::hex(&Sha256::digest(&bytes)));
    assert_eq!(a.player.name, "s1mple");
    let after = srv.running().core().clock().now_ms();
    assert!(
        (before + 43_000..=after + 47_000).contains(&a.lease_until_ms),
        "аренда ≈ сейчас + 45 с: {} при {before}..{after}",
        a.lease_until_ms
    );

    let job = srv.job(id);
    assert_eq!(job.state, JobState::Assigned);
    assert_eq!((job.grants, job.epoch), (1, 1));
    let runner = server::auth::list_runners(srv.running().core().db()).unwrap();
    assert_eq!(job.runner_id, Some(runner[0].id));

    // Тот же раннер повторил запрос, пока задача не подтверждена: ответ мог потеряться, поэтому выдача
    // повторяется без новой эпохи и без новой выдачи. Другой раннер получает пустую очередь.
    let (status, again) = srv.claim(&token, 0);
    assert_eq!(status, 200, "повторная выдача той же задачи");
    let again = again.expect("та же задача");
    assert_eq!((again.job_id, again.epoch), (id, 1));
    assert_eq!(srv.job(id).grants, 1);
    let other = srv.add_runner("other");
    let (status, none) = srv.claim(&other, 0);
    assert_eq!(status, 204, "очередь пуста");
    assert!(none.is_none());

    let log = srv.sink.lines().join("\n");
    assert!(
        log.lines()
            .any(|l| l.contains("POST /v1/runner/claim") && l.contains("200")),
        "{log}"
    );
    assert!(log.contains("раннер main"), "{log}");
    assert!(!log.contains(&token), "токена в журнале быть не должно");
}

#[test]
fn concurrent_claims_hand_the_job_to_exactly_one_runner() {
    let srv = TestServer::start();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let tokens = [srv.add_runner("a"), srv.add_runner("b")];
    let results = claim_together(&srv, &tokens);
    let won: Vec<_> = results.iter().filter(|(st, _)| *st == 200).collect();
    assert_eq!(
        won.len(),
        1,
        "ответы: {:?}",
        results.iter().map(|r| r.0).collect::<Vec<_>>()
    );
    assert_eq!(won[0].1.as_ref().unwrap().job_id, id);
    assert_eq!(results.iter().filter(|(st, _)| *st == 204).count(), 1);
    assert_eq!(srv.job(id).grants, 1);

    // Четыре раннера и две задачи: ровно две выдачи с разными id.
    let srv = TestServer::start();
    let a = srv.submit(&fake_demo(4096), "one");
    let b = srv.submit(&fake_demo(4096), "two");
    let tokens: Vec<String> = (0..4).map(|n| srv.add_runner(&format!("r{n}"))).collect();
    let results = claim_together(&srv, &tokens);
    let mut ids: Vec<i64> = results
        .iter()
        .filter_map(|(_, a)| a.as_ref().map(|a| a.job_id))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![a, b]);
    assert_eq!(results.iter().filter(|(st, _)| *st == 204).count(), 2);
}

#[test]
fn two_processes_on_one_database_hand_out_each_job_once() {
    let srv = TestServer::start();
    let ids: Vec<i64> = (0..6)
        .map(|n| srv.submit(&fake_demo(2048), &format!("p{n}")))
        .collect();
    // Второй `Core` на той же папке — второе соединение, как у отдельного процесса `submit` или второго `serve`.
    let second = Core::open(CoreOptions {
        data_dir: srv.dir.path.join("data"),
        limits: Limits::default(),
        clock: Arc::new(SystemClock),
        redactor: Redactor::default(),
    })
    .unwrap();
    let first = srv.running().core().clone();
    // Раннеров больше, чем задач: каждый берёт не больше одной, повторная выдача своей задачи не в счёт.
    let runners: Vec<i64> = (0..8)
        .map(|n| {
            let (r, _) = server::auth::add_runner(first.db(), &format!("r{n}"), 1).unwrap();
            r.id
        })
        .collect();
    let gate = Barrier::new(8);
    let mut got: Vec<i64> = std::thread::scope(|s| {
        let hs: Vec<_> = runners
            .iter()
            .enumerate()
            .map(|(n, rid)| {
                let core = if n % 2 == 0 {
                    first.clone()
                } else {
                    second.clone()
                };
                let gate = &gate;
                s.spawn(move || {
                    gate.wait();
                    let mut mine = Vec::new();
                    for _ in 0..10 {
                        if let Some(a) = core.claim(*rid).unwrap()
                            && !mine.contains(&a.job_id)
                        {
                            mine.push(a.job_id);
                        }
                    }
                    mine
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    got.sort_unstable();
    assert_eq!(got, ids, "каждая задача выдана ровно один раз");
}

#[test]
fn long_poll_returns_as_soon_as_a_job_is_queued() {
    let srv = TestServer::start_with(TestOptions {
        longpoll_secs: 5,
        ..Default::default()
    });
    let token = srv.add_runner("main");
    let started = Instant::now();
    let (status, got, id) = std::thread::scope(|s| {
        let waiter = s.spawn(|| srv.claim(&token, 5));
        std::thread::sleep(Duration::from_millis(300));
        let id = srv.submit(&fake_demo(4096), "s1mple");
        let (status, got) = waiter.join().unwrap();
        (status, got, id)
    });
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().job_id, id);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "ответ должен прийти сразу после постановки, а не по таймауту: {:?}",
        started.elapsed()
    );
}

#[test]
fn long_poll_times_out_with_204_within_its_limit() {
    let srv = TestServer::start();
    let token = srv.add_runner("main");
    let t = Instant::now();
    assert_eq!(srv.claim(&token, 1).0, 204);
    let spent = t.elapsed();
    assert!(
        spent >= Duration::from_millis(900) && spent <= Duration::from_secs(3),
        "wait=1 занял {spent:?}"
    );
    // Просьба подождать дольше предела сервера (longpoll_secs = 2) обрезается до предела.
    let t = Instant::now();
    assert_eq!(srv.claim(&token, 999).0, 204);
    assert!(t.elapsed() <= Duration::from_secs(4), "{:?}", t.elapsed());
}

#[test]
fn missing_malformed_unknown_or_revoked_token_is_refused() {
    let srv = TestServer::start();
    srv.submit(&fake_demo(4096), "s1mple");
    let good = srv.add_runner("good");
    let revoked = srv.add_runner("old");
    let core = srv.running().core();
    server::auth::revoke_runner(core.db(), "old", core.clock().now_ms()).unwrap();
    let stranger = server::auth::issue_token().unwrap();
    let path = format!("{}?wait=0", wire::paths::CLAIM);
    let body = Some(r#"{"status":"idle"}"#);

    for (label, header) in [
        ("без заголовка", None),
        ("Basic", Some("Basic abc".to_owned())),
        ("короткий токен", Some("Bearer abc".to_owned())),
        ("чужой токен", Some(format!("Bearer {stranger}"))),
        ("отозванный токен", Some(format!("Bearer {revoked}"))),
    ] {
        let r = srv.send(&path, header.as_deref(), body);
        assert_eq!(r.status, 401, "{label}: {}", r.body);
        assert_eq!(r.error_code(), "unauthorized", "{label}");
        assert!(
            !r.body.contains(&stranger) && !r.body.contains(&revoked),
            "{label}"
        );
    }
    // Задача при этом никому не досталась.
    assert_eq!(srv.claim(&good, 0).0, 200);

    let log = srv.sink.lines().join("\n");
    for token in [&stranger, &revoked, &good] {
        assert!(!log.contains(token.as_str()), "токен в журнале: {log}");
    }
    for token in [&stranger, &revoked] {
        let fp = server::auth::fingerprint(&server::auth::hash_token(token));
        assert_eq!(fp.len(), 8);
        assert!(
            log.contains(&fp),
            "отпечаток {fp} должен быть в журнале: {log}"
        );
    }
}

#[test]
fn overflow_answers_503_with_retry_after() {
    let srv = TestServer::start_with(TestOptions {
        threads: 1,
        longpoll_secs: 3,
        ..Default::default()
    });
    let token = srv.add_runner("main");
    std::thread::scope(|s| {
        let a = s.spawn(|| srv.claim(&token, 3));
        std::thread::sleep(Duration::from_millis(300));
        let r = srv.send(
            &format!("{}?wait=0", wire::paths::CLAIM),
            Some(&format!("Bearer {token}")),
            Some(r#"{"status":"idle"}"#),
        );
        assert_eq!(r.status, 503, "{}", r.body);
        assert_eq!(r.retry_after.as_deref(), Some("2"));
        assert_eq!(r.error_code(), "overloaded");
        assert_eq!(a.join().unwrap().0, 204);
    });
    assert_eq!(
        srv.claim(&token, 0).0,
        204,
        "после ответа первому запросу сервер снова свободен"
    );
}

#[test]
fn unknown_path_is_404_json() {
    let srv = TestServer::start();
    let r = srv.send("/v1/nothing", None, None);
    assert_eq!(r.status, 404);
    assert_eq!(r.error_code(), "not_found");
}

#[test]
fn handler_panic_answers_500_and_keeps_serving() {
    let sink = Arc::new(MemorySink::default());
    let log = Log::new(sink.clone(), Redactor::default());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let handler: server::http::Handler = Arc::new(|rq| {
        if rq.url() == "/boom" {
            panic!("тестовая паника");
        }
        let _ = rq.respond(tiny_http::Response::from_string("ok"));
    });
    let http = server::http::start(listener, 2, log, handler).unwrap();
    let base = format!("http://{}", http.addr());

    let r = common::send(&format!("{base}/boom"), None, None);
    assert_eq!(r.status, 500);
    let r = common::send(&format!("{base}/fine"), None, None);
    assert_eq!((r.status, r.body.as_str()), (200, "ok"));
    let lines = sink.lines().join("\n");
    assert!(lines.contains("обработчик упал"), "{lines}");
    assert!(lines.contains("тестовая паника"), "{lines}");
    http.stop();
}

#[test]
fn stop_returns_while_a_long_poll_is_waiting() {
    let mut srv = TestServer::start_with(TestOptions {
        longpoll_secs: 30,
        ..Default::default()
    });
    let token = srv.add_runner("main");
    let url = srv.url(&format!("{}?wait=30", wire::paths::CLAIM));
    let client = std::thread::spawn(move || {
        common::agent()
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send(r#"{"status":"idle"}"#)
            .map(|r| r.status().as_u16())
    });
    std::thread::sleep(Duration::from_millis(400));
    let t = Instant::now();
    srv.stop();
    assert!(
        t.elapsed() <= Duration::from_secs(3),
        "остановка заняла {:?}",
        t.elapsed()
    );
    // Клиент получает 204 или обрыв соединения: оба исхода допустимы.
    if let Ok(status) = client.join().unwrap() {
        assert_eq!(status, 204);
    }
}

/// Заголовки сырого `POST claim` без токена с объявленной длиной `len`.
fn raw_claim_head(len: &str) -> String {
    format!(
        "POST {} HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {len}\r\n\r\n",
        wire::paths::CLAIM
    )
}

#[test]
fn huge_content_length_without_a_token_does_not_kill_the_server() {
    use std::io::Write;
    let srv = TestServer::start();
    let token = srv.add_runner("main");

    let mut conn = common::raw_request(srv.addr());
    conn.write_all(raw_claim_head("1000000000000000").as_bytes())
        .unwrap();
    conn.write_all(b"0123456789abcdef").unwrap();
    let r = common::read_reply(&mut conn, false);
    assert_eq!(r.status(), 413, "{}", r.status_line);
    let json: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(json["error"], "too_large");
    drop(conn);

    let started = Instant::now();
    assert_eq!(srv.claim(&token, 0).0, 204, "сервер должен отвечать дальше");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn oversized_body_on_any_path_is_413_before_the_handler() {
    use std::io::Write;
    let srv = TestServer::start();
    let token = srv.add_runner("main");

    // Неизвестный путь: 413, а не 404 — тело отвергается до обработчика.
    let mut conn = common::raw_request(srv.addr());
    conn.write_all(
        b"POST /nope HTTP/1.1\r\nHost: test\r\nContent-Length: 1000000000000000\r\n\r\n",
    )
    .unwrap();
    let r = common::read_reply(&mut conn, false);
    assert_eq!(r.status(), 413, "{}", r.status_line);
    drop(conn);

    // Кусок результата на байт больше потолка — тоже 413, с верным токеном и эпохой.
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!((status, got.unwrap().job_id), (200, id));
    let too_big = wire::CHUNK_SIZE as usize + 64 * 1024 + 1;
    let mut conn = common::raw_request(srv.addr());
    conn.write_all(
        common::raw_put_head(&token, id, 1, 0, &format!("Content-Length: {too_big}\r\n"))
            .as_bytes(),
    )
    .unwrap();
    let r = common::read_reply(&mut conn, false);
    assert_eq!(r.status(), 413, "{}", r.status_line);
    drop(conn);

    // Задача уже у первого раннера, поэтому живость проверяет второй: свободной работы нет, но сервер отвечает.
    let other = srv.add_runner("second");
    assert_eq!(srv.claim(&other, 0).0, 204);
}
