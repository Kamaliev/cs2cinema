//! Протокол сервера и раннера: только serde-типы и константы.
//!
//! Крейт ни от чего не зависит в воркспейсе и собирается и на Linux, и на Windows: его читают и сервер очереди,
//! и раннер. Правила совместимости: неизвестные варианты перечислений читаются как `Unknown` (сервер новее раннера
//! или наоборот не падает на незнакомом значении), необязательные поля помечаются `#[serde(default)]`.
//! Коды сбоев раннера (`FailCode` v1.0) ходят строкой, чтобы этот крейт не зависел от `pipeline`.

pub mod faceit;

use serde::{Deserialize, Serialize};

/// Версия протокола; растёт только при несовместимом изменении.
pub const PROTOCOL_VERSION: u32 = 1;

/// Размер куска при передаче файла: 8 МиБ.
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Состояние задачи на сервере (SRV-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Демка ещё принимается.
    Receiving,
    /// Ждёт раннера.
    Queued,
    /// Выдана раннеру, он ещё не начал.
    Assigned,
    /// Раннер работает.
    Running,
    /// Раннер загружает результат.
    Uploading,
    /// Готово.
    Done,
    /// Сбой.
    Failed,
    /// Отменена.
    Cancelled,
    /// Значение от более новой стороны протокола.
    #[serde(other)]
    Unknown,
}

impl JobState {
    /// Все известные состояния в порядке жизни задачи.
    pub const ALL: [JobState; 8] = [
        JobState::Receiving,
        JobState::Queued,
        JobState::Assigned,
        JobState::Running,
        JobState::Uploading,
        JobState::Done,
        JobState::Failed,
        JobState::Cancelled,
    ];

    /// Строка для базы и JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Receiving => "receiving",
            JobState::Queued => "queued",
            JobState::Assigned => "assigned",
            JobState::Running => "running",
            JobState::Uploading => "uploading",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Unknown => "unknown",
        }
    }

    /// Обратное к `as_str` для восьми известных состояний; прочее — `None`.
    pub fn parse(s: &str) -> Option<JobState> {
        JobState::ALL.into_iter().find(|j| j.as_str() == s)
    }

    /// Задача закончена и состояния больше не меняет.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }

    /// Задача выдана раннеру и держится арендой.
    pub fn is_leased(self) -> bool {
        matches!(
            self,
            JobState::Assigned | JobState::Running | JobState::Uploading
        )
    }

    /// Название для людей.
    pub fn title(self) -> &'static str {
        match self {
            JobState::Receiving => "приём",
            JobState::Queued => "в очереди",
            JobState::Assigned => "выдана раннеру",
            JobState::Running => "идёт",
            JobState::Uploading => "загрузка результата",
            JobState::Done => "готово",
            JobState::Failed => "сбой",
            JobState::Cancelled => "отменена",
            JobState::Unknown => "неизвестно",
        }
    }
}

/// Запрошенная отмена задачи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelMode {
    /// Отмена не запрошена.
    #[serde(rename = "none")]
    NotRequested,
    /// Мягкая: раннер доделывает безопасный шаг.
    Soft,
    /// Жёсткая: раннер обрывает работу.
    Force,
    /// Значение от более новой стороны протокола.
    #[serde(other)]
    Unknown,
}

impl CancelMode {
    /// Строка для базы и JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            CancelMode::NotRequested => "none",
            CancelMode::Soft => "soft",
            CancelMode::Force => "force",
            CancelMode::Unknown => "unknown",
        }
    }

    /// Обратное к `as_str` для трёх известных значений.
    pub fn parse(s: &str) -> Option<CancelMode> {
        [
            CancelMode::NotRequested,
            CancelMode::Soft,
            CancelMode::Force,
        ]
        .into_iter()
        .find(|m| m.as_str() == s)
    }
}

/// Строки кодов сбоев: свои у сервера и все, что может прислать раннер. Менять можно только добавлением:
/// коды читают сервер, раннер и бот.
pub mod fail {
    /// Задача выдавалась раннеру слишком много раз подряд.
    pub const TOO_MANY_GRANTS: &str = "TOO_MANY_GRANTS";
    /// Сервер не смог принять демку.
    pub const RECEIVE_FAILED: &str = "RECEIVE_FAILED";

    // коды записи v1.0 (`pipeline::FailCode`)
    pub const LAUNCHER_FAILED: &str = "LAUNCHER_FAILED";
    pub const GAME_NOT_STARTED: &str = "GAME_NOT_STARTED";
    pub const CONSOLE_NOT_OPEN: &str = "CONSOLE_NOT_OPEN";
    pub const DEMO_NOT_LOADED: &str = "DEMO_NOT_LOADED";
    pub const SCRIPT_NOT_RUN: &str = "SCRIPT_NOT_RUN";
    pub const TAKE_TIMEOUT: &str = "TAKE_TIMEOUT";
    pub const GAME_EXITED: &str = "GAME_EXITED";
    pub const HLAE_ERROR: &str = "HLAE_ERROR";
    pub const CANCELLED: &str = "CANCELLED";
    pub const PRECHECK_FAILED: &str = "PRECHECK_FAILED";
    pub const FOREIGN_GAME_RUNNING: &str = "FOREIGN_GAME_RUNNING";
    pub const PREV_GAME_STUCK: &str = "PREV_GAME_STUCK";
    pub const TAKE_INVALID: &str = "TAKE_INVALID";
    pub const RETRY_NO_DATA: &str = "RETRY_NO_DATA";

    // коды фазы 6
    /// Ника нет в демке.
    pub const PLAYER_NOT_FOUND: &str = "PLAYER_NOT_FOUND";
    /// Ник подходит нескольким игрокам.
    pub const PLAYER_AMBIGUOUS: &str = "PLAYER_AMBIGUOUS";
    /// У игрока нет убийств и полезных гранат.
    pub const NO_HIGHLIGHTS: &str = "NO_HIGHLIGHTS";
    /// Демка не разбирается.
    pub const BAD_DEMO: &str = "BAD_DEMO";
    /// Демка больше допустимого размера.
    pub const DEMO_TOO_LARGE: &str = "DEMO_TOO_LARGE";
    /// Готовый ролик не прошёл проверку перед отправкой.
    pub const RESULT_INVALID: &str = "RESULT_INVALID";
    /// Источник задачи раннер не берёт.
    pub const UNSUPPORTED_SOURCE: &str = "UNSUPPORTED_SOURCE";
    /// Раннер не смог скачать демку FACEIT по ссылке задачи (нет ключа, нет демки у матча, отказ API или сети).
    pub const FACEIT_FETCH_FAILED: &str = "FACEIT_FETCH_FAILED";
    /// Исполнитель раннера вернул ошибку без кода или упал.
    pub const JOB_ERROR: &str = "JOB_ERROR";

    /// Все известные коды. Страж фазы 6 в раннере сверяет с этим списком `pipeline::FailCode::ALL`.
    pub const KNOWN: &[&str] = &[
        LAUNCHER_FAILED,
        GAME_NOT_STARTED,
        CONSOLE_NOT_OPEN,
        DEMO_NOT_LOADED,
        SCRIPT_NOT_RUN,
        TAKE_TIMEOUT,
        GAME_EXITED,
        HLAE_ERROR,
        CANCELLED,
        PRECHECK_FAILED,
        FOREIGN_GAME_RUNNING,
        PREV_GAME_STUCK,
        TAKE_INVALID,
        RETRY_NO_DATA,
        PLAYER_NOT_FOUND,
        PLAYER_AMBIGUOUS,
        NO_HIGHLIGHTS,
        BAD_DEMO,
        DEMO_TOO_LARGE,
        RESULT_INVALID,
        UNSUPPORTED_SOURCE,
        JOB_ERROR,
        TOO_MANY_GRANTS,
        RECEIVE_FAILED,
        FACEIT_FETCH_FAILED,
    ];
}

/// Тело `POST /v1/runner/claim`: раннер сообщает, чем занят.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaimRequest {
    /// Состояние раннера («idle» и т. п.); сервер пока только принимает его.
    #[serde(default)]
    pub status: String,
}

/// Демка задачи: размер и SHA-256 для проверки после скачивания.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DemoInfo {
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub orig_name: String,
}

/// Игрок, чьи моменты нужно смонтировать.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// Steam64, если он известен.
    #[serde(default)]
    pub steam64: Option<String>,
    pub name: String,
}

/// Выдача задачи раннеру: ответ 200 на `claim`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub job_id: i64,
    /// Эпоха выдачи: растёт при каждой новой выдаче, раннер присылает её в `X-Epoch`.
    pub epoch: i64,
    /// Конец аренды по часам сервера, unix-миллисекунды.
    pub lease_until_ms: i64,
    #[serde(default)]
    pub demo: DemoInfo,
    #[serde(default)]
    pub faceit_link: Option<String>,
    pub player: Player,
    #[serde(default)]
    pub requested_by: String,
    /// Параметры монтажа; неизвестные ключи раннер пропускает.
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// Этап работы раннера. `fetch` — скачивание демки с сервера, `upload` — загрузка результата, `precheck` —
/// предпроверка: разбор демки в дочернем процессе и проверка окружения до запуска CS2 (его шлёт раннер сам),
/// остальные — этапы `pipeline::Stage`, раннер переводит их в эти имена сам. Незнакомое имя читается как
/// `Unknown` (старый сервер читает `precheck` как «идёт работа»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Fetch,
    /// Предпроверка: разбор демки в дочернем процессе и проверка окружения до запуска CS2.
    Precheck,
    Download,
    Parse,
    Positions,
    Geometry,
    Plan,
    Record,
    Assemble,
    Upload,
    #[serde(other)]
    Unknown,
}

impl Phase {
    /// Все известные этапы.
    pub const ALL: [Phase; 10] = [
        Phase::Fetch,
        Phase::Precheck,
        Phase::Download,
        Phase::Parse,
        Phase::Positions,
        Phase::Geometry,
        Phase::Plan,
        Phase::Record,
        Phase::Assemble,
        Phase::Upload,
    ];

    /// Строка для базы и JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Fetch => "fetch",
            Phase::Precheck => "precheck",
            Phase::Download => "download",
            Phase::Parse => "parse",
            Phase::Positions => "positions",
            Phase::Geometry => "geometry",
            Phase::Plan => "plan",
            Phase::Record => "record",
            Phase::Assemble => "assemble",
            Phase::Upload => "upload",
            Phase::Unknown => "unknown",
        }
    }

    /// Обратное к `as_str` для десяти известных этапов; прочее (в том числе пустая строка) — `None`.
    pub fn parse(s: &str) -> Option<Phase> {
        Phase::ALL.into_iter().find(|p| p.as_str() == s)
    }
}

/// Номер дубля записи: `n` из `m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Take {
    pub n: u32,
    pub m: u32,
}

/// Тело `POST /v1/jobs/{id}/progress`. Прогресс — он же heartbeat: раннер шлёт его раз в `heartbeat_secs`,
/// отдельной ручки heartbeat нет. Каждый запрос продлевает аренду, если эпоха текущая.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressRequest {
    /// Эпоха выдачи из `Assignment`.
    pub epoch: i64,
    /// Номер сообщения раннера: растёт; не больше последнего принятого — дубль или опоздавший.
    pub seq: u64,
    pub phase: Phase,
    #[serde(default)]
    pub take: Option<Take>,
    /// Короткий текст состояния для людей.
    #[serde(default)]
    pub status: String,
}

/// Ответ на прогресс: новый конец аренды и запрошенная отмена (`none`, если отмены нет).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressReply {
    pub lease_until_ms: i64,
    pub cancel: CancelMode,
}

fn default_true() -> bool {
    true
}

/// Тело `POST /v1/jobs/{id}/release`: раннер возвращает задачу в очередь.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRequest {
    pub epoch: i64,
    /// Почему возвращает (для журнала сервера).
    #[serde(default)]
    pub reason: String,
    /// `false` — раннер возвращает задачу не по её вине, и выдача не засчитывается. Когда так делать,
    /// решает раннер фазы 6; по умолчанию выдача засчитывается.
    #[serde(default = "default_true")]
    pub counts: bool,
}

/// Сбой задачи, как его видит раннер. Коды (`FailCode` v1.0), класс («запуск»/«окружение»/«прочее»), этап и
/// подсказка приходят от раннера строками; сервер хранит их как есть после очистки от секретов и обрезки по длине.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// Код сбоя раннера, например `HLAE_ERROR`; допустимы только `A-Z`, `0-9`, `_`, иначе сервер сохранит `UNKNOWN`.
    pub code: String,
    #[serde(default)]
    pub class: String,
    /// Состояние автомата записи раннера в момент сбоя.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub message: String,
    /// Хвост консоли или журнала раннера.
    #[serde(default)]
    pub tail: String,
    /// Сколько миллисекунд прошло от начала задачи до сбоя.
    #[serde(default)]
    pub elapsed_ms: u64,
}

/// Итог задачи, который сообщает раннер. Незнакомый итог от более новой стороны протокола читается как `Unknown`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// Результат загружен целиком: размер и SHA-256 всего файла. Сервер пересчитывает хеш сам и принимает файл
    /// только при совпадении обоих значений.
    Done {
        size: u64,
        sha256: String,
    },
    Failed(Failure),
    Cancelled,
    #[serde(other)]
    Unknown,
}

/// Тело `POST /v1/jobs/{id}/finish`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishRequest {
    pub epoch: i64,
    pub outcome: Outcome,
}

/// Ответ на `finish`: состояние задачи после итога.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishReply {
    pub state: JobState,
}

/// Что раннер держит у себя: задача и эпоха её выдачи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holding {
    pub job_id: i64,
    pub epoch: i64,
}

/// Тело `POST /v1/runner/hello`: раннер представляется после запуска или обрыва связи и сверяет задачи.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloRequest {
    /// Версия протокола раннера; не совпала с серверной — 426.
    pub protocol: u32,
    /// Имя раннера (токен всё равно решает, кто это); для журнала.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub status: String,
    /// Задачи, которые раннер считает своими.
    #[serde(default)]
    pub holding: Vec<Holding>,
}

/// Что делать раннеру с удерживаемой задачей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveAction {
    /// Продолжать: аренда сервером подтверждена.
    Keep,
    /// Бросить: задача выдана заново, завершена или неизвестна.
    Abandon,
    /// Остановить работу: отмена запрошена.
    Cancel,
    #[serde(other)]
    Unknown,
}

/// Указание по одной задаче из `holding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    pub job_id: i64,
    pub action: DirectiveAction,
}

/// Ответ на `hello`: настройки сервера для раннера и указания по его задачам.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloReply {
    pub server_version: String,
    pub server_time_ms: i64,
    /// Сколько секунд раннер просит задачу в long-poll.
    pub poll_secs: u64,
    /// Как часто слать прогресс.
    pub heartbeat_secs: u64,
    pub chunk_size: u64,
    #[serde(default)]
    pub directives: Vec<Directive>,
}

/// Тело любого ответа с ошибкой.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    /// ASCII-код из `codes`.
    pub error: String,
    /// Текст для человека, по-русски.
    #[serde(default)]
    pub message: String,
    /// Принятое сервером смещение файла (для `offset_mismatch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// ASCII-коды ошибок API; текст для человека — в `ApiError.message` по-русски.
pub mod codes {
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const BAD_REQUEST: &str = "bad_request";
    pub const NOT_FOUND: &str = "not_found";
    pub const TOO_LARGE: &str = "too_large";
    pub const LENGTH_REQUIRED: &str = "length_required";
    pub const OVERLOADED: &str = "overloaded";
    pub const INTERNAL: &str = "internal";
    pub const STALE_EPOCH: &str = "stale_epoch";
    pub const JOB_TERMINAL: &str = "job_terminal";
    pub const ALREADY_HOLDING: &str = "already_holding";
    pub const OFFSET_MISMATCH: &str = "offset_mismatch";
    pub const UPLOAD_BUSY: &str = "upload_busy";
    pub const SHA256_MISMATCH: &str = "sha256_mismatch";
    pub const RANGE_NOT_SATISFIABLE: &str = "range_not_satisfiable";
    pub const PROTOCOL_MISMATCH: &str = "protocol_mismatch";
}

/// Адреса API.
pub mod paths {
    pub const CLAIM: &str = "/v1/runner/claim";
    pub const HELLO: &str = "/v1/runner/hello";

    /// Адрес действия над задачей: `/v1/jobs/{id}/{action}`.
    pub fn job(id: i64, action: &str) -> String {
        format!("/v1/jobs/{id}/{action}")
    }
}

/// Имена заголовков протокола.
pub mod headers {
    pub const EPOCH: &str = "X-Epoch";
    pub const UPLOAD_OFFSET: &str = "Upload-Offset";
    pub const RETRY_AFTER: &str = "Retry-After";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_without_optional_fields_still_parses() {
        let a: Assignment = serde_json::from_str(
            r#"{"job_id":7,"epoch":1,"lease_until_ms":99,"player":{"name":"s1mple"}}"#,
        )
        .unwrap();
        assert_eq!((a.job_id, a.epoch, a.lease_until_ms), (7, 1, 99));
        assert_eq!(a.player.name, "s1mple");
        assert_eq!(a.demo, DemoInfo::default());
        assert!(a.faceit_link.is_none() && a.requested_by.is_empty() && a.options.is_empty());
        assert_eq!(
            serde_json::from_str::<ClaimRequest>("{}").unwrap(),
            ClaimRequest::default()
        );
        assert_eq!(paths::job(5, "heartbeat"), "/v1/jobs/5/heartbeat");
    }

    #[test]
    fn known_codes_are_ascii_and_unique() {
        for code in fail::KNOWN {
            assert!(
                !code.is_empty()
                    && code
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{code}"
            );
        }
        let mut sorted = fail::KNOWN.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), fail::KNOWN.len(), "повторы в KNOWN");
        for code in [
            fail::PLAYER_NOT_FOUND,
            fail::PLAYER_AMBIGUOUS,
            fail::NO_HIGHLIGHTS,
            fail::BAD_DEMO,
            fail::DEMO_TOO_LARGE,
            fail::RESULT_INVALID,
            fail::UNSUPPORTED_SOURCE,
            fail::JOB_ERROR,
            fail::TOO_MANY_GRANTS,
            fail::RECEIVE_FAILED,
        ] {
            assert!(fail::KNOWN.contains(&code), "{code} не в KNOWN");
        }
        assert_eq!(fail::KNOWN.len(), 25);
    }

    #[test]
    fn known_codes_include_faceit_fetch_failed() {
        assert_eq!(fail::FACEIT_FETCH_FAILED, "FACEIT_FETCH_FAILED");
        assert!(fail::KNOWN.contains(&fail::FACEIT_FETCH_FAILED));
        assert_eq!(fail::KNOWN.len(), 25);
        let mut sorted = fail::KNOWN.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 25, "повторов нет");
        assert!(fail::KNOWN.iter().all(|c| {
            c.chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        }));
    }

    #[test]
    fn precheck_phase_is_additive_and_parses() {
        assert_eq!(Phase::parse("precheck"), Some(Phase::Precheck));
        assert_eq!(Phase::ALL.len(), 10);
        let p: ProgressRequest =
            serde_json::from_str(r#"{"epoch":1,"seq":1,"phase":"precheck"}"#).unwrap();
        assert_eq!(p.phase, Phase::Precheck);
    }

    #[test]
    fn progress_request_without_take_and_status_parses() {
        let p: ProgressRequest =
            serde_json::from_str(r#"{"epoch":2,"seq":5,"phase":"record"}"#).unwrap();
        assert_eq!((p.epoch, p.seq, p.phase), (2, 5, Phase::Record));
        assert!(p.take.is_none() && p.status.is_empty());
        let p: ProgressRequest = serde_json::from_str(
            r#"{"epoch":1,"seq":1,"phase":"teleport","take":{"n":1,"m":3},"status":"запись"}"#,
        )
        .unwrap();
        assert_eq!(p.phase, Phase::Unknown, "незнакомый этап не роняет разбор");
        assert_eq!(p.take, Some(Take { n: 1, m: 3 }));
        for ph in Phase::ALL {
            assert_eq!(Phase::parse(ph.as_str()), Some(ph));
            assert_eq!(
                serde_json::to_string(&ph).unwrap(),
                format!("\"{}\"", ph.as_str())
            );
        }
        let r = serde_json::to_string(&ProgressReply {
            lease_until_ms: 9,
            cancel: CancelMode::NotRequested,
        })
        .unwrap();
        assert_eq!(r, r#"{"lease_until_ms":9,"cancel":"none"}"#);
    }

    #[test]
    fn hello_request_defaults_and_unknown_directive_parse() {
        let h: HelloRequest = serde_json::from_str(r#"{"protocol":1}"#).unwrap();
        assert_eq!(h.protocol, 1);
        assert!(h.holding.is_empty() && h.name.is_empty());
        let r: HelloReply = serde_json::from_str(
            r#"{"server_version":"0.1.0","server_time_ms":5,"poll_secs":25,"heartbeat_secs":10,
                "chunk_size":8388608,"directives":[{"job_id":3,"action":"pause"}]}"#,
        )
        .unwrap();
        assert_eq!(r.directives[0].action, DirectiveAction::Unknown);
        assert_eq!(
            serde_json::to_string(&Directive {
                job_id: 3,
                action: DirectiveAction::Keep
            })
            .unwrap(),
            r#"{"job_id":3,"action":"keep"}"#
        );
    }

    #[test]
    fn finish_request_round_trips_failed_and_cancelled() {
        let failed = FinishRequest {
            epoch: 3,
            outcome: Outcome::Failed(Failure {
                code: "HLAE_ERROR".to_owned(),
                class: "запуск".to_owned(),
                state: "take".to_owned(),
                hint: "проверьте HLAE".to_owned(),
                message: "boom".to_owned(),
                tail: "строка".to_owned(),
                elapsed_ms: 1234,
            }),
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains(r#""kind":"failed""#), "{json}");
        assert_eq!(
            serde_json::from_str::<FinishRequest>(&json).unwrap(),
            failed
        );

        let cancelled = FinishRequest {
            epoch: 1,
            outcome: Outcome::Cancelled,
        };
        let json = serde_json::to_string(&cancelled).unwrap();
        assert_eq!(json, r#"{"epoch":1,"outcome":{"kind":"cancelled"}}"#);
        assert_eq!(
            serde_json::from_str::<FinishRequest>(&json).unwrap(),
            cancelled
        );

        // Минимальный сбой: только код; остальное по умолчанию.
        let min: FinishRequest =
            serde_json::from_str(r#"{"epoch":2,"outcome":{"kind":"failed","code":"X"}}"#).unwrap();
        let Outcome::Failed(f) = min.outcome else {
            panic!("ожидался сбой");
        };
        assert_eq!((f.code.as_str(), f.elapsed_ms), ("X", 0));

        // Итог от более новой стороны не роняет разбор.
        let new: FinishRequest =
            serde_json::from_str(r#"{"epoch":2,"outcome":{"kind":"something_new","x":1}}"#)
                .unwrap();
        assert_eq!(new.outcome, Outcome::Unknown);
        assert_eq!(
            serde_json::to_string(&FinishReply {
                state: JobState::Failed
            })
            .unwrap(),
            r#"{"state":"failed"}"#
        );
    }

    #[test]
    fn done_outcome_round_trips() {
        let done = FinishRequest {
            epoch: 2,
            outcome: Outcome::Done {
                size: 17_825_792,
                sha256: "ab".repeat(32),
            },
        };
        let json = serde_json::to_string(&done).unwrap();
        assert!(json.contains(r#""kind":"done""#), "{json}");
        assert!(json.contains(r#""size":17825792"#), "{json}");
        assert_eq!(serde_json::from_str::<FinishRequest>(&json).unwrap(), done);
        // Без размера или хеша итог `done` не читается: Unknown не должен молча принять неполный итог.
        let bad = serde_json::from_str::<FinishRequest>(
            r#"{"epoch":1,"outcome":{"kind":"done","size":5}}"#,
        );
        assert!(bad.is_err(), "итог done без sha256 не разбирается");
    }

    #[test]
    fn job_state_round_trips_through_json_and_db_strings() {
        for s in JobState::ALL {
            let json = serde_json::to_string(&s).unwrap();
            assert_eq!(json, format!("\"{}\"", s.as_str()));
            assert_eq!(serde_json::from_str::<JobState>(&json).unwrap(), s);
            assert_eq!(JobState::parse(s.as_str()), Some(s));
        }
        for m in [
            CancelMode::NotRequested,
            CancelMode::Soft,
            CancelMode::Force,
        ] {
            let json = serde_json::to_string(&m).unwrap();
            assert_eq!(serde_json::from_str::<CancelMode>(&json).unwrap(), m);
            assert_eq!(CancelMode::parse(m.as_str()), Some(m));
        }
    }

    #[test]
    fn unknown_state_from_a_newer_peer_reads_as_unknown() {
        assert_eq!(
            serde_json::from_str::<JobState>("\"paused\"").unwrap(),
            JobState::Unknown
        );
        assert_eq!(JobState::parse("paused"), None);
        assert_eq!(JobState::parse("unknown"), None);
        assert_eq!(
            serde_json::from_str::<CancelMode>("\"later\"").unwrap(),
            CancelMode::Unknown
        );
        assert!(JobState::Done.is_terminal() && !JobState::Queued.is_terminal());
        assert!(JobState::Running.is_leased() && !JobState::Queued.is_leased());
    }
}
