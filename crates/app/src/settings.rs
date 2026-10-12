use std::path::{Path, PathBuf};

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
    /// Подстроки строк AFXERROR, которые не останавливают запись (D-09). Правится руками в settings.json, экрана нет; по умолчанию пусто.
    pub afxerror_ignore: Vec<String>,
    /// Адрес cs2-server для режима раннера; пусто — режим выключен.
    pub runner_server: String,
}

impl Settings {
    pub fn frame_size(&self) -> (u32, u32) {
        let h = self.resolution.clamp(360, 2160) / 2 * 2;
        (h * 16 / 9 / 2 * 2, h)
    }
}

/// Ищет CS2 в стандартных местах Steam и в `libraryfolders.vdf`.
pub fn find_cs2() -> Option<PathBuf> {
    const REL: &str = "steamapps/common/Counter-Strike Global Offensive";
    let mut libs: Vec<PathBuf> = Vec::new();
    for root in ["C:/Program Files (x86)/Steam", "C:/Program Files/Steam", "D:/Steam", "D:/SteamLibrary", "E:/SteamLibrary"] {
        libs.push(root.into());
        if let Ok(vdf) = std::fs::read_to_string(Path::new(root).join("steamapps/libraryfolders.vdf")) {
            for line in vdf.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("\"path\"") {
                    let p = rest.trim().trim_matches('"').replace("\\\\", "/");
                    libs.push(p.into());
                }
            }
        }
    }
    libs.into_iter().map(|l| l.join(REL)).find(|p| p.join("game/bin/win64").is_dir())
}

/// Ищет папку с `HLAE.exe`: типичные места, затем неглубокий обход дисков и профиля пользователя.
pub fn find_hlae() -> Option<PathBuf> {
    fn has_hlae(d: &Path) -> bool {
        d.join("HLAE.exe").is_file()
    }
    // обход вглубь не больше `depth`, служебные и огромные папки пропускаем
    fn walk(dir: &Path, depth: u32) -> Option<PathBuf> {
        if has_hlae(dir) {
            return Some(dir.to_path_buf());
        }
        if depth == 0 {
            return None;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if matches!(name.as_str(), "windows" | "$recycle.bin" | "system volume information" | "node_modules" | "target" | ".git" | "steamapps" | "appdata")
            {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if let Some(p) = walk(&e.path(), depth - 1) {
                    return Some(p);
                }
            }
        }
        None
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["USERPROFILE", "LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
        if let Some(v) = std::env::var_os(var) {
            roots.push(v.into());
        }
    }
    if let Some(p) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.extend(["Downloads", "Desktop", "Documents"].map(|d| p.join(d)));
    }
    // HLAE — портативная программа: чаще всего распакована прямо в корень диска или в Downloads
    let drives: Vec<PathBuf> = ('A'..='Z').map(|c| PathBuf::from(format!("{c}:/"))).filter(|p| p.is_dir()).collect();
    roots.extend(drives.iter().cloned());
    // сначала быстро (глубина 3 в известных местах), затем глубже
    for depth in [3, 5] {
        for r in roots.iter().filter(|r| r.is_dir()) {
            let d = if drives.contains(r) { depth - 1 } else { depth };
            if let Some(p) = walk(r, d) {
                return Some(p);
            }
        }
    }
    None
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hlae_dir: String::new(),
            cs2_dir: find_cs2().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
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
            afxerror_ignore: Vec::new(),
            runner_server: String::new(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
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

    #[test]
    fn settings_without_afxerror_ignore_still_load() {
        let old: Settings = serde_json::from_str(r#"{"hlae_dir":"C:/hlae","fps":90}"#).unwrap();
        assert!(old.afxerror_ignore.is_empty(), "старый файл без поля — пустой список");
        let new: Settings = serde_json::from_str(r#"{"afxerror_ignore":["shader cache","audio"]}"#).unwrap();
        assert_eq!(new.afxerror_ignore, vec!["shader cache".to_owned(), "audio".to_owned()]);
    }

    #[test]
    fn settings_without_runner_server_still_load() {
        let old: Settings = serde_json::from_str(r#"{"hlae_dir":"C:/hlae","fps":90}"#).unwrap();
        assert!(old.runner_server.is_empty(), "старый файл без поля — режим раннера выключен");
        assert!(Settings::default().runner_server.is_empty(), "по умолчанию адрес сервера не задан");
        let new: Settings = serde_json::from_str(r#"{"runner_server":"http://192.168.1.5:8080"}"#).unwrap();
        assert_eq!(new.runner_server, "http://192.168.1.5:8080");
    }
}
