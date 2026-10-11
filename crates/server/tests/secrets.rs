//! SEC-04: токены не утекают из сервера. Подставные токены раннера и бота после принудительных сбоев не должны
//! находиться ни в журнале, ни в байтах базы, ни в ответах API.
//!
//! Правило против холостого теста (Pitfall 10): сначала доказывается, что подставной токен БЫЛ во входе
//! (в отправленных запросах), и только потом — что его нет на выходе. Сама проверка `assert_absent` падает
//! на буфере, где токен есть, и это тоже проверено отдельным тестом.

mod common;

use common::{Resp, TestOptions, TestServer, fake_demo};
use server::core::Submit;
use server::log::{Log, MemorySink};
use server::secret::{Redactor, Secret};
use std::sync::{Arc, Mutex};
use wire::{Failure, FinishReply, FinishRequest, JobState, Outcome, Phase, ProgressRequest, codes};

/// Паникует, если `needle` найден в `haystack`. Сообщение называет место, но не печатает соседний текст:
/// иначе провал теста сам выдал бы секрет в вывод `cargo test`.
fn assert_absent(haystack: &[u8], needle: &str, place: &str) {
    assert!(!needle.is_empty(), "пустая игла проверяла бы всё подряд");
    let found = haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes());
    assert!(!found, "подставной секрет найден: {place}");
}

fn progress(epoch: i64, seq: u64, phase: Phase) -> ProgressRequest {
    ProgressRequest {
        epoch,
        seq,
        phase,
        take: None,
        status: String::new(),
    }
}

#[test]
fn runner_token_in_a_failure_report_stays_out_of_logs_db_and_responses() {
    let mut srv = TestServer::start();
    let token = srv.add_runner("alpha");
    let id = srv.submit(&fake_demo(4096), "s1mple");

    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let epoch = got.expect("выдача").epoch;
    let (st, body) = srv.progress(&token, id, &progress(epoch, 1, Phase::Record));
    assert_eq!(st, 200, "{body}");

    let failed = FinishRequest {
        epoch,
        outcome: Outcome::Failed(Failure {
            code: "HLAE_ERROR".to_owned(),
            class: "окружение".to_owned(),
            state: "take".to_owned(),
            hint: "проверьте HLAE".to_owned(),
            message: format!("boom {token}"),
            tail: format!("строка\nAuthorization: Bearer {token}"),
            elapsed_ms: 1234,
        }),
    };
    let sent = serde_json::to_string(&failed).unwrap();
    // Против холостого теста: токен действительно ушёл серверу в теле запроса.
    assert!(sent.contains(&token), "токен должен быть во входе");

    let mut responses = String::new();
    let (st, body) = srv.finish(&token, id, &failed);
    assert_eq!(st, 200, "{body}");
    responses.push_str(&body);
    let reply: FinishReply = serde_json::from_str(&body).unwrap();
    assert_eq!(reply.state, JobState::Failed);

    let job = srv.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("HLAE_ERROR"));
    assert_eq!(job.fail_state.as_deref(), Some("take"));
    let message = job.fail_message.clone().unwrap();
    assert!(
        message.contains("boom") && message.contains("cs2r_***"),
        "{message}"
    );
    assert_eq!(job.fail_elapsed_ms, Some(1234));
    assert!(job.finished_at.is_some());
    assert!(job.lease_until.is_none());

    // Повтор того же итога идемпотентен, другой итог после конца — отказ.
    let (st, body) = srv.finish(&token, id, &failed);
    assert_eq!(st, 200, "{body}");
    responses.push_str(&body);
    let cancelled = FinishRequest {
        epoch,
        outcome: Outcome::Cancelled,
    };
    let (st, body) = srv.finish(&token, id, &cancelled);
    assert_eq!(st, 409, "{body}");
    assert!(body.contains(codes::JOB_TERMINAL), "{body}");
    responses.push_str(&body);

    assert_absent(responses.as_bytes(), &token, "тела ответов API");
    assert_absent(srv.sink_text().as_bytes(), &token, "журнал сервера");
    assert_absent(&srv.db_bytes(), &token, "байты server.db и -wal");
    srv.stop();
    assert_absent(&srv.db_bytes(), &token, "байты server.db после остановки");
}

// ---------------------------------------------------------------------------------------------------------
// Принудительные сбои, паника, настройки и проверка самой проверки (SEC-04, План 05-04, Task 3)
// ---------------------------------------------------------------------------------------------------------

/// Токен бота Telegram правильной формы: цифры, двоеточие, 37 символов секрета.
const BOT: &str = "123456789:AAEsentinelSENTINELsentinelSENTINEL01";

/// Паника в одном тесте не должна попасть в журнал другого: тесты, меняющие глобальный обработчик паник
/// или паникующие нарочно, берут эту блокировку.
static PANIC_LOCK: Mutex<()> = Mutex::new(());

/// Запросы к стенду с учётом всего, что ушло серверу (`sent`) и что вернулось (`received`: тела и заголовки).
struct Tap<'a> {
    srv: &'a TestServer,
    sent: String,
    received: Vec<u8>,
}

impl<'a> Tap<'a> {
    fn new(srv: &'a TestServer) -> Self {
        Self {
            srv,
            sent: String::new(),
            received: Vec::new(),
        }
    }

    fn call(&mut self, path: &str, authorization: Option<&str>, body: Option<&str>) -> Resp {
        self.sent.push_str(path);
        self.sent.push('\n');
        if let Some(a) = authorization {
            self.sent.push_str(a);
            self.sent.push('\n');
        }
        if let Some(b) = body {
            self.sent.push_str(b);
            self.sent.push('\n');
        }
        let r = self.srv.send(path, authorization, body);
        self.received.extend_from_slice(r.body.as_bytes());
        for (name, value) in &r.headers {
            self.received
                .extend_from_slice(format!("\n{name}: {value}").as_bytes());
        }
        r
    }

    fn claim(&mut self, authorization: &str) -> Resp {
        let path = format!("{}?wait=0", wire::paths::CLAIM);
        self.call(&path, Some(authorization), Some(r#"{"status":"idle"}"#))
    }
}

#[test]
fn forced_failures_leave_no_token_anywhere() {
    let mut srv = TestServer::start_with(TestOptions {
        bot_token: Some(BOT.to_owned()),
        ..Default::default()
    });
    // T — токен раннера, R — отозванный, U — правильной формы, но неизвестный, B — токен бота.
    let t = srv.add_runner("alpha");
    let r = srv.add_runner("old");
    {
        let core = srv.running().core();
        server::auth::revoke_runner(core.db(), "old", core.clock().now_ms()).unwrap();
    }
    let u = server::auth::issue_token().unwrap();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let mut tap = Tap::new(&srv);

    // Чужой, отозванный и кривой токен; не тот способ входа.
    for bad in [
        format!("Bearer {u}"),
        format!("Bearer {r}"),
        format!("Bearer {BOT}"),
        format!("Basic {t}"),
    ] {
        let resp = tap.claim(&bad);
        assert_eq!(resp.status, 401, "{}", resp.body);
    }

    let good = format!("Bearer {t}");
    let resp = tap.claim(&good);
    assert_eq!(resp.status, 200, "{}", resp.body);

    // Неверный JSON с токеном внутри: ответ называет место, а не значение.
    let progress_path = wire::paths::job(id, "progress");
    let resp = tap.call(
        &progress_path,
        Some(&good),
        Some(&format!(r#"{{"epoch":"{t}","seq":1,"phase":"record"}}"#)),
    );
    assert_eq!(resp.status, 400, "{}", resp.body);
    // Устаревшая эпоха и токены в статусе.
    let resp = tap.call(
        &progress_path,
        Some(&good),
        Some(&format!(
            r#"{{"epoch":99,"seq":1,"phase":"record","status":"{t} {BOT}"}}"#
        )),
    );
    assert_eq!(resp.status, 409, "{}", resp.body);
    let resp = tap.call(
        &progress_path,
        Some(&good),
        Some(&format!(
            r#"{{"epoch":1,"seq":1,"phase":"record","status":"{BOT}"}}"#
        )),
    );
    assert_eq!(resp.status, 200, "{}", resp.body);

    // hello: токены в имени, статусе и версии.
    let hello = serde_json::json!({
        "protocol": wire::PROTOCOL_VERSION,
        "name": t,
        "status": BOT,
        "app_version": u,
        "holding": [{"job_id": id, "epoch": 1}],
    });
    let resp = tap.call(wire::paths::HELLO, Some(&good), Some(&hello.to_string()));
    assert_eq!(resp.status, 200, "{}", resp.body);

    // release с токенами в причине, затем новая выдача.
    let resp = tap.call(
        &wire::paths::job(id, "release"),
        Some(&good),
        Some(&serde_json::json!({"epoch": 1, "reason": format!("{t} {BOT}")}).to_string()),
    );
    assert_eq!(resp.status, 204, "{}", resp.body);
    let resp = tap.claim(&good);
    assert_eq!(resp.status, 200, "{}", resp.body);
    let epoch = serde_json::from_str::<wire::Assignment>(&resp.body)
        .unwrap()
        .epoch;

    // finish: токены в сообщении, подсказке, состоянии и хвосте.
    let failed = serde_json::json!({
        "epoch": epoch,
        "outcome": {
            "kind": "failed",
            "code": "HLAE_ERROR",
            "state": r,
            "hint": BOT,
            "message": format!("boom {t} {BOT}"),
            "tail": format!("Bearer {t}\n{u}"),
            "elapsed_ms": 7,
        }
    });
    let resp = tap.call(
        &wire::paths::job(id, "finish"),
        Some(&good),
        Some(&failed.to_string()),
    );
    assert_eq!(resp.status, 200, "{}", resp.body);
    assert_eq!(srv.job(id).state, JobState::Failed);

    // Имя файла демки, похожее на токен, и игрок с токеном.
    let src = srv.dir.path.join("named.dem");
    std::fs::write(&src, fake_demo(2048)).unwrap();
    let orig_name = format!("{u}.dem");
    tap.sent.push_str(&orig_name);
    srv.running()
        .core()
        .submit(
            &src,
            &Submit {
                player: t.clone(),
                requested_by: BOT.to_owned(),
                orig_name,
                ..Default::default()
            },
        )
        .unwrap();

    // Против холостого теста: каждый подставной токен действительно был во входе.
    let secrets = [
        ("токен раннера", &t),
        ("отозванный токен", &r),
        ("неизвестный токен", &u),
    ];
    for (name, value) in secrets {
        assert!(tap.sent.contains(value.as_str()), "{name} не был отправлен");
    }
    assert!(tap.sent.contains(BOT), "токен бота не был отправлен");

    let received = tap.received.clone();
    let sink = srv.sink_text();
    let db_before = srv.db_bytes();
    srv.stop();
    let db_after = srv.db_bytes();
    let all = [
        ("токен раннера", t.as_str()),
        ("отозванный токен", r.as_str()),
        ("неизвестный токен", u.as_str()),
        ("токен бота", BOT),
    ];
    for (name, value) in all {
        assert_absent(
            &received,
            value,
            &format!("{name}: тела и заголовки ответов API"),
        );
        assert_absent(sink.as_bytes(), value, &format!("{name}: журнал сервера"));
        assert_absent(
            &db_before,
            value,
            &format!("{name}: байты server.db и -wal"),
        );
        assert_absent(
            &db_after,
            value,
            &format!("{name}: байты server.db после остановки"),
        );
    }
    // Журнал не пуст: проверка не прошла бы вхолостую на пустом приёмнике.
    assert!(
        sink.contains("вход отклонён"),
        "в журнале нет отказов входа"
    );
    assert!(sink.contains("задача #"), "в журнале нет строк о задаче");
}

#[test]
fn panic_message_with_a_token_is_redacted() {
    let _guard = PANIC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = format!("cs2r_{}", "ef56".repeat(16));
    let sink = Arc::new(MemorySink::default());
    let bot = Secret::new(BOT.to_owned());
    let log = Log::new(sink.clone(), Redactor::new(&[&bot]));
    let previous = std::panic::take_hook();
    server::log::install_panic_hook(log.clone());

    let message = format!("{BOT} {t}");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let handler: server::http::Handler = {
        let message = message.clone();
        Arc::new(move |_rq: tiny_http::Request| panic!("{}", message))
    };
    let http = server::http::start(listener, 2, log, handler).unwrap();
    let resp = common::send(&format!("http://{}/x", http.addr()), None, Some("{}"));
    http.stop();
    // Вернуть обычный обработчик паник до любых проверок, чтобы их провал был виден.
    let _ = std::panic::take_hook();
    std::panic::set_hook(previous);

    assert_eq!(resp.status, 500, "{}", resp.body);
    let text = sink.lines().join("\n");
    assert!(
        text.contains("обработчик упал"),
        "в журнале нет записи об упавшем обработчике"
    );
    assert!(
        text.contains("паника:"),
        "обработчик паник не записал строку"
    );
    assert!(text.contains("***"), "секреты должны быть замаскированы");
    assert!(
        message.contains(BOT) && message.contains(&t),
        "в панику ушли оба токена"
    );
    assert_absent(text.as_bytes(), BOT, "токен бота в журнале паник");
    assert_absent(text.as_bytes(), &t, "токен раннера в журнале паник");
    let mut body = resp.body.into_bytes();
    for (k, v) in &resp.headers {
        body.extend_from_slice(format!("{k}: {v}").as_bytes());
    }
    assert_absent(&body, BOT, "ответ на упавший обработчик");
    assert_absent(&body, &t, "ответ на упавший обработчик");
}

fn cs2_server() -> std::process::Command {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_cs2-server"));
    for k in [
        "CS2_SERVER_CONFIG",
        "CS2_SERVER_BIND",
        "CS2_SERVER_DATA_DIR",
        "CS2_SERVER_THREADS",
        "CS2_SERVER_LEASE_TTL_SECS",
        "CS2_SERVER_HEARTBEAT_SECS",
        "CS2_SERVER_LONGPOLL_SECS",
        "CS2_SERVER_MAX_GRANTS",
        "CS2_SERVER_STARTUP_GRACE_SECS",
        "CS2_SERVER_MAX_DEMO_MIB",
        "CS2_SERVER_MAX_RESULT_MIB",
        "CS2_SERVER_TELEGRAM_BOT_TOKEN",
        "TELEGRAM_BOT_TOKEN",
        "STATE_DIRECTORY",
    ] {
        c.env_remove(k);
    }
    c
}

#[test]
fn bot_token_never_leaves_config_errors_or_config_show() {
    let dir = common::TestDir::new("secrets-cfg");
    let data = dir.path.join("data");
    let data = data.to_str().unwrap();

    // Токен в окружении, ошибка в файле настроек: сообщение называет место, а не значения.
    let bad_file = dir.path.join("bad.json");
    std::fs::write(&bad_file, format!(r#"{{"threads": "{BOT}"}}"#)).unwrap();
    let out = cs2_server()
        .args([
            "--config",
            bad_file.to_str().unwrap(),
            "config",
            "show",
            "--data-dir",
            data,
        ])
        .env("TELEGRAM_BOT_TOKEN", BOT)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "ожидалась ошибка настроек");
    assert_absent(&out.stderr, BOT, "stderr при ошибке в файле настроек");
    assert_absent(&out.stdout, BOT, "stdout при ошибке в файле настроек");
    assert!(!out.stderr.is_empty(), "ошибка должна быть напечатана");

    // Токен в файле, ошибка в окружении.
    let token_file = dir.path.join("token.json");
    std::fs::write(&token_file, format!(r#"{{"telegram_bot_token": "{BOT}"}}"#)).unwrap();
    let out = cs2_server()
        .args([
            "--config",
            token_file.to_str().unwrap(),
            "config",
            "show",
            "--data-dir",
            data,
        ])
        .env("CS2_SERVER_THREADS", format!("плохо {BOT}"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_absent(&out.stderr, BOT, "stderr при ошибке в окружении");
    assert_absent(&out.stdout, BOT, "stdout при ошибке в окружении");

    // Верные настройки: токен прочитан (так видно, что он действительно был на входе) и показан как `***`.
    for via_env in [true, false] {
        let mut cmd = cs2_server();
        if via_env {
            cmd.env("TELEGRAM_BOT_TOKEN", BOT)
                .args(["config", "show", "--data-dir", data]);
        } else {
            cmd.args([
                "--config",
                token_file.to_str().unwrap(),
                "config",
                "show",
                "--data-dir",
                data,
            ]);
        }
        let out = cmd.output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("telegram_bot_token = ***"),
            "токен должен быть прочитан и скрыт: {stdout}"
        );
        assert_absent(&out.stdout, BOT, "stdout config show");
        assert_absent(&out.stderr, BOT, "stderr config show");
    }
}

#[test]
fn the_check_itself_catches_a_leak() {
    let _guard = PANIC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let token = format!("cs2r_{}", "01ab".repeat(16));
    let leaky = format!("до {token} после");
    let clean = "здесь нет секретов";

    assert_absent(clean.as_bytes(), &token, "чистый буфер");
    let caught =
        std::panic::catch_unwind(|| assert_absent(leaky.as_bytes(), &token, "буфер с утечкой"));
    let payload = caught.expect_err("проверка обязана упасть на буфере с токеном");
    let text = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default();
    assert!(
        text.contains("буфер с утечкой"),
        "сообщение должно называть место: {text}"
    );
    assert!(
        !text.contains(&token) && !text.contains("после"),
        "сообщение не должно печатать соседний текст"
    );

    // Пустая игла тоже отвергается, а не «находится» везде.
    assert!(std::panic::catch_unwind(|| assert_absent(b"x", "", "пустая игла")).is_err());
}

// ---------------------------------------------------------------------------------------------------------
// Отказы при передаче кусков (SEC-04, План 05-05, Task 2)
// ---------------------------------------------------------------------------------------------------------

#[test]
fn chunk_rejections_do_not_leak_the_token() {
    use common::{raw_request, read_reply, result_bytes};
    use std::io::Write;

    let mut srv = TestServer::start_with(TestOptions {
        bot_token: Some(BOT.to_owned()),
        ..Default::default()
    });
    let t = srv.add_runner("alpha");
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let (status, got) = srv.claim(&t, 0);
    assert_eq!(status, 200);
    let epoch = got.expect("выдача").epoch;
    let (st, body) = srv.progress(&t, id, &progress(epoch, 1, Phase::Record));
    assert_eq!(st, 200, "{body}");

    // Токены раннера и бота кладутся и в Authorization, и в query: путь в журнал идёт без query.
    let head = |len: u64, offset: u64| {
        format!(
            "PUT /v1/jobs/{id}/result?token={t}&bot={BOT} HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {t}\r\nX-Epoch: {epoch}\r\nUpload-Offset: {offset}\r\nContent-Length: {len}\r\n\r\n"
        )
    };
    let mut sent = String::new();
    let mut received: Vec<u8> = Vec::new();
    let mut keep = |r: &common::RawReply| {
        received.extend_from_slice(r.status_line.as_bytes());
        for (k, v) in &r.headers {
            received.extend_from_slice(format!("\n{k}: {v}").as_bytes());
        }
        received.extend_from_slice(&r.body);
    };

    // 1. Слишком большой кусок: отказ до тела.
    let too_big = head(wire::CHUNK_SIZE + 1, 0);
    sent.push_str(&too_big);
    let mut conn = raw_request(srv.addr());
    conn.write_all(too_big.as_bytes()).unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 413, "{}", reply.status_line);
    keep(&reply);
    drop(conn);

    // 2. Оборванный кусок: заявлено 1 МиБ, прислано полмебибайта.
    let data = result_bytes(1024 * 1024);
    let aborted = head(data.len() as u64, 0);
    sent.push_str(&aborted);
    let mut conn = raw_request(srv.addr());
    conn.write_all(aborted.as_bytes()).unwrap();
    conn.write_all(&data[..data.len() / 2]).unwrap();
    conn.shutdown(std::net::Shutdown::Write).unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 400, "{}", reply.status_line);
    keep(&reply);
    assert!(
        srv.sink_text().contains("оборван"),
        "обрыв дошёл до обработчика"
    );

    // 3. Кусок с чужим смещением.
    let wrong = head(1024, 777);
    sent.push_str(&wrong);
    let mut conn = raw_request(srv.addr());
    conn.write_all(wrong.as_bytes()).unwrap();
    conn.write_all(&data[..1024]).unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 409, "{}", reply.status_line);
    keep(&reply);
    drop(conn);

    // Против холостого теста: оба токена действительно были во входе.
    assert!(sent.contains(&t) && sent.contains(BOT));
    assert_absent(&received, &t, "ответы на отказанные куски (токен раннера)");
    assert_absent(&received, BOT, "ответы на отказанные куски (токен бота)");
    for secret in [t.as_str(), BOT] {
        assert_absent(srv.sink_text().as_bytes(), secret, "журнал сервера");
        assert_absent(&srv.db_bytes(), secret, "байты server.db и -wal");
    }
    srv.stop();
    for secret in [t.as_str(), BOT] {
        assert_absent(&srv.db_bytes(), secret, "байты server.db после остановки");
    }
}
