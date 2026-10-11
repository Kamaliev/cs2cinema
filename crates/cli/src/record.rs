//! Запись и повтор задачи из консоли тем же кодом, что и очередь приложения — `pipeline::job::run_job`.
//! Подкоманды `record` и `retry` только разбирают аргументы в `JobSpec` и печатают итог; весь ход задачи
//! (предпроверка, демка, план, запись с проверкой дублей, склейка) живёт в `pipeline`.

use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use pipeline::{
    RunError,
    job::{DemoSource, JobConfig, JobSpec, run_job},
    manifest::Manifest,
};

/// Использование подкоманд `record` и `retry`; печатается и по `--help`.
pub const USAGE_RECORD: &str = "\
cs2-cli record / retry — запись и склейка ролика тем же кодом, что и очередь приложения

Использование:
  cs2-cli record <ссылка FACEIT | .dem> --player <ник> [--hlae <папка>] [--cs2 <папка>] [--out <папка>] [--size ШxВ] [--fps N] [--margin X] [--no-gpu]
  cs2-cli retry <папка задачи> [--hlae <папка>] [--cs2 <папка>] [--margin X] [--no-gpu]

Параметры:
  --player <ник>   игрок, про которого монтируется ролик (обязателен для record)
  --hlae <папка>   папка с HLAE.exe                      (иначе $HLAE_DIR)
  --cs2 <папка>    папка установки CS2                   (иначе $CS2_DIR)
  --out <папка>    корень результатов                    (по умолчанию ./out)
  --size <ШxВ>     размер кадра                          (по умолчанию 1280x720)
  --fps <N>        fps записи                            (по умолчанию 60)
  --margin <X>     запас по времени: множитель дедлайнов (по умолчанию 1)
  --no-gpu         не кодировать на видеокарте (NVENC)

record: предпроверка → демка → план → запись с проверкой дублей → склейка в highlights.mp4.
retry: по record.json из папки задачи, без скачивания и плана; если все дубли уже проверены — сразу склейка.

Папки HLAE и CS2 нужно указать: поиска по дискам в консоли нет. Пустую папку объяснит предпроверка.
Ключ FACEIT берётся из переменной FACEIT_API_KEY.
Закрытие консоли закрывает и запущенную игру.";

/// Прогресс в консоли: этапы и строки журнала идут в stderr. Отмены нет: остановка CLI закрывает
/// дескриптор Job Object, и игра уходит вместе с ним.
pub struct ConsoleProgress;

impl pipeline::Progress for ConsoleProgress {
    fn stage(&self, s: pipeline::Stage) {
        eprintln!("== {}", s.title());
    }
    fn log(&self, msg: &str) {
        eprintln!("{msg}");
    }
}

/// Печатает сбой задачи: код, сообщение и подсказка по-русски; отмена — одним словом.
pub fn print_failure(e: &RunError) {
    match &e.fail {
        Some(f) => {
            eprintln!("ошибка: {}: {}", f.code.as_str(), f.message);
            if !f.hint.is_empty() {
                eprintln!("подсказка: {}", f.hint);
            }
        }
        None if e.message == pipeline::CANCELLED => eprintln!("отменено"),
        None => eprintln!("ошибка: {}", e.message),
    }
}

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Разобранные аргументы: позиционные, конфигурация задачи и ник игрока.
struct Parsed {
    positional: Vec<String>,
    cfg: JobConfig,
    player: Option<String>,
}

/// Общий разбор флагов конфигурации. `plan_flags` — разрешены ли флаги плана (`--player`, `--out`, `--size`,
/// `--fps`): у `retry` план берётся из `record.json`, и эти флаги ничего не изменили бы.
fn parse_cfg(args: &[String], env: Env, plan_flags: bool) -> Result<Parsed, String> {
    let nonempty = |k: &str| env(k).filter(|v| !v.trim().is_empty());
    let mut cfg = JobConfig {
        faceit_key: nonempty("FACEIT_API_KEY").unwrap_or_default(),
        out_root: PathBuf::from("out"),
        ..JobConfig::default()
    };
    let mut hlae = nonempty("HLAE_DIR");
    let mut cs2 = nonempty("CS2_DIR");
    let mut player = None;
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} требует значение"))
        };
        match a.as_str() {
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
            "--player" if plan_flags => player = Some(value("--player")?),
            "--out" if plan_flags => cfg.out_root = value("--out")?.into(),
            "--fps" if plan_flags => {
                cfg.fps = value("--fps")?.parse().map_err(|_| "--fps: нужно число")?;
                if cfg.fps == 0 {
                    return Err("--fps: нужно число больше нуля".into());
                }
            }
            "--size" if plan_flags => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size: нужно ШxВ")?;
                let w: u32 = w.parse().map_err(|_| "--size: нужно число")?;
                let h: u32 = h.parse().map_err(|_| "--size: нужно число")?;
                if w == 0 || h == 0 {
                    return Err("--size: нужно ШxВ больше нуля".into());
                }
                cfg.frame = (w, h);
            }
            s if s.starts_with("--") => {
                return Err(if plan_flags {
                    format!("неизвестный параметр {s}")
                } else {
                    format!("неизвестный параметр {s} (у retry план берётся из record.json)")
                });
            }
            s => positional.push(s.to_owned()),
        }
    }
    cfg.hlae_dir = hlae.map(PathBuf::from).unwrap_or_default();
    cfg.cs2_dir = cs2.map(PathBuf::from).unwrap_or_default();
    Ok(Parsed {
        positional,
        cfg,
        player,
    })
}

/// Номер следующей попытки: на единицу больше наибольшего в `clips/manifest.json`; без манифеста — 1.
fn next_attempt(job_dir: &Path) -> u32 {
    Manifest::load(&job_dir.join("clips"))
        .and_then(|m| m.attempts.iter().map(|a| a.n).max())
        .map_or(1, |n| n + 1)
}

/// `cs2-cli retry <папка задачи>`: «Повторить» по `record.json`.
pub fn parse_retry(args: &[String], env: Env) -> Result<JobSpec, String> {
    let Parsed {
        mut positional,
        cfg,
        ..
    } = parse_cfg(args, env, false)?;
    if positional.is_empty() {
        return Err("укажите папку задачи".into());
    }
    let dir = PathBuf::from(positional.remove(0));
    if let Some(extra) = positional.first() {
        return Err(format!("лишний аргумент {extra}"));
    }
    Ok(JobSpec {
        job_id: 0,
        attempt: next_attempt(&dir),
        record: true,
        resume: true,
        // источник и герой при повторе не нужны — всё в record.json
        source: DemoSource::File(PathBuf::new()),
        hero_steam: 0,
        hero_name: String::new(),
        known_demo: None,
        last_good: None,
        out_dir: Some(dir),
        cfg,
    })
}

/// `cs2-cli record <ссылка|.dem> --player <ник>`: полная задача.
pub fn parse_record(args: &[String], env: Env) -> Result<JobSpec, String> {
    let Parsed {
        mut positional,
        cfg,
        player,
    } = parse_cfg(args, env, true)?;
    if positional.is_empty() {
        return Err("укажите ссылку FACEIT или путь к .dem".into());
    }
    let input = positional.remove(0);
    if let Some(extra) = positional.first() {
        return Err(format!("лишний аргумент {extra}"));
    }
    let Some(player) = player else {
        return Err("укажите --player <ник>".into());
    };
    // существующий файл важнее ссылки: как и в обычном режиме cs2-cli
    let source = match faceit::parse_match_id(&input) {
        Some(id) if !Path::new(&input).exists() => DemoSource::Faceit(id),
        _ => DemoSource::File(PathBuf::from(&input)),
    };
    Ok(JobSpec {
        job_id: 0,
        attempt: 1,
        record: true,
        resume: false,
        source,
        hero_steam: 0,
        hero_name: player,
        known_demo: None,
        last_good: None,
        out_dir: None,
        cfg,
    })
}

fn process_env(k: &str) -> Option<String> {
    env::var(k).ok()
}

/// Абсолютные пути: HLAE пишет относительные от папки exe игры, а `run_job` строит скрипты из `out_root`.
fn absolutize(spec: &mut JobSpec) {
    let abs = |p: &Path| {
        if p.as_os_str().is_empty() {
            p.to_path_buf()
        } else {
            std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
        }
    };
    spec.cfg.out_root = abs(&spec.cfg.out_root);
    if let Some(d) = spec.out_dir.as_mut() {
        *d = abs(d);
    }
    if let DemoSource::File(p) = &mut spec.source {
        *p = abs(p);
    }
}

/// Общая обвязка подкоманд: `--help`, разбор, запуск `run_job`, печать итога. Коды выхода: 0, 1 — сбой, 2 — аргументы.
fn run_cmd(
    parse: fn(&[String], Env) -> Result<JobSpec, String>,
    announce: fn(&JobSpec) -> Option<String>,
) -> ExitCode {
    let args: Vec<String> = env::args().skip(2).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE_RECORD}");
        return ExitCode::SUCCESS;
    }
    let mut spec = match parse(&args, &process_env) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("ошибка: {msg}\n\n{USAGE_RECORD}");
            return ExitCode::from(2);
        }
    };
    absolutize(&mut spec);
    if let Some(line) = announce(&spec) {
        eprintln!("{line}");
    }
    match run_job(&*pipeline::host::default_host(), &spec, &ConsoleProgress) {
        Ok(o) => {
            println!("Готово: {}", o.result.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            print_failure(&e);
            ExitCode::FAILURE
        }
    }
}

/// `cs2-cli record …`.
pub fn record_cmd() -> ExitCode {
    run_cmd(parse_record, |s| {
        let source = match &s.source {
            DemoSource::File(p) => p.display().to_string(),
            DemoSource::Faceit(id) => format!("матч FACEIT {id}"),
        };
        // ключ FACEIT в строку не попадает (T-04-16)
        Some(format!(
            "запись: {source} · игрок {} · кадр {}x{} · результаты в {}",
            s.hero_name,
            s.cfg.frame.0,
            s.cfg.frame.1,
            s.cfg.out_root.display()
        ))
    })
}

/// `cs2-cli retry …`.
pub fn retry_cmd() -> ExitCode {
    run_cmd(parse_retry, |s| {
        s.out_dir
            .as_ref()
            .map(|d| format!("повтор: задача в {}", d.display()))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn retry_takes_the_folder_and_hlae_flag() {
        let env = env_of(&[]);
        let s = parse_retry(&args(&["C:/out/job", "--hlae", "C:/hlae"]), &env).unwrap();
        assert!(s.resume && s.record);
        assert_eq!(s.out_dir, Some(PathBuf::from("C:/out/job")));
        assert_eq!(s.cfg.hlae_dir, PathBuf::from("C:/hlae"));
        assert_eq!(s.attempt, 1, "без манифеста попытка первая");
    }

    #[test]
    fn retry_reads_dirs_from_the_environment() {
        let env = env_of(&[("HLAE_DIR", "D:/hlae"), ("CS2_DIR", "D:/cs2")]);
        let s = parse_retry(&args(&["job"]), &env).unwrap();
        assert_eq!(s.cfg.hlae_dir, PathBuf::from("D:/hlae"));
        assert_eq!(s.cfg.cs2_dir, PathBuf::from("D:/cs2"));
        let s = parse_retry(&args(&["job", "--hlae", "E:/h"]), &env).unwrap();
        assert_eq!(
            s.cfg.hlae_dir,
            PathBuf::from("E:/h"),
            "флаг важнее переменной"
        );
        let s = parse_retry(&args(&["job"]), &env_of(&[])).unwrap();
        assert!(s.cfg.hlae_dir.as_os_str().is_empty() && s.cfg.cs2_dir.as_os_str().is_empty());
    }

    #[test]
    fn retry_requires_a_folder_and_rejects_plan_flags() {
        let env = env_of(&[]);
        assert_eq!(
            parse_retry(&args(&[]), &env).unwrap_err(),
            "укажите папку задачи"
        );
        assert!(
            parse_retry(&args(&["a", "b"]), &env)
                .unwrap_err()
                .contains("лишний аргумент b")
        );
        assert!(
            parse_retry(&args(&["a", "--size", "1920x1080"]), &env)
                .unwrap_err()
                .contains("неизвестный параметр --size")
        );
    }

    #[test]
    fn record_parses_a_faceit_link() {
        let env = env_of(&[("FACEIT_API_KEY", "секрет")]);
        let s = parse_record(
            &args(&[
                "https://www.faceit.com/ru/cs2/room/1-0e0b2a0c-6a3b-4b2d-9c3e-1a2b3c4d5e6f",
                "--player",
                "wytom",
            ]),
            &env,
        )
        .unwrap();
        assert_eq!(
            s.source,
            DemoSource::Faceit("1-0e0b2a0c-6a3b-4b2d-9c3e-1a2b3c4d5e6f".into())
        );
        assert!(s.record && !s.resume);
        assert_eq!((s.hero_name.as_str(), s.hero_steam), ("wytom", 0));
        assert_eq!(s.cfg.faceit_key, "секрет");
        assert!(
            !format!("{s:?}").contains("секрет"),
            "ключ не должен попасть в Debug"
        );
    }

    #[test]
    fn record_parses_a_demo_file_with_frame_and_fps() {
        let env = env_of(&[]);
        let s = parse_record(
            &args(&[
                "match.dem",
                "--player",
                "wytom",
                "--size",
                "1920x1080",
                "--fps",
                "30",
                "--margin",
                "1.5",
                "--no-gpu",
            ]),
            &env,
        )
        .unwrap();
        assert_eq!(s.source, DemoSource::File(PathBuf::from("match.dem")));
        assert_eq!((s.cfg.frame, s.cfg.fps), ((1920, 1080), 30));
        assert_eq!(s.cfg.time_margin, 1.5);
        assert!(!s.cfg.gpu_encode);
    }

    #[test]
    fn record_reports_argument_errors_in_russian() {
        let env = env_of(&[]);
        let err = |a: &[&str]| parse_record(&args(a), &env).unwrap_err();
        assert_eq!(err(&["match.dem"]), "укажите --player <ник>");
        assert_eq!(
            err(&["match.dem", "--player", "x", "--size", "1920"]),
            "--size: нужно ШxВ"
        );
        assert!(err(&["a.dem", "b.dem", "--player", "x"]).contains("лишний аргумент b.dem"));
        assert!(err(&["a.dem", "--player", "x", "--nope"]).contains("неизвестный параметр --nope"));
        assert!(err(&["a.dem", "--player"]).contains("--player требует значение"));
        assert_eq!(
            err(&["--player", "x"]),
            "укажите ссылку FACEIT или путь к .dem"
        );
    }
}
