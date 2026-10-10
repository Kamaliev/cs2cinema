//! База приложения (SQLite): задачи на монтаж, кэш ответов FACEIT, реестр скачанных демок.
//!
//! Задача на монтаж — отдельная сущность: откуда демка, про кого ролик, план или запись,
//! статус и этап, журнал, результат. Задачи переживают перезапуск приложения и выполняются
//! по очереди (CS2 пишет только одну).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    /// Только план и скрипты для HLAE.
    Plan,
    /// План, запись в игре и склейка.
    Record,
}

impl JobKind {
    pub fn title(self) -> &'static str {
        match self {
            JobKind::Plan => "план",
            JobKind::Record => "ролик",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            JobKind::Plan => "plan",
            JobKind::Record => "record",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "plan" { JobKind::Plan } else { JobKind::Record }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn title(self) -> &'static str {
        match self {
            JobStatus::Queued => "в очереди",
            JobStatus::Running => "выполняется",
            JobStatus::Done => "готово",
            JobStatus::Failed => "ошибка",
            JobStatus::Cancelled => "отменена",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Failed,
        }
    }
}

/// Откуда брать демку.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobSource {
    /// Матч FACEIT (id); демка скачивается или берётся из реестра.
    Faceit(String),
    /// Файл на диске.
    File(PathBuf),
}

impl JobSource {
    fn parts(&self) -> (&'static str, String) {
        match self {
            JobSource::Faceit(id) => ("faceit", id.clone()),
            JobSource::File(p) => ("file", p.to_string_lossy().into_owned()),
        }
    }
    fn from_parts(kind: &str, value: String) -> Self {
        if kind == "faceit" { JobSource::Faceit(value) } else { JobSource::File(value.into()) }
    }
}

/// Что нужно, чтобы поставить задачу.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: JobKind,
    pub source: JobSource,
    /// Что показывать в списке: матч или имя файла.
    pub label: String,
    pub hero_name: String,
    /// SteamID64 героя из демки — чтобы найти его снова при повторе.
    pub hero_steam: u64,
    pub map: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub kind: JobKind,
    pub source: JobSource,
    pub label: String,
    pub hero_name: String,
    pub hero_steam: u64,
    pub map: String,
    pub status: JobStatus,
    /// Текущий этап конвейера (название), пока выполняется.
    pub stage: String,
    pub out_dir: Option<PathBuf>,
    /// Готовый ролик (или папка плана).
    pub result: Option<PathBuf>,
    pub error: Option<String>,
    pub shots: Option<u32>,
    /// Длина ролика по плану, секунды.
    pub secs: Option<f32>,
    pub log: String,
    /// Код сбоя записи (`CONSOLE_NOT_OPEN`…); пусто — сбоя не было или он не типизирован.
    pub fail_code: Option<String>,
    /// Код состояния записи, в котором случился сбой (`console`…).
    pub fail_state: Option<String>,
    /// Подсказка, что делать со сбоем.
    pub fail_hint: Option<String>,
    /// Хвост консоли игры на момент сбоя, строки через `\n`.
    pub fail_tail: Option<String>,
}

impl Job {
    pub fn is_active(&self) -> bool {
        matches!(self.status, JobStatus::Queued | JobStatus::Running)
    }
}

/// Итог выполненной задачи.
#[derive(Debug, Clone)]
pub struct JobOutcome {
    pub out_dir: PathBuf,
    pub result: PathBuf,
    pub shots: u32,
    pub secs: f32,
}

/// Сбой записи строками — store не знает типов pipeline (D-05).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JobFailure {
    /// Сообщение по-русски — попадает в `error`.
    pub message: String,
    pub code: String,
    pub state: String,
    pub hint: String,
    pub tail: String,
}

/// Общее соединение: одно на приложение, безопасно делится между потоками.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn err(e: rusqlite::Error) -> String {
    format!("база данных: {e}")
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source      TEXT NOT NULL,
    label       TEXT NOT NULL,
    hero_name   TEXT NOT NULL,
    hero_steam  INTEGER NOT NULL DEFAULT 0,
    map         TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL,
    stage       TEXT NOT NULL DEFAULT '',
    out_dir     TEXT,
    result      TEXT,
    error       TEXT,
    shots       INTEGER,
    secs        REAL,
    log         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS jobs_status ON jobs(status);
CREATE TABLE IF NOT EXISTS faceit_cache (
    key        TEXT PRIMARY KEY,
    json       TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS demos (
    match_id      TEXT PRIMARY KEY,
    path          TEXT NOT NULL,
    downloaded_at INTEGER NOT NULL
);
";

// Версии схемы: 1 — сбой записи (fail_*, D-05).
// Новая версия — только новым шагом в конце списка и только добавлением колонок.
// Элемент k поднимает схему до версии k+1; шаг идёт в одной транзакции со своим `user_version`.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE jobs ADD COLUMN fail_code TEXT;
     ALTER TABLE jobs ADD COLUMN fail_state TEXT;
     ALTER TABLE jobs ADD COLUMN fail_hint TEXT;
     ALTER TABLE jobs ADD COLUMN fail_tail TEXT;",
];

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let conn = Connection::open(path).map_err(err)?;
        Self::init(conn)
    }

    /// База в памяти — для тестов.
    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;").map_err(err)?;
        conn.execute_batch(SCHEMA).map_err(err)?;
        Self::migrate(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Применяет ещё не применённые шаги `MIGRATIONS`; ошибка шага откатывает его транзакцию.
    fn migrate(conn: &Connection) -> Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
        for (k, sql) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
            let tx = conn.unchecked_transaction().map_err(err)?;
            tx.execute_batch(sql).map_err(|e| format!("миграция базы до версии {}: {e}", k + 1))?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", k + 1)).map_err(err)?;
            tx.commit().map_err(err)?;
        }
        Ok(())
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.conn.lock().map_err(|_| "база данных занята".to_owned())?;
        f(&conn).map_err(err)
    }

    // ---- задачи ----

    pub fn add_job(&self, j: &NewJob) -> Result<i64> {
        let (sk, sv) = j.source.parts();
        let t = now();
        self.with(|c| {
            c.execute(
                "INSERT INTO jobs (created_at, updated_at, kind, source_kind, source, label, hero_name, hero_steam, map, status)
                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued')",
                params![t, j.kind.as_str(), sk, sv, j.label, j.hero_name, j.hero_steam as i64, j.map],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    fn row(r: &rusqlite::Row) -> rusqlite::Result<Job> {
        Ok(Job {
            id: r.get("id")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            kind: JobKind::parse(&r.get::<_, String>("kind")?),
            source: JobSource::from_parts(&r.get::<_, String>("source_kind")?, r.get("source")?),
            label: r.get("label")?,
            hero_name: r.get("hero_name")?,
            hero_steam: r.get::<_, i64>("hero_steam")? as u64,
            map: r.get("map")?,
            status: JobStatus::parse(&r.get::<_, String>("status")?),
            stage: r.get("stage")?,
            out_dir: r.get::<_, Option<String>>("out_dir")?.map(PathBuf::from),
            result: r.get::<_, Option<String>>("result")?.map(PathBuf::from),
            error: r.get("error")?,
            shots: r.get::<_, Option<i64>>("shots")?.map(|n| n as u32),
            secs: r.get::<_, Option<f64>>("secs")?.map(|s| s as f32),
            log: r.get("log")?,
            fail_code: r.get("fail_code")?,
            fail_state: r.get("fail_state")?,
            fail_hint: r.get("fail_hint")?,
            fail_tail: r.get("fail_tail")?,
        })
    }

    /// Все задачи, новые сверху.
    pub fn jobs(&self) -> Result<Vec<Job>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT * FROM jobs ORDER BY id DESC")?;
            let rows = st.query_map([], Self::row)?;
            rows.collect()
        })
    }

    pub fn job(&self, id: i64) -> Result<Option<Job>> {
        self.with(|c| c.query_row("SELECT * FROM jobs WHERE id = ?1", [id], Self::row).optional())
    }

    /// Самая старая задача в очереди.
    pub fn next_queued(&self) -> Result<Option<Job>> {
        self.with(|c| c.query_row("SELECT * FROM jobs WHERE status = 'queued' ORDER BY id LIMIT 1", [], Self::row).optional())
    }

    pub fn running(&self) -> Result<Option<Job>> {
        self.with(|c| c.query_row("SELECT * FROM jobs WHERE status = 'running' ORDER BY id LIMIT 1", [], Self::row).optional())
    }

    pub fn start_job(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET status = 'running', stage = '', error = NULL, result = NULL, log = '', fail_code = NULL, fail_state = NULL, fail_hint = NULL, fail_tail = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now()],
            )?;
            Ok(())
        })
    }

    pub fn set_stage(&self, id: i64, stage: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE jobs SET stage = ?2, updated_at = ?3 WHERE id = ?1", params![id, stage, now()])?;
            Ok(())
        })
    }

    pub fn append_log(&self, id: i64, line: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE jobs SET log = log || ?2 || char(10), updated_at = ?3 WHERE id = ?1", params![id, line, now()])?;
            Ok(())
        })
    }

    pub fn finish_job(&self, id: i64, outcome: std::result::Result<&JobOutcome, &str>) -> Result<()> {
        self.with(|c| {
            match outcome {
                Ok(o) => c.execute(
                    "UPDATE jobs SET status = 'done', stage = '', out_dir = ?2, result = ?3, shots = ?4, secs = ?5, error = NULL, updated_at = ?6 WHERE id = ?1",
                    params![id, o.out_dir.to_string_lossy(), o.result.to_string_lossy(), o.shots as i64, o.secs as f64, now()],
                )?,
                Err(e) => c.execute("UPDATE jobs SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1", params![id, e, now()])?,
            };
            Ok(())
        })
    }

    /// Задача упала с типизированным сбоем записи; `stage` не трогаем — этап сбоя остаётся виден.
    pub fn fail_job(&self, id: i64, f: &JobFailure) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET status = 'failed', error = ?2, fail_code = ?3, fail_state = ?4, fail_hint = ?5, fail_tail = ?6, updated_at = ?7 WHERE id = ?1",
                params![id, f.message, f.code, f.state, f.hint, f.tail, now()],
            )?;
            Ok(())
        })
    }

    /// Задача отменена (из очереди или на ходу — тогда исполнитель сам доводит её до этого статуса).
    pub fn cancel_job(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE jobs SET status = 'cancelled', stage = '', updated_at = ?2 WHERE id = ?1", params![id, now()])?;
            Ok(())
        })
    }

    /// Поставить задачу в очередь заново (после ошибки или чтобы перезаписать).
    pub fn requeue(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET status = 'queued', stage = '', error = NULL, fail_code = NULL, fail_state = NULL, fail_hint = NULL, fail_tail = NULL, updated_at = ?2 WHERE id = ?1",
                params![id, now()],
            )?;
            Ok(())
        })
    }

    pub fn delete_job(&self, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Задачи, оставшиеся «выполняются» после падения приложения, — в ошибку.
    pub fn reset_running(&self) -> Result<usize> {
        self.with(|c| {
            c.execute(
                "UPDATE jobs SET status = 'failed', error = 'прервано: приложение закрылось во время выполнения', updated_at = ?1 WHERE status = 'running'",
                [now()],
            )
        })
    }

    // ---- кэш FACEIT ----

    pub fn cache_get(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let s: Option<String> = self.with(|c| c.query_row("SELECT json FROM faceit_cache WHERE key = ?1", [key], |r| r.get(0)).optional())?;
        Ok(s.and_then(|s| serde_json::from_str(&s).ok()))
    }

    pub fn cache_put(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        let s = value.to_string();
        self.with(|c| {
            c.execute("INSERT OR REPLACE INTO faceit_cache (key, json, fetched_at) VALUES (?1, ?2, ?3)", params![key, s, now()])?;
            Ok(())
        })
    }

    // ---- демки ----

    pub fn demo_path(&self, match_id: &str) -> Result<Option<PathBuf>> {
        let p: Option<String> = self.with(|c| c.query_row("SELECT path FROM demos WHERE match_id = ?1", [match_id], |r| r.get(0)).optional())?;
        Ok(p.map(PathBuf::from).filter(|p| p.is_file()))
    }

    pub fn remember_demo(&self, match_id: &str, path: &Path) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO demos (match_id, path, downloaded_at) VALUES (?1, ?2, ?3)",
                params![match_id, path.to_string_lossy(), now()],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_job(kind: JobKind) -> NewJob {
        NewJob { kind, source: JobSource::Faceit("1-abc".into()), label: "матч".into(), hero_name: "me".into(), hero_steam: 42, map: "de_nuke".into() }
    }

    #[test]
    fn job_lifecycle() {
        let s = Store::in_memory().unwrap();
        let a = s.add_job(&new_job(JobKind::Record)).unwrap();
        let b = s.add_job(&new_job(JobKind::Plan)).unwrap();
        assert_eq!(s.next_queued().unwrap().unwrap().id, a, "очередь — по порядку постановки");
        s.start_job(a).unwrap();
        s.set_stage(a, "Запись").unwrap();
        s.append_log(a, "первая строка").unwrap();
        s.append_log(a, "вторая").unwrap();
        let j = s.job(a).unwrap().unwrap();
        assert_eq!((j.status, j.stage.as_str(), j.hero_steam), (JobStatus::Running, "Запись", 42));
        assert_eq!(j.log, "первая строка\nвторая\n");
        assert_eq!(s.running().unwrap().unwrap().id, a);
        assert_eq!(s.next_queued().unwrap().unwrap().id, b);

        let out = JobOutcome { out_dir: "C:/out".into(), result: "C:/out/highlights.mp4".into(), shots: 12, secs: 90.5 };
        s.finish_job(a, Ok(&out)).unwrap();
        let j = s.job(a).unwrap().unwrap();
        assert_eq!((j.status, j.shots, j.result.is_some()), (JobStatus::Done, Some(12), true));
        s.finish_job(b, Err("нет ffmpeg")).unwrap();
        assert_eq!(s.job(b).unwrap().unwrap().error.as_deref(), Some("нет ffmpeg"));
        s.requeue(b).unwrap();
        assert_eq!(s.next_queued().unwrap().unwrap().id, b);
        s.cancel_job(b).unwrap();
        assert_eq!(s.job(b).unwrap().unwrap().status, JobStatus::Cancelled);
        assert!(s.next_queued().unwrap().is_none());
        s.requeue(b).unwrap();
        assert_eq!(s.jobs().unwrap().iter().map(|j| j.id).collect::<Vec<_>>(), vec![b, a], "новые сверху");
        s.delete_job(a).unwrap();
        assert!(s.job(a).unwrap().is_none());
    }

    #[test]
    fn crash_recovery_and_caches() {
        let s = Store::in_memory().unwrap();
        let a = s.add_job(&new_job(JobKind::Plan)).unwrap();
        s.start_job(a).unwrap();
        assert_eq!(s.reset_running().unwrap(), 1);
        assert_eq!(s.job(a).unwrap().unwrap().status, JobStatus::Failed);

        assert!(s.cache_get("stats:1").unwrap().is_none());
        s.cache_put("stats:1", &serde_json::json!({"rounds": []})).unwrap();
        assert_eq!(s.cache_get("stats:1").unwrap().unwrap()["rounds"], serde_json::json!([]));

        assert!(s.demo_path("1-abc").unwrap().is_none());
        let here = std::env::current_exe().unwrap();
        s.remember_demo("1-abc", &here).unwrap();
        assert_eq!(s.demo_path("1-abc").unwrap(), Some(here));
        s.remember_demo("1-gone", Path::new("C:/nope/x.dem")).unwrap();
        assert!(s.demo_path("1-gone").unwrap().is_none(), "удалённый файл — как будто не скачан");
    }

    /// Временный файл базы; удаляется вместе с `-wal`/`-shm`.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("cs2cinema-store-{}-{}.db", std::process::id(), name));
            let t = TempDb(p);
            t.cleanup();
            t
        }
        fn cleanup(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut s = self.0.clone().into_os_string();
                s.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(s));
            }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    fn user_version(s: &Store) -> i64 {
        s.with(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0))).unwrap()
    }

    /// Задача, вставленная в базу сырым соединением, как это делала старая версия приложения.
    fn insert_old_job(c: &Connection) {
        c.execute(
            "INSERT INTO jobs (created_at, updated_at, kind, source_kind, source, label, hero_name, hero_steam, map, status, error)
             VALUES (1, 1, 'record', 'faceit', '1-abc', 'матч', 'me', 42, 'de_nuke', 'failed', 'старая ошибка')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn old_database_gains_failure_columns() {
        let db = TempDb::new("old");
        {
            let c = Connection::open(&db.0).unwrap();
            c.execute_batch(SCHEMA).unwrap();
            insert_old_job(&c);
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 0, "старая база без версии схемы");
        }
        let s = Store::open(&db.0).unwrap();
        let j = s.jobs().unwrap().pop().unwrap();
        assert_eq!(j.error.as_deref(), Some("старая ошибка"));
        assert_eq!((j.fail_code, j.fail_state, j.fail_hint, j.fail_tail), (None, None, None, None));
        assert_eq!(user_version(&s), 1);
        drop(s);
        let s = Store::open(&db.0).unwrap();
        assert_eq!(user_version(&s), 1, "повторное открытие ничего не меняет");
        assert_eq!(s.jobs().unwrap().len(), 1);
    }

    #[test]
    fn typed_failure_round_trips_and_retry_clears_it() {
        let s = Store::in_memory().unwrap();
        let a = s.add_job(&new_job(JobKind::Record)).unwrap();
        let f = JobFailure {
            message: "Игра запущена, но консольный порт не открылся за 90 с".into(),
            code: "CONSOLE_NOT_OPEN".into(),
            state: "console".into(),
            hint: "Проверьте -netconport в параметрах запуска".into(),
            tail: "строка 1\nстрока 2 'кавычка'; DROP TABLE jobs;--".into(),
        };
        s.start_job(a).unwrap();
        s.set_stage(a, "Запись").unwrap();
        s.fail_job(a, &f).unwrap();
        let j = s.job(a).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.error.as_deref(), Some(f.message.as_str()));
        assert_eq!(j.fail_code.as_deref(), Some("CONSOLE_NOT_OPEN"));
        assert_eq!(j.fail_state.as_deref(), Some("console"));
        assert_eq!(j.fail_hint.as_deref(), Some(f.hint.as_str()));
        assert_eq!(j.fail_tail.as_deref(), Some(f.tail.as_str()));
        assert_eq!(j.stage, "Запись", "этап сбоя остаётся виден");

        s.requeue(a).unwrap();
        let j = s.job(a).unwrap().unwrap();
        assert_eq!((j.error, j.fail_code, j.fail_state, j.fail_hint, j.fail_tail), (None, None, None, None, None));

        s.start_job(a).unwrap();
        s.fail_job(a, &f).unwrap();
        s.start_job(a).unwrap();
        let j = s.job(a).unwrap().unwrap();
        assert_eq!((j.status, j.error, j.fail_code, j.fail_state, j.fail_hint, j.fail_tail), (JobStatus::Running, None, None, None, None, None));
    }
}
