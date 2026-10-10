//! Предпроверка окружения перед записью: сбор фактов (хост, файлы, утилиты — `gather`, план 02-05)
//! и чистая оценка [`evaluate`], которая превращает факты в отчёт из пунктов `ok`/`warn`/`fail`.
//!
//! Весь русский текст пунктов живёт здесь, чтобы задача из очереди и кнопка «Проверить окружение»
//! показывали один и тот же отчёт (D-01). Старт блокирует только `fail`, `warn` старт не
//! блокирует (D-02). Каждая ветка оценки — чистая функция от фактов, поэтому тестируется без ОС.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::record::failure::{FailCode, RecordFailure};
use crate::record::machine::RecordState;

/// Уровень пункта предпроверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// Один пункт отчёта: что проверяли, чем кончилось и что делать.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Идентификатор из [`ids`].
    pub id: String,
    pub level: Level,
    pub message: String,
    /// Подсказка по-русски; у `Ok` пустая.
    pub hint: String,
}

impl Item {
    fn ok(id: &str, message: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Ok,
            message: message.into(),
            hint: String::new(),
        }
    }

    fn warn(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Warn,
            message: message.into(),
            hint: hint.into(),
        }
    }

    fn fail(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Fail,
            message: message.into(),
            hint: hint.into(),
        }
    }
}

/// Отчёт предпроверки: пункты, сборка CS2 и версия HLAE на момент проверки.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Report {
    pub items: Vec<Item>,
    pub cs2_build: Option<String>,
    pub hlae_version: Option<String>,
}

impl Report {
    /// Худший уровень среди пунктов; пустой отчёт — `Ok`.
    pub fn worst(&self) -> Level {
        self.items
            .iter()
            .map(|i| i.level)
            .max()
            .unwrap_or(Level::Ok)
    }

    /// Блокирует ли отчёт старт записи: только `fail` (D-02).
    pub fn blocks(&self) -> bool {
        self.worst() == Level::Fail
    }

    /// Заменить пункты с теми же `id`, новые дописать в конец (второй проход оценки места).
    pub fn replace(&mut self, items: Vec<Item>) {
        for new in items {
            match self.items.iter_mut().find(|i| i.id == new.id) {
                Some(old) => *old = new,
                None => self.items.push(new),
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<Report> {
        serde_json::from_str(s).ok()
    }

    /// Сбой записи на ступени «предпроверка»; `None`, если проваленных пунктов нет.
    pub fn failure(&self, elapsed: Duration) -> Option<RecordFailure> {
        let failed: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.level == Level::Fail)
            .collect();
        let first = failed.first()?;
        let has = |id: &str| failed.iter().any(|i| i.id == id);
        let code = if has(ids::PREV_GAME) {
            FailCode::PrevGameStuck
        } else if has(ids::FOREIGN_GAME) {
            FailCode::ForeignGameRunning
        } else {
            FailCode::PrecheckFailed
        };
        let message = format!(
            "предпроверка: {}",
            failed
                .iter()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
        let mut f = RecordFailure::new(code, RecordState::Precheck, message, elapsed);
        if !first.hint.is_empty() {
            f.hint = first.hint.clone();
        }
        Some(f)
    }
}

/// Идентификаторы пунктов отчёта.
pub mod ids {
    pub const PREV_GAME: &str = "prev_game";
    pub const FOREIGN_GAME: &str = "foreign_game";
    pub const STEAM: &str = "steam";
    pub const STEAM_UPDATE: &str = "steam_update";
    pub const CS2_BUILD: &str = "cs2_build";
    pub const HLAE: &str = "hlae";
    pub const HOOK: &str = "hook";
    pub const CS2_EXE: &str = "cs2_exe";
    pub const FFMPEG: &str = "ffmpeg";
    pub const FFPROBE: &str = "ffprobe";
    pub const HLAE_FFMPEG: &str = "hlae_ffmpeg";
    pub const DEMO: &str = "demo";
    pub const OUT_DIR: &str = "out_dir";
    pub const SPACE_RESULTS: &str = "space_results";
    pub const SPACE_CS2: &str = "space_cs2";
}

/// Что известно о своей игре от прошлой задачи (D-14).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum PrevGameFact {
    /// Не проверялось: пункта в отчёте не будет.
    #[default]
    Unchecked,
    /// Прошлой игры нет или она уже закрыта.
    Clear,
    /// Игра закрылась после ожидания.
    Waited(Duration),
    /// Игра не закрылась за срок; `confirmed_alive` — выход проверен и игра жива.
    Stuck {
        pid: u32,
        secs: u64,
        confirmed_alive: bool,
    },
}

/// Факты окружения. Значение по умолчанию — «не проверялось»: такой пункт в отчёт не попадает.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub prev_game: PrevGameFact,
    /// Процессы системы: PID и имя образа; ошибка — список не получен.
    pub processes: Option<Result<Vec<(u32, String)>, String>>,
}

const STEAM_HINT: &str = "Запустите Steam и войдите в аккаунт: без него CS2 не стартует.";

/// Оценить факты и собрать отчёт.
pub fn evaluate(f: &Facts) -> Report {
    let mut items = vec![];

    let own_pid = match &f.prev_game {
        PrevGameFact::Unchecked => None,
        PrevGameFact::Clear => {
            items.push(Item::ok(ids::PREV_GAME, "игра прошлой задачи закрыта"));
            None
        }
        PrevGameFact::Waited(d) => {
            items.push(Item::ok(
                ids::PREV_GAME,
                format!(
                    "игра прошлой задачи закрылась, ждал {} с",
                    d.as_secs_f64().round() as u64
                ),
            ));
            None
        }
        PrevGameFact::Stuck {
            pid,
            secs,
            confirmed_alive,
        } => {
            let tail = if *confirmed_alive {
                format!("не закрылась за {secs} с после завершения своего job")
            } else {
                format!(
                    "не закрылась за {secs} с после завершения своего job, выход не удалось подтвердить"
                )
            };
            items.push(Item::fail(
                ids::PREV_GAME,
                format!("игра прошлой задачи (cs2.exe, PID {pid}) {tail}"),
                FailCode::PrevGameStuck.hint(),
            ));
            Some(*pid)
        }
    };

    match &f.processes {
        None => {}
        Some(Err(e)) => {
            let hint = "Проверка чужого CS2 и Steam пропущена: закройте лишнее вручную.";
            items.push(Item::warn(
                ids::FOREIGN_GAME,
                format!("не удалось получить список процессов: {e}"),
                hint,
            ));
            items.push(Item::warn(
                ids::STEAM,
                format!("не удалось получить список процессов: {e}"),
                hint,
            ));
        }
        Some(Ok(list)) => {
            let foreign: Vec<u32> = list
                .iter()
                .filter(|(pid, name)| name.eq_ignore_ascii_case("cs2.exe") && Some(*pid) != own_pid)
                .map(|(pid, _)| *pid)
                .collect();
            if foreign.is_empty() {
                items.push(Item::ok(ids::FOREIGN_GAME, "чужой CS2 не запущен"));
            } else {
                let pids = foreign
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                items.push(Item::fail(
                    ids::FOREIGN_GAME,
                    format!("запущен CS2, который приложение не запускало (PID {pids})"),
                    FailCode::ForeignGameRunning.hint(),
                ));
            }
            if list
                .iter()
                .any(|(_, name)| name.eq_ignore_ascii_case("steam.exe"))
            {
                items.push(Item::ok(ids::STEAM, "Steam запущен"));
            } else {
                items.push(Item::fail(ids::STEAM, "Steam не запущен", STEAM_HINT));
            }
        }
    }

    Report {
        items,
        cs2_build: None,
        hlae_version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(list: &[(u32, &str)]) -> Option<Result<Vec<(u32, String)>, String>> {
        Some(Ok(list
            .iter()
            .map(|(p, n)| (*p, (*n).to_owned()))
            .collect()))
    }

    fn item<'a>(r: &'a Report, id: &str) -> &'a Item {
        r.items
            .iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("нет пункта {id}"))
    }

    #[test]
    fn foreign_cs2_blocks_with_foreign_game_running() {
        let r = evaluate(&Facts {
            processes: procs(&[(10, "steam.exe"), (77, "CS2.EXE")]),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::FOREIGN_GAME).level, Level::Fail);
        assert!(item(&r, ids::FOREIGN_GAME).message.contains("PID 77"));
        let f = r.failure(Duration::ZERO).expect("есть провал");
        assert_eq!(f.code, FailCode::ForeignGameRunning);
        assert_eq!(f.state, RecordState::Precheck);
        assert!(f.message.starts_with("предпроверка: "));
        assert!(f.hint.contains("Закройте CS2"));
    }

    #[test]
    fn own_stuck_game_is_prev_game_stuck_not_foreign() {
        let r = evaluate(&Facts {
            prev_game: PrevGameFact::Stuck {
                pid: 500,
                secs: 30,
                confirmed_alive: true,
            },
            processes: procs(&[(1, "Steam.exe"), (500, "cs2.exe")]),
        });
        assert_eq!(item(&r, ids::PREV_GAME).level, Level::Fail);
        assert_eq!(
            item(&r, ids::FOREIGN_GAME).level,
            Level::Ok,
            "свой PID чужим не считается"
        );
        assert_eq!(
            r.failure(Duration::ZERO).unwrap().code,
            FailCode::PrevGameStuck
        );
        let unsure = evaluate(&Facts {
            prev_game: PrevGameFact::Stuck {
                pid: 500,
                secs: 30,
                confirmed_alive: false,
            },
            ..Facts::default()
        });
        let msg = &item(&unsure, ids::PREV_GAME).message;
        assert!(msg.contains("выход не удалось подтвердить"));
        assert!(!msg.contains("жива"));
    }

    #[test]
    fn missing_steam_fails_and_an_unknown_process_list_only_warns() {
        let r = evaluate(&Facts {
            processes: procs(&[(5, "explorer.exe")]),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::STEAM).level, Level::Fail);
        assert!(item(&r, ids::STEAM).hint.contains("Запустите Steam"));
        assert_eq!(
            r.failure(Duration::ZERO).unwrap().code,
            FailCode::PrecheckFailed
        );
        let r = evaluate(&Facts {
            processes: Some(Err("отказано".to_owned())),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::STEAM).level, Level::Warn);
        assert_eq!(item(&r, ids::FOREIGN_GAME).level, Level::Warn);
        assert!(!r.blocks());
    }

    #[test]
    fn warnings_do_not_block_only_failures_do() {
        let mut r = Report::default();
        assert_eq!(r.worst(), Level::Ok);
        assert!(!r.blocks());
        assert!(r.failure(Duration::ZERO).is_none());
        r.items.push(Item::warn("a", "предупреждение", "подсказка"));
        assert_eq!(r.worst(), Level::Warn);
        assert!(!r.blocks());
        assert!(r.failure(Duration::ZERO).is_none());
        r.items.push(Item::fail("b", "провал", ""));
        assert!(r.blocks());
        let f = r.failure(Duration::ZERO).unwrap();
        assert_eq!(
            f.hint,
            FailCode::PrecheckFailed.hint(),
            "пустая подсказка пункта -> подсказка кода"
        );
    }

    #[test]
    fn report_survives_a_json_round_trip() {
        let mut r = evaluate(&Facts {
            processes: procs(&[(1, "steam.exe")]),
            ..Facts::default()
        });
        r.cs2_build = Some("25815307".to_owned());
        r.hlae_version = Some("2.192.7".to_owned());
        let back = Report::from_json(&r.to_json()).expect("разбирается");
        assert_eq!(back, r);
        assert!(Report::from_json("не json").is_none());
    }

    #[test]
    fn replace_swaps_items_by_id() {
        let mut r = Report {
            items: vec![Item::ok("a", "1"), Item::ok("b", "2")],
            ..Report::default()
        };
        r.replace(vec![Item::fail("a", "новый", "x"), Item::ok("c", "3")]);
        assert_eq!(r.items.len(), 3);
        assert_eq!(r.items[0].message, "новый");
        assert_eq!(r.items[0].level, Level::Fail);
        assert_eq!(r.items[2].id, "c");
    }
}
