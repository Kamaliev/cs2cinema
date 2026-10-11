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
pub mod gate;
pub mod runner;
mod unsupported;
#[cfg(windows)]
pub mod win;

use std::path::{Path, PathBuf};
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

/// Потолок длины каждой строки [`WindowInfo`] в символах: текст окон чужого процесса недоверенный.
pub const WINDOW_TEXT_MAX: usize = 300;

/// Видимое верхнее окно процесса своего job (D-10, LCH-05).
///
/// Все строки — недоверенный текст чужого процесса, каждая не длиннее [`WINDOW_TEXT_MAX`] символов.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub pid: u32,
    /// Класс окна, например `#32770` (диалог).
    pub class: String,
    pub title: String,
    /// Для диалога `#32770` — текст дочерних окон `Static` через « / »; иначе пусто.
    pub text: String,
}

/// Процесс из снимка процессов системы.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Имя образа, например `steam.exe`.
    pub exe_name: String,
}

/// Процесс, запущенный приложением: PID, время создания (FILETIME, 100 нс от 1601 г.) и имя образа.
///
/// PID переиспользуется системой, поэтому один PID ничего не доказывает — только пара с временем
/// создания. Имя образа — как его отдаёт ОС (например `cs2.exe`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcIdentity {
    pub pid: u32,
    pub created: u64,
    pub image: String,
}

/// Образы, которые приложение вправе завершить как свои осиротевшие процессы (сравнение без
/// учёта регистра). Дочерний ffmpeg HLAE завершается вместе с job и сюда не входит.
pub const ORPHAN_IMAGES: [&str; 2] = ["cs2.exe", "hlae.exe"];

/// Итог попытки завершить осиротевший процесс прошлого экземпляра приложения (D-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanOutcome {
    /// Процесс совпал по всем признакам и завершён.
    Killed,
    /// Процесса уже нет (или он успел выйти).
    Gone,
    /// PID занят другим процессом или образ не из списка своих: ничего не тронуто.
    Mismatch(String),
    /// Процесс свой, но завершить его не удалось.
    Failed(String),
    /// Платформа не умеет завершать процессы по идентичности.
    Unsupported,
}

impl std::fmt::Display for OrphanOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrphanOutcome::Killed => f.write_str("завершён"),
            OrphanOutcome::Gone => f.write_str("уже не работает"),
            OrphanOutcome::Mismatch(why) => write!(f, "не тот процесс: {why}"),
            OrphanOutcome::Failed(why) => write!(f, "не удалось завершить: {why}"),
            OrphanOutcome::Unsupported => f.write_str("недоступно на этой платформе"),
        }
    }
}

/// Итог ожидания своей игры от прошлой сессии, пережившей завершение job (D-14, TMO-07).
#[derive(Debug, Clone, PartialEq)]
pub enum PrevGame {
    /// Своих оставшихся игр нет (или они вышли); `waited` — сколько ждали.
    Clear { waited: Duration },
    /// Своя игра жива через `limit` после завершения своего job.
    Stuck {
        pid: u32,
        since_end: Duration,
        liveness: Liveness,
    },
    /// Ожидание прервано отменой.
    Cancelled,
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

    /// Видимые верхние окна процессов своего job. Не блокирует и ничего не отправляет окнам
    /// (читается только кэшированный заголовок). Ошибка перечисления — пустой список (D-10).
    fn windows(&mut self) -> Vec<WindowInfo> {
        Vec::new()
    }

    /// Свои процессы сессии для записи `processes.json`: лаунчер с запуска, игра с `GameAppeared`.
    fn processes(&self) -> Vec<ProcIdentity> {
        Vec::new()
    }

    /// Ждёт выхода игры по handle не дольше `limit`. `Exited` — игра вышла и в своём job не
    /// осталось процессов; `Alive` — жива после `limit`; `Unknown` — проверить не удалось
    /// (никогда не выдаётся за `Alive`); `NotStarted` — игры не было. Процессы сам не
    /// завершает (TMO-04).
    fn wait_exit(&mut self, limit: Duration) -> Liveness {
        let _ = limit;
        self.game_liveness()
    }
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

    /// Ждёт по handle выхода своих `cs2.exe`, переживших свою сессию; после последнего выхода
    /// выдерживает паузу `settle`. Игра, живая через `limit` после завершения своего job, —
    /// `Stuck`. Чужие процессы хост не видит и не трогает (D-14, TMO-05, TMO-07).
    fn wait_previous_game(
        &self,
        limit: Duration,
        settle: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> PrevGame {
        let _ = (limit, settle, cancel);
        PrevGame::Clear {
            waited: Duration::ZERO,
        }
    }

    /// Снимок процессов системы — только для проверок «Steam запущен» и «запущен чужой CS2».
    /// Завершать по снимку нечем: единственный способ остановить процесс по PID —
    /// [`Host::kill_orphan`] по полной идентичности (PRE-04, PRE-05, TMO-05).
    fn processes(&self) -> Result<Vec<ProcInfo>, HostError> {
        Err(HostError::Unsupported)
    }

    /// Завершает процесс прошлого экземпляра приложения, только если PID, время создания и имя
    /// образа совпали, а образ входит в [`ORPHAN_IMAGES`]; проверка и завершение идут на одном
    /// handle. По имени или по одному PID хост не завершает ничего (D-15, TMO-05).
    fn kill_orphan(&self, id: &ProcIdentity) -> OrphanOutcome {
        let _ = id;
        OrphanOutcome::Unsupported
    }

    /// Байты, доступные вызывающему (с учётом квот), на томе пути; у несуществующего пути —
    /// на томе ближайшего существующего предка (PRE-06).
    fn free_space(&self, path: &Path) -> Result<u64, HostError> {
        let _ = path;
        Err(HostError::Unsupported)
    }

    /// Версия файла `a.b.c.d` из ресурса версии, например `HLAE.exe` -> `2.192.7.0` (DIF-01).
    fn file_version(&self, path: &Path) -> Option<String> {
        let _ = path;
        None
    }

    /// Уведомление в трее; не блокирует — показ и уборка значка идут на своём потоке; `false` —
    /// не показано (нет платформы или Windows отказал). Текст сюда приходит как есть: хост
    /// сам чистит его через [`notify_text`] (OBS-04).
    fn notify(&self, title: &str, text: &str) -> bool {
        let _ = (title, text);
        false
    }
}

/// Предел заголовка уведомления Windows в единицах UTF-16 (`szInfoTitle` — 64 с нулём).
pub const NOTIFY_TITLE_MAX: usize = 63;
/// Предел текста уведомления Windows в единицах UTF-16 (`szInfo` — 256 с нулём).
pub const NOTIFY_TEXT_MAX: usize = 255;

/// Готовит строку для уведомления: убирает управляющие символы и, если строка длиннее
/// `max_units` единиц UTF-16, обрезает по границе символа и дописывает «…» так, чтобы итог
/// уложился в предел. Один код для настоящего хоста и фейка (T-04-06).
pub fn notify_text(s: &str, max_units: usize) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control()).collect();
    if clean.encode_utf16().count() <= max_units {
        return clean;
    }
    const ELLIPSIS: char = '…';
    let budget = max_units.saturating_sub(ELLIPSIS.len_utf16());
    let mut out = String::new();
    let mut used = 0;
    for c in clean.chars() {
        let n = c.len_utf16();
        if used + n > budget {
            break;
        }
        out.push(c);
        used += n;
    }
    if max_units >= ELLIPSIS.len_utf16() {
        out.push(ELLIPSIS);
    }
    out
}

/// Годится ли строка для консоли игры: не пустая и без `\r`, `\n`, `\0`.
pub fn console_line_ok(line: &str) -> bool {
    !line.is_empty() && !line.contains(['\r', '\n', '\0'])
}

/// Хост по умолчанию: `WinHost` на Windows, на остальных платформах — заглушка `UnsupportedHost`.
pub fn default_host() -> Box<dyn Host> {
    #[cfg(windows)]
    {
        Box::new(win::WinHost::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(UnsupportedHost)
    }
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
    fn notify_text_strips_control_chars_and_fits_the_limit() {
        assert_eq!(notify_text("a\u{7}b\nc", NOTIFY_TEXT_MAX), "abc");
        assert_eq!(
            notify_text("2 готово / 1 упало", NOTIFY_TEXT_MAX),
            "2 готово / 1 упало"
        );
        let long = "я".repeat(400);
        let cut = notify_text(&long, NOTIFY_TEXT_MAX);
        assert!(cut.encode_utf16().count() <= NOTIFY_TEXT_MAX);
        assert!(cut.ends_with('…'));
        // эмодзи занимает две единицы UTF-16: на границе его нельзя резать пополам
        let emoji = "😀".repeat(100);
        let cut = notify_text(&emoji, 10);
        assert!(cut.encode_utf16().count() <= 10, "{cut}");
        assert!(cut.ends_with('…'));
        assert!(cut.chars().filter(|&c| c == '😀').count() == 4, "{cut}");
        assert_eq!(
            notify_text(&"x".repeat(NOTIFY_TITLE_MAX), NOTIFY_TITLE_MAX).len(),
            NOTIFY_TITLE_MAX
        );
    }

    #[test]
    fn unsupported_host_does_not_notify() {
        assert!(!UnsupportedHost.notify("CS2 Cinema", "текст"));
    }

    #[test]
    fn host_error_turns_into_a_russian_string() {
        let s: String = HostError::BadCommand.into();
        assert!(s.contains("перевод строки"), "{s}");
        assert_eq!(String::from(HostError::Cancelled), "отменено");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn default_host_is_win_host_on_windows() {
        // путей нет, поэтому ничего не запускается: настоящий хост отвечает `Spawn`, а не `Unsupported`
        let Err(e) = default_host().launch(&LaunchSpec::default()) else {
            panic!("пустой LaunchSpec не должен запускаться")
        };
        assert!(matches!(e, HostError::Spawn(_)), "{e:?}");
    }
}
