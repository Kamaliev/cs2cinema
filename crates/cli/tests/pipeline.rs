//! Сквозной тест на синтетической демке: контейнер -> события -> хайлайты -> план -> файлы.

use demo::proto::{
    CDemoFileHeader, CDemoPacket, CMsgSource1LegacyGameEvent as Event, CsvcMsgGameEventList as List,
    c_msg_source1_legacy_game_event::KeyT, csvc_msg_game_event_list::{DescriptorT, KeyT as ListKey},
};
use prost::Message;

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

fn command(kind: u32, tick: u32, payload: &[u8], out: &mut Vec<u8>) {
    varint(kind, out);
    varint(tick, out);
    varint(payload.len() as u32, out);
    out.extend_from_slice(payload);
}

/// Вложенное сообщение пакета (ubitvar + varuint32 + байты), битовая упаковка.
fn inner(kind: u32, payload: &[u8]) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    let mut push = |v: u32, n: usize| (0..n).for_each(|i| bits.push(v >> i & 1 == 1));
    if kind < 16 {
        push(kind, 6);
    } else {
        push(0x10 | (kind & 15), 6);
        push(kind >> 4, 4);
    }
    let mut size = Vec::new();
    varint(payload.len() as u32, &mut size);
    size.iter().for_each(|b| push(*b as u32, 8));
    payload.iter().for_each(|b| push(*b as u32, 8));
    let mut out = vec![0u8; bits.len().div_ceil(8)];
    bits.iter().enumerate().for_each(|(i, b)| out[i / 8] |= (*b as u8) << (i % 8));
    out
}

fn packet(data: Vec<u8>) -> Vec<u8> {
    CDemoPacket { data: Some(data) }.encode_to_vec()
}

fn event(id: i32, keys: Vec<KeyT>) -> Vec<u8> {
    inner(207, &Event { event_name: None, eventid: Some(id), keys }.encode_to_vec())
}

fn int(v: i32) -> KeyT {
    KeyT { val_short: Some(v), ..Default::default() }
}

fn flag(v: bool) -> KeyT {
    KeyT { val_bool: Some(v), ..Default::default() }
}

fn synthetic_demo() -> Vec<u8> {
    let desc = |id, name: &str, keys: &[&str]| DescriptorT {
        eventid: Some(id),
        name: Some(name.into()),
        keys: keys.iter().map(|k| ListKey { r#type: Some(4), name: Some((*k).into()) }).collect(),
    };
    let list = List {
        descriptors: vec![
            desc(1, "round_start", &[]),
            desc(2, "round_end", &["winner"]),
            desc(3, "player_death", &["userid", "attacker", "headshot"]),
        ],
    };

    let mut d = b"PBDEMS2\0".to_vec();
    d.extend_from_slice(&[0; 8]);
    let header = CDemoFileHeader { demo_file_stamp: "PBDEMS2".into(), map_name: Some("de_mirage".into()), ..Default::default() };
    command(1, 0, &header.encode_to_vec(), &mut d);
    command(8, 0, &packet(inner(205, &list.encode_to_vec())), &mut d);

    command(7, 1000, &packet(event(1, vec![])), &mut d);
    // трое убитых подряд игроком 0 — 3K
    for (i, tick) in [1400u32, 1520, 1640].into_iter().enumerate() {
        command(7, tick, &packet(event(3, vec![int(10 + i as i32), int(0), flag(true)])), &mut d);
    }
    command(7, 2000, &packet(event(2, vec![int(2)])), &mut d);
    command(0, 2001, &[], &mut d);
    d
}

#[test]
fn synthetic_demo_goes_through_whole_pipeline() {
    let parsed = demo::parse(&synthetic_demo()).unwrap();
    assert_eq!(parsed.map_name, "de_mirage");
    assert_eq!(parsed.events.iter().filter(|e| e.name == "player_death").count(), 3);

    // игроков в таблице нет — расставляем сами, как это сделал бы userinfo
    let players = (0..13).map(|i| (i, i, format!("player{i}"), 76_561_197_960_265_728 + i as u64)).collect();
    let m = cs2::Match::build(parsed.map_name.clone(), 64.0, players, &parsed.events);
    assert_eq!((m.rounds.len(), m.kills.len()), (1, 3));

    let found = highlights::find(&m, &highlights::Config::default());
    assert_eq!(found[0].kind, highlights::Kind::MultiKill(3));

    let timeline = director::direct(&m, &found, &director::Config::default(), &director::NoPositions);
    assert_eq!(timeline.shots.len(), 1);

    let out = renderer::render(&m, &timeline, &renderer::Config::default());
    let names: Vec<_> = out.files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"plan.json") && names.contains(&"highlights.cfg") && names.contains(&"assemble.sh"));
}

#[test]
fn lfs_pointer_gives_clear_error() {
    let err = demo::parse(b"version https://git-lfs.github.com/spec/v1\noid sha256:6bec\nsize 282915215\n").unwrap_err();
    assert!(err.to_string().contains("lfs"), "{err}");
}
