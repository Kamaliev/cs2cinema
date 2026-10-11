//! Потолки чтения демки: бомбы, огромный файл, мусор и синтетическая демка с игроками.

use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use demo::{
    DemoError, DemoLimits, read_demo_limited,
    synth::{self, SynthKill, SynthPlayer},
};

const MIB: u64 = 1 << 20;

/// Временный файл с уникальным по PID и имени теста путём; удаляется при уничтожении.
struct Temp(PathBuf);

impl Temp {
    fn new(name: &str, bytes: &[u8]) -> Temp {
        let path = std::env::temp_dir().join(format!("cs2c_limits_{}_{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn players() -> Vec<SynthPlayer> {
    (0..4)
        .map(|i| SynthPlayer {
            userid: i,
            slot: i,
            name: format!("player{i}"),
            xuid: 76_561_197_960_265_728 + i as u64,
        })
        .collect()
}

fn kills() -> Vec<SynthKill> {
    [1400u32, 1520, 1640]
        .into_iter()
        .enumerate()
        .map(|(i, tick)| SynthKill { tick, attacker: 0, victim: 1 + (i as i32 % 3), headshot: true })
        .collect()
}

fn small_limits() -> DemoLimits {
    DemoLimits { max_file: 100 * MIB, max_unpacked: MIB }
}

#[test]
fn synthetic_demo_with_players_parses_back() {
    let file = Temp::new("synth.dem", &synth::demo("de_mirage", &players(), &kills()));
    let bytes = read_demo_limited(&file.0, &DemoLimits::default()).unwrap();
    let parsed = demo::parse(&bytes).unwrap();

    assert_eq!(parsed.map_name, "de_mirage");
    assert_eq!(parsed.players.len(), 4);
    for (i, p) in parsed.players.iter().enumerate() {
        assert_eq!(p.name, format!("player{i}"));
        assert_eq!(p.xuid, 76_561_197_960_265_728 + i as u64);
    }
    assert_eq!(parsed.events.iter().filter(|e| e.name == "player_death").count(), 3);
}

#[test]
fn compressed_demo_reads_the_same_bytes() {
    let raw = synth::demo("de_mirage", &players(), &kills());
    let plain = Temp::new("plain.dem", &raw);
    let zst = Temp::new("packed.dem.zst", &zstd::encode_all(raw.as_slice(), 0).unwrap());

    let a = read_demo_limited(&plain.0, &DemoLimits::default()).unwrap();
    let b = read_demo_limited(&zst.0, &DemoLimits::default()).unwrap();
    assert_eq!(a, b);
    assert_eq!(a, raw);
}

#[test]
fn zstd_bomb_is_too_large_without_unpacking_it_all() {
    let file = Temp::new("bomb.dem.zst", &synth::zstd_bomb(8 << 20));
    let started = Instant::now();
    let err = read_demo_limited(&file.0, &small_limits()).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1), "бомба разбиралась слишком долго");
    match &err {
        DemoError::TooLarge { what, .. } => assert!(what.contains("распакованн"), "{what}"),
        other => panic!("ожидали TooLarge, получили {other:?}"),
    }
    assert!(err.to_string().contains("распакованн"));
}

#[test]
fn file_over_the_size_limit_is_too_large_before_reading() {
    let file = Temp::new("big.dem", &vec![0u8; 2 << 20]);
    let limits = DemoLimits { max_file: MIB, max_unpacked: 10 * MIB };
    let err = read_demo_limited(&file.0, &limits).unwrap_err();
    match &err {
        DemoError::TooLarge { what, .. } => assert!(what.contains("файл"), "{what}"),
        other => panic!("ожидали TooLarge, получили {other:?}"),
    }
}

#[test]
fn text_file_and_lfs_pointer_are_bad_demos() {
    let text = Temp::new("hello.dem", b"hello");
    assert!(matches!(read_demo_limited(&text.0, &DemoLimits::default()), Err(DemoError::Bad(_))));

    let pointer = Temp::new(
        "lfs.dem",
        b"version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 123\n",
    );
    match read_demo_limited(&pointer.0, &DemoLimits::default()) {
        Err(DemoError::Bad(msg)) => assert!(msg.contains("git lfs pull"), "{msg}"),
        other => panic!("ожидали Bad, получили {other:?}"),
    }
}

#[test]
fn garbage_after_a_zstd_header_is_a_bad_demo() {
    let mut bytes = vec![0x28, 0xB5, 0x2F, 0xFD];
    bytes.extend_from_slice(&[0xFF; 64]);
    let file = Temp::new("garbage.dem.zst", &bytes);
    assert!(matches!(read_demo_limited(&file.0, &DemoLimits::default()), Err(DemoError::Bad(_))));
}

#[test]
fn gzip_bomb_is_too_large() {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(b"PBDEMS2").unwrap();
    enc.write_all(&vec![0u8; 8 << 20]).unwrap();
    let file = Temp::new("bomb.dem.gz", &enc.finish().unwrap());

    let err = read_demo_limited(&file.0, &small_limits()).unwrap_err();
    assert!(matches!(err, DemoError::TooLarge { .. }), "{err:?}");
}
