//! Сквозной тест `WinSession` на стенд-процессах: настоящие Job Object, сокет и процессы Windows,
//! но вместо HLAE и CS2 — `cmd`/`ping` и тестовый TCP-сервер вместо консоли игры.
#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use host::win::WinHost;
use host::{HostError, Liveness, Session, SessionEvent, Source};

/// Лаунчер-заглушка: `cmd` выходит с кодом 0, а `PING.EXE` остаётся в job и изображает игру.
fn stand_in_launcher() -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "start /b ping -n 30 127.0.0.1 >nul"]);
    cmd
}

/// Читает события, пока `done` не станет истиной по накопленному списку или не выйдет лимит.
fn poll_until(
    session: &mut dyn Session,
    limit: Duration,
    events: &mut Vec<SessionEvent>,
    done: impl Fn(&[SessionEvent]) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(ev) = session.poll(Duration::from_millis(100)) {
            events.push(ev);
        }
        if done(events) {
            return true;
        }
    }
    done(events)
}

#[test]
fn stand_in_launcher_game_and_console_flow_through_one_session() {
    // консоль игры: принимает одно подключение, здоровается и пересылает тесту первую строку
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.write_all(b"hello from game\n");
        let mut r = BufReader::new(stream);
        let mut line = String::new();
        if r.read_line(&mut line).is_ok() {
            let _ = line_tx.send(line.trim().to_owned());
        }
    });

    let mut session = WinHost::new()
        .launch_with(stand_in_launcher(), "ping.exe", port, None)
        .expect("сессия должна запуститься");

    let mut events = Vec::new();
    let all_seen = poll_until(
        session.as_mut(),
        Duration::from_secs(10),
        &mut events,
        |e| {
            let has = |f: &dyn Fn(&SessionEvent) -> bool| e.iter().any(f);
            has(&|x| matches!(x, SessionEvent::LauncherExited { code: Some(0) }))
                && has(&|x| matches!(x, SessionEvent::GameAppeared { .. }))
                && has(&|x| matches!(x, SessionEvent::ConsoleUp))
                && has(
                    &|x| matches!(x, SessionEvent::Line { source: Source::Netcon, text } if text == "hello from game"),
                )
        },
    );
    assert!(
        all_seen,
        "за 10 с не пришли все события, пришло: {events:?}"
    );
    assert_eq!(session.launcher_exit(), Some(Some(0)));

    // команда уходит в консоль игры
    session
        .send("echo test")
        .expect("send в подключённую консоль");
    assert_eq!(
        line_rx.recv_timeout(Duration::from_secs(5)).as_deref(),
        Ok("echo test")
    );
    assert_eq!(session.game_liveness(), Liveness::Alive);
    assert_eq!(session.send("a\nb"), Err(HostError::BadCommand));
    assert_eq!(session.send(""), Err(HostError::BadCommand));

    // завершение: игра умирает, job пустеет; повторный вызов безвреден
    session.terminate();
    let gone = poll_until(session.as_mut(), Duration::from_secs(5), &mut events, |e| {
        e.iter()
            .any(|x| matches!(x, SessionEvent::GameExited { .. } | SessionEvent::JobEmpty))
    });
    assert!(
        gone,
        "после terminate не пришло GameExited/JobEmpty: {events:?}"
    );
    assert!(
        matches!(session.game_liveness(), Liveness::Exited(_)),
        "{:?}",
        session.game_liveness()
    );
    session.terminate();
}

/// Временный файл `console.log`; удаляется в `Drop`.
struct TempLog(std::path::PathBuf);

impl Drop for TempLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn console_log_tail_starts_at_the_launch_offset() {
    use std::io::Write;

    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let log = TempLog(std::env::temp_dir().join(format!(
        "cs2cinema-win-session-{}-{n}.log",
        std::process::id()
    )));
    // прошлый запуск: BOM и старая метка, которую нельзя принять за новую
    std::fs::write(
        &log.0,
        b"\xef\xbb\xbf10/10 16:00:00 CS2C ab12cd34 READY\r\n",
    )
    .unwrap();

    // порт, на котором никто не слушает: консоль не подключается, нужен только хвост файла
    let port = host::win::netcon::free_port();
    let mut session = WinHost::new()
        .launch_with(stand_in_launcher(), "ping.exe", port, Some(log.0.clone()))
        .expect("сессия должна запуститься");

    let mut events = Vec::new();
    assert!(
        poll_until(
            session.as_mut(),
            Duration::from_secs(10),
            &mut events,
            |e| {
                e.iter()
                    .any(|x| matches!(x, SessionEvent::GameAppeared { .. }))
            }
        ),
        "игра-заглушка не появилась: {events:?}"
    );

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&log.0)
        .unwrap();
    f.write_all("строка 1\n".as_bytes()).unwrap();
    f.write_all("вторая ".as_bytes()).unwrap();
    f.flush().unwrap();
    let has = |e: &[SessionEvent], want: &str| {
        e.iter().any(|x| {
            matches!(x, SessionEvent::Line { source: Source::ConsoleLog, text } if text == want)
        })
    };
    assert!(
        poll_until(session.as_mut(), Duration::from_secs(5), &mut events, |e| {
            has(e, "строка 1")
        }),
        "первая строка хвоста не пришла: {events:?}"
    );
    assert!(
        !has(&events, "вторая"),
        "обрывок строки не должен приходить"
    );

    f.write_all("целая\n".as_bytes()).unwrap();
    f.flush().unwrap();
    assert!(
        poll_until(session.as_mut(), Duration::from_secs(5), &mut events, |e| {
            has(e, "вторая целая")
        }),
        "склеенная строка не пришла: {events:?}"
    );
    for ev in &events {
        if let SessionEvent::Line {
            source: Source::ConsoleLog,
            text,
        } = ev
        {
            assert!(!text.contains("READY"), "старая строка пришла: {text:?}");
            assert!(!text.contains('\u{feff}'), "BOM в тексте: {text:?}");
        }
    }
    session.terminate();
}
