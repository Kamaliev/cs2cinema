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
pub mod processes;
pub mod verify;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use host::{Host, LaunchSpec, Liveness, Session, SessionEvent, Source};
use renderer::TakePlan;

use crate::{Progress, manifest};
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
    /// Проверка дублей `ffprobe` и `clips/manifest.json` (D-02); `None` — без проверки, как раньше
    /// (старые тесты и CLI до фазы 4).
    pub verify: Option<Verify>,
}

/// Как проверять дубли по ходу записи (D-02, D-04, D-05).
#[derive(Debug, Clone)]
pub struct Verify {
    /// `ffprobe`: полный путь или имя из `PATH`.
    pub ffprobe: PathBuf,
    /// Хеш скриптов плана для `clips/manifest.json`.
    pub plan_hash: String,
    /// Номер попытки записи (с единицы) — в манифест.
    pub attempt: u32,
    /// Снимок окружения попытки (OBS-03); без него в манифест идёт пустой.
    pub env: Option<manifest::EnvSnapshot>,
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
pub(crate) fn take_number(name: &str) -> Option<u32> {
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

/// Дубль, закончившийся по `TAKE_END`, но ещё не получивший окончательного решения.
struct Pending {
    /// Позиция дубля в `takes`.
    take: usize,
    /// Когда пришёл `TAKE_END`.
    first_at: Duration,
    /// Не раньше этого момента проверять снова.
    next_at: Duration,
    /// Размеры `video.mp4` и `audio.wav` при прошлой проверке.
    sizes: Option<(u64, u64)>,
    /// Первое «ещё дописывается» уже записано в журнал.
    noted: bool,
}

/// Через сколько повторять проверку недописанного файла.
const REPROBE: Duration = Duration::from_secs(1);

/// Негодный ответ при неизменных размерах файлов становится окончательным через столько.
const SETTLE: Duration = Duration::from_secs(2);

/// Решение по проверенному дублю.
enum Verdict {
    /// Дубль годен или проверка будет повторена позже.
    Settled,
    /// Пользователь отменил задачу во время `ffprobe`.
    Cancelled,
    /// Дубль негоден окончательно: готовое сообщение сбоя.
    Rejected(String),
}

/// Ведёт `clips/manifest.json` и очередь проверок дублей. Ничего не ждёт: каждая проверка — один
/// вызов `Host::run` с дедлайном и отменой, не больше одной за шаг цикла драйвера (T-03-11).
struct Verifier<'a> {
    cfg: &'a Verify,
    clips: PathBuf,
    manifest: manifest::Manifest,
    pending: Vec<Pending>,
    /// (номер, папка) негодного дубля — для подсказки сбоя (D-04).
    invalid: Option<(u32, String)>,
}

impl<'a> Verifier<'a> {
    /// Манифест попытки: все дубли `pending`, снимок окружения, первая запись на диск.
    fn open(req: &RecordRequest, cfg: &'a Verify, now: Duration, j: &mut Journal) -> Self {
        let mut manifest = manifest::Manifest::new(
            cfg.plan_hash.clone(),
            req.launch.nonce.clone(),
            req.launch.size,
            &req.takes,
        );
        manifest.begin_attempt(cfg.attempt, cfg.env.clone().unwrap_or_default());
        let v = Verifier {
            cfg,
            clips: req.out_dir.join("clips"),
            manifest,
            pending: vec![],
            invalid: None,
        };
        v.save(now, j);
        v
    }

    /// Сохранить манифест; ошибка — строка журнала, запись идёт дальше.
    fn save(&self, now: Duration, j: &mut Journal) {
        if let Err(e) = self.manifest.save(&self.clips) {
            j.line(now, "app", &format!("manifest.json не записан: {e}"));
        }
    }

    /// Размеры файлов дубля, если оба на месте.
    fn sizes(&self, dir: &str) -> Option<(u64, u64)> {
        let size = |file: &str| {
            std::fs::metadata(self.clips.join(dir).join(file))
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len())
        };
        Some((size(TakePlan::FILES[0])?, size(TakePlan::FILES[1])?))
    }

    /// `TAKE_END` принят: файлы дубля легли на диск, проверка — сразу (D-02).
    fn take_ended(&mut self, take: usize, req: &RecordRequest, now: Duration, j: &mut Journal) {
        let plan = &req.takes[take];
        let sizes = self.sizes(&plan.dir_name());
        if let Some(s) = sizes {
            self.manifest.mark_recorded(plan.index, s);
            self.save(now, j);
        }
        self.pending.push(Pending {
            take,
            first_at: now,
            next_at: now,
            sizes,
            noted: false,
        });
    }

    /// Проверить первый дубль, которому пора. Возвращает действия машины, если проверка её
    /// остановила (сбой дубля или отмена).
    fn probe_due(
        &mut self,
        host: &dyn Host,
        req: &RecordRequest,
        p: &dyn Progress,
        j: &mut Journal,
        machine: &mut Machine,
    ) -> Vec<Action> {
        let Some(pos) = self.pending.iter().position(|x| x.next_at <= host.now()) else {
            return vec![];
        };
        match self.probe(pos, host, req, p, j, false) {
            Verdict::Settled => vec![],
            Verdict::Cancelled => machine.step(host.now(), Input::Cancel),
            Verdict::Rejected(message) => machine.step(host.now(), Input::TakeInvalid { message }),
        }
    }

    /// Одна проверка дубля `pos` из очереди и решение по ней. Окончательно решённый дубль уходит из
    /// очереди. `last` — игра закрыта, ждать дописывания больше нечего: повторов нет.
    fn probe(
        &mut self,
        pos: usize,
        host: &dyn Host,
        req: &RecordRequest,
        p: &dyn Progress,
        j: &mut Journal,
        last: bool,
    ) -> Verdict {
        let take = self.pending[pos].take;
        let plan = &req.takes[take];
        let dir = plan.dir_name();
        let (n, m) = (take as u32 + 1, req.takes.len() as u32);
        let current = self.sizes(&dir);
        let outcome = verify::probe_take(
            host,
            &self.cfg.ffprobe,
            &self.clips,
            plan,
            req.launch.size,
            req.deadlines.ffprobe,
            &|| p.cancelled() || p.force_cancelled(),
        );
        let now = host.now();
        // ffprobe прочитал контейнер, который ffmpeg HLAE ещё дописывает: размер после проверки ушёл
        // вперёд от снятого до неё. Это «ещё пишется», а не «годен» — иначе склейка увидит другой размер.
        let outcome = match outcome {
            verify::ProbeOutcome::Valid(f) => {
                let before = (f.video_bytes, f.audio_bytes);
                let after = self.sizes(&dir);
                if after == Some(before) {
                    verify::ProbeOutcome::Valid(f)
                } else {
                    if let Some(s) = after {
                        self.manifest.mark_recorded(plan.index, s);
                        self.save(now, j);
                    }
                    verify::ProbeOutcome::NotReady(failure::msg_take_growing(before, after))
                }
            }
            other => other,
        };
        let finalize = req.deadlines.take_finalize;
        let pend = &mut self.pending[pos];
        let changed = current != pend.sizes;
        pend.sizes = current;
        let waited = now.saturating_sub(pend.first_at);
        let reason = match outcome {
            verify::ProbeOutcome::Cancelled => return Verdict::Cancelled,
            verify::ProbeOutcome::Valid(f) => {
                self.manifest
                    .mark_validated(plan.index, &f, self.cfg.attempt);
                self.save(now, j);
                j.emit(now, p, &Event::Fact(failure::msg_take_validated(n, m, &f)));
                self.pending.remove(pos);
                return Verdict::Settled;
            }
            verify::ProbeOutcome::NotReady(r) => {
                if !last && waited < finalize {
                    pend.next_at = now + REPROBE;
                    if !pend.noted {
                        pend.noted = true;
                        let text = format!(
                            "дубль {n}/{m}: {dir} ещё дописывается (ffmpeg HLAE), проверю снова: {r}"
                        );
                        j.line(now, "app", &text);
                    }
                    return Verdict::Settled;
                }
                failure::msg_take_not_final(&dir, finalize, &r)
            }
            verify::ProbeOutcome::Invalid(r) => {
                if !last && waited < finalize && (changed || waited < SETTLE) {
                    pend.next_at = now + REPROBE;
                    return Verdict::Settled;
                }
                r
            }
        };
        self.manifest
            .mark_invalid(plan.index, &reason, self.cfg.attempt);
        self.save(now, j);
        self.pending.remove(pos);
        self.invalid.get_or_insert((n, dir.clone()));
        Verdict::Rejected(failure::msg_take_invalid(n, m, &dir, &reason))
    }

    /// После закрытия игры (job пуст — ffmpeg HLAE закончил) каждый оставшийся дубль проверяется
    /// один раз. Негодный — сбой `TAKE_INVALID` (первый из негодных), отмена — сбой `Cancelled`.
    fn verify_rest(
        &mut self,
        host: &dyn Host,
        req: &RecordRequest,
        p: &dyn Progress,
        j: &mut Journal,
    ) -> Option<RecordFailure> {
        let waited = self
            .pending
            .first()
            .map(|x| host.now().saturating_sub(x.first_at))
            .unwrap_or_default();
        let mut first: Option<String> = None;
        while !self.pending.is_empty() {
            match self.probe(0, host, req, p, j, true) {
                Verdict::Settled => {}
                Verdict::Cancelled => {
                    return Some(RecordFailure::new(
                        FailCode::Cancelled,
                        RecordState::Closing,
                        failure::msg_cancelled(),
                        waited,
                    ));
                }
                Verdict::Rejected(message) => {
                    first.get_or_insert(message);
                }
            }
        }
        first.map(|message| {
            let mut f =
                RecordFailure::new(FailCode::TakeInvalid, RecordState::Take, message, waited);
            if let Some(hint) = self.hint() {
                f.hint = hint;
            }
            f
        })
    }

    /// Подсказка сбоя `TAKE_INVALID` с номером и папкой дубля (D-04).
    fn hint(&self) -> Option<String> {
        self.invalid
            .as_ref()
            .map(|(n, dir)| failure::hint_take_invalid(*n, dir))
    }

    /// Итог попытки в манифест.
    fn end(&mut self, outcome: &str, now: Duration, j: &mut Journal) {
        self.manifest.end_attempt(outcome);
        self.save(now, j);
    }
}

/// Записать свои процессы сессии в `processes.json` (D-15); пустой список не пишется, ошибка записи —
/// строка журнала.
fn save_processes(out_dir: &Path, session: &Guard, j: &mut Journal, now: Duration) {
    let list = session.0.processes();
    if list.is_empty() {
        return;
    }
    if let Err(e) = processes::save(out_dir, &list) {
        j.line(now, "app", &format!("processes.json не записан: {e}"));
    }
}

/// После закрытия игры: подтверждённый выход (или игры не было) — `processes.json` больше не нужен;
/// иначе он остаётся для следующего запуска приложения.
fn settle_processes(out_dir: &Path, live: &Liveness, j: &mut Journal, now: Duration) {
    match live {
        Liveness::Exited(_) | Liveness::NotStarted => processes::remove(out_dir),
        Liveness::Alive | Liveness::Unknown(_) => j.line(
            now,
            "app",
            "processes.json оставлен: игра может быть жива — её завершит следующий запуск приложения",
        ),
    }
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
    machine.ignore_afxerror(req.afxerror_ignore.clone());
    if !req.afxerror_ignore.is_empty() {
        // что могло быть пропущено, видно в журнале (T-02-13)
        j.line(
            t0,
            "app",
            &format!("исключения AFXERROR: {}", req.afxerror_ignore.join(" | ")),
        );
    }
    for a in machine.start(t0) {
        if let Action::Emit(e) = a {
            j.emit(t0, p, &e);
        }
    }
    let mut verifier = req
        .verify
        .as_ref()
        .map(|v| Verifier::open(req, v, t0, &mut j));
    let session = match host.launch(&req.launch) {
        Ok(s) => s,
        Err(e) => {
            let mut f = machine.launch_failed(host.now(), &e);
            j.failure(host.now(), p, &mut f);
            if let Some(vf) = &mut verifier {
                vf.end(
                    &format!("{}: {}", f.code.as_str(), f.message),
                    host.now(),
                    &mut j,
                );
            }
            return Err(f);
        }
    };
    let mut session = Guard(session);
    save_processes(&req.out_dir, &session, &mut j, host.now());
    // опрос окон своего job (D-10): когда смотрели в последний раз и какой набор окон был тогда
    let mut windows_at: Option<Duration> = None;
    let mut windows_seen: Vec<(String, String)> = vec![];

    loop {
        let ev = session.0.poll(TICK);
        let now = host.now();
        if let Some(e) = &ev {
            j.session_event(now, e);
            if matches!(e, SessionEvent::GameAppeared { .. }) {
                save_processes(&req.out_dir, &session, &mut j, now);
            }
        }
        let windows = if (RecordState::Launcher..=RecordState::Take).contains(&machine.state())
            && windows_at.is_none_or(|t| now.saturating_sub(t) >= req.deadlines.window_poll)
        {
            windows_at = Some(now);
            let w = session.0.windows();
            let seen: Vec<(String, String)> = w
                .iter()
                .map(|x| (x.class.clone(), x.title.clone()))
                .collect();
            if seen != windows_seen {
                j.line(now, "host", &failure::windows_line(&w));
                windows_seen = seen;
            }
            Some(w)
        } else {
            None
        };
        let input = if p.cancelled() || p.force_cancelled() {
            Input::Cancel
        } else {
            ev.map_or(Input::Tick, Input::Event)
        };
        let mut queue: VecDeque<Action> = machine.step(now, input).into();
        if let Some(w) = windows {
            // у завершённой машины `step` ничего не возвращает
            queue.extend(machine.step(now, Input::Windows(w)));
        }
        let mut probed = false;
        loop {
            while let Some(action) = queue.pop_front() {
                match action {
                    Action::Verify { take } => {
                        if let Some(vf) = &mut verifier {
                            vf.take_ended(take, req, host.now(), &mut j);
                        }
                    }
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
                    Action::Finish(Ok(())) => {
                        // как была игра в момент решения, до закрытия: закрываем её мы сами
                        let game = session.0.game_liveness();
                        let launcher = machine.launcher();
                        let stop = stop_for(&Ok(()));
                        let live = shutdown(
                            host,
                            &mut session,
                            &machine,
                            &mut j,
                            p,
                            &req.deadlines,
                            stop,
                        );
                        settle_processes(&req.out_dir, &live, &mut j, host.now());
                        // job пуст: ffmpeg HLAE закончил, последний дубль проверяется один раз
                        if let Some(vf) = &mut verifier
                            && let Some(mut f) = vf.verify_rest(host, req, p, &mut j)
                        {
                            f.game = game;
                            f.launcher = launcher;
                            j.failure(host.now(), p, &mut f);
                            let outcome = format!("{}: {}", f.code.as_str(), f.message);
                            vf.end(&outcome, host.now(), &mut j);
                            return Err(f);
                        }
                        let now = host.now();
                        j.success(now, p, req.takes.len(), &launcher, &game);
                        if let Some(vf) = &mut verifier {
                            vf.end("готово", now, &mut j);
                        }
                        return Ok(RecordReport {
                            takes: machine.timings().to_vec(),
                            elapsed: now.saturating_sub(t0),
                            launcher,
                            game,
                        });
                    }
                    Action::Finish(Err(mut f)) => {
                        f.game = session.0.game_liveness();
                        f.launcher = machine.launcher();
                        if matches!(
                            f.code,
                            FailCode::LauncherFailed
                                | FailCode::GameNotStarted
                                | FailCode::ConsoleNotOpen
                                | FailCode::DemoNotLoaded
                        ) {
                            // дедлайн запуска: заголовки окон игры объясняют, чего она ждёт (D-10)
                            let w = session.0.windows();
                            f.message.push_str(&failure::msg_windows(&w));
                        }
                        if f.code == FailCode::TakeInvalid
                            && let Some(hint) = verifier.as_ref().and_then(|v| v.hint())
                        {
                            f.hint = hint;
                        }
                        let stop = stop_for(&Err(f.clone()));
                        let live = shutdown(
                            host,
                            &mut session,
                            &machine,
                            &mut j,
                            p,
                            &req.deadlines,
                            stop,
                        );
                        settle_processes(&req.out_dir, &live, &mut j, host.now());
                        // дубль могла держать запись до самого выхода игры: помечаем после него
                        let done: Vec<u32> = machine.timings().iter().map(|t| t.index).collect();
                        for note in mark_partial(&req.out_dir.join("clips"), &done) {
                            j.line(host.now(), "app", &note);
                        }
                        j.failure(host.now(), p, &mut f);
                        if let Some(vf) = &mut verifier {
                            let outcome = format!("{}: {}", f.code.as_str(), f.message);
                            vf.end(&outcome, host.now(), &mut j);
                        }
                        return Err(f);
                    }
                }
            }
            // проверка дублей: не больше одной за шаг, чтобы смерть игры, AFXERROR, отмена и
            // следующая метка не ждали файла (T-03-11)
            if probed {
                break;
            }
            probed = true;
            if let Some(vf) = &mut verifier {
                queue.extend(vf.probe_due(host, req, p, &mut j, &mut machine));
            }
            if queue.is_empty() {
                break;
            }
        }
    }
}
