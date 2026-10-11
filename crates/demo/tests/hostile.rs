//! Враждебные демки: усечённые и побитово испорченные входы не роняют и не вешают разбор,
//! а команда с заявленными гигабайтами отвергается до выделения памяти.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::mpsc,
    time::{Duration, Instant},
};

use demo::{
    proto::CDemoFileHeader,
    synth::{self, SynthKill, SynthPlayer},
};
use prost::Message;

fn sample() -> Vec<u8> {
    let players: Vec<SynthPlayer> = (0..4)
        .map(|i| SynthPlayer {
            userid: i,
            slot: i,
            name: format!("player{i}"),
            xuid: 76_561_197_960_265_728 + i as u64,
        })
        .collect();
    let kills: Vec<SynthKill> = [1400u32, 1520, 1640]
        .into_iter()
        .enumerate()
        .map(|(i, tick)| SynthKill { tick, attacker: 0, victim: 1 + i as i32, headshot: true })
        .collect();
    synth::demo("de_mirage", &players, &kills)
}

/// Разбирает вариант в отдельном потоке: `Ok` и `Err` допустимы, паника и зависание — нет.
fn parse_guarded(label: &str, data: Vec<u8>) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = demo::parse(&data);
        }));
        let _ = tx.send(r.is_ok());
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => {}
        Ok(false) => panic!("{label}: разбор упал паникой"),
        Err(_) => panic!("{label}: разбор завис дольше 5 с"),
    }
}

/// Детерминированный LCG: позиции и маски без новых зависимостей.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn truncated_demos_end_without_panic_or_hang() {
    let demo = sample();
    let step = (demo.len() / 50).max(1);
    for cut in (0..demo.len()).step_by(step) {
        parse_guarded(&format!("усечение до {cut}"), demo[..cut].to_vec());
    }
}

#[test]
fn flipped_bytes_end_without_panic_or_hang() {
    let demo = sample();
    let mut rng = Lcg(0x5EED_CAFE);
    for n in 0..200 {
        let pos = rng.next() as usize % demo.len();
        let mask = (rng.next() % 255) as u8 + 1;
        let mut copy = demo.clone();
        copy[pos] ^= mask;
        parse_guarded(&format!("вариант {n}: байт {pos} ^ {mask:#x}"), copy);
    }
}

fn varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn container(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut d = b"PBDEMS2\0".to_vec();
    d.extend_from_slice(&[0; 8]);
    varint(kind, &mut d);
    varint(0, &mut d);
    varint(payload.len() as u32, &mut d);
    d.extend_from_slice(payload);
    d
}

#[test]
fn command_claiming_four_gib_is_rejected_before_allocation() {
    // snappy-заголовок: varint длины 0xFFFF_FFFF, дальше пара произвольных байт
    let mut payload = Vec::new();
    varint(0xFFFF_FFFF, &mut payload);
    payload.extend_from_slice(&[0, 1, 2, 3]);
    let data = container(1 | 64, &payload);

    let started = Instant::now();
    let err = demo::parse(&data).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(err.to_string().contains("МиБ"), "{err}");
}

#[test]
fn small_compressed_command_still_unpacks() {
    let header = CDemoFileHeader {
        demo_file_stamp: "PBDEMS2".into(),
        map_name: Some("de_inferno".into()),
        ..Default::default()
    };
    let packed = snap::raw::Encoder::new().compress_vec(&header.encode_to_vec()).unwrap();
    let data = container(1 | 64, &packed);

    let parsed = demo::parse(&data).unwrap();
    assert_eq!(parsed.map_name, "de_inferno");
}
