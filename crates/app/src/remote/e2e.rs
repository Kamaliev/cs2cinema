//! Сквозные проверки фазы 8 на настоящем `server::serve` и `FakeHost`: сборка из `AppStand::boot` повторяет запуск
//! приложения (`App::new`), а вместо CS2 стоят заглушки, которые считают одновременные записи. Живой стенд (настоящие
//! CS2, HLAE, Telegram) отложен в фазу 9.

use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use pipeline::{FailCode, RecordFailure, RecordState, RunError, host::fake::FakeHost};
use runner::RunnerState;
use store::{JobKind, JobOutcome, JobSource, JobStatus, NewJob, Store};
use wire::JobState;

use super::control::RemoteStatus;
use super::token;
use crate::settings::Settings;
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

/// Ждёт условия до `secs` секунд; на каждом шаге складывает в `seen` новое состояние раннера.
fn watch(app: &AppStand, seen: &mut Vec<RemoteStatus>, secs: u64, cond: impl Fn() -> bool) -> bool {
    wait_until(secs, || {
        let now = app.remote.status();
        if seen.last() != Some(&now) {
            seen.push(now);
        }
        cond()
    })
}

/// Все файлы папки рекурсивно.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Есть ли `needle` в `haystack`.
fn has(haystack: &[u8], needle: &str) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Паникует, если токен найден. Сообщение называет место, но не печатает текст рядом: провал теста не должен сам выдать секрет.
fn assert_absent(haystack: &[u8], token: &str, place: &str) {
    assert!(!has(haystack, token), "токен раннера найден: {place}");
}

#[test]
fn issued_token_is_found_only_in_its_file() {
    let stand = Stand::start();
    let tools = TestDir::new("tools");
    let db_dir = TestDir::new("db");
    let runner_dir = stand.runner_dir();
    let token = stand.token.clone();
    let host = probing_host();
    // первая задача падает типизированным сбоем, вторая доходит до done
    let calls = Arc::new(AtomicUsize::new(0));
    let ok = gated_job_fn(Recordings::new(), flag(true));
    let factory = {
        let calls = calls.clone();
        pipeline_factory(&host, &tools, runner_dir.clone(), move |h, spec, p| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let f = RecordFailure::new(FailCode::TakeInvalid, RecordState::Take, "дубль записан неверно".to_owned(), Duration::ZERO);
                return Err(RunError { message: f.message.clone(), fail: Some(f) });
            }
            ok(h, spec, p)
        })
    };
    let mut app = AppStand::boot(BootOpts {
        db: db_dir.path.join("cs2cinema.db"),
        runner_dir: runner_dir.clone(),
        host: host.clone(),
        factory,
        local: gated_local(Recordings::new(), flag(true)),
        address: stand.base.clone(),
    });
    let mut seen = vec![app.remote.status()];
    assert_eq!(seen[0], RemoteStatus::NoToken, "без файла токена раннер не запускается");

    // токен вводится в окне: сохранение в файл с суженным доступом и переподключение
    app.remote.save_token(&token).expect("токен должен сохраниться");
    let failed = stand.submit(&fake_demo(MIB), "s1mple");
    assert!(watch(&app, &mut seen, 30, || stand.job(failed).state == JobState::Failed), "отказ задачи не дошёл до сервера: {:?}", stand.job(failed).state);
    let done = stand.submit(&fake_demo(MIB), "s1mple");
    assert!(watch(&app, &mut seen, 30, || stand.job(done).state == JobState::Done), "вторая задача не стала done: {:?}", stand.job(done).state);
    assert!(watch(&app, &mut seen, 10, || matches!(app.remote.status(), RemoteStatus::Online { .. })), "{:?}", app.remote.status());

    // недоступный адрес
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let nowhere = format!("http://127.0.0.1:{port}");
    app.remote.apply(&nowhere);
    assert!(watch(&app, &mut seen, 30, || matches!(app.remote.status(), RemoteStatus::NetError { .. })), "{:?}", app.remote.status());

    // токен отозван на сервере
    app.remote.apply(&stand.base);
    assert!(watch(&app, &mut seen, 20, || matches!(app.remote.status(), RemoteStatus::Online { .. })), "{:?}", app.remote.status());
    let core = stand.core();
    server::auth::revoke_runner(core.db(), "tester", core.clock().now_ms()).unwrap();
    assert!(watch(&app, &mut seen, 20, || app.remote.status() == RemoteStatus::TokenRejected), "{:?}", app.remote.status());
    assert!(seen.iter().any(|s| matches!(s, RemoteStatus::Working { .. })), "раннер брался за задачу: {seen:?}");
    assert!(seen.iter().any(|s| matches!(s, RemoteStatus::NetError { .. })) && seen.contains(&RemoteStatus::TokenRejected), "{seen:?}");
    let jobs = format!("{:?}", app.store.jobs());
    app.shutdown();

    // токен БЫЛ во входе: он лежит в своём файле и сервер принимал им запросы; поиск не холостой
    let in_file = std::fs::read(runner_dir.join(token::TOKEN_FILE)).expect("файл токена");
    assert!(has(&in_file, &token), "токен должен лежать в своём файле");
    assert!(has(b"xx token yy".as_slice(), "token") && !has(b"xx tokn yy".as_slice(), "token"), "проверка поиска");

    // сырые байты всего, что осталось на диске, кроме самого файла токена
    let own = runner_dir.join(token::TOKEN_FILE);
    let mut checked = 0;
    for path in files_under(&runner_dir).into_iter().chain(files_under(&db_dir.path)).filter(|p| *p != own) {
        assert_absent(&std::fs::read(&path).unwrap_or_default(), &token, &format!("файл {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        checked += 1;
    }
    assert!(checked >= 3, "проверено слишком мало файлов: {checked}");
    let log = std::fs::read(runner_dir.join("runner.log")).expect("runner.log");
    assert!(!log.is_empty(), "журнал раннера не пуст");

    // строки: база, состояния, настройки, настройки раннера, запуски программ
    assert_absent(jobs.as_bytes(), &token, "строки базы");
    for s in &seen {
        assert_absent(s.text().as_bytes(), &token, "текст состояния раннера");
        assert_absent(format!("{s:?}").as_bytes(), &token, "Debug состояния раннера");
        assert_absent(s.short().as_bytes(), &token, "короткая строка состояния");
    }
    let settings = Settings { runner_server: stand.base.clone(), ..Settings::default() };
    assert_absent(serde_json::to_string(&settings).unwrap().as_bytes(), &token, "settings.json");
    assert_absent(format!("{settings:?}").as_bytes(), &token, "Debug настроек");
    let cfg = runner::Config::new(stand.base.clone(), runner::Token::new(token.clone()), runner_dir.clone());
    assert_absent(format!("{cfg:?}").as_bytes(), &token, "Debug настроек раннера");
    assert_absent(format!("{:?}", host.runs()).as_bytes(), &token, "запуски программ (icacls)");
}

/// Исполнитель локальных задач для проверок «как в v1.0»: первый вызов — сбой запуска игры, дальше удача.
fn launch_failure() -> RunError {
    let f = RecordFailure::new(FailCode::ConsoleNotOpen, RecordState::Console, "консоль игры не открылась".to_owned(), Duration::ZERO);
    RunError { message: f.message.clone(), fail: Some(f) }
}

/// Папка раннера в пустом состоянии: нет ни замка, ни журнала, ни состояния.
fn assert_no_runner_traces(dir: &Path) {
    for name in ["runner.lock", "runner.log", "runner_state.json"] {
        assert!(!dir.join(name).exists(), "без адреса сервера раннер оставил {name}");
    }
}

#[test]
fn without_an_address_the_app_has_no_runner_and_behaves_like_v1() {
    let runner = TestDir::new("runner");
    let host = probing_host();
    // удалённых карточек без адреса нет: фабрика не должна вызываться
    let no_factory = || -> ExecFactory { Arc::new(|| Err("исполнитель удалённых задач без адреса не нужен".to_owned())) };
    let boot = |db: &TestDir, local: crate::queue::Runner| {
        AppStand::boot(BootOpts { db: db.path.join("cs2cinema.db"), runner_dir: runner.path.clone(), host: host.clone(), factory: no_factory(), local, address: String::new() })
    };
    let off = |app: &AppStand| {
        assert!(!app.remote.is_running(), "без адреса потока раннера нет");
        assert_eq!(app.remote.status(), RemoteStatus::Off);
        assert!(!app.hub.has_slots());
    };

    // сбой запуска: один авто-повтор и Done
    let db = TestDir::new("db-retry");
    let calls = Arc::new(AtomicUsize::new(0));
    let local: crate::queue::Runner = {
        let calls = calls.clone();
        Arc::new(move |job, _| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(launch_failure());
            }
            Ok(JobOutcome { out_dir: "out/local".into(), result: format!("out/local/{}.mp4", job.id).into(), shots: 1, secs: 1.0 })
        })
    };
    let app = boot(&db, local);
    off(&app);
    let id = app.store.add_job(&local_job("своя")).unwrap();
    assert!(wait_until(15, || status_of(&app.store, id) == Some(JobStatus::Done)), "задача после авто-повтора не дошла до Done: {}", card_log(&app, id));
    assert!(card_log(&app, id).contains("авто-повтор"), "{}", card_log(&app, id));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "ровно один авто-повтор");
    assert_eq!(app.queue.view().paused, None);
    off(&app);
    app.shutdown();

    // отмена идущей локальной задачи ставит паузу
    let db = TestDir::new("db-cancel");
    let rec = Recordings::new();
    let app = boot(&db, gated_local(rec.clone(), flag(false)));
    let id = app.store.add_job(&local_job("своя")).unwrap();
    assert!(wait_until(10, || rec.current() == 1), "локальная задача не началась");
    let (_, token) = app.queue.view().running.expect("задача выполняется");
    token.cancel();
    app.store.set_cancelling(id).unwrap();
    assert!(wait_until(10, || status_of(&app.store, id) == Some(JobStatus::Cancelled)), "задача не отменилась");
    assert!(wait_until(5, || app.queue.view().paused == Some(PauseReason::Cancelled)), "отмена автором ставит очередь на паузу");
    off(&app);
    app.shutdown();

    // приложение закрыто посреди своей задачи: пауза «Прервана», «Повторить» идёт через исполнитель
    let db = TestDir::new("db-kill");
    let killed = {
        let store = Store::open(&db.path.join("cs2cinema.db")).unwrap();
        let id = store.add_job(&local_job("своя")).unwrap();
        store.start_job(id).unwrap();
        id
    };
    let resumes = Arc::new(Mutex::new(Vec::new()));
    let local: crate::queue::Runner = {
        let resumes = resumes.clone();
        Arc::new(move |job, _| {
            resumes.lock().unwrap().push(job.resume);
            Ok(JobOutcome { out_dir: "out/local".into(), result: format!("out/local/{}.mp4", job.id).into(), shots: 1, secs: 1.0 })
        })
    };
    let app = boot(&db, local);
    assert_eq!((app.reset.interrupted, app.reset.interrupted_remote), (1, 0), "{:?}", app.reset);
    assert_eq!(status_of(&app.store, killed), Some(JobStatus::Interrupted));
    assert_eq!(app.queue.view().paused, Some(PauseReason::Interrupted), "пауза после прерванной задачи, как в v1.0");
    std::thread::sleep(Duration::from_millis(300));
    assert!(resumes.lock().unwrap().is_empty(), "на паузе ничего не запускается");
    app.store.requeue_resume(killed).unwrap();
    app.queue.resume();
    assert!(wait_until(10, || status_of(&app.store, killed) == Some(JobStatus::Done)), "«Повторить» не довёл задачу до Done");
    assert_eq!(*resumes.lock().unwrap(), vec![true], "исполнитель получил продолжение");
    off(&app);
    app.shutdown();

    assert_no_runner_traces(&runner.path);
}
