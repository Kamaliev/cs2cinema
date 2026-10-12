//! Ворота допуска на настоящем сервере: закрытые ворота — нет `claim`, раннер «на связи»; задача, выданная в момент
//! закрытия, возвращается без засчёта выдачи.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::{Scenario, ScriptedExecutor, Stand, fake_demo};
use runner::{Admission, Config, Runner, RunnerState, Tick};
use wire::JobState;

const MIB: usize = 1024 * 1024;

/// Статус раннера в базе сервера, как его видит оператор.
fn runner_status(stand: &Stand) -> String {
    stand
        .core()
        .db()
        .read(|c| {
            c.query_row(
                "SELECT status FROM runners WHERE name = 'tester'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map_err(server::db::db_err)
        })
        .expect("статус раннера читается")
}

/// Настройки стенда с воротами.
fn gated(stand: &Stand, gate: impl Fn() -> Admission + Send + Sync + 'static) -> Config {
    let mut cfg = stand.runner_config();
    cfg.admission = Some(Arc::new(gate));
    cfg
}

#[test]
fn closed_gate_takes_no_job_and_keeps_the_runner_online() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let cfg = gated(&stand, || {
        Admission::Closed("идёт задача приложения".to_owned())
    });
    let mut runner = Runner::new(cfg, exec.clone()).unwrap();

    for _ in 0..2 {
        assert_eq!(
            runner.tick().unwrap(),
            Tick::Gated("идёт задача приложения".to_owned())
        );
    }

    let job = stand.job(id);
    assert_eq!(
        job.state,
        JobState::Queued,
        "задача должна остаться в очереди сервера"
    );
    assert_eq!(job.grants, 0, "claim не вызывался, выдач быть не должно");
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        0,
        "исполнитель не должен вызываться"
    );
    assert!(
        stand.core().runner_online().unwrap(),
        "раннер за воротами должен быть на связи"
    );
    let status = runner_status(&stand);
    assert!(status.starts_with("занят:"), "статус раннера: {status:?}");
    assert!(
        status.contains("идёт задача приложения"),
        "статус раннера: {status:?}"
    );
}

#[test]
fn opened_gate_takes_the_job_as_before() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let open = Arc::new(AtomicBool::new(false));
    let flag = open.clone();
    let cfg = gated(&stand, move || {
        if flag.load(Ordering::SeqCst) {
            Admission::Open
        } else {
            Admission::Closed("очередь занята".to_owned())
        }
    });
    let mut runner = Runner::new(cfg, exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Gated("очередь занята".to_owned())
    );
    assert_eq!(stand.job(id).state, JobState::Queued);

    open.store(true, Ordering::SeqCst);
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(id).state, JobState::Done);
    assert_eq!(stand.job(id).grants, 1);
}

#[test]
fn gate_closing_after_claim_returns_the_job_without_counting_the_grant() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    // Первый вызов (перед claim) открыт, дальше закрыто: автор успел поставить свою задачу.
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let reopen = Arc::new(AtomicBool::new(false));
    let reopened = reopen.clone();
    let cfg = gated(&stand, move || {
        if reopened.load(Ordering::SeqCst) || counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Admission::Open
        } else {
            Admission::Closed("в очереди приложения есть задачи".to_owned())
        }
    });
    let mut runner = Runner::new(cfg, exec.clone()).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Gated("в очереди приложения есть задачи".to_owned())
    );
    let job = stand.job(id);
    assert_eq!(
        job.state,
        JobState::Queued,
        "задача должна вернуться в очередь сервера"
    );
    assert_eq!(job.grants, 0, "выдача не должна засчитываться");
    assert_eq!(job.runner_id, None);
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        0,
        "исполнитель не должен вызываться"
    );
    let (state, warning) = RunnerState::load(&stand.runner_dir());
    assert!(warning.is_none(), "{warning:?}");
    assert!(
        state.job.is_none(),
        "в runner_state.json не должно быть задачи"
    );

    // Ворота открылись: задача доходит до конца, и теперь выдача одна.
    reopen.store(true, Ordering::SeqCst);
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Done);
    assert_eq!(job.grants, 1);
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
}
