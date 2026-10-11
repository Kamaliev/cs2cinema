//! Сквозной путь раннера на настоящем сервере: выдача, демка, исполнитель, результат, итог.

mod common;

use common::{Scenario, ScriptedExecutor, Stand, fake_demo, result_bytes};
use pipeline::{FailCode, RecordState};
use runner::{Runner, Tick};
use wire::JobState;

const MIB: usize = 1024 * 1024;

#[test]
fn submitted_demo_reaches_done_through_the_runner() {
    let stand = Stand::start();
    let demo = fake_demo(9 * MIB);
    let id = stand.submit(&demo, "s1mple");

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(17 * MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(id).state, JobState::Done);

    // Принятый сервером результат равен тому, что отдал исполнитель.
    let expected = result_bytes(17 * MIB);
    let stored = std::fs::read(stand.results_dir(id).join("highlights.mp4")).unwrap();
    assert!(stored == expected, "результат на сервере отличается");

    // Исполнитель получил демку под именем из номера задачи, а не под именем от пользователя.
    let task = exec.last_task();
    assert!(
        task.demo.ends_with(format!("remote/srv{id}.dem")),
        "{:?}",
        task.demo
    );
    assert!(task.demo.starts_with(stand.runner_dir()), "{:?}", task.demo);
    // Принятая сервером задача освобождает диск: демка раннера удалена (WR-03).
    assert!(!task.demo.exists(), "демка осталась после принятого результата");
    assert_eq!(task.hero_name, "s1mple");
    assert_eq!(task.job_id, id);
    assert!(!task.resume);
    assert_eq!(exec.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// Прогоняет одну задачу с заданным сценарием исполнителя.
fn settle(scenario: Scenario) -> (Stand, i64, Runner, Tick) {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![scenario]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    let tick = runner.tick().unwrap();
    (stand, id, runner, tick)
}

#[test]
fn empty_queue_tick_is_idle() {
    let (stand, id, mut runner, tick) = settle(Scenario::Result(MIB));
    assert_eq!(
        tick,
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(runner.tick().unwrap(), Tick::Idle);
    assert_eq!(stand.job(id).state, JobState::Done);
}

#[test]
fn executor_failure_reaches_the_server_with_its_code() {
    let (stand, id, _runner, tick) = settle(Scenario::Fail {
        code: FailCode::TakeTimeout,
        state: RecordState::Take,
    });
    assert_eq!(
        tick,
        Tick::Settled {
            job_id: id,
            state: JobState::Failed
        }
    );
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("TAKE_TIMEOUT"));
    assert_eq!(job.fail_state.as_deref(), Some("take"));
    assert_eq!(job.fail_class.as_deref(), Some("прочее"));
    assert!(
        job.fail_message
            .as_deref()
            .is_some_and(|m| m.contains("дубль не записан")),
        "{:?}",
        job.fail_message
    );
    assert!(
        job.fail_tail
            .as_deref()
            .is_some_and(|t| t.contains("строка консоли")),
        "{:?}",
        job.fail_tail
    );
    assert!(job.fail_elapsed_ms.is_some());
}

#[test]
fn executor_error_without_a_code_is_job_error() {
    let (stand, id, _runner, _) = settle(Scenario::Plain("сломалось\nвторая строка".to_owned()));
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("JOB_ERROR"));
    let message = job.fail_message.unwrap();
    assert!(message.contains("сломалось"), "{message}");
    assert!(!message.contains('\n'), "{message:?}");
}

#[test]
fn executor_cancel_closes_the_job_as_cancelled() {
    let (stand, id, _runner, tick) = settle(Scenario::Cancel);
    assert_eq!(
        tick,
        Tick::Settled {
            job_id: id,
            state: JobState::Cancelled
        }
    );
    assert_eq!(stand.job(id).state, JobState::Cancelled);
}

#[test]
fn runner_log_never_contains_the_token() {
    let (stand, _id, _runner, _) = settle(Scenario::Result(MIB));
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    assert!(!log.is_empty(), "runner.log пуст");
    assert!(!log.contains(&stand.token), "токен в runner.log");
    let shown = format!("{:?}", stand.runner_config());
    assert!(!shown.contains(&stand.token), "токен в Debug: {shown}");
    assert!(shown.contains("***"), "{shown}");
}
