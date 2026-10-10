//! Граница с ОС: только этот крейт запускает процессы, открывает сокеты и вызывает Win32;
//! политика ожиданий и дедлайнов — в `pipeline::record`.
//!
//! Правило крейта: хост сообщает факты (строка консоли, выход лаунчера, появление и смерть игры,
//! пустой job), а решения («долго ли ждать», «это сбой или нет») принимает вызывающий. Поэтому
//! единственное блокирующее ожидание сессии — [`Session::poll`] с верхней границей, а время
//! читается через [`Host::now`]: у фейка ([`fake::FakeHost`]) и то и другое виртуальное, и тест
//! с ожиданием в 150 с проходит за микросекунды.
//!
//! Файловая система не абстрагируется: пути, `console.log` и папки дублей читает сам вызывающий.

pub mod fake;
mod unsupported;
#[cfg(windows)]
pub mod win;

use std::path::PathBuf;
use std::time::Duration;

pub use unsupported::UnsupportedHost;

/// Откуда пришла строка в журнал записи (`record.log`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Консольный порт игры (`-netconport`).
    Netcon,
    /// Файл `game/csgo/console.log` (`-condebug`).
    ConsoleLog,
    /// Стандартный вывод лаунчера HLAE.
    HlaeStdout,
    /// Стандартный поток ошибок лаунчера HLAE.
    HlaeStderr,
}

impl Source {
    /// Метка источника в `record.log`.
    pub fn label(self) -> &'static str {
        match self {
            Source::Netcon => "netcon",
            Source::ConsoleLog => "console.log",
            Source::HlaeStdout => "hlae-stdout",
            Source::HlaeStderr => "hlae-stderr",
        }
    }
}

/// Что произошло с сессией игры.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// HLAE.exe (лаунчер) завершился. Код Windows может быть отрицательным в `i32` — печатать как `0x…`.
    LauncherExited { code: Option<i32> },
    /// `cs2.exe` появился в нашем job (или принят по PID).
    GameAppeared { pid: u32 },
    /// Консольный порт игры принял соединение.
    ConsoleUp,
    /// Соединение с консолью закрылось или чтение сорвалось.
    ConsoleDown,
    /// Одна строка консоли: байты как lossy UTF-8, без пробелов по краям и без BOM.
    Line { source: Source, text: String },
    /// Процесс игры завершился (handle сработал).
    GameExited { code: Option<u32> },
    /// В job не осталось процессов (включая ffmpeg от HLAE) после появления игры.
    JobEmpty,
    /// Диагностика хоста для `record.log`, например «cs2.exe принят по PID».
    Note(String),
}

/// Жива ли игра. Ошибка опроса — это `Unknown`, а не `Alive`: «не знаю» нельзя выдавать за «работает».
#[derive(Debug, Clone, PartialEq)]
pub enum Liveness {
    /// Игра ещё не появлялась.
    NotStarted,
    Alive,
    /// Завершилась; код выхода, если удалось прочитать.
    Exited(Option<u32>),
    /// Опросить не удалось; внутри — текст ошибки ОС.
    Unknown(String),
}

/// Ошибка хоста.
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    /// Платформа не Windows.
    Unsupported,
    /// Процесс не запустился.
    Spawn(String),
    /// Процесс не попал в Job Object.
    JobAssign(String),
    /// Консольный порт игры не подключён.
    NotConnected,
    /// В команде для консоли есть `\r`, `\n` или `\0`.
    BadCommand,
    Io(String),
    /// Вышел дедлайн запуска утилиты.
    Timeout,
    /// Пользователь отменил задачу.
    Cancelled,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Unsupported => f.write_str("запись возможна только в Windows"),
            HostError::Spawn(e) => write!(f, "не удалось запустить процесс: {e}"),
            HostError::JobAssign(e) => write!(f, "процесс не помещён в Job Object: {e}"),
            HostError::NotConnected => f.write_str("консоль игры не подключена"),
            HostError::BadCommand => {
                f.write_str("команда для консоли содержит перевод строки или нулевой символ")
            }
            HostError::Io(e) => write!(f, "ввод-вывод: {e}"),
            HostError::Timeout => f.write_str("превышено время ожидания"),
            HostError::Cancelled => f.write_str("отменено"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<HostError> for String {
    fn from(e: HostError) -> String {
        e.to_string()
    }
}

/// Что запустить: игру с демкой через HLAE.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaunchSpec {
    /// Папка HLAE (`HLAE.exe`, `x64/AfxHookSource2.dll`).
    pub hlae_dir: PathBuf,
    /// Путь к `cs2.exe`.
    pub cs2_exe: PathBuf,
    /// Имя демки в `game/csgo` (для `+playdemo`).
    pub demo_name: String,
    /// Размер окна = размер кадра ролика.
    pub size: (u32, u32),
    /// Код задачи для меток консоли (`CS2C <nonce> …`); хост его только передаёт фейку.
    pub nonce: String,
    /// Куда игра зеркалит консоль (`game/csgo/console.log`), если его нужно читать.
    pub console_log: Option<PathBuf>,
}

/// Запуск внешней утилиты (ffmpeg, ffprobe) с жёстким дедлайном.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Дольше этого процесс будет остановлен, а результат — `HostError::Timeout`.
    pub deadline: Duration,
}

/// Итог запуска утилиты.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdOutput {
    /// Код выхода; `None`, если процесс остановлен.
    pub code: Option<i32>,
    /// Последние строки stderr — для причины сбоя.
    pub stderr_tail: Vec<String>,
    pub elapsed: Duration,
}

/// Запущенная сессия игры: HLAE, `cs2.exe`, консоль, job.
pub trait Session: Send {
    /// Единственное блокирующее ожидание сессии: возвращает следующее событие, но ждёт не дольше
    /// `max_wait`. У фейка без событий сдвигает виртуальные часы ровно на `max_wait`.
    fn poll(&mut self, max_wait: Duration) -> Option<SessionEvent>;

    /// Отправить команду в консоль игры (только netcon). `BadCommand` — если в строке `\r`, `\n`
    /// или `\0` (ничего не уходит); `NotConnected` — если порт ещё не открыт или уже закрыт.
    fn send(&mut self, line: &str) -> Result<(), HostError>;

    /// Жива ли игра — по handle процесса. Ошибка опроса даёт `Unknown`, не `Alive`.
    fn game_liveness(&mut self) -> Liveness;

    /// Код выхода лаунчера: `None`, пока выход не наблюдался; `Some(None)` — вышел, код неизвестен.
    fn launcher_exit(&self) -> Option<Option<i32>>;

    /// Завершить всё дерево процессов сессии. Идемпотентно; реализации вызывают и из `Drop`.
    fn terminate(&mut self);
}

/// Хост: запускает сессии и утилиты, отдаёт часы.
pub trait Host: Send + Sync {
    /// Запустить игру через HLAE в новом job.
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError>;

    /// Монотонные часы хоста (у фейка — виртуальные, их двигает только `poll`).
    fn now(&self) -> Duration;

    /// Запустить утилиту с дедлайном `cmd.deadline`; строки её stdout приходят в `on_line` на
    /// вызывающем потоке. `cancel` опрашивается между строками и по таймеру.
    fn run(
        &self,
        cmd: &CmdSpec,
        cancel: &dyn Fn() -> bool,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, HostError>;
}

/// Годится ли строка для консоли игры: не пустая и без `\r`, `\n`, `\0`.
pub fn console_line_ok(line: &str) -> bool {
    !line.is_empty() && !line.contains(['\r', '\n', '\0'])
}

/// Хост по умолчанию. Пока везде `UnsupportedHost`; на Windows его заменит `WinHost`.
pub fn default_host() -> Box<dyn Host> {
    Box::new(UnsupportedHost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_line_rejects_breaks_and_empty() {
        assert!(console_line_ok("exec highlights"));
        assert!(!console_line_ok(""));
        assert!(!console_line_ok("quit\n"));
        assert!(!console_line_ok("a\rb"));
        assert!(!console_line_ok("a\0b"));
    }

    #[test]
    fn source_labels_match_the_record_log_format() {
        assert_eq!(Source::Netcon.label(), "netcon");
        assert_eq!(Source::ConsoleLog.label(), "console.log");
        assert_eq!(Source::HlaeStdout.label(), "hlae-stdout");
        assert_eq!(Source::HlaeStderr.label(), "hlae-stderr");
    }

    #[test]
    fn host_error_turns_into_a_russian_string() {
        let s: String = HostError::BadCommand.into();
        assert!(s.contains("перевод строки"), "{s}");
        assert_eq!(String::from(HostError::Cancelled), "отменено");
    }
}
