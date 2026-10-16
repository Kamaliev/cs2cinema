//! Подключение раннера через Telegram: заявка без токена (`POST /v1/runner/pair`), код у админа,
//! токен принимается только после `/runner код`. Telegram подставной, сервер и база настоящие.

mod common;

use common::{BotStand, Resp};
use server::auth;
use std::time::Duration;
use wire::{HelloRequest, PairReply};

const ADMIN: i64 = 1;
const FRIEND: i64 = 2;

fn say(st: &BotStand, from: i64, text: &str) {
    st.tg.push_update(st.tg.text_update(from, from, text));
}

fn last_text(st: &BotStand, chat: i64) -> String {
    st.tg
        .texts_to(chat)
        .pop()
        .unwrap_or_else(|| "<ничего не пришло>".to_owned())
}

/// Код ответа `hello` с токеном: 200 — сервер знает раннера, 401 — нет.
fn hello_status(st: &BotStand, token: &str) -> u16 {
    let req = HelloRequest {
        protocol: wire::PROTOCOL_VERSION,
        name: "my-pc".to_owned(),
        app_version: "test".to_owned(),
        status: String::new(),
        holding: Vec::new(),
    };
    st.server.hello(token, &req).0
}

/// Заявка приложения: серверу уходит только хеш токена.
fn pair(st: &BotStand, name: &str, token: &str) -> Resp {
    let body =
        serde_json::json!({ "name": name, "token_hash": auth::hash_token(token) }).to_string();
    st.server.send(wire::paths::PAIR, None, Some(&body))
}

#[test]
fn admin_connects_a_runner_by_code_from_the_chat() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "друг");
    let token = auth::issue_token().unwrap();

    let r = pair(&st, "my-pc", &token);
    assert_eq!(r.status, 200, "{}", r.body);
    let reply: PairReply = serde_json::from_str(&r.body).unwrap();
    assert_eq!(reply.code.len(), 6, "{}", reply.code);
    assert!(reply.code.bytes().all(|b| b.is_ascii_digit()));
    assert_eq!(reply.expires_in_secs, 600);
    assert!(
        !r.body.contains(&token),
        "токен в ответе сервера не появляется"
    );
    assert_eq!(
        hello_status(&st, &token),
        401,
        "до одобрения токен не принимается"
    );

    // Друг (не админ) одобрить не может.
    say(&st, FRIEND, &format!("/runner {}", reply.code));
    st.step();
    assert!(
        last_text(&st, FRIEND).contains("только для администратора"),
        "{}",
        last_text(&st, FRIEND)
    );
    assert_eq!(hello_status(&st, &token), 401);

    // Без кода — формат; чужой код — отказ.
    say(&st, ADMIN, "/runner");
    st.step();
    assert!(
        last_text(&st, ADMIN).starts_with("Формат:"),
        "{}",
        last_text(&st, ADMIN)
    );
    let wrong = if reply.code == "000000" {
        "000001"
    } else {
        "000000"
    };
    say(&st, ADMIN, &format!("/runner {wrong}"));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("нет или код устарел"),
        "{}",
        last_text(&st, ADMIN)
    );
    assert_eq!(hello_status(&st, &token), 401);

    // Одобрение: раннер создан, токен принят.
    say(&st, ADMIN, &format!("/runner {}", reply.code));
    st.step();
    let ok = last_text(&st, ADMIN);
    assert!(ok.contains("Раннер «my-pc» подключён"), "{ok}");
    assert_eq!(
        hello_status(&st, &token),
        200,
        "после одобрения токен принимается"
    );
    let core = st.server.running().core();
    let runners = auth::list_runners(core.db()).unwrap();
    assert_eq!(runners.len(), 1);
    assert_eq!(runners[0].name, "my-pc");
    assert!(
        !st.server.sink_text().contains(&token),
        "токен не попадает в журнал сервера"
    );

    // Повторное одобрение того же кода — заявки больше нет.
    say(&st, ADMIN, &format!("/runner {}", reply.code));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("Не подключил"),
        "{}",
        last_text(&st, ADMIN)
    );

    // Список и отзыв.
    say(&st, ADMIN, "/runners");
    st.step();
    let list = last_text(&st, ADMIN);
    assert!(
        list.contains("my-pc") && list.contains("отпечаток") && list.contains("на связи"),
        "{list}"
    );
    say(&st, ADMIN, "/runner revoke my-pc");
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("отозван"),
        "{}",
        last_text(&st, ADMIN)
    );
    assert_eq!(
        hello_status(&st, &token),
        401,
        "отозванный токен не принимается"
    );
    say(&st, ADMIN, "/runners");
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("отозван"),
        "{}",
        last_text(&st, ADMIN)
    );

    // Кривая заявка — 400 без следа в базе.
    let bad = st.server.send(
        wire::paths::PAIR,
        None,
        Some(r#"{"name":"x","token_hash":"zz"}"#),
    );
    assert_eq!(bad.status, 400, "{}", bad.body);
    assert_eq!(bad.error_code(), wire::codes::BAD_REQUEST);
}

#[test]
fn pairing_code_expires_and_a_busy_name_can_be_renamed_by_the_admin() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.runner_token(); // раннер «bot-runner» уже есть

    let token = auth::issue_token().unwrap();
    let r = pair(&st, "bot-runner", &token);
    assert_eq!(r.status, 200, "{}", r.body);
    let code = serde_json::from_str::<PairReply>(&r.body).unwrap().code;

    // Имя занято: заявка ждёт, админ называет другое.
    say(&st, ADMIN, &format!("/runner {code}"));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("уже есть"),
        "{}",
        last_text(&st, ADMIN)
    );
    assert_eq!(hello_status(&st, &token), 401);

    // Срок вышел — код не действует.
    st.clock().advance(Duration::from_secs(11 * 60));
    say(&st, ADMIN, &format!("/runner {code} second"));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("устарел"),
        "{}",
        last_text(&st, ADMIN)
    );
    assert_eq!(hello_status(&st, &token), 401);

    // Новая заявка тем же токеном и одобрение под своим именем.
    let r = pair(&st, "bot-runner", &token);
    assert_eq!(r.status, 200, "{}", r.body);
    let code = serde_json::from_str::<PairReply>(&r.body).unwrap().code;
    say(&st, ADMIN, &format!("/runner {code} second"));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("Раннер «second» подключён"),
        "{}",
        last_text(&st, ADMIN)
    );
    assert_eq!(hello_status(&st, &token), 200);
}
