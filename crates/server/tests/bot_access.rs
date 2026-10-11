//! Доступ к боту (фаза 7, план 07-04): белый список, команды админа, выбор игрока и судьба задач убранного друга.
//! Telegram подставной (`FakeTelegram`), сервер и база настоящие, время двигают ручные часы.

mod common;

use common::BotStand;
use std::time::Duration;

const ADMIN: i64 = 1;

/// Пользователь `from` пишет боту в личный чат текст.
fn say(st: &BotStand, from: i64, text: &str) {
    st.tg.push_update(st.tg.text_update(from, from, text));
}

/// Последний текст бота в чат пользователя.
fn last_text(st: &BotStand, chat: i64) -> String {
    st.tg
        .texts_to(chat)
        .pop()
        .unwrap_or_else(|| "<ничего не пришло>".to_owned())
}

#[test]
fn admin_adds_and_removes_a_friend_from_the_chat() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");

    // Незнакомец получает свой ID.
    say(&st, 222, "привет");
    st.step();
    assert!(
        last_text(&st, 222).contains("222"),
        "незнакомцу пришёл его ID: {}",
        last_text(&st, 222)
    );

    // Админ добавляет друга.
    say(&st, ADMIN, "/adduser 222 петя");
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("Добавил петя (222)"),
        "ответ админу: {}",
        last_text(&st, ADMIN)
    );
    let core = st.server.running().core();
    let petya = server::users::list_users(core.db())
        .unwrap()
        .into_iter()
        .find(|u| u.name == "петя")
        .expect("петя в списке");
    assert_eq!(petya.tg_id, Some(222));
    assert_eq!(petya.role, "member");

    // Друг присылает демку.
    st.send_demo(222, "s1mple");
    st.step();
    assert_eq!(st.tg.get_file_calls(), 1, "демка друга скачана");
    assert_eq!(core.jobs().unwrap().len(), 1, "задача поставлена");

    // Админ убирает друга.
    say(&st, ADMIN, "/removeuser 222");
    st.step();
    let reply = last_text(&st, ADMIN);
    assert!(reply.contains("Убрал петя (222)"), "ответ админу: {reply}");
    assert!(
        reply.contains("Активных задач: 1") && reply.contains("ролики будут доставлены"),
        "ответ называет число задач: {reply}"
    );

    // Убранный друг снова получает только свой ID (после паузы между ответами незнакомцу).
    st.clock().advance(Duration::from_secs(11 * 60));
    st.send_demo(222, "s1mple");
    st.step();
    assert_eq!(
        st.tg.get_file_calls(),
        1,
        "после /removeuser файл не скачивается"
    );
    assert!(
        last_text(&st, 222).contains("222"),
        "{}",
        last_text(&st, 222)
    );
    assert_eq!(core.jobs().unwrap().len(), 1, "новых задач нет");

    // Список показывает отметку «убран».
    say(&st, ADMIN, "/users");
    st.step();
    let list = last_text(&st, ADMIN);
    let line = list
        .lines()
        .find(|l| l.contains("петя"))
        .unwrap_or_else(|| panic!("в списке есть петя: {list}"));
    assert!(line.contains("убран"), "строка друга: {line}");

    // Повторное добавление возвращает доступ.
    say(&st, ADMIN, "/adduser 222");
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("Вернул доступ петя (222)"),
        "{}",
        last_text(&st, ADMIN)
    );
    st.send_demo(222, "s1mple");
    st.step();
    assert_eq!(st.tg.get_file_calls(), 2, "доступ вернулся");
}

const FRIEND: i64 = 111;

/// Сколько строк в таблице базы сервера.
fn count_rows(st: &BotStand, table: &str) -> i64 {
    st.server
        .running()
        .core()
        .db()
        .read(|c| {
            c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .map_err(|e| e.to_string())
        })
        .unwrap()
}

/// Сколько раз бот отправлял сообщения (всем чатам).
fn sent_messages(st: &BotStand) -> usize {
    st.tg
        .calls()
        .iter()
        .filter(|c| c.method == "send_message")
        .count()
}

/// Обновление «документ от `from` в чат `chat`» с заданным видом чата и номером обновления, как в Bot API.
fn document_json(kind: &str, update_id: i64, from: i64, chat: i64, chat_kind: &str) -> String {
    format!(
        r#"{{"update_id":{update_id},"{kind}":{{"message_id":{update_id},"date":1,
            "chat":{{"id":{chat},"type":"{chat_kind}"}},"from":{{"id":{from},"is_bot":false,"first_name":"x"}},
            "document":{{"file_id":"f-{update_id}","file_unique_id":"u-{update_id}","file_name":"m.dem","file_size":4096}}}}}}"#
    )
}

#[test]
fn stranger_gets_only_the_id_and_nothing_is_fetched() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    let core = st.server.running().core();
    let stranger = 333;

    let mut last_update = 0;
    let mut push = |u: server::tg::Update| {
        last_update = last_update.max(u.update_id);
        st.tg.push_update(u);
    };
    push(st.tg.text_update(stranger, stranger, "привет"));
    let bytes = common::fake_demo(4096);
    st.tg.put_document("doc-1", "uniq-1", &bytes);
    push(st.tg.document_update(
        stranger,
        stranger,
        "doc-1",
        "uniq-1",
        "a.dem",
        4096,
        Some("s1mple"),
    ));
    st.tg.put_document("doc-2", "uniq-2", &bytes);
    let mut forwarded = st
        .tg
        .document_update(stranger, stranger, "doc-2", "uniq-2", "b.dem", 4096, None);
    forwarded.message.as_mut().unwrap().forward_origin =
        Some(serde_json::json!({"type": "user", "date": 1}));
    push(forwarded);
    push(st.tg.text_update(stranger, stranger, "/start"));
    push(st.tg.text_update(stranger, stranger, "/adduser 333"));
    st.step();

    let texts = st.tg.texts_to(stranger);
    assert_eq!(texts.len(), 1, "незнакомцу ровно одно сообщение: {texts:?}");
    assert!(texts[0].contains("333"), "в ответе его ID: {}", texts[0]);
    assert!(
        !texts[0].to_lowercase().contains("файл"),
        "про файл ни слова: {}",
        texts[0]
    );
    assert_eq!(st.tg.get_file_calls(), 0, "getFile не вызывался");
    assert_eq!(count_rows(&st, "tg_inbox"), 0, "входящих нет");
    assert_eq!(count_rows(&st, "jobs"), 0, "задач нет");
    assert_eq!(
        server::users::list_users(core.db()).unwrap().len(),
        1,
        "список пользователей не изменился"
    );
    assert_eq!(
        server::bot::store::offset(core.db()).unwrap(),
        last_update + 1,
        "смещение сдвинуто за все обновления"
    );
}

#[test]
fn edited_channel_and_group_updates_are_ignored() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = common::fake_demo(4096);
    for id in [9001, 9002, 9005] {
        st.tg
            .put_document(&format!("f-{id}"), &format!("u-{id}"), &bytes);
    }
    st.tg.push_json(&document_json(
        "edited_message",
        9001,
        FRIEND,
        FRIEND,
        "private",
    ));
    st.tg.push_json(&document_json(
        "channel_post",
        9002,
        FRIEND,
        -100500,
        "channel",
    ));
    st.tg.push_json(&format!(
        r#"{{"update_id":9003,"my_chat_member":{{"chat":{{"id":{FRIEND},"type":"private"}},"from":{{"id":{FRIEND},"is_bot":false,"first_name":"x"}},"date":1}}}}"#
    ));
    st.tg.push_json(&format!(
        r#"{{"update_id":9004,"callback_query":{{"id":"c1","from":{{"id":{FRIEND},"is_bot":false,"first_name":"x"}},"data":"x"}}}}"#
    ));
    st.tg
        .push_json(&document_json("message", 9005, FRIEND, -4242, "group"));
    st.step();

    assert_eq!(sent_messages(&st), 0, "бот не отправил ни одного сообщения");
    assert_eq!(st.tg.get_file_calls(), 0, "getFile не вызывался");
    assert_eq!(count_rows(&st, "jobs"), 0, "задач нет");
    assert_eq!(count_rows(&st, "tg_inbox"), 0, "входящих нет");
    assert_eq!(
        server::bot::store::offset(st.server.running().core().db()).unwrap(),
        9006,
        "смещение сдвинуто за все пять обновлений"
    );
}

#[test]
fn stranger_reply_is_rate_limited() {
    let st = BotStand::start();
    let stranger = 444;
    say(&st, stranger, "раз");
    say(&st, stranger, "два");
    st.step();
    assert_eq!(
        st.tg.texts_to(stranger).len(),
        1,
        "два сообщения — один ответ"
    );

    st.clock().advance(Duration::from_secs(9 * 60));
    say(&st, stranger, "три");
    st.step();
    assert_eq!(
        st.tg.texts_to(stranger).len(),
        1,
        "через 9 минут ответа ещё нет"
    );

    st.clock().advance(Duration::from_secs(60));
    say(&st, stranger, "четыре");
    st.step();
    let texts = st.tg.texts_to(stranger);
    assert_eq!(
        texts.len(),
        2,
        "через 10 минут ответ снова приходит: {texts:?}"
    );
    assert!(texts[1].contains("444"));
}

#[test]
fn member_cannot_run_admin_commands() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    for cmd in ["/adduser 444 вася2", "/removeuser 1", "/users"] {
        say(&st, FRIEND, cmd);
    }
    st.step();
    let texts = st.tg.texts_to(FRIEND);
    assert_eq!(texts.len(), 3, "три ответа: {texts:?}");
    for t in &texts {
        assert!(t.contains("только для администратора"), "{t}");
    }
    let list = server::users::list_users(st.server.running().core().db()).unwrap();
    assert_eq!(list.len(), 2, "список не изменился");
    assert!(list.iter().all(|u| u.is_active()), "админ не убран");
}

#[test]
fn adduser_never_grants_admin() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    let db = st.server.running().core().db();

    say(&st, ADMIN, "/adduser 555 ада --admin");
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("Формат"),
        "отказ с подсказкой: {}",
        last_text(&st, ADMIN)
    );
    assert!(
        server::users::find_user_by_tg(db, 555).unwrap().is_none(),
        "никого не добавили"
    );
    for bad in [
        "/adduser 555 --admin",
        "/adduser abc",
        "/adduser -5",
        "/adduser",
    ] {
        say(&st, ADMIN, bad);
    }
    st.step();
    assert!(server::users::find_user_by_tg(db, 555).unwrap().is_none());

    say(&st, ADMIN, "/adduser 555 ада");
    st.step();
    let ada = server::users::find_user_by_tg(db, 555).unwrap().unwrap();
    assert_eq!(ada.role, "member", "через чат создаётся только участник");

    say(&st, ADMIN, &format!("/removeuser {ADMIN}"));
    st.step();
    assert!(
        last_text(&st, ADMIN).contains("оператор"),
        "админа убирает оператор: {}",
        last_text(&st, ADMIN)
    );
    assert!(
        server::users::find_user_by_tg(db, ADMIN)
            .unwrap()
            .unwrap()
            .is_active()
    );
}

#[test]
fn forwarded_document_from_a_friend_is_his_own_demo() {
    let st = BotStand::start();
    let user_id = st.add_friend(FRIEND, "вася");
    let bytes = common::fake_demo(4096);
    st.tg.put_document("doc-f", "uniq-f", &bytes);
    let mut upd = st.tg.document_update(
        FRIEND,
        FRIEND,
        "doc-f",
        "uniq-f",
        "fwd.dem",
        4096,
        Some("ZywOo"),
    );
    upd.message.as_mut().unwrap().forward_origin =
        Some(serde_json::json!({"type": "channel", "date": 1}));
    st.tg.push_update(upd);
    st.step();

    assert_eq!(st.tg.get_file_calls(), 1);
    let jobs = st.server.running().core().jobs().unwrap();
    assert_eq!(jobs.len(), 1, "задача поставлена");
    assert_eq!(jobs[0].user_id, Some(user_id));
    assert_eq!(jobs[0].player_name, "ZywOo");
}

#[test]
fn player_is_saved_and_fixed_in_the_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let core = st.server.running().core();

    say(&st, FRIEND, "/player 76561198034202275");
    st.step();
    assert_eq!(last_text(&st, FRIEND), "Игрок: 76561198034202275");

    st.send_demo(FRIEND, "");
    st.step();
    let jobs = core.jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].player_steam.as_deref(), Some("76561198034202275"));

    say(&st, FRIEND, "/player ZywOo");
    st.step();
    assert_eq!(last_text(&st, FRIEND), "Игрок: ZywOo");
    let first = core.job(jobs[0].id).unwrap().unwrap();
    assert_eq!(
        first.player_steam.as_deref(),
        Some("76561198034202275"),
        "уже поставленная задача не меняется"
    );

    st.send_demo(FRIEND, "");
    st.step();
    let jobs = core.jobs().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[1].player_name, "ZywOo");
    assert_eq!(jobs[1].player_steam, None, "ник не выдаётся за SteamID");

    say(&st, FRIEND, "/player");
    st.step();
    assert_eq!(last_text(&st, FRIEND), "Игрок: ZywOo");
}

#[test]
fn caption_overrides_the_saved_player_for_one_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let core = st.server.running().core();

    say(&st, FRIEND, "/player s1mple");
    st.send_demo(FRIEND, "ZywOo");
    st.step();
    st.send_demo(FRIEND, "");
    st.step();

    let jobs = core.jobs().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].player_name, "ZywOo", "подпись главнее сохранённого");
    assert_eq!(jobs[1].player_name, "s1mple", "без подписи — сохранённый");
    let user = server::users::find_user_by_tg(core.db(), FRIEND)
        .unwrap()
        .unwrap();
    assert_eq!(
        user.player_name.as_deref(),
        Some("s1mple"),
        "сохранённый игрок не изменился"
    );
}

#[test]
fn player_with_control_or_bidi_characters_is_refused() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let core = st.server.running().core();
    say(&st, FRIEND, "/player s1mple");
    st.step();

    let too_long = format!("/player {}", "я".repeat(65));
    for bad in [
        "/player a\u{202E}b",
        "/player a\u{2028}b",
        "/player \u{7}",
        too_long.as_str(),
    ] {
        let before = st.tg.texts_to(FRIEND).len();
        say(&st, FRIEND, bad);
        st.step();
        let texts = st.tg.texts_to(FRIEND);
        assert_eq!(texts.len(), before + 1, "на {bad:?} один ответ");
        assert!(
            texts[before].contains("Не принял игрока"),
            "отказ с объяснением на {bad:?}: {}",
            texts[before]
        );
        let user = server::users::find_user_by_tg(core.db(), FRIEND)
            .unwrap()
            .unwrap();
        assert_eq!(
            user.player_name.as_deref(),
            Some("s1mple"),
            "сохранённый игрок прежний после {bad:?}"
        );
    }
}

#[test]
fn start_and_help_list_the_commands() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");

    say(&st, FRIEND, "/start");
    say(&st, FRIEND, "/help");
    say(&st, ADMIN, "/help");
    st.step();

    let friend = st.tg.texts_to(FRIEND);
    assert!(friend[0].contains("/player") && friend[0].contains(".dem"));
    assert!(
        !friend[1].contains("/adduser"),
        "друг не видит команды управления: {}",
        friend[1]
    );
    assert!(friend[1].contains("/player"));
    let admin = st.tg.texts_to(ADMIN);
    assert!(admin[0].contains("/adduser") && admin[0].contains("/removeuser"));

    let names: Vec<String> = server::bot::commands::menu()
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    for want in ["start", "help", "player"] {
        assert!(names.iter().any(|n| n == want), "в меню нет {want}");
    }
}

#[test]
fn removed_friend_job_still_finishes_and_is_delivered() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    let core = st.server.running().core();

    st.send_demo(FRIEND, "s1mple");
    st.step();
    let job_id = core.jobs().unwrap()[0].id;

    say(&st, ADMIN, &format!("/removeuser {FRIEND}"));
    st.step();
    assert!(last_text(&st, ADMIN).contains("Убрал вася"));

    let token = st.runner_token();
    st.finish_done(job_id, &token);
    st.step();

    let videos = st.tg.videos();
    assert_eq!(
        videos.len(),
        1,
        "ролик доставлен убранному другу: {videos:?}"
    );
    assert_eq!(videos[0].chat, FRIEND);
    assert_eq!(st.tg_job(job_id).delivery, "sent");

    // Новая демка от убранного друга — только его ID.
    st.clock().advance(Duration::from_secs(11 * 60));
    st.send_demo(FRIEND, "s1mple");
    st.step();
    assert!(
        last_text(&st, FRIEND).contains("111"),
        "{}",
        last_text(&st, FRIEND)
    );
    assert_eq!(core.jobs().unwrap().len(), 1, "новых задач нет");
    assert_eq!(st.tg.get_file_calls(), 1);
}
