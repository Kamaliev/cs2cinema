//! Ядро сервера: задачи и их файлы поверх базы, без HTTP. Время берётся из внедрённых часов.
//!
//! Правило: копирование и хеширование файлов идут вне мьютекса базы, затем одна короткая транзакция.

use crate::Result;
use crate::blobs::{Blobs, Finalized, Ingested, demo_rel, result_rel, sniff_demo};
use crate::clock::Clock;
use crate::db::{Db, db_err};
use crate::rules::{self, Event, FinishKind};
use crate::secret::Redactor;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use wire::{
    Assignment, CancelMode, DemoInfo, Directive, DirectiveAction, Failure, FinishReply,
    FinishRequest, HelloReply, HelloRequest, JobState, Outcome, Player, ProgressReply,
    ProgressRequest, ReleaseRequest, codes,
};

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

/// Отказ ядра запросу: код из `wire::codes`, текст для человека по-русски и, при необходимости,
/// подробность для журнала (в ответ раннеру она не попадает).
#[derive(Debug, Clone)]
pub struct Reject {
    pub code: &'static str,
    pub message: String,
    pub offset: Option<u64>,
    pub detail: Option<String>,
    /// Дополнительные заголовки ответа (`Retry-After`, `Content-Range`).
    pub headers: Vec<(&'static str, String)>,
}

impl Reject {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            offset: None,
            detail: None,
            headers: Vec::new(),
        }
    }
}

/// Строка ошибки ядра (база, файлы) — внутренняя ошибка: раннер видит общий текст, журнал — подробность.
impl From<String> for Reject {
    fn from(detail: String) -> Self {
        Self {
            code: wire::codes::INTERNAL,
            message: "внутренняя ошибка сервера".to_owned(),
            offset: None,
            detail: Some(detail),
            headers: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Core {
    db: Db,
    blobs: Blobs,
    clock: Arc<dyn Clock>,
    redactor: Redactor,
    limits: Limits,
    data_dir: PathBuf,
    /// Поколение «появилась работа» и условная переменная для long-poll.
    wake: Arc<(Mutex<u64>, Condvar)>,
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
pub fn is_steam64(s: &str) -> bool {
    s.len() == 17 && s.starts_with("7656") && s.bytes().all(|b| b.is_ascii_digit())
}

/// Ключ игрока для поиска дублей: SteamID64 как есть, иначе имя без пробелов по краям, в нижнем регистре и с
/// сериями пробелов, сжатыми в один.
pub fn player_key(player: &str) -> String {
    let p = player.trim();
    if is_steam64(p) {
        return p.to_owned();
    }
    p.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Состояния «задача ещё не закончена» для лимитов бота.
const ACTIVE_STATES: &str = "('receiving','queued','assigned','running','uploading')";
/// Состояния, в которых задача считается дублем: активные и готовая.
const DEDUP_STATES: &str = "('receiving','queued','assigned','running','uploading','done')";

/// Сколько задач не закончилось у пользователя и во всей очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveCounts {
    pub user: i64,
    pub total: i64,
}

/// Ключ поиска дубля: тот же файл (по SHA-256) или тот же матч FACEIT и тот же игрок (`player_key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupKey {
    File { sha256: String, player_key: String },
    Faceit { link: String, player_key: String },
}

fn query_jobs(c: &Connection, sql: &str, arg: &str) -> Result<Vec<JobRow>> {
    let mut st = c.prepare(sql).map_err(db_err)?;
    let rows = st.query_map([arg], job_from_row).map_err(db_err)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
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
            wake: Arc::new((Mutex::new(0), Condvar::new())),
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

    /// Ставит в очередь матч FACEIT по ссылке: задача сразу `queued`, без файла (демку скачивает раннер).
    /// Ссылка проверяется строгим разбором `wire::faceit`; в базу идёт только канонический вид из id (D-01),
    /// сервер сам по ней никуда не ходит.
    pub fn submit_faceit(&self, link: &str, s: &Submit) -> Result<i64> {
        let Some(match_id) = wire::faceit::room_match_id(link) else {
            return Err("ссылка не похожа на комнату матча FACEIT".to_owned());
        };
        let now = self.clock.now_ms();
        let player = self.redactor.clean(s.player.trim(), 64);
        if player.is_empty() {
            return Err("не указан игрок".to_owned());
        }
        let steam = is_steam64(&player).then(|| player.clone());
        let by = self.redactor.clean(&s.requested_by, 64);
        let canonical = wire::faceit::canonical_url(&match_id);
        let orig = format!("faceit-{match_id}");
        let id = self.db.write(|tx| {
            tx.execute(
                "INSERT INTO jobs(user_id, requested_by, state, source_kind, orig_name, faceit_link, player_steam, player_name, created_at, updated_at)
                 VALUES (?1, ?2, 'queued', 'faceit', ?3, ?4, ?5, ?6, ?7, ?7)",
                params![s.user_id, by, orig, canonical, steam, player, now],
            )
            .map_err(db_err)?;
            Ok::<i64, String>(tx.last_insert_rowid())
        })?;
        self.wake();
        Ok(id)
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
                    let before = job_by_id(tx, job_id)?
                        .ok_or_else(|| format!("нет задачи #{job_id}"))?;
                    let d = rules::decide(&before, &Event::Received, now, &self.limits)
                        .map_err(|r| r.message)?;
                    apply(tx, &before, &d.job, now)?;
                    tx.execute(
                        "INSERT INTO files(job_id, kind, rel_path, size, sha256, created_at) VALUES (?1, 'demo', ?2, ?3, ?4, ?5)",
                        params![job_id, demo_rel(job_id), got.size as i64, got.sha256, now],
                    )
                    .map_err(db_err)?;
                    Ok::<(), String>(())
                })?;
                self.wake();
                Ok(got)
            }
            Err(e) => {
                let now = self.clock.now_ms();
                let msg = self
                    .redactor
                    .clean(&format!("не удалось принять демку: {e}"), 2000);
                let ev = Event::ReceiveFailed {
                    message: msg.clone(),
                };
                self.db.write(|tx| {
                    // Задача уже не в приёме (например, её отменили): оставляем как есть.
                    if let Some(before) = job_by_id(tx, job_id)?
                        && let Ok(d) = rules::decide(&before, &ev, now, &self.limits)
                    {
                        apply(tx, &before, &d.job, now)?;
                    }
                    Ok::<(), String>(())
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

    /// Новая задача на демке другой задачи (второй игрок той же демки, повтор): файл берётся жёсткой ссылкой
    /// (`Blobs::link_demo`, копирование — запасной путь), FACEIT — та же ссылка без файла. Задача получает
    /// `queued` только когда файл на месте, чтобы раннер не получил задачу без демки.
    pub fn submit_from(&self, demo_job_id: i64, s: &Submit) -> Result<i64> {
        let player = self.redactor.clean(s.player.trim(), 64);
        if player.is_empty() {
            return Err("не указан игрок".to_owned());
        }
        let (src, known) = self.db.read(|c| {
            Ok::<_, String>((job_by_id(c, demo_job_id)?, demo_by_job(c, demo_job_id)?))
        })?;
        let gone = || format!("демки задачи #{demo_job_id} больше нет — пришлите файл заново");
        let src = src.ok_or_else(gone)?;
        let steam = is_steam64(&player).then(|| player.clone());
        let by = self.redactor.clean(&s.requested_by, 64);
        let orig = if s.orig_name.trim().is_empty() {
            src.orig_name.clone()
        } else {
            self.redactor.clean(&s.orig_name, 120)
        };
        let now = self.clock.now_ms();

        if src.source_kind == "faceit" {
            let match_id = src
                .faceit_link
                .as_deref()
                .and_then(wire::faceit::room_match_id)
                .ok_or_else(gone)?;
            let canonical = wire::faceit::canonical_url(&match_id);
            let id = self.db.write(|tx| {
                tx.execute(
                    "INSERT INTO jobs(user_id, requested_by, state, source_kind, orig_name, faceit_link, player_steam, player_name, created_at, updated_at)
                     VALUES (?1, ?2, 'queued', 'faceit', ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![s.user_id, by, orig, canonical, steam, player, now],
                )
                .map_err(db_err)?;
                Ok::<i64, String>(tx.last_insert_rowid())
            })?;
            self.wake();
            return Ok(id);
        }

        let known = known.ok_or_else(gone)?;
        let id = self.db.write(|tx| {
            tx.execute(
                "INSERT INTO jobs(user_id, requested_by, state, source_kind, orig_name, player_steam, player_name, created_at, updated_at)
                 VALUES (?1, ?2, 'receiving', 'file', ?3, ?4, ?5, ?6, ?6)",
                params![s.user_id, by, orig, steam, player, now],
            )
            .map_err(db_err)?;
            Ok::<i64, String>(tx.last_insert_rowid())
        })?;
        match self.blobs.link_demo(demo_job_id, id, &known) {
            Ok(got) => {
                let now = self.clock.now_ms();
                self.db.write(|tx| {
                    let before =
                        job_by_id(tx, id)?.ok_or_else(|| format!("нет задачи #{id}"))?;
                    let d = rules::decide(&before, &Event::Received, now, &self.limits)
                        .map_err(|r| r.message)?;
                    apply(tx, &before, &d.job, now)?;
                    tx.execute(
                        "INSERT INTO files(job_id, kind, rel_path, size, sha256, created_at) VALUES (?1, 'demo', ?2, ?3, ?4, ?5)",
                        params![id, demo_rel(id), got.size as i64, got.sha256, now],
                    )
                    .map_err(db_err)?;
                    Ok::<(), String>(())
                })?;
                self.wake();
                Ok(id)
            }
            Err(e) => {
                let now = self.clock.now_ms();
                let ev = Event::ReceiveFailed {
                    message: self.redactor.clean(
                        &format!("не удалось взять демку задачи #{demo_job_id}: {e}"),
                        2000,
                    ),
                };
                self.db.write(|tx| {
                    if let Some(before) = job_by_id(tx, id)?
                        && let Ok(d) = rules::decide(&before, &ev, now, &self.limits)
                    {
                        apply(tx, &before, &d.job, now)?;
                    }
                    Ok::<(), String>(())
                })?;
                Err(gone())
            }
        }
    }

    /// Повтор задачи, закончившейся сбоем или отменой: тот же игрок, тот же пользователь, та же демка (или ссылка
    /// FACEIT), заказчик — `requested_by`. Задача в другом состоянии повторяться не может.
    pub fn resubmit(&self, job_id: i64, requested_by: &str) -> Result<i64> {
        let src = self
            .job(job_id)?
            .ok_or_else(|| format!("нет задачи #{job_id}"))?;
        if !matches!(src.state, JobState::Failed | JobState::Cancelled) {
            return Err(format!(
                "задачу #{job_id} нельзя повторить: она не закончилась сбоем или отменой"
            ));
        }
        self.submit_from(
            job_id,
            &Submit {
                player: src.player_name.clone(),
                user_id: src.user_id,
                requested_by: requested_by.to_owned(),
                orig_name: src.orig_name.clone(),
            },
        )
    }

    /// Сколько задач пользователя и всей очереди ещё не закончились.
    pub fn active_counts(&self, user_id: i64) -> Result<ActiveCounts> {
        self.db.read(|c| {
            let sql = |extra: &str| {
                format!("SELECT COUNT(*) FROM jobs WHERE state IN {ACTIVE_STATES}{extra}")
            };
            let user = c
                .query_row(&sql(" AND user_id = ?1"), [user_id], |r| r.get(0))
                .map_err(db_err)?;
            let total = c.query_row(&sql(""), [], |r| r.get(0)).map_err(db_err)?;
            Ok(ActiveCounts { user, total })
        })
    }

    /// Самая свежая задача с таким ключом среди активных и готовых. Упавшая и отменённая дублем не считаются:
    /// повтор после сбоя разрешён.
    pub fn find_duplicate(&self, key: &DedupKey) -> Result<Option<JobRow>> {
        self.find_duplicate_impl(key, None)
    }

    /// Дубль с точки зрения пользователя `user_id` (WR-01): его собственная задача (активная или готовая) либо чужая
    /// ещё идущая. Чужой готовый ролик дублем не считается: другу он нужен тоже, а `/resend` чужой задачи ему
    /// запрещён, так что отказ был бы тупиком.
    pub fn find_duplicate_for(&self, key: &DedupKey, user_id: i64) -> Result<Option<JobRow>> {
        self.find_duplicate_impl(key, Some(user_id))
    }

    fn find_duplicate_impl(&self, key: &DedupKey, user: Option<i64>) -> Result<Option<JobRow>> {
        // Готовая задача — дубль, только пока её ролик ещё можно отдать: он лежит на диске или в Telegram
        // сохранён file_id. Иначе бот ответил бы «уже готов: /resend N», а /resend — «ролика нет» (тупик, 09-02).
        let alive = "(state != 'done' \
             OR EXISTS (SELECT 1 FROM files r WHERE r.job_id = jobs.id AND r.kind = 'result' AND r.deleted_at IS NULL) \
             OR EXISTS (SELECT 1 FROM tg_jobs t WHERE t.job_id = jobs.id AND t.result_file_id IS NOT NULL))";
        let (rows, wanted): (Vec<JobRow>, &str) = match key {
            DedupKey::File { sha256, player_key } => (
                self.db.read(|c| {
                    query_jobs(
                        c,
                        &format!(
                            "SELECT {JOB_COLUMNS} FROM jobs WHERE id IN (SELECT job_id FROM files WHERE kind = 'demo' AND sha256 = ?1) \
                             AND state IN {DEDUP_STATES} AND {alive} ORDER BY id DESC"
                        ),
                        sha256,
                    )
                })?,
                player_key,
            ),
            DedupKey::Faceit { link, player_key } => {
                let link = wire::faceit::room_match_id(link)
                    .map(|id| wire::faceit::canonical_url(&id))
                    .unwrap_or_else(|| link.clone());
                (
                    self.db.read(|c| {
                        query_jobs(
                            c,
                            &format!(
                                "SELECT {JOB_COLUMNS} FROM jobs WHERE source_kind = 'faceit' AND faceit_link = ?1 \
                                 AND state IN {DEDUP_STATES} AND {alive} ORDER BY id DESC"
                            ),
                            &link,
                        )
                    })?,
                    player_key,
                )
            }
        };
        Ok(rows.into_iter().find(|j| {
            player_key(&j.player_name) == wanted
                && user.is_none_or(|u| j.user_id == Some(u) || j.state != JobState::Done)
        }))
    }

    /// Сколько задач обгонит эта: для `queued` — задачи в очереди с меньшим id плюс выданные раннеру и ведущиеся;
    /// для любого другого состояния — 0.
    pub fn jobs_ahead(&self, job_id: i64) -> Result<i64> {
        self.db.read(|c| {
            let Some(job) = job_by_id(c, job_id)? else {
                return Ok(0);
            };
            if job.state != JobState::Queued {
                return Ok(0);
            }
            c.query_row(
                "SELECT COUNT(*) FROM jobs WHERE (state = 'queued' AND id < ?1) \
                 OR state IN ('assigned','running','uploading')",
                [job_id],
                |r| r.get(0),
            )
            .map_err(db_err)
        })
    }

    /// Есть ли раннер, который выходил на связь недавно: неотозванный, `last_seen_at` не старше
    /// `longpoll + lease_ttl` (раннер в long-poll молчит до `longpoll_secs`, аренда живёт `lease_ttl`).
    pub fn runner_online(&self) -> Result<bool> {
        let window = self.limits.longpoll_secs as i64 * 1000 + self.limits.lease_ttl_ms;
        let since = self.clock.now_ms() - window;
        self.db.read(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM runners WHERE revoked_at IS NULL AND last_seen_at >= ?1)",
                [since],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n != 0)
            .map_err(db_err)
        })
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
        self.db.read(|c| demo_by_job(c, job_id))
    }

    /// Принятый результат задачи (строка `files` с `kind = 'result'`, не удалённая): размер и SHA-256.
    pub fn result_file(&self, job_id: i64) -> Result<Option<FileRow>> {
        self.db.read(|c| result_by_job(c, job_id))
    }

    /// Выдача задачи раннеру (SRV-04, D-04). Тонкая обёртка над `claim_report`: пустая очередь — `None`.
    pub fn claim(&self, runner_id: i64) -> std::result::Result<Option<Assignment>, Reject> {
        Ok(self.claim_report(runner_id)?.assignment)
    }

    /// Выдача задачи раннеру одной транзакцией `BEGIN IMMEDIATE` (SRV-04, SRV-07, D-04), шаги по порядку:
    /// 1. истёкшие аренды возвращаются в очередь;
    /// 2. задача раннера в `assigned` выдаётся повторно без новой эпохи и выдачи (ответ мог потеряться в сети);
    /// 3. задача раннера в `running` или `uploading` — отказ `already_holding`;
    /// 4. задачи в очереди, выданные `max_grants` раз, падают с `TOO_MANY_GRANTS` — «ядовитая» демка не крутится по кругу;
    /// 5. самая старая задача в очереди получает эпоху +1, раннера и аренду одним `UPDATE … RETURNING`.
    pub fn claim_report(&self, runner_id: i64) -> std::result::Result<Claimed, Reject> {
        let now = self.clock.now_ms();
        let lease_until = now + self.limits.lease_ttl_ms;
        let max = self.limits.max_grants;
        self.db.write(|tx| {
            expire_leases(tx, now, &self.limits)?;

            let held: Option<i64> = tx
                .query_row(
                    "SELECT id FROM jobs WHERE runner_id = ?1 AND state IN ('assigned','running','uploading') \
                     ORDER BY id LIMIT 1",
                    [runner_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            if let Some(id) = held {
                let job = job_by_id(tx, id)?.ok_or_else(|| format!("задача #{id} исчезла"))?;
                if job.state != JobState::Assigned {
                    return Err(Reject::new(
                        codes::ALREADY_HOLDING,
                        format!("раннер уже ведёт задачу #{id}: сначала завершите её"),
                    ));
                }
                tx.execute(
                    "UPDATE jobs SET lease_until = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, lease_until, now],
                )
                .map_err(db_err)?;
                let job = job_by_id(tx, id)?.ok_or_else(|| format!("задача #{id} исчезла"))?;
                return Ok(Claimed {
                    assignment: Some(assignment_of(tx, &job)?),
                    failed: Vec::new(),
                });
            }

            // WR-02: задача, потерявшая аренду на последней разрешённой выдаче, ждёт своего раннера окно
            // `lease_ttl + startup_grace` (он вернёт её через `hello`); сам владелец, просящий новую задачу, уже
            // свободен — для него задача ядовита сразу. Без окна чужой `claim` ронял бы задачу, раннер которой
            // просто перезапускается.
            let return_window = now - (self.limits.lease_ttl_ms + self.limits.startup_grace_ms);
            let poisoned: Vec<(i64, i64)> = {
                let mut st = tx
                    .prepare(
                        "SELECT id, grants FROM jobs WHERE state = 'queued' AND grants >= ?1 \
                         AND (lease_lost = 0 OR runner_id IS NULL OR runner_id = ?2 OR updated_at <= ?3) ORDER BY id",
                    )
                    .map_err(db_err)?;
                let rows = st
                    .query_map(params![max, runner_id, return_window], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)?
            };
            let mut failed = Vec::new();
            for (id, grants) in poisoned {
                tx.execute(
                    "UPDATE jobs SET state = 'failed', fail_code = ?2, fail_class = 'server', fail_state = 'queued', \
                     fail_message = ?3, fail_hint = ?4, finished_at = ?5, updated_at = ?5, lease_until = NULL, lease_lost = 0 \
                     WHERE id = ?1 AND state = 'queued'",
                    params![
                        id,
                        wire::fail::TOO_MANY_GRANTS,
                        format!(
                            "задачу выдавали раннеру {grants} раз, и ни разу она не дошла до конца — похоже, демка роняет раннер"
                        ),
                        "проверьте демку и поставьте задачу заново",
                        now
                    ],
                )
                .map_err(db_err)?;
                failed.push(id);
            }

            // Номер сообщения раннера считается заново в каждой эпохе: `last_seq`, этап и дубль сбрасываются.
            let id: Option<i64> = tx
                .query_row(
                    "UPDATE jobs SET state = 'assigned', epoch = epoch + 1, grants = grants + 1, runner_id = ?1, \
                     lease_until = ?2, lease_lost = 0, last_seq = 0, phase = '', take_n = NULL, take_m = NULL, updated_at = ?3 \
                     WHERE id = (SELECT id FROM jobs WHERE state = 'queued' AND grants < ?4 ORDER BY id LIMIT 1) RETURNING id",
                    params![runner_id, lease_until, now, max],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            let Some(id) = id else {
                return Ok(Claimed {
                    assignment: None,
                    failed,
                });
            };
            // Раннер просит новую задачу — значит, он свободен: прежняя задача с потерянной арендой ему больше не
            // принадлежит, иначе поздний прогресс или `hello` вернули бы её, и раннер вёл бы две задачи сразу.
            tx.execute(
                "UPDATE jobs SET runner_id = NULL, lease_lost = 0, updated_at = ?3 \
                 WHERE runner_id = ?1 AND state = 'queued' AND lease_lost = 1 AND id != ?2",
                params![runner_id, id, now],
            )
            .map_err(db_err)?;
            let job = job_by_id(tx, id)?
                .ok_or_else(|| format!("задача #{id} исчезла сразу после выдачи"))?;
            Ok(Claimed {
                assignment: Some(assignment_of(tx, &job)?),
                failed,
            })
        })
    }

    /// `hello`: раннер представляется и сверяет удерживаемые задачи (SRV-05). Версию протокола проверяет
    /// вызывающий до любых записей. По каждой задаче из `holding` ответ — `keep` (аренда подтверждена и
    /// продлена, потерянная аренда возвращена раннеру), `cancel` (запрошена отмена) или `abandon`
    /// (задача выдана заново, завершена или неизвестна).
    pub fn hello(
        &self,
        runner_id: i64,
        req: &HelloRequest,
    ) -> std::result::Result<HelloReply, Reject> {
        let now = self.clock.now_ms();
        let version = self.redactor.clean(&req.app_version, 64);
        let status = self.redactor.clean(&req.status, 200);
        let directives = self.db.write(|tx| {
            tx.execute(
                "UPDATE runners SET version = ?2, status = ?3, last_seen_at = ?4 WHERE id = ?1",
                params![runner_id, version, status, now],
            )
            .map_err(db_err)?;
            let mut out = Vec::with_capacity(req.holding.len());
            for h in &req.holding {
                let ev = Event::Reattach {
                    runner_id,
                    epoch: h.epoch,
                };
                let action = match job_by_id(tx, h.job_id)? {
                    None => DirectiveAction::Abandon,
                    Some(before) => match rules::decide(&before, &ev, now, &self.limits) {
                        Ok(d) => {
                            apply(tx, &before, &d.job, now)?;
                            if d.job.cancel_requested == CancelMode::NotRequested {
                                DirectiveAction::Keep
                            } else {
                                DirectiveAction::Cancel
                            }
                        }
                        Err(_) => DirectiveAction::Abandon,
                    },
                };
                out.push(Directive {
                    job_id: h.job_id,
                    action,
                });
            }
            Ok::<Vec<Directive>, String>(out)
        })?;
        Ok(HelloReply {
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            server_time_ms: now,
            poll_secs: self.limits.longpoll_secs,
            heartbeat_secs: self.limits.heartbeat_secs,
            chunk_size: wire::CHUNK_SIZE,
            directives,
        })
    }

    /// Восстановление после перезапуска, до открытия порта: зависшие приёмы падают с `RECEIVE_FAILED`,
    /// аренды живых раннеров продлеваются до `now + startup_grace`, чтобы раннер успел вернуться (SRV-05).
    pub fn recover(&self) -> Result<Recovered> {
        let now = self.clock.now_ms();
        let grace_until = now + self.limits.startup_grace_ms;
        let mut out = self.db.write(|tx| {
            let receive_failed = fail_stale_receiving(tx, now, &self.limits)?;
            let ids: Vec<i64> = {
                let mut st = tx
                    .prepare("SELECT id FROM jobs WHERE state IN ('assigned','running','uploading') ORDER BY id")
                    .map_err(db_err)?;
                let rows = st.query_map([], |r| r.get(0)).map_err(db_err)?;
                rows.collect::<rusqlite::Result<Vec<i64>>>().map_err(db_err)?
            };
            let mut grace_extended = 0;
            for id in ids {
                let Some(before) = job_by_id(tx, id)? else {
                    continue;
                };
                let d = rules::decide(&before, &Event::Recover { grace_until }, now, &self.limits)
                    .map_err(|r| r.message)?;
                if d.job != before {
                    apply(tx, &before, &d.job, now)?;
                    grace_extended += 1;
                }
            }
            Ok::<Recovered, String>(Recovered {
                receive_failed,
                grace_extended,
                parts_discarded: 0,
            })
        })?;
        // Куски результата окончательных задач без живой выдачи больше никому не нужны (Pitfall 7).
        let finished: Vec<i64> = self.db.read(|c| {
            let mut st = c
                .prepare(
                    "SELECT id FROM jobs WHERE state IN ('done','failed','cancelled') ORDER BY id",
                )
                .map_err(db_err)?;
            let rows = st.query_map([], |r| r.get(0)).map_err(db_err)?;
            rows.collect::<rusqlite::Result<Vec<i64>>>().map_err(db_err)
        })?;
        for id in finished {
            if self.blobs.results_dir(id).is_dir() {
                out.parts_discarded += self.blobs.discard_parts(id).unwrap_or(0);
            }
        }
        Ok(out)
    }

    /// Раннер возвращает задачу в очередь (`release`): окружение не готово, раннер уходит и т. п.
    /// `counts = false` возвращает и одну выдачу. Возврат чужой выдачи — `stale_epoch`.
    /// Запрошенная отмена превращает возврат в отмену: задача становится `cancelled` и в очередь не возвращается.
    pub fn release(
        &self,
        runner_id: i64,
        job_id: i64,
        req: &ReleaseRequest,
    ) -> std::result::Result<(), Reject> {
        let now = self.clock.now_ms();
        let ev = Event::Release {
            runner_id,
            epoch: req.epoch,
            counts: req.counts,
        };
        let discard = self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<bool, Reject>(d.discard_parts)
        })?;
        if discard {
            // Как в `cancel`: не удалось убрать недозагруженные куски — остатки уберёт `recover`.
            let _ = self.blobs.discard_parts(job_id);
        }
        self.wake();
        Ok(())
    }

    /// Отмена задачи оператором (`job cancel`, SRV-03). Задача в `receiving` и `queued` отменяется сразу; выданной
    /// ставится флаг (мягкий, повторный вызов или `force` — жёсткий), который раннер увидит в ответе на прогресс и
    /// в `hello`; окончательная задача — отказ «задача уже завершена». Возвращает состояние после отмены.
    pub fn cancel(&self, job_id: i64, force: bool) -> Result<JobState> {
        let now = self.clock.now_ms();
        let (state, discard) = self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(|| format!("нет задачи #{job_id}"))?;
            let d = rules::decide(&before, &Event::Cancel { force }, now, &self.limits)
                .map_err(|r| r.message)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<_, String>((d.job.state, d.discard_parts))
        })?;
        if discard {
            // Нечего удалять или не удалось — на отмену не влияет: остатки уберёт `recover`.
            let _ = self.blobs.discard_parts(job_id);
        }
        Ok(state)
    }

    /// Очищает текст сбоя раннера до записи в базу (SEC-04, T-05-19): секреты маскируются, управляющие символы
    /// удаляются, длины ограничены. Код сбоя — только `[A-Z0-9_]{1,64}`, иначе `UNKNOWN`.
    fn sanitize_failure(&self, f: &Failure) -> Failure {
        let code_ok = !f.code.is_empty()
            && f.code.len() <= 64
            && f.code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        Failure {
            code: if code_ok {
                f.code.clone()
            } else {
                "UNKNOWN".to_owned()
            },
            class: self.redactor.clean(&f.class, 32),
            state: self.redactor.clean(&f.state, 64),
            hint: self.redactor.clean(&f.hint, 1000),
            message: self.redactor.clean(&f.message, 2000),
            tail: self.redactor.clean(&f.tail, 16_384),
            elapsed_ms: f.elapsed_ms,
        }
    }

    /// Итог задачи от раннера (SRV-03): сбой или отмена. Поля сбоя очищаются до записи; повтор того же итога
    /// идемпотентен, другой итог после конца — `job_terminal`, устаревшая эпоха — `stale_epoch`.
    pub fn finish(
        &self,
        runner_id: i64,
        job_id: i64,
        req: &FinishRequest,
    ) -> std::result::Result<FinishReply, Reject> {
        Ok(self.finish_report(runner_id, job_id, req)?.reply)
    }

    /// То же, что `finish`, и ошибка удаления недозагруженных кусков (итог уже записан, ответ раннеру остаётся 200).
    pub fn finish_report(
        &self,
        runner_id: i64,
        job_id: i64,
        req: &FinishRequest,
    ) -> std::result::Result<Finished, Reject> {
        let kind = match &req.outcome {
            Outcome::Done { size, sha256 } => {
                return self.finish_done(runner_id, job_id, req.epoch, *size, sha256);
            }
            Outcome::Failed(f) => FinishKind::Failed(self.sanitize_failure(f)),
            Outcome::Cancelled => FinishKind::Cancelled,
            Outcome::Unknown => {
                return Err(Reject::new(
                    codes::BAD_REQUEST,
                    "неизвестный итог задачи: обновите сервер",
                ));
            }
        };
        let now = self.clock.now_ms();
        let ev = Event::Finish {
            runner_id,
            epoch: req.epoch,
            kind,
        };
        let (state, discard) = self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<_, Reject>((d.job.state, d.discard_parts && before != d.job))
        })?;
        let discard_error = if discard {
            self.blobs
                .discard_parts(job_id)
                .err()
                .map(|e| format!("задача #{job_id}: не удалось удалить куски результата: {e}"))
        } else {
            None
        };
        Ok(Finished {
            reply: FinishReply { state },
            discard_error,
        })
    }

    /// Итог `done` (SRV-06): результат становится файлом задачи только после совпадения размера и SHA-256.
    /// Порядок: пробное решение автомата без записи -> проверка файла вне мьютекса базы под охраной загрузки ->
    /// вторая транзакция с повторным решением и строкой `files` (сравнение-и-запись: если задачу отменили, пока шёл хеш,
    /// готовый файл удаляется и раннер получает отказ).
    fn finish_done(
        &self,
        runner_id: i64,
        job_id: i64,
        epoch: i64,
        size: u64,
        sha256: &str,
    ) -> std::result::Result<Finished, Reject> {
        let sha = sha256.to_ascii_lowercase();
        if size == 0 || size > self.limits.max_result_bytes {
            return Err(Reject::new(
                codes::BAD_REQUEST,
                format!(
                    "размер результата должен быть от 1 байта до {} МиБ",
                    self.limits.max_result_bytes / MIB
                ),
            ));
        }
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Reject::new(
                codes::BAD_REQUEST,
                "sha256 результата должен быть из 64 шестнадцатеричных знаков",
            ));
        }
        let ev = Event::Finish {
            runner_id,
            epoch,
            kind: FinishKind::Done,
        };

        // Проверка 1: пробное решение без записи. Задача уже `done` у этого раннера и этой эпохи — повтор идемпотентен,
        // если принятый файл тот же.
        let now = self.clock.now_ms();
        let repeat = self.db.read(|c| {
            let before = job_by_id(c, job_id)?.ok_or_else(not_found)?;
            rules::decide(&before, &ev, now, &self.limits)?;
            if before.state != JobState::Done {
                return Ok::<bool, Reject>(false);
            }
            match result_by_job(c, job_id)? {
                Some(f) if f.sha256 == sha && f.size == size => Ok(true),
                _ => Err(Reject::new(codes::JOB_TERMINAL, "задача уже завершена")),
            }
        })?;
        if repeat {
            return Ok(Finished {
                reply: FinishReply {
                    state: JobState::Done,
                },
                discard_error: None,
            });
        }

        // Файл: вне мьютекса базы, под охраной загрузки этой эпохи.
        let Some(_guard) = self.blobs.lock_upload(job_id, epoch) else {
            return Err(Reject::new(
                codes::UPLOAD_BUSY,
                "идёт загрузка куска этой задачи: повторите итог чуть позже",
            ));
        };
        match self.blobs.finalize_result(job_id, epoch, size, &sha)? {
            Finalized::Ok => {}
            Finalized::SizeMismatch { actual } => {
                let mut r = Reject::new(
                    codes::OFFSET_MISMATCH,
                    format!("размер результата {actual}, а не {size}: догрузите недостающее"),
                );
                r.offset = Some(actual);
                return Err(r);
            }
            Finalized::ShaMismatch => {
                return Err(Reject::new(
                    codes::SHA256_MISMATCH,
                    "SHA-256 результата не совпал: загрузите результат заново с нуля",
                ));
            }
        }

        // Транзакция 2: повторное решение (задачу могли отменить или выдать заново) и строка файла.
        let now = self.clock.now_ms();
        let committed = self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            tx.execute(
                "INSERT INTO files(job_id, kind, rel_path, size, sha256, created_at) VALUES (?1, 'result', ?2, ?3, ?4, ?5)",
                params![job_id, result_rel(job_id), size as i64, sha, now],
            )
            .map_err(db_err)?;
            Ok::<JobState, Reject>(d.job.state)
        });
        let state = match committed {
            Ok(s) => s,
            Err(rej) => {
                // Такой файл не становится результатом: убираем его, если задача не закрыта другой записью.
                let closed = matches!(self.job(job_id), Ok(Some(j)) if j.state == JobState::Done);
                if !closed {
                    let _ = self.blobs.remove_result(job_id);
                }
                return Err(rej);
            }
        };
        let discard_error = self
            .blobs
            .discard_parts(job_id)
            .err()
            .map(|e| format!("задача #{job_id}: не удалось удалить куски результата: {e}"));
        Ok(Finished {
            reply: FinishReply { state },
            discard_error,
        })
    }

    /// Загрузка куска результата: ограждение эпохой, `-> uploading` и продление аренды. Отказ окончательной
    /// задаче сопровождается уборкой её недозагруженных кусков (между отказом и удалением мог дописаться кусок).
    pub fn touch_upload(
        &self,
        runner_id: i64,
        job_id: i64,
        epoch: i64,
    ) -> std::result::Result<(), Reject> {
        let now = self.clock.now_ms();
        let ev = Event::Chunk { runner_id, epoch };
        let out = self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<(), Reject>(())
        });
        if let Err(r) = &out
            && r.code == codes::JOB_TERMINAL
        {
            let _ = self.blobs.discard_parts(job_id);
        }
        out
    }

    /// `HEAD` результата и скачивание демки: ограждение эпохой и продление аренды без смены состояния.
    pub fn touch(
        &self,
        runner_id: i64,
        job_id: i64,
        epoch: i64,
    ) -> std::result::Result<JobRow, Reject> {
        let now = self.clock.now_ms();
        let ev = Event::Touch { runner_id, epoch };
        self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<JobRow, Reject>(d.job)
        })
    }

    /// Сколько байт результата сервер уже принял в эту эпоху (для `HEAD`): после ограждения - длина `.part`.
    pub fn result_offset(
        &self,
        runner_id: i64,
        job_id: i64,
        epoch: i64,
    ) -> std::result::Result<u64, Reject> {
        self.touch(runner_id, job_id, epoch)?;
        self.blobs
            .part_len(job_id, epoch)
            .map_err(|e| Reject::from(format!("задача #{job_id}: длина .part: {e}")))
    }

    /// Демка задачи для скачивания раннером: ограждение, продление аренды и строка файла (нет демки - 404).
    pub fn demo_for(
        &self,
        runner_id: i64,
        job_id: i64,
        epoch: i64,
    ) -> std::result::Result<FileRow, Reject> {
        self.touch(runner_id, job_id, epoch)?;
        self.demo_file(job_id)?
            .ok_or_else(|| Reject::new(codes::NOT_FOUND, "у задачи нет демки"))
    }

    /// Прогресс раннера (он же heartbeat): продлевает аренду и двигает состояние. Возвращает ответ и признак того,
    /// что раннер вернул себе задачу с потерянной арендой (вызывающий пишет это в журнал).
    pub fn progress(
        &self,
        runner_id: i64,
        job_id: i64,
        req: &ProgressRequest,
    ) -> std::result::Result<(ProgressReply, bool), Reject> {
        let now = self.clock.now_ms();
        let ev = Event::Progress {
            runner_id,
            epoch: req.epoch,
            seq: req.seq,
            phase: req.phase,
            take: req.take,
        };
        self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            let reply = ProgressReply {
                lease_until_ms: d.job.lease_until.unwrap_or(now),
                cancel: d.job.cancel_requested,
            };
            Ok((reply, d.reattached))
        })
    }

    /// Возвращает в очередь задачи с истёкшей арендой (или отменяет, если отмена была запрошена) и роняет
    /// зависший приём демки. Зовётся потоком `sweep` раз в 5 с; аренды истекают и лениво из `claim`.
    pub fn sweep(&self) -> Result<Sweep> {
        let now = self.clock.now_ms();
        let out = self.db.write(|tx| {
            let mut out = expire_leases(tx, now, &self.limits)?;
            out.receive_failed = fail_stale_receiving(tx, now, &self.limits)?;
            Ok::<Sweep, String>(out)
        })?;
        for id in &out.cancelled {
            let _ = self.blobs.discard_parts(*id);
        }
        if !out.requeued.is_empty() {
            self.wake();
        }
        Ok(out)
    }

    /// Будит всех, кто ждёт работу в long-poll: в очередь встала задача.
    pub fn wake(&self) {
        let (lock, cv) = &*self.wake;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        cv.notify_all();
    }

    /// Номер поколения «работа появилась»; брать до проверки очереди, чтобы не потерять пробуждение.
    pub fn work_generation(&self) -> u64 {
        *self.wake.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ждёт, пока поколение не уйдёт от `seen`, но не дольше `timeout`.
    pub fn wait_for_work_since(&self, seen: u64, timeout: Duration) {
        let (lock, cv) = &*self.wake;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = cv
            .wait_timeout_while(guard, timeout, |g| *g == seen)
            .unwrap_or_else(|e| e.into_inner());
    }

    /// Ждёт любой `wake` или `timeout`.
    pub fn wait_for_work(&self, timeout: Duration) {
        self.wait_for_work_since(self.work_generation(), timeout);
    }
}

/// Выдача для раннера по строке задачи и её демке.
fn assignment_of(c: &Connection, job: &JobRow) -> Result<Assignment> {
    let id = job.id;
    let demo = match demo_by_job(c, id)? {
        Some(d) => DemoInfo {
            size: d.size,
            sha256: d.sha256,
            orig_name: job.orig_name.clone(),
        },
        // Матч FACEIT: файла на сервере нет, раннер скачивает демку сам по `faceit_link`.
        None if job.source_kind == "faceit" => DemoInfo {
            size: 0,
            sha256: String::new(),
            orig_name: job.orig_name.clone(),
        },
        None => return Err(format!("у задачи #{id} в очереди нет демки")),
    };
    Ok(Assignment {
        job_id: id,
        epoch: job.epoch,
        lease_until_ms: job.lease_until.unwrap_or_default(),
        demo,
        faceit_link: job.faceit_link.clone(),
        player: Player {
            steam64: job.player_steam.clone(),
            name: job.player_name.clone(),
        },
        requested_by: job.requested_by.clone(),
        options: serde_json::Map::new(),
    })
}

fn not_found() -> Reject {
    Reject::new(codes::NOT_FOUND, "нет такой задачи")
}

/// Итог `recover`: упавшие приёмы и число продлённых аренд.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovered {
    pub receive_failed: Vec<i64>,
    pub grace_extended: usize,
    /// Сколько недозагруженных кусков `*.part` окончательных задач удалено.
    pub parts_discarded: usize,
}

/// Итог `claim_report`: выдача (если есть) и задачи, упавшие с `TOO_MANY_GRANTS` по дороге.
#[derive(Debug, Clone)]
pub struct Claimed {
    pub assignment: Option<Assignment>,
    pub failed: Vec<i64>,
}

/// Итог `finish_report`: ответ раннеру и ошибка удаления кусков результата, если она была.
#[derive(Debug, Clone)]
pub struct Finished {
    pub reply: FinishReply,
    pub discard_error: Option<String>,
}

/// Что сделал `sweep`: id задач по исходам.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Аренда истекла, задача снова в очереди.
    pub requeued: Vec<i64>,
    /// Аренда истекла у задачи с запрошенной отменой — она отменена.
    pub cancelled: Vec<i64>,
    /// Приём демки завис дольше предела — задача упала.
    pub receive_failed: Vec<i64>,
}

/// Записывает изменённую строку задачи: все изменяемые колонки разом, `updated_at = now`.
fn apply(tx: &rusqlite::Transaction<'_>, before: &JobRow, after: &JobRow, now: i64) -> Result<()> {
    if before == after {
        return Ok(());
    }
    tx.execute(
        "UPDATE jobs SET state = ?2, cancel_requested = ?3, epoch = ?4, grants = ?5, runner_id = ?6, lease_until = ?7,          lease_lost = ?8, last_seq = ?9, phase = ?10, take_n = ?11, take_m = ?12, fail_code = ?13, fail_class = ?14,          fail_state = ?15, fail_hint = ?16, fail_message = ?17, fail_tail = ?18, fail_elapsed_ms = ?19,          finished_at = ?20, updated_at = ?21 WHERE id = ?1",
        params![
            after.id,
            after.state.as_str(),
            after.cancel_requested.as_str(),
            after.epoch,
            after.grants,
            after.runner_id,
            after.lease_until,
            after.lease_lost as i64,
            after.last_seq,
            after.phase,
            after.take_n,
            after.take_m,
            after.fail_code,
            after.fail_class,
            after.fail_state,
            after.fail_hint,
            after.fail_message,
            after.fail_tail,
            after.fail_elapsed_ms,
            after.finished_at,
            now,
        ],
    )
    .map_err(db_err)?;
    Ok(())
}

/// Приёмы демки без движения дольше `RECEIVING_STALE_MS` падают с `RECEIVE_FAILED`.
fn fail_stale_receiving(
    tx: &rusqlite::Transaction<'_>,
    now: i64,
    limits: &Limits,
) -> Result<Vec<i64>> {
    let ids: Vec<i64> = {
        let mut st = tx
            .prepare(
                "SELECT id FROM jobs WHERE state = 'receiving' AND updated_at < ?1 ORDER BY id",
            )
            .map_err(db_err)?;
        let rows = st
            .query_map([now - rules::RECEIVING_STALE_MS], |r| r.get(0))
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(db_err)?
    };
    let mut failed = Vec::new();
    for id in ids {
        let Some(before) = job_by_id(tx, id)? else {
            continue;
        };
        let d = rules::decide(&before, &Event::ReceiveStale, now, limits).map_err(|r| r.message)?;
        if d.job != before {
            apply(tx, &before, &d.job, now)?;
            failed.push(id);
        }
    }
    Ok(failed)
}

/// Истекшие аренды через `rules::decide(Expire)`. Общая для `sweep` и ленивого истечения в начале `claim`.
fn expire_leases(tx: &rusqlite::Transaction<'_>, now: i64, limits: &Limits) -> Result<Sweep> {
    let ids: Vec<i64> = {
        let mut st = tx
            .prepare(
                "SELECT id FROM jobs WHERE state IN ('assigned','running','uploading')                  AND (lease_until IS NULL OR lease_until <= ?1) ORDER BY id",
            )
            .map_err(db_err)?;
        let rows = st.query_map([now], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(db_err)?
    };
    let mut out = Sweep::default();
    for id in ids {
        let Some(before) = job_by_id(tx, id)? else {
            continue;
        };
        let d = rules::decide(&before, &Event::Expire, now, limits).map_err(|r| r.message)?;
        apply(tx, &before, &d.job, now)?;
        match d.job.state {
            JobState::Queued => out.requeued.push(id),
            JobState::Cancelled => out.cancelled.push(id),
            _ => {}
        }
    }
    Ok(out)
}

/// Принятый результат задачи (строка `files` с `kind = 'result'`, не удалённая).
fn result_by_job(c: &Connection, job_id: i64) -> Result<Option<FileRow>> {
    c.query_row(
        "SELECT rel_path, size, sha256 FROM files WHERE job_id = ?1 AND kind = 'result' AND deleted_at IS NULL ORDER BY id DESC LIMIT 1",
        [job_id],
        |r| Ok(FileRow { rel_path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, sha256: r.get(2)? }),
    )
    .optional()
    .map_err(db_err)
}

fn demo_by_job(c: &Connection, job_id: i64) -> Result<Option<FileRow>> {
    c.query_row(
        "SELECT rel_path, size, sha256 FROM files WHERE job_id = ?1 AND kind = 'demo' AND deleted_at IS NULL ORDER BY id DESC LIMIT 1",
        [job_id],
        |r| Ok(FileRow { rel_path: r.get(0)?, size: r.get::<_, i64>(1)? as u64, sha256: r.get(2)? }),
    )
    .optional()
    .map_err(db_err)
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
    fn failure_fields_are_cleaned_and_capped() {
        let (core, d) = core("sanitize", Limits::default());
        let token = format!("cs2r_{}", "ab".repeat(32));
        let dirty = Failure {
            code: "hlae error".to_owned(),
            class: "к".repeat(100),
            state: "take".to_owned(),
            hint: format!("подсказка {token}"),
            message: format!("боль\u{7}шая {}", "я".repeat(3000)),
            tail: format!("a\nb\tc\u{1b}[0m {token}"),
            elapsed_ms: 5,
        };
        let c = core.sanitize_failure(&dirty);
        assert_eq!(c.code, "UNKNOWN", "код не из [A-Z0-9_]");
        assert_eq!(c.class.chars().count(), 32);
        assert_eq!(c.hint, "подсказка cs2r_***");
        assert!(!c.message.contains('\u{7}') && c.message.chars().count() == 2000);
        assert!(c.message.ends_with('…'));
        assert_eq!(c.tail, "a\nb\tc[0m cs2r_***");
        assert_eq!(c.elapsed_ms, 5);

        for (code, want) in [
            ("HLAE_ERROR", "HLAE_ERROR"),
            ("", "UNKNOWN"),
            ("E1", "E1"),
            (&"A".repeat(64), &"A".repeat(64)),
            (&"A".repeat(65), "UNKNOWN"),
            ("ёж", "UNKNOWN"),
        ] {
            let f = Failure {
                code: code.to_owned(),
                ..Default::default()
            };
            assert_eq!(core.sanitize_failure(&f).code, want);
        }
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
