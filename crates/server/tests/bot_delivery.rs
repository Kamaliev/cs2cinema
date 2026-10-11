//! Доставка ролика и отчёты о сбое (план 07-09): неясная отправка не повторяется вслепую, `/resend` отправляет
//! ролик ещё раз, повторы и пределы, фразы другу и подробный отчёт админу. Telegram подставной, время ручное.

mod common;

use common::BotStand;
use server::tg::TgError;
use std::time::Duration;
use wire::JobState;

const FRIEND: i64 = 111;

/// Друг пишет боту текст.
fn say(st: &BotStand, from: i64, text: &str) {
    let u = st.tg.text_update(from, from, text);
    st.tg.push_update(u);
}

/// Друг присылает демку, раннер доводит задачу до `done`; возвращает номер задачи.
fn done_job(st: &BotStand, from: i64) -> i64 {
    let before = st.server.running().core().jobs().unwrap().len();
    st.send_demo(from, "s1mple");
    st.step();
    let jobs = st.server.running().core().jobs().unwrap();
    assert_eq!(jobs.len(), before + 1, "задача должна появиться");
    let id = jobs.iter().map(|j| j.id).max().unwrap();
    let token = st.runner_token();
    st.finish_done(id, &token);
    assert_eq!(st.server.job(id).state, JobState::Done);
    id
}

fn videos_to(st: &BotStand, chat: i64) -> usize {
    st.tg.videos().iter().filter(|v| v.chat == chat).count()
}

#[test]
fn timeout_after_upload_marks_uncertain_and_resend_sends_once_more() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.step();
    let id = st.server.running().core().jobs().unwrap()[0].id;
    let token = st.runner_token();
    st.finish_done(id, &token);

    // Запрос ушёл, ответа нет: фейк записал ролик доставленным.
    st.tg
        .fail_next("send_video", TgError::Timeout { sent: true });
    st.step();
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "uncertain", "{tj:?}");
    assert_eq!(videos_to(&st, FRIEND), 1);
    let texts = st.tg.texts_to(FRIEND);
    let want = format!(
        "Не уверен, что ролик задачи #{id} дошёл. Если его нет в чате — отправьте /resend {id}"
    );
    assert!(texts.iter().any(|t| *t == want), "{texts:?}");
    assert_eq!(
        texts.iter().filter(|t| t.contains("Не уверен")).count(),
        1,
        "одно сообщение на исход"
    );

    // Сам бот второй раз не отправляет, сколько бы времени ни прошло.
    for _ in 0..3 {
        st.clock().advance(Duration::from_secs(60));
        st.step();
    }
    assert_eq!(videos_to(&st, FRIEND), 1, "слепого повтора нет");
    assert_eq!(st.tg_job(id).delivery, "uncertain");

    // Друг просит сам: предупреждение о дубле, затем отправка с файла.
    say(&st, FRIEND, &format!("/resend {id}"));
    st.step();
    let texts = st.tg.texts_to(FRIEND);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("если ролик уже пришёл, дубль — нормально")),
        "{texts:?}"
    );
    assert_eq!(videos_to(&st, FRIEND), 2, "ролик отправлен ещё раз");
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "sent");
    assert!(tj.result_file_id.is_some());
    assert_eq!(tj.delivery_attempts, 1, "счётчик попыток начат заново");
}

#[test]
fn sent_video_is_resent_by_file_id() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let id = done_job(&st, FRIEND);
    st.step();
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "sent");
    let file_id = tj.result_file_id.clone().expect("file_id после отправки");
    let first_msg = tj.result_message_id;
    let reads_before = st.tg.videos().len();

    say(&st, FRIEND, &format!("/resend {id}"));
    st.step();

    let calls: Vec<_> = st
        .tg
        .calls()
        .into_iter()
        .filter(|c| c.method == "send_video")
        .collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[1].detail,
        format!("id:{file_id}"),
        "повтор идёт по file_id, файл не читается"
    );
    assert_eq!(st.tg.videos().len(), reads_before + 1);
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "sent");
    assert_ne!(tj.result_message_id, first_msg, "номер сообщения новый");
    // Без номера — последняя готовая задача.
    say(&st, FRIEND, "/resend");
    st.step();
    assert_eq!(st.tg.videos().len(), reads_before + 2);
}
