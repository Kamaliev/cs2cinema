//! Приём ссылок FACEIT ботом (фаза 7, план 07): строгий шаблон, объяснение отказа, повторы и лимиты.
//! Telegram подставной (`FakeTelegram`), сервер и база настоящие; по ссылкам бот никуда не ходит.

mod common;

use common::BotStand;
use server::bot::store;
use wire::JobState;

const FRIEND: i64 = 111;
const ID: &str = "1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";

fn room(id: &str) -> String {
    format!("https://www.faceit.com/ru/cs2/room/{id}")
}

fn say(st: &BotStand, text: &str) {
    st.tg.push_update(st.tg.text_update(FRIEND, FRIEND, text));
}

fn jobs(st: &BotStand) -> Vec<server::core::JobRow> {
    st.server.running().core().jobs().unwrap()
}

fn inbox(st: &BotStand) -> Vec<store::InboxRow> {
    store::inbox(st.server.running().core().db()).unwrap()
}

fn chat_text(st: &BotStand) -> String {
    st.tg.texts_to(FRIEND).join("\n")
}

#[test]
fn strict_link_is_queued_without_get_file() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    say(&st, &format!("{} s1mple", room(ID)));
    // Опрос: подтверждение приёма ссылки уходит до всякой работы с задачей.
    st.bot.poll_once(0).unwrap();
    let ack = chat_text(&st);
    assert!(ack.contains("FACEIT"), "подтверждение: {ack}");
    assert!(ack.contains("s1mple"), "подтверждение: {ack}");
    st.step();

    let all = jobs(&st);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].state, JobState::Queued);
    assert_eq!(all[0].source_kind, "faceit");
    assert_eq!(all[0].player_name, "s1mple");
    assert_eq!(
        all[0].faceit_link.as_deref(),
        Some(format!("https://www.faceit.com/en/cs2/room/{ID}").as_str())
    );
    let text = chat_text(&st);
    assert!(text.contains(&format!("#{}", all[0].id)), "номер задачи: {text}");
    assert_eq!(st.tg.get_file_calls(), 0, "по ссылке getFile не вызывается");
}

#[test]
fn non_room_links_are_refused_with_an_explanation() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bad = [
        format!("https://www.faceit.com.evil.com/en/cs2/room/{ID}"),
        format!("http://www.faceit.com/en/cs2/room/{ID}"),
        ID.to_owned(),
        "https://www.faceit.com/en/players/s1mple".to_owned(),
    ];
    for (n, link) in bad.iter().enumerate() {
        let before = st.tg.texts_to(FRIEND).len();
        say(&st, &format!("{link} s1mple"));
        st.step();
        let texts = st.tg.texts_to(FRIEND);
        assert_eq!(texts.len(), before + 1, "один ответ на каждое: {texts:?}");
        let reply = texts.last().unwrap();
        assert!(
            reply.contains("Это не ссылка на комнату матча FACEIT"),
            "{n}: {reply}"
        );
        assert!(reply.contains("https://www.faceit.com/ru/cs2/room/"), "пример: {reply}");
        assert!(!reply.contains("evil.com"), "присланная строка не повторяется: {reply}");
        assert!(!reply.contains("players/s1mple"), "{reply}");
    }
    assert!(jobs(&st).is_empty(), "задач нет");
    assert!(inbox(&st).is_empty(), "входящих нет");
}

#[test]
fn plain_text_gets_a_hint() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    say(&st, "привет");
    st.step();
    let text = chat_text(&st);
    assert!(text.contains(".dem"), "{text}");
    assert!(text.contains(".dem.zst"), "{text}");
    assert!(text.contains("FACEIT"), "{text}");
    assert!(jobs(&st).is_empty());
}

#[test]
fn link_without_player_asks_for_player() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    say(&st, &room(ID));
    st.step();
    let text = chat_text(&st);
    assert!(text.contains("/player"), "{text}");
    assert!(jobs(&st).is_empty());
    assert!(inbox(&st).is_empty());
}

#[test]
fn same_match_same_player_is_a_duplicate() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    say(&st, &format!("{} s1mple", room(ID)));
    st.step();
    let first = jobs(&st)[0].id;

    // Тот же матч другой записью (регистр и хвост пути), тот же игрок: повтор.
    let again = format!(
        "https://WWW.FACEIT.COM/en/cs2/room/{}/scoreboard S1MPLE",
        ID.to_uppercase()
    );
    say(&st, &again);
    st.step();
    assert_eq!(jobs(&st).len(), 1, "вторая задача не нужна");
    let text = chat_text(&st);
    assert!(
        text.contains(&format!("уже в очереди: задача #{first}")),
        "{text}"
    );

    // Другой игрок на том же матче: вторая задача.
    say(&st, &format!("{} ZywOo", room(ID)));
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[1].player_name, "ZywOo");
    assert_eq!(all[1].faceit_link, all[0].faceit_link);
}

#[test]
fn two_pending_rows_for_the_same_pair_make_one_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    // Обе ссылки приняты опросом до того, как первая стала задачей.
    say(&st, &format!("{} s1mple", room(ID)));
    say(&st, &format!("{} s1mple", room(ID)));
    st.bot.poll_once(0).unwrap();
    assert_eq!(inbox(&st).len(), 2);
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 1, "{all:?}");
    let text = chat_text(&st);
    assert!(text.contains("уже в очереди"), "{text}");
}

#[test]
fn link_respects_limits() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    say(&st, &format!("{} s1mple", room(ID)));
    say(&st, &format!("{} ZywOo", room(ID)));
    say(&st, &format!("{} donk", room(ID)));
    // Только опрос: ни одной задачи, а лимит уже считает строки tg_inbox.
    st.bot.poll_once(0).unwrap();
    assert_eq!(inbox(&st).len(), 2, "две строки во входящих");
    let text = chat_text(&st).to_lowercase();
    assert!(
        text.contains("у вас уже 2 активные задачи — дождитесь окончания или отмените /cancel"),
        "тот же текст лимита, что у файла: {text}"
    );
    st.step();
    assert_eq!(jobs(&st).len(), 2);
    assert_eq!(st.tg.get_file_calls(), 0);
}
