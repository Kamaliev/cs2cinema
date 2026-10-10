//! Поток наблюдения за сессией: выход лаунчера, игра в job, смерть игры, пустой job.
//!
//! Факты лаунчера и игры независимы (LCH-06): HLAE выходит через 2–3 с с кодом 0, а игра живёт
//! дальше (спайк S1). Живость игры — ожидание на её handle; ошибка опроса — `Note`, а не «жива».
//! Завершать по имени образа здесь нечего: поток только наблюдает, а убивает `Session::terminate`.

use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::job::Job;
use super::process::OwnedProc;
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
