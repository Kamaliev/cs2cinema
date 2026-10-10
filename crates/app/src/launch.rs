//! Поиск CS2 и HLAE, проверка путей, установка файлов записи в игру, проверка NVENC. Запуск игры, консоль и живость — в `host::win::WinHost`.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
pub fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
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
            if matches!(name.as_str(), "windows" | "$recycle.bin" | "system volume information" | "node_modules" | "target" | ".git" | "steamapps" | "appdata") {
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

pub fn cs2_exe(cs2_dir: &str) -> PathBuf {
    Path::new(cs2_dir).join("game/bin/win64/cs2.exe")
}

/// Проверка путей перед запуском; возвращает понятное сообщение об ошибке.
pub fn check_paths(hlae_dir: &str, cs2_dir: &str) -> Result<(), String> {
    if !Path::new(hlae_dir).join("HLAE.exe").is_file() {
        return Err("Укажите папку HLAE в настройках (там должен лежать HLAE.exe)".into());
    }
    if !Path::new(hlae_dir).join("x64/AfxHookSource2.dll").is_file() {
        return Err("В папке HLAE нет x64\\AfxHookSource2.dll — нужна свежая версия HLAE с поддержкой CS2".into());
    }
    if !cs2_exe(cs2_dir).is_file() {
        return Err("Не найден cs2.exe: укажите папку «Counter-Strike Global Offensive» в настройках".into());
    }
    Ok(())
}

/// Копирует скрипт и демку в игру. Возвращает имя демки (без `.dem`) для `playdemo`.
pub fn install_files(cs2_dir: &str, out_dir: &Path, demo_name: &str, write_demo: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    let csgo = Path::new(cs2_dir).join("game/csgo");
    let cfg_dir = csgo.join("cfg");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| format!("{}: {e}", cfg_dir.display()))?;
    // highlights.cfg, скрипты шотов cs2c_NN.cfg и пути камер cs2c_NN_K.xml (HLAE грузит их от game/bin/win64)
    for e in std::fs::read_dir(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name == "highlights.cfg" || (name.starts_with("cs2c_") && (name.ends_with(".cfg") || name.ends_with(".xml")));
        if ours {
            std::fs::copy(e.path(), cfg_dir.join(&name)).map_err(|e| format!("копирование {name}: {e}"))?;
        }
    }
    write_demo(&csgo.join(format!("{demo_name}.dem")))
}

/// ffmpeg, которым пишет HLAE: `Path=` из `ffmpeg\ffmpeg.ini` в папке HLAE.
pub fn hlae_ffmpeg(hlae_dir: &str) -> Option<PathBuf> {
    let ini = std::fs::read_to_string(Path::new(hlae_dir).join("ffmpeg/ffmpeg.ini")).ok()?;
    let path = ini.lines().find_map(|l| l.trim().strip_prefix("Path=").map(|p| p.trim().trim_matches('"').to_owned()))?;
    Some(PathBuf::from(path)).filter(|p| p.is_file())
}

/// Работает ли NVENC у этого ffmpeg: короткий пробный энкод (кодер может быть в сборке, но без драйвера NVIDIA).
pub fn has_nvenc(ffmpeg: &Path) -> bool {
    nvenc_check(ffmpeg).is_ok()
}

/// Пробное кодирование NVENC; в ошибке — первая строка ffmpeg (например, «драйвер старее, чем нужно»).
pub fn nvenc_check(ffmpeg: &Path) -> Result<(), String> {
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=256x144:d=0.2", "-c:v", "h264_nvenc", "-f", "null", "-"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    no_window(&mut c);
    let out = c.output().map_err(|e| format!("ffmpeg не запустился: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().find(|l| l.contains("nvenc") || l.contains("driver") || l.contains("Driver")).or_else(|| err.lines().next()).unwrap_or("");
    // «[h264_nvenc @ 0x…] Driver does not support…» — префикс не нужен
    let msg = line.rsplit("] ").next().unwrap_or(line).trim();
    Err(if msg.is_empty() { "h264_nvenc не заработал".into() } else { msg.to_owned() })
}
