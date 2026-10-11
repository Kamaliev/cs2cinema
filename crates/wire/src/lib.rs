//! Протокол сервера и раннера: только serde-типы и константы.
//!
//! Крейт ни от чего не зависит в воркспейсе и собирается и на Linux, и на Windows: его читают и сервер очереди,
//! и раннер. Правила совместимости: неизвестные варианты перечислений читаются как `Unknown` (сервер новее раннера
//! или наоборот не падает на незнакомом значении), необязательные поля помечаются `#[serde(default)]`.
//! Коды сбоев раннера (`FailCode` v1.0) ходят строкой, чтобы этот крейт не зависел от `pipeline`.

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

/// Коды сбоев, которые ставит сам сервер (остальные приходят от раннера строкой).
pub mod fail {
    /// Задача выдавалась раннеру слишком много раз подряд.
    pub const TOO_MANY_GRANTS: &str = "TOO_MANY_GRANTS";
    /// Сервер не смог принять демку.
    pub const RECEIVE_FAILED: &str = "RECEIVE_FAILED";
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

/// Этап работы раннера. `fetch` — скачивание демки с сервера, `upload` — загрузка результата, остальные —
/// этапы `pipeline::Stage`, раннер (фаза 6) переводит их в эти имена сам. Незнакомое имя читается как `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Fetch,
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
    pub const ALL: [Phase; 9] = [
        Phase::Fetch,
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

    /// Обратное к `as_str` для девяти известных этапов; прочее (в том числе пустая строка) — `None`.
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
