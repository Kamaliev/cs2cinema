//! Протокол сервера и раннера через настоящий HTTP на localhost (D-05): сервер поднимается в процессе теста,
//! клиент — `ureq`. Часы настоящие там, где важно реальное ожидание, и ручные там, где важна аренда.

mod common;

use common::{TestServer, fake_demo};
use sha2::{Digest, Sha256};
use wire::JobState;

const MIB: usize = 1024 * 1024;

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

    let (status, again) = srv.claim(&token, 0);
    assert_eq!(status, 204, "очередь пуста");
    assert!(again.is_none());

    let log = srv.sink.lines().join("\n");
    assert!(
        log.lines()
            .any(|l| l.contains("POST /v1/runner/claim") && l.contains("200")),
        "{log}"
    );
    assert!(log.contains("раннер main"), "{log}");
    assert!(!log.contains(&token), "токена в журнале быть не должно");
}
