//! Поток наблюдения за сессией: выход лаунчера, игра в job, смерть игры, пустой job.
//!
//! Факты лаунчера и игры независимы (LCH-06): HLAE выходит через 2–3 с с кодом 0, а игра живёт
//! дальше (спайк S1). Живость игры — ожидание на её handle; ошибка опроса — `Note`, а не «жива».
//! Завершать по имени образа здесь нечего: поток только наблюдает, а убивает `Session::terminate`.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::job::Job;
use super::process::{OwnedProc, snapshot_processes};
use super::{Shared, Sink, lock};
use crate::{Liveness, SessionEvent};

/// Шаг опроса: достаточно часто для дедлайнов в секундах и достаточно редко для процессора.
const STEP: Duration = Duration::from_millis(250);
/// Сколько поток живёт после остановки сессии, чтобы увидеть выход игры и пустой job.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// Всё, что нужно потоку наблюдения.
pub(crate) struct Watch {
    pub job: Arc<Job>,
    pub child: Child,
    pub shared: Arc<Shared>,
    pub sink: Arc<Sink>,
    /// Имя образа игры, например `cs2.exe`.
    pub game_image: String,
    /// Ожидаемый путь образа игры; `None` — принятие по PID выключено.
    pub expected_exe: Option<PathBuf>,
    /// Время создания лаунчера (FILETIME): игру старше него принимать нельзя.
    pub launcher_created: Option<u64>,
}

pub(crate) fn spawn(w: Watch) {
    std::thread::spawn(move || run(w));
}

/// Что поток уже сообщил (каждое событие — один раз).
#[derive(Default)]
struct Told {
    launcher: bool,
    game_exit: bool,
    job_empty: bool,
    job_error: bool,
    liveness_unknown: bool,
    child_error: bool,
    launcher_gone_at: Option<Instant>,
    last_adopt_try: Option<Instant>,
    last_note: Option<String>,
}

impl Told {
    /// `Note`, который не повторяется подряд с тем же текстом (попытки принятия идут раз в секунду).
    fn note_once(&mut self, sink: &Sink, text: String) {
        if self.last_note.as_deref() != Some(text.as_str()) {
            sink.note(text.clone());
            self.last_note = Some(text);
        }
    }
}

fn run(mut w: Watch) {
    let mut told = Told::default();
    let mut stop_seen: Option<Instant> = None;
    loop {
        // после `terminate` ещё немного наблюдаем: выход игры и пустой job — факты для журнала
        if w.shared.stopped() {
            let since = *stop_seen.get_or_insert_with(Instant::now);
            if since.elapsed() > STOP_GRACE {
                return;
            }
        }
        launcher(&mut w, &mut told);
        find_game(&w, &mut told);
        adopt_game(&w, &mut told);
        if game_state(&w, &mut told) {
            return;
        }
        std::thread::sleep(STEP);
    }
}

/// (1) Пока лаунчер не вышел — ждём его выхода.
fn launcher(w: &mut Watch, told: &mut Told) {
    if told.launcher {
        return;
    }
    match w.child.try_wait() {
        Ok(Some(status)) => {
            told.launcher = true;
            told.launcher_gone_at = Some(Instant::now());
            let code = status.code();
            *lock(&w.shared.launcher_exit) = Some(code);
            w.sink.send(SessionEvent::LauncherExited { code });
        }
        Ok(None) => {}
        Err(e) => {
            if !told.child_error {
                told.child_error = true;
                w.sink
                    .note(format!("состояние лаунчера не определено: {e}"));
            }
        }
    }
}

/// (2) Пока игры нет — ищем процесс с нужным образом среди PID job (кроме лаунчера).
fn find_game(w: &Watch, told: &mut Told) {
    if lock(&w.shared.game).is_some() {
        return;
    }
    let pids = match w.job.pids() {
        Ok(p) => p,
        Err(e) => {
            if !told.job_error {
                told.job_error = true;
                w.sink.note(format!("список процессов job: {e}"));
            }
            return;
        }
    };
    let launcher_pid = w.child.id();
    for pid in pids.into_iter().filter(|p| *p != launcher_pid) {
        let Ok(proc) = OwnedProc::open(pid) else {
            continue;
        };
        let Some(path) = proc.image_path() else {
            continue;
        };
        let is_game = path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&w.game_image));
        if is_game {
            *lock(&w.shared.game) = Some(proc);
            w.sink.send(SessionEvent::GameAppeared { pid });
            return;
        }
    }
}

/// Через сколько после выхода лаунчера, если игры в job нет, пробуем принять её по PID.
const ADOPT_AFTER: Duration = Duration::from_secs(5);
/// Как часто повторяются попытки принятия.
const ADOPT_RETRY: Duration = Duration::from_secs(1);

/// (2б) Запасной путь D-02: лаунчер вышел, а игры в job нет — ищем её среди процессов системы.
/// Принимается только единственный кандидат с ожидаемым путём, созданный после лаунчера (T-01-19).
fn adopt_game(w: &Watch, told: &mut Told) {
    let (Some(expected), Some(not_before), Some(gone_at)) = (
        w.expected_exe.as_deref(),
        w.launcher_created,
        told.launcher_gone_at,
    ) else {
        return;
    };
    if lock(&w.shared.game).is_some() || w.shared.stopped() || gone_at.elapsed() < ADOPT_AFTER {
        return;
    }
    if told
        .last_adopt_try
        .is_some_and(|t| t.elapsed() < ADOPT_RETRY)
    {
        return;
    }
    told.last_adopt_try = Some(Instant::now());

    let entries = match snapshot_processes() {
        Ok(e) => e,
        Err(e) => {
            told.note_once(&w.sink, format!("список процессов системы: {e}"));
            return;
        }
    };
    let mut found = 0usize;
    let mut candidates = Vec::new();
    for entry in entries
        .iter()
        .filter(|e| e.exe_name.eq_ignore_ascii_case(&w.game_image))
    {
        found += 1;
        let Ok(proc) = OwnedProc::open(entry.pid) else {
            continue;
        };
        let (Some(image), Some(created)) = (proc.image_path(), proc.creation_time()) else {
            continue;
        };
        candidates.push(Candidate {
            pid: entry.pid,
            image,
            created,
            // не удалось узнать — считаем, что процесс не наш, но и не принимаем (осторожно)
            in_our_job: proc.in_job(&w.job).unwrap_or(true),
        });
    }
    let Some(pid) = pick_adoptee(&candidates, expected, not_before) else {
        if found > 0 {
            told.note_once(
                &w.sink,
                format!(
                    "найдено {found} процессов {}, ни один не принят",
                    w.game_image
                ),
            );
        }
        return;
    };

    let image = &w.game_image;
    let (proc, without_job) = match w.job.assign_pid(pid) {
        Ok(()) => {
            w.sink.note(format!(
                "{image} не попала в job через HLAE — принята по PID {pid}"
            ));
            (OwnedProc::open(pid), false)
        }
        Err(e) => {
            w.sink.note(format!(
                "{image} не удалось поместить в job ({e}) — живость и завершение по своему handle"
            ));
            (OwnedProc::open_owned(pid), true)
        }
    };
    match proc {
        Ok(proc) => {
            w.shared
                .adopted_without_job
                .store(without_job, Ordering::SeqCst);
            *lock(&w.shared.game) = Some(proc);
            w.sink.send(SessionEvent::GameAppeared { pid });
        }
        Err(e) => told.note_once(&w.sink, format!("принятый процесс {pid} не открылся: {e}")),
    }
}

/// (3)–(4) Живость игры по handle и пустой job. `true` — наблюдать больше нечего.
fn game_state(w: &Watch, told: &mut Told) -> bool {
    let liveness = match lock(&w.shared.game).as_ref() {
        Some(game) => game.liveness(),
        None => return false,
    };
    match liveness {
        Liveness::Exited(code) if !told.game_exit => {
            told.game_exit = true;
            w.shared.game_gone.store(true, Ordering::SeqCst);
            w.sink.send(SessionEvent::GameExited { code });
        }
        Liveness::Unknown(e) if !told.liveness_unknown => {
            told.liveness_unknown = true;
            w.sink
                .note(format!("живость {} не определена: {e}", w.game_image));
        }
        _ => {}
    }
    if w.shared.adopted_without_job.load(Ordering::SeqCst) {
        // игра не в нашем job: пустота job ничего не говорит, конец наблюдения — смерть игры
        return told.game_exit;
    }
    match w.job.active_count() {
        Ok(0) if !told.job_empty => {
            told.job_empty = true;
            w.sink.send(SessionEvent::JobEmpty);
            true
        }
        Ok(_) => false,
        Err(e) => {
            if !told.job_error {
                told.job_error = true;
                w.sink.note(format!("учёт процессов job: {e}"));
            }
            false
        }
    }
}

/// Процесс-кандидат на принятие по PID (запасной путь D-02).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub pid: u32,
    /// Полный путь образа.
    pub image: PathBuf,
    /// Время создания процесса, FILETIME.
    pub created: u64,
    /// Процесс уже состоит в нашем job.
    pub in_our_job: bool,
}

/// Путь для сравнения: без регистра, с одним видом разделителей, без префикса `\?\`.
fn normalized(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    let s = s.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(s);
    s.trim_end_matches('\\').to_owned()
}

/// Кого принять по PID (T-01-19): только единственного кандидата, чей путь образа совпал с
/// ожидаемым, созданного не раньше лаунчера (`not_before`) и ещё не состоящего в нашем job.
/// Два подходящих или ни одного — `None`: чужой `cs2.exe` пользователя не принимается и не
/// завершается.
pub(crate) fn pick_adoptee(
    candidates: &[Candidate],
    expected: &Path,
    not_before: u64,
) -> Option<u32> {
    let expected = normalized(expected);
    let mut fit = candidates
        .iter()
        .filter(|c| !c.in_our_job && c.created >= not_before && normalized(&c.image) == expected);
    let first = fit.next()?;
    fit.next().is_none().then_some(first.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: &str = r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe";
    const LAUNCHER_AT: u64 = 1_000;

    fn cand(pid: u32, image: &str, created: u64, in_our_job: bool) -> Candidate {
        Candidate {
            pid,
            image: image.into(),
            created,
            in_our_job,
        }
    }

    fn pick(c: &[Candidate]) -> Option<u32> {
        pick_adoptee(c, Path::new(EXPECTED), LAUNCHER_AT)
    }

    #[test]
    fn adoption_takes_only_a_single_fresh_candidate_with_the_expected_path() {
        // один подходящий
        assert_eq!(pick(&[cand(7, EXPECTED, 2_000, false)]), Some(7));
        // регистр и разделители не важны
        let slashed = EXPECTED.replace('\\', "/").to_lowercase();
        assert_eq!(pick(&[cand(8, &slashed, LAUNCHER_AT, false)]), Some(8));
        // два подходящих — не принимаем никого
        assert_eq!(
            pick(&[
                cand(7, EXPECTED, 2_000, false),
                cand(9, EXPECTED, 3_000, false)
            ]),
            None
        );
        // чужой путь
        assert_eq!(pick(&[cand(7, r"D:\games\cs2.exe", 2_000, false)]), None);
        // создан раньше лаунчера (чужой, давно запущенный cs2.exe пользователя)
        assert_eq!(pick(&[cand(7, EXPECTED, LAUNCHER_AT - 1, false)]), None);
        // уже в нашем job — принимать нечего
        assert_eq!(pick(&[cand(7, EXPECTED, 2_000, true)]), None);
        // лишние кандидаты не мешают единственному подходящему
        assert_eq!(
            pick(&[
                cand(7, EXPECTED, 2_000, false),
                cand(9, EXPECTED, 3_000, true),
                cand(11, r"D:\games\cs2.exe", 3_000, false),
                cand(13, EXPECTED, 10, false),
            ]),
            Some(7)
        );
        assert_eq!(pick(&[]), None);
    }
}
