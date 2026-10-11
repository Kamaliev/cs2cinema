//! Опоры бота в ядре: задача FACEIT без файла, счётчики, дубли, место в очереди, «раннер в сети», повтор (07-03).

mod common;

use common::TestServer;
use server::core::Submit;
use wire::JobState;

const LINK: &str =
    "https://www.faceit.com/ru/cs2/room/1-0F9A1B2C-3D4E-5F60-7182-93A4B5C6D7E8/scoreboard";
const CANON: &str = "https://www.faceit.com/en/cs2/room/1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";

fn who(player: &str) -> Submit {
    Submit {
        player: player.to_owned(),
        requested_by: "друг".to_owned(),
        ..Default::default()
    }
}

#[test]
fn faceit_job_is_claimed_over_http_without_a_demo_file() {
    let srv = TestServer::start();
    let token = srv.add_runner("pc");
    let core = srv.running().core();

    let id = core.submit_faceit(LINK, &who("s1mple")).unwrap();
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.source_kind, "faceit");
    assert_eq!(job.faceit_link.as_deref(), Some(CANON));
    assert_eq!(
        job.orig_name,
        "faceit-1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8"
    );
    assert!(
        core.demo_file(id).unwrap().is_none(),
        "файла у задачи FACEIT нет"
    );

    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let a = got.expect("задача выдана");
    assert_eq!(a.job_id, id);
    assert_eq!(a.demo.size, 0);
    assert_eq!(a.demo.sha256, "");
    assert_eq!(a.faceit_link.as_deref(), Some(CANON));
    assert_eq!(a.player.name, "s1mple");

    // Файла нет: скачивание отвечает чистым 404, а не 500.
    let r = srv.get_range(&token, id, a.epoch, None);
    assert_eq!(r.status, 404);

    // Раннер, не сумевший скачать демку, завершает задачу сбоем.
    let req = wire::FinishRequest {
        epoch: a.epoch,
        outcome: wire::Outcome::Failed(wire::Failure {
            code: "FACEIT_FETCH_FAILED".to_owned(),
            message: "нет ключа".to_owned(),
            ..Default::default()
        }),
    };
    let (status, body) = srv.finish(&token, id, &req);
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Failed);
}

#[test]
fn faceit_submit_refuses_foreign_links_and_empty_player() {
    let srv = TestServer::start();
    let core = srv.running().core();
    let err = core
        .submit_faceit(
            "https://evil.com/en/cs2/room/1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8",
            &who("a"),
        )
        .unwrap_err();
    assert_eq!(err, "ссылка не похожа на комнату матча FACEIT");
    assert!(core.submit_faceit(LINK, &who("  ")).is_err());
    assert!(core.jobs().unwrap().is_empty(), "отказ не оставляет задач");
}

// ---------------------------------------------------------------------------------------------------------
// Счётчики, дубли, место в очереди, раннер в сети, повтор
// ---------------------------------------------------------------------------------------------------------

use common::{TestOptions, fake_demo, result_bytes, sha256_hex};
use server::clock::ManualClock;
use server::core::{DedupKey, player_key};
use server::{auth, users};
use std::time::Duration;
use wire::{FinishRequest, Outcome, Phase, ProgressRequest};

const T0: i64 = 1_800_000_000_000;

fn manual_server() -> (TestServer, ManualClock) {
    let clock = ManualClock::new(T0);
    let srv = TestServer::start_with(TestOptions {
        clock: Some(clock.clone()),
        ..Default::default()
    });
    (srv, clock)
}

fn put_demo(srv: &TestServer, bytes: &[u8], player: &str, user_id: Option<i64>) -> i64 {
    let src = srv.dir.path.join(format!("src-{}.dem", sha256_hex(bytes)));
    std::fs::write(&src, bytes).unwrap();
    let s = Submit {
        player: player.to_owned(),
        user_id,
        requested_by: "друг".to_owned(),
        orig_name: "m.dem".to_owned(),
    };
    srv.running().core().submit(&src, &s).unwrap().0
}

fn add_user(srv: &TestServer, name: &str) -> i64 {
    let core = srv.running().core();
    users::add_user(core.db(), name, None, false, core.clock().now_ms())
        .unwrap()
        .id
}

/// Раннер берёт самую старую задачу очереди и доводит её до `running`; возвращает эпоху.
fn claim_running(srv: &TestServer, token: &str, id: i64) -> i64 {
    let (status, got) = srv.claim(token, 0);
    assert_eq!(status, 200);
    let a = got.expect("задача выдана");
    assert_eq!(a.job_id, id, "выдана не та задача");
    let req = ProgressRequest {
        epoch: a.epoch,
        seq: 1,
        phase: Phase::Record,
        take: None,
        status: String::new(),
    };
    assert_eq!(srv.progress(token, id, &req).0, 200);
    a.epoch
}

fn finish_failed(srv: &TestServer, token: &str, id: i64) {
    let epoch = claim_running(srv, token, id);
    let req = FinishRequest {
        epoch,
        outcome: Outcome::Failed(wire::Failure {
            code: "HLAE_ERROR".to_owned(),
            message: "сбой для теста".to_owned(),
            ..Default::default()
        }),
    };
    let (status, body) = srv.finish(token, id, &req);
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Failed);
}

fn upload_and_finish(srv: &TestServer, token: &str, id: i64, epoch: i64) {
    let data = result_bytes(4096);
    let (status, _, body) = srv.put_chunk(token, id, epoch, 0, &data);
    assert_eq!(status, 204, "{body}");
    let (status, body) = srv.finish_done(token, id, epoch, data.len() as u64, &sha256_hex(&data));
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Done);
}

fn finish_done(srv: &TestServer, token: &str, id: i64) {
    let epoch = claim_running(srv, token, id);
    upload_and_finish(srv, token, id, epoch);
}

#[test]
fn active_counts_cover_every_unfinished_state() {
    let (srv, _clock) = manual_server();
    let token = srv.add_runner("pc");
    let core = srv.running().core();
    let a = add_user(&srv, "a");
    let b = add_user(&srv, "b");

    let failed = put_demo(&srv, &fake_demo(2001), "p1", Some(a));
    finish_failed(&srv, &token, failed);
    let done = put_demo(&srv, &fake_demo(2002), "p2", Some(a));
    finish_done(&srv, &token, done);
    let cancelled = put_demo(&srv, &fake_demo(2003), "p3", Some(a));
    core.cancel(cancelled, false).unwrap();
    assert_eq!(
        core.active_counts(a).unwrap().user,
        0,
        "закончившиеся не считаются"
    );

    let running = put_demo(&srv, &fake_demo(2004), "p4", Some(a));
    claim_running(&srv, &token, running);
    let queued = put_demo(&srv, &fake_demo(2005), "p5", Some(a));
    let other = put_demo(&srv, &fake_demo(2006), "p6", Some(b));

    let c = core.active_counts(a).unwrap();
    assert_eq!((c.user, c.total), (2, 3));
    let cb = core.active_counts(b).unwrap();
    assert_eq!((cb.user, cb.total), (1, 3));
    assert_eq!(srv.job(queued).state, JobState::Queued);
    assert_eq!(srv.job(other).state, JobState::Queued);
}

#[test]
fn player_key_normalizes_names_and_keeps_steam_ids() {
    assert_eq!(player_key("  Big   Boss "), "big boss");
    assert_eq!(player_key("S1mple "), "s1mple");
    assert_eq!(player_key("ЗАЙЦЕВ  Иван"), "зайцев иван");
    assert_eq!(player_key("76561198034202275"), "76561198034202275");
    assert_eq!(player_key(" 76561198034202275 "), "76561198034202275");
}

#[test]
fn duplicate_is_found_by_hash_and_player_key_but_not_after_failure() {
    let (srv, _clock) = manual_server();
    let token = srv.add_runner("pc");
    let core = srv.running().core();
    let bytes = fake_demo(3000);
    let sha = sha256_hex(&bytes);
    let first = put_demo(&srv, &bytes, "s1mple", None);

    let key = |p: &str| DedupKey::File {
        sha256: sha.clone(),
        player_key: player_key(p),
    };
    assert_eq!(
        core.find_duplicate(&key("S1mple ")).unwrap().unwrap().id,
        first
    );
    assert!(core.find_duplicate(&key("zywoo")).unwrap().is_none());

    // Самая свежая из подходящих.
    let second = core.submit_from(first, &who("s1mple")).unwrap();
    assert_eq!(
        core.find_duplicate(&key("s1mple")).unwrap().unwrap().id,
        second
    );

    // Отменённая и упавшая дублем не считаются, готовая — считается.
    core.cancel(second, false).unwrap();
    assert_eq!(
        core.find_duplicate(&key("s1mple")).unwrap().unwrap().id,
        first
    );
    finish_failed(&srv, &token, first);
    assert!(core.find_duplicate(&key("s1mple")).unwrap().is_none());
    let third = core.resubmit(first, "друг").unwrap();
    finish_done(&srv, &token, third);
    assert_eq!(
        core.find_duplicate(&key("s1mple")).unwrap().unwrap().id,
        third
    );

    // Ключ FACEIT: по канонической ссылке и игроку, в любом виде ссылки.
    let f = core.submit_faceit(LINK, &who("S1mple")).unwrap();
    let fkey = |link: &str, p: &str| DedupKey::Faceit {
        link: link.to_owned(),
        player_key: player_key(p),
    };
    assert_eq!(
        core.find_duplicate(&fkey(CANON, "s1mple"))
            .unwrap()
            .unwrap()
            .id,
        f
    );
    assert_eq!(
        core.find_duplicate(&fkey(LINK, "s1mple"))
            .unwrap()
            .unwrap()
            .id,
        f
    );
    assert!(
        core.find_duplicate(&fkey(CANON, "zywoo"))
            .unwrap()
            .is_none()
    );
    core.cancel(f, false).unwrap();
    assert!(
        core.find_duplicate(&fkey(CANON, "s1mple"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn jobs_ahead_counts_queued_before_and_running() {
    let (srv, _clock) = manual_server();
    let token = srv.add_runner("pc");
    let core = srv.running().core();
    let j1 = put_demo(&srv, &fake_demo(3101), "a", None);
    let j2 = put_demo(&srv, &fake_demo(3102), "b", None);
    let j3 = put_demo(&srv, &fake_demo(3103), "c", None);
    assert_eq!([j1, j2, j3].map(|j| core.jobs_ahead(j).unwrap()), [0, 1, 2]);

    let (status, got) = srv.claim(&token, 0);
    assert_eq!((status, got.unwrap().job_id), (200, j1));
    assert_eq!(
        core.jobs_ahead(j1).unwrap(),
        0,
        "выданная ничего не обгоняет"
    );
    assert_eq!(core.jobs_ahead(j2).unwrap(), 1);
    assert_eq!(core.jobs_ahead(j3).unwrap(), 2);
    assert_eq!(core.jobs_ahead(9999).unwrap(), 0, "нет такой задачи");
}

#[test]
fn runner_online_follows_last_seen() {
    let (srv, clock) = manual_server();
    let core = srv.running().core();
    assert!(!core.runner_online().unwrap(), "раннеров нет");

    let token = srv.add_runner("pc");
    assert!(
        !core.runner_online().unwrap(),
        "раннер ещё не выходил на связь"
    );
    let (status, _) = srv.claim(&token, 0);
    assert!(status == 200 || status == 204, "{status}");
    assert!(core.runner_online().unwrap());

    let window = Duration::from_millis(
        core.limits().longpoll_secs * 1000 + core.limits().lease_ttl_ms as u64,
    );
    clock.advance(window);
    assert!(core.runner_online().unwrap(), "на границе окна ещё в сети");
    clock.advance(Duration::from_secs(1));
    assert!(!core.runner_online().unwrap());

    let (status, _) = srv.claim(&token, 0);
    assert!(status == 200 || status == 204, "{status}");
    assert!(core.runner_online().unwrap());
    auth::revoke_runner(core.db(), "pc", core.clock().now_ms()).unwrap();
    assert!(
        !core.runner_online().unwrap(),
        "отозванный раннер не считается"
    );
}

#[test]
fn resubmit_reuses_the_demo_by_a_hard_link() {
    let (srv, _clock) = manual_server();
    let token = srv.add_runner("pc");
    let core = srv.running().core();
    let u = add_user(&srv, "friend");
    let bytes = fake_demo(4000);
    let old = put_demo(&srv, &bytes, "s1mple", Some(u));

    assert!(core.resubmit(old, "x").is_err(), "queued повторять нельзя");
    finish_failed(&srv, &token, old);
    let before = srv.job(old);

    let new = core.resubmit(old, "друг").unwrap();
    assert_ne!(new, old);
    let j = srv.job(new);
    assert_eq!(j.state, JobState::Queued);
    assert_eq!((j.player_name.as_str(), j.user_id), ("s1mple", Some(u)));
    assert_eq!(j.requested_by, "друг");
    assert_eq!(j.source_kind, "file");
    let old_file = core.demo_file(old).unwrap().unwrap();
    let new_file = core.demo_file(new).unwrap().unwrap();
    assert_eq!(
        (new_file.size, &new_file.sha256),
        (old_file.size, &old_file.sha256)
    );
    assert_eq!(std::fs::read(core.blobs().demo_path(new)).unwrap(), bytes);
    assert_eq!(srv.job(old), before, "старая задача не меняется");

    // Новая выдаётся раннеру вместе с демкой.
    let (_, got) = srv.claim(&token, 0);
    let a = got.unwrap();
    assert_eq!((a.job_id, a.demo.size), (new, bytes.len() as u64));
    let req = ProgressRequest {
        epoch: a.epoch,
        seq: 1,
        phase: Phase::Record,
        take: None,
        status: String::new(),
    };
    assert_eq!(srv.progress(&token, new, &req).0, 200);
    upload_and_finish(&srv, &token, new, a.epoch);
    assert!(core.resubmit(new, "x").is_err(), "готовую повторять нельзя");

    // Повтор FACEIT-задачи: та же ссылка и ни одного файла.
    let f = core.submit_faceit(LINK, &who("s1mple")).unwrap();
    finish_failed(&srv, &token, f);
    let f2 = core.resubmit(f, "друг").unwrap();
    let fj = srv.job(f2);
    assert_eq!(
        (fj.state, fj.faceit_link.as_deref()),
        (JobState::Queued, Some(CANON))
    );
    assert_eq!(fj.source_kind, "faceit");
    assert!(core.demo_file(f2).unwrap().is_none());
}

#[test]
fn second_player_job_reuses_the_demo_without_a_copy() {
    let (srv, _clock) = manual_server();
    let core = srv.running().core();
    let bytes = fake_demo(5000);
    let a = put_demo(&srv, &bytes, "s1mple", None);

    let b = core.submit_from(a, &who("zywoo")).unwrap();
    let jb = srv.job(b);
    assert_eq!(
        (jb.state, jb.player_name.as_str()),
        (JobState::Queued, "zywoo")
    );
    let fa = core.demo_file(a).unwrap().unwrap();
    let fb = core.demo_file(b).unwrap().unwrap();
    assert_eq!((fa.size, &fa.sha256), (fb.size, &fb.sha256));
    assert_eq!(std::fs::read(core.blobs().demo_path(b)).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let nlink = std::fs::metadata(core.blobs().demo_path(a))
            .unwrap()
            .nlink();
        assert!(
            nlink >= 2,
            "файл демки должен быть жёсткой ссылкой, а не копией"
        );
    }

    // Строка демки исходной задачи убрана: новой задачи не появляется.
    let c = put_demo(&srv, &fake_demo(5001), "x", None);
    let before = core.jobs().unwrap().len();
    core.db()
        .write(|tx| {
            tx.execute("UPDATE files SET deleted_at = 1 WHERE job_id = ?1", [c])
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let err = core.submit_from(c, &who("y")).unwrap_err();
    assert!(err.contains("пришлите файл заново"), "{err}");
    assert_eq!(core.jobs().unwrap().len(), before);
}
