//! Запуск внешней утилиты (ffmpeg, ffprobe) с жёстким дедлайном и отменой (TMO-01, D-03).
//!
//! Только `std`, без ОС-специфики кроме флага «без окна консоли» в Windows. Дедлайн и отмена
//! проверяются на каждом шаге (не реже раза в 100 мс), процесс завершается своим `Child::kill`,
//! а дочитывание pipe после выхода ограничено 2 с: потомок, унаследовавший pipe, не удерживает вызов.

use std::time::Duration;

use crate::{CmdOutput, CmdSpec, HostError};

/// Запустить `cmd`, отдавая строки stdout в `on_line` на вызывающем потоке. RED: пока заглушка.
pub fn run_bounded(
    cmd: &CmdSpec,
    cancel: &dyn Fn() -> bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<CmdOutput, HostError> {
    let _ = (cmd, cancel, on_line, Duration::ZERO);
    Err(HostError::Unsupported)
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
