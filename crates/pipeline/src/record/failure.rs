//! Коды сбоев записи (D-08) и каталог сообщений по-русски. Весь русский текст сбоев живёт только
//! здесь: машина и драйвер передают сюда факты (секунды, коды выхода), а не строят фразы сами.
//!
//! Что в каждом сбое: код, состояние лестницы, сообщение, подсказка и прошедшее время. Хвост
//! консоли и строку фактов HLAE.exe/cs2.exe заполняет драйвер `record()` (`facts_line`).

use std::time::Duration;

use host::{HostError, Liveness, WindowInfo};

use super::machine::RecordState;
use super::verify::TakeFacts;

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
    /// Предпроверка окружения не пройдена.
    PrecheckFailed,
    /// Запущен CS2, который приложение не запускало.
    ForeignGameRunning,
    /// Своя игра прошлой задачи не закрылась.
    PrevGameStuck,
    /// Дубль записан, но не прошёл проверку ffprobe: нет звука, обрезан, другое разрешение.
    TakeInvalid,
    /// Повтору нечего взять из папки задачи: нет или испорчен `record.json`, изменились скрипты.
    RetryNoData,
    /// Ника нет в демке: ни по SteamID, ни по имени на одном из ярусов выбора.
    PlayerNotFound,
    /// Ник подходит двум и более игрокам на первом сработавшем ярусе: герой неизвестен.
    PlayerAmbiguous,
    /// Игрок найден, но убийств и полезных гранат у него нет: снимать нечего.
    NoHighlights,
    /// Демка не разбирается: обрезана, повреждена или не из CS2.
    BadDemo,
    /// Демка больше допустимого размера.
    DemoTooLarge,
    /// Раннер собрал ролик, но он не прошёл проверку перед отправкой на сервер.
    ResultInvalid,
    /// Источник задачи раннер не умеет брать (ссылка в задаче не похожа на комнату матча FACEIT).
    UnsupportedSource,
    /// Раннер не смог скачать демку FACEIT по ссылке задачи (нет ключа, нет демки у матча, отказ API или сети).
    FaceitFetchFailed,
}

impl FailCode {
    /// Все коды по порядку объявления: по этому списку раннер сверяет `wire::fail::KNOWN`.
    pub const ALL: [FailCode; 22] = [
        FailCode::LauncherFailed,
        FailCode::GameNotStarted,
        FailCode::ConsoleNotOpen,
        FailCode::DemoNotLoaded,
        FailCode::ScriptNotRun,
        FailCode::TakeTimeout,
        FailCode::GameExited,
        FailCode::HlaeError,
        FailCode::Cancelled,
        FailCode::PrecheckFailed,
        FailCode::ForeignGameRunning,
        FailCode::PrevGameStuck,
        FailCode::TakeInvalid,
        FailCode::RetryNoData,
        FailCode::PlayerNotFound,
        FailCode::PlayerAmbiguous,
        FailCode::NoHighlights,
        FailCode::BadDemo,
        FailCode::DemoTooLarge,
        FailCode::ResultInvalid,
        FailCode::UnsupportedSource,
        FailCode::FaceitFetchFailed,
    ];

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
            FailCode::PrecheckFailed => "PRECHECK_FAILED",
            FailCode::ForeignGameRunning => "FOREIGN_GAME_RUNNING",
            FailCode::PrevGameStuck => "PREV_GAME_STUCK",
            FailCode::TakeInvalid => "TAKE_INVALID",
            FailCode::RetryNoData => "RETRY_NO_DATA",
            FailCode::PlayerNotFound => "PLAYER_NOT_FOUND",
            FailCode::PlayerAmbiguous => "PLAYER_AMBIGUOUS",
            FailCode::NoHighlights => "NO_HIGHLIGHTS",
            FailCode::BadDemo => "BAD_DEMO",
            FailCode::DemoTooLarge => "DEMO_TOO_LARGE",
            FailCode::ResultInvalid => "RESULT_INVALID",
            FailCode::UnsupportedSource => "UNSUPPORTED_SOURCE",
            FailCode::FaceitFetchFailed => "FACEIT_FETCH_FAILED",
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
            FailCode::PrecheckFailed => {
                "Исправьте пункты предпроверки, отмеченные ✖, и нажмите «Повторить» (или «Заново», если задача не дошла до записи); проверить окружение можно в настройках."
            }
            FailCode::ForeignGameRunning => {
                "Закройте CS2, запущенный не приложением, и нажмите «Повторить» (или «Заново», если задача не дошла до записи). Приложение чужие процессы не закрывает."
            }
            FailCode::PrevGameStuck => {
                "Игра прошлой задачи не закрылась за 30 с. Завершите cs2.exe в диспетчере задач и нажмите «Повторить» (или «Заново», если задача не дошла до записи)."
            }
            FailCode::TakeInvalid => {
                "Посмотрите дубль, названный в сообщении, в папке clips: он сохранится как улика. «Повторить» запишет дубли заново; если сбой повторяется — проверьте ffmpeg HLAE и свободное место."
            }
            FailCode::RetryNoData => {
                "Данных для повтора нет — нажмите «Заново»: задача начнётся со скачивания и плана."
            }
            FailCode::PlayerNotFound | FailCode::PlayerAmbiguous => {
                "Укажите ник точнее или SteamID64 игрока."
            }
            FailCode::NoHighlights => "Выберите другого игрока или другой матч.",
            FailCode::BadDemo => "Пришлите целую демку .dem или .dem.zst из матча CS2.",
            FailCode::DemoTooLarge => {
                "Демка больше допустимого размера — пришлите демку одной карты."
            }
            FailCode::ResultInvalid => {
                "Ролик сохранён у раннера как улика; задачу можно поставить заново."
            }
            FailCode::UnsupportedSource => {
                "Пришлите файл демки или ссылку на комнату матча FACEIT."
            }
            FailCode::FaceitFetchFailed => {
                "Проверьте, что матч доигран и демка есть на странице матча; иначе пришлите файл демки."
            }
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

/// Игра не появилась после выхода лаунчера.
pub fn msg_game_not_started(limit: Duration) -> String {
    format!("CS2 не запустилась за {} с после выхода HLAE", secs(limit))
}

/// Консольный порт не открылся.
pub fn msg_console_not_open(limit: Duration) -> String {
    format!(
        "Игра запущена, но консольный порт не открылся за {} с",
        secs(limit)
    )
}

/// Скрипт `highlights` не выполнился.
pub fn msg_script_not_run(limit: Duration) -> String {
    format!(
        "Демка загружена, но скрипт highlights не выполнился: метка SCRIPT не пришла за {} с",
        secs(limit)
    )
}

/// Метка дубля не пришла; `n` — номер дубля с единицы, `kind` — `TAKE_START` или `TAKE_END`.
pub fn msg_take_timeout(n: u32, m: u32, kind: &str, limit: Duration) -> String {
    format!(
        "Дубль {n} из {m}: метка {kind} не пришла за {} с",
        secs(limit)
    )
}

/// Дубль записан, но не прошёл проверку (D-04): номер с единицы, папка и причина от `judge`.
pub fn msg_take_invalid(n: u32, m: u32, dir: &str, reason: &str) -> String {
    format!("дубль {n} из {m} ({dir}) не прошёл проверку: {reason}")
}

/// Подсказка с номером и папкой негодного дубля — драйвер ставит её вместо общей (D-04).
pub fn hint_take_invalid(n: u32, dir: &str) -> String {
    format!(
        "Посмотрите дубль {n} в папке clips/{dir}: он сохранится как улика. «Повторить» запишет дубли заново; если сбой повторяется — проверьте ffmpeg HLAE и свободное место."
    )
}

/// Повтору нечего взять из папки задачи (D-08).
pub fn msg_retry_no_data(reason: &str) -> String {
    format!("нет данных для повтора: {reason} — запустите задачу заново")
}

/// Самое длинное имя или запрос пользователя в тексте сбоя, символов.
pub const TEXT_NAME_MAX: usize = 64;

/// Строка из недоверенного источника (ник из запроса, имя из демки) для текста сбоя: без управляющих
/// символов и не длиннее `max` символов (обрезка по символам, не по байтам; обрезанное помечается «…»).
pub fn clean_text(s: &str, max: usize) -> String {
    let all: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
    let mut out: String = all.iter().take(max).collect();
    if all.len() > max {
        out.push('…');
    }
    out
}

/// «a, b, c[, и ещё N]» — список для сообщений о выборе игрока.
fn list_with_more(items: &[String], more: usize) -> String {
    let mut text = items.join(", ");
    if more > 0 {
        text.push_str(&format!(", и ещё {more}"));
    }
    text
}

/// Игрока с таким ником (SteamID64) в демке нет; `players` — кого можно выбрать.
pub fn msg_player_not_found(query: &str, players: &[String], more: usize) -> String {
    let q = clean_text(query, TEXT_NAME_MAX);
    if players.is_empty() {
        return format!("игрок «{q}» в демке не найден");
    }
    format!("игрок «{q}» в демке не найден; в демке: {}", list_with_more(players, more))
}

/// Ник подходит нескольким игрокам: герой не выбран, чтобы не снять ролик про чужого.
pub fn msg_player_ambiguous(query: &str, candidates: &[String], more: usize) -> String {
    let q = clean_text(query, TEXT_NAME_MAX);
    format!("ник «{q}» подходит нескольким игрокам: {}", list_with_more(candidates, more))
}

/// У игрока нет моментов для ролика.
pub fn msg_no_highlights(name: &str) -> String {
    format!("у игрока {} в этом матче нет ни убийств, ни полезных гранат", clean_text(name, TEXT_NAME_MAX))
}

/// Демка не разобралась.
pub fn msg_bad_demo(reason: &str) -> String {
    format!("демка не разбирается: {reason}")
}

/// Демка больше допустимого размера.
pub fn msg_demo_too_large(reason: &str) -> String {
    format!("демка слишком большая: {reason}")
}

/// Готовый ролик не прошёл проверку перед отправкой на сервер.
pub fn msg_result_invalid(reason: &str) -> String {
    format!("ролик не прошёл проверку перед отправкой: {reason}")
}

/// Источник задачи раннер не берёт.
pub fn msg_unsupported_source() -> String {
    "ссылка в задаче не похожа на комнату матча FACEIT".to_owned()
}

/// Раннер не смог скачать демку FACEIT; `reason` — фиксированная фраза без адресов и ключа.
pub fn msg_faceit_fetch_failed(reason: &str) -> String {
    reason.to_owned()
}

/// Дубль проверен: длина, размер кадра и длина звука.
pub fn msg_take_validated(n: u32, m: u32, f: &TakeFacts) -> String {
    format!(
        "дубль {n}/{m} проверен: {:.1} с, {}x{}, звук {:.1} с",
        f.video_secs, f.width, f.height, f.audio_secs
    )
}

/// Файл дубля не дописан за отведённое время.
pub fn msg_take_not_final(dir: &str, limit: Duration, reason: &str) -> String {
    format!("{dir}: файл не дописан за {} с — {reason}", secs(limit))
}

/// Размер файлов дубля сдвинулся, пока шла проверка: ffmpeg HLAE ещё пишет. `before` — размеры
/// `(video.mp4, audio.wav)` до `ffprobe`, `after` — после; `None` — один из файлов пропал.
pub fn msg_take_growing(before: (u64, u64), after: Option<(u64, u64)>) -> String {
    match after {
        Some(a) => format!(
            "размер менялся во время проверки (ffmpeg HLAE ещё пишет): video.mp4 {} → {} байт, audio.wav {} → {} байт",
            before.0, a.0, before.1, a.1
        ),
        None => "video.mp4 или audio.wav пропал во время проверки".to_owned(),
    }
}

/// Пришла метка, но не та, что ждали.
pub fn msg_misordered(got: &str, expected: &str) -> String {
    format!("пришла метка {got} вместо {expected}")
}

/// Машина просит READY напрямую, потому что демка уже идёт.
pub fn msg_ready_direct() -> String {
    "демка идёт, а метки READY нет — запрашиваю её напрямую".to_owned()
}

/// Команду в консоль отправить не удалось.
pub fn msg_send_failed(line: &str, error: &str) -> String {
    format!("не удалось отправить в консоль «{line}»: {error}")
}

/// Код завершения Windows в сообщении: большие (`0xC0000005`) — шестнадцатеричным строчными
/// (так их ищут в справочниках), остальные — десятичным.
pub fn code_text(code: u32) -> String {
    if code >= 0x8000_0000 {
        format!("{code:#x}")
    } else {
        code.to_string()
    }
}

/// Строка фактов о двух процессах: код выхода `HLAE.exe` и живость `cs2.exe` — раздельно,
/// потому что лаунчер выходит сразу после старта игры (LCH-06). Проверка живости, которая не
/// удалась, всегда «неизвестно» и никогда «жива» (TMO-02).
pub fn facts_line(launcher: &LauncherFact, game: &Liveness) -> String {
    let hlae = match launcher {
        LauncherFact::NotLaunched => "не запускался".to_owned(),
        LauncherFact::Running => "ещё работает".to_owned(),
        LauncherFact::Exited(Some(c)) => format!("код {}", code_text(*c as u32)),
        LauncherFact::Exited(None) => "код неизвестен".to_owned(),
    };
    let cs2 = match game {
        Liveness::NotStarted => "не запускалась".to_owned(),
        Liveness::Alive => "жива".to_owned(),
        Liveness::Exited(Some(c)) => format!("не жива (код {})", code_text(*c)),
        Liveness::Exited(None) => "не жива".to_owned(),
        Liveness::Unknown(e) => format!("неизвестно ({e})"),
    };
    format!("HLAE.exe: {hlae} · cs2.exe: {cs2}")
}

/// Первая строка `AFXERROR` в консоли: HLAE сам сказал, что не так, ждать дедлайна незачем (D-09).
pub fn msg_hlae_error_now(afx: &str, state: RecordState) -> String {
    format!("HLAE сообщил об ошибке: {afx} (этап «{}»)", state.title())
}

/// Окно ошибки HLAE/AfxHookSource2 (D-10): заголовок и текст диалога.
pub fn msg_hlae_dialog(title: &str, text: &str, state: RecordState) -> String {
    let body = if text.is_empty() {
        String::new()
    } else {
        format!(": {text}")
    };
    format!(
        "окно ошибки HLAE «{title}»{body} (этап «{}»)",
        state.title()
    )
}

/// Игра показала диалог, не похожий на ошибку HLAE: только факт в журнал.
pub fn msg_dialog_seen(title: &str, text: &str) -> String {
    if text.is_empty() {
        format!("игра показала окно «{title}»")
    } else {
        format!("игра показала окно «{title}»: {text}")
    }
}

/// Строка `AFXERROR` пропущена по списку исключений из настроек.
pub fn msg_afx_ignored(text: &str) -> String {
    format!("строка AFXERROR пропущена по списку исключений: {text}")
}

/// Заголовки окон игры в конец сообщения о сбое по дедлайну запуска: не больше 5 непустых.
pub fn msg_windows(list: &[WindowInfo]) -> String {
    let titles: Vec<String> = list
        .iter()
        .map(|w| super::log::clean(&w.title))
        .filter(|t| !t.is_empty())
        .take(5)
        .map(|t| format!("«{t}»"))
        .collect();
    if titles.is_empty() {
        String::new()
    } else {
        format!("; окна игры: {}", titles.join(", "))
    }
}

/// Строка `record.log` со всеми окнами своего job: по ней видно нормальный набор окон (Pitfall 6).
pub fn windows_line(list: &[WindowInfo]) -> String {
    if list.is_empty() {
        return "окна: нет".to_owned();
    }
    let items: Vec<String> = list
        .iter()
        .map(|w| {
            format!(
                "{} [{}] «{}»",
                w.pid,
                super::log::clean(&w.class),
                super::log::clean(&w.title)
            )
        })
        .collect();
    format!("окна: {}", items.join(" · "))
}

/// После выхода игры в job остались процессы (ffmpeg HLAE не дописал файл): закрываем свой job.
pub fn msg_job_not_empty(limit: Duration) -> String {
    format!(
        "после выхода игры в job остались процессы {} с — завершаю свой job",
        secs(limit)
    )
}

/// Игра появилась в нашем job.
pub fn msg_game_appeared(pid: u32) -> String {
    format!("cs2.exe запущена (PID {pid})")
}

/// Процесс игры завершился (отдельно от кода лаунчера).
pub fn msg_game_ended(code: Option<u32>) -> String {
    match code {
        Some(c) => format!("cs2.exe завершилась (код {})", code_text(c)),
        None => "cs2.exe завершилась (код неизвестен)".to_owned(),
    }
}

/// Сколько реального времени заняла запись дубля `n` из `m` — данные для подстройки дедлайна дубля.
pub fn msg_take_timing(n: u32, m: u32, wall: Duration, game_secs: f32) -> String {
    let wall_secs = wall.as_secs_f32();
    let ratio = if game_secs > 0.0 {
        format!("{:.1}", wall_secs / game_secs)
    } else {
        "—".to_owned()
    };
    format!("дубль {n}/{m}: {wall_secs:.0} с записи на {game_secs:.1} с игры (×{ratio})")
}

/// `quit` не ушёл в консоль: дальше ждать выхода игры нечего.
pub fn msg_quit_not_sent(error: &str) -> String {
    format!("quit не отправлен: {error}")
}

/// Игра вышла сама после `quit`.
pub fn msg_quit_exited(elapsed: Duration) -> String {
    format!("игра вышла после quit за {:.1} с", elapsed.as_secs_f64())
}

/// Игра не вышла после `quit` за отведённое время.
pub fn msg_quit_ignored(limit: Duration) -> String {
    format!(
        "игра не вышла за {} с после quit — завершаю свой job",
        secs(limit)
    )
}

/// Принудительная отмена: `quit` и ожидание пропущены (D-12).
pub fn msg_force_cancel() -> String {
    "принудительная отмена — quit и ожидание пропущены".to_owned()
}

/// Свой job завершён.
pub fn msg_job_ended() -> String {
    "свой job завершён".to_owned()
}

/// Выход игры подтверждён по handle.
pub fn msg_exit_confirmed() -> String {
    "игра закрыта (подтверждено по handle)".to_owned()
}

/// Игра жива после завершения job: следующая задача подождёт её (D-14).
pub fn msg_exit_pending(confirm: Duration, next_wait: Duration) -> String {
    format!(
        "игра не закрылась за {} с после завершения job — следующая задача подождёт её до {} с",
        secs(confirm),
        secs(next_wait)
    )
}

/// Выход игры подтвердить не удалось (ошибка опроса).
pub fn msg_exit_unknown(error: &str) -> String {
    format!("закрытие игры не удалось подтвердить: {error}")
}

/// Недописанный дубль помечен.
pub fn msg_partial_marked(name: &str, target: &str) -> String {
    format!("дубль {name} не дописан — помечен как {target}")
}

/// Недописанный дубль пометить не удалось: файл держит чужой процесс, уборка отложена.
pub fn msg_partial_failed(name: &str, error: &str) -> String {
    format!("не удалось пометить {name}: {error} (уборка отложена)")
}

/// Класс сбоя для политики очереди (DIF-03, DIF-04, DC-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailClass {
    /// Игра не дошла до дубля: очередь делает один авто-повтор после уборки и паузы (DIF-04).
    Launch,
    /// Неисправность окружения: два одинаковых сбоя подряд ставят очередь на паузу (DIF-03).
    Environment,
    /// Ни то, ни другое: авто-повтора нет, паузы нет.
    Other,
}

impl FailClass {
    pub fn title(self) -> &'static str {
        match self {
            FailClass::Launch => "запуск",
            FailClass::Environment => "окружение",
            FailClass::Other => "прочее",
        }
    }
}

/// Класс сбоя по коду и состоянию записи, в котором он случился. Без ветки `_`: новый код заставит решить его класс.
/// `GAME_EXITED` до дубля — «игра умерла рано» (запуск); посреди дубля — «прочее» и не повторяется,
/// иначе учение «завершить cs2.exe посреди дубля» прошло бы молча.
pub fn classify(code: FailCode, state: RecordState) -> FailClass {
    match code {
        FailCode::GameNotStarted | FailCode::ConsoleNotOpen | FailCode::DemoNotLoaded => FailClass::Launch,
        FailCode::GameExited if state < RecordState::Take => FailClass::Launch,
        FailCode::GameExited => FailClass::Other,
        FailCode::PrecheckFailed
        | FailCode::ForeignGameRunning
        | FailCode::PrevGameStuck
        | FailCode::HlaeError
        | FailCode::LauncherFailed => FailClass::Environment,
        FailCode::ScriptNotRun
        | FailCode::TakeTimeout
        | FailCode::TakeInvalid
        | FailCode::RetryNoData
        | FailCode::PlayerNotFound
        | FailCode::PlayerAmbiguous
        | FailCode::NoHighlights
        | FailCode::BadDemo
        | FailCode::DemoTooLarge
        | FailCode::ResultInvalid
        | FailCode::UnsupportedSource
        | FailCode::FaceitFetchFailed
        | FailCode::Cancelled => FailClass::Other,
    }
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
        assert_eq!(FailCode::PrecheckFailed.as_str(), "PRECHECK_FAILED");
        assert_eq!(
            FailCode::ForeignGameRunning.as_str(),
            "FOREIGN_GAME_RUNNING"
        );
        assert_eq!(FailCode::PrevGameStuck.as_str(), "PREV_GAME_STUCK");
        assert!(FailCode::PrecheckFailed.hint().contains("Повторить"));
        assert!(FailCode::ForeignGameRunning.hint().contains("не закрывает"));
        assert!(FailCode::PrevGameStuck.hint().contains("30 с"));
        // полный список: 22 разных кода, у каждого непустая строка
        let names: std::collections::HashSet<_> =
            FailCode::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), 22);
        assert!(FailCode::ALL.iter().all(|c| !c.as_str().is_empty()));
        for c in [
            FailCode::PlayerNotFound,
            FailCode::PlayerAmbiguous,
            FailCode::NoHighlights,
            FailCode::BadDemo,
            FailCode::DemoTooLarge,
            FailCode::ResultInvalid,
            FailCode::UnsupportedSource,
            FailCode::FaceitFetchFailed,
        ] {
            assert!(!c.hint().is_empty(), "{c}: нет подсказки");
        }
        assert_eq!(FailCode::PlayerAmbiguous.as_str(), "PLAYER_AMBIGUOUS");
        assert_eq!(FailCode::UnsupportedSource.as_str(), "UNSUPPORTED_SOURCE");
        assert_eq!(FailCode::FaceitFetchFailed.as_str(), "FACEIT_FETCH_FAILED");
        assert!(FailCode::PlayerAmbiguous.hint().contains("SteamID64"));
    }

    #[test]
    fn new_codes_are_class_other_in_any_state() {
        for state in [RecordState::Precheck, RecordState::Take] {
            for code in [
                FailCode::PlayerNotFound,
                FailCode::PlayerAmbiguous,
                FailCode::NoHighlights,
                FailCode::BadDemo,
                FailCode::DemoTooLarge,
                FailCode::ResultInvalid,
                FailCode::UnsupportedSource,
                FailCode::FaceitFetchFailed,
            ] {
                assert_eq!(classify(code, state), FailClass::Other, "{code} в {state:?}");
            }
        }
        assert_eq!(
            classify(FailCode::PlayerAmbiguous, RecordState::Precheck),
            FailClass::Other
        );
    }

    #[test]
    fn player_texts_name_candidates_and_cut_untrusted_input() {
        let list = vec!["a (1)".to_owned(), "b (2)".to_owned()];
        assert_eq!(
            msg_player_ambiguous("a", &list, 0),
            "ник «a» подходит нескольким игрокам: a (1), b (2)"
        );
        assert!(msg_player_ambiguous("a", &list, 3).ends_with("b (2), и ещё 3"));
        assert_eq!(
            msg_player_not_found("zz", &[], 0),
            "игрок «zz» в демке не найден"
        );
        assert!(msg_player_not_found("zz", &list, 0).contains("в демке: a (1), b (2)"));
        assert_eq!(clean_text("a\u{7}b\nc", 64), "abc");
        assert_eq!(clean_text(&"я".repeat(70), 64).chars().count(), 65);
        assert_eq!(
            msg_no_highlights("q"),
            "у игрока q в этом матче нет ни убийств, ни полезных гранат"
        );
    }

    #[test]
    fn take_invalid_names_the_take_in_message_and_hint() {
        assert_eq!(FailCode::TakeInvalid.as_str(), "TAKE_INVALID");
        let msg = msg_take_invalid(2, 3, "take0001", "audio.wav: нет аудиопотока");
        assert!(
            msg.contains("дубль 2 из 3") && msg.contains("take0001"),
            "{msg}"
        );
        assert!(msg.contains("нет аудиопотока"), "{msg}");
        let hint = hint_take_invalid(2, "take0001");
        assert!(
            hint.contains("дубль 2") && hint.contains("take0001"),
            "{hint}"
        );
        assert!(FailCode::TakeInvalid.hint().contains("улика"));
        let f = TakeFacts {
            video_secs: 5.0,
            audio_secs: 5.0,
            width: 1280,
            height: 720,
            video_bytes: 1,
            audio_bytes: 1,
        };
        assert_eq!(
            msg_take_validated(1, 3, &f),
            "дубль 1/3 проверен: 5.0 с, 1280x720, звук 5.0 с"
        );
        let nf = msg_take_not_final("take0000", Duration::from_secs(20), "moov atom not found");
        assert_eq!(
            nf,
            "take0000: файл не дописан за 20 с — moov atom not found"
        );
        let g = msg_take_growing((18087984, 100), Some((20592482, 100)));
        assert!(
            g.contains("video.mp4") && g.contains("18087984") && g.contains("20592482"),
            "{g}"
        );
    }

    #[test]
    fn retry_no_data_has_its_own_code_and_hint() {
        assert_eq!(FailCode::RetryNoData.as_str(), "RETRY_NO_DATA");
        let msg = msg_retry_no_data("record.json нет");
        assert_eq!(
            msg,
            "нет данных для повтора: record.json нет — запустите задачу заново"
        );
        assert!(FailCode::RetryNoData.hint().contains("Заново"));
    }

    #[test]
    fn negative_exit_codes_are_printed_as_hex() {
        assert_eq!(exit_code_text(1), "1");
        assert_eq!(exit_code_text(-1073741819), "0xC0000005");
    }

    #[test]
    fn facts_line_prints_windows_codes_in_hex_and_unknown_as_unknown() {
        let line = |l, g| facts_line(&l, &g);
        assert_eq!(
            line(LauncherFact::Exited(Some(0)), Liveness::Alive),
            "HLAE.exe: код 0 · cs2.exe: жива"
        );
        assert_eq!(
            line(
                LauncherFact::Exited(Some(0)),
                Liveness::Exited(Some(0xC0000005))
            ),
            "HLAE.exe: код 0 · cs2.exe: не жива (код 0xc0000005)"
        );
        assert_eq!(
            line(
                LauncherFact::Running,
                Liveness::Unknown("отказано в доступе".to_owned())
            ),
            "HLAE.exe: ещё работает · cs2.exe: неизвестно (отказано в доступе)"
        );
        assert_eq!(
            line(LauncherFact::NotLaunched, Liveness::NotStarted),
            "HLAE.exe: не запускался · cs2.exe: не запускалась"
        );
        assert_eq!(
            line(
                LauncherFact::Exited(Some(-1073741819)),
                Liveness::Exited(None)
            ),
            "HLAE.exe: код 0xc0000005 · cs2.exe: не жива"
        );
        assert_eq!(
            line(LauncherFact::Exited(None), Liveness::Alive),
            "HLAE.exe: код неизвестен · cs2.exe: жива"
        );
    }

    #[test]
    fn launch_class_covers_start_codes_and_an_early_game_exit() {
        assert_eq!(classify(FailCode::GameNotStarted, RecordState::Game), FailClass::Launch);
        assert_eq!(classify(FailCode::ConsoleNotOpen, RecordState::Console), FailClass::Launch);
        assert_eq!(classify(FailCode::DemoNotLoaded, RecordState::Demo), FailClass::Launch);
        assert_eq!(classify(FailCode::GameExited, RecordState::Demo), FailClass::Launch);
        assert_eq!(classify(FailCode::GameExited, RecordState::Script), FailClass::Launch);
        assert_eq!(FailClass::Launch.title(), "запуск");
    }

    #[test]
    fn game_exit_mid_take_is_not_retried() {
        assert_eq!(classify(FailCode::GameExited, RecordState::Take), FailClass::Other);
        assert_eq!(classify(FailCode::GameExited, RecordState::Closing), FailClass::Other);
    }

    #[test]
    fn environment_class_holds_deterministic_codes() {
        assert_eq!(classify(FailCode::PrecheckFailed, RecordState::Precheck), FailClass::Environment);
        assert_eq!(classify(FailCode::ForeignGameRunning, RecordState::Precheck), FailClass::Environment);
        assert_eq!(classify(FailCode::PrevGameStuck, RecordState::Precheck), FailClass::Environment);
        assert_eq!(classify(FailCode::HlaeError, RecordState::Take), FailClass::Environment);
        assert_eq!(classify(FailCode::LauncherFailed, RecordState::Launcher), FailClass::Environment);
        assert_eq!(FailClass::Environment.title(), "окружение");
    }

    #[test]
    fn everything_else_is_other() {
        assert_eq!(classify(FailCode::TakeInvalid, RecordState::Take), FailClass::Other);
        assert_eq!(classify(FailCode::Cancelled, RecordState::Game), FailClass::Other);
        assert_eq!(classify(FailCode::TakeTimeout, RecordState::Take), FailClass::Other);
        assert_eq!(classify(FailCode::ScriptNotRun, RecordState::Script), FailClass::Other);
        assert_eq!(classify(FailCode::RetryNoData, RecordState::Precheck), FailClass::Other);
        assert_eq!(FailClass::Other.title(), "прочее");
    }
}
