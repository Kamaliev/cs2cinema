//! Запись дублей в CS2 через HLAE: явная машина состояний поверх трейта `host::Host`.
//!
//! Устройство: чистая [`machine::Machine`] решает, что делать и сколько ждать; этот модуль — тонкий
//! драйвер: берёт событие сессии (не дольше 100 мс), проверяет отмену, спрашивает у хоста время,
//! отдаёт всё машине и выполняет её действия. Других ожиданий, кроме `Session::poll`, у драйвера
//! нет, поэтому на фейковом хосте с виртуальными часами тест с дедлайном в 150 с идёт микросекунды.

pub mod deadlines;
pub mod failure;
pub mod log;
pub mod machine;
pub mod markers;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use host::{Host, LaunchSpec, Liveness, Session, SessionEvent, Source};
use renderer::TakePlan;

use crate::Progress;
pub use deadlines::Deadlines;
pub use failure::{FailCode, LauncherFact, RecordFailure, facts_line};
pub use machine::{Action, Input, Machine, RecordState};

/// Верхняя граница одного ожидания события: так часто проверяются дедлайны и отмена.
const TICK: Duration = Duration::from_millis(100);

/// Что записывать и по каким правилам.
#[derive(Debug, Clone)]
pub struct RecordRequest {
    /// Папка результата задачи (там лежат `clips/` и скрипты; в ней же будет `record.log`).
    pub out_dir: PathBuf,
    /// Как запускать игру; `launch.nonce` — код задачи для меток консоли.
    pub launch: LaunchSpec,
    /// Ожидаемые дубли, выведенные из плана до записи.
    pub takes: Vec<TakePlan>,
    pub deadlines: Deadlines,
    /// Заголовок журнала записи (кто, что, с какими настройками).
    pub header: String,
    /// Подстроки строк `AFXERROR`, которые не останавливают запись (D-09, из `settings.json`).
    pub afxerror_ignore: Vec<String>,
}

/// Сколько заняла запись одного дубля — для будущей настройки дедлайнов.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeTiming {
    pub index: u32,
    /// Реальное время от `TAKE_START` до `TAKE_END`.
    pub wall: Duration,
    /// Игровая длина дубля по плану, секунды.
    pub game_secs: f32,
}

/// Итог удачной записи.
#[derive(Debug, Clone)]
pub struct RecordReport {
    pub takes: Vec<TakeTiming>,
    pub elapsed: Duration,
    pub launcher: LauncherFact,
    pub game: Liveness,
}

/// Событие записи для окна и журнала.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Вошли в состояние лестницы; для дубля — `(номер, всего)`.
    State {
        state: RecordState,
        take: Option<(u32, u32)>,
    },
    /// Дубль `n` из `m` записан.
    TakeDone { n: u32, m: u32 },
    /// Любой факт, который стоит показать как строку журнала.
    Fact(String),
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::State {
                state: RecordState::Take,
                take: Some((n, m)),
            } => write!(f, "запись: дубль {n} из {m}"),
            Event::State { state, .. } => write!(f, "запись: {}", state.journal()),
            // этот формат читает окно (`status.rs`): не менять
            Event::TakeDone { n, m } => write!(f, "записан дубль {n}/{m}"),
            Event::Fact(s) => f.write_str(s),
        }
    }
}

/// Сессия игры, которая завершается при любом выходе из `record`.
struct Guard(Box<dyn Session>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.terminate();
    }
}

/// Строка о том, какие дубли ждём: пишется в журнал до запуска лаунчера (TAK-01).
fn expected_line(takes: &[TakePlan]) -> String {
    let files = TakePlan::FILES.join(" и ");
    match (takes.first(), takes.last()) {
        (Some(a), Some(b)) => {
            format!(
                "ожидаю дублей: {} — {}..{}, в каждом {files}",
                takes.len(),
                a.dir_name(),
                b.dir_name()
            )
        }
        _ => "ожидаю дублей: 0".to_owned(),
    }
}

/// Журнал одной записи: `record.log`, счётчики строк HLAE и время от начала записи.
struct Journal {
    log: Option<log::RecordLog>,
    t0: Duration,
    hlae_stdout: u32,
    hlae_stderr: u32,
    tail: log::Tail,
    /// Консольный порт сейчас подключён: строки `console.log` тогда дублируют netcon.
    netcon_up: bool,
}

impl Journal {
    /// Открыть `record.log` до запуска лаунчера; не открылся — запись идёт без файла.
    fn open(req: &RecordRequest, t0: Duration, p: &dyn Progress) -> Self {
        let log = match log::RecordLog::open(
            &req.out_dir,
            &req.header,
            &req.launch.nonce,
            req.takes.len(),
        ) {
            Ok(l) => Some(l),
            Err(e) => {
                p.log(&format!("record.log не записан: {e}"));
                None
            }
        };
        Journal {
            log,
            t0,
            hlae_stdout: 0,
            hlae_stderr: 0,
            tail: log::Tail::new(),
            netcon_up: false,
        }
    }

    fn line(&mut self, now: Duration, source: &str, text: &str) {
        let at = now.saturating_sub(self.t0);
        if let Some(l) = &mut self.log {
            l.line(at, source, text);
        }
    }

    /// Событие записи: в окно и в журнал.
    fn emit(&mut self, now: Duration, p: &dyn Progress, e: &Event) {
        p.event(e);
        self.line(now, "app", &e.to_string());
    }

    /// Событие сессии: строки консоли дословно, остальное — фактами хоста.
    fn session_event(&mut self, now: Duration, ev: &SessionEvent) {
        match ev {
            SessionEvent::Line { source, text } => {
                match source {
                    Source::HlaeStdout => self.hlae_stdout += 1,
                    Source::HlaeStderr => self.hlae_stderr += 1,
                    _ => {}
                }
                self.line(now, source.label(), text);
                // netcon уже даёт те же строки, что и `console.log`: второй канал в хвост не дублируется
                if *source == Source::ConsoleLog && self.netcon_up {
                    self.tail.shadow(text);
                } else {
                    self.tail.push(source.label(), text);
                }
            }
            SessionEvent::Note(t) => self.line(now, "host", t),
            SessionEvent::ConsoleUp => {
                self.netcon_up = true;
                self.line(now, "host", "консольный порт подключён");
            }
            SessionEvent::ConsoleDown => {
                self.netcon_up = false;
                self.line(now, "host", "консольный порт закрыт");
            }
            SessionEvent::JobEmpty => self.line(now, "host", "в job не осталось процессов"),
            // выход лаунчера, появление и выход игры машина сама превращает в факты
            SessionEvent::LauncherExited { .. }
            | SessionEvent::GameAppeared { .. }
            | SessionEvent::GameExited { .. } => {}
        }
    }

    /// Сколько строк напечатал HLAE.exe: ноль — тоже факт, а не отсутствие данных.
    fn hlae_counts(&mut self, now: Duration) {
        let text = format!(
            "HLAE.exe: stdout — {} строк, stderr — {} строк",
            self.hlae_stdout, self.hlae_stderr
        );
        self.line(now, "app", &text);
    }

    fn success(
        &mut self,
        now: Duration,
        p: &dyn Progress,
        takes: usize,
        launcher: &LauncherFact,
        game: &Liveness,
    ) {
        self.emit(now, p, &Event::Fact(facts_line(launcher, game)));
        self.hlae_counts(now);
        let secs = now.saturating_sub(self.t0).as_secs_f64().round() as u64;
        self.line(
            now,
            "app",
            &format!("запись завершена: дублей {takes} за {secs} с"),
        );
    }

    /// Дописать к сбою то, что знает журнал: хвост консоли и строку фактов о двух процессах.
    fn failure(&mut self, now: Duration, p: &dyn Progress, f: &mut RecordFailure) {
        f.console_tail = self.tail.render(30);
        let facts = facts_line(&f.launcher, &f.game);
        if f.code != FailCode::Cancelled {
            f.message = format!("{} ({facts})", f.message);
        }
        self.emit(now, p, &Event::Fact(facts));
        self.hlae_counts(now);
        let at = now.saturating_sub(self.t0);
        if let Some(l) = &mut self.log {
            l.failure(at, f);
        }
    }
}

/// Как закрывать игру после конца записи (TMO-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Сначала `quit` (если консоль открыта) и ожидание выхода, потом завершение своего job.
    Graceful,
    /// Без `quit`: свой job завершается сразу, строки консоли дочитываются не дольше `drain`.
    Immediate { drain: Duration },
}

/// Режим закрытия по итогу записи. Сломанная игра `quit` не выполнит, но шторм строк `AFXERROR`
/// дочитывается секунду (хвост консоли в сбое должен его содержать).
fn stop_for(result: &Result<(), RecordFailure>) -> Stop {
    match result {
        Ok(()) => Stop::Immediate {
            drain: Duration::ZERO,
        },
        Err(f) => match f.code {
            FailCode::HlaeError => Stop::Immediate {
                drain: Duration::from_secs(1),
            },
            FailCode::GameExited => Stop::Immediate {
                drain: Duration::ZERO,
            },
            _ => Stop::Graceful,
        },
    }
}

/// Поэтапное закрытие игры (TMO-04, D-12): `quit` -> ожидание выхода не дольше `quit_wait` ->
/// завершение своего job -> подтверждение выхода по handle не дольше `kill_confirm`. Принудительная
/// отмена пропускает `quit` и ожидание. Возвращает то, что известно о живости игры после закрытия.
fn shutdown(
    host: &dyn Host,
    session: &mut Guard,
    machine: &Machine,
    j: &mut Journal,
    p: &dyn Progress,
    d: &Deadlines,
    stop: Stop,
) -> Liveness {
    let forced = p.force_cancelled();
    let stop = if forced {
        j.line(host.now(), "app", &failure::msg_force_cancel());
        Stop::Immediate {
            drain: Duration::ZERO,
        }
    } else {
        stop
    };
    match stop {
        Stop::Graceful if machine.console_up() => {
            j.line(host.now(), "app", "→ quit");
            match session.0.send("quit") {
                Err(e) => j.line(
                    host.now(),
                    "app",
                    &failure::msg_quit_not_sent(&e.to_string()),
                ),
                Ok(()) => {
                    let start = host.now();
                    let mut exited = false;
                    while !exited
                        && host.now().saturating_sub(start) < d.quit_wait
                        && !p.force_cancelled()
                    {
                        if let Some(ev) = session.0.poll(TICK) {
                            j.session_event(host.now(), &ev);
                            exited = matches!(
                                ev,
                                SessionEvent::GameExited { .. } | SessionEvent::JobEmpty
                            );
                        }
                    }
                    let waited = host.now().saturating_sub(start);
                    let text = if exited {
                        failure::msg_quit_exited(waited)
                    } else {
                        failure::msg_quit_ignored(d.quit_wait)
                    };
                    j.line(host.now(), "app", &text);
                }
            }
        }
        Stop::Graceful => {}
        Stop::Immediate { drain } => {
            let start = host.now();
            while host.now().saturating_sub(start) < drain {
                if let Some(ev) = session.0.poll(TICK) {
                    j.session_event(host.now(), &ev);
                }
            }
        }
    }
    session.0.terminate();
    j.line(host.now(), "app", &failure::msg_job_ended());
    let l = session.0.wait_exit(d.kill_confirm);
    let text = match &l {
        Liveness::Exited(_) => Some(failure::msg_exit_confirmed()),
        Liveness::Alive => Some(failure::msg_exit_pending(d.kill_confirm, d.prev_game_exit)),
        Liveness::Unknown(e) => Some(failure::msg_exit_unknown(e)),
        Liveness::NotStarted => None,
    };
    if let Some(t) = text {
        j.line(host.now(), "app", &t);
    }
    l
}

/// Номер дубля из имени папки: ровно `take` и четыре цифры.
fn take_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("take")?;
    if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// Пометить недописанные дубли (D-07): папка `takeNNNN`, которой нет среди готовых, становится
/// `takeNNNN.partial`. Готовые дубли и всё, что не похоже на `take` + 4 цифры, не трогаются; ничего
/// не удаляется. Вызывать только после подтверждённого выхода игры: файл дубля может держать
/// ffmpeg HLAE. Возвращает строки для журнала.
pub fn mark_partial(clips: &Path, done: &[u32]) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(clips) else {
        return vec![];
    };
    let mut found: Vec<(u32, String)> = dir
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            take_number(&name).map(|n| (n, name))
        })
        .filter(|(n, _)| !done.contains(n))
        .collect();
    found.sort();
    let mut notes = vec![];
    for (_, name) in found {
        let mut target_name = format!("{name}.partial");
        if clips.join(&target_name).exists() {
            let stamp = std::time::UNIX_EPOCH
                .elapsed()
                .map(|d| d.as_secs())
                .unwrap_or(0);
            target_name = format!("{name}.partial-{stamp}");
        }
        let from = clips.join(&name);
        let mut last_error = String::new();
        let mut renamed = false;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(500));
            }
            match std::fs::rename(&from, clips.join(&target_name)) {
                Ok(()) => {
                    renamed = true;
                    break;
                }
                Err(e) => last_error = e.to_string(),
            }
        }
        notes.push(if renamed {
            failure::msg_partial_marked(&name, &target_name)
        } else {
            failure::msg_partial_failed(&name, &last_error)
        });
    }
    notes
}

/// Записать все дубли плана. Успех — все метки пришли в срок и игра закрыта; иначе `RecordFailure`
/// с кодом, состоянием и прошедшим временем. Игра в любом случае завершается до возврата.
pub fn record(
    host: &dyn Host,
    req: &RecordRequest,
    p: &dyn Progress,
) -> Result<RecordReport, RecordFailure> {
    let t0 = host.now();
    let mut j = Journal::open(req, t0, p);
    j.emit(t0, p, &Event::Fact(expected_line(&req.takes)));
    let mut machine = Machine::new(
        req.launch.nonce.clone(),
        req.takes.clone(),
        req.deadlines.clone(),
    );
    for a in machine.start(t0) {
        if let Action::Emit(e) = a {
            j.emit(t0, p, &e);
        }
    }
    let session = match host.launch(&req.launch) {
        Ok(s) => s,
        Err(e) => {
            let mut f = machine.launch_failed(host.now(), &e);
            j.failure(host.now(), p, &mut f);
            return Err(f);
        }
    };
    let mut session = Guard(session);
    // как была игра в момент решения: после Terminate она уже «не жива» из-за нас
    let mut liveness: Option<Liveness> = None;

    loop {
        let ev = session.0.poll(TICK);
        let now = host.now();
        if let Some(e) = &ev {
            j.session_event(now, e);
        }
        let input = if p.cancelled() || p.force_cancelled() {
            Input::Cancel
        } else {
            ev.map_or(Input::Tick, Input::Event)
        };
        let mut queue: VecDeque<Action> = machine.step(now, input).into();
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Send(line) => {
                    j.line(host.now(), "app", &format!("→ {line}"));
                    if let Err(e) = session.0.send(&line) {
                        let more = machine.step(
                            host.now(),
                            Input::SendFailed {
                                line,
                                error: e.to_string(),
                            },
                        );
                        queue.extend(more);
                    }
                }
                Action::Emit(e) => j.emit(host.now(), p, &e),
                Action::Terminate => {
                    liveness.get_or_insert_with(|| session.0.game_liveness());
                    session.0.terminate();
                }
                Action::Finish(Ok(())) => {
                    let game = liveness.unwrap_or_else(|| session.0.game_liveness());
                    let launcher = machine.launcher();
                    let stop = stop_for(&Ok(()));
                    shutdown(
                        host,
                        &mut session,
                        &machine,
                        &mut j,
                        p,
                        &req.deadlines,
                        stop,
                    );
                    let now = host.now();
                    j.success(now, p, req.takes.len(), &launcher, &game);
                    return Ok(RecordReport {
                        takes: machine.timings().to_vec(),
                        elapsed: now.saturating_sub(t0),
                        launcher,
                        game,
                    });
                }
                Action::Finish(Err(mut f)) => {
                    f.game = liveness.unwrap_or_else(|| session.0.game_liveness());
                    f.launcher = machine.launcher();
                    let stop = stop_for(&Err(f.clone()));
                    shutdown(
                        host,
                        &mut session,
                        &machine,
                        &mut j,
                        p,
                        &req.deadlines,
                        stop,
                    );
                    // дубль могла держать запись до самого выхода игры: помечаем после него
                    let done: Vec<u32> = machine.timings().iter().map(|t| t.index).collect();
                    for note in mark_partial(&req.out_dir.join("clips"), &done) {
                        j.line(host.now(), "app", &note);
                    }
                    j.failure(host.now(), p, &mut f);
                    return Err(f);
                }
            }
        }
    }
}
