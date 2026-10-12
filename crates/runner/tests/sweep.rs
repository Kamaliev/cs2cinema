//! Уборка рабочей папки раннера: сиротские дубли, скачанные демки, журнал, подтверждённые задачи.
//! Возраст файлов задаётся `File::set_modified`, сервер — настоящий (`Stand`).

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{Scenario, ScriptedExecutor, Stand, TestDir};
use runner::{Runner, Tick, sweep};

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

#[test]
fn old_remote_demos_parts_and_faceit_folders_are_swept() {
    let tmp = TestDir::new("sweep-remote");
    let dir = &tmp.path;
    let old = days(2);
    put(&dir.join("remote/srv9.dem"), old);
    put(&dir.join("remote/srv9.dem.part"), old);
    put(&dir.join("remote/srv9.faceit/demo.dem.zst"), old);
    put(&dir.join("remote/srv10.dem"), Duration::ZERO);

    let swept = sweep::sweep(dir, None, SystemTime::now());

    assert!(!dir.join("remote/srv9.dem").exists());
    assert!(!dir.join("remote/srv9.dem.part").exists());
    assert!(!dir.join("remote/srv9.faceit").exists());
    assert!(
        dir.join("remote/srv10.dem").exists(),
        "свежая демка удалена"
    );
    assert_eq!(swept.remote_items, 3, "{swept:?}");
    assert_eq!(swept.errors, 0);
}

#[test]
fn held_job_is_never_swept() {
    let tmp = TestDir::new("sweep-held");
    let dir = &tmp.path;
    let ancient = days(10);
    put(&dir.join("out/srv5_s1mple/take.mp4"), ancient);
    put(&dir.join("remote/srv5.dem"), ancient);
    put(&dir.join("out/srv6_s1mple/take.mp4"), ancient);

    let swept = sweep::sweep(dir, Some(5), SystemTime::now());

    assert!(dir.join("out/srv5_s1mple/take.mp4").exists());
    assert!(dir.join("remote/srv5.dem").exists());
    assert!(
        !dir.join("out/srv6_s1mple").exists(),
        "чужая старая папка осталась"
    );
    assert_eq!(swept.out_dirs, vec![6]);
}

#[test]
fn foreign_names_and_links_are_left_alone() {
    let tmp = TestDir::new("sweep-foreign");
    let dir = &tmp.path;
    let ancient = days(30);
    let keep = [
        "out/notes.txt",
        "out/srvX/a.bin",
        "out/srv12abc/a.bin",
        "out/srv_12/a.bin",
        "out/srv/a.bin",
        "out/srv012/a.bin",
        "remote/other.dem",
        "remote/srv3.mp4",
        "remote/srv.dem",
        "remote/srv3x.dem",
        "remote/srv3.dem.bak",
    ];
    for name in keep {
        put(&dir.join(name), ancient);
    }
    // Файл с именем папки дублей — не папка.
    put(&dir.join("out/srv14"), ancient);
    // Файл вне out/ и remote/ рабочей папки.
    put(&dir.join("srv15/take.mp4"), ancient);
    put(&dir.join("runner.log"), ancient);

    #[cfg(unix)]
    let outside = {
        let outside = TestDir::new("sweep-outside");
        put(&outside.path.join("take.mp4"), ancient);
        std::os::unix::fs::symlink(&outside.path, dir.join("out/srv13")).unwrap();
        outside
    };

    let swept = sweep::sweep(dir, None, SystemTime::now());

    for name in keep {
        assert!(dir.join(name).exists(), "удалено чужое: {name}");
    }
    assert!(dir.join("out/srv14").is_file());
    assert!(dir.join("srv15/take.mp4").exists());
    assert!(dir.join("runner.log").exists());
    assert!(swept.is_empty(), "{swept:?}");
    #[cfg(unix)]
    {
        assert!(dir.join("out/srv13").is_symlink(), "ссылка удалена");
        assert!(
            outside.path.join("take.mp4").exists(),
            "цель ссылки удалена"
        );
    }
}

/// Заполняет журнал строками «СТАРОЕ …» так, что он больше `bytes`.
fn fill_log(path: &Path, bytes: u64) {
    let line = "СТАРОЕ строка журнала раннера\n";
    let mut out = std::io::BufWriter::new(File::create(path).unwrap());
    let mut written = 0u64;
    while written <= bytes {
        out.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
    }
    out.flush().unwrap();
}

#[test]
fn runner_log_is_rotated_when_it_grows_too_large() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    fs::create_dir_all(&dir).unwrap();
    fill_log(&dir.join("runner.log"), sweep::LOG_MAX_BYTES);

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    let old = fs::read_to_string(dir.join("runner.log.1")).unwrap();
    assert!(old.contains("СТАРОЕ"), "прежний журнал не перенесён в .1");
    let new = fs::read_to_string(dir.join("runner.log")).unwrap();
    assert!(
        new.len() < 1 << 20,
        "новый журнал велик: {} байт",
        new.len()
    );
    assert!(
        !new.contains("СТАРОЕ"),
        "старые строки остались в новом журнале"
    );
    assert!(
        new.contains("на связи с сервером"),
        "в новом журнале нет строк:\n{new}"
    );
    assert!(
        !old.contains(&stand.token) && !new.contains(&stand.token),
        "токен в журнале"
    );
}

#[test]
fn runner_log_is_rotated_on_the_sweep_pass() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    // Журнал разросся, пока раннер работал: его хэндл открыт, запись идёт в конец.
    {
        let mut f = File::options()
            .append(true)
            .open(dir.join("runner.log"))
            .unwrap();
        let line = "СТАРОЕ строка журнала раннера\n";
        let mut written = 0u64;
        while written <= sweep::LOG_MAX_BYTES {
            f.write_all(line.as_bytes()).unwrap();
            written += line.len() as u64;
        }
    }
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    let old = fs::read_to_string(dir.join("runner.log.1")).unwrap();
    assert!(old.contains("СТАРОЕ"));
    let new = fs::read_to_string(dir.join("runner.log")).unwrap_or_default();
    assert!(!new.contains("СТАРОЕ"));
    // Новые строки идут уже в новый файл.
    let before = new.len();
    runner.tick().unwrap();
    let after = fs::read_to_string(dir.join("runner.log")).unwrap().len();
    assert!(after >= before);
}

#[test]
fn sweep_runs_at_most_once_per_interval() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    put(&dir.join("out/srv21/take1.mp4"), days(4));
    assert_eq!(runner.tick().unwrap(), Tick::Idle);
    assert!(
        dir.join("out/srv21").exists(),
        "уборка прошла раньше, чем через SWEEP_EVERY"
    );
}
