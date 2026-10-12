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

use pipeline::{RunError, host::fake::FakeHost};
use runner::RunnerState;
use store::{JobKind, JobSource, JobStatus, NewJob, Store};
use wire::JobState;

use super::control::RemoteStatus;
use super::hub::ExecFactory;
use super::testkit::{
    AppStand, BootOpts, JobFnCalls, Recordings, Stand, TestDir, after_kill, after_kill_without_dir, after_upload_cut, fake_demo, faststart_mp4, gated_job_fn,
    gated_local, inspect_runs, pipeline_factory, probing_host, put_token, wait_until,
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

/// Рабочая папка раннера, токен и файловая база приложения для сценариев «убитое приложение».
struct Disk {
    stand: Stand,
    tools: TestDir,
    db_dir: TestDir,
}

impl Disk {
    fn new() -> Disk {
        let stand = Stand::start();
        put_token(&stand.runner_dir(), &stand.token);
        Disk { stand, tools: TestDir::new("tools"), db_dir: TestDir::new("db") }
    }

    fn db(&self) -> std::path::PathBuf {
        self.db_dir.path.join("cs2cinema.db")
    }

    /// Запуск приложения на этих папках и этом сервере.
    fn boot(&self, host: &FakeHost, factory: ExecFactory, local: crate::queue::Runner) -> AppStand {
        AppStand::boot(BootOpts { db: self.db(), runner_dir: self.stand.runner_dir(), host: host.clone(), factory, local, address: self.stand.base.clone() })
    }
}

fn card_log(app: &AppStand, card: i64) -> String {
    app.store.job(card).unwrap().map(|j| j.log).unwrap_or_default()
}

#[test]
fn killed_during_recording_the_app_finishes_the_server_job_by_itself() {
    let disk = Disk::new();
    let runner_dir = disk.stand.runner_dir();
    let dead = after_kill(&disk.stand, &runner_dir, &disk.db());
    assert_eq!(disk.stand.job(dead.job_id).state, JobState::Assigned, "сервер считает задачу выданной раннеру");

    let (rec, release, calls) = (Recordings::new(), flag(false), JobFnCalls::default());
    let host = probing_host();
    let gated = gated_job_fn(rec.clone(), release.clone());
    let factory = {
        let calls = calls.clone();
        pipeline_factory(&host, &disk.tools, runner_dir.clone(), move |h, spec, p| {
            calls.record(spec);
            gated(h, spec, p)
        })
    };
    let local = Recordings::new();
    let app = disk.boot(&host, factory, gated_local(local.clone(), flag(true)));
    assert_eq!((app.reset.interrupted, app.reset.interrupted_remote), (0, 1), "карточка сервера считается отдельно: {:?}", app.reset);
    assert_eq!(app.queue.view().paused, None, "прерванная карточка сервера очередь на паузу не ставит");

    // запись продолжается сама; пока сервер не решил, карточка удержана раннером
    assert!(wait_until(20, || rec.current() == 1), "карточка не продолжила запись: {}", card_log(&app, dead.card));
    let card = app.store.job(dead.card).unwrap().unwrap();
    assert!(app.hub.is_held(&card), "карточка должна быть отмечена удержанной");
    assert_eq!(app.queue.view().paused, None);
    release.store(true, Ordering::SeqCst);

    assert!(
        wait_until(20, || disk.stand.job(dead.job_id).state == JobState::Done),
        "задача не стала done: {:?}\n{}",
        disk.stand.job(dead.job_id).state,
        card_log(&app, dead.card)
    );
    assert!(wait_until(10, || status_of(&app.store, dead.card) == Some(JobStatus::Done)), "карточка не стала Done:\n{}", card_log(&app, dead.card));
    assert!(wait_until(10, || card_log(&app, dead.card).contains("результат принят сервером")), "{}", card_log(&app, dead.card));
    let log = card_log(&app, dead.card);
    assert!(log.contains("продолжение после перезапуска"), "{log}");

    assert_eq!(calls.count(), 1, "запись продолжена один раз");
    assert_eq!(calls.resumes(), vec![true], "по record.json, а не заново");
    assert_eq!(calls.out_dirs(), vec![Some(dead.out_dir.clone())], "в папке, которую раннер сохранил");
    assert_eq!(inspect_runs(&host), 0, "повторной предпроверки демки нет");
    assert_eq!(local.starts(), Vec::<String>::new(), "локальный исполнитель не звался");
    let uploaded = std::fs::read(disk.stand.results_dir(dead.job_id).join("highlights.mp4")).expect("ролик принят сервером");
    assert_eq!(uploaded, faststart_mp4());
    assert_eq!(app.store.jobs().unwrap().iter().filter(|j| j.external_ref.is_some()).count(), 1, "карточка усыновлена, а не создана заново");
    assert_eq!(app.queue.view().paused, None);
    app.shutdown();
}

#[test]
fn killed_during_upload_the_app_uploads_without_recording_again() {
    let disk = Disk::new();
    let runner_dir = disk.stand.runner_dir();
    let dead = after_upload_cut(&disk.stand, &runner_dir, &disk.db());
    assert_eq!(disk.stand.job(dead.job_id).state, JobState::Assigned, "сервер ещё не видел результата");

    let calls = JobFnCalls::default();
    let host = probing_host();
    let factory = {
        let calls = calls.clone();
        pipeline_factory(&host, &disk.tools, runner_dir.clone(), move |_, spec, _| {
            calls.record(spec);
            Err(RunError::from("ролик уже готов: запись повторяться не должна"))
        })
    };
    let local = Recordings::new();
    let app = disk.boot(&host, factory, gated_local(local.clone(), flag(true)));
    assert_eq!((app.reset.interrupted, app.reset.interrupted_remote), (0, 0), "готовая карточка не прерывается: {:?}", app.reset);

    assert!(wait_until(20, || disk.stand.job(dead.job_id).state == JobState::Done), "результат не дошёл до сервера: {:?}", disk.stand.job(dead.job_id).state);
    let uploaded = std::fs::read(disk.stand.results_dir(dead.job_id).join("highlights.mp4")).expect("ролик принят сервером");
    assert_eq!(uploaded, faststart_mp4(), "результат на сервере равен файлу раннера");
    assert!(wait_until(10, || card_log(&app, dead.card).contains("результат принят сервером")), "{}", card_log(&app, dead.card));
    assert_eq!(status_of(&app.store, dead.card), Some(JobStatus::Done), "карточка остаётся Done");

    assert_eq!(calls.count(), 0, "запись не повторялась");
    assert_eq!(inspect_runs(&host), 0, "предпроверка не звалась");
    assert_eq!(host.launches(), 0, "игра не запускалась");
    assert_eq!(local.starts(), Vec::<String>::new());
    assert_eq!(app.queue.view().paused, None);
    assert!(wait_until(10, || RunnerState::load(&runner_dir).0.job.is_none()), "выдача снята с раннера");
    assert_eq!(app.store.jobs().unwrap().len(), 1, "новых карточек нет");
    app.shutdown();
}

#[test]
fn closed_app_with_a_waiting_server_card_resumes_it() {
    let disk = Disk::new();
    let runner_dir = disk.stand.runner_dir();
    let (dead, local_id) = after_kill_without_dir(&disk.stand, &runner_dir, &disk.db());

    let (rec, calls) = (Recordings::new(), JobFnCalls::default());
    let host = probing_host();
    let gated = gated_job_fn(rec.clone(), flag(true));
    let factory = {
        let calls = calls.clone();
        pipeline_factory(&host, &disk.tools, runner_dir.clone(), move |h, spec, p| {
            calls.record(spec);
            gated(h, spec, p)
        })
    };
    let local = Recordings::new();
    let app = disk.boot(&host, factory, gated_local(local.clone(), flag(true)));
    assert_eq!((app.reset.interrupted, app.reset.interrupted_remote), (1, 1), "{:?}", app.reset);
    assert_eq!(status_of(&app.store, local_id), Some(JobStatus::Interrupted), "своя задача прервана, как в v1.0");
    assert_eq!(app.queue.view().paused, Some(PauseReason::Interrupted), "очередь на паузе, как в v1.0");

    // карточка сервера вернулась в очередь по выдаче и ждёт, пока пауза не снята
    assert!(wait_until(20, || status_of(&app.store, dead.card) == Some(JobStatus::Queued)), "карточка не вернулась в очередь: {}", card_log(&app, dead.card));
    let card = app.store.job(dead.card).unwrap().unwrap();
    assert!(app.hub.is_held(&card), "карточка удержана раннером");
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        assert_eq!(status_of(&app.store, dead.card), Some(JobStatus::Queued), "пока очередь на паузе, карточка ждёт");
        assert_eq!(calls.count(), 0);
        std::thread::sleep(Duration::from_millis(20));
    }

    app.queue.resume();
    assert!(
        wait_until(30, || disk.stand.job(dead.job_id).state == JobState::Done),
        "задача не стала done: {:?}\n{}",
        disk.stand.job(dead.job_id).state,
        card_log(&app, dead.card)
    );
    assert!(wait_until(10, || status_of(&app.store, dead.card) == Some(JobStatus::Done)), "карточка не стала Done:\n{}", card_log(&app, dead.card));
    // папку пайплайн назвать не успел, продолжать нечем: раннер начинает запись заново, с предпроверкой демки, но ровно один раз
    assert_eq!(calls.resumes(), vec![false], "без папки задачи запись начинается заново");
    assert_eq!(calls.count(), 1, "запись шла один раз");
    assert_eq!(inspect_runs(&host), 1, "демка друга проверена перед записью");
    assert_eq!(status_of(&app.store, local_id), Some(JobStatus::Interrupted), "своя задача сама не запустилась: её ждёт «Повторить»");
    assert_eq!(local.starts(), Vec::<String>::new(), "локальный исполнитель не звался");
    assert_eq!(rec.max_concurrent(), 1);
    app.shutdown();
}
