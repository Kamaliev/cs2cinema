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
use wire::{CancelMode, JobState, Phase, Take, codes};

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
    /// Раннер возвращает задачу в очередь; `counts = false` возвращает и выдачу.
    Release {
        runner_id: i64,
        epoch: i64,
        counts: bool,
    },
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
            j.state = JobState::Queued;
            j.runner_id = None;
            j.lease_until = None;
            j.lease_lost = false;
            j.phase = String::new();
            j.take_n = None;
            j.take_m = None;
            if !*counts {
                j.grants = (j.grants - 1).max(0);
            }
            Ok(Decision::of(j))
        }
        Event::Expire => {
            let overdue = job.state.is_leased() && job.lease_until.is_none_or(|l| l < now_ms);
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
