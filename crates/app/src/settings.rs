use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Папка с `HLAE.exe`.
    pub hlae_dir: String,
    /// Папка установки CS2 (внутри неё `game/bin/win64/cs2.exe`).
    pub cs2_dir: String,
    /// Куда складывать планы, скрипты и готовые ролики.
    pub out_root: String,
    pub faceit_key: String,
    /// Ник на FACEIT — для списка матчей.
    pub nickname: String,
    pub fps: u32,
    pub chronological: bool,
    pub flybys: bool,
    pub walls: bool,
    pub minimize_to_tray: bool,
    pub auto_update: bool,
    /// Сколько секунд ждать загрузки демки перед `exec highlights`.
    pub load_wait_secs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hlae_dir: String::new(),
            cs2_dir: crate::launch::find_cs2().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            out_root: default_out_root().to_string_lossy().into_owned(),
            faceit_key: String::new(),
            nickname: String::new(),
            fps: 60,
            chronological: false,
            flybys: true,
            walls: true,
            minimize_to_tray: true,
            auto_update: true,
            load_wait_secs: 25,
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("XDG_DATA_HOME")).map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
    });
    base.unwrap_or_else(|| PathBuf::from(".")).join("cs2cinema")
}

fn default_out_root() -> PathBuf {
    // без пробелов: путь попадает в консольные команды HLAE
    data_dir().join("out")
}

impl Settings {
    fn file() -> PathBuf {
        data_dir().join("settings.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::file(), s);
        }
    }
}
