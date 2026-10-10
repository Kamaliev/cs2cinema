//! Машина состояний записи: чистое решение «что делать дальше» без ОС, потоков и системных часов.
//!
//! `Machine::step(now, input)` получает время аргументом и возвращает действия; ввод-вывод и
//! часы — забота драйвера (`record()`), поэтому тест проходит дедлайн в один вызов.
//!
//! Лестница: лаунчер → игра → консоль → демка → скрипт → дубль N/M → закрытие → готово. У каждого
//! ожидания свой дедлайн из `Deadlines` и свой код сбоя. Пропавшая метка — всегда сбой, а не
//! предупреждение; `ALL_DONE` никогда не засчитывается вместо `TAKE_END` (LCH-03).

use std::time::Duration;

use host::{HostError, SessionEvent};
use renderer::TakePlan;

use super::deadlines::Deadlines;
use super::failure::{self, FailCode, LauncherFact, RecordFailure};
use super::markers::{self, EXEC, Marker};
use super::{Event, TakeTiming};

/// Состояние записи — ступень лестницы, которую видит пользователь.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordState {
    Launcher,
    Game,
    Console,
    Demo,
    Script,
    Take,
    Closing,
    Done,
}

impl RecordState {
    /// Ступени лестницы до «готово», по порядку.
    pub const LADDER: [RecordState; 7] = [
        RecordState::Launcher,
        RecordState::Game,
        RecordState::Console,
        RecordState::Demo,
        RecordState::Script,
        RecordState::Take,
        RecordState::Closing,
    ];

    /// Код состояния в базе (`jobs.phase`, `fail_state`).
    pub fn code(self) -> &'static str {
        match self {
            RecordState::Launcher => "launcher",
            RecordState::Game => "game",
            RecordState::Console => "console",
            RecordState::Demo => "demo",
            RecordState::Script => "script",
            RecordState::Take => "take",
            RecordState::Closing => "closing",
            RecordState::Done => "done",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        [RecordState::Done]
            .into_iter()
            .chain(Self::LADDER)
            .find(|st| st.code() == s)
    }

    /// Название ступени лестницы.
    pub fn title(self) -> &'static str {
        match self {
            RecordState::Launcher => "лаунчер",
            RecordState::Game => "игра",
            RecordState::Console => "консоль",
            RecordState::Demo => "демка",
            RecordState::Script => "скрипт",
            RecordState::Take => "дубль",
            RecordState::Closing => "закрытие",
            RecordState::Done => "готово",
        }
    }

    /// Строка журнала задачи при входе в состояние.
    pub fn journal(self) -> &'static str {
        match self {
            RecordState::Launcher => "запуск лаунчера (HLAE)",
            RecordState::Game => "ожидание игры (cs2.exe)",
            RecordState::Console => "игра запущена, жду консоль",
            RecordState::Demo => "консоль открыта, жду загрузку демки",
            RecordState::Script => "демка готова, скрипт запущен",
            RecordState::Take => "дубль",
            RecordState::Closing => "все дубли записаны, закрываю игру",
            RecordState::Done => "готово",
        }
    }
}

/// Что пришло в машину.
#[derive(Debug, Clone)]
pub enum Input {
    /// Ничего не произошло, прошло время (драйвер шлёт примерно каждые 100 мс).
    Tick,
    /// Пользователь отменил задачу.
    Cancel,
    Event(SessionEvent),
    /// Команду в консоль отправить не удалось.
    SendFailed {
        line: String,
        error: String,
    },
}

/// Что должен сделать драйвер.
#[derive(Debug, Clone)]
pub enum Action {
    /// Отправить строку в консоль игры.
    Send(String),
    /// Сообщить о событии.
    Emit(Event),
    /// Завершить дерево процессов сессии.
    Terminate,
    /// Запись закончена: успех или сбой.
    Finish(Result<(), RecordFailure>),
}

pub struct Machine {
    nonce: String,
    takes: Vec<TakePlan>,
    d: Deadlines,
    state: RecordState,
    /// Когда вошли в текущее состояние.
    entered: Duration,
    /// От какого момента считается дедлайн текущего ожидания.
    since: Duration,
    launcher: LauncherFact,
    launcher_exit_at: Option<Duration>,
    appeared_at: Option<Duration>,
    console_up: bool,
    /// Индекс текущего дубля (в `takes`).
    take: usize,
    /// В состоянии `Take`: true — дубль пишется (ждём TAKE_END), false — ждём TAKE_START.
    recording: bool,
    take_started: Duration,
    /// Принятые метки: их повтор из второго канала молча игнорируется.
    accepted: Vec<Marker>,
    timings: Vec<TakeTiming>,
    game_exited_at: Option<Duration>,
    finished: bool,
}

impl Machine {
    pub fn new(nonce: String, takes: Vec<TakePlan>, deadlines: Deadlines) -> Self {
        Machine {
            nonce,
            takes,
            d: deadlines,
            state: RecordState::Launcher,
            entered: Duration::ZERO,
            since: Duration::ZERO,
            launcher: LauncherFact::NotLaunched,
            launcher_exit_at: None,
            appeared_at: None,
            console_up: false,
            take: 0,
            recording: false,
            take_started: Duration::ZERO,
            accepted: vec![],
            timings: vec![],
            game_exited_at: None,
            finished: false,
        }
    }

    pub fn state(&self) -> RecordState {
        self.state
    }

    pub fn launcher(&self) -> LauncherFact {
        self.launcher
    }

    /// Сколько заняла запись каждого принятого дубля.
    pub fn timings(&self) -> &[TakeTiming] {
        &self.timings
    }

    /// Вход в лестницу: лаунчер запускается.
    pub fn start(&mut self, now: Duration) -> Vec<Action> {
        self.launcher = LauncherFact::Running;
        let mut acts = vec![];
        self.enter(RecordState::Launcher, now, now, &mut acts);
        acts
    }

    /// Хост не смог запустить лаунчер.
    pub fn launch_failed(&mut self, now: Duration, err: &HostError) -> RecordFailure {
        self.launcher = LauncherFact::NotLaunched;
        self.finished = true;
        RecordFailure::new(
            FailCode::LauncherFailed,
            RecordState::Launcher,
            failure::msg_launch_error(err),
            now.saturating_sub(self.since),
        )
    }

    /// Один шаг: ввод и текущее время (с момента старта хоста) -> действия для драйвера.
    pub fn step(&mut self, now: Duration, input: Input) -> Vec<Action> {
        let mut acts = vec![];
        if self.finished {
            return acts;
        }
        match input {
            Input::Cancel => {
                acts.push(Action::Terminate);
                let f = self.fail(
                    now,
                    FailCode::Cancelled,
                    self.entered,
                    failure::msg_cancelled(),
                );
                acts.push(f);
                return acts;
            }
            Input::Tick => {}
            Input::Event(ev) => self.on_event(now, ev, &mut acts),
            Input::SendFailed { line, error } => self.on_send_failed(now, &line, &error, &mut acts),
        }
        if !self.finished {
            self.advance(now, &mut acts);
        }
        if !self.finished {
            self.check_deadline(now, &mut acts);
        }
        acts
    }

    fn take_label(&self) -> Option<(u32, u32)> {
        Some((self.take as u32 + 1, self.takes.len() as u32))
    }

    fn enter(&mut self, st: RecordState, now: Duration, since: Duration, acts: &mut Vec<Action>) {
        self.state = st;
        self.entered = now;
        self.since = since;
        let take = if st == RecordState::Take {
            self.take_label()
        } else {
            None
        };
        acts.push(Action::Emit(Event::State { state: st, take }));
    }

    /// Сбой в текущем состоянии; `base` — от какого момента считать прошедшее время.
    fn fail(&mut self, now: Duration, code: FailCode, base: Duration, message: String) -> Action {
        self.finished = true;
        let mut f = RecordFailure::new(code, self.state, message, now.saturating_sub(base));
        f.launcher = self.launcher;
        Action::Finish(Err(f))
    }

    fn finish_ok(&mut self, now: Duration, acts: &mut Vec<Action>) {
        self.enter(RecordState::Done, now, now, acts);
        self.finished = true;
        acts.push(Action::Finish(Ok(())));
    }

    fn on_event(&mut self, now: Duration, ev: SessionEvent, acts: &mut Vec<Action>) {
        match ev {
            SessionEvent::LauncherExited { code } => {
                self.launcher = LauncherFact::Exited(code);
                self.launcher_exit_at = Some(now);
            }
            SessionEvent::GameAppeared { .. } => {
                self.appeared_at.get_or_insert(now);
            }
            SessionEvent::ConsoleUp => self.console_up = true,
            SessionEvent::ConsoleDown => self.console_up = false,
            SessionEvent::Line { text, .. } => {
                if let Some(mk) = markers::parse_marker(&text, &self.nonce) {
                    self.on_marker(now, mk, acts);
                }
            }
            SessionEvent::GameExited { .. } => {
                if self.state == RecordState::Closing {
                    self.game_exited_at.get_or_insert(now);
                } else {
                    self.game_gone(now, acts);
                }
            }
            SessionEvent::JobEmpty => {
                if self.state == RecordState::Closing {
                    self.finish_ok(now, acts);
                } else if self.appeared_at.is_some() {
                    self.game_gone(now, acts);
                }
            }
            SessionEvent::Note(_) => {}
        }
    }

    fn game_gone(&mut self, now: Duration, acts: &mut Vec<Action>) {
        let msg = failure::msg_game_exited(
            self.state,
            self.take_label()
                .filter(|_| self.state == RecordState::Take),
        );
        let f = self.fail(now, FailCode::GameExited, self.entered, msg);
        acts.push(f);
    }

    fn on_send_failed(&mut self, now: Duration, line: &str, _error: &str, acts: &mut Vec<Action>) {
        if line == "quit" {
            // игра не приняла quit — закрываем свой job и считаем запись законченной
            acts.push(Action::Terminate);
            self.finish_ok(now, acts);
        }
    }

    /// Метка, которую ждёт машина прямо сейчас.
    fn expected(&self) -> Option<Marker> {
        match self.state {
            RecordState::Demo => Some(Marker::Ready),
            RecordState::Script => Some(Marker::Script),
            RecordState::Take => {
                let n = self.takes[self.take].index;
                Some(if self.recording {
                    Marker::TakeEnd(n)
                } else {
                    Marker::TakeStart(n)
                })
            }
            _ => None,
        }
    }

    fn on_marker(&mut self, now: Duration, mk: Marker, acts: &mut Vec<Action>) {
        if self.state == RecordState::Closing {
            // конец скрипта после принятого TAKE_END последнего дубля
            if mk == Marker::AllDone && !self.accepted.contains(&mk) {
                self.accepted.push(mk);
            }
            return;
        }
        if self.accepted.contains(&mk) || self.expected() != Some(mk) {
            return;
        }
        self.accepted.push(mk);
        match mk {
            Marker::Ready => {
                self.enter(RecordState::Script, now, now, acts);
                acts.push(Action::Send(EXEC.to_owned()));
            }
            Marker::Script => {
                if self.takes.is_empty() {
                    self.close(now, acts);
                } else {
                    self.begin_take_wait(0, now, acts);
                }
            }
            Marker::TakeStart(_) => {
                self.recording = true;
                self.take_started = now;
                self.since = now;
            }
            Marker::TakeEnd(_) => {
                let t = &self.takes[self.take];
                self.timings.push(TakeTiming {
                    index: t.index,
                    wall: now.saturating_sub(self.take_started),
                    game_secs: t.game_secs,
                });
                let n = self.take as u32 + 1;
                let m = self.takes.len() as u32;
                acts.push(Action::Emit(Event::TakeDone { n, m }));
                if (self.take + 1) < self.takes.len() {
                    self.begin_take_wait(self.take + 1, now, acts);
                } else {
                    self.close(now, acts);
                }
            }
            Marker::AllDone => {}
        }
    }

    fn begin_take_wait(&mut self, idx: usize, now: Duration, acts: &mut Vec<Action>) {
        self.take = idx;
        self.recording = false;
        self.enter(RecordState::Take, now, now, acts);
    }

    fn close(&mut self, now: Duration, acts: &mut Vec<Action>) {
        self.enter(RecordState::Closing, now, now, acts);
        acts.push(Action::Send("quit".to_owned()));
    }

    /// Переходы, которые не ждут метки: лаунчер -> игра -> консоль -> демка.
    fn advance(&mut self, now: Duration, acts: &mut Vec<Action>) {
        loop {
            match self.state {
                RecordState::Launcher => match self.launcher {
                    LauncherFact::Exited(Some(0)) => {
                        let at = self.launcher_exit_at.unwrap_or(now);
                        self.enter(RecordState::Game, now, at, acts);
                    }
                    LauncherFact::Exited(code) => {
                        let f = self.fail(
                            now,
                            FailCode::LauncherFailed,
                            self.since,
                            failure::msg_launcher_exit(code),
                        );
                        acts.push(f);
                        return;
                    }
                    _ => return,
                },
                RecordState::Game => match self.appeared_at {
                    Some(at) => self.enter(RecordState::Console, now, at, acts),
                    None => return,
                },
                RecordState::Console => {
                    if !self.console_up {
                        return;
                    }
                    self.enter(RecordState::Demo, now, now, acts);
                    acts.push(Action::Send(markers::ready_request(&self.nonce)));
                }
                _ => return,
            }
        }
    }

    /// Дедлайн текущего ожидания.
    fn limit(&self) -> Option<(Duration, FailCode)> {
        let d = &self.d;
        Some(match self.state {
            RecordState::Launcher => (d.launcher, FailCode::LauncherFailed),
            RecordState::Game => (d.game_appear, FailCode::GameNotStarted),
            RecordState::Console => (d.console, FailCode::ConsoleNotOpen),
            RecordState::Demo => (d.demo_load, FailCode::DemoNotLoaded),
            RecordState::Script => (d.script, FailCode::ScriptNotRun),
            RecordState::Take if self.recording => {
                (d.take_run(&self.takes[self.take]), FailCode::TakeTimeout)
            }
            RecordState::Take if self.take == 0 => (d.first_take, FailCode::TakeTimeout),
            RecordState::Take => (d.take_seek, FailCode::TakeTimeout),
            RecordState::Closing | RecordState::Done => return None,
        })
    }

    fn check_deadline(&mut self, now: Duration, acts: &mut Vec<Action>) {
        if self.state == RecordState::Closing {
            if self
                .game_exited_at
                .is_some_and(|t| now.saturating_sub(t) >= self.d.close_settle)
            {
                self.finish_ok(now, acts);
            } else if now.saturating_sub(self.since) >= self.d.game_close {
                // игра не закрылась после quit: это не сбой — все дубли уже записаны
                acts.push(Action::Emit(Event::Fact(failure::msg_game_not_closed(
                    self.d.game_close,
                ))));
                acts.push(Action::Terminate);
                self.finish_ok(now, acts);
            }
            return;
        }
        let Some((limit, code)) = self.limit() else {
            return;
        };
        if now.saturating_sub(self.since) < limit {
            return;
        }
        let msg = self.timeout_message(code, limit);
        let f = self.fail(now, code, self.since, msg);
        acts.push(f);
    }

    fn timeout_message(&self, code: FailCode, limit: Duration) -> String {
        match code {
            FailCode::LauncherFailed => failure::msg_launcher_timeout(limit),
            FailCode::DemoNotLoaded => failure::msg_demo_not_loaded(limit, false),
            _ => failure::msg_generic(code, limit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::Source;

    const NONCE: &str = "ab12cd34";

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn takes(n: u32) -> Vec<TakePlan> {
        (0..n)
            .map(|i| TakePlan {
                index: i,
                shot: i as usize,
                game_secs: 5.0,
                capture_fps: 60,
            })
            .collect()
    }

    fn line(text: &str) -> Input {
        Input::Event(SessionEvent::Line {
            source: Source::Netcon,
            text: text.to_owned(),
        })
    }

    fn marker(what: &str) -> Input {
        line(&renderer::marker(NONCE, what))
    }

    fn finish(acts: &[Action]) -> Option<&Result<(), RecordFailure>> {
        acts.iter().find_map(|a| match a {
            Action::Finish(r) => Some(r),
            _ => None,
        })
    }

    fn sent(acts: &[Action]) -> Vec<&str> {
        acts.iter()
            .filter_map(|a| match a {
                Action::Send(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Лаунчер, игра, консоль, демка и скрипт пройдены; машина ждёт `TAKE_START 0`.
    fn up_to_take_wait(m: &mut Machine, d: &Deadlines) -> Duration {
        m.start(ms(0));
        m.step(
            ms(0),
            Input::Event(SessionEvent::LauncherExited { code: Some(0) }),
        );
        m.step(ms(1), Input::Event(SessionEvent::GameAppeared { pid: 7 }));
        let acts = m.step(ms(2), Input::Event(SessionEvent::ConsoleUp));
        assert_eq!(
            sent(&acts),
            vec![markers::ready_request(NONCE)],
            "после ConsoleUp уходит запрос READY"
        );
        let acts = m.step(ms(3), marker("READY"));
        assert_eq!(
            sent(&acts),
            vec![EXEC],
            "после READY уходит exec highlights"
        );
        m.step(ms(4), marker("SCRIPT"));
        assert_eq!(m.state(), RecordState::Take);
        assert!(d.first_take > ms(10));
        ms(4)
    }

    #[test]
    fn console_that_never_opens_fails_with_console_not_open_after_the_deadline() {
        let d = Deadlines::fast();
        let mut m = Machine::new(NONCE.into(), takes(3), d.clone());
        m.start(ms(0));
        m.step(
            ms(0),
            Input::Event(SessionEvent::LauncherExited { code: Some(0) }),
        );
        m.step(ms(10), Input::Event(SessionEvent::GameAppeared { pid: 7 }));
        assert!(
            finish(&m.step(ms(10) + d.console - ms(1), Input::Tick)).is_none(),
            "до дедлайна ещё ждём"
        );
        let acts = m.step(ms(10) + d.console + ms(1), Input::Tick);
        match finish(&acts) {
            Some(Err(f)) => {
                assert_eq!(f.code, FailCode::ConsoleNotOpen);
                assert_eq!(f.state, RecordState::Console);
                assert!(f.elapsed >= d.console, "{:?}", f.elapsed);
            }
            other => panic!("ждали сбой CONSOLE_NOT_OPEN, получили {other:?}"),
        }
    }

    #[test]
    fn all_done_never_completes_a_take() {
        let d = Deadlines::fast();
        let ts = takes(3);
        let mut m = Machine::new(NONCE.into(), ts.clone(), d.clone());
        let mut now = up_to_take_wait(&mut m, &d);
        // дубли 0 и 1 записаны честно, на последнем TAKE_END теряется
        for i in 0..2 {
            now += ms(1);
            m.step(now, marker(&format!("TAKE_START {i}")));
            now += ms(1);
            m.step(now, marker(&format!("TAKE_END {i}")));
        }
        now += ms(1);
        m.step(now, marker("TAKE_START 2"));
        let started = now;

        let acts = m.step(now + ms(1), marker("ALL_DONE"));
        assert!(sent(&acts).is_empty(), "quit не отправляется: {acts:?}");
        assert!(finish(&acts).is_none());
        assert_eq!(m.state(), RecordState::Take, "в Closing не перешли");

        let run = d.take_run(&ts[2]);
        assert!(
            finish(&m.step(started + run - ms(1), Input::Tick)).is_none(),
            "дедлайн дубля ещё не вышел"
        );
        let acts = m.step(started + run, Input::Tick);
        match finish(&acts) {
            Some(Err(f)) => {
                assert_eq!(f.code, FailCode::TakeTimeout);
                assert_eq!(f.state, RecordState::Take);
            }
            other => panic!("ждали сбой TAKE_TIMEOUT, получили {other:?}"),
        }
    }

    #[test]
    fn closing_waits_for_job_empty_and_a_late_game_is_terminated_not_failed() {
        let d = Deadlines::fast();
        let mut m = Machine::new(NONCE.into(), takes(1), d.clone());
        let mut now = up_to_take_wait(&mut m, &d);
        now += ms(1);
        m.step(now, marker("TAKE_START 0"));
        now += ms(1);
        let acts = m.step(now, marker("TAKE_END 0"));
        assert_eq!(sent(&acts), vec!["quit"]);
        assert_eq!(m.state(), RecordState::Closing);
        assert_eq!(m.timings().len(), 1);
        let acts = m.step(now + d.game_close, Input::Tick);
        assert!(
            acts.iter().any(|a| matches!(a, Action::Terminate)),
            "свой job завершён"
        );
        assert!(
            matches!(finish(&acts), Some(Ok(()))),
            "закрытие по таймеру — не сбой"
        );
        assert_eq!(m.state(), RecordState::Done);
    }
}
