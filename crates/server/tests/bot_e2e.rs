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

// ---------------------------------------------------------------------------------------------------------
// serve на настоящем клиенте: HttpTg против поддельного локального Bot API
// ---------------------------------------------------------------------------------------------------------

use common::FakeBotApi;

const API_TOKEN: &str = "123456789:AAEsentinelSENTINELsentinelSENTINEL01";

#[test]
fn serve_starts_the_bot_against_a_local_bot_api_and_stops_quickly() {
    // Поддельный bot-api объявлен первым: он умирает последним, после остановки сервера.
    let api = FakeBotApi::start(API_TOKEN);
    let mut srv = TestServer::start_with(TestOptions {
        bot_token: Some(API_TOKEN.to_owned()),
        bot_api_url: Some(api.url()),
        ..Default::default()
    });
    assert!(srv.running().bot().is_some(), "бот должен быть запущен");
    assert!(
        wait_until(10, || !api.requests_to("getUpdates").is_empty()),
        "опрос не начался: {:?}",
        api.requests().iter().map(|r| &r.method).collect::<Vec<_>>()
    );
    assert!(!api.requests_to("getMe").is_empty(), "самопроверка getMe");
    assert!(!api.requests_to("setMyCommands").is_empty(), "меню команд");
    let poll = &api.requests_to("getUpdates")[0];
    assert_eq!(poll.body["allowed_updates"], serde_json::json!(["message"]));
    assert!(
        api.requests().iter().all(|r| r.token_ok),
        "все запросы с верным токеном"
    );

    let started = Instant::now();
    srv.stop();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "остановка заняла {:?}",
        started.elapsed()
    );
    let log = srv.sink_text();
    assert!(log.contains("бот @cs2cinema_test_bot запущен"), "{log}");
    assert!(!log.contains(API_TOKEN), "токен в журнале сервера: {log}");
    assert!(!log.contains(&api.url()), "адрес bot-api в журнале: {log}");
}

#[test]
fn bot_failing_getme_stops_serve_with_a_clear_error() {
    let api = FakeBotApi::start(API_TOKEN);
    api.reply(
        "getMe",
        serde_json::json!({"ok": false, "error_code": 401, "description": "Unauthorized"}),
    );
    let dir = TestDir::new("e2e-getme");
    // Свободный порт: занять и сразу отпустить, потом проверить, что сервер его не удержал.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let cfg = server::config::Config {
        bind: format!("127.0.0.1:{port}"),
        data_dir: dir.path.join("data"),
        telegram_bot_token: Some(server::secret::Secret::new(API_TOKEN.to_owned())),
        bot_api_url: Some(api.url()),
        bot_api_files_root: Some(dir.path.join("bot-files")),
        ..Default::default()
    };
    let sink = std::sync::Arc::new(server::log::MemorySink::default());
    let err = match server::serve(
        &cfg,
        std::sync::Arc::new(server::clock::SystemClock),
        sink.clone(),
    ) {
        Ok(running) => {
            running.stop();
            panic!("сервер не должен был запуститься при отказе getMe");
        }
        Err(e) => e,
    };
    assert!(err.contains("бот не запустился"), "{err}");
    assert!(err.contains("getMe"), "{err}");
    assert!(
        !err.contains(API_TOKEN) && !err.contains(&api.url()),
        "в ошибке токен или адрес: {err}"
    );
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "порт должен остаться свободным"
    );
}

// ---------------------------------------------------------------------------------------------------------
// Отчёт о сбое на запущенном сервере с потоками бота (07-09)
// ---------------------------------------------------------------------------------------------------------

#[test]
fn failure_report_reaches_friend_and_admin_through_the_running_server() {
    const ADMIN: i64 = 900;
    let tg_dir = TestDir::new("e2e-fail-tg");
    let tg = FakeTelegram::new(tg_dir.path.clone());
    let mut srv = TestServer::start_with(TestOptions {
        tg: Some(tg.clone()),
        clock: Some(ManualClock::new(1_800_000_000_000)),
        ..Default::default()
    });
    {
        let core = srv.running().core();
        let now = core.clock().now_ms();
        server::users::add_user(core.db(), "вася", Some(FRIEND), false, now).unwrap();
        server::users::add_user(core.db(), "автор", Some(ADMIN), true, now).unwrap();
    }

    let bytes = common::fake_demo(4096);
    tg.put_document("file-f", "uniq-f", &bytes);
    let upd = tg.document_update(
        FRIEND,
        FRIEND,
        "file-f",
        "uniq-f",
        "match.dem",
        bytes.len() as u64,
        Some("s1mple"),
    );
    tg.push_update(upd);
    assert!(
        wait_until(10, || srv.running().core().jobs().unwrap().len() == 1),
        "задача не появилась; журнал: {}",
        srv.sink_text()
    );
    let job_id = srv.running().core().jobs().unwrap()[0].id;
    // Задача появляется в `Receiving`, пока демка копируется из bot-api; claim имеет смысл только после `Queued`.
    assert!(
        wait_until(10, || srv.job(job_id).state == JobState::Queued),
        "задача не дошла до очереди: {:?}; журнал: {}",
        srv.job(job_id).state,
        srv.sink_text()
    );

    // Фейковый раннер по HTTP берёт задачу и сообщает о сбое запуска игры.
    let token = srv.add_runner("e2e-fail-runner");
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
    let (status, body) = srv.finish(
        &token,
        job_id,
        &wire::FinishRequest {
            epoch: a.epoch,
            outcome: wire::Outcome::Failed(wire::Failure {
                code: wire::fail::GAME_NOT_STARTED.to_owned(),
                class: "launch".to_owned(),
                state: "launching".to_owned(),
                hint: "проверьте путь к CS2".to_owned(),
                message: "игра не запустилась за 120 с".to_owned(),
                tail: String::new(),
                elapsed_ms: 120_000,
            }),
        },
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(srv.job(job_id).state, JobState::Failed);

    // Без участия теста: другу приходит фраза, админу — отчёт с кодом.
    assert!(
        wait_until(10, || {
            tg.texts_to(FRIEND)
                .iter()
                .any(|t| t.contains(&format!("Задача #{job_id} не получилась")))
                && !tg.texts_to(ADMIN).is_empty()
        }),
        "отчёты не дошли; друг: {:?}; админ: {:?}; журнал: {}",
        tg.texts_to(FRIEND),
        tg.texts_to(ADMIN),
        srv.sink_text()
    );
    let friend = tg.texts_to(FRIEND);
    let msg = friend.iter().find(|t| t.contains("не получилась")).unwrap();
    assert!(msg.contains("не запустилась игра"), "{msg}");
    assert!(
        !msg.contains("GAME_NOT_STARTED") && msg.contains(&format!("/retry {job_id}")),
        "{msg}"
    );
    let admin = tg.texts_to(ADMIN);
    assert_eq!(admin.len(), 1, "{admin:?}");
    assert!(admin[0].contains("GAME_NOT_STARTED"), "{}", admin[0]);
    assert!(admin[0].contains("launching"), "{}", admin[0]);
    srv.stop();
}
