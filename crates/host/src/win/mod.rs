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
pub mod system;
pub mod watch;
pub mod windows;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::gate::{DemoGate, Verdict};
use crate::{
    CmdOutput, CmdSpec, Host, HostError, LaunchSpec, Liveness, PrevGame, ProcInfo, Session,
    SessionEvent, Source, WindowInfo, console_line_ok,
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
///
/// Строки консоли проходят ворота [`DemoGate`]: метка нашей задачи, пришедшая раньше подтверждения
/// демки (ложный `READY` на меню-карте, спайк S3c), в канал не попадает — вместо неё идёт `Note`.
pub(crate) struct Sink {
    tx: Sender<SessionEvent>,
    gate: Mutex<DemoGate>,
    dropped: AtomicU32,
}

/// Сколько отброшенных меток описывается в журнале; остальные считаются молча.
const MAX_DROP_NOTES: u32 = 10;

impl Sink {
    pub fn new(tx: Sender<SessionEvent>, nonce: &str) -> Self {
        Sink {
            tx,
            gate: Mutex::new(DemoGate::new(nonce)),
            dropped: AtomicU32::new(0),
        }
    }

    pub fn send(&self, ev: SessionEvent) {
        // получатель закрыт — сессия уже не нужна, потерять событие не страшно
        let _ = self.tx.send(ev);
    }

    pub fn note(&self, text: impl Into<String>) {
        self.send(SessionEvent::Note(text.into()));
    }

    pub fn line(&self, source: Source, text: String) {
        // замок держим до отправки: подтверждение демки уходит в канал раньше следующих меток
        let mut gate = lock(&self.gate);
        match gate.admit(&text) {
            Verdict::Pass => self.send(SessionEvent::Line { source, text }),
            Verdict::Drop => {
                let n = self.dropped.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= MAX_DROP_NOTES {
                    self.note(format!(
                        "метка до подтверждения демки отброшена [{}]: {text}",
                        source.label()
                    ));
                }
            }
        }
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
    /// `port` — порт netcon на `127.0.0.1`; `console_log` — файл `-condebug`, его хвост читается
    /// от длины на момент запуска. Принятие игры по PID и ворота меток здесь выключены (путь образа
    /// и код задачи неизвестны): этот вход нужен тестам со стенд-процессами, настоящий запуск —
    /// [`Host::launch`].
    pub fn launch_with(
        &self,
        cmd: Command,
        game_image: &str,
        port: u16,
        console_log: Option<PathBuf>,
    ) -> Result<Box<dyn Session>, HostError> {
        self.launch_session(
            cmd,
            SessionParams {
                game_image: game_image.to_owned(),
                expected_exe: None,
                port,
                console_log,
                nonce: String::new(),
            },
        )
    }

    pub fn launch_session(
        &self,
        mut cmd: Command,
        params: SessionParams,
    ) -> Result<Box<dyn Session>, HostError> {
        // смещение хвоста — до запуска: всё, что игра писала раньше, к этой сессии не относится
        let offset = params
            .console_log
            .as_deref()
            .map(console_log::current_offset)
            .unwrap_or(0);
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

        // время создания лаунчера — нижняя граница для принятия игры по PID
        let launcher_created = OwnedProc::open(child.id())
            .ok()
            .and_then(|p| p.creation_time());

        let (tx, rx) = mpsc::channel::<SessionEvent>();
        let sink = Arc::new(Sink::new(tx, &params.nonce));
        let shared = Arc::new(Shared::default());

        if let Some(out) = child.stdout.take() {
            spawn_pipe_reader(out, Source::HlaeStdout, sink.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_pipe_reader(err, Source::HlaeStderr, sink.clone());
        }
        netcon::spawn(params.port, shared.clone(), sink.clone());
        if let Some(path) = params.console_log {
            console_log::spawn(path, offset, shared.clone(), sink.clone());
        }
        watch::spawn(watch::Watch {
            job: job.clone(),
            child,
            shared: shared.clone(),
            sink: sink.clone(),
            game_image: params.game_image,
            expected_exe: params.expected_exe,
            launcher_created,
        });

        Ok(Box::new(WinSession {
            rx,
            shared,
            job,
            sink,
            terminated: false,
            terminated_at: None,
        }))
    }
}

/// Параметры одной сессии: всё, чем настоящий запуск отличается от стенд-процессов в тестах.
/// Публичны ради тестов с настоящими процессами Windows (`tests/win_session.rs`).
pub struct SessionParams {
    pub game_image: String,
    /// Ожидаемый путь `cs2.exe`; без него принятие по PID выключено.
    pub expected_exe: Option<PathBuf>,
    pub port: u16,
    pub console_log: Option<PathBuf>,
    /// Код задачи для ворот меток; пустой — ворота выключены.
    pub nonce: String,
}

/// Использовать ли `-afxFixNetCon`. Значение `fix_netcon = on` из таблицы «Решения для 01-06» в
/// `.planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md` (S2: 5/5 и 5/5
/// подключений, флаг безвреден).
const FIX_NETCON: bool = true;

/// Образ игры, который ищется среди процессов job.
const GAME_IMAGE: &str = "cs2.exe";

/// Строка параметров игры для `-cmdLine` (порядок и набор флагов — по `01-SPIKES.md`).
fn game_cmdline(spec: &LaunchSpec, port: u16) -> String {
    let (w, h) = spec.size;
    let fix = if FIX_NETCON { " -afxFixNetCon" } else { "" };
    // `-insecure` обязателен для хука HLAE; приложение только проигрывает демки и не подключается к серверам
    format!(
        "-insecure -steam -novid -allow_third_party_software -condebug -netconport {port}{fix} \
         -windowed -w {w} -h {h} +demo_ui_mode 0 +playdemo {}",
        spec.demo_name
    )
}

/// Команда запуска HLAE: единственное место, где собирается строка запуска.
fn hlae_command(spec: &LaunchSpec, hlae: &Path, hook: &Path, cs2_exe: &Path, port: u16) -> Command {
    let mut cmd = Command::new(hlae);
    cmd.current_dir(&spec.hlae_dir)
        .args(["-customLoader", "-noGui", "-autoStart", "-noConfig"])
        .arg("-hookDllPath")
        .arg(hook)
        .arg("-programPath")
        .arg(cs2_exe)
        .arg("-cmdLine")
        .arg(game_cmdline(spec, port));
    cmd
}

/// Имя демки идёт в `+playdemo` без кавычек. Спайк S4: кириллица в пути через `-cmdLine`
/// искажается (игра «не находит файл»), ASCII работает. Копирование демки в `game/csgo` остаётся
/// (TMO-06), поэтому достаточно простого ASCII-имени; иначе — понятная ошибка до запуска игры.
fn check_demo_name(name: &str) -> Result<(), HostError> {
    if name.is_empty() {
        return Err(HostError::Spawn("не задано имя демки для +playdemo".into()));
    }
    if !name.is_ascii() {
        return Err(HostError::Spawn(format!(
            "имя демки «{name}» содержит не-ASCII символы: через HLAE игра получает искажённый путь и не находит файл (нужно имя из латиницы)"
        )));
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | ';'))
    {
        return Err(HostError::Spawn(format!(
            "имя демки «{name}» содержит пробел, кавычку или «;»: +playdemo разберёт его неверно"
        )));
    }
    Ok(())
}

impl Host for WinHost {
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError> {
        let hlae = spec.hlae_dir.join("HLAE.exe");
        let hook = spec.hlae_dir.join("x64").join("AfxHookSource2.dll");
        // пути проверяются до запуска чего-либо: нет файла — нет и процесса
        for file in [&hlae, &hook, &spec.cs2_exe] {
            if !file.is_file() {
                return Err(HostError::Spawn(format!("нет {}", file.display())));
            }
        }
        check_demo_name(&spec.demo_name)?;
        let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let (hook, cs2_exe) = (absolute(&hook), absolute(&spec.cs2_exe));
        let port = netcon::free_port();
        let cmd = hlae_command(spec, &hlae, &hook, &cs2_exe, port);
        self.launch_session(
            cmd,
            SessionParams {
                game_image: GAME_IMAGE.to_owned(),
                expected_exe: Some(cs2_exe),
                port,
                console_log: spec.console_log.clone(),
                nonce: spec.nonce.clone(),
            },
        )
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn run(
        &self,
        cmd: &CmdSpec,
        cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError> {
        crate::runner::run_bounded(cmd, cancel, on_line)
    }

    fn processes(&self) -> Result<Vec<ProcInfo>, HostError> {
        Ok(process::snapshot_processes()?
            .into_iter()
            .map(|e| ProcInfo {
                pid: e.pid,
                exe_name: e.exe_name,
            })
            .collect())
    }

    fn free_space(&self, path: &Path) -> Result<u64, HostError> {
        system::free_space(path)
    }

    fn file_version(&self, path: &Path) -> Option<String> {
        system::file_version(path)
    }

    fn wait_previous_game(
        &self,
        limit: Duration,
        settle: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> PrevGame {
        let start = Instant::now();
        loop {
            if cancel() {
                return PrevGame::Cancelled;
            }
            {
                let mut reg = lock(&OWN_GAMES);
                let mut any_exited = false;
                reg.leftovers.retain(|l| {
                    let gone = matches!(l.game.wait(Duration::ZERO), Liveness::Exited(_));
                    any_exited |= gone;
                    !gone
                });
                if any_exited {
                    reg.last_exit = Some(Instant::now());
                }
                if reg.leftovers.is_empty() {
                    if reg.last_exit.is_none_or(|t| t.elapsed() >= settle) {
                        return PrevGame::Clear {
                            waited: start.elapsed(),
                        };
                    }
                } else if let Some(l) = reg.leftovers.iter().find(|l| l.ended.elapsed() >= limit) {
                    // свой handle, свой процесс: одна попытка завершить, потом честный ответ
                    let _ = l.game.kill(1);
                    let liveness = l.game.wait(WAIT_SLICE);
                    if !matches!(liveness, Liveness::Exited(_)) {
                        return PrevGame::Stuck {
                            pid: l.game.pid(),
                            since_end: l.ended.elapsed(),
                            liveness,
                        };
                    }
                }
            }
            std::thread::sleep(WAIT_SLICE);
        }
    }
}

/// Срез ожидания на handle игры: дольше `Shared.game` не держим (его каждые 250 мс берёт наблюдатель).
const WAIT_SLICE: Duration = Duration::from_millis(100);
/// Шаг опроса пустоты job, когда игра уже вышла, а в job ещё есть процессы.
const JOB_POLL: Duration = Duration::from_millis(50);

/// Своя игра, пережившая свою сессию: handle открыт, PID повторно не открывается (T-02-02).
struct Leftover {
    game: OwnedProc,
    /// Когда завершён job игры.
    ended: Instant,
}

/// Реестр своих игр, общий на процесс: задача за задачей создаёт хост заново, а игра прошлой
/// задачи могла не выйти (D-14, TMO-07). Держит только `OwnedProc` своих сессий — чужие
/// процессы сюда не попадают и завершать по нему нечего (T-02-01).
struct OwnGames {
    leftovers: Vec<Leftover>,
    /// Последний подтверждённый выход своей игры.
    last_exit: Option<Instant>,
}

static OWN_GAMES: Mutex<OwnGames> = Mutex::new(OwnGames {
    leftovers: Vec::new(),
    last_exit: None,
});

/// Сессия игры: события из канала, команды в netcon, факты по handle.
pub struct WinSession {
    rx: Receiver<SessionEvent>,
    shared: Arc<Shared>,
    job: Arc<Job>,
    sink: Arc<Sink>,
    terminated: bool,
    /// Когда сессию завершили впервые: от этого момента считается «игра не выходит».
    terminated_at: Option<Instant>,
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

    fn windows(&mut self) -> Vec<WindowInfo> {
        let mut pids = self.job.pids().unwrap_or_default();
        // игра, принятая без job, не числится в списке job, но окна у неё свои
        if self.shared.adopted_without_job.load(Ordering::SeqCst)
            && let Some(game) = lock(&self.shared.game).as_ref()
        {
            pids.push(game.pid());
        }
        windows::list(&pids)
    }

    fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        self.terminated_at = Some(Instant::now());
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

    fn wait_exit(&mut self, limit: Duration) -> Liveness {
        let deadline = Instant::now() + limit;
        let adopted = self.shared.adopted_without_job.load(Ordering::SeqCst);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            // mutex игры держим ровно один срез: между срезами его берёт наблюдатель
            let waited = match lock(&self.shared.game).as_ref() {
                Some(game) => game.wait(left.min(WAIT_SLICE)),
                None => return Liveness::NotStarted,
            };
            match waited {
                Liveness::Exited(code) => {
                    // игра вышла; подтверждение — ещё и пустой job (ffmpeg от HLAE дописывает файлы)
                    if adopted {
                        return Liveness::Exited(code);
                    }
                    match self.job.active_count() {
                        Ok(0) => return Liveness::Exited(code),
                        Ok(_) if left.is_zero() => return Liveness::Alive,
                        Ok(_) => std::thread::sleep(left.min(JOB_POLL)),
                        Err(e) => return Liveness::Unknown(e.to_string()),
                    }
                }
                Liveness::Alive if left.is_zero() => return Liveness::Alive,
                Liveness::Alive => {}
                other => return other,
            }
        }
    }
}

impl Drop for WinSession {
    fn drop(&mut self) {
        self.terminate();
        // игра переживает сессию только с открытым handle: она переезжает в общий реестр
        let Some(game) = lock(&self.shared.game).take() else {
            return;
        };
        let mut reg = lock(&OWN_GAMES);
        match game.wait(Duration::ZERO) {
            Liveness::Exited(_) => reg.last_exit = Some(Instant::now()),
            _ => reg.leftovers.push(Leftover {
                game,
                ended: self.terminated_at.unwrap_or_else(Instant::now),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            hlae_dir: r"C:\Program Files (x86)\HLAE".into(),
            cs2_exe: r"C:\Steam\game\bin\win64\cs2.exe".into(),
            demo_name: "cs2cinema_1-abc.dem".into(),
            size: (1280, 720),
            nonce: "ab12cd34".into(),
            console_log: None,
        }
    }

    #[test]
    fn game_line_follows_the_spike_decisions() {
        let line = game_cmdline(&spec(), 55433);
        for must in [
            "-insecure",
            "-condebug",
            "-netconport 55433",
            "-afxFixNetCon",
            "-windowed -w 1280 -h 720",
            "+demo_ui_mode 0",
            "+playdemo cs2cinema_1-abc.dem",
        ] {
            assert!(line.contains(must), "нет {must:?} в {line:?}");
        }
        assert!(line.ends_with("+playdemo cs2cinema_1-abc.dem"), "{line}");
    }

    #[test]
    fn hlae_command_has_every_launcher_flag_in_order() {
        let s = spec();
        let cmd = hlae_command(
            &s,
            &s.hlae_dir.join("HLAE.exe"),
            &s.hlae_dir.join("x64").join("AfxHookSource2.dll"),
            &s.cs2_exe,
            2121,
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[..4],
            ["-customLoader", "-noGui", "-autoStart", "-noConfig"]
        );
        assert_eq!(args[4], "-hookDllPath");
        assert!(args[5].ends_with("AfxHookSource2.dll"), "{args:?}");
        assert_eq!(args[6], "-programPath");
        assert!(args[7].ends_with("cs2.exe"), "{args:?}");
        assert_eq!(args[8], "-cmdLine");
        assert!(args[9].contains("-netconport 2121"), "{args:?}");
        assert_eq!(args.len(), 10);
        assert_eq!(cmd.get_current_dir(), Some(s.hlae_dir.as_path()));
    }

    #[test]
    fn demo_name_must_be_plain_ascii() {
        assert!(check_demo_name("cs2cinema_1-abc.dem").is_ok());
        assert!(check_demo_name("sub/dir.dem").is_ok());
        for bad in [
            "",
            "демо.dem",
            "a b.dem",
            "a\".dem",
            "a;quit.dem",
            "a\nb.dem",
        ] {
            assert!(
                matches!(check_demo_name(bad), Err(HostError::Spawn(_))),
                "{bad:?} должно отвергаться"
            );
        }
        let Err(HostError::Spawn(msg)) = check_demo_name("демо.dem") else {
            panic!("ожидалась ошибка")
        };
        assert!(msg.contains("не-ASCII"), "{msg}");
    }

    #[test]
    fn launch_refuses_missing_files_before_starting_anything() {
        let Err(e) = WinHost::new().launch(&spec()) else {
            panic!("несуществующие пути не должны запускаться")
        };
        let HostError::Spawn(msg) = e else {
            panic!("ожидалась Spawn, получено {e:?}")
        };
        assert!(msg.starts_with("нет "), "{msg}");
    }
}
