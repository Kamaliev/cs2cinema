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
