//! Приём демки ботом (фаза 7, план 06): повторы и несколько игроков на одной демке, лимиты и размер до `getFile`,
//! магия после скачивания, восстановление после падения и безопасный путь файла из bot-api.
//! Telegram подставной (`FakeTelegram`), сервер и база настоящие, время двигают ручные часы.

mod common;

use common::{BotStand, TestDir, fake_demo};
use server::bot::store;
use std::path::{Path, PathBuf};
use wire::JobState;

const FRIEND: i64 = 111;
const MIB: u64 = 1024 * 1024;

/// Кладёт документ на диск фейка и регистрирует его для `getFile`. Возвращает путь файла на диске.
fn put(st: &BotStand, file_id: &str, unique: &str, bytes: &[u8]) -> PathBuf {
    st.tg.put_document(file_id, unique, bytes)
}

/// Толкает обновление «прислал документ»; `size = None` — документ без размера.
fn push(
    st: &BotStand,
    from: i64,
    file_id: &str,
    unique: &str,
    name: &str,
    size: Option<u64>,
    caption: &str,
) {
    let caption = (!caption.is_empty()).then_some(caption);
    let mut upd = st.tg.document_update(
        from,
        from,
        file_id,
        unique,
        name,
        size.unwrap_or(0),
        caption,
    );
    if size.is_none() {
        upd.message
            .as_mut()
            .and_then(|m| m.document.as_mut())
            .expect("документ")
            .file_size = None;
    }
    st.tg.push_update(upd);
}

/// Обычная демка на 4096 байт.
fn demo_bytes() -> Vec<u8> {
    fake_demo(4096)
}

fn sql(st: &BotStand, q: &str) {
    st.server
        .running()
        .core()
        .db()
        .write(|tx| tx.execute(q, []).map(|_| ()).map_err(|e| e.to_string()))
        .unwrap();
}

fn jobs(st: &BotStand) -> Vec<server::core::JobRow> {
    st.server.running().core().jobs().unwrap()
}

fn inbox(st: &BotStand) -> Vec<store::InboxRow> {
    store::inbox(st.server.running().core().db()).unwrap()
}

fn chat_text(st: &BotStand, chat: i64) -> String {
    st.tg.texts_to(chat).join("\n")
}

/// Один документ от друга с подписью: кладёт файл, толкает обновление.
fn send(st: &BotStand, n: usize, name: &str, caption: &str) {
    let bytes = demo_bytes();
    let (file_id, unique) = (format!("doc-{n}"), format!("uniq-{n}"));
    put(st, &file_id, &unique, &bytes);
    push(
        st,
        FRIEND,
        &file_id,
        &unique,
        name,
        Some(bytes.len() as u64),
        caption,
    );
}

// ---------------------------------------------------------------------------------------------------------
// Повторы и несколько игроков (Task 1)
// ---------------------------------------------------------------------------------------------------------

#[test]
fn same_demo_for_another_player_becomes_a_second_job_without_a_second_download() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    put(&st, "f1", "u1", &bytes);
    let len = Some(bytes.len() as u64);

    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 1);
    let a = all[0].id;
    assert_eq!(st.tg.get_file_calls(), 1);

    // Тот же документ для другого игрока: вторая задача без второго `getFile`.
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "ZywOo");
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 2, "две задачи: {all:?}");
    let b = all[1].id;
    assert_eq!(all[1].player_name, "ZywOo");
    assert_eq!(st.tg.get_file_calls(), 1, "второго скачивания нет");
    let core = st.server.running().core();
    let (da, db_) = (
        core.demo_file(a).unwrap().unwrap(),
        core.demo_file(b).unwrap().unwrap(),
    );
    assert_eq!(da.sha256, db_.sha256, "демка у задач одна и та же");

    // Снова первый игрок (другая запись ника): ответ про существующую задачу.
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "S1MPLE ");
    st.step();
    assert_eq!(jobs(&st).len(), 2, "новых задач нет");
    assert_eq!(st.tg.get_file_calls(), 1);
    let text = chat_text(&st, FRIEND);
    assert!(
        text.contains(&format!("уже в очереди: задача #{a}")),
        "ответ про дубль: {text}"
    );
}

#[test]
fn done_duplicate_points_to_resend() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    put(&st, "f1", "u1", &bytes);
    let len = Some(bytes.len() as u64);
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let a = jobs(&st)[0].id;
    let token = st.runner_token();
    st.finish_done(a, &token);
    st.step();
    assert_eq!(st.server.job(a).state, JobState::Done);

    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    assert_eq!(jobs(&st).len(), 1);
    let text = chat_text(&st, FRIEND);
    assert!(text.contains(&format!("/resend {a}")), "ответ: {text}");
}

#[test]
fn failed_duplicate_is_not_a_duplicate() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    put(&st, "f1", "u1", &bytes);
    let len = Some(bytes.len() as u64);
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let a = jobs(&st)[0].id;
    let token = st.runner_token();
    st.finish_failed(a, &token, "HLAE_ERROR");
    assert_eq!(st.server.job(a).state, JobState::Failed);

    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 2, "после сбоя повтор разрешён: {all:?}");
    assert_eq!(all[1].state, JobState::Queued);
    assert_eq!(st.tg.get_file_calls(), 1, "демка взята у прежней задачи");
}

#[test]
fn same_bytes_from_another_upload_are_found_by_hash() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    let len = Some(bytes.len() as u64);
    put(&st, "f1", "u1", &bytes);
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let a = jobs(&st)[0].id;

    // Те же байты, но другая загрузка (другой `file_unique_id`): узнаётся только по хешу после копирования.
    put(&st, "f2", "u2", &bytes);
    push(&st, FRIEND, "f2", "u2", "again.dem", len, "s1mple");
    st.step();
    assert_eq!(st.tg.get_file_calls(), 2);
    let text = chat_text(&st, FRIEND);
    assert!(
        text.contains(&format!("уже в очереди: задача #{a}")),
        "ответ: {text}"
    );
    let all = jobs(&st);
    assert_eq!(all.len(), 2, "лишняя копия создана и отменена: {all:?}");
    let active: Vec<_> = all.iter().filter(|j| !j.state.is_terminal()).collect();
    assert_eq!(active.len(), 1, "активная задача ключа одна");
    assert_eq!(active[0].id, a);
    assert_eq!(all[1].state, JobState::Cancelled);
    let rows = inbox(&st);
    assert_eq!(rows[1].state, "rejected");
    assert_eq!(
        rows[1].reason.as_deref(),
        Some(&*format!("дубль задачи #{a}"))
    );

    // Следующая пересылка второй загрузки узнаётся уже до `getFile`.
    push(&st, FRIEND, "f2", "u2", "again.dem", len, "s1mple");
    st.step();
    assert_eq!(st.tg.get_file_calls(), 2);
}

// ---------------------------------------------------------------------------------------------------------
// Лимиты, размер, имя, магия, место в очереди (Task 2)
// ---------------------------------------------------------------------------------------------------------

#[test]
fn oversized_or_sizeless_or_non_demo_name_is_refused_before_get_file() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    let len = Some(bytes.len() as u64);
    put(&st, "f1", "u1", &bytes);

    push(&st, FRIEND, "f1", "u1", "match.dem", None, "s1mple");
    push(
        &st,
        FRIEND,
        "f1",
        "u1",
        "match.dem",
        Some(2000 * MIB + 1),
        "s1mple",
    );
    push(&st, FRIEND, "f1", "u1", "video.mp4", len, "s1mple");
    st.step();
    let text = chat_text(&st, FRIEND).to_lowercase();
    assert!(
        text.contains("пришлите демку файлом"),
        "без размера: {text}"
    );
    assert!(text.contains("2000 миб"), "лимит в МиБ: {text}");
    assert!(text.contains("нужен файл .dem или .dem.zst"), "имя: {text}");
    assert_eq!(st.tg.get_file_calls(), 0, "getFile не звали");
    assert!(inbox(&st).is_empty(), "строк tg_inbox нет");
    assert!(jobs(&st).is_empty());

    // Имя в верхнем регистре с расширением `.dem.zst` проходит.
    push(&st, FRIEND, "f1", "u1", "MATCH.DEM.ZST", len, "s1mple");
    st.step();
    assert_eq!(jobs(&st).len(), 1);
    assert_eq!(st.tg.get_file_calls(), 1);
}

#[test]
fn garbage_bytes_with_dem_name_are_refused_after_download_without_a_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = vec![0x41u8; 4096];
    put(&st, "f1", "u1", &bytes);
    push(&st, FRIEND, "f1", "u1", "m.dem", Some(4096), "s1mple");
    st.step();
    assert!(jobs(&st).is_empty(), "задача не создана");
    let text = chat_text(&st, FRIEND);
    assert!(text.contains("это не демка CS2"), "ответ: {text}");
    let rows = inbox(&st);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "rejected");
}

#[test]
fn friend_limit_counts_pending_downloads() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    for n in 0..5 {
        send(&st, n, "match.dem", "s1mple");
    }
    // Только опрос: ни одного скачивания, а лимит уже считает строки tg_inbox.
    st.bot.poll_once(0).unwrap();
    assert_eq!(inbox(&st).len(), 2, "две строки во входящих");
    let text = chat_text(&st, FRIEND).to_lowercase();
    let refusals = text
        .matches("у вас уже 2 активные задачи — дождитесь окончания или отмените /cancel")
        .count();
    assert_eq!(refusals, 3, "три отказа: {text}");
    st.step();
    assert_eq!(jobs(&st).len(), 2, "после скачивания две задачи");
}

#[test]
fn admin_limit_is_five_and_queue_limit_is_ten() {
    let st = BotStand::start();
    st.add_admin(FRIEND, "автор");
    for n in 0..6 {
        send(&st, n, "match.dem", "s1mple");
    }
    st.bot.poll_once(0).unwrap();
    assert_eq!(inbox(&st).len(), 5, "админу пять");
    assert!(
        chat_text(&st, FRIEND)
            .to_lowercase()
            .contains("у вас уже 5 активных задач"),
        "шестая — отказ"
    );

    // Общая очередь: десять активных задач от оператора, друг без своих задач получает отказ.
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    for _ in 0..10 {
        st.server.submit(&demo_bytes(), "s1mple");
    }
    send(&st, 0, "match.dem", "s1mple");
    st.step();
    assert!(inbox(&st).is_empty());
    assert_eq!(jobs(&st).len(), 10);
    let text = chat_text(&st, FRIEND).to_lowercase();
    assert!(text.contains("общая очередь заполнена"), "ответ: {text}");
    assert_eq!(st.tg.get_file_calls(), 0);
}

#[test]
fn ack_shows_the_place_in_queue_before_the_download() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    for _ in 0..3 {
        st.server.submit(&demo_bytes(), "ZywOo");
    }
    send(&st, 0, "match.dem", "s1mple");
    st.bot.poll_once(0).unwrap();
    assert_eq!(st.tg.get_file_calls(), 0, "подтверждение до getFile");
    let texts = st.tg.texts_to(FRIEND);
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("перед вами 3"),
        "подтверждение: {}",
        texts[0]
    );

    st.step();
    let id = jobs(&st).last().unwrap().id;
    let texts = st.tg.texts_to(FRIEND);
    assert_eq!(texts.len(), 1, "подтверждение правится: {texts:?}");
    assert!(texts[0].contains(&format!("#{id}")), "{}", texts[0]);
    assert!(texts[0].contains("перед вами 3"), "{}", texts[0]);
}

#[test]
fn crash_after_submit_relinks_to_the_existing_job() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    send(&st, 0, "match.dem", "s1mple");
    st.step();
    let a = jobs(&st)[0].id;
    assert_eq!(st.tg.get_file_calls(), 1);

    // Падение между `Core::submit` и записью `queued`: строка осталась `downloading` без задачи и без `tg_jobs`.
    sql(
        &st,
        "UPDATE tg_inbox SET state = 'downloading', job_id = NULL",
    );
    sql(&st, "DELETE FROM tg_jobs");
    st.bot.recover().unwrap();
    st.step();

    let rows = inbox(&st);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "queued");
    assert_eq!(rows[0].job_id, Some(a), "привязана к прежней задаче");
    assert_eq!(jobs(&st).len(), 1, "второй задачи нет");
    assert_eq!(st.tg.get_file_calls(), 1, "повторного скачивания нет");
    assert_eq!(
        store::tg_jobs(st.server.running().core().db())
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------------------------------------
// Путь файла из bot-api (Task 3)
// ---------------------------------------------------------------------------------------------------------

/// Отправляет документ, который `getFile` отдаёт по пути `path` вместо настоящего.
fn send_with_path(st: &BotStand, n: usize, claimed: u64, path: &Path) {
    let (file_id, unique) = (format!("p-{n}"), format!("pu-{n}"));
    put(st, &file_id, &unique, &demo_bytes());
    st.tg.set_file_path(&file_id, &path.to_string_lossy());
    push(
        st,
        FRIEND,
        &file_id,
        &unique,
        "match.dem",
        Some(claimed),
        "s1mple",
    );
}

fn assert_refused_without_job(st: &BotStand) {
    assert!(jobs(st).is_empty(), "задачи нет");
    let rows = inbox(st);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "rejected");
    let text = chat_text(st, FRIEND).to_lowercase();
    assert!(text.contains("не удалось принять файл"), "ответ: {text}");
    assert!(!text.contains("демка"), "ответ не про магию: {text}");
}

#[test]
fn file_outside_the_bot_api_root_is_refused() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let other = TestDir::new("outside");
    let outside = other.path.join("x.dem");
    std::fs::write(&outside, b"garbage garbage garbage").unwrap();
    send_with_path(&st, 0, 23, &outside);
    st.step();
    assert_refused_without_job(&st);
    assert!(outside.exists(), "чужой файл не тронут");
}

#[test]
fn dot_dot_path_is_refused() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let real = put(&st, "tmp", "tmpu", &demo_bytes());
    let name = real.file_name().unwrap().to_owned();
    let tricky = st
        .tg
        .files_dir()
        .join("documents")
        .join("..")
        .join("documents")
        .join(name);
    send_with_path(&st, 0, demo_bytes().len() as u64, &tricky);
    st.step();
    assert_refused_without_job(&st);
}

/// Создаёт символическую ссылку; `false` — не получилось (на Windows нужны права).
fn try_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

#[test]
fn symlinked_file_is_refused() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let other = TestDir::new("link-target");
    let target = other.path.join("real.dem");
    std::fs::write(&target, demo_bytes()).unwrap();
    let link = st.tg.files_dir().join("link.dem");
    if !try_symlink(&target, &link) {
        eprintln!("пропуск: символическую ссылку создать нельзя");
        return;
    }
    send_with_path(&st, 0, demo_bytes().len() as u64, &link);
    st.step();
    assert_refused_without_job(&st);
}

#[test]
fn size_mismatch_is_refused() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    put(&st, "f1", "u1", &bytes);
    push(
        &st,
        FRIEND,
        "f1",
        "u1",
        "match.dem",
        Some(bytes.len() as u64 + 1),
        "s1mple",
    );
    st.step();
    assert_refused_without_job(&st);
}

#[test]
fn original_is_removed_after_copy() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let bytes = demo_bytes();
    let original = put(&st, "f1", "u1", &bytes);
    push(
        &st,
        FRIEND,
        "f1",
        "u1",
        "match.dem",
        Some(bytes.len() as u64),
        "s1mple",
    );
    st.step();
    let all = jobs(&st);
    assert_eq!(all.len(), 1);
    assert!(!original.exists(), "оригинал в папке bot-api удалён");
    let stored = st.server.running().core().blobs().demo_path(all[0].id);
    assert!(stored.exists(), "демка лежит в хранилище сервера");
    assert_eq!(std::fs::metadata(stored).unwrap().len(), bytes.len() as u64);
}

#[test]
fn ingest_errors_name_no_paths() {
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let other = TestDir::new("secret-outside");
    let outside = other.path.join("x.dem");
    std::fs::write(&outside, b"garbage").unwrap();

    // Три разных отказа: вне корня, путь с `..`, неверный размер.
    send_with_path(&st, 0, 7, &outside);
    st.step();
    let real = put(&st, "tmp", "tmpu", &demo_bytes());
    let tricky = st
        .tg
        .files_dir()
        .join("documents")
        .join("..")
        .join("documents")
        .join(real.file_name().unwrap());
    send_with_path(&st, 1, demo_bytes().len() as u64, &tricky);
    st.step();
    let bytes = demo_bytes();
    put(&st, "f3", "u3", &bytes);
    push(
        &st,
        FRIEND,
        "f3",
        "u3",
        "match.dem",
        Some(bytes.len() as u64 + 1),
        "s1mple",
    );
    st.step();

    let rows = inbox(&st);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.state == "rejected"), "{rows:?}");

    let mut hay = vec![st.server.sink_text(), chat_text(&st, FRIEND)];
    hay.extend(rows.iter().filter_map(|r| r.reason.clone()));
    let hay = hay.join("\n");
    let secrets = [
        st.tg.files_dir().to_string_lossy().into_owned(),
        st.tg_dir
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        other
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        "documents".to_owned(),
    ];
    for s in secrets {
        assert!(!hay.contains(&s), "утечка «{s}» в:\n{hay}");
    }
}

/// WR-01: чужая задача не загоняет второго друга в тупик. Пока она идёт, ответ без номера и без `/resend`;
/// когда её ролик готов, второй друг получает свою задачу.
#[test]
fn another_friends_duplicate_is_never_a_dead_end() {
    const OTHER: i64 = 222;
    let st = BotStand::start();
    st.add_friend(FRIEND, "вася");
    let other_uid = st.add_friend(OTHER, "петя");
    let bytes = demo_bytes();
    put(&st, "f1", "u1", &bytes);
    put(&st, "f2", "u2", &bytes);
    let len = Some(bytes.len() as u64);
    push(&st, FRIEND, "f1", "u1", "match.dem", len, "s1mple");
    st.step();
    let a = jobs(&st)[0].id;

    // Идёт у первого друга: второму отвечают без номера чужой задачи и без /resend.
    push(&st, OTHER, "f2", "u2", "match.dem", len, "s1mple");
    st.step();
    let text = chat_text(&st, OTHER);
    assert!(text.contains("уже в очереди"), "{text}");
    assert!(
        !text.contains(&format!("#{a}")) && !text.contains("/resend"),
        "чужой номер и /resend не показываем: {text}"
    );
    assert!(
        jobs(&st)
            .iter()
            .all(|j| j.user_id != Some(other_uid) || j.state == JobState::Cancelled),
        "активной задачи у второго друга нет"
    );

    // Ролик первого друга готов: второй получает свою задачу, а не «/resend чужой».
    let token = st.runner_token();
    st.finish_done(a, &token);
    st.step();
    push(&st, OTHER, "f2", "u2", "match.dem", len, "s1mple");
    st.step();
    let mine: Vec<_> = jobs(&st)
        .into_iter()
        .filter(|j| j.user_id == Some(other_uid) && j.state == JobState::Queued)
        .collect();
    assert_eq!(mine.len(), 1, "своя задача поставлена: {:?}", jobs(&st));
    assert!(!chat_text(&st, OTHER).contains("/resend"));
}
