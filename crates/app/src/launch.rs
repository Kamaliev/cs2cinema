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

pub fn cs2_exe(cs2_dir: &str) -> PathBuf {
    Path::new(cs2_dir).join("game/bin/win64/cs2.exe")
}

/// Где HLAE хранит клипы: `game/bin/win64` (внутри `clips/shot_NN/takeNNNN`).
pub fn clips_root(cs2_dir: &str) -> PathBuf {
    Path::new(cs2_dir).join("game/bin/win64")
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
    std::fs::copy(out_dir.join("highlights.cfg"), cfg_dir.join("highlights.cfg")).map_err(|e| format!("копирование highlights.cfg: {e}"))?;
    write_demo(&csgo.join(format!("{demo_name}.dem")))
}

pub fn launch(hlae_dir: &str, cs2_dir: &str, demo_name: &str) -> Result<Child, String> {
    let hook = Path::new(hlae_dir).join("x64/AfxHookSource2.dll");
    let cmdline = format!(
        "-insecure -steam -novid -allow_third_party_software -netconport {NETCON_PORT} -windowed -w 1920 -h 1080 +playdemo {demo_name}"
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
pub fn wait_for_clips(root: &Path, last_shot: usize, since: SystemTime, alive: &mut dyn FnMut() -> bool, progress: &dyn Fn(&str)) -> Result<(), String> {
    let dir = root.join("clips").join(format!("shot_{:02}", last_shot + 1));
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
