//! Windows-часть хоста: Job Object, процессы и сессия игры `WinHost` (D-02, D-04).
//!
//! Устройство сессии: лаунчер (HLAE) запускается в собственном job, а дальше работают независимые
//! потоки — наблюдатель (`watch`: выход лаунчера, игра в job, смерть игры, пустой job), консоль
//! (`netcon`) и чтение pipe лаунчера. Все они пишут события в один канал, а [`WinSession::poll`]
//! читает его. Потоки не ждутся через `join` (pipe HLAE закрываются только при выходе игры,
//! спайк S1): каждый заканчивается сам — по EOF, по ошибке или по флагу `stop`.
//!
//! Весь `unsafe` фазы 1 живёт только здесь, у каждого блока есть `// SAFETY:`.

pub mod console_log;
pub mod job;
pub mod netcon;
pub mod process;
pub mod watch;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{
    CmdOutput, CmdSpec, Host, HostError, LaunchSpec, Liveness, Session, SessionEvent, Source,
    console_line_ok,
};
use job::Job;
use process::OwnedProc;

/// Последняя ошибка Win32: код и текст системы.
pub(crate) fn last_error() -> String {
    let e = std::io::Error::last_os_error();
    format!("код {}: {e}", e.raw_os_error().unwrap_or(0))
}

/// Блокировка, которая не паникует из-за отравленного mutex: сессия должна завершаться, а не падать.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Общее состояние потоков одной сессии.
#[derive(Default)]
pub(crate) struct Shared {
    /// Сессия закрывается: все потоки выходят на ближайшем шаге.
    pub stop: AtomicBool,
    /// `None` — лаунчер ещё работает; `Some(None)` — вышел, код неизвестен.
    pub launcher_exit: Mutex<Option<Option<i32>>>,
    /// Процесс игры (handle), когда он найден.
    pub game: Mutex<Option<OwnedProc>>,
    /// Сокет консоли для записи команд.
    pub netcon: Mutex<Option<TcpStream>>,
    /// Игра завершилась: консоль больше не переподключается (порт мог достаться другому процессу).
    pub game_gone: AtomicBool,
    /// Игра принята по PID, но в job не помещена: завершать её нужно своим handle.
    pub adopted_without_job: AtomicBool,
}

impl Shared {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Куда потоки сессии пишут события.
pub(crate) struct Sink {
    tx: Sender<SessionEvent>,
}

impl Sink {
    pub fn send(&self, ev: SessionEvent) {
        // получатель закрыт — сессия уже не нужна, потерять событие не страшно
        let _ = self.tx.send(ev);
    }

    pub fn note(&self, text: impl Into<String>) {
        self.send(SessionEvent::Note(text.into()));
    }

    pub fn line(&self, source: Source, text: String) {
        self.send(SessionEvent::Line { source, text });
    }
}

/// Читает pipe лаунчера построчно до EOF; байты не обязаны быть UTF-8.
fn spawn_pipe_reader(pipe: impl Read + Send + 'static, source: Source, sink: Arc<Sink>) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(pipe);
        let mut buf = Vec::new();
        while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
            let text = String::from_utf8_lossy(&buf).trim().to_owned();
            buf.clear();
            if !text.is_empty() {
                sink.line(source, text);
            }
        }
    });
}

/// Настоящий хост для Windows: игра и утилиты запускаются в этом процессе, владение — через Job Object.
pub struct WinHost {
    started: Instant,
}

impl Default for WinHost {
    fn default() -> Self {
        Self::new()
    }
}

impl WinHost {
    pub fn new() -> Self {
        WinHost {
            started: Instant::now(),
        }
    }

    /// Запустить готовую команду лаунчера в новом job и вернуть сессию.
    ///
    /// `game_image` — имя образа игры (`cs2.exe`), по нему игра ищется среди процессов job;
    /// `port` — порт netcon на `127.0.0.1`; `console_log` — файл `-condebug` (хвост читается с Task 2).
    /// Этот вход нужен тестам со стенд-процессами; настоящий запуск — [`Host::launch`].
    pub fn launch_with(
        &self,
        mut cmd: Command,
        game_image: &str,
        port: u16,
        _console_log: Option<PathBuf>,
    ) -> Result<Box<dyn Session>, HostError> {
        // job создаётся до запуска: лаунчер попадает в него сразу после spawn, потомки наследуют
        let job = Arc::new(Job::new()?);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            HostError::Spawn(format!("{}: {e}", cmd.get_program().to_string_lossy()))
        })?;
        if let Err(e) = job.assign_child(&child) {
            // своего ребёнка завершаем своим handle; по имени образа ничего не трогаем
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }

        let (tx, rx) = mpsc::channel::<SessionEvent>();
        let sink = Arc::new(Sink { tx });
        let shared = Arc::new(Shared::default());

        if let Some(out) = child.stdout.take() {
            spawn_pipe_reader(out, Source::HlaeStdout, sink.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_pipe_reader(err, Source::HlaeStderr, sink.clone());
        }
        netcon::spawn(port, shared.clone(), sink.clone());
        watch::spawn(watch::Watch {
            job: job.clone(),
            child,
            shared: shared.clone(),
            sink: sink.clone(),
            game_image: game_image.to_owned(),
        });

        Ok(Box::new(WinSession {
            rx,
            shared,
            job,
            sink,
            terminated: false,
        }))
    }
}

impl Host for WinHost {
    fn launch(&self, _spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError> {
        Err(HostError::Unsupported)
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn run(
        &self,
        _cmd: &CmdSpec,
        _cancel: &dyn Fn() -> bool,
        _on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError> {
        Err(HostError::Unsupported)
    }
}

/// Сессия игры: события из канала, команды в netcon, факты по handle.
pub struct WinSession {
    rx: Receiver<SessionEvent>,
    shared: Arc<Shared>,
    job: Arc<Job>,
    sink: Arc<Sink>,
    terminated: bool,
}

impl Session for WinSession {
    fn poll(&mut self, max_wait: Duration) -> Option<SessionEvent> {
        match self.rx.recv_timeout(max_wait) {
            Ok(ev) => Some(ev),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                // все потоки закончились: событий больше не будет, но драйвер не должен крутиться вхолостую
                std::thread::sleep(max_wait);
                None
            }
        }
    }

    fn send(&mut self, line: &str) -> Result<(), HostError> {
        if !console_line_ok(line) {
            return Err(HostError::BadCommand);
        }
        let mut guard = lock(&self.shared.netcon);
        let Some(stream) = guard.as_mut() else {
            return Err(HostError::NotConnected);
        };
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        match stream.write_all(&data) {
            Ok(()) => Ok(()),
            Err(e) => {
                // сокет больше не годится: поток консоли увидит разрыв и переподключится
                if let Some(s) = guard.take() {
                    let _ = s.shutdown(Shutdown::Both);
                }
                Err(HostError::Io(format!("консоль игры: {e}")))
            }
        }
    }

    fn game_liveness(&mut self) -> Liveness {
        match lock(&self.shared.game).as_ref() {
            Some(game) => game.liveness(),
            None => Liveness::NotStarted,
        }
    }

    fn launcher_exit(&self) -> Option<Option<i32>> {
        *lock(&self.shared.launcher_exit)
    }

    fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Err(e) = self.job.terminate(1) {
            self.sink.note(format!("завершение job: {e}"));
        }
        if self.shared.adopted_without_job.load(Ordering::SeqCst)
            && let Some(game) = lock(&self.shared.game).as_ref()
            && let Err(e) = game.kill(1)
        {
            self.sink.note(format!("завершение игры по handle: {e}"));
        }
        if let Some(s) = lock(&self.shared.netcon).take() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for WinSession {
    fn drop(&mut self) {
        self.terminate();
    }
}
