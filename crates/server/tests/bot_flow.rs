//! Трассирующий срез бота (фаза 7): друг присылает демку, получает подтверждение, задача встаёт в очередь,
//! фейковый раннер доводит её до `done`, и готовый ролик приходит новым сообщением. Telegram подставной
//! (`FakeTelegram`), сервер и база настоящие, время двигают ручные часы.

mod common;

use common::BotStand;
use server::bot::store;
use wire::JobState;

const FRIEND: i64 = 111;

#[test]
fn friend_demo_becomes_a_job_and_the_video_comes_back_as_a_new_message() {
    let st = BotStand::start();
    let user_id = st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.step();

    let sent: Vec<_> = st
        .tg
        .calls()
        .into_iter()
        .filter(|c| c.method == "send_message" && c.chat == Some(FRIEND))
        .collect();
    assert!(
        sent[0].detail.contains("Принял"),
        "первое сообщение — подтверждение приёма: {:?}",
        sent[0].detail
    );
    assert_eq!(st.tg.get_file_calls(), 1, "getFile ровно один раз");

    let jobs = st.server.running().core().jobs().unwrap();
    assert_eq!(jobs.len(), 1, "одна задача");
    let job = &jobs[0];
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.player_name, "s1mple");
    assert_eq!(job.user_id, Some(user_id));
    assert_eq!(job.requested_by, "вася");
    let texts = st.tg.texts_to(FRIEND);
    assert_eq!(
        texts.len(),
        1,
        "подтверждение правится, а не дублируется: {texts:?}"
    );
    assert!(
        texts[0].contains(&format!("#{}", job.id)),
        "после правки в тексте номер задачи: {}",
        texts[0]
    );
    let tj = st.tg_job(job.id);
    let status_id = tj.status_message_id.expect("сообщение статуса");
    assert_eq!(tj.delivery, "none");

    let token = st.runner_token();
    let size = st.finish_done(job.id, &token);
    st.step();

    let videos = st.tg.videos();
    assert_eq!(videos.len(), 1, "ровно один ролик: {videos:?}");
    assert_eq!(videos[0].chat, FRIEND);
    assert!(
        videos[0].path.ends_with(std::path::Path::new(&format!(
            "results/{}/highlights.mp4",
            job.id
        ))),
        "путь ролика: {:?}",
        videos[0].path
    );
    let on_server = std::fs::metadata(st.server.results_dir(job.id).join("highlights.mp4"))
        .unwrap()
        .len();
    assert_eq!(videos[0].size, on_server);
    assert_eq!(on_server, size);
    let tj = st.tg_job(job.id);
    assert_eq!(tj.delivery, "sent");
    assert!(tj.result_message_id.is_some());
    assert!(tj.result_file_id.is_some());
    assert_ne!(
        tj.result_message_id,
        Some(status_id),
        "ролик — новое сообщение, а не правка статуса"
    );
    assert_eq!(
        store::tg_jobs(st.server.running().core().db())
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------------------------------------
// Ровно один раз: повтор обновления, перезапуск, белый список (Task 2)
// ---------------------------------------------------------------------------------------------------------

use server::bot::Bot;
use server::tg::Tg;
use std::sync::Arc;
use std::time::Duration;

/// Выполняет SQL на базе стенда.
fn sql(st: &BotStand, q: &str) {
    st.server
        .running()
        .core()
        .db()
        .write(|tx| tx.execute(q, []).map(|_| ()).map_err(|e| e.to_string()))
        .unwrap();
}

/// Новый `Bot` на том же ядре, фейке и корнях файлов: как после перезапуска процесса.
fn restarted(st: &BotStand) -> Bot {
    Bot::new(
        st.server.running().core().clone(),
        Arc::new(st.tg.clone()) as Arc<dyn Tg>,
        st.server.running().log().clone(),
        server::bot::BotSettings::new(&st.server.dir.path.join("data"), st.tg.files_dir()),
    )
}

fn sent_messages(st: &BotStand, chat: i64) -> usize {
    st.tg
        .calls()
        .iter()
        .filter(|c| c.method == "send_message" && c.chat == Some(chat))
        .count()
}

fn inbox_len(st: &BotStand) -> usize {
    store::inbox(st.server.running().core().db()).unwrap().len()
}

fn jobs_len(st: &BotStand) -> usize {
    st.server.running().core().jobs().unwrap().len()
}

#[test]
fn repeated_update_creates_one_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    let first = st.tg.get_updates(0, 0).unwrap();
    assert_eq!(first.len(), 1);
    // То же обновление приходит дважды в одной пачке.
    st.tg.push_update(first[0].clone());
    st.step();
    assert_eq!(inbox_len(&st), 1, "одна строка во входящих");
    assert_eq!(jobs_len(&st), 1, "одна задача");
    assert_eq!(sent_messages(&st, FRIEND), 1, "одно подтверждение");

    // Смещение потеряно (падение до записи), Telegram присылает то же обновление снова.
    sql(&st, "DELETE FROM tg_kv WHERE key = 'offset'");
    st.tg.push_update(first[0].clone());
    st.step();
    assert_eq!(inbox_len(&st), 1, "повтор не создал строку");
    assert_eq!(jobs_len(&st), 1, "повтор не создал задачу");
    assert_eq!(
        sent_messages(&st, FRIEND),
        1,
        "повтор не породил второе подтверждение"
    );
}

#[test]
fn restart_after_poll_ingests_the_pending_demo_once() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    assert_eq!(st.bot.poll_once(0).unwrap(), 1);
    assert_eq!(inbox_len(&st), 1);
    assert_eq!(jobs_len(&st), 0, "до ingest задачи ещё нет");

    let bot = restarted(&st);
    bot.recover().unwrap();
    bot.step().unwrap();
    bot.step().unwrap();
    assert_eq!(jobs_len(&st), 1, "ровно одна задача");
    assert_eq!(st.tg.get_file_calls(), 1);
}

#[test]
fn missing_confirmation_is_sent_before_the_download() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.bot.poll_once(0).unwrap();
    // Падение между записью строки и ответом: подтверждения нет.
    sql(&st, "UPDATE tg_inbox SET status_message_id = NULL");
    let bot = restarted(&st);
    bot.step().unwrap();
    let id = st.server.running().core().jobs().unwrap()[0].id;
    let tj = st.tg_job(id);
    assert!(
        tj.status_message_id.is_some(),
        "сообщение статуса сохранено"
    );
    assert!(
        st.tg
            .texts_to(FRIEND)
            .iter()
            .any(|t| t.contains(&format!("#{id}"))),
        "{:?}",
        st.tg.texts_to(FRIEND)
    );
}

#[test]
fn interrupted_download_is_retried_after_restart() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.bot.poll_once(0).unwrap();
    sql(&st, "UPDATE tg_inbox SET state = 'downloading'");
    let bot = restarted(&st);
    bot.recover().unwrap();
    let rows = store::inbox(st.server.running().core().db()).unwrap();
    assert_eq!(rows[0].state, "pending", "recover вернул строку в pending");
    bot.step().unwrap();
    assert_eq!(jobs_len(&st), 1);
}

#[test]
fn sending_delivery_becomes_uncertain_after_restart_without_a_resend() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.step();
    let id = st.server.running().core().jobs().unwrap()[0].id;
    let token = st.runner_token();
    st.finish_done(id, &token);
    assert_eq!(st.bot.notify_once().unwrap(), 1);
    assert_eq!(st.tg_job(id).delivery, "pending");
    // Процесс упал внутри отправки.
    sql(&st, "UPDATE tg_jobs SET delivery = 'sending'");

    let bot = restarted(&st);
    bot.recover().unwrap();
    let tj = st.tg_job(id);
    assert_eq!(tj.delivery, "uncertain");
    assert!(tj.delivery_error.unwrap().contains("перезапустился"));
    bot.step().unwrap();
    bot.step().unwrap();
    assert_eq!(
        st.tg_job(id).delivery,
        "uncertain",
        "статус не меняется сам"
    );
    assert!(
        !st.tg.calls().iter().any(|c| c.method == "send_video"),
        "после перезапуска ролик вслепую не отправляется"
    );
    assert!(st.tg.videos().is_empty());
}

#[test]
fn poll_does_not_call_get_file() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    assert_eq!(st.bot.poll_once(0).unwrap(), 1);
    assert_eq!(
        st.tg.get_file_calls(),
        0,
        "getFile случается только в ingest_once"
    );
    assert!(st.bot.ingest_once().unwrap());
    assert_eq!(st.tg.get_file_calls(), 1);
    assert!(!st.bot.ingest_once().unwrap(), "ждать больше нечего");
}

#[test]
fn document_without_player_asks_for_player_and_queues_nothing() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "");
    st.step();
    let texts = st.tg.texts_to(FRIEND);
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains("/player"), "{}", texts[0]);
    assert_eq!(inbox_len(&st), 0, "строки во входящих нет");
    assert_eq!(jobs_len(&st), 0, "задач нет");
    assert!(
        store::offset(st.server.running().core().db()).unwrap() > 0,
        "смещение сдвинулось"
    );
    assert_eq!(st.tg.get_file_calls(), 0);
}

#[test]
fn saved_player_is_used_when_the_caption_is_empty() {
    let st = BotStand::start();
    let uid = st.add_friend(FRIEND, "вася");
    sql(
        &st,
        &format!("UPDATE users SET player_name = 'ZywOo' WHERE id = {uid}"),
    );
    st.send_demo(FRIEND, "");
    st.step();
    let jobs = st.server.running().core().jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].player_name, "ZywOo");
}

#[test]
fn stranger_document_is_never_fetched() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    st.send_demo(999, "s1mple");
    st.step();
    let texts = st.tg.texts_to(999);
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(
        texts[0].contains("999"),
        "в ответе ID незнакомца: {}",
        texts[0]
    );
    assert_eq!(
        st.tg.get_file_calls(),
        0,
        "getFile незнакомцу не вызывается"
    );
    assert_eq!(inbox_len(&st), 0);
    assert_eq!(jobs_len(&st), 0);

    // Второй файл сразу после первого остаётся без ответа, через десять минут ответ снова приходит.
    st.send_demo(999, "s1mple");
    st.step();
    assert_eq!(st.tg.texts_to(999).len(), 1, "не чаще раза за период");
    st.clock().advance(Duration::from_secs(601));
    st.send_demo(999, "s1mple");
    st.step();
    assert_eq!(st.tg.texts_to(999).len(), 2);
    assert_eq!(st.tg.get_file_calls(), 0);
}

#[test]
fn offset_is_last_update_id_plus_one_and_reaches_the_next_poll() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let a = st.tg.text_update(FRIEND, FRIEND, "привет");
    let b = st.tg.text_update(FRIEND, FRIEND, "ещё раз");
    let last = b.update_id;
    st.tg.push_update(a);
    st.tg.push_update(b);
    assert_eq!(st.step().updates, 2);
    assert_eq!(
        store::offset(st.server.running().core().db()).unwrap(),
        last + 1
    );
    assert_eq!(st.step().updates, 0, "обработанное не приходит снова");
    assert_eq!(st.tg.last_offset(), last + 1);
}
