//! Игровая модель матча CS2, собранная из событий демки.

use std::collections::HashMap;

use demo::{
    Parsed,
    events::GameEvent,
};
use serde::{Deserialize, Serialize};

/// Идентификатор игрока: значение `userid` из игровых событий.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId(pub i32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub xuid: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kill {
    pub tick: i32,
    pub round: u32,
    pub attacker: PlayerId,
    pub victim: PlayerId,
    pub assister: Option<PlayerId>,
    pub weapon: String,
    pub headshot: bool,
    pub wallbang: bool,
    pub through_smoke: bool,
    pub no_scope: bool,
    pub attacker_blind: bool,
    pub attacker_in_air: bool,
    pub distance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    pub number: u32,
    pub start_tick: i32,
    pub freeze_end_tick: Option<i32>,
    pub end_tick: Option<i32>,
    pub winner_team: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Match {
    pub map: String,
    pub tick_rate: f32,
    /// Смещение игровых тиков относительно тиков демки (из заголовка демки).
    #[serde(default)]
    pub server_start_tick: i32,
    pub players: Vec<Player>,
    pub rounds: Vec<Round>,
    pub kills: Vec<Kill>,
    /// `(tick, team)` по игрокам — для отсева тимкиллов, если команды известны.
    #[serde(skip)]
    teams: HashMap<PlayerId, Vec<(i32, u8)>>,
}

impl Match {
    pub fn from_parsed(parsed: &Parsed) -> Self {
        let players = parsed
            .players
            .iter()
            .map(|p| (p.userid, p.slot, p.name.clone(), p.xuid))
            .collect();
        let mut m = Self::build(parsed.map_name.clone(), parsed.tick_rate(), players, &parsed.events);
        m.server_start_tick = parsed.server_start_tick;
        m
    }

    /// `players`: `(userid, slot, name, xuid)`.
    pub fn build(
        map: String,
        tick_rate: f32,
        players: Vec<(i32, i32, String, u64)>,
        events: &[GameEvent],
    ) -> Self {
        // событие может ссылаться на игрока как по userid, так и по слоту
        let mut known: HashMap<i32, PlayerId> = HashMap::new();
        let mut list = Vec::new();
        for (userid, slot, name, xuid) in players {
            let id = PlayerId(userid);
            known.insert(slot, id);
            known.insert(userid, id);
            list.push(Player { id, name, xuid });
        }
        let resolve = |e: &GameEvent, key: &str| -> Option<PlayerId> {
            e.int(key).and_then(|v| i32::try_from(v).ok()).and_then(|v| known.get(&v).copied())
        };

        let mut m = Match {
            map,
            tick_rate,
            server_start_tick: 0,
            players: list,
            rounds: Vec::new(),
            kills: Vec::new(),
            teams: HashMap::new(),
        };

        // В демках с серверов FACEIT (SourceTV) нет round_start/round_end —
        // раунд начинается на round_prestart. Выбираем то, что есть в демке.
        let has = |name: &str| events.iter().any(|e| e.name == name);
        let start_event = if has("round_start") { "round_start" } else { "round_prestart" };
        let end_event = if has("round_end") { "round_end" } else { "round_officially_ended" };

        let mut last_start: Option<i32> = None;
        for e in events {
            match e.name.as_str() {
                // перезапуск матча после разминки — всё, что было раньше, не считается
                // (раунд, начавшийся прямо перед перезапуском, остаётся: round_prestart идёт раньше)
                "begin_new_match" => {
                    m.rounds.clear();
                    m.kills.clear();
                    if let Some(start_tick) = last_start.filter(|t| e.tick - t <= (5.0 * tick_rate) as i32) {
                        m.rounds.push(Round { number: 1, start_tick, freeze_end_tick: None, end_tick: None, winner_team: None });
                    }
                }
                n if n == start_event => {
                    last_start = Some(e.tick);
                    let number = m.rounds.len() as u32 + 1;
                    m.rounds.push(Round {
                        number,
                        start_tick: e.tick,
                        freeze_end_tick: None,
                        end_tick: None,
                        winner_team: None,
                    });
                }
                "round_freeze_end" => {
                    if let Some(r) = m.rounds.last_mut() {
                        r.freeze_end_tick = Some(e.tick);
                    }
                }
                n if n == end_event => {
                    if let Some(r) = m.rounds.last_mut() {
                        r.end_tick = Some(e.tick);
                        r.winner_team = e.int("winner").and_then(|w| u8::try_from(w).ok());
                    }
                }
                "player_team" => {
                    if let (Some(p), Some(t)) = (resolve(e, "userid"), e.int("team")) {
                        if let Ok(t) = u8::try_from(t) {
                            m.teams.entry(p).or_default().push((e.tick, t));
                        }
                    }
                }
                "player_death" => {
                    let Some(round) = m.rounds.last().map(|r| r.number) else { continue };
                    let (Some(victim), Some(attacker)) = (resolve(e, "userid"), resolve(e, "attacker"))
                    else {
                        continue; // смерть от мира/падения — не хайлайт
                    };
                    if victim == attacker {
                        continue;
                    }
                    m.kills.push(Kill {
                        tick: e.tick,
                        round,
                        attacker,
                        victim,
                        assister: resolve(e, "assister"),
                        weapon: e.str("weapon").unwrap_or("unknown").trim_start_matches("weapon_").to_owned(),
                        headshot: e.bool("headshot"),
                        wallbang: e.int("penetrated").unwrap_or(0) > 0,
                        through_smoke: e.bool("thrusmoke"),
                        no_scope: e.bool("noscope"),
                        attacker_blind: e.bool("attackerblind"),
                        attacker_in_air: e.bool("attackerinair"),
                        distance: e.float("distance").unwrap_or(0.0),
                    });
                }
                _ => {}
            }
        }

        // тимкиллы убираем, только если обе команды известны наверняка
        let teams = std::mem::take(&mut m.teams);
        let team_at = |p: PlayerId, tick: i32| -> Option<u8> {
            teams.get(&p)?.iter().rev().find(|(t, _)| *t <= tick).map(|(_, team)| *team)
        };
        m.kills.retain(|k| {
            match (team_at(k.attacker, k.tick), team_at(k.victim, k.tick)) {
                (Some(a), Some(v)) => a != v,
                _ => true,
            }
        });
        m.teams = teams;
        m
    }

    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn name(&self, id: PlayerId) -> String {
        self.player(id).map_or_else(|| format!("player {}", id.0), |p| p.name.clone())
    }

    pub fn round(&self, number: u32) -> Option<&Round> {
        self.rounds.get(number.checked_sub(1)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use demo::events::Value;

    fn ev(tick: i32, name: &str, f: &[(&str, i64)]) -> GameEvent {
        GameEvent {
            tick,
            name: name.into(),
            fields: f.iter().map(|(k, v)| (k.to_string(), Value::Int(*v))).collect(),
        }
    }

    fn death(tick: i32, victim: i64, attacker: i64) -> GameEvent {
        let mut e = ev(tick, "player_death", &[("userid", victim), ("attacker", attacker), ("headshot", 1)]);
        e.fields.insert("weapon".into(), Value::Str("weapon_ak47".into()));
        e
    }

    fn players() -> Vec<(i32, i32, String, u64)> {
        (0..4).map(|i| (i, i, format!("p{i}"), i as u64)).collect()
    }

    #[test]
    fn warmup_is_dropped_and_rounds_are_numbered() {
        let events = vec![
            ev(10, "round_start", &[]),
            death(20, 1, 0),
            ev(1000, "begin_new_match", &[]),
            ev(1040, "round_start", &[]),
            death(1050, 1, 0),
            ev(1060, "round_end", &[("winner", 3)]),
            ev(1070, "round_start", &[]),
            death(1080, 2, 0),
        ];
        let m = Match::build("de_mirage".into(), 64.0, players(), &events);
        assert_eq!(m.rounds.len(), 2);
        assert_eq!(m.rounds[0].winner_team, Some(3));
        assert_eq!(m.kills.iter().map(|k| (k.tick, k.round)).collect::<Vec<_>>(), vec![(1050, 1), (1080, 2)]);
        assert_eq!(m.kills[0].weapon, "ak47");
    }

    #[test]
    fn round_started_right_before_restart_is_kept() {
        // как в демках FACEIT: round_prestart, через ~1 с begin_new_match, раунд идёт дальше
        let events = vec![
            ev(10, "round_start", &[]),
            death(20, 1, 0),
            ev(990, "round_start", &[]),
            ev(1050, "begin_new_match", &[]),
            death(1200, 1, 0),
            ev(2000, "round_start", &[]),
            death(2100, 2, 0),
        ];
        let m = Match::build("x".into(), 64.0, players(), &events);
        assert_eq!(m.kills.iter().map(|k| (k.tick, k.round)).collect::<Vec<_>>(), vec![(1200, 1), (2100, 2)]);
        assert_eq!(m.rounds[0].start_tick, 990);
    }

    #[test]
    fn suicides_world_and_teamkills_are_ignored() {
        let events = vec![
            ev(0, "player_team", &[("userid", 0), ("team", 2)]),
            ev(0, "player_team", &[("userid", 1), ("team", 2)]),
            ev(0, "player_team", &[("userid", 2), ("team", 3)]),
            ev(5, "round_start", &[]),
            death(10, 0, 0),   // суицид
            death(11, 1, 99),  // мир
            death(12, 1, 0),   // тимкилл
            death(13, 2, 0),   // нормальное убийство
        ];
        let m = Match::build("x".into(), 64.0, players(), &events);
        assert_eq!(m.kills.len(), 1);
        assert_eq!(m.kills[0].victim, PlayerId(2));
    }
}
