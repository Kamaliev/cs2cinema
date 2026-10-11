//! Пульс, отмена и остановка на настоящем сервере: сервер видит этап и дубль, отмена с сервера доходит до
//! исполнителя за секунды (мягкая и жёсткая), застрявшая мягкая отмена сама становится жёсткой, остановка раннера
//! оставляет задачу, потерянная задача бросается без `finish`.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::{
    FinishThenCancel, IgnoreSoftCancel, Scenario, ScriptedExecutor, Stand, StopMidway,
    WaitForCancel, fake_demo, result_bytes, sha256_hex, wait_until,
};
use runner::transfer::StopWhy;
use runner::{Config, HeldJob, Link, Runner, RunnerState, Stage, Tick, Token};
use wire::{Assignment, JobState, Phase, ProgressRequest};

const MIB: usize = 1024 * 1024;

/// Настройки с быстрым пульсом, чтобы тесты не ждали секунды.
fn fast(stand: &Stand) -> Config {
    let mut cfg = stand.runner_config();
    cfg.heartbeat = Some(Duration::from_millis(200));
    cfg.debounce = Duration::from_millis(50);
    cfg
}

fn runner_log(stand: &Stand) -> String {
    std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap_or_default()
}

fn state_of(stand: &Stand) -> RunnerState {
    let (state, warning) = RunnerState::load(&stand.runner_dir());
    assert!(warning.is_none(), "{warning:?}");
    state
}

fn is_terminal(s: JobState) -> bool {
    matches!(s, JobState::Done | JobState::Failed | JobState::Cancelled)
}

/// Запускает один `tick` раннера в отдельном потоке.
fn spawn_tick(mut runner: Runner) -> std::thread::JoinHandle<Result<Tick, String>> {
    std::thread::spawn(move || runner.tick())
}

/// Берёт задачу у сервера «руками», как это сделал бы раннер, который потом умер.
fn claim_by_hand(stand: &Stand) -> Assignment {
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    link.claim(0).unwrap().expect("задача должна быть выдана")
}

#[test]
fn server_sees_stage_and_take_and_cancel_reaches_the_executor() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(2 * MIB), "s1mple");
    // Задержка внеочередного пульса 300 мс и пауза до этапа `record` оставляют серверу время показать `fetch` и
    // `precheck`, даже если опрашивающий поток отстаёт.
    let exec = WaitForCancel::new(Duration::from_millis(600));
    let mut cfg = fast(&stand);
    cfg.debounce = Duration::from_millis(300);
    let runner = Runner::new(cfg, exec.clone()).unwrap();
    let handle = spawn_tick(runner);

    // Этапы на сервере идут по порядку: fetch, precheck, record.
    let mut seen: Vec<String> = Vec::new();
    let reached = wait_until(5, || {
        let job = stand.job(id);
        if !job.phase.is_empty() && seen.last() != Some(&job.phase) {
            seen.push(job.phase.clone());
        }
        job.phase == "record" && job.take_n == Some(2) && job.take_m == Some(3)
    });
    assert!(reached, "сервер не увидел запись, дубль 2 из 3: {seen:?}");
    let order = ["fetch", "precheck", "record"];
    let mut next = 0;
    for phase in &seen {
        if next < order.len() && phase == order[next] {
            next += 1;
        }
    }
    assert_eq!(next, order.len(), "этапы на сервере: {seen:?}");

    // Мягкая отмена доходит до исполнителя за секунду.
    let at = Instant::now();
    stand.cancel(id, false);
    assert!(
        wait_until(2, || exec.cancel_seen.lock().unwrap().is_some()),
        "исполнитель не увидел отмену"
    );
    let delay = exec.cancel_seen.lock().unwrap().unwrap().duration_since(at);
    assert!(delay < Duration::from_secs(1), "отмена шла {delay:?}");

    let tick = handle.join().unwrap().unwrap();
    assert_eq!(
        tick,
        Tick::Settled {
            job_id: id,
            state: JobState::Cancelled
        }
    );
    assert_eq!(stand.job(id).state, JobState::Cancelled);
    assert!(
        state_of(&stand).job.is_none(),
        "задача осталась в состоянии"
    );
    assert!(
        exec.force_seen.lock().unwrap().is_none(),
        "мягкая отмена не должна быть жёсткой"
    );
}

#[test]
fn force_cancel_on_the_server_sets_force_cancelled() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = WaitForCancel::new(Duration::ZERO);
    let handle = spawn_tick(Runner::new(fast(&stand), exec.clone()).unwrap());
    assert!(wait_until(5, || stand.job(id).phase == "record"));

    let at = Instant::now();
    stand.cancel(id, true);
    assert!(
        wait_until(2, || exec.force_seen.lock().unwrap().is_some()),
        "исполнитель не увидел жёсткую отмену"
    );
    let delay = exec.force_seen.lock().unwrap().unwrap().duration_since(at);
    assert!(
        delay < Duration::from_secs(1),
        "жёсткая отмена шла {delay:?}"
    );

    handle.join().unwrap().unwrap();
    assert_eq!(stand.job(id).state, JobState::Cancelled);
}

#[test]
fn soft_cancel_escalates_to_force_within_the_bound() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = IgnoreSoftCancel::new();
    let mut cfg = fast(&stand);
    cfg.force_after = Duration::from_millis(300);
    let handle = spawn_tick(Runner::new(cfg, exec.clone()).unwrap());
    assert!(wait_until(5, || stand.job(id).phase == "record"));

    let at = Instant::now();
    stand.cancel(id, false);
    assert!(
        wait_until(4, || exec.force_seen.lock().unwrap().is_some()),
        "мягкая отмена не стала жёсткой"
    );
    let delay = exec.force_seen.lock().unwrap().unwrap().duration_since(at);
    assert!(
        delay < Duration::from_millis(1500),
        "от отмены до жёсткой прошло {delay:?}"
    );
    assert!(exec.soft_seen.lock().unwrap().is_some());

    handle.join().unwrap().unwrap();
    assert_eq!(stand.job(id).state, JobState::Cancelled);
    let log = runner_log(&stand);
    assert_eq!(
        log.matches("закрываю игру принудительно").count(),
        1,
        "{log}"
    );
    assert!(
        log.contains("мягкая отмена не завершила задачу за"),
        "{log}"
    );
}

#[test]
fn soft_cancel_that_the_executor_honours_does_not_escalate() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = WaitForCancel::new(Duration::ZERO);
    let mut cfg = fast(&stand);
    cfg.force_after = Duration::from_millis(300);
    let handle = spawn_tick(Runner::new(cfg, exec.clone()).unwrap());
    assert!(wait_until(5, || stand.job(id).phase == "record"));

    stand.cancel(id, false);
    handle.join().unwrap().unwrap();
    assert_eq!(stand.job(id).state, JobState::Cancelled);
    assert!(exec.cancel_seen.lock().unwrap().is_some());
    assert!(
        exec.force_seen.lock().unwrap().is_none(),
        "force_cancelled() не должно было стать истинным"
    );
    assert!(
        !runner_log(&stand).contains("закрываю игру принудительно"),
        "строка эскалации не нужна"
    );
}

#[test]
fn local_stop_keeps_the_job_for_resume() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = StopMidway::new();
    let mut runner = Runner::new(fast(&stand), exec.clone()).unwrap();
    *exec.flag.lock().unwrap() = Some(runner.stop_flag());

    assert_eq!(runner.tick().unwrap(), Tick::Stopped);
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    assert!(
        !exec.force_seen.load(Ordering::SeqCst),
        "локальная остановка не должна быть жёсткой"
    );
    let state = state_of(&stand);
    let held = state.job.expect("задача должна остаться в состоянии");
    assert_eq!(held.stage, Stage::Executing);
    assert!(
        !is_terminal(stand.job(id).state),
        "задача закрыта на сервере"
    );
    assert_eq!(
        stand.job(id).cancel_requested,
        wire::CancelMode::NotRequested
    );

    // Новый раннер в той же папке продолжает: hello `keep`, исполнитель идёт с resume.
    let exec2 = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut second = Runner::new(fast(&stand), exec2.clone()).unwrap();
    assert_eq!(
        second.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    let task = exec2.last_task();
    assert!(task.resume, "исполнитель должен идти с resume");
    assert!(task.out_dir.is_some(), "папка прошлой попытки потеряна");
}

#[test]
fn lost_lease_stops_the_executor_and_drops_the_job_without_finish() {
    let stand = Stand::start_manual();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = WaitForCancel::new(Duration::ZERO);
    let mut cfg = fast(&stand);
    // Редкий пульс: между возвратом задачи в очередь и выдачей второму раннеру первый не успевает «вернуть себе» её.
    cfg.heartbeat = Some(Duration::from_secs(2));
    let handle = spawn_tick(Runner::new(cfg, exec.clone()).unwrap());
    assert!(wait_until(5, || stand.job(id).phase == "record"));

    // Аренда первого раннера истекла, задачу забрал второй.
    stand.advance(Duration::from_secs(120));
    stand.sweep();
    assert_eq!(stand.job(id).state, JobState::Queued);
    let second = Link::new(&stand.base, Token::new(stand.second_token())).unwrap();
    let taken = second
        .claim(0)
        .unwrap()
        .expect("второй раннер получил задачу");
    assert_eq!(taken.job_id, id);
    assert_eq!(taken.epoch, 2);

    // Следующий пульс первого получает stale_epoch: исполнитель остановлен, задача брошена без finish.
    let tick = handle.join().unwrap().unwrap();
    assert_eq!(tick, Tick::Dropped { job_id: id });
    assert!(
        exec.force_seen.lock().unwrap().is_some(),
        "исполнитель должен был увидеть жёсткую остановку"
    );
    assert_eq!(
        stand.job(id).state,
        JobState::Assigned,
        "первый раннер не должен был закрывать чужую задачу"
    );
    assert!(state_of(&stand).job.is_none());
    let log = runner_log(&stand);
    assert!(log.contains("stale_epoch"), "{log}");
}

#[test]
fn seq_keeps_growing_after_restart_in_the_same_epoch() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, id);

    // Прошлый раннер успел отправить прогресс с большим seq (часы шли вперёд) и записал его в состояние.
    let big = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 3_600_000;
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    link.progress(
        id,
        &ProgressRequest {
            epoch: a.epoch,
            seq: big,
            phase: Phase::Fetch,
            take: None,
            status: String::new(),
        },
    )
    .unwrap();
    std::fs::create_dir_all(stand.runner_dir().join("remote")).unwrap();
    let demo = stand
        .runner_dir()
        .join("remote")
        .join(format!("srv{id}.dem"));
    RunnerState {
        job: Some(HeldJob::claimed(&a, demo)),
        last_seq: big,
        ..Default::default()
    }
    .save(&stand.runner_dir())
    .unwrap();

    let exec = WaitForCancel::new(Duration::ZERO);
    let handle = spawn_tick(Runner::new(fast(&stand), exec.clone()).unwrap());
    assert!(
        wait_until(5, || stand.job(id).phase == "record"),
        "сервер не принял этап после перезапуска: seq откатился? фаза {:?}",
        stand.job(id).phase
    );
    assert!(stand.job(id).last_seq as u64 > big);

    stand.cancel(id, false);
    handle.join().unwrap().unwrap();
    // После остановки пульса последний seq лежит в состоянии.
    assert!(state_of(&stand).last_seq > big);
}

#[test]
fn cancel_during_upload_stops_between_chunks_and_closes_the_job() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    // Исполнитель отменяет задачу на сервере, ждёт, пока раннер это узнает, и отдаёт результат: загрузка
    // начинается уже при отмене и прерывается на первой же проверке.
    let exec = FinishThenCancel::new(stand.runner_dir(), stand.core(), 20 * MIB);
    let mut runner = Runner::new(fast(&stand), exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Cancelled
        }
    );
    assert_eq!(stand.job(id).state, JobState::Cancelled);
    let result = stand
        .runner_dir()
        .join("out")
        .join(format!("srv{id}"))
        .join("highlights.mp4");
    assert!(!result.exists(), "файл результата остался в outbox");
    assert!(state_of(&stand).job.is_none());
    assert!(!stand.results_dir(id).join("highlights.mp4").exists());
}

#[test]
fn upload_stops_between_chunks_when_asked() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, id);

    let dir = stand.runner_dir();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    let bytes = result_bytes(20 * MIB);
    let path = dir.join("out").join("highlights.mp4");
    std::fs::write(&path, &bytes).unwrap();
    let mut held = HeldJob::claimed(&a, dir.join("srv.dem"));
    held.stage = Stage::Finished;
    held.result = Some(runner::ResultFile {
        path,
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
    });

    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    // Первые две проверки (до HEAD и до первого куска) пропускают, третья — перед вторым куском — отменяет.
    let calls = std::sync::atomic::AtomicU32::new(0);
    let stop = || (calls.fetch_add(1, Ordering::SeqCst) >= 2).then_some(StopWhy::Cancelled);
    let end = runner::transfer::upload_result(&link, &mut held, &mut |_| {}, &stop, None, &|_| {});

    assert_eq!(end, runner::TransferEnd::Cancelled);
    let accepted = link.result_head(id, a.epoch).unwrap();
    assert_eq!(
        accepted,
        wire::CHUNK_SIZE,
        "сервер должен принять ровно один кусок"
    );
}
