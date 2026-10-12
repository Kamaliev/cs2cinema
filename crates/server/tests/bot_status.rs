//! Ход задачи в чате и управление ею (план 07-08): одно сообщение статуса с ограничением частоты, `/status` и
//! `/queue`, ошибки правки по чатам, `/cancel` и `/retry`. Telegram подставной, сервер и база настоящие, время
//! двигают ручные часы, раннер изображают HTTP-помощники стенда.

mod common;

use common::BotStand;
use server::clock::Clock;
use server::tg::TgError;
use std::time::Duration;
use wire::{
    CancelMode, FinishRequest, JobState, Outcome, Phase, ProgressRequest, ReleaseRequest, Take,
};

const A: i64 = 111;
const B: i64 = 222;
const ADMIN: i64 = 999;

/// Друг пишет боту текст; обработает его следующий `step()`.
fn say(st: &BotStand, from: i64, text: &str) {
    st.tg.push_update(st.tg.text_update(from, from, text));
}

/// Друг пишет и бот отвечает (один шаг).
fn ask(st: &BotStand, from: i64, text: &str) -> String {
    say(st, from, text);
    st.step();
    last_text(st, from)
}

fn last_text(st: &BotStand, chat: i64) -> String {
    st.tg
        .texts_to(chat)
        .pop()
        .unwrap_or_else(|| "<ничего не пришло>".to_owned())
}

fn secs(st: &BotStand, n: u64) {
    st.clock().advance(Duration::from_secs(n));
}

/// Сколько раз бот правил сообщения в этом чате (удачные и неудачные попытки).
fn edit_calls(st: &BotStand, chat: i64) -> usize {
    st.tg
        .calls()
        .iter()
        .filter(|c| c.method == "edit_message_text" && c.chat == Some(chat))
        .count()
}

/// Раннер забирает следующую задачу из очереди; возвращает её номер и эпоху.
fn claim(st: &BotStand, token: &str) -> (i64, i64) {
    let (status, got) = st.server.claim(token, 0);
    assert_eq!(status, 200, "раннер должен получить задачу");
    let a = got.unwrap();
    (a.job_id, a.epoch)
}

/// Прогресс раннера; возвращает запрошенную отмену из ответа.
fn progress(
    st: &BotStand,
    token: &str,
    job_id: i64,
    epoch: i64,
    seq: u64,
    phase: Phase,
    take: Option<(u32, u32)>,
) -> CancelMode {
    let req = ProgressRequest {
        epoch,
        seq,
        phase,
        take: take.map(|(n, m)| Take { n, m }),
        status: String::new(),
    };
    let (status, body) = st.server.progress(token, job_id, &req);
    assert_eq!(status, 200, "{body}");
    serde_json::from_str::<wire::ProgressReply>(&body)
        .unwrap()
        .cancel
}

fn job_state(st: &BotStand, id: i64) -> JobState {
    st.server.job(id).state
}

fn texts_equal(st: &BotStand, chat: i64, wanted: &str) -> usize {
    st.tg
        .texts_to(chat)
        .iter()
        .filter(|t| t.as_str() == wanted)
        .count()
}

// ---------------------------------------------------------------------------------------------------------
// Task 1: сообщение статуса
// ---------------------------------------------------------------------------------------------------------

#[test]
fn status_message_is_edited_at_most_every_ten_seconds_with_stage_and_take() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.send_demo(A, "s1mple");
    st.step();
    let job = st.server.running().core().jobs().unwrap()[0].id;
    let token = st.runner_token();
    let (id, epoch) = claim(&st, &token);
    assert_eq!(id, job);
    let t0 = st.clock().now_ms();
    let before = edit_calls(&st, A);

    // Каждые 2 секунды раннер шлёт новый дубль записи; бот шагает после каждого прогресса.
    for k in 1..=5u32 {
        secs(&st, 2);
        progress(
            &st,
            &token,
            job,
            epoch,
            u64::from(k),
            Phase::Record,
            Some((k, 5)),
        );
        st.step();
        if k < 5 {
            let text = st.tg.texts_to(A)[0].clone();
            assert!(
                !text.contains("запись"),
                "до 10 секунд статус не правится (k={k}): {text}"
            );
        }
    }
    assert_eq!(st.clock().now_ms() - t0, 10_000);
    let in_window = edit_calls(&st, A) - before;
    assert!(
        (1..=2).contains(&in_window),
        "за 10 с не больше двух правок, было {in_window}"
    );
    let text = st.tg.texts_to(A)[0].clone();
    assert!(text.contains("запись"), "{text}");
    assert!(text.contains("дубль 5 из 5"), "{text}");
    assert_eq!(
        st.tg.texts_to(A).len(),
        1,
        "статус правится, а не дублируется"
    );

    // Этап сменился, но 10 секунд ещё не прошло: текст прежний.
    secs(&st, 2);
    progress(&st, &token, job, epoch, 6, Phase::Assemble, None);
    st.step();
    assert!(st.tg.texts_to(A)[0].contains("запись"));
    assert_eq!(edit_calls(&st, A) - before, in_window, "правки нет");

    // Прошло 10 секунд с прошлой правки: появилась «склейка».
    secs(&st, 10);
    st.step();
    let text = st.tg.texts_to(A)[0].clone();
    assert!(text.contains("склейка"), "{text}");
    let after = edit_calls(&st, A);

    // Без изменений задачи правок нет.
    for _ in 0..3 {
        st.step();
        secs(&st, 5);
    }
    st.step();
    assert_eq!(edit_calls(&st, A), after);

    // Раннер замолчал дольше окна «в сети»: это изменение текста, и бот честно говорит об этом.
    secs(&st, 60);
    st.step();
    let text = st.tg.texts_to(A)[0].clone();
    assert!(text.contains("Раннер сейчас не в сети"), "{text}");
}

// ---------------------------------------------------------------------------------------------------------
// Task 2: /status, /queue, офлайн-раннер, ошибки правки
// ---------------------------------------------------------------------------------------------------------

#[test]
fn status_and_queue_show_places_and_an_offline_runner() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.add_friend(B, "петя");
    st.add_friend(333, "оля");
    for (from, player) in [(A, "p1"), (A, "p2"), (B, "p3")] {
        st.send_demo(from, player);
        st.step();
    }
    let status = ask(&st, A, "/status");
    assert!(status.contains("#1"), "{status}");
    assert!(status.contains("#2"), "{status}");
    assert!(!status.contains("#3"), "чужая задача не видна: {status}");
    let line = |id: i64| -> String {
        status
            .lines()
            .find(|l| l.starts_with(&format!("#{id}")))
            .unwrap_or_else(|| panic!("нет строки задачи #{id}: {status}"))
            .to_owned()
    };
    assert!(line(1).contains("перед вами 0"), "{status}");
    assert!(line(2).contains("перед вами 1"), "{status}");
    assert!(status.contains("Раннер сейчас не в сети"), "{status}");

    let queue = ask(&st, A, "/queue");
    assert!(queue.contains("В очереди 3, в работе 0"), "{queue}");
    assert!(queue.contains("#1") && queue.contains("#2"), "{queue}");
    assert!(!queue.contains("#3"), "чужая задача не видна: {queue}");
    assert!(queue.contains("Раннер сейчас не в сети"), "{queue}");

    // Раннер взял первую задачу: офлайн-строки нет, у выданной задачи этап «выдана раннеру».
    let token = st.runner_token();
    assert_eq!(claim(&st, &token).0, 1);
    let status = ask(&st, A, "/status");
    assert!(!status.contains("не в сети"), "{status}");
    assert!(status.contains("выдана раннеру"), "{status}");
    assert!(status.contains("перед вами 1"), "{status}");
    let queue = ask(&st, A, "/queue");
    assert!(queue.contains("В очереди 2, в работе 1"), "{queue}");
    assert!(!queue.contains("не в сети"), "{queue}");

    // У друга без задач — прямой ответ.
    let none = ask(&st, 333, "/status");
    assert!(none.contains("У вас нет активных задач"), "{none}");
}

#[test]
fn offline_runner_is_shown_in_the_status_message() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.send_demo(A, "s1mple");
    st.step();
    let text = st.tg.texts_to(A)[0].clone();
    assert!(text.contains("Раннер сейчас не в сети"), "{text}");
    assert!(text.contains("в очереди"), "{text}");

    // Раннер появился (взял задачу): через 10 секунд строка пропадает.
    let token = st.runner_token();
    claim(&st, &token);
    st.step();
    assert!(
        st.tg.texts_to(A)[0].contains("не в сети"),
        "до 10 секунд статус не правится"
    );
    secs(&st, 10);
    st.step();
    let text = st.tg.texts_to(A)[0].clone();
    assert!(!text.contains("не в сети"), "{text}");
    assert!(text.contains("выдана раннеру"), "{text}");
}

#[test]
fn edit_errors_are_handled_per_chat() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.add_friend(B, "петя");
    st.send_demo(A, "p1");
    st.step();
    st.send_demo(B, "p2");
    st.step();
    let (job_a, job_b) = (1, 2);
    let r1 = st.runner_token();
    let r2 = st.server.add_runner("second");
    let (got, epoch_a) = claim(&st, &r1);
    assert_eq!(got, job_a);
    let (got, epoch_b) = claim(&st, &r2);
    assert_eq!(got, job_b);
    let api = |code: i64, text: &str, retry_after: Option<u64>| TgError::Api {
        code,
        description: text.to_owned(),
        retry_after,
    };

    // t = 10: у обоих чатов текст изменился. Первая правка (чат A) получает 429 на 30 секунд.
    secs(&st, 10);
    st.tg.fail_next(
        "edit_message_text",
        api(429, "Too Many Requests: retry after 30", Some(30)),
    );
    st.step();
    let a_texts = st.tg.texts_to(A);
    assert!(
        a_texts[0].contains("в очереди"),
        "чат A не поправлен: {a_texts:?}"
    );
    let b_texts = st.tg.texts_to(B);
    assert!(
        b_texts[0].contains("выдана раннеру"),
        "чат B правится по своему графику: {b_texts:?}"
    );
    let a_calls = edit_calls(&st, A);

    // t = 20: у чата B сообщение пропало — приходит новое, его номер сохранён.
    secs(&st, 10);
    progress(&st, &r2, job_b, epoch_b, 1, Phase::Record, Some((1, 3)));
    st.tg.fail_next(
        "edit_message_text",
        api(400, "Bad Request: message to edit not found", None),
    );
    let old_id = st.tg_job(job_b).status_message_id.unwrap();
    st.step();
    let new_id = st.tg_job(job_b).status_message_id.unwrap();
    assert_ne!(old_id, new_id, "новое сообщение статуса");
    let b_texts = st.tg.texts_to(B);
    assert_eq!(b_texts.len(), 2, "{b_texts:?}");
    assert!(b_texts[1].contains("дубль 1 из 3"), "{b_texts:?}");
    assert_eq!(edit_calls(&st, A), a_calls, "чат A ждёт свои 30 секунд");

    // t = 39: 29 секунд после 429 правки в чат A ещё нет, хотя текст другой.
    secs(&st, 19);
    st.step();
    assert_eq!(edit_calls(&st, A), a_calls, "раньше retry_after не правим");
    assert!(st.tg.texts_to(A)[0].contains("в очереди"));

    // t = 40: срок вышел; «message is not modified» считается успехом: текст запомнен, повторов нет.
    secs(&st, 1);
    progress(&st, &r1, job_a, epoch_a, 1, Phase::Parse, None);
    st.tg.fail_next(
        "edit_message_text",
        api(400, "Bad Request: message is not modified", None),
    );
    st.step();
    assert_eq!(edit_calls(&st, A), a_calls + 1);
    assert!(
        st.tg_job(job_a).status_text.contains("разбор демки"),
        "{:?}",
        st.tg_job(job_a).status_text
    );
    st.step();
    secs(&st, 5);
    st.step();
    assert_eq!(
        edit_calls(&st, A),
        a_calls + 1,
        "после «not modified» правок нет"
    );

    // t = 50: чат B заблокировал бота (403) — он заглушён, остальные чаты не страдают.
    secs(&st, 5);
    progress(&st, &r2, job_b, epoch_b, 2, Phase::Record, Some((2, 3)));
    st.tg.fail_next(
        "edit_message_text",
        api(403, "Forbidden: bot was blocked by the user", None),
    );
    st.step();
    assert!(st.tg_job(job_b).muted, "чат B заглушён");
    assert!(!st.tg_job(job_a).muted);
    let b_calls = edit_calls(&st, B);

    // t = 70: оба задачи меняются; правка уходит только в чат A.
    secs(&st, 20);
    progress(&st, &r2, job_b, epoch_b, 3, Phase::Record, Some((3, 3)));
    progress(&st, &r1, job_a, epoch_a, 2, Phase::Record, Some((1, 2)));
    st.step();
    assert_eq!(edit_calls(&st, B), b_calls, "заглушённому чату не правим");
    assert!(
        st.tg.texts_to(A)[0].contains("дубль 1 из 2"),
        "{:?}",
        st.tg.texts_to(A)
    );
}

// ---------------------------------------------------------------------------------------------------------
// Task 3: /cancel и /retry
// ---------------------------------------------------------------------------------------------------------

#[test]
fn cancel_stops_a_queued_job_and_flags_a_running_one() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.send_demo(A, "p1");
    st.step();
    st.send_demo(A, "p2");
    st.step();
    let token = st.runner_token();
    let (job1, epoch) = claim(&st, &token);
    assert_eq!(job1, 1);
    progress(&st, &token, 1, epoch, 1, Phase::Record, Some((1, 4)));

    // Без номера — самая свежая активная задача (вторая, она ещё в очереди): отменена сразу.
    let reply = ask(&st, A, "/cancel");
    assert!(reply.contains("Задача #2 отменена"), "{reply}");
    assert_eq!(job_state(&st, 2), JobState::Cancelled);
    st.step();
    st.step();
    assert_eq!(
        texts_equal(&st, A, "Задача #2 отменена."),
        1,
        "об отмене сказано один раз: {:?}",
        st.tg.texts_to(A)
    );
    assert!(st.tg_job(2).final_reported_ms.is_some());

    // Первая у раннера: мягкий флаг, затем жёсткий.
    let reply = ask(&st, A, "/cancel 1");
    assert!(
        reply.contains("Отменяю задачу #1 — раннер остановит запись"),
        "{reply}"
    );
    assert_eq!(st.server.job(1).cancel_requested, CancelMode::Soft);
    assert_eq!(job_state(&st, 1), JobState::Running);
    let reply = ask(&st, A, "/cancel 1");
    assert!(reply.contains("жёстко"), "{reply}");
    assert_eq!(st.server.job(1).cancel_requested, CancelMode::Force);

    // Раннер узнаёт об отмене из ответа на прогресс и завершает задачу.
    let cancel = progress(&st, &token, 1, epoch, 2, Phase::Record, Some((2, 4)));
    assert_eq!(cancel, CancelMode::Force);
    let (code, body) = st.server.finish(
        &token,
        1,
        &FinishRequest {
            epoch,
            outcome: Outcome::Cancelled,
        },
    );
    assert_eq!(code, 200, "{body}");
    assert_eq!(job_state(&st, 1), JobState::Cancelled);
    st.step();
    st.step();
    secs(&st, 30);
    st.step();
    assert_eq!(
        texts_equal(&st, A, "Задача #1 отменена."),
        1,
        "{:?}",
        st.tg.texts_to(A)
    );
    assert!(st.tg_job(1).final_reported_ms.is_some());
    // Сообщение статуса тоже сказало «отменена».
    let status_id = st.tg_job(1).status_message_id.unwrap();
    assert!(status_id > 0);
    assert!(
        st.tg.texts_to(A).iter().any(|t| t == "Задача #1 отменена"),
        "{:?}",
        st.tg.texts_to(A)
    );
}

#[test]
fn cancel_then_release_is_cancelled_not_requeued() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.send_demo(A, "p1");
    st.step();
    let token = st.runner_token();
    let (job, epoch) = claim(&st, &token);
    progress(&st, &token, job, epoch, 1, Phase::Record, None);
    let reply = ask(&st, A, "/cancel");
    assert!(reply.contains("Отменяю задачу #1"), "{reply}");

    let (code, body) = st.server.release(
        &token,
        job,
        &ReleaseRequest {
            epoch,
            reason: "раннер уходит".to_owned(),
            counts: false,
        },
    );
    assert!(code == 200 || code == 204, "{code}: {body}");
    assert_eq!(
        job_state(&st, job),
        JobState::Cancelled,
        "не вернулась в очередь"
    );
    let (status, next) = st.server.claim(&token, 0);
    assert!(
        next.is_none() && status == 204,
        "в очереди пусто, а статус {status}"
    );
    st.step();
    assert_eq!(
        texts_equal(&st, A, "Задача #1 отменена."),
        1,
        "{:?}",
        st.tg.texts_to(A)
    );
}

#[test]
fn retry_requeues_from_the_stored_demo_without_get_file() {
    let st = BotStand::start();
    let uid = st.add_friend(A, "вася");
    let uid_b = st.add_friend(B, "петя");
    st.send_demo(A, "s1mple");
    st.step();
    let token = st.runner_token();
    st.finish_failed(1, &token, "RECORD_TIMEOUT");
    st.step(); // отчёт о сбое (план 07-09) приходит до команды, а не вместе с ней
    assert_eq!(job_state(&st, 1), JobState::Failed);
    let downloads = st.tg.get_file_calls();
    assert_eq!(downloads, 1);

    let reply = ask(&st, A, "/retry");
    assert!(reply.contains("повтор задачи #1"), "{reply}");
    assert_eq!(st.tg.get_file_calls(), downloads, "getFile не вызывался");
    let core = st.server.running().core();
    let new = core.job(2).unwrap().expect("новая задача");
    assert_eq!(new.state, JobState::Queued);
    assert_eq!(new.player_name, "s1mple");
    assert_eq!(new.user_id, Some(uid));
    assert_eq!(
        core.demo_file(2).unwrap().unwrap().sha256,
        core.demo_file(1).unwrap().unwrap().sha256,
        "та же демка"
    );
    let tj = st.tg_job(2);
    assert_eq!(tj.chat_id, A);
    assert!(tj.status_message_id.is_some());
    assert_eq!(st.tg.texts_to(A).last().unwrap(), &tj.status_text);

    // Задача FACEIT повторяется с той же ссылкой. Сначала раннер заканчивает вторую, чтобы получить следующую.
    st.finish_failed(2, &token, "RECORD_TIMEOUT");
    let link = "https://www.faceit.com/en/cs2/room/1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
    let faceit = core
        .submit_faceit(
            link,
            &server::core::Submit {
                player: "zywoo".to_owned(),
                user_id: Some(uid_b),
                requested_by: "петя".to_owned(),
                orig_name: String::new(),
            },
        )
        .unwrap();
    server::bot::store::insert_tg_job(
        core.db(),
        &server::bot::store::NewTgJob {
            job_id: faceit,
            chat_id: B,
            user_id: uid_b,
            status_message_id: None,
            status_text: "",
            next_edit_ms: 0,
        },
        st.clock().now_ms(),
    )
    .unwrap();
    st.finish_failed(faceit, &token, "FACEIT_FETCH_FAILED");
    st.step(); // отчёт о сбое (план 07-09) приходит до команды, а не вместе с ней
    let reply = ask(&st, B, "/retry");
    assert!(
        reply.contains(&format!("повтор задачи #{faceit}")),
        "{reply}"
    );
    let again = core.jobs().unwrap().into_iter().last().unwrap();
    assert_ne!(again.id, faceit);
    assert_eq!(again.source_kind, "faceit");
    assert_eq!(again.faceit_link.as_deref(), Some(link));
    assert_eq!(again.state, JobState::Queued);
    assert_eq!(
        st.tg.get_file_calls(),
        downloads,
        "getFile так и не вызывался"
    );
}

#[test]
fn retry_respects_limits_and_ownership() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.add_friend(B, "петя");
    st.add_admin(ADMIN, "автор");
    let token = st.runner_token();

    // Задача 1 друга A упала, задача 2 друга B готова.
    st.send_demo(A, "p1");
    st.step();
    st.finish_failed(1, &token, "RECORD_TIMEOUT");
    st.send_demo(B, "pb");
    st.step();
    st.finish_done(2, &token);
    st.step();
    // У A две активные задачи (3 и 4).
    st.send_demo(A, "p2");
    st.step();
    st.send_demo(A, "p3");
    st.step();
    let jobs = st.server.running().core().jobs().unwrap().len();
    assert_eq!(jobs, 4);

    // Лимит D-03: третья активная задача не ставится.
    let reply = ask(&st, A, "/retry 1");
    assert!(reply.contains("У вас уже 2 активные задачи"), "{reply}");
    assert_eq!(st.server.running().core().jobs().unwrap().len(), jobs);

    // Чужие задачи: не повторить и не отменить.
    let reply = ask(&st, A, "/retry 2");
    assert!(reply.contains("Это не ваша задача"), "{reply}");
    let reply = ask(&st, B, "/cancel 3");
    assert!(reply.contains("Это не ваша задача"), "{reply}");
    assert_eq!(job_state(&st, 3), JobState::Queued);
    let reply = ask(&st, B, "/cancel 99");
    assert!(reply.contains("Это не ваша задача"), "{reply}");
    let reply = ask(&st, B, "/status");
    assert!(!reply.contains("#3") && !reply.contains("#4"), "{reply}");

    // Готовая задача — подсказка про /resend; идущая — «ещё идёт».
    let reply = ask(&st, B, "/retry 2");
    assert!(reply.contains("/resend 2"), "{reply}");
    let reply = ask(&st, A, "/retry 3");
    assert!(reply.contains("ещё идёт"), "{reply}");
    assert_eq!(st.server.running().core().jobs().unwrap().len(), jobs);

    // Неверный формат.
    let reply = ask(&st, A, "/cancel abc");
    assert!(reply.contains("Формат: /cancel"), "{reply}");
    let reply = ask(&st, A, "/retry -3");
    assert!(reply.contains("Формат: /retry"), "{reply}");

    // Админ отменяет чужую задачу, владелец узнаёт об этом; повторить чужую админ не может.
    let reply = ask(&st, ADMIN, "/cancel 3");
    assert!(reply.contains("Задача #3 отменена"), "{reply}");
    assert_eq!(job_state(&st, 3), JobState::Cancelled);
    st.step();
    assert_eq!(
        texts_equal(&st, A, "Задача #3 отменена."),
        1,
        "{:?}",
        st.tg.texts_to(A)
    );
    let reply = ask(&st, ADMIN, "/retry 1");
    assert!(reply.contains("Это не ваша задача"), "{reply}");
}

/// CR-02: строка `tg_jobs` создаётся до вызова Telegram. Подтверждение не ушло, а задача уже в очереди и
/// записана для доставки; статус пришлёт тик.
#[test]
fn retry_records_the_delivery_row_even_when_the_confirmation_is_lost() {
    let st = BotStand::start();
    st.add_friend(A, "вася");
    st.send_demo(A, "s1mple");
    st.step();
    let token = st.runner_token();
    st.finish_failed(1, &token, "RECORD_TIMEOUT");
    st.step();

    st.tg.fail_next(
        "send_message",
        TgError::Io {
            sent: false,
            detail: "обрыв".to_owned(),
        },
    );
    say(&st, A, "/retry");
    st.step();
    assert_eq!(job_state(&st, 2), JobState::Queued);
    let tj = st.tg_job(2);
    assert_eq!(tj.chat_id, A);
    assert!(tj.status_message_id.is_none(), "подтверждение не дошло");

    secs(&st, 60);
    st.step();
    assert!(
        st.tg_job(2).status_message_id.is_some(),
        "тик сам присылает сообщение статуса"
    );
}
