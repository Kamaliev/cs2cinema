//! Ядро сервера: задачи и их файлы поверх базы, без HTTP. Время берётся из внедрённых часов.
//!
//! Правило: копирование и хеширование файлов идут вне мьютекса базы, затем одна короткая транзакция.

use crate::Result;
use crate::blobs::{Blobs, Ingested, demo_rel, sniff_demo};
use crate::clock::Clock;
use crate::db::{Db, db_err};
use crate::secret::Redactor;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wire::{CancelMode, JobState};

const MIB: u64 = 1024 * 1024;

/// Пределы и сроки сервера (в миллисекундах, где указано).
#[derive(Clone, Debug)]
pub struct Limits {
    pub lease_ttl_ms: i64,
    pub heartbeat_secs: u64,
    pub longpoll_secs: u64,
    pub max_grants: i64,
    pub startup_grace_ms: i64,
    pub max_demo_bytes: u64,
    pub max_result_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            lease_ttl_ms: 45_000,
            heartbeat_secs: 10,
            longpoll_secs: 25,
            max_grants: 3,
            startup_grace_ms: 90_000,
            max_demo_bytes: 2000 * MIB,
            max_result_bytes: 2000 * MIB,
        }
    }
}

pub struct CoreOptions {
    pub data_dir: PathBuf,
    pub limits: Limits,
    pub clock: Arc<dyn Clock>,
    pub redactor: Redactor,
}

#[derive(Clone)]
pub struct Core {
    db: Db,
    blobs: Blobs,
    clock: Arc<dyn Clock>,
    redactor: Redactor,
    limits: Limits,
    data_dir: PathBuf,
}

/// Заявка на постановку демки в очередь.
#[derive(Debug, Clone, Default)]
pub struct Submit {
    pub player: String,
    pub user_id: Option<i64>,
    pub requested_by: String,
    pub orig_name: String,
}

/// Строка задачи со всеми колонками.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub id: i64,
    pub user_id: Option<i64>,
    pub requested_by: String,
    pub state: JobState,
    pub cancel_requested: CancelMode,
    pub epoch: i64,
    pub grants: i64,
    pub runner_id: Option<i64>,
    pub lease_until: Option<i64>,
    pub lease_lost: bool,
    pub last_seq: i64,
    pub source_kind: String,
    pub orig_name: String,
    pub faceit_link: Option<String>,
    pub player_steam: Option<String>,
    pub player_name: String,
    pub phase: String,
    pub take_n: Option<i64>,
    pub take_m: Option<i64>,
    pub fail_code: Option<String>,
    pub fail_class: Option<String>,
    pub fail_state: Option<String>,
    pub fail_hint: Option<String>,
    pub fail_message: Option<String>,
    pub fail_tail: Option<String>,
    pub fail_elapsed_ms: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

/// Файл задачи в хранилище.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub rel_path: String,
    pub size: u64,
    pub sha256: String,
}

const JOB_COLUMNS: &str = "id, user_id, requested_by, state, cancel_requested, epoch, grants, runner_id, lease_until, \
    lease_lost, last_seq, source_kind, orig_name, faceit_link, player_steam, player_name, phase, take_n, take_m, \
    fail_code, fail_class, fail_state, fail_hint, fail_message, fail_tail, fail_elapsed_ms, created_at, updated_at, finished_at";

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<JobRow> {
    let state: String = r.get(3)?;
    let cancel: String = r.get(4)?;
    Ok(JobRow {
        id: r.get(0)?,
        user_id: r.get(1)?,
        requested_by: r.get(2)?,
        state: JobState::parse(&state).unwrap_or(JobState::Unknown),
        cancel_requested: CancelMode::parse(&cancel).unwrap_or(CancelMode::Unknown),
        epoch: r.get(5)?,
        grants: r.get(6)?,
        runner_id: r.get(7)?,
        lease_until: r.get(8)?,
        lease_lost: r.get::<_, i64>(9)? != 0,
        last_seq: r.get(10)?,
        source_kind: r.get(11)?,
        orig_name: r.get(12)?,
        faceit_link: r.get(13)?,
        player_steam: r.get(14)?,
        player_name: r.get(15)?,
        phase: r.get(16)?,
        take_n: r.get(17)?,
        take_m: r.get(18)?,
        fail_code: r.get(19)?,
        fail_class: r.get(20)?,
        fail_state: r.get(21)?,
        fail_hint: r.get(22)?,
        fail_message: r.get(23)?,
        fail_tail: r.get(24)?,
        fail_elapsed_ms: r.get(25)?,
        created_at: r.get(26)?,
        updated_at: r.get(27)?,
        finished_at: r.get(28)?,
    })
}

/// Ник вида Steam64 (17 цифр, начинается с 7656) — это steamid, иначе обычное имя.
fn is_steam64(s: &str) -> bool {
    s.len() == 17 && s.starts_with("7656") && s.bytes().all(|b| b.is_ascii_digit())
}

impl Core {
    pub fn open(opts: CoreOptions) -> Result<Core> {
        let db = Db::open(&opts.data_dir.join("server.db"))?;
        let blobs = Blobs::new(opts.data_dir.join("blobs"));
        Ok(Core {
            db,
            blobs,
            clock: opts.clock,
            redactor: opts.redactor,
            limits: opts.limits,
            data_dir: opts.data_dir,
        })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn blobs(&self) -> &Blobs {
        &self.blobs
    }

    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Создаёт задачу в состоянии `receiving`; демка копируется следом (`finish_receive`).
    pub fn begin_receive(&self, s: &Submit) -> Result<i64> {
        let now = self.clock.now_ms();
        let player = self.redactor.clean(s.player.trim(), 64);
        if player.is_empty() {
            return Err("не указан игрок".to_owned());
        }
        let steam = is_steam64(&player).then(|| player.clone());
        let orig = self.redactor.clean(&s.orig_name, 120);
        let by = self.redactor.clean(&s.requested_by, 64);
        self.db.write(|tx| {
            tx.execute(
                "INSERT INTO jobs(user_id, requested_by, state, source_kind, orig_name, player_steam, player_name, created_at, updated_at)
                 VALUES (?1, ?2, 'receiving', 'file', ?3, ?4, ?5, ?6, ?6)",
                params![s.user_id, by, orig, steam, player, now],
            )
            .map_err(db_err)?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// Копирует демку в хранилище (вне мьютекса базы), затем одной транзакцией ставит `queued` и пишет строку файла.
    /// Сбой копирования переводит задачу в `failed` с кодом `RECEIVE_FAILED`.
    pub fn finish_receive(&self, job_id: i64, src: &Path) -> Result<Ingested> {
        match self
            .blobs
            .ingest_demo(job_id, src, self.limits.max_demo_bytes)
        {
            Ok(got) => {
                let now = self.clock.now_ms();
                self.db.write(|tx| {
                    let n = tx
                        .execute(
                            "UPDATE jobs SET state = 'queued', updated_at = ?2 WHERE id = ?1 AND state = 'receiving'",
                            params![job_id, now],
                        )
                        .map_err(db_err)?;
                    if n != 1 {
                        return Err(format!("задача #{job_id} не в состоянии приёма"));
                    }
                    tx.execute(
                        "INSERT INTO files(job_id, kind, rel_path, size, sha256, created_at) VALUES (?1, 'demo', ?2, ?3, ?4, ?5)",
                        params![job_id, demo_rel(job_id), got.size as i64, got.sha256, now],
                    )
                    .map_err(db_err)?;
                    Ok(())
                })?;
                Ok(got)
            }
            Err(e) => {
                let now = self.clock.now_ms();
                let msg = self
                    .redactor
                    .clean(&format!("не удалось принять демку: {e}"), 2000);
                self.db.write(|tx| {
                    tx.execute(
                        "UPDATE jobs SET state = 'failed', fail_code = ?2, fail_message = ?3, updated_at = ?4, finished_at = ?4
                         WHERE id = ?1 AND state = 'receiving'",
                        params![job_id, wire::fail::RECEIVE_FAILED, msg, now],
                    )
                    .map(|_| ())
                    .map_err(db_err)
                })?;
                Err(msg)
            }
        }
    }

    /// Ставит демку в очередь: проверка магии и размера до создания задачи, затем приём.
    pub fn submit(&self, src: &Path, s: &Submit) -> Result<(i64, Ingested)> {
        sniff_demo(src)?;
        let size = std::fs::metadata(src)
            .map_err(|e| format!("{}: {e}", src.display()))?
            .len();
        if size > self.limits.max_demo_bytes {
            return Err(format!(
                "файл {} МиБ больше лимита {} МиБ (max_demo_mib)",
                size / MIB,
                self.limits.max_demo_bytes / MIB
            ));
        }
        let id = self.begin_receive(s)?;
        let got = self.finish_receive(id, src)?;
        Ok((id, got))
    }

    /// Все задачи по возрастанию id.
    pub fn jobs(&self) -> Result<Vec<JobRow>> {
        self.db.read(|c| {
            let mut st = c
                .prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs ORDER BY id"))
                .map_err(db_err)?;
            let rows = st.query_map([], job_from_row).map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
        })
    }

    pub fn job(&self, id: i64) -> Result<Option<JobRow>> {
        self.db.read(|c| job_by_id(c, id))
    }

    /// Демка задачи (строка `files` с `kind = 'demo'`, не удалённая).
    pub fn demo_file(&self, job_id: i64) -> Result<Option<FileRow>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT rel_path, size, sha256 FROM files WHERE job_id = ?1 AND kind = 'demo' AND deleted_at IS NULL ORDER BY id DESC LIMIT 1",
                [job_id],
                |r| Ok(FileRow { rel_path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, sha256: r.get(2)? }),
            )
            .optional()
            .map_err(db_err)
        })
    }
}

fn job_by_id(c: &Connection, id: i64) -> Result<Option<JobRow>> {
    c.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"),
        [id],
        job_from_row,
    )
    .optional()
    .map_err(db_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs2srv-core-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn core(tag: &str, limits: Limits) -> (Core, PathBuf) {
        let d = test_dir(tag);
        let core = Core::open(CoreOptions {
            data_dir: d.join("data"),
            limits,
            clock: Arc::new(ManualClock::new(1_000)),
            redactor: Redactor::default(),
        })
        .unwrap();
        (core, d)
    }

    fn demo(path: &Path, size: usize) {
        let mut bytes = b"PBDEMS2\0".to_vec();
        bytes.resize(size, 7);
        std::fs::write(path, bytes).unwrap();
    }

    fn who(player: &str) -> Submit {
        Submit {
            player: player.to_owned(),
            requested_by: "оператор".to_owned(),
            orig_name: "m.dem".to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn oversized_demo_is_refused_before_a_job_row() {
        let (core, d) = core(
            "big",
            Limits {
                max_demo_bytes: 1024,
                ..Limits::default()
            },
        );
        let src = d.join("big.dem");
        demo(&src, 2048);
        assert!(core.submit(&src, &who("s1mple")).is_err());
        assert!(core.jobs().unwrap().is_empty(), "строки задачи нет");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_ingest_leaves_the_job_failed_with_receive_failed() {
        let (core, d) = core("fail", Limits::default());
        let id = core.begin_receive(&who("s1mple")).unwrap();
        assert!(core.finish_receive(id, &d.join("нет-такого.dem")).is_err());
        let job = core.job(id).unwrap().unwrap();
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.fail_code.as_deref(), Some("RECEIVE_FAILED"));
        assert!(job.finished_at.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn steam_id_player_fills_both_columns() {
        let (core, d) = core("steam", Limits::default());
        let a = core.begin_receive(&who("76561198000000001")).unwrap();
        let b = core.begin_receive(&who("s1mple")).unwrap();
        let ja = core.job(a).unwrap().unwrap();
        assert_eq!(ja.player_steam.as_deref(), Some("76561198000000001"));
        assert_eq!(ja.player_name, "76561198000000001");
        let jb = core.job(b).unwrap().unwrap();
        assert_eq!(jb.player_steam, None);
        assert_eq!(jb.player_name, "s1mple");
        assert_eq!(jb.state, JobState::Receiving);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn submitted_demo_is_queued_with_its_hash() {
        let (core, d) = core("ok", Limits::default());
        let src = d.join("m.dem");
        demo(&src, 4096);
        let (id, got) = core.submit(&src, &who("s1mple")).unwrap();
        assert_eq!(core.job(id).unwrap().unwrap().state, JobState::Queued);
        let f = core.demo_file(id).unwrap().unwrap();
        assert_eq!(
            (f.size, f.sha256.as_str(), f.rel_path.as_str()),
            (4096, got.sha256.as_str(), "blobs/incoming/1/demo")
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
