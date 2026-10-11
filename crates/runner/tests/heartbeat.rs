//! Пульс, отмена и остановка на настоящем сервере: сервер видит этап и дубль, отмена с сервера доходит до
//! исполнителя за секунды (мягкая и жёсткая), застрявшая мягкая отмена сама становится жёсткой, остановка раннера
//! оставляет задачу, потерянная задача бросается без `finish`.
#![allow(unused_imports, dead_code)]

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

