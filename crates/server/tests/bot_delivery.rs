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

/// Друг присылает демку, бот ставит задачу в очередь; возвращает номер задачи. Шаг бота заодно отправляет
/// всё, что уже ждёт доставки, поэтому тесты с несколькими задачами сначала ставят их все.
fn queue_job(st: &BotStand, from: i64, player: &str) -> i64 {
    let before = st.server.running().core().jobs().unwrap().len();
    st.send_demo(from, player);
    st.step();
    let jobs = st.server.running().core().jobs().unwrap();
    assert_eq!(jobs.len(), before + 1, "задача должна появиться");
    jobs.iter().map(|j| j.id).max().unwrap()
}

/// Фейковый раннер доводит задачу до `done` (задачи берутся в порядке номеров).
fn finish_job(st: &BotStand, id: i64) {
    let token = st.runner_token();
    st.finish_done(id, &token);
    assert_eq!(st.server.job(id).state, JobState::Done);
}

/// Задача друга доведена до `done`.
fn done_job(st: &BotStand, from: i64, player: &str) -> i64 {
    let id = queue_job(st, from, player);
    finish_job(st, id);
    id
}

fn send_video_calls(st: &BotStand) -> usize {
    st.tg
        .calls()
        .iter()
        .filter(|c| c.method == "send_video")
        .count()
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
    let id = done_job(&st, FRIEND, "s1mple");
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

// ---------------------------------------------------------------------------------------------------------
// Повторы, 403, предел размера, документ, чужой /resend (Task 2)
// ---------------------------------------------------------------------------------------------------------

use server::bot::{Bot, BotSettings, SendAs};
use server::clock::Clock;
use server::tg::Tg;
use std::sync::Arc;

const OTHER: i64 = 222;

#[test]
fn rate_limit_and_unsent_errors_retry_without_duplicates() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let a = queue_job(&st, FRIEND, "s1mple");
    let b = queue_job(&st, FRIEND, "ZywOo");
    finish_job(&st, a);

    // 429: ролик остаётся в очереди и уходит ровно через retry_after.
    let t0 = st.clock().now_ms();
    st.tg.fail_next(
        "send_video",
        TgError::Api {
            code: 429,
            description: "Too Many Requests: retry after 20".to_owned(),
            retry_after: Some(20),
        },
    );
    st.step();
    let tj = st.tg_job(a);
    assert_eq!(tj.delivery, "pending", "{tj:?}");
    assert_eq!(tj.next_try_ms, t0 + 20_000);
    assert_eq!(send_video_calls(&st), 1);
    st.clock().advance(Duration::from_secs(19));
    st.step();
    assert_eq!(send_video_calls(&st), 1, "раньше срока вызова нет");
    st.clock().advance(Duration::from_secs(1));
    st.step();
    assert_eq!(send_video_calls(&st), 2);
    assert_eq!(videos_to(&st, FRIEND), 1, "ролик отправлен один раз");
    assert_eq!(st.tg_job(a).delivery, "sent");

    // Запрос не ушёл (тайм-аут до отправки): пять попыток с паузой 30 с, потом failed.
    finish_job(&st, b);
    for _ in 0..5 {
        st.tg
            .fail_next("send_video", TgError::Timeout { sent: false });
    }
    for i in 0..5 {
        st.step();
        let tj = st.tg_job(b);
        if i < 4 {
            assert_eq!(tj.delivery, "pending", "попытка {}: {tj:?}", i + 1);
            assert_eq!(tj.delivery_attempts, i + 1);
        }
        st.clock().advance(Duration::from_secs(30));
    }
    let tj = st.tg_job(b);
    assert_eq!(tj.delivery, "failed", "{tj:?}");
    assert_eq!(tj.delivery_attempts, 5);
    assert_eq!(videos_to(&st, FRIEND), 1, "второй ролик не уходил");
    let texts = st.tg.texts_to(FRIEND);
    let failed: Vec<_> = texts
        .iter()
        .filter(|t| t.contains(&format!("ролик задачи #{b}")))
        .collect();
    assert_eq!(failed.len(), 1, "одно сообщение о неудаче: {texts:?}");
    assert!(failed[0].contains(&format!("/resend {b}")), "{failed:?}");
    // Дальше сам бот не пробует.
    st.clock().advance(Duration::from_secs(300));
    st.step();
    assert_eq!(videos_to(&st, FRIEND), 1);

    // /resend после неудачи: попытки с нуля, отправка с файла.
    say(&st, FRIEND, &format!("/resend {b}"));
    st.step();
    assert_eq!(st.tg_job(b).delivery, "sent");
    assert_eq!(videos_to(&st, FRIEND), 2);
}

#[test]
fn forbidden_marks_undeliverable() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.add_friend(OTHER, "петя");
    let a = queue_job(&st, FRIEND, "s1mple");
    let b = queue_job(&st, OTHER, "ZywOo");
    finish_job(&st, a);
    finish_job(&st, b);
    let before = st.tg.texts_to(FRIEND).len();

    st.tg.fail_next(
        "send_video",
        TgError::Api {
            code: 403,
            description: "Forbidden: bot was blocked by the user".to_owned(),
            retry_after: None,
        },
    );
    st.step();

    let ta = st.tg_job(a);
    assert_eq!(ta.delivery, "failed", "{ta:?}");
    assert!(ta.muted, "чат заглушён");
    assert_eq!(
        st.tg.texts_to(FRIEND).len(),
        before,
        "заблокировавшему бота не пишут"
    );
    assert_eq!(videos_to(&st, FRIEND), 0);
    let tb = st.tg_job(b);
    assert_eq!(tb.delivery, "sent", "другой чат не пострадал: {tb:?}");
    assert!(!tb.muted);
    assert_eq!(videos_to(&st, OTHER), 1);
}

#[test]
fn oversized_result_is_refused_before_sending() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let id = done_job(&st, FRIEND, "s1mple");
    st.server
        .running()
        .core()
        .db()
        .write(|tx| {
            tx.execute(
                "UPDATE files SET size = ?2 WHERE job_id = ?1 AND kind = 'result'",
                rusqlite::params![id, 2000_i64 * 1024 * 1024],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
        .unwrap();
    st.step();

    assert_eq!(send_video_calls(&st), 0, "вызова Telegram не было");
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "failed", "{tj:?}");
    let texts = st.tg.texts_to(FRIEND);
    assert!(
        texts
            .iter()
            .any(|t| t.contains(&format!("Ролик задачи #{id} больше лимита Telegram"))),
        "{texts:?}"
    );
}

#[test]
fn document_mode_sends_a_document() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let mut settings = BotSettings::new(&st.server.dir.path.join("data"), st.tg.files_dir());
    settings.send_as = SendAs::Document;
    let bot = Bot::new(
        st.server.running().core().clone(),
        Arc::new(st.tg.clone()) as Arc<dyn Tg>,
        st.server.running().log().clone(),
        settings,
    );
    st.send_demo(FRIEND, "s1mple");
    bot.step().unwrap();
    let id = st.server.running().core().jobs().unwrap()[0].id;
    finish_job(&st, id);
    bot.step().unwrap();

    let docs: Vec<_> = st
        .tg
        .calls()
        .into_iter()
        .filter(|c| c.method == "send_document")
        .collect();
    assert_eq!(docs.len(), 1, "{:?}", st.tg.calls());
    assert!(
        docs[0].detail.starts_with("file:///") && docs[0].detail.ends_with("/highlights.mp4"),
        "{}",
        docs[0].detail
    );
    assert_eq!(send_video_calls(&st), 0);
    assert_eq!(st.tg.videos()[0].method, "send_document");
    assert_eq!(st.tg_job(id).delivery, "sent");
}

#[test]
fn other_users_cannot_resend() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.add_friend(OTHER, "петя");
    let id = done_job(&st, FRIEND, "s1mple");
    st.step();
    assert_eq!(st.tg_job(id).delivery, "sent");
    let calls = send_video_calls(&st);

    say(&st, OTHER, &format!("/resend {id}"));
    say(&st, OTHER, "/resend 9999");
    st.step();
    let texts = st.tg.texts_to(OTHER);
    assert_eq!(
        texts.iter().filter(|t| *t == "Это не ваша задача.").count(),
        2,
        "{texts:?}"
    );
    assert_eq!(send_video_calls(&st), calls, "отправки не было");
    assert_eq!(videos_to(&st, OTHER), 0);
}

#[test]
fn resend_says_when_the_video_is_not_ready_or_already_going() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let id = queue_job(&st, FRIEND, "s1mple");
    say(&st, FRIEND, &format!("/resend {id}"));
    st.step();
    assert!(
        st.tg
            .texts_to(FRIEND)
            .iter()
            .any(|t| t.contains(&format!("Ролик задачи #{id} ещё не готов"))),
        "{:?}",
        st.tg.texts_to(FRIEND)
    );
    finish_job(&st, id);
    // Ролик готов, доставка ещё не началась (pending): повтор не нужен.
    st.bot.notify_once().unwrap();
    assert_eq!(st.tg_job(id).delivery, "pending");
    say(&st, FRIEND, &format!("/resend {id}"));
    st.bot.poll_once(0).unwrap();
    assert!(
        st.tg
            .texts_to(FRIEND)
            .iter()
            .any(|t| t.contains("уже отправляется")),
        "{:?}",
        st.tg.texts_to(FRIEND)
    );
}
