//! Запуск CS2 через HLAE, управление консолью игры и ожидание готовых клипов.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::mpsc,
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

/// Свободный TCP-порт для консоли игры (если не нашёлся — `NETCON_PORT`).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(NETCON_PORT)
}

pub fn launch(hlae_dir: &str, cs2_dir: &str, demo_name: &str, (w, h): (u32, u32), port: u16) -> Result<Child, String> {
    let hook = Path::new(hlae_dir).join("x64/AfxHookSource2.dll");
    // окно = кадр ролика: лишние пиксели игра рендерит и кодирует зря
    let cmdline = format!(
        "-insecure -steam -novid -allow_third_party_software -netconport {port} -windowed -w {w} -h {h} +demo_ui_mode 0 +playdemo {demo_name}"
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

/// Консоль игры по netcon: команды туда, строки вывода — обратно (читаются в своём потоке).
pub struct Console {
    stream: TcpStream,
    lines: mpsc::Receiver<String>,
}

impl Console {
    /// Ждёт, пока игра откроет консольный порт.
    pub fn connect(port: u16, timeout: Duration, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        let start = Instant::now();
        loop {
            if cancelled() {
                return Err(pipeline::CANCELLED.to_owned());
            }
            match Self::open(port) {
                Ok(c) => return Ok(c),
                Err(e) if start.elapsed() > timeout => return Err(format!("консоль игры не открылась ({e})")),
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    }

    /// Одна попытка подключиться (для повторов, пока ждём дубли).
    pub fn try_connect(port: u16) -> Option<Self> {
        Self::open(port).ok()
    }

    fn open(port: u16) -> std::io::Result<Self> {
        let stream = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_secs(2))?;
        let (tx, lines) = mpsc::channel();
        let reader = stream.try_clone()?;
        std::thread::spawn(move || {
            let mut r = BufReader::new(reader);
            let mut buf = Vec::new();
            // вывод консоли не обязан быть UTF-8 — читаем байты
            while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                if tx.send(String::from_utf8_lossy(&buf).trim().to_owned()).is_err() {
                    break;
                }
                buf.clear();
            }
        });
        Ok(Self { stream, lines })
    }

    pub fn send(&mut self, cmd: &str) -> Result<(), String> {
        self.stream.write_all(format!("{cmd}\n").as_bytes()).map_err(|e| format!("консоль игры: {e}"))
    }

    /// Строки, пришедшие с прошлого вызова.
    pub fn drain(&self) -> Vec<String> {
        self.lines.try_iter().collect()
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

/// Запущена ли игра. HLAE.exe — только лаунчер и завершается сразу после старта cs2,
/// поэтому смотрим на сам процесс cs2.exe.
pub fn game_alive() -> bool {
    #[cfg(windows)]
    {
        let mut c = Command::new("tasklist");
        c.args(["/FI", "IMAGENAME eq cs2.exe", "/NH", "/FO", "CSV"]);
        match no_window(&mut c).output() {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains("cs2.exe"),
            Err(_) => true, // не смогли спросить — считаем, что жива, и ждём дальше
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Закрыть игру (при отмене записи).
pub fn kill_game() {
    #[cfg(windows)]
    {
        let mut c = Command::new("taskkill");
        c.args(["/IM", "cs2.exe", "/F"]);
        let _ = no_window(&mut c).output();
    }
}

/// Ждёт, пока HLAE допишет клип последнего дубля (папка появилась после `since` и перестала меняться).
/// `finished` — игра сообщила, что последний дубль записан (маркер в консоли): тогда хватает 3 с тишины,
/// иначе — 20 с. `alive` — false, если игра закрыта: ждать дальше нечего; `cancelled` — отмена задачи.
pub fn wait_for_clips(
    out_dir: &Path,
    last_take: usize,
    since: SystemTime,
    finished: &mut dyn FnMut() -> bool,
    alive: &dyn Fn() -> bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let dir = out_dir.join("clips").join(format!("take{last_take:04}"));
    // размер, а не только время: HLAE дописывает mp4 после конца записи
    let mut last_seen: Option<(SystemTime, u64, Instant)> = None;
    loop {
        if cancelled() {
            return Err(pipeline::CANCELLED.to_owned());
        }
        // спрашиваем каждый раз: заодно показывает прогресс по дублям
        let finished = finished();
        if let Some((t, size)) = newest_mtime(&dir).map(|(t, _)| (t, dir_size(&dir))) {
            if t >= since {
                match last_seen {
                    Some((pt, ps, quiet)) if pt == t && ps == size => {
                        let q = quiet.elapsed();
                        if q > Duration::from_secs(3) && finished || q > Duration::from_secs(8) && !alive() || q > Duration::from_secs(20) {
                            return Ok(());
                        }
                    }
                    _ => last_seen = Some((t, size, Instant::now())),
                }
            }
        } else if !alive() {
            return Err("игра закрыта, а последний клип так и не появился".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}
