//! `processes.json`: свои процессы сессии в папке задачи, пока игра может быть жива.
//!
//! Если приложение упало посреди записи, `HLAE.exe` и `cs2.exe` остаются в системе. По этому файлу
//! запуск после падения находит осиротевшую игру и завершает её (D-15). Файл лежит в папке задачи и
//! недоверенный: его мог подменить кто угодно, поэтому читается строго (версия, число записей,
//! ненулевой PID, образ только из [`host::ORPHAN_IMAGES`]), а завершение само по себе дополнительно
//! сверяет время создания и образ на одном handle (T-03-12).

use std::path::{Path, PathBuf};

use host::{ORPHAN_IMAGES, ProcIdentity};
use serde::{Deserialize, Serialize};

use crate::manifest::{atomic_write, read_limited};

/// Имя файла в папке задачи.
pub const PROCESSES_FILE: &str = "processes.json";

/// Версия формата файла.
const VERSION: u32 = 1;

/// Больше записей у сессии не бывает: лаунчер и игра.
const MAX_ENTRIES: usize = 8;

#[derive(Serialize, Deserialize)]
struct Entry {
    pid: u32,
    /// Время создания процесса (FILETIME).
    created: u64,
    image: String,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    processes: Vec<Entry>,
}

fn path(out_dir: &Path) -> PathBuf {
    out_dir.join(PROCESSES_FILE)
}

/// Записать идентичности процессов атомарно (временный файл и `rename`).
pub fn save(out_dir: &Path, list: &[ProcIdentity]) -> Result<(), String> {
    let file = File {
        version: VERSION,
        processes: list
            .iter()
            .map(|p| Entry {
                pid: p.pid,
                created: p.created,
                image: p.image.clone(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    atomic_write(&path(out_dir), &text)
}

/// Прочитать идентичности. Нет файла — `Ok(None)`; мусор, чужая версия, больше 8 записей, нулевой
/// PID или образ вне [`ORPHAN_IMAGES`] — `Err` с причиной по-русски: такому файлу верить нельзя.
pub fn load(out_dir: &Path) -> Result<Option<Vec<ProcIdentity>>, String> {
    let p = path(out_dir);
    if !p.exists() {
        return Ok(None);
    }
    let text = read_limited(&p)?;
    let file: File = serde_json::from_str(&text).map_err(|e| format!("{PROCESSES_FILE}: {e}"))?;
    if file.version != VERSION {
        return Err(format!(
            "{PROCESSES_FILE}: неизвестная версия {}",
            file.version
        ));
    }
    if file.processes.len() > MAX_ENTRIES {
        return Err(format!(
            "{PROCESSES_FILE}: слишком много записей ({})",
            file.processes.len()
        ));
    }
    let mut out = Vec::with_capacity(file.processes.len());
    for e in file.processes {
        if e.pid == 0 {
            return Err(format!("{PROCESSES_FILE}: нулевой PID"));
        }
        if !ORPHAN_IMAGES
            .iter()
            .any(|i| i.eq_ignore_ascii_case(&e.image))
        {
            return Err(format!(
                "{PROCESSES_FILE}: образ «{}» не из списка своих процессов",
                e.image
            ));
        }
        out.push(ProcIdentity {
            pid: e.pid,
            created: e.created,
            image: e.image,
        });
    }
    Ok(Some(out))
}

/// Удалить файл (игра подтверждённо вышла). Нет файла — не ошибка.
pub fn remove(out_dir: &Path) {
    let _ = std::fs::remove_file(path(out_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs2cinema-procs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn id(pid: u32, image: &str) -> ProcIdentity {
        ProcIdentity {
            pid,
            created: 77,
            image: image.to_owned(),
        }
    }

    #[test]
    fn saved_identities_round_trip_and_remove_deletes_the_file() {
        let d = dir("round");
        assert_eq!(load(&d), Ok(None), "нет файла — нет данных");
        let list = vec![id(10, "HLAE.exe"), id(11, "cs2.exe")];
        save(&d, &list).unwrap();
        assert_eq!(load(&d).unwrap(), Some(list));
        remove(&d);
        assert_eq!(load(&d), Ok(None));
        remove(&d);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn foreign_image_garbage_and_oversized_lists_are_rejected() {
        let d = dir("reject");
        let file = d.join(PROCESSES_FILE);
        for (text, need) in [
            (
                r#"{"version":1,"processes":[{"pid":5,"created":1,"image":"explorer.exe"}]}"#,
                "не из списка",
            ),
            (
                r#"{"version":1,"processes":[{"pid":0,"created":1,"image":"cs2.exe"}]}"#,
                "нулевой PID",
            ),
            (r#"{"version":2,"processes":[]}"#, "версия"),
            ("не json", PROCESSES_FILE),
        ] {
            std::fs::write(&file, text).unwrap();
            let e = load(&d).expect_err(text);
            assert!(e.contains(need), "{e}");
        }
        let many: Vec<ProcIdentity> = (1..=9).map(|n| id(n, "cs2.exe")).collect();
        save(&d, &many).unwrap();
        assert!(load(&d).unwrap_err().contains("слишком много"));
        // образ без учёта регистра
        save(&d, &[id(3, "CS2.EXE")]).unwrap();
        assert!(load(&d).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&d);
    }
}
