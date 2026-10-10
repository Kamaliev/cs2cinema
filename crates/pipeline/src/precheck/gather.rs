//! Сбор фактов окружения для предпроверки: хост (процессы, место, версии), файлы, утилиты с
//! дедлайном. Оценка фактов — [`super::evaluate`]; здесь ничего не завершается и не запускается,
//! кроме `-version` утилит: у хоста нет метода «завершить по PID или имени» (PRE-05, TMO-05).
//!
//! Одну и ту же [`run`] вызывают задача из очереди и кнопка «Проверить окружение» в настройках,
//! поэтому отчёт у них общий (D-01, PRE-01).

use std::path::PathBuf;

use host::{Host, Liveness, PrevGame};

use super::{Facts, Level, PrevGameFact, Report, evaluate};
use crate::record::Deadlines;
use crate::{CANCELLED, Progress};

/// Откуда берётся демка задачи.
#[derive(Debug, Clone)]
pub enum Demo {
    /// Готовый файл: проверяется по первым байтам.
    File(PathBuf),
    /// Демку ещё предстоит скачать с FACEIT.
    Download,
    /// Проверка из настроек, без задачи.
    NotApplicable,
}

/// Что проверять.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// Папка HLAE (`HLAE.exe`, `x64/AfxHookSource2.dll`).
    pub hlae_dir: PathBuf,
    /// Папка игры («Counter-Strike Global Offensive»).
    pub cs2_dir: PathBuf,
    /// Корневая папка результатов.
    pub out_root: PathBuf,
    pub demo: Demo,
    /// Пара (сборка CS2, версия HLAE) последней удачной записи.
    pub last_good: Option<(String, String)>,
    /// Дедлайны — уже с «Запасом по времени», как у записи.
    pub deadlines: Deadlines,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Собрать все факты окружения и оценить их. Ждёт свою игру прошлой задачи (D-14), поэтому может
/// занять до `prev_game_exit + next_job_settle`; отмена прерывает ожидание и утилиты.
pub fn run(host: &dyn Host, i: &Inputs, p: &dyn Progress) -> crate::Result<Report> {
    crate::check(p)?;
    let mut facts = Facts::default();

    // D-14, TMO-07: сначала дожидаемся своей игры прошлой задачи, и только потом смотрим процессы —
    // иначе она сама окажется «чужим» CS2.
    facts.prev_game = match host.wait_previous_game(
        i.deadlines.prev_game_exit,
        i.deadlines.next_job_settle,
        &|| p.cancelled(),
    ) {
        PrevGame::Cancelled => return Err(CANCELLED.to_owned()),
        PrevGame::Clear { waited } if waited.is_zero() => PrevGameFact::Clear,
        PrevGame::Clear { waited } => {
            if waited.as_secs() >= 1 {
                p.log(&format!(
                    "предпроверка: ждал закрытия игры прошлой задачи {} с",
                    waited.as_secs_f64().round() as u64
                ));
            }
            PrevGameFact::Waited(waited)
        }
        PrevGame::Stuck {
            pid,
            since_end,
            liveness,
        } => PrevGameFact::Stuck {
            pid,
            secs: since_end.as_secs(),
            confirmed_alive: liveness == Liveness::Alive,
        },
    };
    crate::check(p)?;

    facts.processes = Some(match host.processes() {
        Ok(list) => Ok(list.into_iter().map(|x| (x.pid, x.exe_name)).collect()),
        Err(e) => Err(e.to_string()),
    });

    let report = evaluate(&facts);
    for item in &report.items {
        match item.level {
            Level::Ok => {}
            Level::Warn => p.log(&format!("предпроверка: ⚠ {}", item.message)),
            Level::Fail => p.log(&format!("предпроверка: ✖ {}", item.message)),
        }
    }
    Ok(report)
}
