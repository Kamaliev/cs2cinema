//! Дочерний разбор демки `cs2-cli inspect` через собранный бинарь: синтетические и враждебные демки,
//! ровно одна JSON-строка в stdout, типизированные отказы и поведение родителя `pipeline::inspect::preflight`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use demo::synth::{SynthKill, SynthPlayer};
use pipeline::inspect::{InspectArgs, InspectLimits, InspectReport};
use pipeline::{FailCode, RecordState};

const EXE: &str = env!("CARGO_BIN_EXE_cs2-cli");

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cs2cinema-inspect-it-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Демка: `s1mple`, `s1mple_fan`, `ZywOo`, `quiet`; у `s1mple` три убийства в одном раунде.
fn synthetic() -> Vec<u8> {
    let p = |userid, name: &str, xuid| SynthPlayer { userid, slot: userid, name: name.into(), xuid };
    let k = |tick, attacker, victim| SynthKill { tick, attacker, victim, headshot: true };
    let players = [
        p(0, "s1mple", 76561198034202275),
        p(1, "s1mple_fan", 76561198000000002),
        p(2, "ZywOo", 76561198000000003),
        p(3, "quiet", 76561198000000004),
    ];
    let kills = [k(1400, 0, 1), k(1520, 0, 2), k(1640, 0, 1), k(1800, 2, 0)];
    demo::synth::demo("de_mirage", &players, &kills)
}

fn write(tag: &str, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir(tag).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn inspect(path: &Path, extra: &[&str]) -> Output {
    Command::new(EXE).arg("inspect").arg(path).args(extra).output().expect("cs2-cli запускается")
}

/// Код выхода 0 и ровно одна непустая строка stdout — JSON-ответ.
fn one_json_line(out: &Output) -> InspectReport {
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "stdout должен быть одной строкой: {text:?}");
    serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("не JSON ({e}): {text:?}"))
}

#[test]
fn inspect_child_answers_with_one_json_line_and_typed_refusals() {
    let demo = write("typed", "match.dem", &synthetic());

    let ok = one_json_line(&inspect(&demo, &["--player", "s1mple", "--no-flybys"]));
    assert!(ok.ok, "{ok:?}");
    assert_eq!(ok.player_name, "s1mple");
    assert_eq!(ok.map, "de_mirage");
    assert!(ok.moments >= 1, "{ok:?}");
    assert_eq!(ok.player_steam, 76561198034202275);

    let amb = one_json_line(&inspect(&demo, &["--player", "mple", "--no-flybys"]));
    assert!(!amb.ok);
    assert_eq!(amb.code, "PLAYER_AMBIGUOUS");
    assert_eq!(amb.candidates.len(), 2, "{amb:?}");

    let none = one_json_line(&inspect(&demo, &["--player", "nobody", "--no-flybys"]));
    assert_eq!((none.ok, none.code.as_str()), (false, "PLAYER_NOT_FOUND"));

    let quiet = one_json_line(&inspect(&demo, &["--player", "quiet", "--no-flybys"]));
    assert_eq!((quiet.ok, quiet.code.as_str()), (false, "NO_HIGHLIGHTS"));
}

#[test]
fn inspect_help_is_russian_and_a_missing_path_is_exit_2() {
    let help = Command::new(EXE).args(["inspect", "--help"]).output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("Использование"), "{}", String::from_utf8_lossy(&help.stdout));

    let bare = Command::new(EXE).arg("inspect").output().unwrap();
    assert_eq!(bare.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("ошибка:"));
    assert!(bare.stdout.is_empty(), "при ошибке аргументов JSON не печатается");
}

fn parent_args(demo: &Path, nick: &str) -> InspectArgs {
    InspectArgs { demo: demo.to_path_buf(), steam: 0, nick: nick.into(), flybys: false, limits: InspectLimits::default() }
}

#[test]
fn parent_preflight_gets_the_player_or_a_typed_refusal_from_a_real_child() {
    let demo = write("parent", "match.dem", &synthetic());
    let host = pipeline::host::default_host();
    let exe = Path::new(EXE);

    let ok = pipeline::inspect::preflight(&*host, exe, &parent_args(&demo, "s1mple"), &|| false).expect("игрок найден");
    assert_eq!(ok.player_name, "s1mple");
    assert!(ok.moments >= 1);

    let e = pipeline::inspect::preflight(&*host, exe, &parent_args(&demo, "mple"), &|| false).expect_err("неоднозначный ник");
    let fail = e.fail.expect("типизированный сбой");
    assert_eq!(fail.code, FailCode::PlayerAmbiguous);
    assert_eq!(fail.state, RecordState::Precheck);
    assert!(e.message.contains("s1mple_fan"), "{}", e.message);
}

#[test]
fn analyze_with_types_a_garbage_file_as_bad_demo() {
    let garbage = write("garbage-lib", "hello.dem", b"hello");
    let e = pipeline::analyze_with(&pipeline::Source::File(garbage), &demo::DemoLimits::default(), &pipeline::Quiet)
        .err()
        .expect("мусор не демка");
    assert_eq!(e.fail.map(|f| f.code), Some(FailCode::BadDemo));
}
