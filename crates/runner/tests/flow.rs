//! Сквозной путь раннера на настоящем сервере: выдача, демка, исполнитель, результат, итог.

mod common;

use common::{Scenario, ScriptedExecutor, Stand, fake_demo, result_bytes, sha256_hex};
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
    assert_eq!(
        sha256_hex(&std::fs::read(&task.demo).unwrap()),
        sha256_hex(&demo)
    );
    assert_eq!(task.hero_name, "s1mple");
    assert_eq!(task.job_id, id);
    assert!(!task.resume);
    assert_eq!(exec.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}
