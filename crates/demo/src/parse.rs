use std::{collections::BTreeMap, io};

use prost::Message;

use crate::{
    events::{EventDecoder, GameEvent},
    proto::{
        CDemoFileHeader, CDemoFileInfo, CDemoFullPacket, CDemoPacket, CDemoStringTables,
        CMsgPlayerInfo,
    },
    read_command,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    /// Индекс записи в таблице `userinfo` (слот).
    pub slot: i32,
    pub userid: i32,
    pub name: String,
    pub xuid: u64,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub map_name: String,
    pub playback_ticks: Option<i32>,
    pub playback_time: Option<f32>,
    pub events: Vec<GameEvent>,
    pub players: Vec<PlayerInfo>,
}

impl Parsed {
    pub fn tick_rate(&self) -> f32 {
        match (self.playback_ticks, self.playback_time) {
            (Some(t), Some(s)) if t > 0 && s > 1.0 => t as f32 / s,
            _ => 64.0,
        }
    }
}

fn bad<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Проходит демку один раз и собирает игровые события и игроков.
pub fn parse(data: &[u8]) -> io::Result<Parsed> {
    if data.len() < 16 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "оборванный заголовок демки"));
    }
    if &data[..8] != b"PBDEMS2\0" {
        return Err(bad("ожидалась демка Source 2 (если файл — LFS-заглушка, сделайте git lfs pull)"));
    }

    let mut pos = 16;
    let mut out = Parsed::default();
    let mut decoder = EventDecoder::default();
    let mut players: BTreeMap<i32, PlayerInfo> = BTreeMap::new();

    while pos < data.len() {
        let command = read_command(data, &mut pos)?;
        let payload = command.decode_payload()?;

        match command.kind {
            0 => break,
            1 => {
                let header = CDemoFileHeader::decode(payload.as_slice()).map_err(bad)?;
                out.map_name = header.map_name.unwrap_or_default();
            }
            2 => {
                let info = CDemoFileInfo::decode(payload.as_slice()).map_err(bad)?;
                out.playback_ticks = info.playback_ticks;
                out.playback_time = info.playback_time;
            }
            // 6 — отдельный снимок string tables, 13 — снимок внутри full packet.
            6 => collect_players(&CDemoStringTables::decode(payload.as_slice()).map_err(bad)?, &mut players),
            13 => {
                let full = CDemoFullPacket::decode(payload.as_slice()).map_err(bad)?;
                if let Some(t) = &full.string_table {
                    collect_players(t, &mut players);
                }
                // содержимое full packet — это состояние, а не новые события
            }
            7 | 8 => {
                let packet = CDemoPacket::decode(payload.as_slice()).map_err(bad)?;
                decoder.feed(command.tick, packet.data.as_deref().unwrap_or_default(), &mut out.events);
            }
            _ => {}
        }
    }

    out.players = players.into_values().collect();
    Ok(out)
}

fn collect_players(tables: &CDemoStringTables, players: &mut BTreeMap<i32, PlayerInfo>) {
    for table in tables.tables.iter().filter(|t| t.table_name() == "userinfo") {
        for (index, item) in table.items.iter().enumerate() {
            let Some(data) = item.data.as_deref() else { continue };
            let Ok(info) = CMsgPlayerInfo::decode(data) else { continue };
            let slot = item.str().parse().unwrap_or(index as i32);
            if info.name().is_empty() {
                continue;
            }
            players.insert(
                slot,
                PlayerInfo { slot, userid: info.userid(), name: info.name().to_owned(), xuid: info.xuid() },
            );
        }
    }
}
