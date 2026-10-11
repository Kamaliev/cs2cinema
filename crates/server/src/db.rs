//! SQLite сервера: одно общее соединение, миграции через `PRAGMA user_version`.
//!
//! Мьютекс базы не держат во время работы с файлами и хеширования: сначала копируют и считают,
//! потом короткая транзакция.

use crate::Result;
use rusqlite::{Connection, TransactionBehavior};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// Общее соединение: безопасно делится между потоками.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

pub fn db_err(e: rusqlite::Error) -> String {
    format!("база данных: {e}")
}

// Версии схемы: 1 — пользователи, раннеры, задачи и файлы.
// Новая версия — только новым шагом в конце списка и только добавлением колонок или таблиц.
// Элемент k поднимает схему до версии k+1; шаг идёт в одной транзакции со своим `user_version`.
const MIGRATIONS: &[&str] = &["
CREATE TABLE users (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL UNIQUE,
    tg_id        INTEGER UNIQUE,
    role         TEXT NOT NULL CHECK(role IN ('admin','member')),
    player_steam TEXT,
    player_name  TEXT,
    created_at   INTEGER NOT NULL
);
CREATE TABLE runners (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT NOT NULL UNIQUE,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    revoked_at   INTEGER,
    last_seen_at INTEGER,
    version      TEXT,
    status       TEXT
);
CREATE TABLE jobs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id          INTEGER REFERENCES users(id),
    requested_by     TEXT NOT NULL DEFAULT '',
    state            TEXT NOT NULL CHECK(state IN ('receiving','queued','assigned','running','uploading','done','failed','cancelled')),
    cancel_requested TEXT NOT NULL DEFAULT 'none' CHECK(cancel_requested IN ('none','soft','force')),
    epoch            INTEGER NOT NULL DEFAULT 0,
    grants           INTEGER NOT NULL DEFAULT 0,
    runner_id        INTEGER REFERENCES runners(id),
    lease_until      INTEGER,
    lease_lost       INTEGER NOT NULL DEFAULT 0,
    last_seq         INTEGER NOT NULL DEFAULT 0,
    source_kind      TEXT NOT NULL CHECK(source_kind IN ('file','faceit')),
    orig_name        TEXT NOT NULL DEFAULT '',
    faceit_link      TEXT,
    player_steam     TEXT,
    player_name      TEXT NOT NULL,
    phase            TEXT NOT NULL DEFAULT '',
    take_n           INTEGER,
    take_m           INTEGER,
    fail_code        TEXT,
    fail_class       TEXT,
    fail_state       TEXT,
    fail_hint        TEXT,
    fail_message     TEXT,
    fail_tail        TEXT,
    fail_elapsed_ms  INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    finished_at      INTEGER
);
CREATE TABLE files (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id     INTEGER NOT NULL REFERENCES jobs(id),
    kind       TEXT NOT NULL CHECK(kind IN ('demo','result')),
    rel_path   TEXT NOT NULL,
    size       INTEGER NOT NULL,
    sha256     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX jobs_state_id ON jobs(state, id);
CREATE INDEX files_job ON files(job_id);
"];

/// Версия схемы, которую создаёт и ожидает эта сборка (число шагов `MIGRATIONS`).
pub fn schema_version() -> usize {
    MIGRATIONS.len()
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        Self::init(conn, &path.display().to_string())
    }

    /// База в памяти — для тестов.
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db_err)?, ":memory:")
    }

    fn init(conn: Connection, name: &str) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
        )
        .map_err(db_err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db_err)?;
        if version > schema_version() as i64 {
            return Err(format!(
                "база {name} версии {version} новее этой сборки (она знает {}): обновите cs2-server",
                schema_version()
            ));
        }
        Self::migrate(&conn, version)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Применяет ещё не применённые шаги `MIGRATIONS`; ошибка шага откатывает его транзакцию.
    fn migrate(conn: &Connection, version: i64) -> Result<()> {
        for (k, sql) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
            let tx = conn.unchecked_transaction().map_err(db_err)?;
            tx.execute_batch(sql)
                .map_err(|e| format!("миграция базы до версии {}: {e}", k + 1))?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", k + 1))
                .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
        }
        Ok(())
    }

    /// Мьютекс берётся с восстановлением: упавший обработчик не ломает следующие запросы.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Чтение: короткий запрос под мьютексом.
    pub fn read<T, E: From<String>>(
        &self,
        f: impl FnOnce(&Connection) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let conn = self.lock();
        f(&conn)
    }

    /// Запись: транзакция `BEGIN IMMEDIATE`, коммит при `Ok`, откат при `Err`.
    pub fn write<T, E: From<String>>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let mut conn = self.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| E::from(db_err(e)))?;
        let out = f(&tx)?;
        tx.commit().map_err(|e| E::from(db_err(e)))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_version(db: &Db) -> i64 {
        db.read(|c| {
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .map_err(db_err)
        })
        .unwrap()
    }

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2srv-db-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("server.db")
    }

    #[test]
    fn fresh_database_migrates_to_the_current_version() {
        let db = Db::in_memory().unwrap();
        assert_eq!(user_version(&db), schema_version() as i64);
        for table in ["users", "runners", "jobs", "files"] {
            let n: i64 = db
                .read(|c| {
                    c.query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                        [table],
                        |r| r.get(0),
                    )
                    .map_err(db_err)
                })
                .unwrap();
            assert_eq!(n, 1, "таблица {table}");
        }
    }

    #[test]
    fn reopening_changes_nothing() {
        let path = temp_db("reopen");
        let db = Db::open(&path).unwrap();
        db.write(|tx| {
            tx.execute(
                "INSERT INTO users(name, role, created_at) VALUES ('a','admin',1)",
                [],
            )
            .map(|_| ())
            .map_err(db_err)
        })
        .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(user_version(&db), schema_version() as i64);
        let n: i64 = db
            .read(|c| {
                c.query_row("SELECT count(*) FROM users", [], |r| r.get(0))
                    .map_err(db_err)
            })
            .unwrap();
        assert_eq!(n, 1, "повторное открытие ничего не меняет");
        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn newer_database_is_refused() {
        let path = temp_db("newer");
        drop(Db::open(&path).unwrap());
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!("PRAGMA user_version = {}", schema_version() + 1))
                .unwrap();
        }
        let e = Db::open(&path)
            .err()
            .expect("новая база должна отвергаться");
        assert!(e.contains("новее этой сборки"), "{e}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
