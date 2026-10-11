//! `cs2-cli runner`: раннер очереди сервера на машине с CS2. Не служба Windows: CS2 нужна интерактивная сессия,
//! поэтому раннер живёт в консоли автора. Подкоманда только собирает настройки и `PipelineExecutor`; весь ход
//! задачи (забрать, проверить демку, записать, проверить ролик, вернуть) живёт в крейте `runner`.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use pipeline::{inspect::InspectLimits, job::JobConfig};
use runner::{Config, InspectSettings, PipelineExecutor, Runner, Token};

/// Справка подкоманды; печатается и по `--help`. Скрытый флаг учений (`--upload-chunk-limit`) сюда не входит.
pub const USAGE_RUNNER: &str = "\
cs2-cli runner — раннер очереди сервера: сам забирает задачи, проверяет чужую демку, записывает ролик и возвращает его

Использование:
  cs2-cli runner --server <адрес> [--token-file <файл>] [--name <имя>] [--dir <папка>] [--hlae <папка>] [--cs2 <папка>]
                 [--margin X] [--no-gpu] [--max-demo-mib N] [--max-unpacked-mib N] [--inspect-mem-mib N] [--inspect-mins N]

Параметры:
  --server <адрес>        адрес cs2-server, например https://хост:8080 (http:// только для localhost и частной сети; иначе $CS2_RUNNER_SERVER)
  --token-file <файл>     файл с токеном раннера, одна строка          (иначе $CS2_RUNNER_TOKEN)
  --name <имя>            имя раннера в журнале сервера                (по умолчанию $COMPUTERNAME или runner)
  --dir <папка>           рабочая папка: remote/ (демки), out/ (результаты), runner.log, runner_state.json
                          (по умолчанию %LOCALAPPDATA%\\cs2cinema\\runner)
  --hlae <папка>         папка с HLAE.exe                              (иначе $HLAE_DIR)
  --cs2 <папка>          папка установки CS2                           (иначе $CS2_DIR)
  --margin <X>            запас по времени: множитель дедлайнов        (по умолчанию 1)
  --no-gpu                не кодировать на видеокарте (NVENC)
  --max-demo-mib <N>      демка больше не скачивается                  (по умолчанию 2000)
  --max-unpacked-mib <N>  потолок демки после распаковки при разборе   (по умолчанию 3072)
  --inspect-mem-mib <N>   потолок памяти дочернего разбора демки       (по умолчанию 6144)
  --inspect-mins <N>      потолок времени дочернего разбора, минут     (по умолчанию 15)

Токен в командной строке не принимается: он виден в списке процессов. Задайте его переменной CS2_RUNNER_TOKEN
или файлом --token-file.

Раннер сам ходит к серверу исходящими запросами, входящий порт не нужен. Задачи идут по одной.
Не запускайте очередь приложения одновременно с раннером: CS2 записывает только один экземпляр.
Ctrl+C — остановка без закрытия задачи (она продолжится при следующем запуске), второе нажатие — выход сразу.
Enter — продолжить после паузы (раннер встаёт на паузу после двух одинаковых неисправностей подряд).
Вторая копия раннера в той же папке не запускается.";

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Разобранные аргументы раннера. Токен в `Debug` не попадает.
pub struct RunnerArgs {
    pub server: String,
    token: String,
    pub name: String,
    pub dir: PathBuf,
    pub cfg: JobConfig,
    pub limits: InspectLimits,
    pub max_demo_bytes: u64,
    /// Скрытый флаг учений «обрыв загрузки».
    pub upload_chunk_limit: Option<u32>,
}

impl std::fmt::Debug for RunnerArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerArgs")
            .field("server", &self.server)
            .field("token", &"***")
            .field("name", &self.name)
            .field("dir", &self.dir)
            .field("cfg", &self.cfg)
            .field("limits", &self.limits)
            .field("max_demo_bytes", &self.max_demo_bytes)
            .field("upload_chunk_limit", &self.upload_chunk_limit)
            .finish()
    }
}

/// Рабочая папка по умолчанию: `%LOCALAPPDATA%\cs2cinema\runner`, иначе `$XDG_DATA_HOME/cs2cinema/runner`,
/// иначе `./runner`.
fn default_dir(env: Env) -> PathBuf {
    let nonempty = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    match nonempty("LOCALAPPDATA").or_else(|| nonempty("XDG_DATA_HOME")) {
        Some(base) => PathBuf::from(base).join("cs2cinema").join("runner"),
        None => PathBuf::from("runner"),
    }
}

/// Целое больше нуля из значения флага.
fn positive(name: &str, v: &str) -> Result<u64, String> {
    match v.trim().parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{name}: нужно целое число больше нуля")),
    }
}

/// Токен из файла: одна строка, пробелы и перевод строки по краям отбрасываются.
fn token_from_file(path: &str) -> Result<String, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("не удалось прочитать файл токена {path}: {e}"))?;
    let token = text.trim();
    if token.is_empty() {
        return Err(format!("файл токена {path} пуст"));
    }
    Ok(token.to_owned())
}

/// Разбор аргументов `cs2-cli runner` по образцу `record::parse_cfg`. Окружение приходит замыканием, чтобы тесты
/// не трогали настоящее.
pub fn parse_runner(args: &[String], env: Env) -> Result<RunnerArgs, String> {
    let nonempty = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    let mut server = nonempty("CS2_RUNNER_SERVER");
    let mut token = nonempty("CS2_RUNNER_TOKEN").map(|t| t.trim().to_owned());
    let mut name = nonempty("COMPUTERNAME");
    let mut dir = None;
    let mut hlae = nonempty("HLAE_DIR");
    let mut cs2 = nonempty("CS2_DIR");
    let mut cfg = JobConfig::default();
    let mut limits = InspectLimits::default();
    let mut max_demo_mib = None;
    let mut upload_chunk_limit = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} требует значение"))
        };
        match a.as_str() {
            "--server" => server = Some(value("--server")?),
            "--token-file" => token = Some(token_from_file(&value("--token-file")?)?),
            s if s == "--token" || s.starts_with("--token=") => {
                return Err("токен не передаётся в командной строке: задайте CS2_RUNNER_TOKEN или --token-file".into());
            }
            "--name" => name = Some(value("--name")?),
            "--dir" => dir = Some(PathBuf::from(value("--dir")?)),
            "--hlae" => hlae = Some(value("--hlae")?),
            "--cs2" => cs2 = Some(value("--cs2")?),
            "--margin" => {
                let m: f32 = value("--margin")?
                    .parse()
                    .map_err(|_| "--margin: нужно число")?;
                if !m.is_finite() || m <= 0.0 {
                    return Err("--margin: нужно положительное число".into());
                }
                cfg.time_margin = m;
            }
            "--no-gpu" => cfg.gpu_encode = false,
            "--max-demo-mib" => {
                let n = positive("--max-demo-mib", &value("--max-demo-mib")?)?;
                max_demo_mib = Some(n);
            }
            "--max-unpacked-mib" => {
                let n = positive("--max-unpacked-mib", &value("--max-unpacked-mib")?)?;
                limits.demo.max_unpacked = n << 20;
            }
            "--inspect-mem-mib" => {
                let n = positive("--inspect-mem-mib", &value("--inspect-mem-mib")?)?;
                limits.mem_bytes = n << 20;
            }
            "--inspect-mins" => {
                let n = positive("--inspect-mins", &value("--inspect-mins")?)?;
                limits.secs = n * 60;
            }
            "--upload-chunk-limit" => {
                let n = positive("--upload-chunk-limit", &value("--upload-chunk-limit")?)?;
                upload_chunk_limit = Some(
                    u32::try_from(n).map_err(|_| "--upload-chunk-limit: слишком большое число")?,
                );
            }
            s if s.starts_with("--") => return Err(format!("неизвестный параметр {s}")),
            s => return Err(format!("лишний аргумент {s}")),
        }
    }
    let Some(server) = server else {
        return Err("укажите --server <адрес> или переменную CS2_RUNNER_SERVER".into());
    };
    let Some(token) = token else {
        return Err(
            "не задан токен раннера: переменная CS2_RUNNER_TOKEN или --token-file <файл>".into(),
        );
    };
    // Токен идёт в заголовок Authorization: пробелы, управляющие и не-ASCII знаки дали бы вечные повторы запроса.
    if !token.chars().all(|c| c.is_ascii_graphic()) {
        return Err("токен должен состоять из печатных ASCII-знаков без пробелов".into());
    }
    // Потолок скачивания и потолок файла при разборе — одно число: что скачали, то и разберут.
    let max_demo_bytes = max_demo_mib.map_or(runner::MAX_DEMO_BYTES, |n| n << 20);
    limits.demo.max_file = max_demo_bytes;
    cfg.hlae_dir = hlae.map(PathBuf::from).unwrap_or_default();
    cfg.cs2_dir = cs2.map(PathBuf::from).unwrap_or_default();
    Ok(RunnerArgs {
        server,
        token,
        name: name.unwrap_or_else(|| "runner".to_owned()),
        dir: dir.unwrap_or_else(|| default_dir(env)),
        cfg,
        limits,
        max_demo_bytes,
        upload_chunk_limit,
    })
}

fn process_env(k: &str) -> Option<String> {
    env::var(k).ok()
}

/// Замок единственной копии: `runner.lock` держится открытым до выхода, ОС снимает его и при падении.
fn lock_dir(dir: &Path) -> Result<fs::File, (String, u8)> {
    fs::create_dir_all(dir)
        .map_err(|e| (format!("не удалось создать {}: {e}", dir.display()), 1))?;
    let path = dir.join("runner.lock");
    let file = fs::File::create(&path)
        .map_err(|e| (format!("не удалось открыть {}: {e}", path.display()), 1))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => {
            Err((format!("раннер уже запущен в {}", dir.display()), 2))
        }
        Err(fs::TryLockError::Error(e)) => {
            Err((format!("не удалось занять {}: {e}", path.display()), 1))
        }
    }
}

/// `cs2-cli runner`. Коды выхода: 0 — остановлен штатно, 1 — отказ сервера или среды, 2 — аргументы или вторая копия.
pub fn runner_cmd() -> ExitCode {
    let argv: Vec<String> = env::args().skip(2).collect();
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE_RUNNER}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_runner(&argv, &process_env) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("ошибка: {msg}\n\n{USAGE_RUNNER}");
            return ExitCode::from(2);
        }
    };
    let dir = std::path::absolute(&args.dir).unwrap_or_else(|_| args.dir.clone());
    // Файл замка живёт до конца функции.
    let _lock = match lock_dir(&dir) {
        Ok(f) => f,
        Err((msg, code)) => {
            eprintln!("ошибка: {msg}");
            return ExitCode::from(code);
        }
    };
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ошибка: не удалось узнать путь к cs2-cli: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut job_cfg = args.cfg.clone();
    job_cfg.out_root = dir.join("out");
    let host: Arc<dyn pipeline::host::Host> = Arc::from(pipeline::host::default_host());
    let exec = PipelineExecutor::new(
        host,
        job_cfg,
        InspectSettings {
            exe,
            limits: args.limits,
        },
    );
    let cfg = Config {
        server: args.server.clone(),
        token: Token::new(args.token.clone()),
        dir: dir.clone(),
        name: args.name.clone(),
        poll_secs: None,
        upload_chunk_limit: args.upload_chunk_limit,
        heartbeat: None,
        debounce: Duration::from_secs(2),
        force_after: runner::FORCE_AFTER,
        release_cooldown: runner::RELEASE_COOLDOWN,
        max_demo_bytes: args.max_demo_bytes,
    };
    let mut runner = match Runner::new(cfg, Arc::new(exec)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Ctrl+C: первое нажатие останавливает раннер, не закрывая задачу; второе уходит сразу (игра закрывается
    // вместе с Job Object хоста).
    let interrupted = Arc::new(AtomicBool::new(false));
    let presses = Arc::new(AtomicUsize::new(0));
    let control = runner.control();
    let handler = {
        let (control, interrupted) = (control.clone(), interrupted.clone());
        move || {
            if presses.fetch_add(1, Ordering::SeqCst) == 0 {
                interrupted.store(true, Ordering::SeqCst);
                control.stop();
                eprintln!("останавливаюсь: задача продолжится при следующем запуске");
            } else {
                eprintln!("повторное нажатие: выхожу сразу");
                std::process::exit(130);
            }
        }
    };
    if let Err(e) = ctrlc::set_handler(handler) {
        eprintln!("ошибка: не удалось поставить обработчик Ctrl+C: {e}");
        return ExitCode::FAILURE;
    }
    // Enter снимает паузу раннера.
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => control.resume(),
            }
        }
    });

    eprintln!(
        "раннер «{}»: сервер {}, папка {}\nCtrl+C — остановка, Enter — продолжить после паузы",
        args.name,
        args.server,
        dir.display()
    );
    match runner.run() {
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
        Ok(()) => {
            if interrupted.load(Ordering::SeqCst) {
                eprintln!("раннер остановлен");
            } else if let Some(n) = args.upload_chunk_limit {
                eprintln!("загрузка остановлена после {n} кусков (учение)");
            }
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<RunnerArgs, String> {
        parse_runner(&strings(args), &env_of(env))
    }

    const TOKEN: (&str, &str) = ("CS2_RUNNER_TOKEN", "s3cr3t-token-value");

    #[test]
    fn parses_server_hlae_cs2_and_keeps_the_token_out_of_debug() {
        let a = parse(
            &[
                "--server",
                "http://127.0.0.1:8080",
                "--hlae",
                "C:/hlae",
                "--cs2",
                "C:/cs2",
            ],
            &[TOKEN],
        )
        .expect("разбор");
        assert_eq!(a.server, "http://127.0.0.1:8080");
        assert_eq!(a.cfg.hlae_dir, PathBuf::from("C:/hlae"));
        assert_eq!(a.cfg.cs2_dir, PathBuf::from("C:/cs2"));
        assert_eq!(a.max_demo_bytes, 2000 << 20);
        assert_eq!(a.name, "runner");
        assert!(a.upload_chunk_limit.is_none());
        let shown = format!("{a:?}");
        assert!(!shown.contains(TOKEN.1), "токен в Debug: {shown}");
        assert!(shown.contains("***"), "{shown}");
    }

    #[test]
    fn token_flag_is_refused_with_an_explanation() {
        let e = parse(&["--token", "abc", "--server", "http://x"], &[]).unwrap_err();
        assert!(e.contains("CS2_RUNNER_TOKEN"), "{e}");
        let e = parse(&["--token=abc", "--server", "http://x"], &[]).unwrap_err();
        assert!(e.contains("CS2_RUNNER_TOKEN"), "{e}");
        assert!(!e.contains("abc"), "значение токена не повторяется: {e}");
    }

    #[test]
    fn missing_token_server_or_bad_numbers_are_errors() {
        let e = parse(&["--server", "http://x"], &[]).unwrap_err();
        assert!(e.contains("токен"), "{e}");
        let e = parse(&[], &[TOKEN]).unwrap_err();
        assert!(e.contains("--server"), "{e}");
        let e = parse(&["--server", "http://x", "--max-demo-mib", "0"], &[TOKEN]).unwrap_err();
        assert!(e.contains("--max-demo-mib"), "{e}");
        assert!(parse(&["--server", "http://x", "--margin", "-1"], &[TOKEN]).is_err());
        assert!(parse(&["--server", "http://x", "--wat"], &[TOKEN]).is_err());
        assert!(parse(&["--server", "http://x", "лишний"], &[TOKEN]).is_err());
        let e = parse(
            &["--server", "http://x"],
            &[("CS2_RUNNER_TOKEN", "два слова")],
        )
        .unwrap_err();
        assert!(e.contains("токен"), "{e}");
    }

    #[test]
    fn token_file_gives_the_token_without_the_line_break() {
        let dir =
            std::env::temp_dir().join(format!("cs2cinema-runner-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token.txt");
        std::fs::write(&file, "from-file-token\r\n").unwrap();
        let a = parse(
            &[
                "--server",
                "http://x",
                "--token-file",
                file.to_str().unwrap(),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(a.token, "from-file-token");
        std::fs::write(&file, "\n").unwrap();
        let e = parse(
            &[
                "--server",
                "http://x",
                "--token-file",
                file.to_str().unwrap(),
            ],
            &[],
        )
        .unwrap_err();
        assert!(e.contains("пуст"), "{e}");
        let e = parse(
            &["--server", "http://x", "--token-file", "нет-такого-файла"],
            &[],
        )
        .unwrap_err();
        assert!(e.contains("файл токена"), "{e}");
    }

    #[test]
    fn server_and_name_come_from_the_environment() {
        let a = parse(
            &[],
            &[
                TOKEN,
                ("CS2_RUNNER_SERVER", "http://env:1"),
                ("COMPUTERNAME", "PC-AUTHOR"),
            ],
        )
        .unwrap();
        assert_eq!(a.server, "http://env:1");
        assert_eq!(a.name, "PC-AUTHOR");
        let b = parse(
            &["--server", "http://flag:2", "--name", "mine"],
            &[TOKEN, ("CS2_RUNNER_SERVER", "http://env:1")],
        )
        .unwrap();
        assert_eq!(
            (b.server.as_str(), b.name.as_str()),
            ("http://flag:2", "mine")
        );
    }

    #[test]
    fn limits_margin_gpu_dir_and_the_hidden_chunk_limit_are_parsed() {
        let a = parse(
            &[
                "--server",
                "http://x",
                "--max-demo-mib",
                "100",
                "--max-unpacked-mib",
                "200",
                "--inspect-mem-mib",
                "300",
                "--inspect-mins",
                "2",
                "--margin",
                "1.5",
                "--no-gpu",
                "--dir",
                "D:/r",
                "--name",
                "pc",
                "--upload-chunk-limit",
                "1",
            ],
            &[TOKEN],
        )
        .unwrap();
        assert_eq!(a.max_demo_bytes, 100 << 20);
        assert_eq!(
            a.limits.demo.max_file,
            100 << 20,
            "потолок разбора совпадает с потолком скачивания"
        );
        assert_eq!(a.limits.demo.max_unpacked, 200 << 20);
        assert_eq!(a.limits.mem_bytes, 300 << 20);
        assert_eq!(a.limits.secs, 120);
        assert_eq!(a.cfg.time_margin, 1.5);
        assert!(!a.cfg.gpu_encode);
        assert_eq!(a.dir, PathBuf::from("D:/r"));
        assert_eq!(a.name, "pc");
        assert_eq!(a.upload_chunk_limit, Some(1));
        assert!(
            !USAGE_RUNNER.contains("upload-chunk-limit"),
            "флаг учений скрыт из справки"
        );
    }

    #[test]
    fn default_dir_follows_localappdata_then_xdg_then_current_dir() {
        let win = default_dir(&env_of(&[("LOCALAPPDATA", "C:/Users/a/AppData/Local")]));
        assert_eq!(
            win,
            PathBuf::from("C:/Users/a/AppData/Local")
                .join("cs2cinema")
                .join("runner")
        );
        let xdg = default_dir(&env_of(&[("XDG_DATA_HOME", "/home/a/.local/share")]));
        assert_eq!(
            xdg,
            PathBuf::from("/home/a/.local/share")
                .join("cs2cinema")
                .join("runner")
        );
        assert_eq!(default_dir(&env_of(&[])), PathBuf::from("runner"));
    }

    #[test]
    fn usage_names_the_token_variable_and_warns_about_the_app_queue() {
        assert!(USAGE_RUNNER.contains("CS2_RUNNER_TOKEN"));
        assert!(USAGE_RUNNER.contains("одновременно"));
    }
}
