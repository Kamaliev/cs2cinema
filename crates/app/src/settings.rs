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
    /// Запас по времени: множитель всех дедлайнов записи и склейки (1.0 — как задумано, больше — для медленной машины).
    pub time_margin: f32,
    /// Высота кадра (720 / 1080 / 1440): окно игры и ролик, 16:9.
    pub resolution: u32,
    /// Кодировать на видеокарте NVIDIA (NVENC), если она есть.
    pub gpu_encode: bool,
    /// Подробный ролик: весь KAST и все ракурсы (в ~2 раза длиннее).
    pub detailed: bool,
}

impl Settings {
    pub fn frame_size(&self) -> (u32, u32) {
        let h = self.resolution.clamp(360, 2160) / 2 * 2;
        (h * 16 / 9 / 2 * 2, h)
    }
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
            time_margin: 1.0,
            resolution: 720,
            gpu_encode: true,
            detailed: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_file_with_load_wait_still_loads() {
        let s: Settings = serde_json::from_str(r#"{"hlae_dir":"C:/hlae","load_wait_secs":40,"fps":90}"#).unwrap();
        assert_eq!((s.hlae_dir.as_str(), s.fps), ("C:/hlae", 90));
        assert_eq!(s.time_margin, 1.0, "прежнее поле игнорируется, запас по умолчанию ×1.0");
    }
}
