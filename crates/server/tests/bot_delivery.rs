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
    assert!(texts.contains(&want), "{texts:?}");
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

// ---------------------------------------------------------------------------------------------------------
// Отчёты о сбое задачи (Task 3)
// ---------------------------------------------------------------------------------------------------------

const ADMIN: i64 = 900;

/// Фейковый раннер берёт задачу и завершает её сбоем с заданным содержимым.
fn fail_job(st: &BotStand, id: i64, f: wire::Failure) {
    let token = st.runner_token();
    let (status, got) = st.server.claim(&token, 0);
    assert_eq!(status, 200, "раннер должен получить задачу");
    let a = got.unwrap();
    assert_eq!(a.job_id, id, "выдана не та задача");
    let req = wire::ProgressRequest {
        epoch: a.epoch,
        seq: 1,
        phase: wire::Phase::Record,
        take: None,
        status: String::new(),
    };
    assert_eq!(st.server.progress(&token, id, &req).0, 200);
    let (status, body) = st.server.finish(
        &token,
        id,
        &wire::FinishRequest {
            epoch: a.epoch,
            outcome: wire::Outcome::Failed(f),
        },
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(st.server.job(id).state, JobState::Failed);
}

#[test]
fn failure_sends_a_friend_phrase_and_an_admin_report() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    let id = queue_job(&st, FRIEND, "s1mple");
    let status_id = st.tg_job(id).status_message_id.expect("сообщение статуса");

    fail_job(
        &st,
        id,
        wire::Failure {
            code: wire::fail::PLAYER_NOT_FOUND.to_owned(),
            class: "other".to_owned(),
            state: "precheck".to_owned(),
            hint: "проверьте ник игрока в демке".to_owned(),
            message: "C:\\Users\\author\\out\\srv5: игрок не найден".to_owned(),
            tail: "хвост консоли".to_owned(),
            elapsed_ms: 1000,
        },
    );
    st.step();

    // Друг: новое сообщение с фразой про ник и /retry, без путей, кода и этапа.
    let texts = st.tg.texts_to(FRIEND);
    let report: Vec<_> = texts
        .iter()
        .filter(|t| t.contains(&format!("Задача #{id} не получилась")))
        .collect();
    assert_eq!(report.len(), 1, "{texts:?}");
    assert!(report[0].contains("ник"), "{}", report[0]);
    assert!(report[0].contains(&format!("/retry {id}")), "{}", report[0]);
    for secret in ["C:\\", "Users", "PLAYER_NOT_FOUND", "precheck", "хвост"] {
        assert!(!report[0].contains(secret), "{secret:?} в {}", report[0]);
    }
    assert!(
        !st.tg.message_ids_to(FRIEND).is_empty()
            && texts.iter().any(|t| *t == format!("Задача #{id}: сбой")),
        "сообщение статуса правится на «сбой»: {texts:?} (статус #{status_id})"
    );

    // Админ: одно сообщение с кодом, этапом, подсказкой и числом выдач.
    let admin = st.tg.texts_to(ADMIN);
    assert_eq!(admin.len(), 1, "{admin:?}");
    for want in [
        &format!("#{id}"),
        "вася",
        "s1mple",
        "PLAYER_NOT_FOUND",
        "precheck",
        "проверьте ник игрока в демке",
        "выдач раннеру: 1",
    ] {
        assert!(admin[0].contains(want), "{want:?} нет в {}", admin[0]);
    }
    assert!(!admin[0].contains("хвост консоли"), "хвост в чат не идёт");

    // Повторные тики ничего не дублируют.
    st.clock().advance(Duration::from_secs(60));
    st.step();
    st.step();
    assert_eq!(st.tg.texts_to(FRIEND), texts);
    assert_eq!(st.tg.texts_to(ADMIN).len(), 1);
    let tj = st.tg_job(id);
    assert!(tj.final_reported_ms.is_some() && tj.admin_reported_ms.is_some());
}

#[test]
fn server_side_failures_are_reported_too() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    let id = queue_job(&st, FRIEND, "s1mple");
    // Задачу трижды выдавали и ни разу не довели: четвёртая выдача её роняет.
    st.server
        .running()
        .core()
        .db()
        .write(|tx| {
            tx.execute("UPDATE jobs SET grants = 3 WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let token = st.runner_token();
    let _ = st.server.claim(&token, 0);
    let job = st.server.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("TOO_MANY_GRANTS"));
    st.step();

    let friend = st.tg.texts_to(FRIEND);
    assert!(
        friend.iter().any(|t| t.contains("срывалась")),
        "фраза про срывы записи: {friend:?}"
    );
    assert!(
        friend.iter().all(|t| !t.contains("TOO_MANY_GRANTS")),
        "{friend:?}"
    );
    let admin = st.tg.texts_to(ADMIN);
    assert_eq!(admin.len(), 1, "{admin:?}");
    assert!(admin[0].contains("TOO_MANY_GRANTS"), "{}", admin[0]);
    assert!(admin[0].contains("выдач раннеру: 3"), "{}", admin[0]);
}

#[test]
fn admin_owner_gets_both_messages_and_a_muted_friend_only_the_admin_report() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    let own = queue_job(&st, ADMIN, "s1mple");
    let friends = queue_job(&st, FRIEND, "ZywOo");
    // Друг заблокировал бота: чат заглушён.
    st.server
        .running()
        .core()
        .db()
        .write(|tx| {
            tx.execute("UPDATE tg_jobs SET muted = 1 WHERE chat_id = ?1", [FRIEND])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let failure = |code: &str| wire::Failure {
        code: code.to_owned(),
        class: "launch".to_owned(),
        state: "launching".to_owned(),
        hint: String::new(),
        message: "сбой для теста".to_owned(),
        tail: String::new(),
        elapsed_ms: 10,
    };
    fail_job(&st, own, failure(wire::fail::GAME_NOT_STARTED));
    fail_job(&st, friends, failure(wire::fail::HLAE_ERROR));
    let friend_before = st.tg.texts_to(FRIEND).len();
    st.step();

    let admin = st.tg.texts_to(ADMIN);
    assert!(
        admin
            .iter()
            .any(|t| t.contains(&format!("Задача #{own} не получилась"))),
        "владелец-админ получает сообщение друга: {admin:?}"
    );
    assert!(
        admin
            .iter()
            .any(|t| t.starts_with(&format!("Сбой задачи #{own}"))),
        "и отчёт админа: {admin:?}"
    );
    assert!(
        admin
            .iter()
            .any(|t| t.starts_with(&format!("Сбой задачи #{friends}"))),
        "отчёт о задаче друга: {admin:?}"
    );
    assert_eq!(
        st.tg.texts_to(FRIEND).len(),
        friend_before,
        "заглушённому чату не пишем"
    );
    assert!(st.tg_job(friends).admin_reported_ms.is_some());
}

/// WR-08: `/resend` доставленного ролика не отправляет его из потока опроса. Команда только возвращает доставку в
/// `pending`; отправку по `file_id` делает поток доставки, а если Telegram `file_id` не принял, ролик уходит с файла.
#[test]
fn resend_of_a_sent_video_is_left_to_the_delivery_thread() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let id = done_job(&st, FRIEND, "s1mple");
    st.step();
    let file_id = st.tg_job(id).result_file_id.clone().expect("file_id");
    assert_eq!(send_video_calls(&st), 1);

    // Опрос обработал команду, доставки ещё не было.
    say(&st, FRIEND, &format!("/resend {id}"));
    st.bot.poll_once(0).unwrap();
    assert_eq!(st.tg_job(id).delivery, "pending");
    assert_eq!(
        send_video_calls(&st),
        1,
        "из потока опроса ничего не уходит"
    );

    // Telegram не принял file_id: запрос точно не сработал, ролик идёт с файла.
    st.tg.fail_next(
        "send_video",
        TgError::Api {
            code: 400,
            description: "Bad Request: wrong file identifier".to_owned(),
            retry_after: None,
        },
    );
    st.step();
    let calls: Vec<_> = st
        .tg
        .calls()
        .into_iter()
        .filter(|c| c.method == "send_video")
        .collect();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[1].detail, format!("id:{file_id}"));
    assert!(calls[2].detail.starts_with("file:"), "{:?}", calls[2]);
    assert_eq!(st.tg_job(id).delivery, "sent");
    assert_eq!(videos_to(&st, FRIEND), 2);
}

/// WR-09: отчёт админу не теряется, если сообщение точно не ушло: отметка снимается и тик повторяет отправку.
#[test]
fn admin_report_is_retried_when_the_send_definitely_failed() {
    let st = BotStand::start();
    st.add_admin(ADMIN, "автор");
    st.add_friend(FRIEND, "вася");
    let id = queue_job(&st, FRIEND, "s1mple");
    let io = || TgError::Io {
        sent: false,
        detail: "нет сети".to_owned(),
    };
    // Первое сообщение тика уходит другу, второе админу: оба не доходят.
    st.tg.fail_next("send_message", io());
    st.tg.fail_next("send_message", io());
    fail_job(
        &st,
        id,
        wire::Failure {
            code: wire::fail::PLAYER_NOT_FOUND.to_owned(),
            class: "other".to_owned(),
            state: "precheck".to_owned(),
            hint: String::new(),
            message: "игрок не найден".to_owned(),
            tail: String::new(),
            elapsed_ms: 1000,
        },
    );
    st.step();
    assert!(
        st.tg.texts_to(ADMIN).is_empty(),
        "отчёт ещё не дошёл: {:?}",
        st.tg.texts_to(ADMIN)
    );
    assert!(st.tg_job(id).admin_reported_ms.is_none(), "отметка снята");

    st.step();
    let admin = st.tg.texts_to(ADMIN);
    assert_eq!(admin.len(), 1, "отчёт пришёл со второй попытки: {admin:?}");
    assert!(st.tg_job(id).admin_reported_ms.is_some());
    st.step();
    assert_eq!(st.tg.texts_to(ADMIN).len(), 1, "и больше не дублируется");
}

/// WR-10: если итог отправки не записывается, строка не остаётся в `sending`: ролик помечается неясным, друг получает
/// сообщение про `/resend`, а шаг бота не падает.
#[test]
fn lost_outcome_write_turns_into_uncertain_and_a_message() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let id = queue_job(&st, FRIEND, "s1mple");
    finish_job(&st, id);
    st.server
        .running()
        .core()
        .db()
        .write(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER no_sent BEFORE UPDATE OF delivery ON tg_jobs WHEN NEW.delivery = 'sent' \
                 BEGIN SELECT RAISE(ABORT, 'database is locked'); END",
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();

    st.step();
    assert_eq!(videos_to(&st, FRIEND), 1, "ролик ушёл один раз");
    assert_eq!(st.tg_job(id).delivery, "uncertain");
    let want = format!(
        "Не уверен, что ролик задачи #{id} дошёл. Если его нет в чате — отправьте /resend {id}"
    );
    assert!(st.tg.texts_to(FRIEND).contains(&want));
    st.step();
    assert_eq!(videos_to(&st, FRIEND), 1, "слепого повтора нет");
}
