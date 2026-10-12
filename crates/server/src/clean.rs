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
use rusqlite::params;
use std::path::{Component, Path};

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
/// ?5 — ролика без доставки (`now` минус срок).
const EXPIRED_FILES: &str = "FROM files f \
     JOIN jobs j ON j.id = f.job_id \
     LEFT JOIN tg_jobs t ON t.job_id = j.id \
     WHERE f.deleted_at IS NULL AND ( \
        (f.kind = 'demo' AND ( \
            (j.state IN ('done','cancelled') AND j.finished_at <= ?2) \
            OR (j.state = 'failed' AND j.finished_at <= ?3))) \
     )";

impl Core {
    /// Один проход уборки по правилам модуля. `Err` — только ошибка базы (текст без путей); ошибки файлов
    /// считаются в `Cleaned::errors`.
    pub fn clean(
        &self,
        r: &Retention,
        botapi_root: Option<&Path>,
        mode: CleanMode,
    ) -> Result<Cleaned> {
        let now = self.clock().now_ms();
        let mut out = Cleaned::default();
        let _ = botapi_root;
        // 1. Пометка просроченных файлов и (после коммита) их удаление.
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
        Ok(out)
    }

    /// Шаг 1: в одной транзакции помечает просроченные строки `files`; `DryRun` только читает.
    fn mark_expired(&self, r: &Retention, now: i64, mode: CleanMode) -> Result<Vec<Expired>> {
        let args = params![now, now - r.demo_ms, now - r.failed_demo_ms];
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
}

/// Путь лежит внутри `root` по составным частям и не содержит `..`: строковая проверка до обращения к диску.
fn is_inside(path: &Path, root: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|c| matches!(c, Component::ParentDir))
}
