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
