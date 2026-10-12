//! Уборщик сервера (план 09-02, OPS-01): сроки хранения на виртуальных часах, двухшаговое удаление,
//! `.part`, остатки, рабочая папка `telegram-bot-api`. Время файлов задаётся `File::set_modified`,
//! время задач — `ManualClock`: настоящих ожиданий в тестах нет, кроме ожидания потока уборки.

mod common;

use common::{TestOptions, TestServer, fake_demo, result_bytes, sha256_hex, wait_until};
use server::clean::{CleanMode, Cleaned, Retention};
use server::clock::ManualClock;
use server::core::Submit;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use wire::{FinishRequest, Outcome, Phase, ProgressRequest};

const T0: i64 = 1_800_000_000_000;

fn hours(n: u64) -> Duration {
    Duration::from_secs(n * 3600)
}

/// Стенд на ручных часах; `interval` — период потока уборки (`None` — умолчание, поток не срабатывает).
fn manual_server(interval: Option<u64>) -> (TestServer, ManualClock) {
    let clock = ManualClock::new(T0);
    let srv = TestServer::start_with(TestOptions {
        clock: Some(clock.clone()),
        clean_interval_secs: interval,
        ..Default::default()
    });
    (srv, clock)
}

fn data(srv: &TestServer) -> PathBuf {
    srv.dir.path.join("data")
}

/// Ставит демку в очередь и возвращает номер задачи.
fn put_demo(srv: &TestServer, size: usize, player: &str) -> i64 {
    put_demo_for(srv, size, player, None)
}

fn put_demo_for(srv: &TestServer, size: usize, player: &str, user_id: Option<i64>) -> i64 {
    let bytes = fake_demo(size);
    let src = srv.dir.path.join(format!("src-{}.dem", sha256_hex(&bytes)));
    std::fs::write(&src, &bytes).unwrap();
    let s = Submit {
        player: player.to_owned(),
        user_id,
        requested_by: "оператор".to_owned(),
        orig_name: "m.dem".to_owned(),
    };
    srv.running().core().submit(&src, &s).unwrap().0
}

/// Раннер берёт самую старую задачу очереди и доводит её до `running`; возвращает эпоху.
fn claim_running(srv: &TestServer, token: &str, id: i64) -> i64 {
    let (status, got) = srv.claim(token, 0);
    assert_eq!(status, 200);
    let a = got.expect("задача выдана");
    assert_eq!(a.job_id, id, "выдана не та задача");
    let req = ProgressRequest {
        epoch: a.epoch,
        seq: 1,
        phase: Phase::Record,
        take: None,
        status: String::new(),
    };
    assert_eq!(srv.progress(token, id, &req).0, 200);
    a.epoch
}

/// Задача от демки до `done` с принятым роликом; возвращает номер.
fn done_job(srv: &TestServer, token: &str, size: usize, player: &str) -> i64 {
    let id = put_demo(srv, size, player);
    let epoch = claim_running(srv, token, id);
    let data = result_bytes(4096);
    let (status, _, body) = srv.put_chunk(token, id, epoch, 0, &data);
    assert_eq!(status, 204, "{body}");
    let (status, body) = srv.finish_done(token, id, epoch, data.len() as u64, &sha256_hex(&data));
    assert_eq!(status, 200, "{body}");
    id
}

/// Задача от демки до `failed`.
fn failed_job(srv: &TestServer, token: &str, size: usize, player: &str) -> i64 {
    let id = put_demo(srv, size, player);
    let epoch = claim_running(srv, token, id);
    let req = FinishRequest {
        epoch,
        outcome: Outcome::Failed(wire::Failure {
            code: "HLAE_ERROR".to_owned(),
            message: "сбой для теста".to_owned(),
            ..Default::default()
        }),
    };
    let (status, body) = srv.finish(token, id, &req);
    assert_eq!(status, 200, "{body}");
    id
}

fn demo_path(srv: &TestServer, id: i64) -> PathBuf {
    data(srv)
        .join("blobs")
        .join("incoming")
        .join(id.to_string())
        .join("demo")
}

fn result_path(srv: &TestServer, id: i64) -> PathBuf {
    data(srv)
        .join("blobs")
        .join("results")
        .join(id.to_string())
        .join("highlights.mp4")
}

/// Время изменения файла: `age` назад от часов стенда (а не от настоящего времени).
fn set_age(path: &Path, clock: &ManualClock, age: Duration) {
    use server::clock::Clock;
    let ms = (clock.now_ms() as u64).saturating_sub(age.as_millis() as u64);
    let t = UNIX_EPOCH + Duration::from_millis(ms);
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

fn clean(srv: &TestServer, mode: CleanMode) -> Cleaned {
    srv.running()
        .core()
        .clean(&Retention::default(), None, mode)
        .unwrap()
}

/// `deleted_at` строки `files` задачи данного вида: `None` — строки нет, `Some(None)` — живая.
fn deleted_at(srv: &TestServer, id: i64, kind: &str) -> Option<Option<i64>> {
    srv.running()
        .core()
        .db()
        .read(|c| {
            c.query_row(
                "SELECT deleted_at FROM files WHERE job_id = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
                rusqlite::params![id, kind],
                |r| r.get::<_, Option<i64>>(0),
            )
            .map_err(|e| e.to_string())
        })
        .ok()
}

// ---------------------------------------------------------------------------------------------------------
// Трассер: ключ настройки → поток serve → Core::clean → строка помечена → файл удалён
// ---------------------------------------------------------------------------------------------------------

#[test]
fn done_job_demo_is_removed_by_the_cleaner_thread_after_its_retention() {
    let (srv, clock) = manual_server(Some(1));
    let token = srv.add_runner("pc");
    let id = done_job(&srv, &token, 3001, "s1mple");
    let demo = demo_path(&srv, id);
    assert!(demo.is_file(), "до срока демка на месте");
    assert!(
        deleted_at(&srv, id, "demo").is_some_and(|d| d.is_none()),
        "строка демки живая"
    );

    clock.advance(hours(25));
    assert!(
        wait_until(10, || !demo.exists()),
        "поток уборки не удалил демку; журнал: {}",
        srv.sink_text()
    );
    let core = srv.running().core();
    assert!(
        core.demo_file(id).unwrap().is_none(),
        "демку никто не видит"
    );
    assert!(
        deleted_at(&srv, id, "demo").is_some_and(|d| d.is_some()),
        "строка демки помечена deleted_at"
    );
    assert!(
        result_path(&srv, id).is_file(),
        "ролик задачи не из бота живёт 168 ч"
    );
    assert!(
        wait_until(10, || srv.sink_text().contains("уборка: демок 1")),
        "в журнале нет итога уборки: {}",
        srv.sink_text()
    );
}

#[test]
fn queued_job_keeps_its_demo_however_old_it_is() {
    let (srv, clock) = manual_server(None);
    let id = put_demo(&srv, 3002, "s1mple");
    clock.advance(hours(100));
    let got = clean(&srv, CleanMode::Apply);
    assert!(got.demos.is_empty(), "{got:?}");
    assert!(demo_path(&srv, id).is_file());
    assert!(srv.running().core().demo_file(id).unwrap().is_some());
}

// ---------------------------------------------------------------------------------------------------------
// Правила по состояниям задач и доставки, куски, остатки, ответы бота (Task 2)
// ---------------------------------------------------------------------------------------------------------

use server::core::{DedupKey, player_key};

fn add_user(srv: &TestServer, name: &str, tg_id: i64) -> i64 {
    let core = srv.running().core();
    server::users::add_user(core.db(), name, Some(tg_id), false, core.clock().now_ms())
        .unwrap()
        .id
}

/// Выполняет SQL над базой стенда (подготовка состояния, которого нет у обычного пути).
fn exec(srv: &TestServer, sql: &str, args: impl rusqlite::Params) {
    srv.running()
        .core()
        .db()
        .write(|tx| tx.execute(sql, args).map(|_| ()).map_err(|e| e.to_string()))
        .unwrap();
}

/// Строка `tg_jobs` с заданной доставкой, как после работы бота.
fn tg_job_row(
    srv: &TestServer,
    id: i64,
    user_id: i64,
    delivery: &str,
    file_id: Option<&str>,
    updated_at: i64,
) {
    exec(
        srv,
        "INSERT INTO tg_jobs(job_id, chat_id, user_id, delivery, result_file_id, created_at, updated_at) \
         VALUES (?1, 111, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![id, user_id, delivery, file_id, updated_at],
    );
}

fn done_job_for(
    srv: &TestServer,
    token: &str,
    size: usize,
    player: &str,
    user: Option<i64>,
) -> i64 {
    let id = put_demo_for(srv, size, player, user);
    let epoch = claim_running(srv, token, id);
    let data = result_bytes(4096);
    let (status, _, body) = srv.put_chunk(token, id, epoch, 0, &data);
    assert_eq!(status, 204, "{body}");
    let (status, body) = srv.finish_done(token, id, epoch, data.len() as u64, &sha256_hex(&data));
    assert_eq!(status, 200, "{body}");
    id
}

#[test]
fn failed_demo_waits_longer_and_then_retry_says_it_is_gone() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let id = failed_job(&srv, &token, 3101, "s1mple");

    clock.advance(hours(25));
    let got = clean(&srv, CleanMode::Apply);
    assert!(
        got.demos.is_empty(),
        "упавшая задача держит демку 72 ч: {got:?}"
    );
    assert!(demo_path(&srv, id).is_file());

    clock.advance(hours(48));
    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.demos, vec![id], "{got:?}");
    assert!(!demo_path(&srv, id).exists());

    let e = srv.running().core().resubmit(id, "друг").unwrap_err();
    assert!(
        e.contains("больше нет") && e.contains("пришлите файл заново"),
        "{e}"
    );
    assert!(!e.contains("blobs"), "в ответе друга нет путей: {e}");
}

#[test]
fn result_is_kept_while_delivery_is_not_finished() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let user = add_user(&srv, "вася", 111);
    let mut ids = Vec::new();
    for (k, delivery) in ["none", "pending", "sending", "uncertain"]
        .iter()
        .enumerate()
    {
        let id = done_job_for(&srv, &token, 3200 + k, "s1mple", Some(user));
        tg_job_row(&srv, id, user, delivery, None, T0);
        ids.push(id);
    }
    clock.advance(hours(400));
    let got = clean(&srv, CleanMode::Apply);
    assert!(
        got.results.is_empty(),
        "ролик с незаконченной доставкой не удаляется: {got:?}"
    );
    for id in ids {
        assert!(result_path(&srv, id).is_file(), "ролик задачи #{id}");
        assert!(srv.running().core().result_file(id).unwrap().is_some());
    }
}

#[test]
fn delivered_result_with_file_id_goes_after_a_day_and_is_still_a_duplicate() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let user = add_user(&srv, "вася", 111);
    let id = done_job_for(&srv, &token, 3301, "S1mple", Some(user));
    tg_job_row(&srv, id, user, "sent", Some("F1"), T0);

    clock.advance(hours(23));
    let got = clean(&srv, CleanMode::Apply);
    assert!(got.results.is_empty(), "{got:?}");
    assert!(result_path(&srv, id).is_file());

    clock.advance(hours(2));
    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.results, vec![id], "{got:?}");
    assert!(!result_path(&srv, id).exists());

    let key = DedupKey::File {
        sha256: sha256_hex(&fake_demo(3301)),
        player_key: player_key("s1mple"),
    };
    let dup = srv.running().core().find_duplicate(&key).unwrap();
    assert_eq!(
        dup.map(|j| j.id),
        Some(id),
        "отправка по file_id возможна, значит это по-прежнему дубль"
    );
}

#[test]
fn result_without_telegram_or_with_failed_delivery_goes_after_a_week() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let user = add_user(&srv, "вася", 111);
    let plain = done_job_for(&srv, &token, 3401, "a", None);
    let failed = done_job_for(&srv, &token, 3402, "b", Some(user));
    tg_job_row(&srv, failed, user, "failed", None, T0);

    clock.advance(hours(167));
    let got = clean(&srv, CleanMode::Apply);
    assert!(got.results.is_empty(), "{got:?}");
    assert!(result_path(&srv, plain).is_file() && result_path(&srv, failed).is_file());

    clock.advance(hours(2));
    let mut got = clean(&srv, CleanMode::Apply);
    got.results.sort();
    assert_eq!(got.results, vec![plain, failed], "{got:?}");
    assert!(!result_path(&srv, plain).exists() && !result_path(&srv, failed).exists());
}

#[test]
fn cleaned_result_is_not_a_duplicate_and_resend_explains_it() {
    use common::BotStand;
    const FRIEND: i64 = 111;
    let st = BotStand::start();
    let user = st.add_friend(FRIEND, "вася");
    st.send_demo(FRIEND, "s1mple");
    st.step();
    let core = st.server.running().core();
    let id = core.jobs().unwrap()[0].id;
    st.finish_done(id, &st.runner_token());
    // Доставка не удалась и file_id нет: ролик нужен только для /resend с диска.
    exec(
        &st.server,
        "UPDATE tg_jobs SET delivery = 'failed', result_file_id = NULL, delivery_error = 'тест' WHERE job_id = ?1",
        rusqlite::params![id],
    );

    st.clock().advance(hours(169));
    let got = core
        .clean(&Retention::default(), None, CleanMode::Apply)
        .unwrap();
    assert_eq!(got.results, vec![id], "{got:?}");

    let key = DedupKey::File {
        sha256: sha256_hex(&fake_demo(4096)),
        player_key: player_key("s1mple"),
    };
    assert!(
        core.find_duplicate_for(&key, user).unwrap().is_none(),
        "готовая задача без ролика и без file_id дублем не считается"
    );

    let u = st.tg.text_update(FRIEND, FRIEND, &format!("/resend {id}"));
    st.tg.push_update(u);
    st.step();
    let texts = st.tg.texts_to(FRIEND);
    let reply = texts
        .iter()
        .find(|t| t.contains("уже удалён с сервера по сроку хранения"))
        .unwrap_or_else(|| panic!("нет ответа про срок хранения: {texts:?}"));
    assert!(
        reply.contains(&format!("#{id}")) && reply.contains("Пришлите демку ещё раз"),
        "{reply}"
    );
    assert!(
        !reply.contains('/') && !reply.contains('\\') && !reply.contains("blobs"),
        "{reply}"
    );
    assert_eq!(
        st.tg_job(id).delivery,
        "failed",
        "доставка заново не ставится"
    );
}

#[test]
fn stale_and_foreign_part_files_go_but_an_active_upload_stays() {
    let (srv, clock) = manual_server(None);
    let id = put_demo(&srv, 3501, "s1mple");
    exec(
        &srv,
        "UPDATE jobs SET state = 'uploading', epoch = 2 WHERE id = ?1",
        rusqlite::params![id],
    );
    let dir = srv.results_dir(id);
    std::fs::create_dir_all(&dir).unwrap();
    let foreign = dir.join("1.part");
    let current = dir.join("2.part");
    std::fs::write(&foreign, b"old epoch").unwrap();
    std::fs::write(&current, b"fresh").unwrap();
    set_age(&foreign, &clock, hours(7));
    set_age(&current, &clock, hours(1));

    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.parts, 1, "{got:?}");
    assert!(!foreign.exists(), "кусок чужой эпохи удалён");
    assert!(current.exists(), "свежий кусок идущей загрузки остаётся");

    // Старый, но загрузка занята — остаётся.
    set_age(&current, &clock, hours(7));
    let guard = srv
        .running()
        .core()
        .blobs()
        .lock_upload(id, 2)
        .expect("загрузка свободна");
    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.parts, 0, "{got:?}");
    assert!(current.exists(), "идущая загрузка не трогается");

    drop(guard);
    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.parts, 1, "{got:?}");
    assert!(!current.exists());
}

#[test]
fn interrupted_clean_is_finished_by_the_next_pass() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let id = done_job(&srv, &token, 3601, "s1mple");
    // Процесс упал между шагами: строка помечена, файл остался.
    exec(
        &srv,
        "UPDATE files SET deleted_at = ?2 WHERE job_id = ?1 AND kind = 'demo'",
        rusqlite::params![id, T0],
    );
    let demo = demo_path(&srv, id);
    assert!(demo.is_file());
    set_age(&demo, &clock, hours(7));

    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.leftovers, 1, "{got:?}");
    assert!(!demo.exists());
    assert!(!demo.parent().unwrap().exists(), "папки incoming/{id} нет");
    assert!(
        result_path(&srv, id).is_file(),
        "ролик живой задачи на месте"
    );
}

#[test]
fn orphan_folders_go_only_when_old_and_never_for_active_jobs() {
    let (srv, clock) = manual_server(None);
    let blobs = data(&srv).join("blobs");
    let mk = |rel: &str, age: Duration| -> PathBuf {
        let dir = blobs.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.bin");
        std::fs::write(&f, b"x").unwrap();
        set_age(&f, &clock, age);
        dir
    };
    let old_incoming = mk("incoming/9999", hours(7));
    let old_results = mk("results/9999", hours(7));
    let fresh = mk("incoming/9998", hours(1));
    let not_a_job = mk("incoming/keep-me", hours(7));

    // Активная задача без живой строки демки и со старым содержимым папки.
    let active = put_demo(&srv, 3701, "s1mple");
    exec(
        &srv,
        "UPDATE files SET deleted_at = ?2 WHERE job_id = ?1 AND kind = 'demo'",
        rusqlite::params![active, T0],
    );
    set_age(&demo_path(&srv, active), &clock, hours(7));

    let got = clean(&srv, CleanMode::Apply);
    assert_eq!(got.leftovers, 2, "{got:?}");
    assert!(!old_incoming.exists() && !old_results.exists());
    assert!(fresh.exists(), "свежая папка остаётся");
    assert!(not_a_job.exists(), "имя не из цифр не трогаем");
    assert!(
        demo_path(&srv, active).is_file(),
        "папка активной задачи остаётся"
    );
}

#[test]
fn dry_run_reports_what_would_go_and_deletes_nothing() {
    let (srv, clock) = manual_server(None);
    let token = srv.add_runner("pc");
    let id = done_job(&srv, &token, 3801, "s1mple");
    let orphan = data(&srv).join("blobs").join("incoming").join("9999");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("x.bin"), b"x").unwrap();
    set_age(&orphan.join("x.bin"), &clock, hours(7));
    clock.advance(hours(25));

    let dry = clean(&srv, CleanMode::DryRun);
    assert_eq!(dry.demos, vec![id], "{dry:?}");
    assert_eq!(dry.leftovers, 1, "{dry:?}");
    assert!(dry.freed_bytes > 0, "{dry:?}");
    assert!(demo_path(&srv, id).is_file(), "ничего не удалено");
    assert!(orphan.exists());
    assert!(
        deleted_at(&srv, id, "demo").is_some_and(|d| d.is_none()),
        "метка deleted_at не поставлена"
    );

    let real = clean(&srv, CleanMode::Apply);
    assert_eq!(real.demos, dry.demos);
    assert_eq!(real.leftovers, 1);
    assert!(!demo_path(&srv, id).exists());
    assert!(!orphan.exists());
}
