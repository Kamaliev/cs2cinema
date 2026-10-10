//! Фейковый хост для тестов машины записи: сценарий вместо настоящей игры и виртуальное время.
//!
//! Модуль обычный (не `cfg(test)`): им пользуются тесты `pipeline`, `app` и `cli`.
//! Часы виртуальные и двигаются только в [`Session::poll`]: если до `max_wait` событий нет, они
//! уходят вперёд ровно на `max_wait`. Поэтому ожидание в 150 с в тесте занимает микросекунды, а
//! сценарий без нужной метки упирается в дедлайн машины, а не висит. Страховка — если часы
//! перешли 10 виртуальных часов, фейк паникует.
//!
//! Фейк отвечает на команды как игра: запрос `READY` даёт строку `CS2C <nonce> READY`,
//! `exec highlights` — `SCRIPT`, затем `TAKE_START i`/`TAKE_END i` на каждый дубль и `ALL_DONE`,
//! `quit` — выход игры и пустой job. Любую метку сценарий может убрать или задержать (задержка
//! сдвигает и все последующие метки той же цепочки — как медленная игра).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::{
    CmdOutput, CmdSpec, Host, HostError, LaunchSpec, Liveness, Session, SessionEvent, Source,
    console_line_ok,
};

/// PID «игры» в событии `GameAppeared`.
const FAKE_PID: u32 = 4242;
/// Потолок виртуального времени: дальше — значит сценарий зациклился.
const MAX_VIRTUAL: Duration = Duration::from_secs(10 * 3600);
/// Через сколько приходит дубль метки из `console.log` (см. `duplicate_via_console_log`).
const CONSOLE_LOG_LAG: Duration = Duration::from_millis(200);

/// Метка, которую игра печатает в консоль.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeMarker {
    Ready,
    Script,
    TakeStart(u32),
    TakeEnd(u32),
    AllDone,
}

/// Сценарий поведения «игры». Все времена отсчитываются от запуска (кроме реакций на команды —
/// те от момента отправки команды).
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Выход лаунчера: через сколько и с каким кодом.
    pub launcher: Option<(Duration, Option<i32>)>,
    /// Когда появляется `cs2.exe`.
    pub game_appear: Option<Duration>,
    /// Когда открывается консольный порт.
    pub console: Option<Duration>,
    /// Через сколько после запроса приходит `READY`.
    pub ready_delay: Duration,
    /// Через сколько после `exec highlights` приходит `SCRIPT`.
    pub script_delay: Duration,
    /// Пауза перед `TAKE_START i` после предыдущей метки.
    pub take_gap: Duration,
    /// Длительность каждого дубля (`TAKE_START i` -> `TAKE_END i`); длина = число дублей.
    pub take_durations: Vec<Duration>,
    /// Метки, которые не приходят.
    pub dropped: Vec<FakeMarker>,
    /// Метки, которые приходят позже (и сдвигают следующие в цепочке).
    pub delays: Vec<(FakeMarker, Duration)>,
    /// Игра умирает в этот момент с этим кодом.
    pub game_dies: Option<(Duration, Option<u32>)>,
    /// Принудительный ответ `game_liveness`.
    pub liveness: Option<Liveness>,
    /// Произвольные строки консоли в заданные моменты.
    pub lines: Vec<(Duration, Source, String)>,
    /// Эхо отправленных команд строкой из netcon.
    pub echo: bool,
    /// Через сколько после `quit` игра выходит; `None` — `quit` игнорируется.
    pub quit_after: Option<Duration>,
    /// Запуск заканчивается этой ошибкой.
    pub launch_error: Option<HostError>,
    /// Куда писать `take{i:04}/video.mp4` и `audio.wav`.
    pub clips: Option<PathBuf>,
    /// Ответ `Host::run`.
    pub run: Result<CmdOutput, HostError>,
    /// Запрос `READY` с `addAtTick` остаётся без ответа, отвечает только прямой `echo` (поздний netcon).
    pub ready_only_direct: bool,
    /// Каждая метка приходит второй раз из `console.log`.
    pub duplicate_log: bool,
}

impl Scenario {
    /// Всё идёт хорошо: `takes` дублей по 5 с.
    pub fn happy(takes: u32) -> Self {
        Scenario {
            launcher: Some((Duration::from_secs(2), Some(0))),
            game_appear: Some(Duration::from_secs(1)),
            console: Some(Duration::from_secs(5)),
            ready_delay: Duration::from_secs(3),
            script_delay: Duration::from_secs(1),
            take_gap: Duration::from_secs(2),
            take_durations: vec![Duration::from_secs(5); takes as usize],
            dropped: vec![],
            delays: vec![],
            game_dies: None,
            liveness: None,
            lines: vec![],
            echo: true,
            quit_after: Some(Duration::from_secs(2)),
            launch_error: None,
            clips: None,
            run: Ok(CmdOutput {
                code: Some(0),
                stderr_tail: vec![],
                elapsed: Duration::ZERO,
            }),
            ready_only_direct: false,
            duplicate_log: false,
        }
    }

    pub fn launcher_exit(mut self, code: Option<i32>, after: Duration) -> Self {
        self.launcher = Some((after, code));
        self
    }

    pub fn launcher_never_exits(mut self) -> Self {
        self.launcher = None;
        self
    }

    pub fn game_appears(mut self, after: Duration) -> Self {
        self.game_appear = Some(after);
        self
    }

    pub fn game_never_appears(mut self) -> Self {
        self.game_appear = None;
        self
    }

    pub fn console_up(mut self, after: Duration) -> Self {
        self.console = Some(after);
        self
    }

    pub fn console_never_up(mut self) -> Self {
        self.console = None;
        self
    }

    pub fn ready_after(mut self, after: Duration) -> Self {
        self.ready_delay = after;
        self
    }

    pub fn drop_marker(mut self, m: FakeMarker) -> Self {
        self.dropped.push(m);
        self
    }

    pub fn delay_marker(mut self, m: FakeMarker, by: Duration) -> Self {
        self.delays.push((m, by));
        self
    }

    /// Длительности дублей; число дублей становится равным длине списка.
    pub fn take_secs(mut self, secs: Vec<Duration>) -> Self {
        self.take_durations = secs;
        self
    }

    pub fn game_dies_at(mut self, at: Duration, code: Option<u32>) -> Self {
        self.game_dies = Some((at, code));
        self
    }

    pub fn liveness_override(mut self, l: Liveness) -> Self {
        self.liveness = Some(l);
        self
    }

    pub fn line_at(mut self, at: Duration, source: Source, text: &str) -> Self {
        self.lines.push((at, source, text.to_owned()));
        self
    }

    pub fn echo_commands(mut self, on: bool) -> Self {
        self.echo = on;
        self
    }

    pub fn ignore_quit(mut self) -> Self {
        self.quit_after = None;
        self
    }

    pub fn launch_error(mut self, e: HostError) -> Self {
        self.launch_error = Some(e);
        self
    }

    pub fn clips_dir(mut self, dir: PathBuf) -> Self {
        self.clips = Some(dir);
        self
    }

    pub fn run_result(mut self, r: Result<CmdOutput, HostError>) -> Self {
        self.run = r;
        self
    }

    pub fn ready_only_direct(mut self) -> Self {
        self.ready_only_direct = true;
        self
    }

    pub fn duplicate_via_console_log(mut self) -> Self {
        self.duplicate_log = true;
        self
    }

    fn delay_of(&self, m: FakeMarker) -> Duration {
        self.delays
            .iter()
            .filter(|(x, _)| *x == m)
            .map(|(_, d)| *d)
            .sum()
    }
}

/// Общее состояние хоста: сценарий, виртуальные часы и счётчики для проверок.
struct HostState {
    scenario: Scenario,
    now: Duration,
    sent: Vec<String>,
    terminated: u32,
    launches: u32,
}

fn lock(h: &Arc<Mutex<HostState>>) -> MutexGuard<'_, HostState> {
    // паника «время перевалило» не должна делать последующие проверки непонятными
    h.lock().unwrap_or_else(|e| e.into_inner())
}

/// Фейковый хост. Клонируется: копия, оставленная в тесте, видит то, что делает сессия.
#[derive(Clone)]
pub struct FakeHost {
    state: Arc<Mutex<HostState>>,
}

impl FakeHost {
    pub fn new(scenario: Scenario) -> Self {
        FakeHost {
            state: Arc::new(Mutex::new(HostState {
                scenario,
                now: Duration::ZERO,
                sent: vec![],
                terminated: 0,
                launches: 0,
            })),
        }
    }

    /// Команды, принятые консолью (отклонённые и отправленные до открытия порта не считаются).
    pub fn sent(&self) -> Vec<String> {
        lock(&self.state).sent.clone()
    }

    /// Сколько раз сессию завершали через `terminate` (по одному разу на сессию).
    pub fn terminated(&self) -> u32 {
        lock(&self.state).terminated
    }

    /// Сколько раз игру запускали.
    pub fn launches(&self) -> u32 {
        lock(&self.state).launches
    }
}

impl Host for FakeHost {
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError> {
        let mut st = lock(&self.state);
        if let Some(e) = st.scenario.launch_error.clone() {
            return Err(e);
        }
        st.launches += 1;
        let t0 = st.now;
        let mut s = FakeSession {
            host: Arc::clone(&self.state),
            nonce: spec.nonce.clone(),
            queue: vec![],
            seq: 0,
            appeared: false,
            exited: None,
            console_up: false,
            launcher_exit: None,
            terminated: false,
        };
        let sc = &st.scenario;
        if let Some((after, code)) = sc.launcher {
            s.push(
                t0 + after,
                Some(SessionEvent::LauncherExited { code }),
                None,
            );
        }
        if let Some(after) = sc.game_appear {
            s.push(
                t0 + after,
                Some(SessionEvent::GameAppeared { pid: FAKE_PID }),
                None,
            );
        }
        if let Some(after) = sc.console {
            s.push(t0 + after, Some(SessionEvent::ConsoleUp), None);
        }
        if let Some((at, code)) = sc.game_dies {
            s.push(t0 + at, Some(SessionEvent::GameExited { code }), None);
            s.push(t0 + at, Some(SessionEvent::JobEmpty), None);
        }
        for (at, source, text) in &sc.lines {
            s.push(
                t0 + *at,
                Some(SessionEvent::Line {
                    source: *source,
                    text: text.clone(),
                }),
                None,
            );
        }
        Ok(Box::new(s))
    }

    fn now(&self) -> Duration {
        lock(&self.state).now
    }

    fn run(
        &self,
        _cmd: &CmdSpec,
        cancel: &dyn Fn() -> bool,
        _on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError> {
        if cancel() {
            return Err(HostError::Cancelled);
        }
        lock(&self.state).scenario.run.clone()
    }
}

/// Запланированное событие. `ev == None` — метка «убрана»: на линии её нет, а побочный эффект
/// (файлы дубля) остаётся.
struct Item {
    at: Duration,
    seq: u64,
    ev: Option<SessionEvent>,
    /// После этого события на диске появляются файлы дубля с таким номером.
    take_files: Option<u32>,
}

/// Сессия фейковой игры.
pub struct FakeSession {
    host: Arc<Mutex<HostState>>,
    nonce: String,
    queue: Vec<Item>,
    seq: u64,
    appeared: bool,
    exited: Option<Option<u32>>,
    console_up: bool,
    launcher_exit: Option<Option<i32>>,
    terminated: bool,
}

impl FakeSession {
    fn push(&mut self, at: Duration, ev: Option<SessionEvent>, take_files: Option<u32>) {
        self.seq += 1;
        self.queue.push(Item {
            at,
            seq: self.seq,
            ev,
            take_files,
        });
    }

    /// Поставить метку `CS2C <nonce> <text>` на время `at` (с учётом «убрана» и дубля из `console.log`).
    fn marker(
        &mut self,
        sc: &Scenario,
        m: FakeMarker,
        at: Duration,
        text: &str,
        take_files: Option<u32>,
    ) {
        if sc.dropped.contains(&m) {
            self.push(at, None, take_files);
            return;
        }
        let line = format!("CS2C {} {text}", self.nonce);
        self.push(
            at,
            Some(SessionEvent::Line {
                source: Source::Netcon,
                text: line.clone(),
            }),
            take_files,
        );
        if sc.duplicate_log {
            self.push(
                at + CONSOLE_LOG_LAG,
                Some(SessionEvent::Line {
                    source: Source::ConsoleLog,
                    text: line,
                }),
                None,
            );
        }
    }

    fn schedule_ready(&mut self, sc: &Scenario, now: Duration) {
        let at = now + sc.ready_delay + sc.delay_of(FakeMarker::Ready);
        self.marker(sc, FakeMarker::Ready, at, "READY", None);
    }

    fn schedule_script(&mut self, sc: &Scenario, now: Duration) {
        let mut t = now + sc.script_delay + sc.delay_of(FakeMarker::Script);
        self.marker(sc, FakeMarker::Script, t, "SCRIPT", None);
        for (i, dur) in sc.take_durations.iter().enumerate() {
            let i = i as u32;
            t += sc.take_gap + sc.delay_of(FakeMarker::TakeStart(i));
            self.marker(
                sc,
                FakeMarker::TakeStart(i),
                t,
                &format!("TAKE_START {i}"),
                None,
            );
            t += *dur + sc.delay_of(FakeMarker::TakeEnd(i));
            self.marker(
                sc,
                FakeMarker::TakeEnd(i),
                t,
                &format!("TAKE_END {i}"),
                Some(i),
            );
        }
        self.marker(
            sc,
            FakeMarker::AllDone,
            t + sc.delay_of(FakeMarker::AllDone),
            "ALL_DONE",
            None,
        );
    }

    /// Состояние сессии после доставки события вызывающему.
    fn delivered(&mut self, ev: &SessionEvent) {
        match ev {
            SessionEvent::LauncherExited { code } => self.launcher_exit = Some(*code),
            SessionEvent::GameAppeared { .. } => self.appeared = true,
            SessionEvent::ConsoleUp => self.console_up = true,
            SessionEvent::GameExited { code } => {
                self.exited = Some(*code);
                self.console_up = false;
                // мёртвая игра больше ничего не печатает
                self.queue.retain(|i| {
                    matches!(
                        i.ev,
                        Some(
                            SessionEvent::LauncherExited { .. }
                                | SessionEvent::JobEmpty
                                | SessionEvent::Note(_)
                        )
                    )
                });
            }
            _ => {}
        }
    }
}

fn write_take_files(dir: &std::path::Path, i: u32) {
    let d = dir.join(format!("take{i:04}"));
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("video.mp4"), b"fake-video");
    let _ = std::fs::write(d.join("audio.wav"), b"fake-audio");
}

impl Session for FakeSession {
    fn poll(&mut self, max_wait: Duration) -> Option<SessionEvent> {
        let host = Arc::clone(&self.host);
        let mut st = lock(&host);
        let limit = st.now + max_wait;
        loop {
            let next = self
                .queue
                .iter()
                .enumerate()
                .filter(|(_, i)| i.at <= limit)
                .min_by_key(|(_, i)| (i.at, i.seq))
                .map(|(idx, _)| idx);
            let Some(idx) = next else {
                st.now = limit;
                let over = st.now > MAX_VIRTUAL;
                drop(st);
                if over {
                    panic!("фейк: виртуальное время перевалило за 10 ч — сценарий завис");
                }
                return None;
            };
            let item = self.queue.remove(idx);
            st.now = st.now.max(item.at);
            if let (Some(i), Some(dir)) = (item.take_files, st.scenario.clips.as_ref()) {
                write_take_files(dir, i);
            }
            if let Some(ev) = item.ev {
                self.delivered(&ev);
                return Some(ev);
            }
        }
    }

    fn send(&mut self, line: &str) -> Result<(), HostError> {
        if !console_line_ok(line) {
            return Err(HostError::BadCommand);
        }
        if !self.console_up {
            return Err(HostError::NotConnected);
        }
        let host = Arc::clone(&self.host);
        let mut st = lock(&host);
        st.sent.push(line.to_owned());
        let now = st.now;
        let sc = st.scenario.clone();
        if sc.echo {
            self.push(
                now,
                Some(SessionEvent::Line {
                    source: Source::Netcon,
                    text: line.to_owned(),
                }),
                None,
            );
        }
        let ready = format!("CS2C {} READY", self.nonce);
        if line == format!("echo {ready}")
            || (line.contains("addAtTick") && line.contains(&ready) && !sc.ready_only_direct)
        {
            self.schedule_ready(&sc, now);
        } else if line == "exec highlights" {
            self.schedule_script(&sc, now);
        } else if line == "quit"
            && let Some(after) = sc.quit_after
        {
            self.push(
                now + after,
                Some(SessionEvent::GameExited { code: Some(0) }),
                None,
            );
            self.push(now + after, Some(SessionEvent::JobEmpty), None);
        }
        Ok(())
    }

    fn game_liveness(&mut self) -> Liveness {
        if let Some(l) = lock(&self.host).scenario.liveness.clone() {
            return l;
        }
        match self.exited {
            Some(code) => Liveness::Exited(code),
            None if self.appeared => Liveness::Alive,
            None => Liveness::NotStarted,
        }
    }

    fn launcher_exit(&self) -> Option<Option<i32>> {
        self.launcher_exit
    }

    fn terminate(&mut self) {
        if !self.terminated {
            self.terminated = true;
            lock(&self.host).terminated += 1;
        }
        self.queue.clear();
        self.console_up = false;
        if self.exited.is_none() {
            self.exited = Some(Some(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const NONCE: &str = "abcd1234";

    fn spec() -> LaunchSpec {
        LaunchSpec {
            nonce: NONCE.to_owned(),
            demo_name: "x.dem".to_owned(),
            size: (1280, 720),
            ..LaunchSpec::default()
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Следующее событие; ожидание шагами по секунде (в виртуальном времени это бесплатно).
    fn next(s: &mut dyn Session) -> SessionEvent {
        loop {
            if let Some(ev) = s.poll(secs(1)) {
                return ev;
            }
        }
    }

    fn line(text: &str) -> SessionEvent {
        SessionEvent::Line {
            source: Source::Netcon,
            text: text.to_owned(),
        }
    }

    fn marker(text: &str) -> SessionEvent {
        line(&format!("CS2C {NONCE} {text}"))
    }

    fn until_console(s: &mut dyn Session) {
        while next(s) != SessionEvent::ConsoleUp {}
    }

    #[test]
    fn fake_session_plays_launch_console_and_markers_in_virtual_time() {
        let real = Instant::now();
        let fake = FakeHost::new(Scenario::happy(2));
        let host: Box<dyn Host> = Box::new(fake.clone());
        let mut s: Box<dyn Session> = host.launch(&spec()).unwrap();

        assert_eq!(
            next(s.as_mut()),
            SessionEvent::GameAppeared { pid: FAKE_PID }
        );
        assert_eq!(s.game_liveness(), Liveness::Alive, "игра появилась");
        assert_eq!(s.launcher_exit(), None, "выход лаунчера ещё не наблюдался");
        assert_eq!(
            next(s.as_mut()),
            SessionEvent::LauncherExited { code: Some(0) }
        );
        assert_eq!(s.launcher_exit(), Some(Some(0)));
        assert_eq!(next(s.as_mut()), SessionEvent::ConsoleUp);

        let ask = format!("mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C {NONCE} READY\"");
        s.send(&ask).unwrap();
        assert_eq!(
            next(s.as_mut()),
            line(&ask),
            "эхо запроса не метка, но приходит строкой"
        );
        assert_eq!(next(s.as_mut()), marker("READY"));

        s.send("exec highlights").unwrap();
        assert_eq!(next(s.as_mut()), line("exec highlights"));
        for m in [
            "SCRIPT",
            "TAKE_START 0",
            "TAKE_END 0",
            "TAKE_START 1",
            "TAKE_END 1",
            "ALL_DONE",
        ] {
            assert_eq!(next(s.as_mut()), marker(m));
        }

        s.send("quit").unwrap();
        assert_eq!(next(s.as_mut()), line("quit"));
        assert_eq!(next(s.as_mut()), SessionEvent::GameExited { code: Some(0) });
        assert_eq!(next(s.as_mut()), SessionEvent::JobEmpty);
        assert_eq!(s.game_liveness(), Liveness::Exited(Some(0)));

        assert!(
            host.now() > secs(15),
            "виртуальные часы ушли вперёд: {:?}",
            host.now()
        );
        assert!(
            real.elapsed() < secs(1),
            "тест не должен спать по-настоящему: {:?}",
            real.elapsed()
        );
        assert_eq!(
            fake.sent(),
            vec![ask, "exec highlights".to_owned(), "quit".to_owned()]
        );
        assert_eq!(fake.launches(), 1);
    }

    #[test]
    fn idle_poll_advances_virtual_time_by_max_wait() {
        let real = Instant::now();
        let host = FakeHost::new(
            Scenario::happy(1)
                .game_never_appears()
                .console_never_up()
                .launcher_never_exits(),
        );
        let mut s = host.launch(&spec()).unwrap();
        assert_eq!(host.now(), Duration::ZERO);
        assert_eq!(s.poll(secs(150)), None, "событий нет — ожидание истекает");
        assert_eq!(host.now(), secs(150));
        assert_eq!(s.poll(Duration::from_millis(100)), None);
        assert_eq!(host.now(), secs(150) + Duration::from_millis(100));
        assert_eq!(s.game_liveness(), Liveness::NotStarted);
        assert!(real.elapsed() < secs(1));
    }

    #[test]
    #[should_panic(expected = "виртуальное время перевалило за 10 ч")]
    fn runaway_scenario_panics_instead_of_hanging() {
        let host = FakeHost::new(
            Scenario::happy(1)
                .game_never_appears()
                .console_never_up()
                .launcher_never_exits(),
        );
        let mut s = host.launch(&spec()).unwrap();
        loop {
            s.poll(secs(3600));
        }
    }

    #[test]
    fn dropped_marker_never_arrives_and_send_rejects_newlines() {
        let fake = FakeHost::new(
            Scenario::happy(1)
                .drop_marker(FakeMarker::Ready)
                .echo_commands(false),
        );
        let mut s = fake.launch(&spec()).unwrap();
        assert_eq!(
            s.send("echo до открытия порта"),
            Err(HostError::NotConnected)
        );
        until_console(s.as_mut());

        assert_eq!(s.send("quit\n"), Err(HostError::BadCommand));
        assert_eq!(s.send("a\rb"), Err(HostError::BadCommand));
        assert_eq!(s.send("a\0b"), Err(HostError::BadCommand));
        assert!(
            fake.sent().is_empty(),
            "отклонённые команды никуда не уходят: {:?}",
            fake.sent()
        );

        s.send(&format!(
            "mirv_cmd addAtTick 64 \"echo CS2C {NONCE} READY\""
        ))
        .unwrap();
        for _ in 0..120 {
            assert_eq!(s.poll(secs(1)), None, "READY убран — его не будет");
        }
    }

    #[test]
    fn terminate_is_counted_and_idempotent() {
        let fake = FakeHost::new(Scenario::happy(1));
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.terminate();
        s.terminate();
        assert_eq!(
            fake.terminated(),
            1,
            "завершение считается один раз на сессию"
        );
        assert_eq!(s.game_liveness(), Liveness::Exited(Some(1)));
        assert_eq!(s.send("quit"), Err(HostError::NotConnected));
        assert_eq!(s.poll(secs(60)), None, "очередь событий очищена");
    }

    #[test]
    fn fake_writes_take_files_into_the_temp_dir() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = FakeHost::new(Scenario::happy(2).clips_dir(dir.clone()));
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.send("exec highlights").unwrap();
        while next(s.as_mut()) != marker("ALL_DONE") {}
        for i in 0..2 {
            let d = dir.join(format!("take{i:04}"));
            assert!(d.join("video.mp4").is_file(), "нет video.mp4 дубля {i}");
            assert!(d.join("audio.wav").is_file(), "нет audio.wav дубля {i}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn late_netcon_loses_the_timed_ready_but_answers_the_direct_echo() {
        let fake = FakeHost::new(Scenario::happy(1).ready_only_direct().echo_commands(false));
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.send(&format!(
            "mirv_cmd addAtTick 64 \"echo CS2C {NONCE} READY\""
        ))
        .unwrap();
        assert_eq!(
            s.poll(secs(60)),
            None,
            "запрос с addAtTick остался без ответа"
        );
        s.send(&format!("echo CS2C {NONCE} READY")).unwrap();
        assert_eq!(next(s.as_mut()), marker("READY"));
    }

    #[test]
    fn delayed_marker_shifts_the_following_ones() {
        let fake = FakeHost::new(
            Scenario::happy(1)
                .echo_commands(false)
                .delay_marker(FakeMarker::TakeStart(0), secs(40)),
        );
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.send("exec highlights").unwrap();
        let t0 = fake.now();
        assert_eq!(next(s.as_mut()), marker("SCRIPT"));
        assert_eq!(next(s.as_mut()), marker("TAKE_START 0"));
        assert_eq!(
            fake.now() - t0,
            secs(1 + 2 + 40),
            "TAKE_START 0 ждали на 40 с дольше"
        );
        assert_eq!(next(s.as_mut()), marker("TAKE_END 0"));
        assert_eq!(
            fake.now() - t0,
            secs(1 + 2 + 40 + 5),
            "TAKE_END сдвинулся вместе с ним"
        );
    }

    #[test]
    fn game_death_silences_pending_markers_and_reports_the_exit() {
        let fake = FakeHost::new(
            Scenario::happy(2)
                .echo_commands(false)
                .game_dies_at(secs(10), Some(0xC000_0005)),
        );
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.send("exec highlights").unwrap();
        assert_eq!(next(s.as_mut()), marker("SCRIPT"));
        assert_eq!(next(s.as_mut()), marker("TAKE_START 0"));
        assert_eq!(
            next(s.as_mut()),
            SessionEvent::GameExited {
                code: Some(0xC000_0005)
            }
        );
        assert_eq!(next(s.as_mut()), SessionEvent::JobEmpty);
        assert_eq!(s.game_liveness(), Liveness::Exited(Some(0xC000_0005)));
        assert_eq!(s.poll(secs(600)), None, "мёртвая игра молчит");
    }

    #[test]
    fn duplicate_markers_come_from_console_log_too() {
        let fake = FakeHost::new(
            Scenario::happy(1)
                .echo_commands(false)
                .duplicate_via_console_log(),
        );
        let mut s = fake.launch(&spec()).unwrap();
        until_console(s.as_mut());
        s.send(&format!("echo CS2C {NONCE} READY")).unwrap();
        assert_eq!(next(s.as_mut()), marker("READY"));
        assert_eq!(
            next(s.as_mut()),
            SessionEvent::Line {
                source: Source::ConsoleLog,
                text: format!("CS2C {NONCE} READY")
            }
        );
    }

    #[test]
    fn launch_error_run_result_and_liveness_override_come_from_the_scenario() {
        let failing =
            FakeHost::new(Scenario::happy(1).launch_error(HostError::Spawn("нет HLAE".into())));
        assert!(matches!(failing.launch(&spec()), Err(HostError::Spawn(_))));
        assert_eq!(failing.launches(), 0);

        let bad = CmdOutput {
            code: Some(1),
            stderr_tail: vec!["boom".into()],
            elapsed: secs(3),
        };
        let host = FakeHost::new(
            Scenario::happy(1)
                .run_result(Ok(bad.clone()))
                .liveness_override(Liveness::Unknown("доступ запрещён".into())),
        );
        let cmd = CmdSpec {
            program: "ffprobe".into(),
            args: vec![],
            cwd: None,
            deadline: secs(10),
        };
        assert_eq!(host.run(&cmd, &|| false, &mut |_| {}), Ok(bad));
        assert_eq!(
            host.run(&cmd, &|| true, &mut |_| {}),
            Err(HostError::Cancelled)
        );
        let mut s = host.launch(&spec()).unwrap();
        assert_eq!(
            s.game_liveness(),
            Liveness::Unknown("доступ запрещён".into())
        );
    }
}
