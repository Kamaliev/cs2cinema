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

const MIB: usize = 1024 * 1024;

#[test]
fn bomb_through_the_child_is_demo_too_large() {
    let bomb = write("bomb", "bomb.dem.zst", &demo::synth::zstd_bomb(8 * MIB));
    assert!(std::fs::metadata(&bomb).unwrap().len() < MIB as u64, "бомба должна быть крошечной на диске");
    let rep = one_json_line(&inspect(&bomb, &["--player", "x", "--max-unpacked-mib", "1", "--no-flybys"]));
    assert_eq!((rep.ok, rep.code.as_str()), (false, "DEMO_TOO_LARGE"), "{rep:?}");
}

#[test]
fn file_over_the_limit_through_the_child_is_demo_too_large() {
    let big = write("big", "big.dem", &vec![0u8; 2 * MIB]);
    let rep = one_json_line(&inspect(&big, &["--max-file-mib", "1", "--no-flybys"]));
    assert_eq!((rep.ok, rep.code.as_str()), (false, "DEMO_TOO_LARGE"), "{rep:?}");
}

#[test]
fn garbage_file_through_the_child_is_bad_demo() {
    let hello = write("hello", "hello.dem", b"hello");
    let rep = one_json_line(&inspect(&hello, &["--player", "x", "--no-flybys"]));
    assert_eq!((rep.ok, rep.code.as_str()), (false, "BAD_DEMO"), "{rep:?}");

    let missing = dir("missing").join("нет-такого.dem");
    let rep = one_json_line(&inspect(&missing, &["--no-flybys"]));
    assert_eq!(rep.code, "BAD_DEMO", "{rep:?}");
}

/// Детерминированный LCG, как в `demo/tests/hostile.rs`: позиции и маски без новых зависимостей.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Один запуск ребёнка на испорченных байтах: меньше 30 с, одна JSON-строка, допустимый исход.
fn mutant_answers(label: &str, bytes: &[u8]) {
    let path = write("mutants", "m.dem", bytes);
    let started = std::time::Instant::now();
    let out = inspect(&path, &["--player", "s1mple", "--secs", "25", "--no-flybys"]);
    assert!(started.elapsed().as_secs() < 30, "{label}: ребёнок работал слишком долго");
    let rep = one_json_line(&out);
    let allowed = ["BAD_DEMO", "PLAYER_NOT_FOUND", "PLAYER_AMBIGUOUS", "NO_HIGHLIGHTS"];
    assert!(rep.ok || allowed.contains(&rep.code.as_str()), "{label}: неожиданный код {rep:?}");
}

#[test]
fn mutated_demos_through_the_child_answer_with_one_json_line() {
    let demo = synthetic();
    let step = (demo.len() / 20).max(1);
    for cut in (0..demo.len()).step_by(step).take(20) {
        mutant_answers(&format!("усечение до {cut}"), &demo[..cut]);
    }
    let mut rng = Lcg(0x5EED_CAFE);
    for n in 0..50 {
        let pos = rng.next() as usize % demo.len();
        let mask = (rng.next() % 255) as u8 + 1;
        let mut copy = demo.clone();
        copy[pos] ^= mask;
        mutant_answers(&format!("вариант {n}: байт {pos} ^ {mask:#x}"), &copy);
    }
}

#[cfg(windows)]
#[test]
fn child_over_the_memory_limit_becomes_bad_demo_for_the_parent() {
    // 200 МиБ нулей после распаковки при потолке памяти ребёнка 64 МиБ: потолок распаковки (2 ГиБ) не сработает,
    // сработает только Job Object
    let bomb = write("mem", "mem.dem.zst", &demo::synth::zstd_bomb(200 * MIB));
    let mut args = parent_args(&bomb, "x");
    args.limits.demo.max_unpacked = 2048 * MIB as u64;
    args.limits.mem_bytes = 64 * MIB as u64;
    args.limits.secs = 60;

    // сам ребёнок упирается в лимит: либо ловит отказ выделения и отвечает BAD_DEMO, либо умирает без ответа
    let direct = Command::new(EXE)
        .arg("inspect")
        .arg(&bomb)
        .args(["--max-unpacked-mib", "2048", "--mem-mib", "64", "--secs", "60", "--no-flybys"])
        .output()
        .unwrap();
    if direct.status.code() == Some(0) {
        let rep = one_json_line(&direct);
        assert_eq!((rep.ok, rep.code.as_str()), (false, "BAD_DEMO"), "{rep:?}");
        assert!(rep.peak_mem_bytes <= 64 * MIB as u64, "пик памяти выше лимита: {rep:?}");
    }

    let host = pipeline::host::default_host();
    let e = pipeline::inspect::preflight(&*host, Path::new(EXE), &args, &|| false).expect_err("ребёнок упёрся в лимит памяти");
    let fail = e.fail.expect("типизированный сбой");
    assert_eq!(fail.code, FailCode::BadDemo);
    assert!(e.message.contains("не уложился в память или время") || e.message.contains("out of memory"), "{}", e.message);
}

#[test]
fn cli_player_flag_refuses_an_ambiguous_nick() {
    let demo = write("cli-player", "match.dem", &synthetic());
    let bad = Command::new(EXE).arg(&demo).args(["--player", "mple", "--list"]).output().unwrap();
    assert_eq!(bad.status.code(), Some(1), "stdout: {}", String::from_utf8_lossy(&bad.stdout));
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("подходит нескольким игрокам"), "{err}");

    let good = Command::new(EXE).arg(&demo).args(["--player", "s1mple", "--list"]).output().unwrap();
    assert_eq!(good.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&good.stderr));
}
