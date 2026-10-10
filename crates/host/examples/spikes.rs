//! Стенд спайков S1–S4: проверки поведения HLAE и CS2 на машине автора.
//!
//! Стенд построен на настоящем `host::win::job::Job` (а не на одноразовом коде) и снимает
//! транскрипты в `%LOCALAPPDATA%\cs2cinema\spikes\<метка>\`. Результаты переносятся вручную в
//! `01-SPIKES.md` фазы 1. Чужие процессы стенд не трогает: при уже запущенном `cs2.exe` он
//! отказывается работать, а свои процессы завершает только через собственный Job Object.

#[cfg(not(windows))]
fn main() {
    println!("стенд спайков — только для Windows");
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    win::main()
}

#[cfg(windows)]
mod win {
    use host::Liveness;
    use host::win::job::Job;
    use host::win::process::{OwnedProc, snapshot_processes};
    use std::collections::{HashMap, HashSet};
    use std::fs::{self, File, OpenOptions};
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const USAGE: &str = "\
стенд спайков S1–S4 (только Windows): проверки поведения HLAE и CS2 на этой машине

использование: cargo run -p host --example spikes -- <подкоманда> [опции]

подкоманды:
  job        S1: попадает ли cs2.exe (и ffmpeg от HLAE) в наш Job Object
  netcon     S2: насколько надёжен консольный порт и помогает ли -afxFixNetCon
  failures   S3: что печатает консоль при сбоях (нет демки, пустая демка, нет cfg, поздний netcon)
  abspath    S4: переживает ли +playdemo абсолютный путь с пробелами и кириллицей

опции:
  --hlae <папка>   папка HLAE (по умолчанию C:\\Program Files (x86)\\HLAE)
  --cs2 <папка>    папка CS2 с game\\ (по умолчанию $CS2_DIR или каталог Steam)
  --demo <имя>     имя .dem в game\\csgo (по умолчанию самая новая, кроме cs2cinema_spike_*)
  --runs <N>       число запусков для netcon (по умолчанию 5)
  --fix on|off     netcon: добавлять ли -afxFixNetCon; без опции запуски чередуются off/on
  --dry-run        только предпроверка и строки запуска, ничего не запускать
  --help           эта справка

Перед запуском игры закройте CS2 вручную (стенд чужие процессы не трогает) и запустите Steam.
Каждая подкоманда укладывается в 10 минут. Результат: %LOCALAPPDATA%\\cs2cinema\\spikes\\<метка>\\
(транскрипты и summary.txt).";

    /// Сколько стенд готов работать над одной подкомандой, прежде чем перестать запускать новые игры.
    const SPIKE_BUDGET: Duration = Duration::from_secs(8 * 60);

    struct Opts {
        cmd: Option<String>,
        hlae: PathBuf,
        cs2: PathBuf,
        demo: Option<String>,
        runs: u32,
        fix: Option<bool>,
        dry_run: bool,
        help: bool,
    }

    fn parse(args: &[String]) -> Result<Opts, String> {
        let mut o = Opts {
            cmd: None,
            hlae: PathBuf::from(r"C:\Program Files (x86)\HLAE"),
            cs2: std::env::var_os("CS2_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive")),
            demo: None,
            runs: 5,
            fix: None,
            dry_run: false,
            help: false,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = |name: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("у {name} нет значения"))
            };
            match a.as_str() {
                "--help" | "-h" => o.help = true,
                "--dry-run" => o.dry_run = true,
                "--hlae" => o.hlae = PathBuf::from(val("--hlae")?),
                "--cs2" => o.cs2 = PathBuf::from(val("--cs2")?),
                "--demo" => o.demo = Some(val("--demo")?),
                "--runs" => {
                    o.runs = val("--runs")?
                        .parse()
                        .map_err(|_| "--runs ждёт число".to_owned())?
                }
                "--fix" => {
                    o.fix = match val("--fix")?.as_str() {
                        "on" => Some(true),
                        "off" => Some(false),
                        _ => return Err("--fix ждёт on или off".into()),
                    }
                }
                s if s.starts_with("--") => return Err(format!("неизвестная опция {s}")),
                s if o.cmd.is_none() => o.cmd = Some(s.to_owned()),
                s => return Err(format!("лишний аргумент {s}")),
            }
        }
        Ok(o)
    }

    pub fn main() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match run(&args) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("ошибка: {e}");
                ExitCode::from(1)
            }
        }
    }

    fn run(args: &[String]) -> Result<ExitCode, String> {
        let o = parse(args)?;
        if o.help {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        let spikes: Vec<&str> = match o.cmd.as_deref() {
            Some(c @ ("job" | "netcon" | "failures" | "abspath")) => vec![c],
            None if o.dry_run => vec!["job", "netcon", "failures", "abspath"],
            Some(c) => return Err(format!("неизвестная подкоманда {c}\n\n{USAGE}")),
            None => return Err(format!("нужна подкоманда\n\n{USAGE}")),
        };

        let checks = preflight(&o);
        for c in &checks {
            println!("{} {}", if c.ok { "ok  " } else { "НЕТ " }, scrub(&c.text));
        }
        if o.dry_run {
            let demo = resolve_demo(&o).unwrap_or_else(|_| "<демка>".to_owned());
            println!(
                "\nпапка результата: {}",
                scrub(&data_dir().join("spikes").join("<метка>").to_string_lossy())
            );
            for s in spikes {
                for (label, args) in dry_lines(&o, s, &demo) {
                    println!(
                        "\n[{s}] {label}\n  HLAE.exe {}",
                        scrub(&args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "))
                    );
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        if let Some(bad) = checks.iter().find(|c| !c.ok) {
            eprintln!("отказ: {}", scrub(&bad.text));
            return Ok(ExitCode::from(bad.code));
        }

        let demo = resolve_demo(&o)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let out = data_dir().join("spikes").join(stamp.to_string());
        fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("результат: {}", scrub(&out.to_string_lossy()));
        for s in spikes {
            let line = match s {
                "job" => spike_job(&o, &out, &demo),
                "netcon" => spike_netcon(&o, &out, &demo),
                "failures" => spike_failures(&o, &out, &demo),
                _ => spike_abspath(&o, &out, &demo),
            };
            println!("{}", scrub(&line));
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(out.join("summary.txt"))
            {
                let _ = writeln!(f, "{}", scrub(&line));
            }
        }
        Ok(ExitCode::SUCCESS)
    }

    // ---------- пути и предпроверка ----------

    fn data_dir() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("cs2cinema")
    }

    fn game_csgo(o: &Opts) -> PathBuf {
        o.cs2.join("game").join("csgo")
    }

    fn cs2_exe(o: &Opts) -> PathBuf {
        o.cs2.join("game").join("bin").join("win64").join("cs2.exe")
    }

    /// Самая новая демка в `game\csgo`, кроме наших служебных.
    fn resolve_demo(o: &Opts) -> Result<String, String> {
        if let Some(d) = &o.demo {
            return Ok(d.clone());
        }
        let dir = game_csgo(o);
        let mut best: Option<(SystemTime, String)> = None;
        for e in fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().ends_with(".dem") || name.starts_with("cs2cinema_spike_") {
                continue;
            }
            if let Ok(t) = e.metadata().and_then(|m| m.modified())
                && best.as_ref().is_none_or(|(b, _)| t > *b)
            {
                best = Some((t, name));
            }
        }
        best.map(|(_, n)| n)
            .ok_or_else(|| format!("в {} нет .dem — укажите --demo", dir.display()))
    }

    struct Check {
        ok: bool,
        text: String,
        code: u8,
    }

    fn preflight(o: &Opts) -> Vec<Check> {
        let mut v = Vec::new();
        match snapshot_processes() {
            Ok(all) => {
                match all.iter().find(|e| e.exe_name.eq_ignore_ascii_case("cs2.exe")) {
                    Some(e) => v.push(Check {
                        ok: false,
                        text: format!("запущен cs2.exe (PID {}) — закройте CS2 вручную и запустите стенд снова; стенд чужие процессы не трогает", e.pid),
                        code: 2,
                    }),
                    None => v.push(Check { ok: true, text: "cs2.exe не запущен".into(), code: 0 }),
                }
                if all
                    .iter()
                    .any(|e| e.exe_name.eq_ignore_ascii_case("steam.exe"))
                {
                    v.push(Check {
                        ok: true,
                        text: "Steam запущен".into(),
                        code: 0,
                    });
                } else {
                    v.push(Check {
                        ok: false,
                        text: "Steam не запущен — запустите Steam".into(),
                        code: 2,
                    });
                }
            }
            Err(e) => v.push(Check {
                ok: false,
                text: format!("не удалось получить список процессов: {e}"),
                code: 1,
            }),
        }
        let files = [
            ("HLAE.exe", o.hlae.join("HLAE.exe")),
            (
                "x64\\AfxHookSource2.dll",
                o.hlae.join("x64").join("AfxHookSource2.dll"),
            ),
            ("game\\bin\\win64\\cs2.exe", cs2_exe(o)),
        ];
        for (name, p) in files {
            v.push(Check {
                ok: p.is_file(),
                text: format!("{name}: {}", p.display()),
                code: 1,
            });
        }
        match resolve_demo(o) {
            Ok(d) => {
                let p = game_csgo(o).join(&d);
                v.push(Check {
                    ok: p.is_file(),
                    text: format!("демка: {}", p.display()),
                    code: 1,
                });
            }
            Err(e) => v.push(Check {
                ok: false,
                text: format!("демка: {e}"),
                code: 1,
            }),
        }
        v
    }

    // ---------- строка запуска ----------

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .unwrap_or(2121)
    }

    /// Аргументы `HLAE.exe` — одни и те же для настоящего запуска и для `--dry-run`.
    fn hlae_args(o: &Opts, port: Option<u16>, fix: bool, demo_arg: &str) -> Vec<String> {
        let hook = o.hlae.join("x64").join("AfxHookSource2.dll");
        let port = port.map_or("<порт>".to_owned(), |p| p.to_string());
        let fixflag = if fix { " -afxFixNetCon" } else { "" };
        // -insecure: запись демки не должна проходить через VAC; окно = кадр ролика
        let cmdline = format!(
            "-insecure -steam -novid -allow_third_party_software -condebug -netconport {port}{fixflag} -windowed -w 1280 -h 720 +demo_ui_mode 0 +playdemo {demo_arg}"
        );
        [
            "-customLoader",
            "-noGui",
            "-autoStart",
            "-noConfig",
            "-hookDllPath",
            &hook.to_string_lossy(),
            "-programPath",
            &cs2_exe(o).to_string_lossy(),
            "-cmdLine",
            &cmdline,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn quote(a: &str) -> String {
        if a.contains(' ') {
            format!("\"{a}\"")
        } else {
            a.to_owned()
        }
    }

    fn abs_demo_path() -> PathBuf {
        std::env::temp_dir()
            .join("cs2cinema spike демо")
            .join("демо.dem")
    }

    fn abs_demo_arg() -> String {
        format!(
            "\"{}\"",
            abs_demo_path().to_string_lossy().replace('\\', "/")
        )
    }

    /// Строки запуска для `--dry-run`.
    fn dry_lines(o: &Opts, spike: &str, demo: &str) -> Vec<(String, Vec<String>)> {
        let fix = o.fix.unwrap_or(false);
        match spike {
            "job" => vec![(
                "S1: боевая строка запуска".into(),
                hlae_args(o, None, fix, demo),
            )],
            "netcon" => vec![
                (
                    "S2: без -afxFixNetCon".into(),
                    hlae_args(o, None, false, demo),
                ),
                ("S2: с -afxFixNetCon".into(), hlae_args(o, None, true, demo)),
            ],
            "failures" => vec![
                (
                    "S3a: демки нет".into(),
                    hlae_args(o, None, fix, "cs2cinema_spike_missing"),
                ),
                (
                    "S3b: пустая демка".into(),
                    hlae_args(o, None, fix, "cs2cinema_spike_empty"),
                ),
                (
                    "S3c/e: настоящая демка (cfg нет / поздний netcon)".into(),
                    hlae_args(o, None, fix, demo),
                ),
            ],
            _ => vec![(
                "S4: абсолютный путь с пробелами и кириллицей".into(),
                hlae_args(o, None, fix, &abs_demo_arg()),
            )],
        }
    }

    // ---------- транскрипт ----------

    /// Заменяет путь профиля пользователя на `C:/Users/USER` (транскрипты потом попадают в репозиторий).
    fn scrub(s: &str) -> String {
        let Some(profile) = std::env::var_os("USERPROFILE")
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty())
        else {
            return s.to_owned();
        };
        let mut out = s.as_bytes().to_vec();
        for pat in [profile.clone(), profile.replace('\\', "/")] {
            let pat = pat.as_bytes();
            let mut res = Vec::with_capacity(out.len());
            let mut i = 0;
            while i < out.len() {
                if out.len() - i >= pat.len() && out[i..i + pat.len()].eq_ignore_ascii_case(pat) {
                    res.extend_from_slice(b"C:/Users/USER");
                    i += pat.len();
                } else {
                    res.push(out[i]);
                    i += 1;
                }
            }
            out = res;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[derive(Default)]
    struct Marks {
        netcon_connected: Option<f64>,
        netcon_closed: Option<f64>,
        pipe_eof: Vec<(&'static str, f64)>,
    }

    /// Общий транскрипт и данные, которые пишут потоки.
    struct Shared {
        t0: Instant,
        file: Mutex<File>,
        lines: Mutex<Vec<(f64, &'static str, String)>>,
        writer: Mutex<Option<TcpStream>>,
        marks: Mutex<Marks>,
    }

    impl Shared {
        fn now(&self) -> f64 {
            self.t0.elapsed().as_secs_f64()
        }

        fn push(&self, src: &'static str, text: &str) {
            let t = self.now();
            let line = format!("+{t:.1} [{src}] {}", scrub(text));
            let _ = writeln!(lk(&self.file), "{line}");
            if src != "console.log" {
                println!("{line}");
            }
            lk(&self.lines).push((t, src, text.to_owned()));
        }

        fn len(&self) -> usize {
            lk(&self.lines).len()
        }

        /// Время первой строки с индекса `from`, подходящей под условие.
        fn find_from(&self, from: usize, pred: impl Fn(&str, &str) -> bool) -> Option<f64> {
            lk(&self.lines)
                .iter()
                .skip(from)
                .find(|(_, s, t)| pred(s, t))
                .map(|(t, _, _)| *t)
        }

        fn has_line(&self, pat: &str) -> bool {
            self.find_from(0, |_, t| t.contains(pat)).is_some()
        }

        /// Первые строки, похожие на сообщение об ошибке, — для краткой сводки.
        fn evidence(&self, from: usize, limit: usize) -> Vec<String> {
            const WORDS: [&str; 7] = [
                "rror",
                "ail",
                "ouldn't",
                "nable to",
                "not found",
                "annot",
                "issing",
            ];
            lk(&self.lines)
                .iter()
                .skip(from)
                .filter(|(_, s, t)| {
                    (*s == "console.log" || *s == "netcon") && WORDS.iter().any(|w| t.contains(w))
                })
                .take(limit)
                .map(|(t, s, x)| {
                    format!("+{t:.0}[{s}] {}", x.chars().take(110).collect::<String>())
                })
                .collect()
        }
    }

    // ---------- потоки чтения ----------

    fn spawn_pipe(sh: Arc<Shared>, src: &'static str, r: impl Read + Send + 'static) {
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                sh.push(src, String::from_utf8_lossy(&buf).trim());
                buf.clear();
            }
            sh.push("job", &format!("EOF {src}"));
            let t = sh.now();
            lk(&sh.marks).pipe_eof.push((src, t));
        });
    }

    /// Хвост `console.log` от смещения (игра дописывает файл; при усечении читаем с начала).
    fn spawn_tail(sh: Arc<Shared>, stop: Arc<AtomicBool>, path: PathBuf, mut pos: u64) {
        std::thread::spawn(move || {
            let mut carry: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                if let Ok(mut f) = File::open(&path) {
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    if len < pos {
                        pos = 0;
                        carry.clear();
                    }
                    if len > pos && f.seek(SeekFrom::Start(pos)).is_ok() {
                        let mut buf = Vec::new();
                        if f.take(len - pos).read_to_end(&mut buf).is_ok() {
                            pos += buf.len() as u64;
                            carry.extend(buf);
                        }
                        while let Some(i) = carry.iter().position(|b| *b == b'\n') {
                            let line: Vec<u8> = carry.drain(..=i).collect();
                            let text = String::from_utf8_lossy(&line);
                            sh.push("console.log", text.trim().trim_start_matches('\u{feff}'));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
    }

    #[derive(Clone, Copy)]
    enum NetconMode {
        /// Подключаться раз в секунду с самого начала.
        Immediate,
        /// Подключиться только через столько после того, как демка пошла (спайк S3e).
        LateAfterIngame(Duration),
    }

    fn sleep_unless_stopped(stop: &AtomicBool, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn spawn_netcon(sh: Arc<Shared>, stop: Arc<AtomicBool>, port: u16, mode: NetconMode) {
        std::thread::spawn(move || {
            if let NetconMode::LateAfterIngame(delay) = mode {
                while !stop.load(Ordering::Relaxed)
                    && !(sh.has_line("CSGO_GAME_UI_STATE_INGAME")
                        || sh.has_line("Host activate: Playing Demo"))
                {
                    std::thread::sleep(Duration::from_millis(200));
                }
                sh.push(
                    "app",
                    &format!(
                        "поздний netcon: демка пошла, ждём {} с до подключения",
                        delay.as_secs()
                    ),
                );
                sleep_unless_stopped(&stop, delay);
            }
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            while !stop.load(Ordering::Relaxed) {
                match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
                    Ok(s) => {
                        let t = sh.now();
                        lk(&sh.marks).netcon_connected.get_or_insert(t);
                        if let Ok(w) = s.try_clone() {
                            *lk(&sh.writer) = Some(w);
                        }
                        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                        sh.push("netcon", "подключено");
                        let mut r = BufReader::new(s);
                        let mut buf = Vec::new();
                        while !stop.load(Ordering::Relaxed) {
                            match r.read_until(b'\n', &mut buf) {
                                Ok(0) => break,
                                Ok(_) => {
                                    sh.push("netcon", String::from_utf8_lossy(&buf).trim());
                                    buf.clear();
                                }
                                Err(e)
                                    if matches!(
                                        e.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) => {}
                                Err(_) => break,
                            }
                        }
                        *lk(&sh.writer) = None;
                        let t = sh.now();
                        lk(&sh.marks).netcon_closed.get_or_insert(t);
                        sh.push("netcon", "соединение закрыто");
                        return;
                    }
                    Err(_) => std::thread::sleep(Duration::from_secs(1)),
                }
            }
        });
    }

    // ---------- сессия игры ----------

    struct Session {
        job: Job,
        child: Child,
        sh: Arc<Shared>,
        stop: Arc<AtomicBool>,
        port: u16,
        seen_pids: HashSet<u32>,
        outside: HashMap<u32, u32>,
        game_pid: Option<u32>,
        game: Option<OwnedProc>,
        game_seen_at: Option<f64>,
        launcher_exit_at: Option<f64>,
        game_exit_at: Option<f64>,
        ffmpeg_in_job: bool,
        cs2_in_job: Option<bool>,
        quit_at: Option<f64>,
        empty_at: Option<f64>,
    }

    struct Finish {
        quit_to_empty: Option<f64>,
        left_in_job: Vec<u32>,
        cs2_left: Vec<u32>,
    }

    impl Finish {
        fn clean(&self) -> bool {
            self.left_in_job.is_empty() && self.cs2_left.is_empty()
        }
    }

    impl Session {
        fn start(
            o: &Opts,
            out: &Path,
            name: &str,
            demo_arg: &str,
            fix: bool,
            mode: NetconMode,
        ) -> Result<Session, String> {
            let file = File::create(out.join(format!("{name}.log")))
                .map_err(|e| format!("транскрипт {name}: {e}"))?;
            let sh = Arc::new(Shared {
                t0: Instant::now(),
                file: Mutex::new(file),
                lines: Mutex::new(Vec::new()),
                writer: Mutex::new(None),
                marks: Mutex::new(Marks::default()),
            });
            let stop = Arc::new(AtomicBool::new(false));
            let job = Job::new().map_err(|e| e.to_string())?;
            let console_log = game_csgo(o).join("console.log");
            let offset = fs::metadata(&console_log).map(|m| m.len()).unwrap_or(0);
            let port = free_port();
            let args = hlae_args(o, Some(port), fix, demo_arg);
            sh.push(
                "app",
                &format!(
                    "запуск: HLAE.exe {}",
                    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
                ),
            );
            let mut child = Command::new(o.hlae.join("HLAE.exe"))
                .current_dir(&o.hlae)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("не удалось запустить HLAE: {e}"))?;
            // job сразу: потомки HLAE наследуют его
            if let Err(e) = job.assign_child(&child) {
                let _ = child.kill();
                return Err(e.to_string());
            }
            if let Some(out) = child.stdout.take() {
                spawn_pipe(sh.clone(), "hlae-stdout", out);
            }
            if let Some(err) = child.stderr.take() {
                spawn_pipe(sh.clone(), "hlae-stderr", err);
            }
            spawn_tail(sh.clone(), stop.clone(), console_log, offset);
            spawn_netcon(sh.clone(), stop.clone(), port, mode);
            Ok(Session {
                job,
                child,
                sh,
                stop,
                port,
                seen_pids: HashSet::new(),
                outside: HashMap::new(),
                game_pid: None,
                game: None,
                game_seen_at: None,
                launcher_exit_at: None,
                game_exit_at: None,
                ffmpeg_in_job: false,
                cs2_in_job: None,
                quit_at: None,
                empty_at: None,
            })
        }

        /// Один обход: состав job, снимок процессов, выход лаунчера и игры.
        fn pump(&mut self) {
            let t = self.sh.now();
            if self.launcher_exit_at.is_none()
                && let Ok(Some(st)) = self.child.try_wait()
            {
                self.launcher_exit_at = Some(t);
                self.sh.push("job", &format!("HLAE.exe завершился: {st}"));
            }
            let in_job: Vec<u32> = self.job.pids().unwrap_or_default();
            let names = snapshot_processes().unwrap_or_default();
            for pid in &in_job {
                let name = names
                    .iter()
                    .find(|e| e.pid == *pid)
                    .map(|e| e.exe_name.clone());
                let Some(name) = name else { continue };
                if !self.seen_pids.insert(*pid) {
                    continue;
                }
                let mut note = format!("в job: PID {pid} {name}");
                if name.eq_ignore_ascii_case("cs2.exe") && self.game_pid.is_none() {
                    self.game_pid = Some(*pid);
                    self.game_seen_at = Some(t);
                    self.game = OwnedProc::open(*pid).ok();
                    if let Some(p) = &self.game {
                        self.cs2_in_job = p.in_job(&self.job);
                        note += &format!(", IsProcessInJob={:?}", self.cs2_in_job);
                        if let Some(img) = p.image_path() {
                            note += &format!(", образ {}", img.display());
                        }
                    }
                }
                if name.eq_ignore_ascii_case("ffmpeg.exe") {
                    self.ffmpeg_in_job = true;
                }
                self.sh.push("job", &note);
            }
            // cs2.exe, которого нет в job: подтверждаем двумя обходами подряд, чтобы не ловить гонку
            for e in names
                .iter()
                .filter(|e| e.exe_name.eq_ignore_ascii_case("cs2.exe") && !in_job.contains(&e.pid))
            {
                let n = self.outside.entry(e.pid).or_insert(0);
                *n += 1;
                if *n == 2 {
                    self.sh.push(
                        "job",
                        &format!("cs2.exe ВНЕ job: PID {} (родитель {})", e.pid, e.parent_pid),
                    );
                }
            }
            if self.game_exit_at.is_none()
                && let Some(p) = &self.game
                && let Liveness::Exited(code) = p.liveness()
            {
                self.game_exit_at = Some(t);
                self.sh
                    .push("job", &format!("cs2.exe завершился, код {code:?}"));
            }
        }

        fn wait_until(
            &mut self,
            timeout: Duration,
            mut cond: impl FnMut(&mut Session) -> bool,
        ) -> bool {
            let end = Instant::now() + timeout;
            loop {
                self.pump();
                if cond(self) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }

        fn wait_game(&mut self, timeout: Duration) -> bool {
            self.wait_until(timeout, |s| s.game_pid.is_some())
        }

        fn connected(&self) -> bool {
            lk(&self.sh.marks).netcon_connected.is_some()
        }

        fn wait_connected(&mut self, timeout: Duration) -> bool {
            self.wait_until(timeout, |s| s.connected())
        }

        fn wait_line(
            &mut self,
            from: usize,
            timeout: Duration,
            pred: impl Fn(&str, &str) -> bool,
        ) -> Option<f64> {
            let sh = self.sh.clone();
            self.wait_until(timeout, |_| sh.find_from(from, &pred).is_some());
            sh.find_from(from, &pred)
        }

        fn send(&self, line: &str) -> bool {
            let ok = {
                let mut w = lk(&self.sh.writer);
                match w.as_mut() {
                    Some(s) => s.write_all(format!("{line}\n").as_bytes()).is_ok(),
                    None => false,
                }
            };
            self.sh.push(
                "app",
                &format!(
                    "{}> {line}",
                    if ok {
                        ""
                    } else {
                        "(консоль не подключена) "
                    }
                ),
            );
            ok
        }

        /// READY по nonce: запрос с `addAtTick`, при молчании — прямой `echo`.
        fn wait_ready(&mut self, nonce: &str, timeout: Duration) -> bool {
            if !self.wait_connected(timeout) {
                return false;
            }
            let marker = format!("CS2C {nonce} READY");
            let is_marker = |m: String| {
                move |_: &str, t: &str| {
                    t.trim().ends_with(&m) && !t.contains("addAtTick") && !t.contains("echo ")
                }
            };
            let from = self.sh.len();
            self.send(&format!(
                "mirv_cmd clear; mirv_cmd addAtTick 64 \"echo {marker}\""
            ));
            if self
                .wait_line(from, Duration::from_secs(100), is_marker(marker.clone()))
                .is_some()
            {
                return true;
            }
            self.sh.push(
                "app",
                "READY по addAtTick не пришёл за 100 с — шлём прямой echo",
            );
            self.send(&format!("echo {marker}"));
            self.wait_line(
                from,
                timeout
                    .saturating_sub(Duration::from_secs(100))
                    .max(Duration::from_secs(10)),
                is_marker(marker),
            )
            .is_some()
        }

        /// Просит игру выйти, ждёт пустой job, при необходимости завершает job, проверяет остатки.
        fn finish(&mut self) -> Finish {
            self.pump();
            if lk(&self.sh.writer).is_some() {
                self.send("quit");
            }
            self.quit_at = Some(self.sh.now());
            let emptied = self.wait_until(Duration::from_secs(30), |s| {
                s.job.active_count().map(|n| n == 0).unwrap_or(false)
            });
            if emptied {
                self.empty_at = Some(self.sh.now());
            } else {
                self.sh.push(
                    "job",
                    "job не опустел за 30 с после quit — TerminateJobObject",
                );
                let _ = self.job.terminate(1);
                self.wait_until(Duration::from_secs(5), |s| {
                    s.job.active_count().map(|n| n == 0).unwrap_or(false)
                });
            }
            // лаунчер: он в job и уже завершён; если нет — свой handle
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
            }
            std::thread::sleep(Duration::from_millis(500));
            self.stop.store(true, Ordering::Relaxed);
            let left_in_job = self.job.pids().unwrap_or_default();
            let cs2_left: Vec<u32> = snapshot_processes()
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.exe_name.eq_ignore_ascii_case("cs2.exe"))
                .map(|e| e.pid)
                .collect();
            let quit_to_empty = self.empty_at.zip(self.quit_at).map(|(e, q)| e - q);
            let f = Finish {
                quit_to_empty,
                left_in_job,
                cs2_left,
            };
            self.sh.push(
                "job",
                &format!(
                    "итог: quit->пустой job {:?} с, осталось в job {:?}, cs2.exe в системе {:?}",
                    f.quit_to_empty, f.left_in_job, f.cs2_left
                ),
            );
            f
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            // закрытие Job (KILL_ON_JOB_CLOSE) уберёт всё, что осталось, даже если спайк упал
        }
    }

    fn secs(v: Option<f64>) -> String {
        v.map_or("нет".to_owned(), |t| format!("{t:.1} с"))
    }

    fn verdict(ok: bool, text: String) -> String {
        if ok {
            format!("PASS — {text}")
        } else {
            format!("FAIL — {text}")
        }
    }

    // ---------- S1 ----------

    fn spike_job(o: &Opts, out: &Path, demo: &str) -> String {
        let label = "S1";
        let mut s = match Session::start(
            o,
            out,
            "s1_job",
            demo,
            o.fix.unwrap_or(false),
            NetconMode::Immediate,
        ) {
            Ok(s) => s,
            Err(e) => return format!("{label}: {}", verdict(false, e)),
        };
        s.wait_game(Duration::from_secs(60));
        // даём ещё пять секунд посмотреть на остальных потомков
        s.wait_until(Duration::from_secs(5), |_| false);
        let in_job = s.game_pid.is_some();
        let outside: Vec<u32> = s
            .outside
            .iter()
            .filter(|(_, n)| **n >= 2)
            .map(|(p, _)| *p)
            .collect();
        let ready = s.wait_ready("spike", Duration::from_secs(150));
        let mut take_note = "дубль не снимали (нет READY)".to_owned();
        if ready {
            let clips = out.join("s1_clips").to_string_lossy().replace('\\', "/");
            s.send(&format!("mirv_streams record name \"{clips}\""));
            s.send("mirv_streams record screen enabled 1");
            s.send("mirv_streams record screen settings afxFfmpegYuv420p");
            s.send("mirv_streams record start");
            s.wait_until(Duration::from_secs(5), |_| false);
            s.send("mirv_streams record end");
            s.wait_until(Duration::from_secs(3), |_| false);
            take_note = format!(
                "ffmpeg.exe в job: {}",
                if s.ffmpeg_in_job { "да" } else { "нет" }
            );
        }
        let fin = s.finish();
        let marks = lk(&s.sh.marks);
        let eof: Vec<String> = marks
            .pipe_eof
            .iter()
            .map(|(src, t)| format!("{src}@{t:.1}"))
            .collect();
        let text = format!(
            "cs2.exe в job: {} (PID {:?}, на {}); IsProcessInJob={:?}; вне job: {:?}; READY: {}; {take_note}; выход лаунчера {}; выход игры {}; EOF pipe: [{}]; quit->пустой job {}; остатки: {}",
            if in_job { "да" } else { "нет" },
            s.game_pid,
            secs(s.game_seen_at),
            s.cs2_in_job,
            outside,
            if ready { "пришёл" } else { "нет" },
            secs(s.launcher_exit_at),
            secs(s.game_exit_at),
            eof.join(", "),
            secs(fin.quit_to_empty),
            if fin.clean() {
                "нет".to_owned()
            } else {
                format!("job {:?}, cs2.exe {:?}", fin.left_in_job, fin.cs2_left)
            },
        );
        format!("{label}: {}", verdict(in_job && fin.clean(), text))
    }

    // ---------- S2 ----------

    /// Строка `netstat -ano` для порта в состоянии LISTENING.
    fn netstat_line(port: u16) -> Option<String> {
        let out = Command::new("netstat").arg("-ano").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let needle = format!(":{port} ");
        text.lines()
            .find(|l| l.contains(&needle) && l.contains("LISTENING"))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    fn median(v: &mut [f64]) -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    }

    fn spike_netcon(o: &Opts, out: &Path, demo: &str) -> String {
        let runs = o.runs.max(1) as usize;
        let variants: Vec<bool> = match o.fix {
            Some(f) => vec![f; runs],
            None => (0..runs).map(|i| i % 2 == 1).collect(),
        };
        let began = Instant::now();
        // (с флагом, подключилось, секунды от появления cs2.exe до connect)
        let mut results: Vec<(bool, bool, Option<f64>)> = Vec::new();
        let mut binds: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for (n, fix) in variants.iter().enumerate() {
            if began.elapsed() > SPIKE_BUDGET {
                notes.push(format!("запуски с {} пропущены: вышло время", n + 1));
                break;
            }
            let name = format!("s2_{}_{}", if *fix { "fix" } else { "nofix" }, n + 1);
            let mut s = match Session::start(o, out, &name, demo, *fix, NetconMode::Immediate) {
                Ok(s) => s,
                Err(e) => {
                    notes.push(format!("{name}: {e}"));
                    continue;
                }
            };
            s.wait_game(Duration::from_secs(90));
            let connected = s.wait_connected(Duration::from_secs(120));
            let t_conn = lk(&s.sh.marks).netcon_connected;
            let delta = t_conn.zip(s.game_seen_at).map(|(c, g)| (c - g).max(0.0));
            if let Some(l) = netstat_line(s.port) {
                s.sh.push("job", &format!("netstat: {l}"));
                binds.push(l.split_whitespace().nth(1).unwrap_or("?").to_owned());
            }
            s.finish();
            results.push((*fix, connected, delta));
        }
        let describe = |fix: bool| {
            let rs: Vec<_> = results.iter().filter(|r| r.0 == fix).collect();
            if rs.is_empty() {
                return None;
            }
            let ok = rs.iter().filter(|r| r.1).count();
            let mut times: Vec<f64> = rs.iter().filter_map(|r| r.2).collect();
            Some(format!(
                "{}: {ok}/{} подключений, медиана {}",
                if fix {
                    "с флагом"
                } else {
                    "без флага"
                },
                rs.len(),
                secs(median(&mut times))
            ))
        };
        let parts: Vec<String> = [describe(false), describe(true)]
            .into_iter()
            .flatten()
            .collect();
        // судим по запускам с флагом; если их не было (--fix off) — по всем
        let judged: Vec<_> = if results.iter().any(|r| r.0) {
            results.iter().filter(|r| r.0).collect()
        } else {
            results.iter().collect()
        };
        let pass = !judged.is_empty() && judged.iter().all(|r| r.1);
        binds.sort();
        binds.dedup();
        format!(
            "S2: {}",
            verdict(
                pass,
                format!(
                    "{}; адрес привязки: {:?}; {}",
                    parts.join("; "),
                    binds,
                    notes.join("; ")
                )
            )
        )
    }

    // ---------- S3 ----------

    /// Удаляет файл при выходе из области видимости (пустая демка для S3b).
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_dir(&self.0);
        }
    }

    /// Транскрипты S3 в папке результата (имена совпадают с теми, что дают случаи a, b, c, e).
    const S3_TRANSCRIPTS: [&str; 4] = [
        "s3a_missing_demo.log",
        "s3b_empty_demo.log",
        "s3c_missing_cfg.log",
        "s3e_late_connect.log",
    ];

    fn spike_failures(o: &Opts, out: &Path, demo: &str) -> String {
        let began = Instant::now();
        let fix = o.fix.unwrap_or(false);
        let mut parts: Vec<String> = Vec::new();
        let mut all_ok = true;
        let mut part = |name: &str, r: Option<Result<(bool, String), String>>| {
            let r = r.unwrap_or_else(|| Err("пропущено: вышло время".to_owned()));
            match r {
                Ok((ok, t)) => {
                    all_ok &= ok;
                    parts.push(format!("{name}: {t}"));
                }
                Err(e) => {
                    all_ok = false;
                    parts.push(format!("{name}: {e}"));
                }
            }
        };
        let in_budget = || began.elapsed() < SPIKE_BUDGET;

        // (a) демки нет
        part(
            "a",
            in_budget().then(|| {
                observe_bad_demo(o, out, "s3a_missing_demo", "cs2cinema_spike_missing", fix)
            }),
        );
        // (b) пустая демка
        let empty = game_csgo(o).join("cs2cinema_spike_empty.dem");
        let _guard = Cleanup(empty.clone());
        part(
            "b",
            in_budget().then(|| {
                fs::write(&empty, b"").map_err(|e| format!("не создать пустую демку: {e}"))?;
                observe_bad_demo(o, out, "s3b_empty_demo", "cs2cinema_spike_empty", fix)
            }),
        );
        // (c) exec несуществующего cfg после READY
        part(
            "c",
            in_budget().then(|| case_missing_cfg(o, out, demo, fix)),
        );
        // (e) поздний connect
        part(
            "e",
            in_budget().then(|| case_late_connect(o, out, demo, fix)),
        );
        format!(
            "S3: {}",
            verdict(
                all_ok,
                format!(
                    "{}; транскрипты: {}",
                    parts.join(" | "),
                    S3_TRANSCRIPTS.join(", ")
                )
            )
        )
    }

    fn observe_bad_demo(
        o: &Opts,
        out: &Path,
        name: &str,
        demo_arg: &str,
        fix: bool,
    ) -> Result<(bool, String), String> {
        let mut s = Session::start(o, out, name, demo_arg, fix, NetconMode::Immediate)?;
        s.wait_game(Duration::from_secs(90));
        let connected = s.wait_connected(Duration::from_secs(120));
        // наблюдаем консоль ещё полминуты
        s.wait_until(Duration::from_secs(30), |_| false);
        let ev = s.sh.evidence(0, 3);
        let playing = s.sh.has_line("Host activate: Playing Demo");
        let fin = s.finish();
        Ok((
            fin.clean(),
            format!(
                "netcon {}, демка пошла: {}, улики: {:?}, транскрипт {name}.log",
                if connected { "да" } else { "нет" },
                if playing { "да" } else { "нет" },
                ev
            ),
        ))
    }

    fn case_missing_cfg(
        o: &Opts,
        out: &Path,
        demo: &str,
        fix: bool,
    ) -> Result<(bool, String), String> {
        let name = "s3c_missing_cfg";
        let mut s = Session::start(o, out, name, demo, fix, NetconMode::Immediate)?;
        s.wait_game(Duration::from_secs(90));
        let ready = s.wait_ready("spike", Duration::from_secs(150));
        let from = s.sh.len();
        s.send("exec cs2cinema_spike_missing_cfg");
        s.wait_until(Duration::from_secs(10), |_| false);
        let ev = s.sh.evidence(from, 3);
        let fin = s.finish();
        Ok((
            fin.clean(),
            format!(
                "READY {}, ответ на exec: {:?}, транскрипт {name}.log",
                if ready { "да" } else { "нет" },
                ev
            ),
        ))
    }

    fn case_late_connect(
        o: &Opts,
        out: &Path,
        demo: &str,
        fix: bool,
    ) -> Result<(bool, String), String> {
        let name = "s3e_late_connect";
        let mut s = Session::start(
            o,
            out,
            name,
            demo,
            fix,
            NetconMode::LateAfterIngame(Duration::from_secs(20)),
        )?;
        s.wait_game(Duration::from_secs(90));
        let connected = s.wait_connected(Duration::from_secs(180));
        let mut ready_timed = false;
        let mut ready_direct = false;
        if connected {
            let from = s.sh.len();
            s.send("mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C spike READY\"");
            ready_timed = s
                .wait_line(from, Duration::from_secs(30), |_, t| {
                    t.trim().ends_with("CS2C spike READY")
                        && !t.contains("addAtTick")
                        && !t.contains("echo ")
                })
                .is_some();
            if !ready_timed {
                let from = s.sh.len();
                s.send("echo CS2C spike READY2");
                ready_direct = s
                    .wait_line(from, Duration::from_secs(10), |_, t| {
                        t.trim().ends_with("CS2C spike READY2") && !t.contains("echo ")
                    })
                    .is_some();
            }
        }
        let fin = s.finish();
        let yn = |b: bool| if b { "да" } else { "нет" };
        Ok((
            fin.clean(),
            format!(
                "netcon {}, READY по addAtTick {}, READY2 по прямому echo {}, транскрипт {name}.log",
                yn(connected),
                yn(ready_timed),
                yn(ready_direct)
            ),
        ))
    }

    // ---------- S4 ----------

    fn spike_abspath(o: &Opts, out: &Path, demo: &str) -> String {
        let src = game_csgo(o).join(demo);
        let dst = abs_demo_path();
        let _guard = Cleanup(dst.clone());
        let dir = dst.parent().map(Path::to_path_buf).unwrap_or_default();
        let result = (|| -> Result<String, String> {
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            fs::copy(&src, &dst).map_err(|e| format!("копия демки: {e}"))?;
            let mut s = Session::start(
                o,
                out,
                "s4_abspath",
                &abs_demo_arg(),
                o.fix.unwrap_or(false),
                NetconMode::Immediate,
            )?;
            let found = s
                .wait_line(0, Duration::from_secs(90), |_, t| {
                    t.contains("Host activate: Playing Demo")
                })
                .is_some();
            let fin = s.finish();
            Ok(verdict(
                found && fin.clean(),
                format!(
                    "«Host activate: Playing Demo» за 90 с: {}, остатки: {}",
                    if found { "да" } else { "нет" },
                    if fin.clean() { "нет" } else { "есть" }
                ),
            ))
        })();
        let _ = fs::remove_dir(&dir);
        match result {
            Ok(v) => format!("S4: {v}"),
            Err(e) => format!("S4: {}", verdict(false, e)),
        }
    }
}
