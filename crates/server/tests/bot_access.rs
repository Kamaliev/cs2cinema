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
