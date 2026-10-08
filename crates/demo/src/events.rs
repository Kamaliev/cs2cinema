use std::collections::{BTreeMap, HashMap};

use prost::Message;

use crate::bits::BitReader;
use crate::proto::{
    CMsgSource1LegacyGameEvent as RawEvent, CsvcMsgGameEventList as RawEventList,
    c_msg_source1_legacy_game_event::KeyT,
};

/// id сообщений внутри `CDemoPacket`.
const SVC_GAME_EVENT_LIST: u32 = 205;
const GE_SOURCE1_LEGACY_GAME_EVENT: u32 = 207;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Float(f32),
    Int(i64),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct GameEvent {
    pub tick: i32,
    pub name: String,
    pub fields: BTreeMap<String, Value>,
}

impl GameEvent {
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.fields.get(key)? {
            Value::Int(v) => Some(*v),
            Value::Bool(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        self.int(key).is_some_and(|v| v != 0)
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        match self.fields.get(key)? {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.fields.get(key)? {
            Value::Str(v) => Some(v),
            _ => None,
        }
    }
}

/// Достаёт из `data` пакета (`CDemoPacket.data`) вложенные сообщения: `(id, payload)`.
pub fn inner_messages(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut reader = BitReader::new(data);
    let mut out = Vec::new();
    // хвост короче байта — выравнивающий мусор
    while reader.bits_left() >= 8 {
        let Some(kind) = reader.read_ubit_var() else { break };
        let Some(size) = reader.read_var_u32() else { break };
        let Some(payload) = reader.read_bytes(size as usize) else { break };
        out.push((kind, payload));
    }
    out
}

/// Собирает события из пакетов: список дескрипторов приходит в signon,
/// сами события — в обычных пакетах.
#[derive(Default)]
pub struct EventDecoder {
    descriptors: HashMap<i32, (String, Vec<(String, i32)>)>,
}

impl EventDecoder {
    pub fn feed(&mut self, tick: i32, packet_data: &[u8], out: &mut Vec<GameEvent>) {
        for (kind, payload) in inner_messages(packet_data) {
            match kind {
                SVC_GAME_EVENT_LIST => {
                    if let Ok(list) = RawEventList::decode(payload.as_slice()) {
                        for d in list.descriptors {
                            let keys = d
                                .keys
                                .iter()
                                .map(|k| (k.name.clone().unwrap_or_default(), k.r#type.unwrap_or(0)))
                                .collect();
                            self.descriptors
                                .insert(d.eventid.unwrap_or(-1), (d.name.unwrap_or_default(), keys));
                        }
                    }
                }
                GE_SOURCE1_LEGACY_GAME_EVENT => {
                    if let Ok(raw) = RawEvent::decode(payload.as_slice()) {
                        if let Some(event) = self.decode(tick, &raw) {
                            out.push(event);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn decode(&self, tick: i32, raw: &RawEvent) -> Option<GameEvent> {
        let (name, keys) = self.descriptors.get(&raw.eventid?)?;
        let fields = keys
            .iter()
            .zip(&raw.keys)
            .filter_map(|((key, _declared), value)| Some((key.clone(), value_of(value)?)))
            .collect();
        Some(GameEvent { tick, name: name.clone(), fields })
    }
}

/// В `key_t` заполнено ровно одно поле `val_*` — по нему и определяем значение,
/// не полагаясь на объявленный тип (в CS2 появились типы вроде player_controller).
fn value_of(k: &KeyT) -> Option<Value> {
    if let Some(v) = &k.val_string {
        Some(Value::Str(v.clone()))
    } else if let Some(v) = k.val_float {
        Some(Value::Float(v))
    } else if let Some(v) = k.val_long {
        Some(Value::Int(v as i64))
    } else if let Some(v) = k.val_short {
        Some(Value::Int(v as i64))
    } else if let Some(v) = k.val_byte {
        Some(Value::Int(v as i64))
    } else if let Some(v) = k.val_bool {
        Some(Value::Bool(v))
    } else {
        k.val_uint64.map(|v| Value::Int(v as i64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::csvc_msg_game_event_list::{DescriptorT, KeyT as ListKey};

    /// Пишем вложенное сообщение так же, как это делает сервер.
    fn pack(kind: u32, payload: &[u8]) -> Vec<u8> {
        // kind < 16 -> 6 бит, дальше varuint32 размера (< 128) и байты; для простоты тест выровнен по байтам
        let mut bits: Vec<bool> = Vec::new();
        let push = |bits: &mut Vec<bool>, v: u32, n: usize| (0..n).for_each(|i| bits.push(v >> i & 1 == 1));
        if kind < 16 {
            push(&mut bits, kind, 6);
        } else {
            push(&mut bits, 0x10 | (kind & 15), 6);
            push(&mut bits, kind >> 4, 4);
        }
        push(&mut bits, payload.len() as u32, 8);
        payload.iter().for_each(|b| push(&mut bits, *b as u32, 8));
        let mut out = vec![0u8; bits.len().div_ceil(8)];
        for (i, b) in bits.iter().enumerate() {
            out[i / 8] |= (*b as u8) << (i % 8);
        }
        out
    }

    #[test]
    fn decodes_event_with_descriptor() {
        let list = RawEventList {
            descriptors: vec![DescriptorT {
                eventid: Some(7),
                name: Some("player_death".into()),
                keys: vec![
                    ListKey { r#type: Some(4), name: Some("userid".into()) },
                    ListKey { r#type: Some(6), name: Some("headshot".into()) },
                    ListKey { r#type: Some(1), name: Some("weapon".into()) },
                ],
            }],
        };
        let event = RawEvent {
            event_name: None,
            eventid: Some(7),
            keys: vec![
                KeyT { val_short: Some(3), ..Default::default() },
                KeyT { val_bool: Some(true), ..Default::default() },
                KeyT { val_string: Some("ak47".into()), ..Default::default() },
            ],
        };

        let mut dec = EventDecoder::default();
        let mut out = vec![];
        dec.feed(0, &pack(SVC_GAME_EVENT_LIST, &list.encode_to_vec()), &mut out);
        assert!(out.is_empty());
        dec.feed(640, &pack(GE_SOURCE1_LEGACY_GAME_EVENT, &event.encode_to_vec()), &mut out);

        let e = &out[0];
        assert_eq!((e.tick, e.name.as_str()), (640, "player_death"));
        assert_eq!(e.int("userid"), Some(3));
        assert!(e.bool("headshot"));
        assert_eq!(e.str("weapon"), Some("ak47"));
    }
}
