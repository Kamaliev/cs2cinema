//! Ядро сервера: задачи и их файлы поверх базы, без HTTP. Время берётся из внедрённых часов.
//!
//! Правило: копирование и хеширование файлов идут вне мьютекса базы, затем одна короткая транзакция.

use crate::Result;
use crate::blobs::{Blobs, Ingested, demo_rel, sniff_demo};
use crate::clock::Clock;
use crate::db::{Db, db_err};
use crate::rules::{self, Event};
use crate::secret::Redactor;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use wire::{
    Assignment, CancelMode, DemoInfo, Directive, DirectiveAction, HelloReply, HelloRequest,
    JobState, Player, ProgressReply, ProgressRequest, ReleaseRequest, codes,
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
}

impl Reject {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            offset: None,
            detail: None,
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
                self.wake();
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
        self.db.read(|c| demo_by_job(c, job_id))
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

            let poisoned: Vec<(i64, i64)> = {
                let mut st = tx
                    .prepare("SELECT id, grants FROM jobs WHERE state = 'queued' AND grants >= ?1 ORDER BY id")
                    .map_err(db_err)?;
                let rows = st
                    .query_map([max], |r| Ok((r.get(0)?, r.get(1)?)))
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
        self.db.write(|tx| {
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
            })
        })
    }

    /// Раннер возвращает задачу в очередь (`release`): окружение не готово, раннер уходит и т. п.
    /// `counts = false` возвращает и одну выдачу. Возврат чужой выдачи — `stale_epoch`.
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
        self.db.write(|tx| {
            let before = job_by_id(tx, job_id)?.ok_or_else(not_found)?;
            let d = rules::decide(&before, &ev, now, &self.limits)?;
            apply(tx, &before, &d.job, now)?;
            Ok::<(), Reject>(())
        })?;
        self.wake();
        Ok(())
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
    let demo = demo_by_job(c, id)?.ok_or_else(|| format!("у задачи #{id} в очереди нет демки"))?;
    Ok(Assignment {
        job_id: id,
        epoch: job.epoch,
        lease_until_ms: job.lease_until.unwrap_or_default(),
        demo: DemoInfo {
            size: demo.size,
            sha256: demo.sha256,
            orig_name: job.orig_name.clone(),
        },
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
}

/// Итог `claim_report`: выдача (если есть) и задачи, упавшие с `TOO_MANY_GRANTS` по дороге.
#[derive(Debug, Clone)]
pub struct Claimed {
    pub assignment: Option<Assignment>,
    pub failed: Vec<i64>,
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
