//! Бинарник `cs2-cli retry` и `cs2-cli record`: ошибки аргументов и объяснимые отказы без игры.

use std::{
    path::PathBuf,
    process::{Command, Output},
};

/// Пустая временная папка; id процесса и метка теста исключают столкновения.
fn job_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cs2cinema-cli-record-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cs2-cli"))
        .args(args)
        .output()
        .expect("cs2-cli запускается")
}

#[test]
fn retry_without_record_json_exits_with_retry_no_data() {
    let dir = job_dir("no-record-json");
    let out = cli(&["retry".as_ref(), dir.as_os_str()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("RETRY_NO_DATA"), "{stderr}");
    assert!(stderr.contains("запустите задачу заново"), "{stderr}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn retry_without_a_folder_prints_usage() {
    let out = cli(&["retry".as_ref()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("укажите папку задачи"), "{stderr}");
    assert!(stderr.contains("cs2-cli retry <папка задачи>"), "{stderr}");
}

#[test]
fn record_without_player_prints_usage() {
    let out = cli(&["record".as_ref(), "match.dem".as_ref()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--player"), "{stderr}");
    assert!(stderr.contains("cs2-cli record <ссылка FACEIT"), "{stderr}");
}
