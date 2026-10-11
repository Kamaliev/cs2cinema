//! Бот внутри запущенного сервера (план 07-10): настоящие потоки `bot-poll`, `bot-ingest`, `bot-notify`,
//! `bot-deliver`, настоящий HTTP для раннеров, подставной Telegram. Время идёт по ручным часам, а ожидание
//! результата — по настоящим (`wait_until`): потоки живут своей жизнью, шагов `step()` здесь нет.

mod common;

use common::{TestDir, TestOptions, TestServer, result_bytes, sha256_hex, wait_until};
use server::clock::ManualClock;
use server::tg::fake::FakeTelegram;
use std::time::{Duration, Instant};
use wire::{JobState, ProgressRequest};

const FRIEND: i64 = 111;

#[test]
fn running_server_with_the_bot_turns_a_friend_demo_into_a_delivered_video() {
    let tg_dir = TestDir::new("e2e-tg");
    let tg = FakeTelegram::new(tg_dir.path.clone());
    let mut srv = TestServer::start_with(TestOptions {
        tg: Some(tg.clone()),
        clock: Some(ManualClock::new(1_800_000_000_000)),
        ..Default::default()
    });
    assert!(srv.running().bot().is_some(), "бот должен быть запущен");
    {
        let core = srv.running().core();
        server::users::add_user(
            core.db(),
            "вася",
            Some(FRIEND),
            false,
            core.clock().now_ms(),
        )
        .unwrap();
    }
    let methods = || -> Vec<String> { tg.calls().into_iter().map(|c| c.method).collect() };
    assert!(methods().contains(&"get_me".to_owned()), "{:?}", methods());
    assert!(
        methods().contains(&"set_my_commands".to_owned()),
        "{:?}",
        methods()
    );

    let bytes = common::fake_demo(4096);
    tg.put_document("file-1", "uniq-1", &bytes);
    let upd = tg.document_update(
        FRIEND,
        FRIEND,
        "file-1",
        "uniq-1",
        "match.dem",
        bytes.len() as u64,
        Some("s1mple"),
    );
    tg.push_update(upd);

    // Друг получает подтверждение с номером задачи.
    assert!(
        wait_until(10, || srv.running().core().jobs().unwrap().len() == 1),
        "задача не появилась; журнал: {}",
        srv.sink_text()
    );
    let job_id = srv.running().core().jobs().unwrap()[0].id;
    assert!(
        wait_until(10, || tg
            .texts_to(FRIEND)
            .iter()
            .any(|t| t.contains(&format!("#{job_id}")))),
        "нет подтверждения с номером задачи: {:?}",
        tg.texts_to(FRIEND)
    );
    assert_eq!(srv.job(job_id).state, JobState::Queued);

    // Фейковый раннер по HTTP доводит задачу до done.
    let token = srv.add_runner("e2e-runner");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let a = got.expect("раннер должен получить задачу");
    assert_eq!(a.job_id, job_id);
    let req = ProgressRequest {
        epoch: a.epoch,
        seq: 1,
        phase: wire::Phase::Record,
        take: None,
        status: String::new(),
    };
    assert_eq!(srv.progress(&token, job_id, &req).0, 200);
    let data = result_bytes(64 * 1024);
    let (status, _, body) = srv.put_chunk(&token, job_id, a.epoch, 0, &data);
    assert_eq!(status, 204, "{body}");
    let (status, body) = srv.finish_done(
        &token,
        job_id,
        a.epoch,
        data.len() as u64,
        &sha256_hex(&data),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.job(job_id).state, JobState::Done);

    // Ролик приходит в чат друга без участия теста.
    assert!(
        wait_until(10, || !tg.videos().is_empty()),
        "ролик не доставлен; журнал: {}",
        srv.sink_text()
    );
    let videos = tg.videos();
    assert_eq!(videos.len(), 1, "{videos:?}");
    assert_eq!(videos[0].chat, FRIEND);
    assert_eq!(videos[0].size, data.len() as u64);

    let started = Instant::now();
    srv.stop();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "остановка заняла {:?}",
        started.elapsed()
    );
}

#[test]
fn server_without_bot_url_does_not_start_the_bot() {
    let srv = TestServer::start_with(TestOptions {
        bot_token: Some("123456789:AAEsentinelSENTINELsentinelSENTINEL01".to_owned()),
        ..Default::default()
    });
    assert!(srv.running().bot().is_none());
    assert!(
        !srv.sink_text().contains("бот @"),
        "бот не должен упоминаться в журнале: {}",
        srv.sink_text()
    );
}
