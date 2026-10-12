//! Сквозные проверки фазы 8 на настоящем `server::serve` и `FakeHost`: сборка из `AppStand::boot` повторяет запуск
//! приложения (`App::new`), а вместо CS2 стоят заглушки, которые считают одновременные записи. Живой стенд (настоящие
//! CS2, HLAE, Telegram) отложен в фазу 9.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use store::{JobKind, JobSource, JobStatus, NewJob, Store};
use wire::JobState;

use super::control::RemoteStatus;
use super::testkit::{
    AppStand, BootOpts, Recordings, Stand, TestDir, fake_demo, gated_job_fn, gated_local, pipeline_factory, probing_host, put_token, wait_until,
};
use crate::queue::PauseReason;

const MIB: usize = 1024 * 1024;

fn local_job(label: &str) -> NewJob {
    NewJob { kind: JobKind::Record, source: JobSource::File("local.dem".into()), label: label.into(), hero_name: String::new(), hero_steam: 0, map: String::new() }
}

fn flag(value: bool) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(value))
}

fn status_of(store: &Store, id: i64) -> Option<JobStatus> {
    store.job(id).unwrap().map(|j| j.status)
}

/// Задача сервера `secs` секунд подряд остаётся в очереди и не выдаётся ни разу.
fn stays_queued_on_the_server(stand: &Stand, id: i64, secs: u64, why: &str) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        let job = stand.job(id);
        assert_eq!((job.state, job.grants), (JobState::Queued, 0), "{why}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn one_recording_at_a_time_and_local_jobs_wait_their_turn() {
    let stand = Stand::start();
    let tools = TestDir::new("tools");
    let db_dir = TestDir::new("db");
    let runner_dir = stand.runner_dir();
    put_token(&runner_dir, &stand.token);
    let rec = Recordings::new();
    let (local_release, card_release) = (flag(false), flag(false));
    let host = probing_host();
    let factory = pipeline_factory(&host, &tools, runner_dir.clone(), gated_job_fn(rec.clone(), card_release.clone()));
    let app = AppStand::boot(BootOpts {
        db: db_dir.path.join("cs2cinema.db"),
        runner_dir,
        host: host.clone(),
        factory,
        local: gated_local(rec.clone(), local_release.clone()),
        address: stand.base.clone(),
    });

    // своя задача A «пишется», когда на сервер встаёт задача друга
    let a = app.store.add_job(&local_job("A")).unwrap();
    assert!(wait_until(10, || rec.current() == 1), "локальная задача A не начала запись");
    let id = stand.submit(&fake_demo(MIB), "s1mple");

    let gated = wait_until(10, || matches!(app.remote.status(), RemoteStatus::Gated { ref reason, .. } if reason == "идёт задача приложения"));
    assert!(gated, "ворота должны назвать причину: {:?}", app.remote.status());
    stays_queued_on_the_server(&stand, id, 3, "пока идёт локальная задача, сервер задачу не выдаёт");
    assert!(stand.core().runner_online().unwrap(), "раннер за закрытыми воротами остаётся на связи: аренда и пульс держатся");
    assert_eq!(rec.starts(), vec!["A"], "карточка не начиналась");

    // A закончилась: ворота открылись, задача сервера идёт карточкой
    local_release.store(true, Ordering::SeqCst);
    assert!(wait_until(20, || rec.starts() == vec!["A", "карточка"]), "карточка не начала запись после A: {:?}", rec.starts());
    assert!(wait_until(10, || matches!(app.remote.status(), RemoteStatus::Working { job_id } if job_id == id)), "{:?}", app.remote.status());

    // своя задача B, поставленная во время записи карточки, ждёт её конца
    let b = app.store.add_job(&local_job("B")).unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        assert_eq!(status_of(&app.store, b), Some(JobStatus::Queued), "B не должна стартовать, пока пишется карточка");
        assert_eq!(rec.current(), 1, "идёт ровно одна запись");
        assert_eq!(rec.starts(), vec!["A", "карточка"]);
        assert!(matches!(app.remote.status(), RemoteStatus::Working { job_id } if job_id == id), "{:?}", app.remote.status());
        std::thread::sleep(Duration::from_millis(20));
    }

    card_release.store(true, Ordering::SeqCst);
    assert!(wait_until(30, || stand.job(id).state == JobState::Done), "задача сервера не стала done: {:?}", stand.job(id).state);
    assert!(wait_until(20, || status_of(&app.store, b) == Some(JobStatus::Done)), "B после карточки не дошла до Done: {:?}", status_of(&app.store, b));

    assert_eq!(rec.max_concurrent(), 1, "одновременно шла не больше одной записи");
    assert_eq!(rec.starts(), vec!["A", "карточка", "B"], "порядок: своя A, задача друга, своя B");
    assert_eq!(status_of(&app.store, a), Some(JobStatus::Done));
    assert_eq!(app.queue.view().paused, None);
    assert_eq!(app.store.jobs().unwrap().iter().filter(|j| j.external_ref.is_some() && j.status == JobStatus::Done).count(), 1, "одна карточка сервера");
    app.shutdown();
}

#[test]
fn paused_queue_keeps_server_jobs_out() {
    let stand = Stand::start();
    let tools = TestDir::new("tools");
    let db_dir = TestDir::new("db");
    let runner_dir = stand.runner_dir();
    put_token(&runner_dir, &stand.token);
    let rec = Recordings::new();
    let host = probing_host();
    let factory = pipeline_factory(&host, &tools, runner_dir.clone(), gated_job_fn(rec.clone(), flag(true)));
    let app = AppStand::boot(BootOpts {
        db: db_dir.path.join("cs2cinema.db"),
        runner_dir,
        host: host.clone(),
        factory,
        local: gated_local(rec.clone(), flag(false)),
        address: stand.base.clone(),
    });

    // автор отменил свою идущую задачу: очередь встала на паузу
    let a = app.store.add_job(&local_job("A")).unwrap();
    assert!(wait_until(10, || rec.current() == 1), "локальная задача A не начала запись");
    let (running, token) = app.queue.view().running.expect("A выполняется");
    assert_eq!(running, a);
    token.cancel();
    app.store.set_cancelling(a).unwrap();
    assert!(wait_until(10, || status_of(&app.store, a) == Some(JobStatus::Cancelled)), "A не отменилась");
    assert!(wait_until(5, || app.queue.view().paused == Some(PauseReason::Cancelled)), "после отмены автора очередь на паузе");

    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let gated = wait_until(10, || matches!(app.remote.status(), RemoteStatus::Gated { ref reason, .. } if reason == "очередь приложения на паузе"));
    assert!(gated, "ворота должны назвать паузу: {:?}", app.remote.status());
    stays_queued_on_the_server(&stand, id, 3, "пока очередь приложения на паузе, сервер задачу не выдаёт");
    assert_eq!(rec.starts(), vec!["A"], "карточка не начиналась");

    app.queue.resume();
    assert!(wait_until(30, || stand.job(id).state == JobState::Done), "после «Продолжить» задача сервера не дошла до done: {:?}", stand.job(id).state);
    assert_eq!(rec.starts(), vec!["A", "карточка"]);
    assert_eq!(rec.max_concurrent(), 1);
    app.shutdown();
}
