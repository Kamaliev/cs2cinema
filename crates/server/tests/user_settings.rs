//! Настройки монтажа пользователя: экран Mini App за подписью Telegram, команда `/settings`, снимок настроек в задаче
//! и выдача их раннеру. Telegram подставной, сервер и база настоящие, время идёт по ручным часам.

mod common;

use common::{BOT_T0, TestOptions, TestServer, fake_demo};
use serde_json::{Value, json};
use server::bot::{Bot, BotSettings};
use server::clock::ManualClock;
use server::core::Submit;
use server::tg::fake::FakeTelegram;
use server::webapp::sign_init_data;
use std::sync::Arc;

const TOKEN: &str = "123456:TEST-bot-token-for-the-miniapp-checks";
const FRIEND: i64 = 4242;

struct Stand {
    server: TestServer,
    tg: FakeTelegram,
    bot: Bot,
    user_id: i64,
    _tg_dir: common::TestDir,
}

fn stand(public_url: Option<&str>) -> Stand {
    let server = TestServer::start_with(TestOptions {
        clock: Some(ManualClock::new(BOT_T0)),
        bot_token: Some(TOKEN.to_owned()),
        ..Default::default()
    });
    let tg_dir = common::TestDir::new("tg");
    let tg = FakeTelegram::new(tg_dir.path.clone());
    let mut settings = BotSettings::new(&server.dir.path.join("data"), tg.files_dir());
    settings.public_url = public_url.map(str::to_owned);
    let core = server.running().core().clone();
    let bot = Bot::new(core.clone(), Arc::new(tg.clone()), server.running().log().clone(), settings);
    let user_id = server::users::add_user(core.db(), "петя", Some(FRIEND), false, core.clock().now_ms()).unwrap().id;
    Stand { server, tg, bot, user_id, _tg_dir: tg_dir }
}

fn call(st: &Stand, method: &str, auth: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let agent = common::agent();
    let url = st.server.url("/app/api/settings");
    let result = match (method, body) {
        ("PUT", body) => {
            let mut rq = agent.put(&url);
            if let Some(a) = auth {
                rq = rq.header("Authorization", a);
            }
            rq.header("Content-Type", "application/json").send(body.unwrap_or(Value::Null).to_string())
        }
        _ => {
            let mut rq = agent.get(&url);
            if let Some(a) = auth {
                rq = rq.header("Authorization", a);
            }
            rq.call()
        }
    };
    let mut resp = result.expect("запрос");
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn init_for(tg_id: i64) -> String {
    format!("tma {}", sign_init_data(TOKEN, tg_id, BOT_T0 / 1000))
}

#[test]
fn miniapp_serves_the_page_and_the_values_only_to_a_signed_friend() {
    let st = stand(None);

    let page = st.server.send("/app/settings", None, None);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Настройки ролика") && page.body.contains("/app/api/settings"));
    assert!(
        page.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.starts_with("text/html")),
        "страница отдаётся как html: {:?}",
        page.headers
    );

    assert_eq!(call(&st, "GET", None, None).0, 401, "без подписи");
    assert_eq!(call(&st, "GET", Some("tma garbage"), None).0, 401, "подпись не подошла");
    assert_eq!(call(&st, "GET", Some("Bearer x"), None).0, 401, "не та схема");
    assert_eq!(call(&st, "GET", Some(&init_for(999)), None).0, 401, "подпись верна, но не друг");
    let forged = format!("tma {}", sign_init_data("1:чужой-токен", FRIEND, BOT_T0 / 1000));
    assert_eq!(call(&st, "GET", Some(&forged), None).0, 401, "подпись чужим токеном");

    let (status, body) = call(&st, "GET", Some(&init_for(FRIEND)), None);
    assert_eq!(status, 200);
    assert_eq!(body["values"]["flybys"], json!(true), "умолчания реестра");
    assert_eq!(body["values"]["fps"], json!(60));
    assert!(body["schema"]["groups"].as_array().is_some_and(|g| !g.is_empty()));
}

#[test]
fn changes_are_saved_per_user_and_bad_ones_are_refused() {
    let st = stand(None);
    server::users::add_user(st.server.running().core().db(), "вася", Some(7), false, BOT_T0).unwrap();
    let auth = init_for(FRIEND);

    let (status, body) = call(&st, "PUT", Some(&auth), Some(json!({"detailed": true, "fps": 30})));
    assert_eq!(status, 200);
    assert_eq!((body["values"]["detailed"].clone(), body["values"]["fps"].clone()), (json!(true), json!(30)));

    // другой друг своих значений не менял
    let (_, theirs) = call(&st, "GET", Some(&init_for(7)), None);
    assert_eq!((theirs["values"]["detailed"].clone(), theirs["values"]["fps"].clone()), (json!(false), json!(60)));

    for bad in [json!({"fps": 45}), json!({"нет такой": true}), json!({"detailed": "да"}), json!([1])] {
        let (status, _) = call(&st, "PUT", Some(&auth), Some(bad.clone()));
        assert_eq!(status, 400, "{bad}");
    }
    let (_, kept) = call(&st, "GET", Some(&auth), None);
    assert_eq!(kept["values"]["fps"], json!(30), "отказ ничего не испортил");

    // null сбрасывает к умолчанию
    let (_, reset) = call(&st, "PUT", Some(&auth), Some(json!({"fps": null})));
    assert_eq!(reset["values"]["fps"], json!(60));
    assert_eq!(reset["values"]["detailed"], json!(true));
}

#[test]
fn job_takes_the_settings_at_submit_time_and_the_runner_gets_them() {
    let st = stand(None);
    let core = st.server.running().core();
    let (r1, r2) = (st.server.add_runner("r1"), st.server.add_runner("r2"));
    let submit = |player: &str| {
        let src = st.server.dir.path.join(format!("{player}.dem"));
        std::fs::write(&src, fake_demo(4096)).unwrap();
        let sub = Submit {
            player: player.to_owned(),
            user_id: Some(st.user_id),
            requested_by: "петя".to_owned(),
            orig_name: "match.dem".to_owned(),
        };
        core.submit(&src, &sub).unwrap().0
    };

    server::users::patch_user_settings(core.db(), st.user_id, &json!({"detailed": true, "resolution": 1080})).unwrap();
    let first = submit("one");
    // правка после отправки не меняет уже поставленную задачу
    server::users::patch_user_settings(core.db(), st.user_id, &json!({"detailed": false, "flybys": false})).unwrap();
    let second = submit("two");

    let a = st.server.claim(&r1, 0).1.expect("выдана первая задача");
    assert_eq!(a.job_id, first);
    let m = wire::settings::Montage::from_options(&a.options);
    assert!(m.detailed && m.flybys, "снимок на момент отправки");
    assert_eq!(m.frame(), (1920, 1080));
    assert_eq!(a.options.len(), wire::settings::DEFS.len(), "раннер получает полный набор");

    let b = st.server.claim(&r2, 0).1.expect("выдана вторая задача");
    assert_eq!(b.job_id, second);
    let m = wire::settings::Montage::from_options(&b.options);
    assert!(!m.detailed && !m.flybys);
}

#[test]
fn settings_command_sends_the_mini_app_button_or_explains_what_is_missing() {
    let with = stand(Some("https://cs2.example.com"));
    with.tg.push_update(with.tg.text_update(FRIEND, FRIEND, "/settings"));
    with.bot.step().unwrap();
    assert_eq!(
        with.tg.web_app_buttons(),
        vec![(FRIEND, "Открыть настройки".to_owned(), "https://cs2.example.com/app/settings".to_owned())]
    );
    let text = with.tg.texts_to(FRIEND).pop().unwrap();
    assert!(text.contains("Подробный ролик: выкл") && text.contains("Кадров в секунду: 60"), "{text}");

    let without = stand(None);
    without.tg.push_update(without.tg.text_update(FRIEND, FRIEND, "/settings"));
    without.bot.step().unwrap();
    assert!(without.tg.web_app_buttons().is_empty());
    let text = without.tg.texts_to(FRIEND).pop().unwrap();
    assert!(text.contains("public_url"), "{text}");
}
