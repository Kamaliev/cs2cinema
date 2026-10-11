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

#[cfg(test)]
mod tests {
    use super::*;

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
