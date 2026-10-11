//! `HttpTg` против поддельного Bot API: форма запросов, разбор ответов, тайм-ауты, токен в ошибках.

mod common;

use common::FakeBotApi;
use serde_json::json;
use server::secret::{Redactor, Secret};
use server::tg::http::{HttpTg, HttpTimeouts, check_base_url};
use server::tg::{InputFile, Tg, TgError};
use std::time::{Duration, Instant};

const TOKEN: &str = "123456789:AAEabcdefghijklmnopqrstuvwxyz0123456789";

fn client(api_url: &str, timeouts: HttpTimeouts) -> HttpTg {
    let token = Secret::new(TOKEN.to_owned());
    HttpTg::new(api_url, &token, Redactor::new(&[&token]), timeouts).expect("клиент")
}

#[test]
fn http_client_speaks_bot_api_against_a_fake_server() {
    let api = FakeBotApi::start(TOKEN);
    let tg = client(&api.url(), HttpTimeouts::default());

    let me = tg.get_me().unwrap();
    assert_eq!(me.username.as_deref(), Some("cs2cinema_test_bot"));

    api.reply(
        "getUpdates",
        json!({"ok": true, "result": [{
            "update_id": 5,
            "message": {
                "message_id": 3, "chat": {"id": 42, "type": "private"},
                "from": {"id": 42, "is_bot": false, "first_name": "Вася"},
                "document": {"file_id": "f1", "file_unique_id": "u1", "file_name": "a.dem", "file_size": 1234}
            }
        }]}),
    );
    let updates = tg.get_updates(5, 1).unwrap();
    assert_eq!(updates.len(), 1);
    let doc = updates[0]
        .message
        .as_ref()
        .unwrap()
        .document
        .as_ref()
        .unwrap();
    assert_eq!(doc.file_size, Some(1234));
    let polls = api.requests_to("getUpdates");
    assert_eq!(polls[0].body["offset"], 5);
    assert_eq!(polls[0].body["timeout"], 1);
    assert_eq!(polls[0].body["allowed_updates"], json!(["message"]));

    api.reply(
        "sendMessage",
        json!({"ok": true, "result": {"message_id": 77}}),
    );
    assert_eq!(tg.send_message(42, "привет").unwrap(), 77);
    let sent = &api.requests_to("sendMessage")[0];
    assert_eq!(sent.body, json!({"chat_id": 42, "text": "привет"}));
    assert!(sent.body.get("parse_mode").is_none());

    api.reply(
        "sendVideo",
        json!({"ok": true, "result": {"message_id": 78, "video": {"file_id": "vid-1"}}}),
    );
    let video = tg
        .send_video(
            42,
            &InputFile::Local("file:///srv/r/7/highlights.mp4".into()),
            "Задача #7",
        )
        .unwrap();
    assert_eq!(
        (video.message_id, video.file_id.as_deref()),
        (78, Some("vid-1"))
    );
    let body = &api.requests_to("sendVideo")[0].body;
    assert_eq!(body["video"], "file:///srv/r/7/highlights.mp4");
    assert_eq!(body["supports_streaming"], true);
    assert_eq!(body["caption"], "Задача #7");
    assert!(body.get("parse_mode").is_none());

    api.reply(
        "sendDocument",
        json!({"ok": true, "result": {"message_id": 79, "document": {"file_id": "doc-1"}}}),
    );
    let doc = tg
        .send_document(42, &InputFile::Id("abc".into()), "подпись")
        .unwrap();
    assert_eq!(doc.file_id.as_deref(), Some("doc-1"));
    let body = &api.requests_to("sendDocument")[0].body;
    assert_eq!(body["document"], "abc");

    api.reply("editMessageText", json!({"ok": true, "result": true}));
    tg.edit_message_text(42, 77, "новый текст").unwrap();
    assert_eq!(
        api.requests_to("editMessageText")[0].body,
        json!({"chat_id": 42, "message_id": 77, "text": "новый текст"})
    );

    api.reply(
        "getFile",
        json!({"ok": true, "result": {"file_id": "f1", "file_size": 9, "file_path": "/var/lib/x/documents/file_0.dem"}}),
    );
    let file = tg.get_file("f1").unwrap();
    assert_eq!(
        file.file_path.as_deref(),
        Some("/var/lib/x/documents/file_0.dem")
    );

    tg.set_my_commands(&[("queue".into(), "очередь".into())])
        .unwrap();
    assert_eq!(
        api.requests_to("setMyCommands")[0].body,
        json!({"commands": [{"command": "queue", "description": "очередь"}]})
    );

    let all = api.requests();
    assert!(all.len() >= 8);
    assert!(
        all.iter().all(|r| r.token_ok),
        "токен должен быть в пути каждого запроса"
    );
}

#[test]
fn api_error_with_retry_after_is_parsed() {
    let api = FakeBotApi::start(TOKEN);
    let tg = client(&api.url(), HttpTimeouts::default());

    api.reply(
        "editMessageText",
        json!({"ok": false, "error_code": 429, "description": "Too Many Requests: retry after 7",
               "parameters": {"retry_after": 7}}),
    );
    let e = tg.edit_message_text(1, 2, "x").unwrap_err();
    assert!(
        matches!(
            e,
            TgError::Api {
                code: 429,
                retry_after: Some(7),
                ..
            }
        ),
        "{e:?}"
    );
    assert_eq!(e.retry_after_ms(), Some(7000));
    assert!(!e.maybe_delivered());

    api.reply(
        "editMessageText",
        json!({"ok": false, "error_code": 400,
               "description": "Bad Request: message is not modified: specified new message content"}),
    );
    assert!(
        tg.edit_message_text(1, 2, "x")
            .unwrap_err()
            .is_not_modified()
    );

    api.reply(
        "editMessageText",
        json!({"ok": false, "error_code": 400, "description": "Bad Request: message to edit not found"}),
    );
    assert!(
        tg.edit_message_text(1, 2, "x")
            .unwrap_err()
            .is_message_missing()
    );

    api.reply(
        "sendMessage",
        json!({"ok": false, "error_code": 403, "description": "Forbidden: bot was blocked by the user"}),
    );
    assert!(tg.send_message(1, "x").unwrap_err().is_forbidden());
}

#[test]
fn silent_server_is_a_timeout_after_the_request_was_sent() {
    let api = FakeBotApi::start(TOKEN);
    let tg = client(
        &api.url(),
        HttpTimeouts {
            transfer: Duration::from_secs(1),
            ..Default::default()
        },
    );
    api.silence("sendVideo");
    let t = Instant::now();
    let e = tg
        .send_video(
            42,
            &InputFile::Local("file:///srv/r/7/highlights.mp4".into()),
            "x",
        )
        .unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(e, TgError::Timeout { sent: true });
    assert!(e.maybe_delivered());
    assert_eq!(
        api.requests_to("sendVideo").len(),
        1,
        "запрос должен был дойти"
    );
}

#[test]
fn silent_poll_times_out_after_the_poll_timeout_plus_extra() {
    let api = FakeBotApi::start(TOKEN);
    let tg = client(
        &api.url(),
        HttpTimeouts {
            poll_extra: Duration::from_secs(1),
            ..Default::default()
        },
    );
    api.silence("getUpdates");
    let t = Instant::now();
    let e = tg.get_updates(0, 1).unwrap_err();
    let spent = t.elapsed();
    assert_eq!(e, TgError::Timeout { sent: true });
    assert!(
        spent >= Duration::from_millis(1800) && spent < Duration::from_secs(6),
        "{spent:?}"
    );
}

#[test]
fn closed_port_error_has_no_token_in_any_form() {
    // Порт, который только что был свободен и теперь закрыт.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let tg = client(&format!("http://127.0.0.1:{port}"), HttpTimeouts::default());
    let e = tg.send_message(42, "x").unwrap_err();
    assert!(
        matches!(
            e,
            TgError::Io { sent: false, .. } | TgError::Timeout { sent: false }
        ),
        "{e:?}"
    );
    assert!(!e.maybe_delivered());
    let secret_part = TOKEN.split_once(':').unwrap().1;
    for text in [format!("{e}"), format!("{e:?}")] {
        assert!(!text.contains(TOKEN), "токен с двоеточием в тексте ошибки");
        assert!(
            !text.contains(&TOKEN.replace(':', "~")),
            "токен с тильдой в тексте ошибки"
        );
        assert!(
            !text.contains(secret_part),
            "секретная часть токена в тексте ошибки"
        );
    }
}

#[test]
fn non_loopback_or_cloud_url_is_refused() {
    for bad in [
        "https://api.telegram.org",
        "http://api.telegram.org",
        "http://10.0.0.5:8081",
        "http://127.0.0.1",
        "http://user@127.0.0.1:8081",
        "ftp://127.0.0.1:8081",
        "http://127.0.0.1:0",
        "http://127.0.0.1:8081/bot",
        "http://127.0.0.1:80x",
        "http://[::1]",
        "",
    ] {
        let err = check_base_url(bad).expect_err(bad);
        assert!(
            bad.is_empty() || !err.contains(bad),
            "адрес повторён в ошибке: {err}"
        );
        let token = Secret::new(TOKEN.to_owned());
        assert!(
            HttpTg::new(bad, &token, Redactor::default(), HttpTimeouts::default()).is_err(),
            "{bad}"
        );
    }
    for good in [
        "http://127.0.0.1:8081",
        "http://localhost:8081/",
        "http://[::1]:8081",
    ] {
        check_base_url(good).expect(good);
    }
}

#[test]
fn unexpected_replies_are_classified_not_panicked_on() {
    let api = FakeBotApi::start(TOKEN);
    let tg = client(&api.url(), HttpTimeouts::default());
    // Нет сценарного ответа на sendMessage: поддельный сервер отвечает 404 и JSON с ok:false.
    let e = tg.send_message(1, "x").unwrap_err();
    assert!(matches!(e, TgError::Api { code: 404, .. }), "{e:?}");
    // ok:true без result — запрос считается ушедшим.
    api.reply("sendMessage", json!({"ok": true}));
    let e = tg.send_message(1, "x").unwrap_err();
    assert!(e.maybe_delivered(), "{e:?}");
}
