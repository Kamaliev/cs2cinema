//! Уборщик сервера (OPS-01): сроки хранения демок и роликов, чистка недозагруженных кусков и остатков.
//!
//! Правило уборки — двухшаговое удаление. Сначала одна транзакция `db.write` ставит строкам `files`
//! метку `deleted_at` (с этого момента ни `demo_file`, ни `result_file`, ни поиск дублей файл не видят),
//! и только после коммита файлы стираются с диска. Уборщик, прерванный между шагами, ничего не теряет:
//! следующий проход находит папки без живой строки `files` и дочищает их. Два уборщика одновременно
//! (поток `serve` и разовый `cs2-server clean`) не удаляют одну строку дважды: `UPDATE` берёт только
//! `deleted_at IS NULL`, а `NotFound` при удалении файла — не ошибка.
//!
//! Что никогда не удаляется: ролик, пока доставка в Telegram в состоянии `none`, `pending`, `sending` или
//! `uncertain`; демка задачи, которая ещё не окончательна; всё вне папки данных и вне корня bot-api; каталоги
//! и служебные файлы bot-api. Время берётся только из внедрённых часов ядра. В журнал уходят числа и номера
//! задач, но не пути: в имени рабочей папки `telegram-bot-api` лежит токен бота.

use crate::Result;
use crate::core::Core;
use crate::db::db_err;
use rusqlite::{OptionalExtension, params};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

const HOUR_MS: i64 = 3_600_000;

/// Сроки хранения в миллисекундах; умолчания — RESEARCH A1 (автор меняет их ключами настроек).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    /// Демка готовой или отменённой задачи — после `finished_at`.
    pub demo_ms: i64,
    /// Демка упавшей задачи — после `finished_at` (запас на `/retry`).
    pub failed_demo_ms: i64,
    /// Ролик, доставленный в Telegram (есть `result_file_id`) — после последнего изменения доставки.
    pub result_ms: i64,
    /// Ролик, который не ушёл в Telegram или вообще не из бота — после `finished_at`.
    pub unsent_result_ms: i64,
    /// Недозагруженные `*.part` и остатки прерванной уборки.
    pub part_ms: i64,
    /// Файлы рабочей папки `telegram-bot-api`.
    pub botapi_ms: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            demo_ms: 24 * HOUR_MS,
            failed_demo_ms: 72 * HOUR_MS,
            result_ms: 24 * HOUR_MS,
            unsent_result_ms: 168 * HOUR_MS,
            part_ms: 6 * HOUR_MS,
            botapi_ms: 6 * HOUR_MS,
        }
    }
}

/// Режим прохода: удалять или только посчитать (`cs2-server clean --dry-run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanMode {
    Apply,
    DryRun,
}

/// Итог прохода: номера задач и счётчики. Путей здесь нет намеренно.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// Задачи, чья демка удалена (в `DryRun` — была бы удалена).
    pub demos: Vec<i64>,
    /// Задачи, чей ролик удалён.
    pub results: Vec<i64>,
    /// Удалённые недозагруженные `*.part`.
    pub parts: usize,
    /// Дочищенные папки без живой строки `files` (остатки прерванной уборки, папки без задачи).
    pub leftovers: usize,
    /// Удалённые файлы рабочей папки `telegram-bot-api`.
    pub botapi_files: usize,
    /// Освобождено байт.
    pub freed_bytes: u64,
    /// Файлы, которые не удалось убрать (причина в журнал не идёт: в ней могут быть пути).
    pub errors: usize,
}

impl Cleaned {
    /// Ничего не удалено и ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.demos.is_empty()
            && self.results.is_empty()
            && self.parts == 0
            && self.leftovers == 0
            && self.botapi_files == 0
            && self.errors == 0
    }

    /// Строка журнала: только числа.
    pub fn summary(&self) -> String {
        let mib = self.freed_bytes as f64 / (1024.0 * 1024.0);
        let mut s = format!(
            "демок {}, роликов {}, кусков {}, остатков {}, файлов bot-api {}, освобождено {:.1} МиБ",
            self.demos.len(),
            self.results.len(),
            self.parts,
            self.leftovers,
            self.botapi_files,
            mib
        );
        if self.errors > 0 {
            s.push_str(&format!(", ошибок {}", self.errors));
        }
        s
    }
}

/// Файл, у которого срок вышел: строка `files` уже помечена (или будет помечена в `DryRun`).
struct Expired {
    job_id: i64,
    kind: String,
    rel_path: String,
    size: u64,
}

/// Отбор просроченных файлов; общий для пометки (`UPDATE … RETURNING`) и для `DryRun` (`SELECT`).
/// Параметры: ?2 — граница демки готовой задачи, ?3 — демки упавшей, ?4 — доставленного ролика,
/// ?5 — ролика без доставки (`now` минус срок). Ролик отбирается только при `sent` с `result_file_id`,
/// `sent` без него, `failed` или без строки `tg_jobs`; при `none`, `pending`, `sending`, `uncertain` — никогда.
const EXPIRED_FILES: &str = "FROM files f \
     JOIN jobs j ON j.id = f.job_id \
     LEFT JOIN tg_jobs t ON t.job_id = j.id \
     WHERE f.deleted_at IS NULL AND ( \
        (f.kind = 'demo' AND ( \
            (j.state IN ('done','cancelled') AND j.finished_at <= ?2) \
            OR (j.state = 'failed' AND j.finished_at <= ?3))) \
        OR (f.kind = 'result' AND j.state = 'done' AND ( \
            (t.job_id IS NULL AND j.finished_at <= ?5) \
            OR (t.delivery = 'sent' AND t.result_file_id IS NOT NULL AND t.updated_at <= ?4) \
            OR (t.delivery = 'sent' AND t.result_file_id IS NULL AND j.finished_at <= ?5) \
            OR (t.delivery = 'failed' AND j.finished_at <= ?5))) \
     )";

impl Core {
    /// Один проход уборки по правилам модуля. `Err` — только ошибка базы (текст без путей); ошибки файлов
    /// считаются в `Cleaned::errors`. Порядок: пометка и удаление просроченных файлов, недозагруженные куски,
    /// остатки, рабочая папка bot-api.
    pub fn clean(
        &self,
        r: &Retention,
        botapi_root: Option<&Path>,
        mode: CleanMode,
    ) -> Result<Cleaned> {
        let now = self.clock().now_ms();
        let mut out = Cleaned::default();
        for e in self.mark_expired(r, now, mode)? {
            match mode {
                CleanMode::Apply => self.remove_expired(&e, &mut out),
                CleanMode::DryRun => out.freed_bytes += e.size,
            }
            match e.kind.as_str() {
                "demo" => out.demos.push(e.job_id),
                _ => out.results.push(e.job_id),
            }
        }
        self.clean_parts(r, now, mode, &mut out)?;
        self.clean_leftovers(r, now, mode, &mut out)?;
        if let Some(root) = botapi_root {
            clean_botapi(root, now, r.botapi_ms, mode, &mut out);
        }
        Ok(out)
    }

    /// Шаг 1: в одной транзакции помечает просроченные строки `files`; `DryRun` только читает.
    fn mark_expired(&self, r: &Retention, now: i64, mode: CleanMode) -> Result<Vec<Expired>> {
        let args = params![
            now,
            now - r.demo_ms,
            now - r.failed_demo_ms,
            now - r.result_ms,
            now - r.unsent_result_ms
        ];
        let from_row = |row: &rusqlite::Row<'_>| {
            Ok(Expired {
                job_id: row.get(0)?,
                kind: row.get(1)?,
                rel_path: row.get(2)?,
                size: row.get::<_, i64>(3)?.max(0) as u64,
            })
        };
        match mode {
            CleanMode::Apply => self.db().write(|tx| {
                let sql = format!(
                    "UPDATE files SET deleted_at = ?1 WHERE id IN (SELECT f.id {EXPIRED_FILES}) \
                     RETURNING job_id, kind, rel_path, size"
                );
                let mut st = tx.prepare(&sql).map_err(db_err)?;
                let rows = st.query_map(args, from_row).map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
            }),
            CleanMode::DryRun => self.db().read(|c| {
                let sql = format!(
                    "SELECT f.job_id, f.kind, f.rel_path, f.size {EXPIRED_FILES} ORDER BY f.id"
                );
                let mut st = c.prepare(&sql).map_err(db_err)?;
                let rows = st.query_map(args, from_row).map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
            }),
        }
    }

    /// Шаг 2: стирает файл помеченной строки и опустевшую папку задачи. Путь строится только из `rel_path`
    /// строки и папки данных и обязан остаться внутри `blobs`.
    fn remove_expired(&self, e: &Expired, out: &mut Cleaned) {
        let path = self.data_dir().join(&e.rel_path);
        if !is_inside(&path, &self.data_dir().join("blobs")) {
            out.errors += 1;
            return;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => out.freed_bytes += e.size,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                out.errors += 1;
                return;
            }
        }
        if let Some(dir) = path.parent() {
            // Папка задачи, если опустела; непустая или уже убранная — не ошибка.
            let _ = std::fs::remove_dir(dir);
        }
    }

    /// Состояние и эпоха задачи; `None` — строки нет.
    fn job_state(&self, id: i64) -> Result<Option<(String, i64)>> {
        self.db().read(|c| -> Result<Option<(String, i64)>> {
            c.query_row("SELECT state, epoch FROM jobs WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(db_err)
        })
    }

    /// Есть ли у задачи живая (не помеченная) строка `files` этого вида.
    fn has_live_file(&self, id: i64, kind: &str) -> Result<bool> {
        self.db().read(|c| -> Result<bool> {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE job_id = ?1 AND kind = ?2 AND deleted_at IS NULL)",
                params![id, kind],
                |r| r.get(0),
            )
            .map_err(db_err)
        })
    }

    /// Недозагруженные куски: `results/<id>/<эпоха>.part` и `incoming/<id>/demo.part`. Кусок идущей загрузки
    /// (занята `Blobs::lock_upload`) и свежий кусок текущей эпохи живой задачи остаются.
    fn clean_parts(
        &self,
        r: &Retention,
        now: i64,
        mode: CleanMode,
        out: &mut Cleaned,
    ) -> Result<()> {
        let blobs = self.data_dir().join("blobs");
        for (id, dir) in numeric_dirs(&blobs.join("results")) {
            let job = self.job_state(id)?;
            for (path, epoch) in part_files(&dir) {
                let Some(meta) = regular_meta(&path) else {
                    continue;
                };
                let stale = is_old(&meta, now, r.part_ms);
                let remove = match &job {
                    None => true,
                    Some((state, _)) if is_terminal(state) => true,
                    Some((_, current)) => epoch != *current || stale,
                };
                if !remove {
                    continue;
                }
                // Идущая загрузка держит охрану: её кусок не трогаем, а охрану держим на время удаления.
                let Some(_guard) = self.blobs().lock_upload(id, epoch) else {
                    continue;
                };
                match mode {
                    CleanMode::Apply => match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(_) => {
                            out.errors += 1;
                            continue;
                        }
                    },
                    CleanMode::DryRun => {}
                }
                out.parts += 1;
                out.freed_bytes += meta.len();
            }
        }
        for (id, dir) in numeric_dirs(&blobs.join("incoming")) {
            let path = dir.join("demo.part");
            let Some(meta) = regular_meta(&path) else {
                continue;
            };
            if !is_old(&meta, now, r.part_ms) {
                continue;
            }
            let receiving = matches!(self.job_state(id)?, Some((state, _)) if state == "receiving");
            if receiving {
                continue;
            }
            if mode == CleanMode::Apply {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        out.errors += 1;
                        continue;
                    }
                }
            }
            out.parts += 1;
            out.freed_bytes += meta.len();
        }
        Ok(())
    }

    /// Остатки: папки `incoming/<id>` и `results/<id>` без живой строки `files` своего вида, у задач, которых
    /// нет или которые окончательны, с содержимым старше `part_ms`. Так добираются остатки прерванной уборки.
    fn clean_leftovers(
        &self,
        r: &Retention,
        now: i64,
        mode: CleanMode,
        out: &mut Cleaned,
    ) -> Result<()> {
        let blobs = self.data_dir().join("blobs");
        for (sub, kind) in [("incoming", "demo"), ("results", "result")] {
            for (id, dir) in numeric_dirs(&blobs.join(sub)) {
                if let Some((state, _)) = self.job_state(id)?
                    && !is_terminal(&state)
                {
                    continue;
                }
                if self.has_live_file(id, kind)? {
                    continue;
                }
                let Some((newest_ms, bytes)) = dir_stats(&dir) else {
                    out.errors += 1;
                    continue;
                };
                if now - newest_ms < r.part_ms {
                    continue;
                }
                if mode == CleanMode::Apply {
                    match std::fs::remove_dir_all(&dir) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(_) => {
                            out.errors += 1;
                            continue;
                        }
                    }
                }
                out.leftovers += 1;
                out.freed_bytes += bytes;
            }
        }
        Ok(())
    }
}

/// Задача окончательна: дальше её файлы никто не пишет.
fn is_terminal(state: &str) -> bool {
    matches!(state, "done" | "failed" | "cancelled")
}

/// Путь лежит внутри `root` по составным частям и не содержит `..`: строковая проверка до обращения к диску.
fn is_inside(path: &Path, root: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Подпапки `root` с именем из одних цифр (номер задачи); ссылки и обычные файлы пропускаются.
fn numeric_dirs(root: &Path) -> Vec<(i64, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        // `DirEntry::file_type` не идёт по ссылкам: ссылка на каталог не каталог.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(id) = name.parse::<i64>() {
            out.push((id, entry.path()));
        }
    }
    out.sort();
    out
}

/// Файлы `<эпоха>.part` в папке результата задачи.
fn part_files(dir: &Path) -> Vec<(PathBuf, i64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "part") {
                return None;
            }
            let epoch = path.file_stem()?.to_str()?.parse::<i64>().ok()?;
            Some((path, epoch))
        })
        .collect()
}

/// Метаданные обычного файла (не ссылки); иначе `None`.
fn regular_meta(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_file())
}

/// Время изменения в unix-миллисекундах; `None`, если система его не сообщает.
fn mtime_ms(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(d.as_millis() as i64)
}

/// Файл не менялся не меньше `age_ms`; время из будущего или неизвестное — не старое.
fn is_old(meta: &std::fs::Metadata, now: i64, age_ms: i64) -> bool {
    mtime_ms(meta).is_some_and(|m| now - m >= age_ms)
}

/// Самое позднее время изменения и общий размер обычных файлов в папке (рекурсивно, ссылки не разворачиваются).
/// Пустая папка — её собственное время. `None` — папку прочитать не удалось.
fn dir_stats(dir: &Path) -> Option<(i64, u64)> {
    fn walk(dir: &Path, depth: u32, newest: &mut Option<i64>, bytes: &mut u64) -> Option<()> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let meta = std::fs::symlink_metadata(entry.path()).ok()?;
            if meta.file_type().is_dir() {
                if depth < 4 {
                    walk(&entry.path(), depth + 1, newest, bytes)?;
                }
                continue;
            }
            *bytes += meta.len();
            if let Some(m) = mtime_ms(&meta) {
                *newest = Some(newest.map_or(m, |n| n.max(m)));
            }
        }
        Some(())
    }
    let (mut newest, mut bytes) = (None, 0);
    walk(dir, 0, &mut newest, &mut bytes)?;
    let newest = match newest {
        Some(n) => n,
        None => mtime_ms(&std::fs::symlink_metadata(dir).ok()?)?,
    };
    Some((newest, bytes))
}

/// Самый глубокий уровень файла от корня bot-api: `<корень>/<папка бота>/documents/<файл>`.
const BOTAPI_MAX_DEPTH: u32 = 3;

/// Рабочая папка `telegram-bot-api`: удаляет старые обычные файлы в подпапках `documents` и `videos` на глубине
/// не больше трёх уровней. Каталоги, `td.binlog`, базы и ссылки не трогаются; каждый кандидат после
/// `canonicalize` обязан лежать внутри канонического корня. Пути и имена (в имени папки бота токен) нигде не
/// печатаются: ошибки — только счётчик. Корня нет — проход пропускается.
fn clean_botapi(root: &Path, now: i64, botapi_ms: i64, mode: CleanMode, out: &mut Cleaned) {
    let Ok(root) = std::fs::canonicalize(root) else {
        return;
    };
    botapi_walk(&root, &root, 0, now, botapi_ms, mode, out);
}

fn botapi_walk(
    root: &Path,
    dir: &Path,
    depth: u32,
    now: i64,
    botapi_ms: i64,
    mode: CleanMode,
    out: &mut Cleaned,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        out.errors += 1;
        return;
    };
    let in_media_dir = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "documents" || n == "videos");
    for entry in entries.flatten() {
        let path = entry.path();
        // `symlink_metadata`, не `metadata`: ссылки не разворачиваются ни для файлов, ни для каталогов.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            out.errors += 1;
            continue;
        };
        let kind = meta.file_type();
        if kind.is_dir() {
            if depth + 1 < BOTAPI_MAX_DEPTH {
                botapi_walk(root, &path, depth + 1, now, botapi_ms, mode, out);
            }
            continue;
        }
        if !kind.is_file() || !in_media_dir || !is_old(&meta, now, botapi_ms) {
            continue;
        }
        match std::fs::canonicalize(&path) {
            Ok(real) if real.starts_with(root) => {}
            Ok(_) => continue,
            Err(_) => {
                out.errors += 1;
                continue;
            }
        }
        if mode == CleanMode::Apply {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    out.errors += 1;
                    continue;
                }
            }
        }
        out.botapi_files += 1;
        out.freed_bytes += meta.len();
    }
}
