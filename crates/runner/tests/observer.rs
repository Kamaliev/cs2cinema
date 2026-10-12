//! События наблюдателя на настоящем сервере: связь, работа, итог, ворота, обрыв сети и отказ токена; значения
//! токена в событиях нет.

mod common;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use common::{Scenario, ScriptedExecutor, Stand, fake_demo, wait_until};
use pipeline::{FailCode, RecordState};
use runner::{Admission, Config, Holding, JobState, Observer, Runner, RunnerEvent, Token};
use server::auth;

const MIB: usize = 1024 * 1024;

type Seen = Arc<Mutex<Vec<(RunnerEvent, Option<Holding>)>>>;

/// Наблюдатель, который складывает пары (событие, удержание) в общий список.
fn recorder() -> (Seen, Observer) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let observer: Observer = Arc::new(move |e: &RunnerEvent, h: Option<Holding>| {
        sink.lock().unwrap().push((e.clone(), h));
    });
    (seen, observer)
}

fn events(seen: &Seen) -> Vec<RunnerEvent> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|(e, _)| e.clone())
        .collect()
}

fn runner_log(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("runner.log")).unwrap_or_default()
}

#[test]
fn observer_sees_connect_work_and_settle_with_the_holding() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let (seen, observer) = recorder();
    let mut cfg = stand.runner_config();
    cfg.observer = Some(observer);
    let mut runner = Runner::new(cfg, exec).unwrap();

    runner.tick().unwrap();

    let epoch = stand.job(id).epoch;
    let held = Some(Holding { job_id: id, epoch });
    let seen = seen.lock().unwrap().clone();
    let positions = |pred: &dyn Fn(&RunnerEvent) -> bool| seen.iter().position(|(e, _)| pred(e));
    let connected = positions(
        &|e| matches!(e, RunnerEvent::Connected { server_version } if !server_version.is_empty()),
    )
    .expect("нет события Connected с версией сервера");
    let working = positions(&|e| *e == RunnerEvent::Working { job_id: id, epoch })
        .expect("нет события Working");
    let settled = positions(&|e| {
        *e == RunnerEvent::Settled {
            job_id: id,
            epoch,
            state: JobState::Done,
        }
    })
    .expect("нет события Settled");
    assert!(
        connected < working && working < settled,
        "порядок событий: {seen:?}"
    );
    assert_eq!(
        seen[working].1, held,
        "во время работы раннер удерживает задачу"
    );
    assert_eq!(seen[settled].1, None, "после итога удержания нет");
    assert_eq!(
        seen.iter()
            .filter(|(e, _)| matches!(e, RunnerEvent::Working { .. }))
            .count(),
        1,
        "Working одно на удержание: {seen:?}"
    );
}

#[test]
fn released_job_is_reported() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(
        stand.runner_dir(),
        vec![Scenario::Fail {
            code: FailCode::ForeignGameRunning,
            state: RecordState::Precheck,
        }],
    );
    let (seen, observer) = recorder();
    let mut cfg = stand.runner_config();
    cfg.observer = Some(observer);
    let mut runner = Runner::new(cfg, exec).unwrap();

    runner.tick().unwrap();

    let epoch = stand.job(id).epoch;
    let all = seen.lock().unwrap().clone();
    let released = all
        .iter()
        .find(|(e, _)| *e == RunnerEvent::Released { job_id: id, epoch })
        .unwrap_or_else(|| panic!("нет события Released: {all:?}"));
    assert_eq!(released.1, None, "после release задачи в удержании нет");
}

#[test]
fn run_reports_gate_and_net_errors_without_the_token() {
    let stand = Stand::start();
    stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let (seen, observer) = recorder();
    let token = stand.token.clone();

    // Ворота закрыты, причина по недосмотру содержит токен: в событие он попасть не должен.
    let mut cfg = stand.runner_config();
    cfg.observer = Some(observer.clone());
    let reason = format!("идёт задача приложения {token}");
    cfg.admission = Some(Arc::new(move || Admission::Closed(reason.clone())));
    let mut runner = Runner::new(cfg, exec.clone()).unwrap();
    let control = runner.control();
    let handle = std::thread::spawn(move || runner.run());
    assert!(
        wait_until(10, || events(&seen)
            .iter()
            .any(|e| matches!(e, RunnerEvent::Gated(_)))),
        "нет события Gated: {:?}",
        events(&seen)
    );
    control.stop();
    handle.join().unwrap().unwrap();
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        0,
        "за закрытыми воротами исполнитель не вызывался"
    );

    // Сервера по адресу нет: ошибка связи и повтор не раньше чем через секунду.
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let dir2 = stand.dir.path.join("runner-net");
    let mut cfg2 = Config::new(dead, Token::new(token.clone()), dir2.clone());
    cfg2.observer = Some(observer);
    cfg2.poll_secs = Some(0);
    let mut runner2 = Runner::new(cfg2, exec).unwrap();
    let control2 = runner2.control();
    let handle2 = std::thread::spawn(move || runner2.run());
    assert!(
        wait_until(10, || events(&seen)
            .iter()
            .any(|e| matches!(e, RunnerEvent::NetError { .. }))),
        "нет события NetError: {:?}",
        events(&seen)
    );
    control2.stop();
    handle2.join().unwrap().unwrap();

    let all = events(&seen);
    let net = all
        .iter()
        .find_map(|e| match e {
            RunnerEvent::NetError { retry_secs, .. } => Some(*retry_secs),
            _ => None,
        })
        .unwrap();
    assert!(net >= 1, "retry_secs: {net}");
    let shown = format!("{all:?}");
    assert!(!shown.contains(&token), "токен в событиях: {shown}");
    assert!(
        all.iter()
            .any(|e| matches!(e, RunnerEvent::Gated(r) if r.contains("***"))),
        "токен в причине должен быть вычеркнут: {shown}"
    );
    for dir in [stand.runner_dir(), dir2] {
        assert!(!runner_log(&dir).contains(&token), "токен в runner.log");
    }
}

#[test]
fn revoked_token_ends_the_run_with_a_fatal_event() {
    let stand = Stand::start();
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let (seen, observer) = recorder();
    let mut cfg = stand.runner_config();
    cfg.observer = Some(observer);
    let mut runner = Runner::new(cfg, exec).unwrap();
    let core = stand.core();
    auth::revoke_runner(core.db(), "tester", core.clock().now_ms()).unwrap();

    let e = runner
        .run()
        .expect_err("отозванный токен останавливает раннер");

    assert_eq!(e, runner::tee::TOKEN_REJECTED);
    let all = events(&seen);
    assert_eq!(
        all.last(),
        Some(&RunnerEvent::Fatal(runner::tee::TOKEN_REJECTED.to_owned())),
        "{all:?}"
    );
}

#[test]
fn config_debug_shows_flags_only() {
    let stand = Stand::start();
    let (_, observer) = recorder();
    let mut cfg = stand.runner_config();
    let plain = format!("{cfg:?}");
    assert!(
        plain.contains("admission: false") && plain.contains("observer: false"),
        "{plain}"
    );
    cfg.observer = Some(observer);
    cfg.admission = Some(Arc::new(|| Admission::Open));
    let shown = format!("{cfg:?}");
    assert!(shown.contains("token: \"***\""), "{shown}");
    assert!(
        shown.contains("admission: true") && shown.contains("observer: true"),
        "{shown}"
    );
    assert!(!shown.contains(&stand.token), "{shown}");
}
