//! Служебная подкоманда `cs2-cli inspect`: разбор недоверенной демки в дочернем процессе.
//!
//! Порядок важен: сначала процесс ограничивает себя (память — Job Object, время — сторожевой поток), и только
//! потом открывает чужой файл. Ответ — ровно одна JSON-строка в stdout (`pipeline::inspect::InspectReport`);
//! код выхода 0 значит «ответ напечатан» (и успех, и типизированный отказ). Любой другой код — процесс
//! не уложился в память или время либо упал, и родитель (`pipeline::inspect::preflight`) считает демку негодной.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use pipeline::host::{self, HostError};
use pipeline::inspect::{InspectArgs, InspectReport, inspect_demo};
use pipeline::{FailCode, Quiet};

const MIB: u64 = 1024 * 1024;

const USAGE: &str = "\
cs2-cli inspect — служебный разбор демки в отдельном процессе (его зовёт раннер, а не человек)

Использование:
  cs2-cli inspect <путь к .dem/.dem.zst> [параметры]

Параметры:
  --player <ник>          игрок, про которого будет ролик
  --steam <SteamID64>     SteamID64 игрока (сильнее ника)
  --max-file-mib <N>      потолок размера файла, МиБ        (по умолчанию 2000)
  --max-unpacked-mib <N>  потолок распакованной демки, МиБ  (по умолчанию 3072)
  --mem-mib <N>           потолок памяти процесса, МиБ      (по умолчанию 6144)
  --secs <N>              потолок времени разбора, с        (по умолчанию 900)
  --no-flybys             не разбирать позиции игроков

В stdout печатается одна строка JSON; код выхода 0 — ответ напечатан (в том числе отказ с кодом),
3 — не уложился во время.";

/// Целое число из значения флага.
fn number(name: &str, v: String) -> Result<u64, String> {
    v.trim().parse::<u64>().map_err(|_| format!("{name}: нужно целое число"))
}

/// Разбор аргументов после слова `inspect`. Потолки в МиБ и секундах; не заданные — по умолчанию.
pub fn parse_inspect(args: &[String]) -> Result<InspectArgs, String> {
    let mut a = InspectArgs {
        demo: Default::default(),
        steam: 0,
        nick: String::new(),
        flybys: true,
        limits: pipeline::inspect::InspectLimits::default(),
    };
    let mut path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} требует значение"));
        match arg.as_str() {
            "--player" => a.nick = value("--player")?,
            "--steam" => a.steam = number("--steam", value("--steam")?)?,
            "--max-file-mib" => a.limits.demo.max_file = number("--max-file-mib", value("--max-file-mib")?)?.saturating_mul(MIB),
            "--max-unpacked-mib" => a.limits.demo.max_unpacked = number("--max-unpacked-mib", value("--max-unpacked-mib")?)?.saturating_mul(MIB),
            "--mem-mib" => a.limits.mem_bytes = number("--mem-mib", value("--mem-mib")?)?.saturating_mul(MIB),
            "--secs" => a.limits.secs = number("--secs", value("--secs")?)?,
            "--no-flybys" => a.flybys = false,
            other if other.starts_with("--") => return Err(format!("неизвестный параметр {other}")),
            other => {
                if path.replace(other.to_owned()).is_some() {
                    return Err("нужен ровно один путь к демке".into());
                }
            }
        }
    }
    a.demo = path.ok_or("не указан путь к демке")?.into();
    if a.limits.secs == 0 {
        return Err("--secs: нужно число больше нуля".into());
    }
    Ok(a)
}

/// Печатает единственную JSON-строку ответа; закрытый stdout не должен ронять процесс паникой.
fn print_report(rep: &InspectReport) {
    let line = serde_json::to_string(rep).unwrap_or_else(|_| r#"{"ok":false,"code":"BAD_DEMO","message":"ответ не сериализовался"}"#.to_owned());
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Текст паники из её полезной нагрузки.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "неизвестная причина".to_owned());
    cs2::text::clean_text(&text, 200)
}

pub fn inspect_cmd() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(2).collect();
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_inspect(&raw) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("ошибка: {msg}\n");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    // сначала самоограничение, потом чужой файл
    host::quiet_crash_dialogs();
    let limit = match host::limit_self_memory(args.limits.mem_bytes) {
        Ok(l) => Some(l),
        Err(HostError::Unsupported) => {
            eprintln!("предупреждение: память разбора не ограничена (только Windows)");
            None
        }
        Err(e) => {
            // неисправность окружения, а не демки: раннер вернёт задачу в очередь
            let text = cs2::text::clean_text(&e.to_string(), 200);
            let message = format!("не удалось ограничить память разбора: {text}");
            print_report(&InspectReport { ok: false, code: FailCode::PrecheckFailed.as_str().to_owned(), message, ..Default::default() });
            return ExitCode::SUCCESS;
        }
    };

    // сторож времени: зависший парсер не держит раннер дольше положенного
    let secs = args.limits.secs;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(secs));
        eprintln!("разбор демки не уложился в {secs} с");
        std::process::exit(3);
    });

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inspect_demo(&args, &Quiet)));
    let mut rep = match outcome {
        Ok(rep) => rep,
        Err(payload) => InspectReport {
            ok: false,
            code: FailCode::BadDemo.as_str().to_owned(),
            message: format!("демка не разбирается: разбор упал ({})", panic_text(payload.as_ref())),
            ..Default::default()
        },
    };
    rep.peak_mem_bytes = limit.as_ref().and_then(|l| l.peak_bytes()).unwrap_or(0);
    print_report(&rep);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_path_player_steam_limits_and_flags() {
        let a = parse_inspect(&args(&["m.dem", "--player", "s1mple", "--steam", "7656", "--max-file-mib", "1", "--max-unpacked-mib", "2", "--mem-mib", "64", "--secs", "5", "--no-flybys"])).unwrap();
        assert_eq!(a.demo.to_string_lossy(), "m.dem");
        assert_eq!((a.nick.as_str(), a.steam, a.flybys), ("s1mple", 7656, false));
        assert_eq!((a.limits.demo.max_file, a.limits.demo.max_unpacked, a.limits.mem_bytes, a.limits.secs), (MIB, 2 * MIB, 64 * MIB, 5));
    }

    #[test]
    fn defaults_match_the_starting_numbers() {
        let a = parse_inspect(&args(&["m.dem"])).unwrap();
        assert_eq!((a.nick.as_str(), a.steam, a.flybys), ("", 0, true));
        assert_eq!(a.limits, pipeline::inspect::InspectLimits::default());
        assert_eq!(a.limits.demo.max_file, 2000 * MIB);
        assert_eq!(a.limits.mem_bytes, 6 * 1024 * MIB);
        assert_eq!(a.limits.secs, 900);
    }

    #[test]
    fn rejects_missing_path_unknown_flags_and_bad_numbers() {
        assert!(parse_inspect(&args(&[])).unwrap_err().contains("путь"));
        assert!(parse_inspect(&args(&["a.dem", "b.dem"])).unwrap_err().contains("один путь"));
        assert!(parse_inspect(&args(&["a.dem", "--zzz"])).unwrap_err().contains("--zzz"));
        assert!(parse_inspect(&args(&["a.dem", "--mem-mib"])).unwrap_err().contains("значение"));
        assert!(parse_inspect(&args(&["a.dem", "--mem-mib", "много"])).unwrap_err().contains("число"));
        assert!(parse_inspect(&args(&["a.dem", "--secs", "0"])).unwrap_err().contains("больше нуля"));
    }

    #[test]
    fn nick_that_looks_like_a_flag_is_taken_as_a_value() {
        let a = parse_inspect(&args(&["m.dem", "--player", "--no-flybys"])).unwrap();
        assert_eq!(a.nick, "--no-flybys");
        assert!(a.flybys, "значение --player не флаг");
    }
}
