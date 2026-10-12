//! Уборка рабочей папки раннера: сиротские папки сырых дублей, скачанные демки и разросшийся журнал.
//!
//! Раннер без удерживаемой задачи раз в `SWEEP_EVERY` обходит `out/` и `remote/` своей рабочей папки и удаляет то,
//! что от задач осталось и больше никому не нужно. Папка дублей не удаляется при сбое (она улика), при отмене и при
//! броске задачи, поэтому без уборки автономный раннер за дни заполнил бы диск. Решает возраст, а не догадка:
//! свежее не трогается никогда, а старое уходит по сроку своего вида.
//!
//! Задачи, чей результат сервер подтвердил, убираются не по возрасту, а по состоянию: раннер пишет метку
//! `out/srv{N}.accepted` до удаления дублей и снимает её, когда ничего от задачи не осталось. Метка переживает
//! обрыв (падение раннера, неудачное удаление), и ближайший проход доделывает уборку сразу, не ожидая срока.
//!
//! Никогда не убирается: удерживаемая задача, имена не вида `srv{число}`, ссылки, всё вне `out/` и `remote/`
//! рабочей папки и сама рабочая папка. Возраст папки — самое позднее изменение обычных файлов внутри: время
//! изменения каталога на Windows меняется непредсказуемо.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
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
    /// Номера задач, чьё подтверждение сервера отработано (метка `out/srv{N}.accepted` найдена).
    pub confirmed: Vec<i64>,
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
            && self.confirmed.is_empty()
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
        if !self.confirmed.is_empty() {
            parts.push(format!(
                "подтверждённых задач {} ({})",
                self.confirmed.len(),
                ids(&self.confirmed)
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
fn real_dir(dir: &Path, sub: &str) -> Option<PathBuf> {
    let path = dir.join(sub);
    let meta = fs::symlink_metadata(&path).ok()?;
    (meta.file_type().is_dir() && !meta.file_type().is_symlink()).then_some(path)
}

/// Один проход уборки по рабочей папке `dir`. `held` — номер удерживаемой задачи: её следы не трогаются.
pub fn sweep(dir: &Path, held: Option<i64>, now: SystemTime) -> Swept {
    let mut swept = Swept::default();
    // Сначала подтверждённые: их судьбу решило состояние, возраст не спрашивается.
    let marked = sweep_confirmed(dir, held, &mut swept);
    sweep_out(dir, held, &marked, now, &mut swept);
    sweep_remote(dir, held, &marked, now, &mut swept);
    swept
}

/// Путь метки подтверждения: `out/srv{id}.accepted`. Метка лежит рядом с папкой дублей, а не в ней: частичное
/// удаление папки не уносит метку раньше самих дублей.
pub fn marker_path(dir: &Path, job_id: i64) -> PathBuf {
    dir.join("out").join(format!("srv{job_id}.accepted"))
}

/// Пишет метку «результат задачи принят сервером»: до удаления дублей, чтобы обрыв не оставил подтверждённую
/// задачу без отметки. В файле нет путей и токена.
pub fn mark_accepted(dir: &Path, job_id: i64) -> std::io::Result<()> {
    fs::create_dir_all(dir.join("out"))?;
    let mut file = File::create(marker_path(dir, job_id))?;
    writeln!(file, "результат задачи #{job_id} принят сервером")?;
    file.sync_all()
}

/// Номер задачи из имени метки `srv{N}.accepted`.
fn marker_number(name: &str) -> Option<i64> {
    let (n, tail) = job_number(name)?;
    (tail == ".accepted").then_some(n)
}

fn merge(into: &mut Swept, from: Swept) {
    into.out_dirs.extend(from.out_dirs);
    into.confirmed.extend(from.confirmed);
    into.remote_items += from.remote_items;
    into.freed_bytes += from.freed_bytes;
    into.errors += from.errors;
}

/// Убирает без проверки возраста всё, что осталось от задачи `job_id`, чей результат сервер подтвердил: папки
/// `out/srv{id}` и `out/srv{id}_…`, демку, её часть и папку FACEIT в `remote/`. Метку снимает последней и только
/// когда удаление прошло без ошибок: иначе следующий проход повторит.
pub fn remove_confirmed(dir: &Path, job_id: i64) -> Swept {
    let mut swept = Swept::default();
    let marker = marker_path(dir, job_id);
    let had_marker = fs::symlink_metadata(&marker).is_ok_and(|m| m.file_type().is_file());
    if let Some(out) = real_dir(dir, "out")
        && let Ok(entries) = fs::read_dir(&out)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let named = entry.file_name().to_str().and_then(out_dir_number) == Some(job_id);
            if !named || !meta.file_type().is_dir() || !path.starts_with(dir) || path == dir {
                continue;
            }
            let (_, bytes) = tree_age(&path, None, SystemTime::now());
            match fs::remove_dir_all(&path) {
                Ok(()) => {
                    swept.out_dirs.push(job_id);
                    swept.freed_bytes += bytes;
                }
                Err(_) => swept.errors += 1,
            }
        }
    }
    if let Some(remote) = real_dir(dir, "remote")
        && let Ok(entries) = fs::read_dir(&remote)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let kind = meta.file_type();
            let Some((number, what)) = entry.file_name().to_str().and_then(remote_item) else {
                continue;
            };
            if number != job_id || kind.is_symlink() || !path.starts_with(dir) || path == dir {
                continue;
            }
            let (removed, bytes) = match what {
                Remote::File if kind.is_file() => (fs::remove_file(&path), meta.len()),
                Remote::Dir if kind.is_dir() => {
                    let (_, bytes) = tree_age(&path, None, SystemTime::now());
                    (fs::remove_dir_all(&path), bytes)
                }
                _ => continue,
            };
            match removed {
                Ok(()) => {
                    swept.remote_items += 1;
                    swept.freed_bytes += bytes;
                }
                Err(_) => swept.errors += 1,
            }
        }
    }
    if had_marker {
        swept.confirmed.push(job_id);
        if swept.errors == 0 && fs::remove_file(&marker).is_err() {
            swept.errors += 1;
        }
    }
    swept
}

/// Проход по меткам подтверждения: для каждой `remove_confirmed`. Возвращает номера, чья метка осталась (удаление
/// не удалось): проходы по возрасту их пропускают, их судьбу решило подтверждение.
fn sweep_confirmed(dir: &Path, held: Option<i64>, swept: &mut Swept) -> Vec<i64> {
    let mut numbers = Vec::new();
    if let Some(out) = real_dir(dir, "out")
        && let Ok(entries) = fs::read_dir(&out)
    {
        for entry in entries.flatten() {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            if let Some(n) = entry.file_name().to_str().and_then(marker_number)
                && Some(n) != held
            {
                numbers.push(n);
            }
        }
    }
    let mut marked = Vec::new();
    for n in numbers {
        merge(swept, remove_confirmed(dir, n));
        if fs::symlink_metadata(marker_path(dir, n)).is_ok() {
            marked.push(n);
        }
    }
    marked
}

/// Папки дублей `out/srv{N}` и `out/srv{N}_…` старше `ORPHAN_OUT_AGE`.
fn sweep_out(dir: &Path, held: Option<i64>, marked: &[i64], now: SystemTime, swept: &mut Swept) {
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
        if Some(number) == held || marked.contains(&number) || !path.starts_with(dir) || path == dir
        {
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

/// Что лежит в `remote/` под именем задачи.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Remote {
    /// `srv{N}.dem` и `srv{N}.dem.part`: обычный файл.
    File,
    /// `srv{N}.faceit`: временная папка скачивания FACEIT.
    Dir,
}

/// Номер задачи и вид следа из имени в `remote/`; всё остальное — чужое.
fn remote_item(name: &str) -> Option<(i64, Remote)> {
    let (n, tail) = job_number(name)?;
    match tail {
        ".dem" | ".dem.part" => Some((n, Remote::File)),
        ".faceit" => Some((n, Remote::Dir)),
        _ => None,
    }
}

/// Скачанные демки, их части и папки FACEIT старше `ORPHAN_REMOTE_AGE`.
fn sweep_remote(dir: &Path, held: Option<i64>, marked: &[i64], now: SystemTime, swept: &mut Swept) {
    let Some(remote) = real_dir(dir, "remote") else {
        return;
    };
    let Ok(entries) = fs::read_dir(&remote) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            continue;
        }
        let Some((number, what)) = entry.file_name().to_str().and_then(remote_item) else {
            continue;
        };
        if Some(number) == held || marked.contains(&number) || !path.starts_with(dir) || path == dir
        {
            continue;
        }
        let (age, bytes) = match what {
            Remote::File if kind.is_file() => {
                let age = meta
                    .modified()
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .unwrap_or(Duration::ZERO);
                (age, meta.len())
            }
            Remote::Dir if kind.is_dir() => tree_age(&path, meta.modified().ok(), now),
            _ => continue,
        };
        if age < ORPHAN_REMOTE_AGE {
            continue;
        }
        let removed = match what {
            Remote::File => fs::remove_file(&path),
            Remote::Dir => fs::remove_dir_all(&path),
        };
        match removed {
            Ok(()) => {
                swept.remote_items += 1;
                swept.freed_bytes += bytes;
            }
            Err(_) => swept.errors += 1,
        }
    }
}

/// Переименовывает `path` в `{имя}.1`, если файл больше `max_bytes`; прежний `.1` заменяется. `true` — файл
/// переименован (вызывающий открывает новый). Нет файла — `Ok(false)`.
pub fn rotate_log(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    let size = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if size <= max_bytes {
        return Ok(false);
    }
    let mut name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("у журнала нет имени файла"))?
        .to_os_string();
    name.push(".1");
    fs::rename(path, path.with_file_name(name))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2sweep-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rotate_log_leaves_small_files_and_replaces_the_previous_rotation() {
        let dir = temp("rotate");
        let log = dir.join("runner.log");
        assert!(
            !rotate_log(&log, 10).unwrap(),
            "нет файла — нечего ротировать"
        );

        fs::write(&log, b"0123456789").unwrap();
        assert!(
            !rotate_log(&log, 10).unwrap(),
            "ровно порог — не больше порога"
        );
        assert!(log.exists() && !dir.join("runner.log.1").exists());

        fs::write(&log, b"0123456789a").unwrap();
        fs::write(dir.join("runner.log.1"), b"prev").unwrap();
        assert!(rotate_log(&log, 10).unwrap());
        assert!(!log.exists());
        assert_eq!(fs::read(dir.join("runner.log.1")).unwrap(), b"0123456789a");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_parsed_strictly() {
        assert_eq!(out_dir_number("srv7"), Some(7));
        assert_eq!(out_dir_number("srv7_s1mple"), Some(7));
        assert_eq!(out_dir_number("srv7_"), Some(7));
        for bad in [
            "srv", "srv_7", "srvX", "srv7abc", "srv07", "srv7.dem", "xsrv7", "srv-7",
        ] {
            assert_eq!(out_dir_number(bad), None, "{bad}");
        }
        assert!(remote_item("srv9.dem") == Some((9, Remote::File)));
        assert!(remote_item("srv9.dem.part") == Some((9, Remote::File)));
        assert!(remote_item("srv9.faceit") == Some((9, Remote::Dir)));
        for bad in ["srv9", "srv9.mp4", "srv9.dem.bak", "srv.dem", "srv9_x.dem"] {
            assert!(remote_item(bad).is_none(), "{bad}");
        }
    }
}
