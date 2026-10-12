//! Уборка рабочей папки раннера: сиротские папки сырых дублей, скачанные демки и разросшийся журнал.
//!
//! Раннер без удерживаемой задачи раз в `SWEEP_EVERY` обходит `out/` и `remote/` своей рабочей папки и удаляет то,
//! что от задач осталось и больше никому не нужно. Папка дублей не удаляется при сбое (она улика), при отмене и при
//! броске задачи, поэтому без уборки автономный раннер за дни заполнил бы диск. Решает возраст, а не догадка:
//! свежее не трогается никогда, а старое уходит по сроку своего вида.
//!
//! Никогда не убирается: удерживаемая задача, имена не вида `srv{число}`, ссылки, всё вне `out/` и `remote/`
//! рабочей папки и сама рабочая папка. Возраст папки — самое позднее изменение обычных файлов внутри: время
//! изменения каталога на Windows меняется непредсказуемо.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Папка дублей без подтверждения сервера старше этого срока удаляется. Срок совпадает со сроком упавшей демки на
/// сервере: пока друг может нажать «Повторить», улики сбоя у автора целы.
pub const ORPHAN_OUT_AGE: Duration = Duration::from_secs(72 * 3600);
/// Скачанная демка, её недокачанная часть и временная папка FACEIT старше этого срока удаляются: демку всегда
/// можно скачать с сервера заново.
pub const ORPHAN_REMOTE_AGE: Duration = Duration::from_secs(24 * 3600);
/// `runner.log` больше этого размера переименовывается в `runner.log.1`.
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Как часто раннер на простое проходит уборку.
pub const SWEEP_EVERY: Duration = Duration::from_secs(600);

/// Глубина обхода папки задачи при оценке возраста: дальше раннер не спускается.
const MAX_DEPTH: u32 = 6;

/// Итог прохода уборки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swept {
    /// Номера задач удалённых папок дублей (по записи на папку).
    pub out_dirs: Vec<i64>,
    /// Сколько скачанных демок, их частей и папок FACEIT удалено.
    pub remote_items: usize,
    /// Сколько байт файлов освобождено.
    pub freed_bytes: u64,
    /// Сколько удалений не удалось.
    pub errors: usize,
}

impl Swept {
    /// Ничего не удалено и ничего не сломалось: в журнал писать нечего.
    pub fn is_empty(&self) -> bool {
        self.out_dirs.is_empty()
            && self.remote_items == 0
            && self.freed_bytes == 0
            && self.errors == 0
    }

    /// Строка итога для `runner.log`: только числа и номера задач, без путей.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if !self.out_dirs.is_empty() {
            parts.push(format!(
                "удалено папок дублей {} (задачи {})",
                self.out_dirs.len(),
                ids(&self.out_dirs)
            ));
        }
        if self.remote_items > 0 {
            parts.push(format!("демок и частей {}", self.remote_items));
        }
        let mib = self.freed_bytes as f64 / (1024.0 * 1024.0);
        parts.push(format!("освобождено {mib:.1} МиБ").replace('.', ","));
        if self.errors > 0 {
            parts.push(format!("ошибок {}", self.errors));
        }
        format!("уборка: {}", parts.join(", "))
    }
}

/// «#7, #9» — номера по возрастанию без повторов.
fn ids(list: &[i64]) -> String {
    let mut sorted = list.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Номер задачи из «srv{число}» и остаток имени после числа. Число — десятичное, без ведущих нулей: так его пишет
/// сам раннер (`srv{id}`), всё остальное — чужое имя.
fn job_number(name: &str) -> Option<(i64, &str)> {
    let rest = name.strip_prefix("srv")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let (num, tail) = rest.split_at(digits);
    let n: i64 = num.parse().ok()?;
    (n.to_string() == num).then_some((n, tail))
}

/// Папка дублей задачи: `srv{N}` (так пишут заглушки тестов) или `srv{N}_{хвост}` (так называет её `run_job`:
/// метка демки, герой).
fn out_dir_number(name: &str) -> Option<i64> {
    let (n, tail) = job_number(name)?;
    (tail.is_empty() || tail.starts_with('_')).then_some(n)
}

/// Самое позднее изменение обычных файлов под `path` и их суммарный размер; ссылки не разыменовываются.
#[derive(Default)]
struct Walk {
    newest: Option<SystemTime>,
    bytes: u64,
}

fn walk(path: &Path, depth: u32, w: &mut Walk) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_file() {
            w.bytes += meta.len();
            if let Ok(m) = meta.modified() {
                w.newest = Some(w.newest.map_or(m, |n| n.max(m)));
            }
        } else if kind.is_dir() && depth < MAX_DEPTH {
            walk(&entry.path(), depth + 1, w);
        }
    }
}

/// Возраст каталога: от самого позднего файла внутри; в пустом каталоге — от изменения самого каталога. Время из
/// будущего (часы ушли назад) даёт нулевой возраст: такое не удаляется.
fn tree_age(path: &Path, own: Option<SystemTime>, now: SystemTime) -> (Duration, u64) {
    let mut w = Walk::default();
    walk(path, 0, &mut w);
    let stamp = w.newest.or(own);
    let age = stamp
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or(Duration::ZERO);
    (age, w.bytes)
}

/// Настоящий каталог рабочей папки (`out`, `remote`): не ссылка и не файл.
fn real_dir(dir: &Path, sub: &str) -> Option<std::path::PathBuf> {
    let path = dir.join(sub);
    let meta = fs::symlink_metadata(&path).ok()?;
    (meta.file_type().is_dir() && !meta.file_type().is_symlink()).then_some(path)
}

/// Один проход уборки по рабочей папке `dir`. `held` — номер удерживаемой задачи: её следы не трогаются.
pub fn sweep(dir: &Path, held: Option<i64>, now: SystemTime) -> Swept {
    let mut swept = Swept::default();
    sweep_out(dir, held, now, &mut swept);
    swept
}

/// Папки дублей `out/srv{N}` и `out/srv{N}_…` старше `ORPHAN_OUT_AGE`.
fn sweep_out(dir: &Path, held: Option<i64>, now: SystemTime, swept: &mut Swept) {
    let Some(out) = real_dir(dir, "out") else {
        return;
    };
    let Ok(entries) = fs::read_dir(&out) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
            continue;
        }
        let Some(number) = entry.file_name().to_str().and_then(out_dir_number) else {
            continue;
        };
        if Some(number) == held || !path.starts_with(dir) || path == dir {
            continue;
        }
        let (age, bytes) = tree_age(&path, meta.modified().ok(), now);
        if age < ORPHAN_OUT_AGE {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => {
                swept.out_dirs.push(number);
                swept.freed_bytes += bytes;
            }
            Err(_) => swept.errors += 1,
        }
    }
}
