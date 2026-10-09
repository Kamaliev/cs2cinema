//! Запуск CS2 через HLAE, управление консолью игры и ожидание готовых клипов.

use std::{
    io::Write,
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant, SystemTime},
};

pub const NETCON_PORT: u16 = 29_700;

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

pub fn launch(hlae_dir: &str, cs2_dir: &str, demo_name: &str) -> Result<Child, String> {
    let hook = Path::new(hlae_dir).join("x64/AfxHookSource2.dll");
    let cmdline = format!(
        "-insecure -steam -novid -allow_third_party_software -netconport {NETCON_PORT} -windowed -w 1920 -h 1080 +demo_ui_mode 0 +playdemo {demo_name}"
    );
    let mut cmd = Command::new(Path::new(hlae_dir).join("HLAE.exe"));
    cmd.current_dir(hlae_dir)
        .args(["-customLoader", "-noGui", "-autoStart", "-hookDllPath"])
        .arg(hook)
        .arg("-programPath")
        .arg(cs2_exe(cs2_dir))
        .args(["-cmdLine", &cmdline]);
    cmd.spawn().map_err(|e| format!("не удалось запустить HLAE: {e}"))
}

/// Ждёт, пока игра откроет консольный порт, и отправляет команды.
pub fn send_console(cmds: &[&str], timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", NETCON_PORT)) {
            Ok(mut s) => {
                for c in cmds {
                    s.write_all(format!("{c}\n").as_bytes()).map_err(|e| e.to_string())?;
                    std::thread::sleep(Duration::from_millis(300));
                }
                return Ok(());
            }
            Err(e) if start.elapsed() > timeout => return Err(format!("консоль игры не открылась ({e})")),
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

fn newest_mtime(dir: &Path) -> Option<(SystemTime, usize)> {
    let mut best: Option<SystemTime> = None;
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
                n += 1;
                best = Some(best.map_or(t, |b| b.max(t)));
            }
        }
    }
    best.map(|b| (b, n))
}

/// Ждёт, пока HLAE допишет клип последнего шота (папка появилась после `since` и перестала меняться).
/// `alive` — вернуть false, если игра закрыта: тогда ждать дальше нечего.
pub fn wait_for_clips(out_dir: &Path, last_shot: usize, since: SystemTime, alive: &mut dyn FnMut() -> bool, progress: &dyn Fn(&str)) -> Result<(), String> {
    let dir = out_dir.join("clips").join(format!("take{last_shot:04}"));
    let mut last_seen: Option<(SystemTime, usize, Instant)> = None;
    loop {
        if let Some((t, n)) = newest_mtime(&dir) {
            if t >= since {
                progress(&format!("записано кадров: {n}"));
                match last_seen {
                    Some((pt, pn, since_i)) if pt == t && pn == n => {
                        if since_i.elapsed() > Duration::from_secs(8) && !alive() {
                            return Ok(());
                        }
                        if since_i.elapsed() > Duration::from_secs(20) {
                            return Ok(());
                        }
                    }
                    _ => last_seen = Some((t, n, Instant::now())),
                }
            }
        } else if !alive() {
            return Err("игра закрыта, а последний клип так и не появился".into());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
