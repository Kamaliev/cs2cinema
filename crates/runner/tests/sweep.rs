//! Уборка рабочей папки раннера: сиротские дубли, скачанные демки, журнал, подтверждённые задачи.
//! Возраст файлов задаётся `File::set_modified`, сервер — настоящий (`Stand`).

mod common;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Scenario, ScriptedExecutor, Stand};
use runner::{Runner, Tick};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// Создаёт файл с содержимым и ставит ему изменение `age` назад.
fn put(path: &Path, age: Duration) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
    path.to_path_buf()
}

fn days(n: u64) -> Duration {
    Duration::from_secs(n * DAY)
}

fn runner_log(stand: &Stand) -> String {
    fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap_or_default()
}

#[test]
fn idle_runner_sweeps_old_orphan_take_folders_and_keeps_fresh_ones() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    // Имя как у настоящего `run_job`: метка демки и герой.
    put(&dir.join("out/srv7_s1mple/take1.mp4"), days(4));
    put(&dir.join("out/srv8/take1.mp4"), Duration::ZERO);

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    assert!(
        !dir.join("out/srv7_s1mple").exists(),
        "старая сиротская папка осталась"
    );
    assert!(dir.join("out/srv8").exists(), "свежая папка удалена");
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("уборка:") && l.contains("#7")),
        "в журнале нет итога уборки:\n{log}"
    );
}
