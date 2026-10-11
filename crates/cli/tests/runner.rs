//! `cs2-cli runner` через собранный бинарь: справка, отказ от токена в командной строке, замок второй копии.
//! Живой сервер здесь не нужен: порт 9 закрыт, раннер ждёт с повторами.

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_cs2-cli");

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cs2cinema-runner-it-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Команда без токена и адреса из окружения тестового процесса.
fn runner() -> Command {
    let mut c = Command::new(EXE);
    c.arg("runner")
        .env_remove("CS2_RUNNER_TOKEN")
        .env_remove("CS2_RUNNER_SERVER")
        .stdin(Stdio::null());
    c
}

/// Дожидается выхода не дольше 20 с; зависший процесс убивается, тест падает.
fn wait_output(mut child: Child) -> Output {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("cs2-cli runner не завершился за 20 с");
}

#[test]
fn runner_help_mentions_the_token_variable_and_the_app_queue() {
    let out = runner()
        .arg("--help")
        .output()
        .expect("cs2-cli запускается");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("CS2_RUNNER_TOKEN"), "{text}");
    assert!(text.contains("одновременно"), "{text}");
    assert!(
        !text.contains("upload-chunk-limit"),
        "флаг учений скрыт: {text}"
    );
}

#[test]
fn token_on_the_command_line_is_refused() {
    let out = runner()
        .args(["--token", "x", "--server", "http://127.0.0.1:9"])
        .output()
        .expect("cs2-cli запускается");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("ошибка:"), "{err}");
    assert!(err.contains("CS2_RUNNER_TOKEN"), "{err}");
}

#[test]
fn second_runner_in_the_same_dir_exits_because_of_the_lock() {
    let d = dir("lock");
    let start = || {
        let mut c = runner();
        c.args(["--server", "http://127.0.0.1:9", "--dir"])
            .arg(&d)
            .env("CS2_RUNNER_TOKEN", "test-token")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        c
    };
    let mut first = start()
        .stderr(Stdio::null())
        .spawn()
        .expect("первый раннер");
    // `runner.log` создаёт `Runner::new` уже после замка: к этому моменту замок точно занят.
    let until = Instant::now() + Duration::from_secs(10);
    while !d.join("runner.log").exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        d.join("runner.lock").exists(),
        "первый раннер не занял замок"
    );
    assert!(
        d.join("runner.log").exists(),
        "первый раннер не стартовал за 10 с"
    );

    let second = wait_output(start().spawn().expect("второй раннер"));
    let _ = first.kill();
    let _ = first.wait();
    assert_eq!(second.status.code(), Some(2));
    let err = String::from_utf8_lossy(&second.stderr);
    assert!(err.contains("уже запущен"), "{err}");
}

#[test]
fn general_help_mentions_the_runner() {
    let out = Command::new(EXE).output().expect("cs2-cli запускается");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("cs2-cli runner --server"), "{text}");
}
