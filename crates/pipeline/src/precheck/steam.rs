//! Состояние CS2 в Steam: разбор `appmanifest_730.acf` и `steam.inf` (DIF-01, DIF-02).
//!
//! Манифест — текст другой программы, поэтому разбор построчный и без внешних крейтов, как в
//! `launch::find_cs2`; нераспознанное даёт `None`, а не ошибку (T-02-10): по мусору старт не блокируется.

use std::path::{Path, PathBuf};

/// Биты `StateFlags`, которые означают «Steam сейчас что-то делает с игрой»: идёт обновление,
/// удаление, резервное копирование и т. п. (таблица LinuxGSM, 2015; A2 исследования).
const UPDATING_MASK: u64 = 256 // UpdateRunning
    | 1024 // UpdateStarted
    | 2048 // Uninstalling
    | 4096 // BackupRunning
    | 65536 // Reconfiguring
    | 131072 // Validating
    | 262144 // AddingFiles
    | 524288 // Preallocating
    | 1048576 // Downloading
    | 2097152 // Staging
    | 4194304 // Committing
    | 8388608; // UpdateStopping
/// `FullyInstalled`.
const FULLY_INSTALLED: u64 = 4;
/// `UpdateRequired` и `UpdatePaused` (A2: 512 — пауза обновления): обновление ждёт, но не идёт.
const UPDATE_WAITING: u64 = 2 | 512;
/// `FilesMissing` и `FilesCorrupt`.
const FILES_BROKEN: u64 = 32 | 128;

/// Нужные поля `appmanifest_730.acf`; чего нет или не разобралось — `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    pub state_flags: Option<u64>,
    pub build_id: Option<String>,
    pub target_build_id: Option<String>,
    pub bytes_to_download: Option<u64>,
    pub bytes_downloaded: Option<u64>,
}

/// Значение строки вида `"key"<пробелы>"value"`, если строка — именно этот ключ.
fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line
        .trim()
        .strip_prefix('"')?
        .strip_prefix(key)?
        .strip_prefix('"')?;
    rest.trim().strip_prefix('"')?.split('"').next()
}

/// Разобрать манифест; берётся первое вхождение каждого ключа.
pub fn parse_manifest(text: &str) -> Manifest {
    let first = |key: &str| text.lines().find_map(|l| value_of(l, key));
    let num = |key: &str| first(key).and_then(|v| v.trim().parse::<u64>().ok());
    let id = |key: &str| {
        first(key)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    Manifest {
        state_flags: num("StateFlags"),
        build_id: id("buildid"),
        target_build_id: id("TargetBuildID"),
        bytes_to_download: num("BytesToDownload"),
        bytes_downloaded: num("BytesDownloaded"),
    }
}

/// Где лежит манифест CS2: папка игры — `<library>/steamapps/common/Counter-Strike Global Offensive`,
/// манифест — в `steamapps` на два уровня выше.
pub fn manifest_path(cs2_dir: &Path) -> Option<PathBuf> {
    Some(cs2_dir.parent()?.parent()?.join("appmanifest_730.acf"))
}

/// Что Steam делает с CS2.
#[derive(Debug, Clone, PartialEq)]
pub enum SteamState {
    /// Установлена, ничего не происходит.
    Idle,
    /// Идёт обновление или другая работа Steam; значение — `StateFlags`.
    Updating(u64),
    /// Обновление ждёт или недокачано, но не идёт.
    UpdatePending(u64),
    /// Установлена не полностью или повреждена.
    NotInstalled(u64),
    /// Флагов нет или они не разобрались.
    Unknown,
}

/// Состояние CS2 по манифесту.
pub fn steam_state(m: &Manifest) -> SteamState {
    let Some(f) = m.state_flags else {
        return SteamState::Unknown;
    };
    if f & UPDATING_MASK != 0 {
        return SteamState::Updating(f);
    }
    if f & FILES_BROKEN != 0 {
        return SteamState::NotInstalled(f);
    }
    if f & FULLY_INSTALLED == 0 {
        return if f & UPDATE_WAITING != 0 {
            SteamState::UpdatePending(f)
        } else {
            SteamState::NotInstalled(f)
        };
    }
    let target_ahead = match (&m.target_build_id, &m.build_id) {
        (Some(t), Some(b)) => t != "0" && t != b,
        (Some(t), None) => t != "0",
        _ => false,
    };
    let unfinished =
        matches!((m.bytes_to_download, m.bytes_downloaded), (Some(all), Some(done)) if done < all);
    if f & UPDATE_WAITING != 0 || target_ahead || unfinished {
        SteamState::UpdatePending(f)
    } else {
        SteamState::Idle
    }
}

/// Версия патча CS2 из `steam.inf` (`PatchVersion=1.41.9.0`).
pub fn patch_version(steam_inf: &str) -> Option<String> {
    steam_inf
        .lines()
        .find_map(|l| l.trim().strip_prefix("PatchVersion="))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"Universe\"\t\t\"1\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n\t\"BytesToDownload\"\t\t\"0\"\n\t\"BytesDownloaded\"\t\t\"0\"\n\t\"TargetBuildID\"\t\t\"0\"\n}\n";

    fn with_flags(flags: u64) -> Manifest {
        Manifest {
            state_flags: Some(flags),
            build_id: Some("100".into()),
            target_build_id: Some("0".into()),
            ..Manifest::default()
        }
    }

    #[test]
    fn idle_manifest_from_the_author_machine_is_idle() {
        let m = parse_manifest(IDLE);
        assert_eq!(m.state_flags, Some(4));
        assert_eq!(m.build_id.as_deref(), Some("25815307"));
        assert_eq!(m.target_build_id.as_deref(), Some("0"));
        assert_eq!(
            (m.bytes_to_download, m.bytes_downloaded),
            (Some(0), Some(0))
        );
        assert_eq!(steam_state(&m), SteamState::Idle);
    }

    #[test]
    fn update_running_bits_mean_updating() {
        assert_eq!(steam_state(&with_flags(4 | 256)), SteamState::Updating(260));
        assert_eq!(
            steam_state(&with_flags(4 | 1048576)),
            SteamState::Updating(4 | 1048576)
        );
        assert_eq!(steam_state(&with_flags(1024)), SteamState::Updating(1024));
        assert_eq!(
            steam_state(&with_flags(4 | 64)),
            SteamState::Idle,
            "бит 64 — игра запущена, не обновление"
        );
        assert_eq!(steam_state(&with_flags(1)), SteamState::NotInstalled(1));
        assert_eq!(
            steam_state(&with_flags(4 | 32)),
            SteamState::NotInstalled(36),
            "файлы отсутствуют"
        );
        assert_eq!(
            steam_state(&with_flags(4 | 128)),
            SteamState::NotInstalled(132),
            "файлы повреждены"
        );
    }

    #[test]
    fn update_required_only_pends() {
        assert_eq!(steam_state(&with_flags(6)), SteamState::UpdatePending(6));
        assert_eq!(
            steam_state(&with_flags(4 | 512)),
            SteamState::UpdatePending(516),
            "512 — пауза обновления"
        );
        let ahead = Manifest {
            target_build_id: Some("200".into()),
            ..with_flags(4)
        };
        assert_eq!(steam_state(&ahead), SteamState::UpdatePending(4));
        let unfinished = Manifest {
            bytes_to_download: Some(100),
            bytes_downloaded: Some(40),
            ..with_flags(4)
        };
        assert_eq!(steam_state(&unfinished), SteamState::UpdatePending(4));
    }

    #[test]
    fn missing_flags_are_unknown() {
        assert_eq!(steam_state(&parse_manifest("")), SteamState::Unknown);
        assert_eq!(
            steam_state(&parse_manifest("мусор\n\"StateFlags\" \"не число\"")),
            SteamState::Unknown
        );
        assert_eq!(parse_manifest("\"StateFlags\"").state_flags, None);
    }

    #[test]
    fn manifest_lives_two_levels_above_the_game_dir() {
        let dir =
            Path::new("D:/Games/SteamLibrary/steamapps/common/Counter-Strike Global Offensive");
        assert_eq!(
            manifest_path(dir),
            Some(Path::new("D:/Games/SteamLibrary/steamapps").join("appmanifest_730.acf"))
        );
        assert_eq!(manifest_path(Path::new("x")), None);
    }

    #[test]
    fn patch_version_is_read_from_steam_inf() {
        let inf =
            "ClientVersion=2000\nServerVersion=2000\nPatchVersion=1.41.9.0\nProductName=cs2\n";
        assert_eq!(patch_version(inf).as_deref(), Some("1.41.9.0"));
        assert_eq!(patch_version("PatchVersion=\n"), None);
        assert_eq!(patch_version("ничего"), None);
    }
}
