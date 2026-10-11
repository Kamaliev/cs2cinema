//! Политика итогов раннера: класс сбоя решает `release` или `finish`; две одинаковые неисправности подряд —
//! пауза; чистая логика без сети и часов, как `QueuePolicy` приложения.
//!
//! Сбой «окружения» или «запуска» — неисправность машины автора, а не демки друга: задача возвращается в очередь
//! через `release`, и каждая такая выдача засчитывается (D-02 буквально). Бесконечный круг останавливают две
//! вещи: пауза раннера после двух одинаковых сбоев подряд и потолок выдач на сервере (`TOO_MANY_GRANTS`).
//! Всё остальное (контентные коды и ошибка без кода) закрывает задачу своим кодом и серию паузы сбрасывает.

use pipeline::job::Outcome;
use pipeline::record::failure::{FailClass, classify};
use pipeline::record::log::one_line;
use pipeline::{FailCode, RunError};
use wire::{CancelMode, Failure, fail};

/// Сколько символов сообщения о сбое уходит на сервер.
const MESSAGE_MAX: usize = 2000;
/// Сколько символов хвоста консоли уходит на сервер (берётся конец: он важнее).
const TAIL_MAX: usize = 2000;
/// Сколько символов сообщения попадает в причину `release`.
const REASON_MAX: usize = 200;
/// Сколько одинаковых сбоев подряд ставят раннер на паузу.
const PAUSE_AFTER: u32 = 2;

/// Что раннер делает с задачей по итогу исполнителя.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// Ролик готов: дальше загрузка результата.
    Done(Outcome),
    /// Закрыть задачу сбоем со своим кодом.
    Fail(Failure),
    /// Вернуть задачу в очередь: неисправность не по вине демки.
    Release {
        /// Причина для журнала сервера: `{код}: {сообщение}`.
        reason: String,
        code: String,
        class: FailClass,
        /// Сообщение и подсказка нужны паузе, чтобы объяснить оператору, что случилось.
        message: String,
        hint: String,
    },
    /// Закрыть задачу итогом `Cancelled`.
    Cancel,
    /// Остановка раннера: задача остаётся в состоянии и продолжится после перезапуска.
    Keep,
    /// Задача потеряна (сервер отозвал выдачу): ничего не закрывать.
    Abandon,
}

/// Обстоятельства, при которых закончился исполнитель.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flags {
    /// Какую отмену запросил сервер.
    pub server_cancel: CancelMode,
    /// Раннер остановлен локально (Ctrl+C, консоль): это не отмена.
    pub local_stop: bool,
    /// Задача потеряна, пока шёл исполнитель.
    pub abandoned: bool,
    /// Сколько миллисекунд прошло от начала задачи.
    pub elapsed_ms: u64,
}

impl Flags {
    /// Ничего особенного не случилось.
    pub fn quiet(elapsed_ms: u64) -> Flags {
        Flags {
            server_cancel: CancelMode::NotRequested,
            local_stop: false,
            abandoned: false,
            elapsed_ms,
        }
    }

    fn server_cancel(&self) -> bool {
        self.server_cancel != CancelMode::NotRequested
    }
}

/// Первые `max` символов строки.
fn head(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Последние `max` символов строки.
fn tail_of(s: &str, max: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(max)).collect()
}

/// Итог для сервера по ошибке исполнителя: сбой записи — со своим кодом, классом и состоянием автомата, ошибка
/// без описания — `JOB_ERROR`. Весь текст проходит `one_line` (без управляющих символов, профиля и секретов).
pub fn failure_of(e: &RunError, elapsed_ms: u64) -> Failure {
    match &e.fail {
        Some(f) => Failure {
            code: f.code.as_str().to_owned(),
            class: classify(f.code, f.state).title().to_owned(),
            state: f.state.code().to_owned(),
            hint: one_line(&f.hint),
            message: head(&one_line(&f.message), MESSAGE_MAX),
            tail: tail_of(
                &f.console_tail
                    .iter()
                    .map(|line| one_line(line))
                    .collect::<Vec<_>>()
                    .join("\n"),
                TAIL_MAX,
            ),
            elapsed_ms,
        },
        None => Failure {
            code: fail::JOB_ERROR.to_owned(),
            message: head(&one_line(&e.message), MESSAGE_MAX),
            elapsed_ms,
            ..Default::default()
        },
    }
}

/// Решение по итогу исполнителя. Порядок проверок: потеря задачи, успех, отмена, класс сбоя.
pub fn dispose(run: &Result<Outcome, RunError>, flags: Flags) -> Disposition {
    if flags.abandoned {
        return Disposition::Abandon;
    }
    let e = match run {
        Ok(outcome) => return Disposition::Done(outcome.clone()),
        Err(e) => e,
    };
    let cancelled = e.message == pipeline::CANCELLED
        || e.fail
            .as_ref()
            .is_some_and(|f| f.code == FailCode::Cancelled);
    // Остановка раннера не отмена; запрошенная сервером отмена важнее остановки.
    if cancelled {
        return if flags.local_stop && !flags.server_cancel() {
            Disposition::Keep
        } else {
            Disposition::Cancel
        };
    }
    let Some(f) = &e.fail else {
        return Disposition::Fail(failure_of(e, flags.elapsed_ms));
    };
    match classify(f.code, f.state) {
        FailClass::Other => Disposition::Fail(failure_of(e, flags.elapsed_ms)),
        class @ (FailClass::Launch | FailClass::Environment) => {
            // WR-01 сервера: `release` не смотрит на запрошенную отмену, поэтому отмену проверяем сами.
            if flags.server_cancel() {
                Disposition::Cancel
            } else if flags.local_stop {
                Disposition::Keep
            } else {
                let message = head(&one_line(&f.message), MESSAGE_MAX);
                Disposition::Release {
                    reason: format!("{}: {}", f.code.as_str(), head(&message, REASON_MAX)),
                    code: f.code.as_str().to_owned(),
                    class,
                    message,
                    hint: one_line(&f.hint),
                }
            }
        }
    }
}

/// Почему раннер встал на паузу.
#[derive(Debug, Clone, PartialEq)]
pub struct PauseReason {
    pub code: String,
    pub class: FailClass,
    /// Сколько сбоев подряд.
    pub jobs: u32,
    pub message: String,
    pub hint: String,
}

impl std::fmt::Display for PauseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "раннер на паузе: {} сбоя подряд «{}» с кодом {}: {}.",
            self.jobs,
            self.class.title(),
            self.code,
            self.message
        )?;
        if !self.hint.is_empty() {
            write!(f, " {}", self.hint)?;
        }
        Ok(())
    }
}

/// Серия одинаковых неисправностей подряд (как `QueuePolicy` приложения): код и класс те же, две — пауза, после
/// паузы серия начинается заново.
#[derive(Default)]
pub struct PausePolicy {
    streak: Option<(String, FailClass, u32)>,
}

impl PausePolicy {
    /// Учитывает итог задачи; `Some` — раннеру пора встать на паузу.
    pub fn after(&mut self, d: &Disposition) -> Option<PauseReason> {
        match d {
            Disposition::Release {
                code,
                class,
                message,
                hint,
                ..
            } => {
                let n = match &self.streak {
                    Some((c, k, n)) if c == code && k == class => n + 1,
                    _ => 1,
                };
                if n < PAUSE_AFTER {
                    self.streak = Some((code.clone(), *class, n));
                    return None;
                }
                self.streak = None;
                Some(PauseReason {
                    code: code.clone(),
                    class: *class,
                    jobs: n,
                    message: message.clone(),
                    hint: hint.clone(),
                })
            }
            // Контентный сбой, успех и отмена обрывают серию.
            Disposition::Done(_) | Disposition::Fail(_) | Disposition::Cancel => {
                self.streak = None;
                None
            }
            Disposition::Keep | Disposition::Abandon => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use pipeline::{RecordFailure, RecordState};

    use super::*;

    fn failed(code: FailCode, state: RecordState) -> Result<Outcome, RunError> {
        let fail = RecordFailure::new(code, state, "сообщение".to_owned(), Duration::from_secs(1));
        Err(RunError {
            message: "сообщение".to_owned(),
            fail: Some(fail),
        })
    }

    fn release(code: &str, class: FailClass) -> Disposition {
        Disposition::Release {
            reason: format!("{code}: сообщение"),
            code: code.to_owned(),
            class,
            message: "сообщение".to_owned(),
            hint: "подсказка".to_owned(),
        }
    }

    fn bad_demo() -> Disposition {
        let r = failed(FailCode::BadDemo, RecordState::Take);
        dispose(&r, Flags::quiet(5))
    }

    #[test]
    fn dispose_foreign_game_in_precheck_is_a_release_of_class_environment() {
        let r = failed(FailCode::ForeignGameRunning, RecordState::Precheck);
        match dispose(&r, Flags::quiet(5)) {
            Disposition::Release {
                reason,
                code,
                class,
                ..
            } => {
                assert_eq!(code, "FOREIGN_GAME_RUNNING");
                assert_eq!(class, FailClass::Environment);
                assert!(reason.starts_with("FOREIGN_GAME_RUNNING: "), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispose_game_not_started_is_a_release_of_class_launch() {
        let r = failed(FailCode::GameNotStarted, RecordState::Game);
        match dispose(&r, Flags::quiet(5)) {
            Disposition::Release { code, class, .. } => {
                assert_eq!(code, "GAME_NOT_STARTED");
                assert_eq!(class, FailClass::Launch);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispose_game_exit_in_the_middle_of_a_take_is_a_failure() {
        let r = failed(FailCode::GameExited, RecordState::Take);
        match dispose(&r, Flags::quiet(5)) {
            Disposition::Fail(f) => {
                assert_eq!(f.code, "GAME_EXITED");
                assert_eq!(f.class, "прочее");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispose_bad_demo_is_a_failure_with_its_code() {
        match bad_demo() {
            Disposition::Fail(f) => assert_eq!(f.code, fail::BAD_DEMO),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispose_error_without_a_code_is_job_error() {
        let r: Result<Outcome, RunError> = Err(RunError {
            message: "сломалось\nвторая строка".to_owned(),
            fail: None,
        });
        match dispose(&r, Flags::quiet(7)) {
            Disposition::Fail(f) => {
                assert_eq!(f.code, fail::JOB_ERROR);
                assert!(!f.message.contains('\n'), "{:?}", f.message);
                assert_eq!(f.elapsed_ms, 7);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dispose_environment_failure_under_a_server_cancel_is_a_cancel() {
        let r = failed(FailCode::ForeignGameRunning, RecordState::Precheck);
        for mode in [CancelMode::Soft, CancelMode::Force] {
            let flags = Flags {
                server_cancel: mode,
                ..Flags::quiet(5)
            };
            assert_eq!(dispose(&r, flags), Disposition::Cancel, "{mode:?}");
        }
    }

    #[test]
    fn dispose_local_stop_keeps_the_job_but_a_server_cancel_wins() {
        let cancelled: Result<Outcome, RunError> = Err(RunError {
            message: pipeline::CANCELLED.to_owned(),
            fail: None,
        });
        let stop = Flags {
            local_stop: true,
            ..Flags::quiet(5)
        };
        assert_eq!(dispose(&cancelled, stop), Disposition::Keep);
        let both = Flags {
            server_cancel: CancelMode::Soft,
            ..stop
        };
        assert_eq!(dispose(&cancelled, both), Disposition::Cancel);
        // Без остановки и без серверной отмены исполнитель сам отменил задачу.
        assert_eq!(dispose(&cancelled, Flags::quiet(5)), Disposition::Cancel);
        // Сбой окружения при остановке раннера тоже не release.
        let env = failed(FailCode::ForeignGameRunning, RecordState::Precheck);
        assert_eq!(dispose(&env, stop), Disposition::Keep);
    }

    #[test]
    fn dispose_lost_job_is_abandoned_whatever_the_result() {
        let flags = Flags {
            abandoned: true,
            ..Flags::quiet(5)
        };
        let env = failed(FailCode::ForeignGameRunning, RecordState::Precheck);
        assert_eq!(dispose(&env, flags), Disposition::Abandon);
        let ok = Ok(Outcome {
            out_dir: PathBuf::new(),
            result: PathBuf::new(),
            shots: 1,
            secs: 1.0,
        });
        assert_eq!(dispose(&ok, flags), Disposition::Abandon);
    }

    #[test]
    fn dispose_failure_text_is_one_line_and_bounded() {
        let mut fail = RecordFailure::new(
            FailCode::TakeTimeout,
            RecordState::Take,
            format!("длинное\n{}", "я".repeat(5000)),
            Duration::from_secs(1),
        );
        fail.console_tail = vec!["строка\nс переводом".to_owned(); 400];
        let r: Result<Outcome, RunError> = Err(RunError {
            message: "x".to_owned(),
            fail: Some(fail),
        });
        let Disposition::Fail(f) = dispose(&r, Flags::quiet(1)) else {
            panic!("ожидался Fail");
        };
        assert_eq!(f.message.chars().count(), MESSAGE_MAX);
        assert!(!f.message.contains('\n'));
        assert!(f.tail.chars().count() <= TAIL_MAX);
    }

    #[test]
    fn pause_comes_after_two_identical_releases() {
        let mut p = PausePolicy::default();
        assert_eq!(
            p.after(&release("FOREIGN_GAME_RUNNING", FailClass::Environment)),
            None
        );
        let reason = p
            .after(&release("FOREIGN_GAME_RUNNING", FailClass::Environment))
            .expect("две подряд — пауза");
        assert_eq!(reason.code, "FOREIGN_GAME_RUNNING");
        assert_eq!(reason.jobs, 2);
        let shown = reason.to_string();
        assert!(
            shown.starts_with("раннер на паузе: 2 сбоя подряд «окружение»"),
            "{shown}"
        );
        assert!(
            shown.contains("FOREIGN_GAME_RUNNING") && shown.contains("подсказка"),
            "{shown}"
        );
    }

    #[test]
    fn pause_needs_the_same_code_and_class() {
        let mut p = PausePolicy::default();
        assert_eq!(
            p.after(&release("FOREIGN_GAME_RUNNING", FailClass::Environment)),
            None
        );
        assert_eq!(
            p.after(&release("PRECHECK_FAILED", FailClass::Environment)),
            None
        );
        assert_eq!(
            p.after(&release("GAME_NOT_STARTED", FailClass::Launch)),
            None
        );
        assert_eq!(
            p.after(&release("GAME_NOT_STARTED", FailClass::Environment)),
            None
        );
    }

    #[test]
    fn pause_series_is_reset_by_a_content_failure_success_and_cancel() {
        for reset in [
            bad_demo(),
            Disposition::Cancel,
            Disposition::Done(Outcome {
                out_dir: PathBuf::new(),
                result: PathBuf::new(),
                shots: 1,
                secs: 1.0,
            }),
        ] {
            let mut p = PausePolicy::default();
            let env = release("FOREIGN_GAME_RUNNING", FailClass::Environment);
            assert_eq!(p.after(&env), None);
            assert_eq!(p.after(&reset), None);
            assert_eq!(p.after(&env), None, "серию должен был сбросить {reset:?}");
        }
    }

    #[test]
    fn pause_series_survives_keep_and_abandon() {
        let mut p = PausePolicy::default();
        let env = release("FOREIGN_GAME_RUNNING", FailClass::Environment);
        assert_eq!(p.after(&env), None);
        assert_eq!(p.after(&Disposition::Keep), None);
        assert_eq!(p.after(&Disposition::Abandon), None);
        assert!(p.after(&env).is_some());
    }

    #[test]
    fn pause_series_starts_over_after_a_pause() {
        let mut p = PausePolicy::default();
        let env = release("FOREIGN_GAME_RUNNING", FailClass::Environment);
        assert_eq!(p.after(&env), None);
        assert!(p.after(&env).is_some());
        assert_eq!(p.after(&env), None, "третий подряд паузы не даёт");
        assert!(p.after(&env).is_some());
    }
}
