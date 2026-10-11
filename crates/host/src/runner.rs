//! Запуск внешней утилиты (ffmpeg, ffprobe) с жёстким дедлайном и отменой (TMO-01, D-03).
//!
//! Только `std`, без ОС-специфики кроме флага «без окна консоли» в Windows. Дедлайн и отмена
//! проверяются на каждом шаге (не реже раза в 100 мс), процесс завершается своим `Child::kill`,
//! а дочитывание pipe после выхода ограничено 2 с: потомок, унаследовавший pipe, не удерживает вызов.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::{CmdOutput, CmdSpec, HostError};

/// Шаг опроса дедлайна, отмены и состояния процесса.
const STEP: Duration = Duration::from_millis(100);
/// Сколько после выхода процесса дочитываем pipe (потомок мог унаследовать их и держать открытыми).
const DRAIN_LIMIT: Duration = Duration::from_secs(2);
/// Сколько последних строк stderr сохраняется для причины сбоя.
const STDERR_TAIL: usize = 20;
/// Флаг создания процесса `CREATE_NO_WINDOW`: утилита не открывает окно консоли.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Строка из pipe утилиты.
enum Msg {
    Out(String),
    Err(String),
}

/// Читает pipe построчно до EOF; байты не обязаны быть UTF-8, перевод строки отрезается.
fn spawn_reader(pipe: impl Read + Send + 'static, tx: Sender<Msg>, wrap: fn(String) -> Msg) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(pipe);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            let line = String::from_utf8_lossy(&buf).trim_end().to_owned();
            buf.clear();
            if !line.is_empty() && tx.send(wrap(line)).is_err() {
                break;
            }
        }
    });
}

/// Останавливает свой процесс и забирает его статус (чтобы не оставлять зомби).
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Запустить `cmd`, отдавая строки stdout в `on_line` на вызывающем потоке.
///
/// - дольше `cmd.deadline` — процесс останавливается, результат `HostError::Timeout`;
/// - `cancel()` вернул `true` — процесс останавливается, результат `HostError::Cancelled`;
/// - процесс не запустился — `HostError::Spawn`.
pub fn run_bounded(
    cmd: &CmdSpec,
    cancel: &dyn Fn() -> bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<CmdOutput, HostError> {
    let started = Instant::now();
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in crate::SECRET_ENV {
        c.env_remove(name);
    }
    if let Some(cwd) = &cmd.cwd {
        c.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = c
        .spawn()
        .map_err(|e| HostError::Spawn(format!("{}: {e}", cmd.program.display())))?;

    let (tx, rx) = mpsc::channel::<Msg>();
    if let Some(out) = child.stdout.take() {
        spawn_reader(out, tx.clone(), Msg::Out);
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(err, tx.clone(), Msg::Err);
    }
    drop(tx);

    let mut tail: VecDeque<String> = VecDeque::new();
    let mut handle = |msg: Msg, on_line: &mut dyn FnMut(&str)| match msg {
        Msg::Out(line) => on_line(&line),
        Msg::Err(line) => {
            if tail.len() == STDERR_TAIL {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    };

    let status = loop {
        // один шаг ожидания строки; без строк он же задаёт период опроса
        match rx.recv_timeout(STEP) {
            Ok(msg) => handle(msg, on_line),
            Err(RecvTimeoutError::Timeout) => {}
            // оба pipe закрыты, а процесс ещё работает: период опроса держит пауза
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(STEP),
        }
        if cancel() {
            stop(&mut child);
            return Err(HostError::Cancelled);
        }
        if started.elapsed() > cmd.deadline {
            stop(&mut child);
            return Err(HostError::Timeout);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                stop(&mut child);
                return Err(HostError::Io(format!(
                    "состояние {}: {e}",
                    cmd.program.display()
                )));
            }
        }
    };

    // дочитываем, что процесс успел написать, но не дольше DRAIN_LIMIT
    let drain_until = Instant::now() + DRAIN_LIMIT;
    loop {
        let left = drain_until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(STEP)) {
            Ok(msg) => handle(msg, on_line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(CmdOutput {
        code: status.code(),
        stderr_tail: tail.into_iter().collect(),
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::time::Instant;

    fn spec(windows: (&str, &[&str]), other: (&str, &[&str]), deadline: Duration) -> CmdSpec {
        let (program, args) = if cfg!(windows) { windows } else { other };
        CmdSpec {
            program: PathBuf::from(program),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            deadline,
        }
    }

    fn long_running(deadline: Duration) -> CmdSpec {
        spec(
            ("ping", &["-n", "30", "127.0.0.1"]),
            ("sleep", &["30"]),
            deadline,
        )
    }

    #[test]
    fn bounded_run_returns_the_exit_code() {
        let cmd = spec(
            ("cmd", &["/C", "exit 3"]),
            ("sh", &["-c", "exit 3"]),
            Duration::from_secs(10),
        );
        let out = run_bounded(&cmd, &|| false, &mut |_| {}).expect("процесс должен завершиться");
        assert_eq!(out.code, Some(3));
        assert!(out.elapsed < Duration::from_secs(5), "{:?}", out.elapsed);
    }

    #[test]
    fn bounded_run_kills_on_deadline() {
        let started = Instant::now();
        let r = run_bounded(
            &long_running(Duration::from_millis(300)),
            &|| false,
            &mut |_| {},
        );
        assert_eq!(r, Err(HostError::Timeout));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn bounded_run_stops_on_cancel() {
        let started = Instant::now();
        let r = run_bounded(
            &long_running(Duration::from_secs(60)),
            &|| started.elapsed() > Duration::from_millis(200),
            &mut |_| {},
        );
        assert_eq!(r, Err(HostError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn bounded_run_streams_stdout_lines() {
        let cmd = spec(
            ("cmd", &["/C", "echo one& echo two"]),
            ("sh", &["-c", "echo one; echo two"]),
            Duration::from_secs(10),
        );
        let mut lines = Vec::new();
        let out = run_bounded(&cmd, &|| false, &mut |l| lines.push(l.to_owned())).unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(lines, vec!["one".to_owned(), "two".to_owned()]);
    }

    /// Токен раннера не доходит до дочернего процесса (WR-07).
    #[test]
    fn bounded_run_does_not_pass_the_runner_token_to_the_child() {
        // SAFETY: имя переменной используется только этим тестом.
        unsafe { std::env::set_var("CS2_RUNNER_TOKEN", "secret-for-test") };
        let cmd = spec(
            (
                "cmd",
                &["/C", "if defined CS2_RUNNER_TOKEN (echo SET) else (echo UNSET)"],
            ),
            ("sh", &["-c", "[ -n \"${CS2_RUNNER_TOKEN+x}\" ] && echo SET || echo UNSET"]),
            Duration::from_secs(10),
        );
        let mut lines = Vec::new();
        run_bounded(&cmd, &|| false, &mut |l| lines.push(l.to_owned())).unwrap();
        assert_eq!(lines, vec!["UNSET".to_owned()]);
    }

    /// Потомок утилиты держит её pipe открытыми: вызов всё равно возвращается после DRAIN_LIMIT.
    #[cfg(windows)]
    #[test]
    fn run_returns_within_the_drain_limit_when_a_grandchild_holds_the_pipe() {
        let cmd = CmdSpec {
            program: PathBuf::from("cmd"),
            args: vec!["/C".into(), "start /b ping -n 12 127.0.0.1".into()],
            cwd: None,
            deadline: Duration::from_secs(30),
        };
        let started = Instant::now();
        let out = run_bounded(&cmd, &|| false, &mut |_| {}).expect("cmd выходит сразу");
        assert_eq!(out.code, Some(0));
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn run_reports_a_missing_program_as_spawn() {
        let calls = Cell::new(0);
        let cmd = CmdSpec {
            program: PathBuf::from("cs2cinema-no-such-program"),
            args: vec![],
            cwd: None,
            deadline: Duration::from_secs(1),
        };
        let r = run_bounded(&cmd, &|| false, &mut |_| calls.set(calls.get() + 1));
        assert!(matches!(r, Err(HostError::Spawn(_))), "{r:?}");
        assert_eq!(calls.get(), 0);
    }
}
