//! Синтетическая демка для тестов и стендов, как `host::fake`; в продукте не используется.
//!
//! Собирает валидный контейнер `PBDEMS2` с игроками (таблица `userinfo`) и событиями
//! `round_start` / `player_death` / `round_end`, а также zstd-бомбу для проверки потолков.

use prost::Message;

use crate::proto::{
    CDemoFileHeader, CDemoPacket, CDemoStringTables, CMsgPlayerInfo,
    CMsgSource1LegacyGameEvent as Event, CsvcMsgGameEventList as List,
    c_msg_source1_legacy_game_event::KeyT,
    c_demo_string_tables::{ItemsT, TableT},
    csvc_msg_game_event_list::{DescriptorT, KeyT as ListKey},
};

/// Игрок синтетической демки (запись таблицы `userinfo`).
#[derive(Debug, Clone)]
pub struct SynthPlayer {
    /// userid, на который ссылаются события.
    pub userid: i32,
    /// Слот в таблице `userinfo`.
    pub slot: i32,
    pub name: String,
    pub xuid: u64,
}

/// Убийство синтетической демки: `attacker` и `victim` — userid игроков.
#[derive(Debug, Clone, Copy)]
pub struct SynthKill {
    pub tick: u32,
    pub attacker: i32,
    pub victim: i32,
    pub headshot: bool,
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

/// Собирает демку: заголовок с картой, таблица `userinfo`, список событий, `round_start`
/// на тике 1000, убийства, `round_end` через 64 тика после последнего убийства.
pub fn demo(map: &str, players: &[SynthPlayer], kills: &[SynthKill]) -> Vec<u8> {
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
    let header = CDemoFileHeader {
        demo_file_stamp: "PBDEMS2".into(),
        map_name: Some(map.into()),
        ..Default::default()
    };
    command(1, 0, &header.encode_to_vec(), &mut d);

    let items = players
        .iter()
        .map(|p| ItemsT {
            str: Some(p.slot.to_string()),
            data: Some(
                CMsgPlayerInfo { name: Some(p.name.clone()), xuid: Some(p.xuid), userid: Some(p.userid) }
                    .encode_to_vec(),
            ),
        })
        .collect();
    let tables = CDemoStringTables {
        tables: vec![TableT { table_name: Some("userinfo".into()), items, ..Default::default() }],
    };
    command(6, 0, &tables.encode_to_vec(), &mut d);

    command(8, 0, &packet(inner(205, &list.encode_to_vec())), &mut d);
    command(7, 1000, &packet(event(1, vec![])), &mut d);
    for k in kills {
        let keys = vec![int(k.victim), int(k.attacker), flag(k.headshot)];
        command(7, k.tick, &packet(event(3, keys)), &mut d);
    }
    let end = kills.iter().map(|k| k.tick).max().unwrap_or(1000) + 64;
    command(7, end, &packet(event(2, vec![int(2)])), &mut d);
    command(0, end + 1, &[], &mut d);
    d
}

/// zstd-бомба: магия `PBDEMS2`, за которой идут `unpacked_zeros` нулей (первый нуль дополняет
/// магию до `PBDEMS2\0`). Сжимается в крошечный файл, а разворачивается в `unpacked_zeros` байт.
pub fn zstd_bomb(unpacked_zeros: usize) -> Vec<u8> {
    let mut raw = b"PBDEMS2".to_vec();
    raw.resize(raw.len() + unpacked_zeros, 0);
    zstd::encode_all(raw.as_slice(), 0).expect("zstd сжимает память")
}
