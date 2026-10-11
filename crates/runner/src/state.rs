//! Outbox раннера: одна удерживаемая задача в `runner_state.json`.
//!
//! Файл пишется атомарно: временный файл → `sync_all` → `rename`; на Windows `rename` заменяет существующий
//! файл (проверено спайком), поэтому читатель видит либо старое состояние, либо новое, но не половину.
//! Результат остаётся на месте (`out/srv{id}/highlights.mp4`); здесь лежит только запись о нём, пока сервер не
//! подтвердил размер и SHA-256. Поле `version` оставляет путь к миграции формата.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use wire::{Assignment, Outcome};

/// Имя файла состояния в папке раннера.
pub const STATE_FILE: &str = "runner_state.json";
/// Временный файл атомарной записи.
const TMP_FILE: &str = "runner_state.json.tmp";
/// Текущая версия формата файла.
pub const VERSION: u32 = 1;

/// Этап удерживаемой задачи; после перезапуска раннер продолжает с него.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Задача выдана, демка ещё не скачана целиком.
    Claimed,
    /// Демка скачана и проверена.
    Fetched,
    /// Исполнитель работает (после перезапуска идёт с `resume`).
    Executing,
    /// Исполнитель отдал результат, сервер его ещё не подтвердил.
    Finished,
    /// Итог (`Failed`/`Cancelled`) известен, сервер о нём ещё не знает.
    Reporting,
}

/// Результат исполнителя, ждущий подтверждения сервера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultFile {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// Задача, которую раннер держит: всё, что нужно, чтобы продолжить после перезапуска.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldJob {
    pub server_job_id: i64,
    pub epoch: i64,
    pub stage: Stage,
    pub demo_path: PathBuf,
    pub demo_size: u64,
    pub demo_sha256: String,
    pub faceit: Option<String>,
    pub hero_steam: u64,
    pub hero_name: String,
    /// Папка результата: известна, когда исполнитель уже начал работу.
    pub out_dir: Option<PathBuf>,
    /// Сколько раз запускался исполнитель.
    pub attempt: u32,
    pub result: Option<ResultFile>,
    /// Итог, который нужно сообщить серверу (этап `Reporting`).
    pub pending: Option<Outcome>,
    /// Сколько раз сервер отверг SHA-256 результата.
    pub sha_retries: u32,
}

impl HeldJob {
    /// Только что выданная задача: демки ещё нет.
    pub fn claimed(a: &Assignment, demo_path: PathBuf) -> HeldJob {
        HeldJob {
            server_job_id: a.job_id,
            epoch: a.epoch,
            stage: Stage::Claimed,
            demo_path,
            demo_size: a.demo.size,
            demo_sha256: a.demo.sha256.clone(),
            faceit: a.faceit_link.clone(),
            hero_steam: a
                .player
                .steam64
                .as_deref()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            hero_name: a.player.name.clone(),
            out_dir: None,
            attempt: 0,
            result: None,
            pending: None,
            sha_retries: 0,
        }
    }
}

/// Содержимое `runner_state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerState {
    pub version: u32,
    pub job: Option<HeldJob>,
    /// Последний выданный `seq` прогресса.
    pub last_seq: u64,
}

impl Default for RunnerState {
    fn default() -> Self {
        RunnerState {
            version: VERSION,
            job: None,
            last_seq: 0,
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RunnerState {
    /// Читает состояние из папки раннера. Второе значение — предупреждение для журнала.
    /// Нет файла — пустое состояние без предупреждения.
    pub fn load(dir: &Path) -> (RunnerState, Option<String>) {
        let path = dir.join(STATE_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (RunnerState::default(), None);
            }
            Err(e) => {
                return (
                    RunnerState::default(),
                    Some(format!("{} не прочитан: {e}", path.display())),
                );
            }
        };
        match serde_json::from_str::<RunnerState>(&text) {
            Ok(s) if s.version == VERSION => (s, None),
            Ok(s) => (
                RunnerState::default(),
                Some(format!(
                    "{}: версия формата {} вместо {VERSION}",
                    path.display(),
                    s.version
                )),
            ),
            Err(e) => (
                RunnerState::default(),
                Some(format!("{} не разобран: {e}", path.display())),
            ),
        }
    }

    /// Записывает состояние атомарно: временный файл → `sync_all` → `rename`.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let tmp = dir.join(TMP_FILE);
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, dir.join(STATE_FILE))
    }

    /// Следующий `seq` прогресса: `max(предыдущий + 1, unix-мс)`. Строго растёт даже при откате часов и после
    /// перезапуска (значение хранится в файле); потолок — `i64::MAX`, потому что сервер хранит его в SQLite.
    pub fn next_seq(&mut self) -> u64 {
        let next = (self.last_seq + 1).max(unix_ms()).min(i64::MAX as u64);
        self.last_seq = next;
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> RunnerState {
        RunnerState {
            version: VERSION,
            job: Some(HeldJob {
                server_job_id: 7,
                epoch: 3,
                stage: Stage::Finished,
                demo_path: PathBuf::from("remote/srv7.dem"),
                demo_size: 10,
                demo_sha256: "ab".repeat(32),
                faceit: Some("https://www.faceit.com/ru/cs2/room/1-x".to_owned()),
                hero_steam: 76561198000000000,
                hero_name: "s1mple".to_owned(),
                out_dir: Some(PathBuf::from("out/srv7")),
                attempt: 2,
                result: Some(ResultFile {
                    path: PathBuf::from("out/srv7/highlights.mp4"),
                    size: 20,
                    sha256: "cd".repeat(32),
                }),
                pending: None,
                sha_retries: 1,
            }),
            last_seq: 42,
        }
    }

    #[test]
    fn state_file_round_trips_through_an_atomic_save() {
        let dir = temp("roundtrip");
        let state = sample();
        state.save(&dir).unwrap();
        let (loaded, warn) = RunnerState::load(&dir);
        assert_eq!(loaded, state);
        assert!(warn.is_none(), "{warn:?}");

        // Повторная запись поверх существующего файла не оставляет временных файлов.
        let mut second = state.clone();
        second.last_seq = 43;
        second.save(&dir).unwrap();
        assert_eq!(RunnerState::load(&dir).0, second);
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![STATE_FILE.to_owned()], "{names:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_state_file_is_an_empty_state_without_warning() {
        let dir = temp("missing");
        let (state, warn) = RunnerState::load(&dir);
        assert_eq!(state, RunnerState::default());
        assert!(warn.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_seq_grows_strictly_and_is_at_least_unix_ms() {
        let mut s = RunnerState::default();
        let before = unix_ms();
        let a = s.next_seq();
        let b = s.next_seq();
        let c = s.next_seq();
        assert!(a >= before, "{a} < {before}");
        assert!(a < b && b < c, "{a} {b} {c}");
        assert!(c <= i64::MAX as u64);

        // Часы ушли назад (или состояние пришло из будущего): последовательность всё равно строго растёт.
        let mut ahead = RunnerState {
            last_seq: unix_ms() + 1_000_000,
            ..Default::default()
        };
        let first = ahead.last_seq;
        assert_eq!(ahead.next_seq(), first + 1);

        // Потолок i64::MAX.
        let mut top = RunnerState {
            last_seq: i64::MAX as u64,
            ..Default::default()
        };
        assert_eq!(top.next_seq(), i64::MAX as u64);
    }
}
