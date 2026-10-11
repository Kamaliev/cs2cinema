//! Чистый автомат задачи: строка + событие + `now_ms` + `Limits` → новая строка или отказ (D-05).
//!
//! Ни базы, ни часов, ни HTTP: ядро (`Core`) грузит строку, зовёт `decide` и записывает изменённые колонки
//! одной транзакцией. Выдача задачи раннеру — единственное исключение: её делает один SQL-шаг в `Core::claim`,
//! потому что выбор «самая старая в очереди» и запись должны быть неделимы.
//!
//! Главные правила: любое изменение от раннера ограждено парой `runner_id` + `epoch` (иначе `stale_epoch`);
//! истечение аренды — не сбой, а обратимая потеря (задача в очереди с `lease_lost`, эпоха и число выдач целы),
//! и тот же раннер с той же эпохой возвращает задачу себе.

use crate::core::{JobRow, Limits, Reject};
use wire::{CancelMode, Failure, JobState, Phase, Take, codes};

/// Приём демки, который не двигался дольше этого, считается оборванным: 30 минут.
pub const RECEIVING_STALE_MS: i64 = 30 * 60 * 1000;

/// Вид итога, который раннер сообщает в `finish`. `Failure` приходит уже очищенным (`Core::sanitize_failure`).
#[derive(Debug, Clone, PartialEq)]
pub enum FinishKind {
    /// Результат загружен и проверен (размер и SHA-256 файла проверяет `Core::finish_done` до записи состояния).
    Done,
    Failed(Failure),
    Cancelled,
}

impl FinishKind {
    /// Состояние, в которое приводит этот итог.
    fn target(&self) -> JobState {
        match self {
            FinishKind::Done => JobState::Done,
            FinishKind::Failed(_) => JobState::Failed,
            FinishKind::Cancelled => JobState::Cancelled,
        }
    }
}

/// Событие, меняющее задачу.
#[derive(Debug, Clone)]
pub enum Event {
    /// Прогресс раннера: он же heartbeat.
    Progress {
        runner_id: i64,
        epoch: i64,
        seq: u64,
        phase: Phase,
        take: Option<Take>,
    },
    /// Проверка срока аренды (по часам сервера).
    Expire,
    /// `hello`: раннер сверяет удерживаемую задачу; то же, что прогресс, но этап берётся из сохранённой колонки.
    Reattach { runner_id: i64, epoch: i64 },
    /// Перезапуск сервера: аренды живых раннеров продлеваются как минимум до `grace_until`.
    Recover { grace_until: i64 },
    /// Приём демки завис: задача в `receiving` без движения дольше `RECEIVING_STALE_MS`.
    ReceiveStale,
    /// Раннер возвращает задачу в очередь; `counts = false` возвращает и выдачу.
    Release {
        runner_id: i64,
        epoch: i64,
        counts: bool,
    },
    /// Раннер прислал кусок результата: `assigned|running` -> `uploading`, аренда продлевается (загрузка - признак
    /// жизни раннера); потерянная аренда возвращается.
    Chunk { runner_id: i64, epoch: i64 },
    /// `HEAD` результата и скачивание демки: аренда продлевается без смены состояния; потерянная аренда
    /// возвращается по сохранённому этапу (как `Reattach`).
    Touch { runner_id: i64, epoch: i64 },
    /// Раннер сообщает итог: сбой, отмену или готовый результат.
    Finish {
        runner_id: i64,
        epoch: i64,
        kind: FinishKind,
    },
    /// Оператор отменяет задачу: в приёме и в очереди сразу, у выданной ставится флаг (`force` — жёсткий).
    Cancel { force: bool },
    /// Демка принята и записана: `receiving` → `queued`.
    Received,
    /// Приём демки не удался: `receiving` → `failed` с кодом `RECEIVE_FAILED`.
    ReceiveFailed { message: String },
}

/// Результат `decide`: новая строка и пометки для вызывающего.
#[derive(Debug, Clone)]
pub struct Decision {
    pub job: JobRow,
    /// Раннер вернул себе задачу, у которой была потеряна аренда.
    pub reattached: bool,
    /// Недозагруженные куски результата нужно выбросить.
    pub discard_parts: bool,
}

impl Decision {
    fn of(job: JobRow) -> Self {
        Self {
            job,
            reattached: false,
            discard_parts: false,
        }
    }
}

/// Строка принадлежит этому раннеру и этой выдаче.
fn fenced(job: &JobRow, runner_id: i64, epoch: i64) -> bool {
    job.runner_id == Some(runner_id) && job.epoch == epoch
}

fn stale() -> Reject {
    Reject::new(
        codes::STALE_EPOCH,
        "задача выдана заново или отозвана — бросьте её",
    )
}

fn terminal() -> Reject {
    Reject::new(codes::JOB_TERMINAL, "задача уже завершена")
}

/// Приём демки не удался: `failed` с кодом `RECEIVE_FAILED`.
fn receive_failed(job: &JobRow, message: &str, now_ms: i64) -> JobRow {
    let mut j = job.clone();
    j.state = JobState::Failed;
    j.fail_code = Some(wire::fail::RECEIVE_FAILED.to_owned());
    j.fail_message = Some(message.to_owned());
    j.finished_at = Some(now_ms);
    j
}

/// Событие приёма на задаче не в приёме — ошибка вызывающего кода, а не раннера.
fn not_receiving(job: &JobRow) -> Reject {
    Reject::new(
        codes::INTERNAL,
        format!("задача #{} не в состоянии приёма", job.id),
    )
}

/// Состояние после возврата аренды по этапу.
fn state_for_phase(phase: Phase) -> JobState {
    match phase {
        Phase::Fetch => JobState::Assigned,
        Phase::Upload => JobState::Uploading,
        _ => JobState::Running,
    }
}

/// Применяет событие к строке задачи.
pub fn decide(job: &JobRow, ev: &Event, now_ms: i64, limits: &Limits) -> Result<Decision, Reject> {
    match ev {
        Event::Progress {
            runner_id,
            epoch,
            seq,
            phase,
            take,
        } => {
            if job.state.is_terminal() {
                return Err(terminal());
            }
            let leased = job.state.is_leased();
            let lost = job.state == JobState::Queued && job.lease_lost;
            if !(leased || lost) || !fenced(job, *runner_id, *epoch) {
                return Err(stale());
            }
            let mut j = job.clone();
            let fresh = *seq as i64 > job.last_seq;
            if fresh {
                j.last_seq = *seq as i64;
                j.phase = phase.as_str().to_owned();
                j.take_n = take.map(|t| i64::from(t.n));
                j.take_m = take.map(|t| i64::from(t.m));
            }
            if lost {
                // Возврат потерянной аренды: эпоха и число выдач не меняются.
                j.state = state_for_phase(*phase);
                j.lease_lost = false;
            } else if fresh {
                j.state = match (j.state, phase) {
                    (JobState::Uploading, _) | (_, Phase::Upload) => JobState::Uploading,
                    (JobState::Assigned, Phase::Fetch) => JobState::Assigned,
                    (JobState::Assigned, _) => JobState::Running,
                    (s, _) => s,
                };
            }
            j.lease_until = Some(now_ms + limits.lease_ttl_ms);
            Ok(Decision {
                job: j,
                reattached: lost,
                discard_parts: false,
            })
        }
        Event::Release {
            runner_id,
            epoch,
            counts,
        } => {
            if job.state.is_terminal() {
                return Err(terminal());
            }
            let lost = job.state == JobState::Queued && job.lease_lost;
            if !(job.state.is_leased() || lost) || !fenced(job, *runner_id, *epoch) {
                return Err(stale());
            }
            let mut j = job.clone();
            j.runner_id = None;
            j.lease_until = None;
            j.lease_lost = false;
            j.phase = String::new();
            j.take_n = None;
            j.take_m = None;
            if job.cancel_requested != CancelMode::NotRequested {
                // Раннер вернул задачу, а отмена уже запрошена: задача окончена, в очередь она не возвращается (WR-01).
                // Число выдач не трогаем: отменённую задачу никто больше не выдаёт.
                j.state = JobState::Cancelled;
                j.finished_at = Some(now_ms);
                return Ok(Decision {
                    job: j,
                    reattached: false,
                    discard_parts: true,
                });
            }
            j.state = JobState::Queued;
            if !*counts {
                j.grants = (j.grants - 1).max(0);
            }
            Ok(Decision::of(j))
        }
        Event::Finish {
            runner_id,
            epoch,
            kind,
        } => {
            let fenced = fenced(job, *runner_id, *epoch);
            if job.state.is_terminal() {
                // Повтор того же итога идемпотентен (ответ раннеру мог потеряться), любой другой — отказ.
                return if fenced && job.state == kind.target() {
                    Ok(Decision::of(job.clone()))
                } else {
                    Err(terminal())
                };
            }
            let lost = job.state == JobState::Queued && job.lease_lost;
            if !(job.state.is_leased() || lost) || !fenced {
                return Err(stale());
            }
            let mut j = job.clone();
            match kind {
                FinishKind::Done => {
                    if !(job.state == JobState::Uploading || lost) {
                        let mut r =
                            Reject::new(codes::OFFSET_MISMATCH, "результат ещё не загружен");
                        r.offset = Some(0);
                        return Err(r);
                    }
                    j.state = JobState::Done;
                }
                FinishKind::Failed(f) => {
                    j.state = JobState::Failed;
                    j.fail_code = Some(f.code.clone());
                    j.fail_class = Some(f.class.clone());
                    j.fail_state = Some(f.state.clone());
                    j.fail_hint = Some(f.hint.clone());
                    j.fail_message = Some(f.message.clone());
                    j.fail_tail = Some(f.tail.clone());
                    j.fail_elapsed_ms = Some(i64::try_from(f.elapsed_ms).unwrap_or(i64::MAX));
                }
                FinishKind::Cancelled => j.state = JobState::Cancelled,
            }
            j.finished_at = Some(now_ms);
            j.lease_until = None;
            j.lease_lost = false;
            Ok(Decision {
                job: j,
                reattached: false,
                discard_parts: true,
            })
        }
        Event::Chunk { runner_id, epoch } => {
            if job.state.is_terminal() {
                return Err(terminal());
            }
            let lost = job.state == JobState::Queued && job.lease_lost;
            if !(job.state.is_leased() || lost) || !fenced(job, *runner_id, *epoch) {
                return Err(stale());
            }
            let mut j = job.clone();
            j.state = JobState::Uploading;
            j.lease_until = Some(now_ms + limits.lease_ttl_ms);
            j.lease_lost = false;
            j.phase = Phase::Upload.as_str().to_owned();
            Ok(Decision {
                job: j,
                reattached: lost,
                discard_parts: false,
            })
        }
        Event::Touch { runner_id, epoch } => decide(
            job,
            &Event::Reattach {
                runner_id: *runner_id,
                epoch: *epoch,
            },
            now_ms,
            limits,
        ),
        Event::Reattach { runner_id, epoch } => {
            if job.state.is_terminal() {
                return Err(terminal());
            }
            let lost = job.state == JobState::Queued && job.lease_lost;
            if !(job.state.is_leased() || lost) || !fenced(job, *runner_id, *epoch) {
                return Err(stale());
            }
            let mut j = job.clone();
            j.lease_until = Some(now_ms + limits.lease_ttl_ms);
            if lost {
                j.state = match Phase::parse(&job.phase) {
                    None | Some(Phase::Fetch) => JobState::Assigned,
                    Some(Phase::Upload) => JobState::Uploading,
                    Some(_) => JobState::Running,
                };
                j.lease_lost = false;
            }
            Ok(Decision {
                job: j,
                reattached: lost,
                discard_parts: false,
            })
        }
        Event::Recover { grace_until } => {
            let mut j = job.clone();
            if job.state.is_leased() {
                // Сервер после перезапуска не возвращает в очередь задачи живого раннера (PITFALLS P3.4).
                j.lease_until = Some(job.lease_until.unwrap_or(0).max(*grace_until));
            }
            Ok(Decision::of(j))
        }
        Event::ReceiveStale => {
            if job.state == JobState::Receiving && job.updated_at < now_ms - RECEIVING_STALE_MS {
                return Ok(Decision::of(receive_failed(
                    job,
                    "приём демки прервался (сервер или submit остановлены) — отправьте демку заново",
                    now_ms,
                )));
            }
            Ok(Decision::of(job.clone()))
        }
        Event::Received => {
            if job.state != JobState::Receiving {
                return Err(not_receiving(job));
            }
            let mut j = job.clone();
            j.state = JobState::Queued;
            Ok(Decision::of(j))
        }
        Event::ReceiveFailed { message } => {
            if job.state != JobState::Receiving {
                return Err(not_receiving(job));
            }
            Ok(Decision::of(receive_failed(job, message, now_ms)))
        }
        Event::Cancel { force } => match job.state {
            JobState::Done | JobState::Failed | JobState::Cancelled => Err(terminal()),
            JobState::Receiving | JobState::Queued => {
                // Раннер не работает (или потерял аренду): отменяем сразу, раннер узнает по `job_terminal`/`abandon`.
                let mut j = job.clone();
                j.state = JobState::Cancelled;
                j.finished_at = Some(now_ms);
                j.lease_until = None;
                j.lease_lost = false;
                Ok(Decision {
                    job: j,
                    reattached: false,
                    discard_parts: true,
                })
            }
            JobState::Assigned | JobState::Running | JobState::Uploading => {
                // Выдана раннеру: ставится флаг, раннер увидит его в ответе на прогресс и в `hello`.
                // Повторная отмена (или `--force`) ужесточает флаг до жёсткого.
                let mut j = job.clone();
                j.cancel_requested = if *force || job.cancel_requested != CancelMode::NotRequested {
                    CancelMode::Force
                } else {
                    CancelMode::Soft
                };
                Ok(Decision::of(j))
            }
            JobState::Unknown => Err(Reject::new(
                codes::INTERNAL,
                "у задачи неизвестное состояние",
            )),
        },
        Event::Expire => {
            let overdue = job.state.is_leased() && job.lease_until.is_none_or(|l| l <= now_ms);
            if !overdue {
                return Ok(Decision::of(job.clone()));
            }
            let mut j = job.clone();
            j.lease_until = None;
            if job.cancel_requested != CancelMode::NotRequested {
                j.state = JobState::Cancelled;
                j.finished_at = Some(now_ms);
                return Ok(Decision {
                    job: j,
                    reattached: false,
                    discard_parts: true,
                });
            }
            // Истечение — не сбой: сервер не роняет задачу из-за молчания раннера.
            j.state = JobState::Queued;
            j.lease_lost = true;
            Ok(Decision::of(j))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn row(state: JobState) -> JobRow {
        JobRow {
            id: 1,
            user_id: None,
            requested_by: String::new(),
            state,
            cancel_requested: CancelMode::NotRequested,
            epoch: 1,
            grants: 1,
            runner_id: Some(7),
            lease_until: Some(NOW + 1_000),
            lease_lost: false,
            last_seq: 0,
            source_kind: "file".to_owned(),
            orig_name: String::new(),
            faceit_link: None,
            player_steam: None,
            player_name: "s1mple".to_owned(),
            phase: String::new(),
            take_n: None,
            take_m: None,
            fail_code: None,
            fail_class: None,
            fail_state: None,
            fail_hint: None,
            fail_message: None,
            fail_tail: None,
            fail_elapsed_ms: None,
            created_at: NOW - 10,
            updated_at: NOW - 10,
            finished_at: None,
        }
    }

    fn progress(runner_id: i64, epoch: i64, seq: u64, phase: Phase) -> Event {
        Event::Progress {
            runner_id,
            epoch,
            seq,
            phase,
            take: None,
        }
    }

    #[test]
    fn expire_requeues_only_an_overdue_lease() {
        let limits = Limits::default();
        let live = row(JobState::Running);
        let d = decide(&live, &Event::Expire, NOW, &limits).unwrap();
        assert_eq!(d.job, live, "срок в будущем — без изменений");

        let mut late = row(JobState::Running);
        late.lease_until = Some(NOW - 1);
        let d = decide(&late, &Event::Expire, NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Queued);
        assert!(d.job.lease_lost && d.job.lease_until.is_none());
        assert_eq!(
            (d.job.epoch, d.job.grants, d.job.runner_id),
            (1, 1, Some(7))
        );
        assert!(!d.discard_parts);

        let queued = row(JobState::Queued);
        let d = decide(&queued, &Event::Expire, NOW, &limits).unwrap();
        assert_eq!(d.job, queued, "задача в очереди не истекает");

        late.cancel_requested = CancelMode::Soft;
        let d = decide(&late, &Event::Expire, NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Cancelled);
        assert_eq!(d.job.finished_at, Some(NOW));
        assert!(d.discard_parts);
    }

    #[test]
    fn progress_of_a_foreign_runner_is_a_stale_epoch() {
        let limits = Limits::default();
        let job = row(JobState::Running);
        for (runner, epoch) in [(8, 1), (7, 2), (7, 0)] {
            let e = decide(
                &job,
                &progress(runner, epoch, 1, Phase::Record),
                NOW,
                &limits,
            )
            .unwrap_err();
            assert_eq!(e.code, codes::STALE_EPOCH, "runner {runner} epoch {epoch}");
        }
        let done = row(JobState::Done);
        let e = decide(&done, &progress(7, 1, 1, Phase::Record), NOW, &limits).unwrap_err();
        assert_eq!(e.code, codes::JOB_TERMINAL);
        let queued = row(JobState::Queued);
        let e = decide(&queued, &progress(7, 1, 1, Phase::Record), NOW, &limits).unwrap_err();
        assert_eq!(e.code, codes::STALE_EPOCH, "в очереди без потери аренды");
    }

    #[test]
    fn progress_moves_assigned_to_running_and_upload_to_uploading() {
        let limits = Limits::default();
        let job = row(JobState::Assigned);
        let d = decide(&job, &progress(7, 1, 1, Phase::Fetch), NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Assigned, "fetch не начинает работу");
        assert_eq!(d.job.lease_until, Some(NOW + 45_000));
        let d = decide(&d.job, &progress(7, 1, 2, Phase::Record), NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Running);
        assert_eq!(d.job.phase, "record");
        let d = decide(&d.job, &progress(7, 1, 3, Phase::Upload), NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Uploading);
        let d = decide(&d.job, &progress(7, 1, 4, Phase::Record), NOW, &limits).unwrap();
        assert_eq!(
            d.job.state,
            JobState::Uploading,
            "из загрузки назад не уходят"
        );
    }

    #[test]
    fn reattach_restores_the_state_from_the_stored_phase() {
        let limits = Limits::default();
        let mut job = row(JobState::Queued);
        job.lease_lost = true;
        job.lease_until = None;
        for (phase, want) in [
            ("", JobState::Assigned),
            ("fetch", JobState::Assigned),
            ("record", JobState::Running),
            ("upload", JobState::Uploading),
        ] {
            job.phase = phase.to_owned();
            let ev = Event::Reattach {
                runner_id: 7,
                epoch: 1,
            };
            let d = decide(&job, &ev, NOW, &limits).unwrap();
            assert!(d.reattached && !d.job.lease_lost, "{phase}");
            assert_eq!(d.job.state, want, "{phase}");
            assert_eq!(d.job.lease_until, Some(NOW + 45_000));
        }
        let ev = Event::Reattach {
            runner_id: 7,
            epoch: 2,
        };
        assert_eq!(
            decide(&job, &ev, NOW, &limits).unwrap_err().code,
            codes::STALE_EPOCH
        );
    }

    #[test]
    fn recover_extends_leases_and_receive_stale_fails_old_receiving() {
        let limits = Limits::default();
        let grace = NOW + 90_000;
        let running = row(JobState::Running);
        let d = decide(
            &running,
            &Event::Recover { grace_until: grace },
            NOW,
            &limits,
        )
        .unwrap();
        assert_eq!(d.job.lease_until, Some(grace));
        let mut far = row(JobState::Running);
        far.lease_until = Some(grace + 5);
        let d = decide(&far, &Event::Recover { grace_until: grace }, NOW, &limits).unwrap();
        assert_eq!(d.job.lease_until, Some(grace + 5), "срок не укорачивается");

        let mut recv = row(JobState::Receiving);
        recv.updated_at = NOW - RECEIVING_STALE_MS - 1;
        let d = decide(&recv, &Event::ReceiveStale, NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Failed);
        assert_eq!(d.job.fail_code.as_deref(), Some("RECEIVE_FAILED"));
        recv.updated_at = NOW - RECEIVING_STALE_MS + 1;
        let d = decide(&recv, &Event::ReceiveStale, NOW, &limits).unwrap();
        assert_eq!(d.job.state, JobState::Receiving);
    }

    #[test]
    fn release_refund_never_goes_below_zero() {
        let limits = Limits::default();
        let mut job = row(JobState::Running);
        job.grants = 0;
        let ev = Event::Release {
            runner_id: 7,
            epoch: 1,
            counts: false,
        };
        assert_eq!(decide(&job, &ev, NOW, &limits).unwrap().job.grants, 0);
    }

    #[test]
    fn release_of_a_job_with_a_requested_cancel_cancels_it() {
        let limits = Limits::default();
        for mode in [CancelMode::Soft, CancelMode::Force] {
            for counts in [true, false] {
                let mut job = row(JobState::Running);
                job.cancel_requested = mode;
                job.phase = "record".to_owned();
                let ev = Event::Release {
                    runner_id: 7,
                    epoch: 1,
                    counts,
                };
                let d = decide(&job, &ev, NOW, &limits).unwrap();
                assert_eq!(d.job.state, JobState::Cancelled, "{mode:?} counts={counts}");
                assert_eq!(d.job.finished_at, Some(NOW));
                assert_eq!((d.job.runner_id, d.job.lease_until), (None, None));
                assert!(!d.job.lease_lost);
                assert!(d.discard_parts);
                assert_eq!(d.job.grants, job.grants, "число выдач не меняется");
            }
        }
        // Чужая эпоха по-прежнему stale, окончательная задача — terminal.
        let mut job = row(JobState::Running);
        job.cancel_requested = CancelMode::Soft;
        let other = Event::Release {
            runner_id: 7,
            epoch: 2,
            counts: true,
        };
        assert_eq!(
            decide(&job, &other, NOW, &limits).unwrap_err().code,
            codes::STALE_EPOCH
        );
        let done = row(JobState::Done);
        let ev = Event::Release {
            runner_id: 7,
            epoch: 1,
            counts: true,
        };
        assert!(decide(&done, &ev, NOW, &limits).is_err());
    }

    #[test]
    fn lost_lease_returns_to_the_same_runner_without_a_new_epoch() {
        let limits = Limits::default();
        let mut job = row(JobState::Queued);
        job.lease_lost = true;
        job.lease_until = None;
        job.last_seq = 2;
        let d = decide(&job, &progress(7, 1, 3, Phase::Record), NOW, &limits).unwrap();
        assert!(d.reattached && !d.job.lease_lost);
        assert_eq!(d.job.state, JobState::Running);
        assert_eq!((d.job.epoch, d.job.grants), (1, 1));
        let e = decide(&job, &progress(8, 1, 3, Phase::Record), NOW, &limits).unwrap_err();
        assert_eq!(e.code, codes::STALE_EPOCH);
    }
}
