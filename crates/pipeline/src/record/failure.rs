//! Коды сбоев записи (D-08) и каталог сообщений по-русски. Весь русский текст сбоев живёт только
//! здесь: машина и драйвер передают сюда факты (секунды, коды выхода), а не строят фразы сами.
//!
//! Что в каждом сбое: код, состояние лестницы, сообщение, подсказка и прошедшее время. Хвост
//! консоли, строку фактов HLAE.exe/cs2.exe и код `HLAE_ERROR` добавляет план 01-08.

use std::time::Duration;

use host::{HostError, Liveness};

use super::machine::RecordState;

/// Код сбоя записи (набор D-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailCode {
    LauncherFailed,
    GameNotStarted,
    ConsoleNotOpen,
    DemoNotLoaded,
    ScriptNotRun,
    TakeTimeout,
    GameExited,
    HlaeError,
    Cancelled,
}

impl FailCode {
    /// Код в базе, журнале и карточке.
    pub fn as_str(self) -> &'static str {
        match self {
            FailCode::LauncherFailed => "LAUNCHER_FAILED",
            FailCode::GameNotStarted => "GAME_NOT_STARTED",
            FailCode::ConsoleNotOpen => "CONSOLE_NOT_OPEN",
            FailCode::DemoNotLoaded => "DEMO_NOT_LOADED",
            FailCode::ScriptNotRun => "SCRIPT_NOT_RUN",
            FailCode::TakeTimeout => "TAKE_TIMEOUT",
            FailCode::GameExited => "GAME_EXITED",
            FailCode::HlaeError => "HLAE_ERROR",
            FailCode::Cancelled => "CANCELLED",
        }
    }

    /// Что делать пользователю. У отмены подсказки нет.
    pub fn hint(self) -> &'static str {
        match self {
            FailCode::LauncherFailed => {
                "Проверьте папку HLAE в настройках и запустите HLAE.exe вручную — он покажет причину."
            }
            FailCode::GameNotStarted => {
                "Убедитесь, что Steam запущен, CS2 не обновляется, а антивирус не блокирует AfxHookSource2.dll."
            }
            FailCode::ConsoleNotOpen => {
                "Закройте CS2 и повторите. Смотрите строки [console.log] в record.log: возможно, HLAE не подходит к текущему обновлению CS2."
            }
            FailCode::DemoNotLoaded => {
                "Демка могла быть повреждена или записана в другой версии CS2 — откройте её в игре вручную."
            }
            FailCode::ScriptNotRun => {
                "Проверьте, что в game\\csgo\\cfg лежат highlights.cfg и cs2c_*.cfg, а в пути результатов нет пробелов."
            }
            FailCode::TakeTimeout => {
                "Если в консоли повторяется «Failed writing image», проверьте ffmpeg HLAE (Path в ffmpeg\\ffmpeg.ini)."
            }
            FailCode::GameExited => {
                "Игру могли закрыть вручную или она упала — см. последние строки консоли."
            }
            FailCode::HlaeError => {
                "Проверьте ffmpeg HLAE: Path в ffmpeg\\ffmpeg.ini должен указывать на рабочий ffmpeg.exe."
            }
            FailCode::Cancelled => "",
        }
    }
}

impl std::fmt::Display for FailCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Что известно о лаунчере HLAE.exe (отдельно от игры: он завершается сразу после старта cs2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherFact {
    /// Запуск не состоялся или ещё не начинался.
    NotLaunched,
    /// Запущен и ещё работает.
    Running,
    /// Завершился; код выхода, если известен.
    Exited(Option<i32>),
}

/// Сбой записи: всё, что нужно показать в карточке и записать в базу.
#[derive(Debug, Clone)]
pub struct RecordFailure {
    pub code: FailCode,
    /// Состояние лестницы, на котором случился сбой.
    pub state: RecordState,
    /// Сообщение по-русски.
    pub message: String,
    /// Подсказка по-русски (`FailCode::hint`).
    pub hint: String,
    /// Время от входа в ожидание, дедлайн которого сработал (у `GAME_EXITED` и `CANCELLED` — от входа
    /// в текущее состояние).
    pub elapsed: Duration,
    /// Свёрнутый хвост консоли: заполняет план 01-08.
    pub console_tail: Vec<String>,
    pub launcher: LauncherFact,
    pub game: Liveness,
}

impl RecordFailure {
    /// Сбой с подсказкой по коду; хвост консоли пуст, факты лаунчера и игры заполняет драйвер.
    pub fn new(code: FailCode, state: RecordState, message: String, elapsed: Duration) -> Self {
        RecordFailure {
            code,
            state,
            message,
            hint: code.hint().to_owned(),
            elapsed,
            console_tail: vec![],
            launcher: LauncherFact::NotLaunched,
            game: Liveness::NotStarted,
        }
    }
}

impl std::fmt::Display for RecordFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Целые секунды дедлайна для сообщения.
pub fn secs(d: Duration) -> u64 {
    d.as_secs_f64().round() as u64
}

/// Код выхода Windows: отрицательный `i32` печатается как `0x…`.
pub fn exit_code_text(code: i32) -> String {
    if code >= 0 {
        code.to_string()
    } else {
        format!("0x{:X}", code as u32)
    }
}

/// Хост не смог запустить лаунчер.
pub fn msg_launch_error(e: &HostError) -> String {
    match e {
        HostError::Unsupported => e.to_string(),
        _ => format!("HLAE не запустился: {e}"),
    }
}

/// Лаунчер завершился с ненулевым (или неизвестным) кодом.
pub fn msg_launcher_exit(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("HLAE.exe завершился с кодом {}", exit_code_text(c)),
        None => "HLAE.exe завершился, код выхода неизвестен".to_owned(),
    }
}

/// Лаунчер не завершился в срок.
pub fn msg_launcher_timeout(limit: Duration) -> String {
    format!(
        "HLAE.exe не завершился за {} с — вероятно, показывает окно с ошибкой",
        secs(limit)
    )
}

/// Демка не загрузилась; `running` — машина видела признак идущей демки.
pub fn msg_demo_not_loaded(limit: Duration, running: bool) -> String {
    if running {
        format!("Демка идёт, но метка READY не пришла за {} с", secs(limit))
    } else {
        format!(
            "Консоль открыта, но демка не загрузилась за {} с",
            secs(limit)
        )
    }
}

/// Отмена пользователем.
pub fn msg_cancelled() -> String {
    "Отменено".to_owned()
}

/// Игра закрылась до `quit`; для дубля называем его номер.
pub fn msg_game_exited(state: RecordState, take: Option<(u32, u32)>) -> String {
    match take {
        Some((n, m)) => format!(
            "Игра закрылась на этапе «{}» (дубль {n} из {m})",
            state.title()
        ),
        None => format!("Игра закрылась на этапе «{}»", state.title()),
    }
}

/// Игра не закрылась после `quit`: не сбой, а предупреждение в журнал.
pub fn msg_game_not_closed(limit: Duration) -> String {
    format!("игра не закрылась за {} с — завершаю свой job", secs(limit))
}

/// Временное сообщение для кодов, чей каталог дописывается следующей задачей плана.
pub fn msg_generic(code: FailCode, limit: Duration) -> String {
    format!(
        "{}: ожидание не уложилось в {} с",
        code.as_str(),
        secs(limit)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_have_stable_names_and_hints() {
        assert_eq!(FailCode::DemoNotLoaded.as_str(), "DEMO_NOT_LOADED");
        assert_eq!(FailCode::Cancelled.as_str(), "CANCELLED");
        assert!(FailCode::LauncherFailed.hint().contains("HLAE.exe"));
        assert_eq!(FailCode::Cancelled.hint(), "");
    }

    #[test]
    fn negative_exit_codes_are_printed_as_hex() {
        assert_eq!(exit_code_text(1), "1");
        assert_eq!(exit_code_text(-1073741819), "0xC0000005");
    }
}
