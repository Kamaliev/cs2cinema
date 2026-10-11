//! Политика итогов раннера на настоящем сервере: класс сбоя решает, вернётся задача в очередь через `release`
//! (выдача засчитывается всегда, D-02) или закроется со своим кодом; пауза после двух одинаковых неисправностей;
//! потолок выдач `TOO_MANY_GRANTS`; страж «коды пайплайна известны протоколу».

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Scenario, ScriptedExecutor, Stand, fake_demo};
use pipeline::job::Outcome;
use pipeline::{FailCode, Progress, RecordFailure, RecordState, RunError};
use runner::{Executor, HeldJob, Link, Runner, RunnerState, Stage, Task, Tick, Token};
use wire::JobState;

const MIB: usize = 1024 * 1024;

fn env_failure() -> Scenario {
    Scenario::Fail {
        code: FailCode::ForeignGameRunning,
        state: RecordState::Precheck,
    }
}

/// Раннер стенда с исполнителем-заглушкой по сценариям.
fn start(stand: &Stand, scenarios: Vec<Scenario>) -> (Runner, Arc<ScriptedExecutor>) {
    let exec = ScriptedExecutor::new(stand.runner_dir(), scenarios);
    let runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    (runner, exec)
}

fn runner_state(stand: &Stand) -> RunnerState {
    let (state, warning) = RunnerState::load(&stand.runner_dir());
    assert!(warning.is_none(), "{warning:?}");
    state
}

#[test]
fn environment_failure_releases_the_job_back_to_the_queue() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let (mut runner, exec) = start(&stand, vec![env_failure()]);

    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: id });

    let job = stand.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.grants, 1, "выдача должна быть засчитана");
    assert_eq!(job.runner_id, None);
    assert_eq!(job.fail_code, None, "finish не вызывался");
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    // Состояние раннера пусто, демка убрана: следующая выдача скачает её заново.
    assert!(runner_state(&stand).job.is_none());
    assert!(
        !stand
            .runner_dir()
            .join("remote")
            .join(format!("srv{id}.dem"))
            .exists()
    );
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    assert!(log.contains("возвращена в очередь"), "{log}");
}

#[test]
fn launch_failure_releases_the_job_with_a_counted_grant() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let (mut runner, _) = start(
        &stand,
        vec![Scenario::Fail {
            code: FailCode::GameNotStarted,
            state: RecordState::Game,
        }],
    );

    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: id });
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.grants, 1);
}

#[test]
fn content_failure_finishes_the_job_with_its_code() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let (mut runner, exec) = start(
        &stand,
        vec![
            env_failure(),
            Scenario::Fail {
                code: FailCode::BadDemo,
                state: RecordState::Take,
            },
            env_failure(),
            Scenario::Result(MIB),
        ],
    );

    // Первая выдача: неисправность окружения, задача возвращается.
    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: id });
    // Вторая выдача той же задачи: демка плохая, задача закрыта своим кодом.
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Failed
        }
    );
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("BAD_DEMO"));
    assert_eq!(job.fail_class.as_deref(), Some("прочее"));

    // Контентный сбой сбросил серию: ещё одна неисправность окружения паузы не даёт.
    let second = stand.submit(&fake_demo(MIB), "s1mple");
    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: second });
    // И следующая задача берётся как обычно.
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: second,
            state: JobState::Done
        }
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 4);
}

/// Исполнитель, который отменяет задачу на сервере, ждёт, пока раннер об этом узнает, и сообщает неисправность
/// окружения: так выглядит отмена оператором в момент сбоя.
struct CancelThenEnvFailure {
    core: server::core::Core,
    calls: AtomicUsize,
}

impl Executor for CancelThenEnvFailure {
    fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.core.cancel(task.job_id, false).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !p.cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(p.cancelled(), "раннер не узнал об отмене за 5 с");
        let fail = RecordFailure::new(
            FailCode::ForeignGameRunning,
            RecordState::Precheck,
            "запущена чужая игра".to_owned(),
            Duration::from_millis(10),
        );
        Err(RunError {
            message: "запущена чужая игра".to_owned(),
            fail: Some(fail),
        })
    }
}

#[test]
fn cancel_requested_before_release_closes_the_job_as_cancelled() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = Arc::new(CancelThenEnvFailure {
        core: stand.core(),
        calls: AtomicUsize::new(0),
    });
    let mut cfg = stand.runner_config();
    cfg.heartbeat = Some(Duration::from_millis(100));
    let mut runner = Runner::new(cfg, exec.clone()).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Cancelled
        }
    );
    let job = stand.job(id);
    assert_eq!(
        job.state,
        JobState::Cancelled,
        "release вернул бы задачу в очередь (WR-01)"
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    assert!(runner_state(&stand).job.is_none());
}

#[test]
fn pending_release_is_repeated_after_restart() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");

    // «Убитый» раннер: задача выдана по-настоящему, решение о `release` записано, запрос не ушёл.
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    let a = link.claim(0).unwrap().expect("задача должна быть выдана");
    assert_eq!(a.job_id, id);
    std::fs::create_dir_all(stand.runner_dir().join("remote")).unwrap();
    let mut held = HeldJob::claimed(
        &a,
        stand
            .runner_dir()
            .join("remote")
            .join(format!("srv{id}.dem")),
    );
    held.stage = Stage::Reporting;
    held.attempt = 1;
    held.pending_release = Some("FOREIGN_GAME_RUNNING: запущена чужая игра".to_owned());
    RunnerState {
        job: Some(held),
        ..Default::default()
    }
    .save(&stand.runner_dir())
    .unwrap();
    assert_eq!(stand.job(id).state, JobState::Assigned);

    // Новый раннер в той же папке повторяет `release`, исполнителя не запускает.
    let (mut runner, exec) = start(&stand, vec![Scenario::Result(MIB)]);
    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: id });
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.grants, 1);
    assert_eq!(exec.calls.load(Ordering::SeqCst), 0);
    assert!(runner_state(&stand).job.is_none());
}
