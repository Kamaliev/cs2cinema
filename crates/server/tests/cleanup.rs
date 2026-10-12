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
    let bytes = fake_demo(size);
    let src = srv.dir.path.join(format!("src-{}.dem", sha256_hex(&bytes)));
    std::fs::write(&src, &bytes).unwrap();
    let s = Submit {
        player: player.to_owned(),
        user_id: None,
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
