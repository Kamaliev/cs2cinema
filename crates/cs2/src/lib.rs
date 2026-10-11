//! Игровая модель матча CS2, собранная из событий демки.

use std::collections::HashMap;

pub mod report;
pub mod text;

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
    /// Слот в таблице `userinfo`; `spec_player` в CS2 принимает слот + 1.
    #[serde(default)]
    pub slot: i32,
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
    /// Ассист — флешкой (`assistedflash`): такой ассист показывается как убийство под флешку.
    #[serde(default)]
    pub assisted_flash: bool,
    pub distance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrenadeKind {
    He,
    Flash,
    /// Молотов или зажигательная.
    Fire,
    Smoke,
}

/// Брошенная граната и что она сделала (только по противникам).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grenade {
    pub kind: GrenadeKind,
    pub thrower: PlayerId,
    pub round: u32,
    /// Тик броска (`weapon_fire`), если нашёлся.
    pub throw_tick: Option<i32>,
    /// Взрыв / начало горения / дым.
    pub detonate_tick: i32,
    pub pos: [f32; 3],
    /// Урон противникам (HE, огонь).
    pub damage: i32,
    /// Кому из противников нанесён урон.
    #[serde(default)]
    pub hurt: Vec<PlayerId>,
    /// Ослеплённые противники и длительность, секунды (флешка).
    pub blinded: Vec<(PlayerId, f32)>,
    /// Противники, убитые гранатой, — или, для флешки, убитые командой, пока были ослеплены.
    pub kills: Vec<PlayerId>,
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
    #[serde(default)]
    pub grenades: Vec<Grenade>,
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
            let mut name = text::clean_text(&name, text::NAME_MAX);
            if name.is_empty() {
                name = format!("игрок {userid}");
            }
            list.push(Player { id, slot, name, xuid });
        }
        let resolve = |e: &GameEvent, key: &str| -> Option<PlayerId> {
            e.int(key).and_then(|v| i32::try_from(v).ok()).and_then(|v| known.get(&v).copied())
        };

        let mut m = Match {
            map: text::clean_map(&map),
            tick_rate,
            server_start_tick: 0,
            players: list,
            rounds: Vec::new(),
            kills: Vec::new(),
            grenades: Vec::new(),
            teams: HashMap::new(),
        };

        // В демках с серверов FACEIT (SourceTV) нет round_start/round_end —
        // раунд начинается на round_prestart. Выбираем то, что есть в демке.
        let has = |name: &str| events.iter().any(|e| e.name == name);
        let start_event = if has("round_start") { "round_start" } else { "round_prestart" };
        let end_event = if has("round_end") { "round_end" } else { "round_officially_ended" };

        let mut last_start: Option<i32> = None;
        let mut live_from = i32::MIN;
        for e in events {
            match e.name.as_str() {
                // перезапуск матча после разминки — всё, что было раньше, не считается
                // (раунд, начавшийся прямо перед перезапуском, остаётся: round_prestart идёт раньше)
                "begin_new_match" => {
                    live_from = e.tick;
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
                        // без поля winner (FACEIT) не затираем победителя, выведенного по бомбе
                        if let Some(w) = e.int("winner").and_then(|w| u8::try_from(w).ok()) {
                            r.winner_team = Some(w);
                        }
                    }
                }
                // в FACEIT-демках у конца раунда нет победителя — бомба его выдаёт
                "bomb_exploded" | "bomb_defused" => {
                    if let Some(r) = m.rounds.last_mut() {
                        r.winner_team.get_or_insert(if e.name == "bomb_exploded" { 2 } else { 3 });
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
                        assisted_flash: e.bool("assistedflash"),
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
        m.grenades = grenades(&m, events, live_from, &resolve, &team_at);
        m.teams = teams;
        m.infer_winners();
        m
    }

    /// Победитель раунда без `winner` в демке: команда, выбившая всех соперников, иначе CT (время вышло).
    /// Бомба (взрыв/разминирование) уже учтена при разборе событий. У последнего раунда матча
    /// конца в демке нет — для него только «всех выбили».
    fn infer_winners(&mut self) {
        let mut winners = Vec::new();
        for r in &self.rounds {
            if r.winner_team.is_some() {
                winners.push(r.winner_team);
                continue;
            }
            let on = |team: u8| self.players.iter().filter(|p| self.team_at(p.id, r.start_tick) == Some(team)).count();
            let dead = |team: u8| self.kills.iter().filter(|k| k.round == r.number && self.team_at(k.victim, k.tick) == Some(team)).count();
            let (t, ct) = (on(2), on(3));
            winners.push(match () {
                _ if t == 0 || ct == 0 => None,
                _ if dead(2) >= t => Some(3),
                _ if dead(3) >= ct => Some(2),
                _ if r.end_tick.is_none() => None,
                _ => Some(3),
            });
        }
        for (r, w) in self.rounds.iter_mut().zip(winners) {
            r.winner_team = w;
        }
    }

    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn name(&self, id: PlayerId) -> String {
        self.player(id).map_or_else(|| format!("player {}", id.0), |p| p.name.clone())
    }

    /// Команда игрока на тике (2 — T, 3 — CT), если демка её сообщила.
    pub fn team_at(&self, id: PlayerId, tick: i32) -> Option<u8> {
        let list = self.teams.get(&id)?;
        list.iter().rev().find(|(t, _)| *t <= tick).or(list.first()).map(|(_, team)| *team)
    }

    pub fn round(&self, number: u32) -> Option<&Round> {
        self.rounds.get(number.checked_sub(1)? as usize)
    }
}

/// Гранаты из событий: бросок (`weapon_fire`) -> взрыв (`*_detonate` / `inferno_startburn`),
/// затем урон (`player_hurt`), ослепления (`player_blind`) и убийства — только по противникам.
fn grenades(
    m: &Match,
    events: &[GameEvent],
    live_from: i32,
    resolve: &dyn Fn(&GameEvent, &str) -> Option<PlayerId>,
    team_at: &dyn Fn(PlayerId, i32) -> Option<u8>,
) -> Vec<Grenade> {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr) as i32;
    let enemies = |a: PlayerId, v: PlayerId, tick: i32| {
        a != v && !matches!((team_at(a, tick), team_at(v, tick)), (Some(x), Some(y)) if x == y)
    };
    let round_at = |tick: i32| m.rounds.iter().rev().find(|r| r.start_tick <= tick).map_or(0, |r| r.number);
    let pos = |e: &GameEvent| [e.float("x").unwrap_or(0.0), e.float("y").unwrap_or(0.0), e.float("z").unwrap_or(0.0)];
    let live = || events.iter().filter(|e| e.tick >= live_from);

    // (бросивший, вид, тик) — ещё не взорвавшиеся броски
    let mut throws: Vec<(PlayerId, GrenadeKind, i32)> = Vec::new();
    let mut out: Vec<(i64, Grenade)> = Vec::new(); // (entityid, граната)
    let mut add = |kind, thrower: PlayerId, entity: i64, tick: i32, p: [f32; 3], throws: &mut Vec<(PlayerId, GrenadeKind, i32)>| {
        // самый ранний подходящий бросок этого игрока (огонь — любого: у inferno_startburn нет userid)
        let i = throws.iter().position(|&(who, k, t)| {
            k == kind && (kind == GrenadeKind::Fire || who == thrower) && t <= tick && tick - t <= secs(8.0)
        });
        let (thrower, throw_tick) = match i.map(|i| throws.remove(i)) {
            Some((who, _, t)) => (who, Some(t)),
            None => (thrower, None),
        };
        let g = Grenade { kind, thrower, round: round_at(tick), throw_tick, detonate_tick: tick, pos: p, damage: 0, hurt: vec![], blinded: vec![], kills: vec![] };
        out.push((entity, g));
    };
    for e in live() {
        let kind = match e.name.as_str() {
            "weapon_fire" => {
                let kind = match e.str("weapon").unwrap_or_default().trim_start_matches("weapon_") {
                    "hegrenade" => GrenadeKind::He,
                    "flashbang" => GrenadeKind::Flash,
                    "molotov" | "incgrenade" => GrenadeKind::Fire,
                    "smokegrenade" => GrenadeKind::Smoke,
                    _ => continue,
                };
                if let Some(p) = resolve(e, "userid") {
                    throws.retain(|&(_, _, t)| e.tick - t <= secs(8.0));
                    throws.push((p, kind, e.tick));
                }
                continue;
            }
            "hegrenade_detonate" => GrenadeKind::He,
            "flashbang_detonate" => GrenadeKind::Flash,
            "smokegrenade_detonate" => GrenadeKind::Smoke,
            "inferno_startburn" => GrenadeKind::Fire,
            _ => continue,
        };
        let thrower = resolve(e, "userid").unwrap_or(PlayerId(-1));
        add(kind, thrower, e.int("entityid").unwrap_or(-1), e.tick, pos(e), &mut throws);
    }
    out.retain(|(_, g)| g.thrower.0 >= 0);

    for e in live() {
        match e.name.as_str() {
            "player_hurt" => {
                let kind = match e.str("weapon").unwrap_or_default() {
                    "hegrenade" => GrenadeKind::He,
                    "inferno" => GrenadeKind::Fire,
                    _ => continue,
                };
                let (Some(a), Some(v)) = (resolve(e, "attacker"), resolve(e, "userid")) else { continue };
                if !enemies(a, v, e.tick) {
                    continue;
                }
                // HE бьёт в тик взрыва, огонь горит ~7 с
                let window = if kind == GrenadeKind::He { secs(0.2) } else { secs(8.0) };
                if let Some((_, g)) = out
                    .iter_mut()
                    .rev()
                    .find(|(_, g)| g.kind == kind && g.thrower == a && g.detonate_tick <= e.tick + 1 && e.tick - g.detonate_tick <= window)
                {
                    g.damage += e.int("dmg_health").unwrap_or(0) as i32;
                    if !g.hurt.contains(&v) {
                        g.hurt.push(v);
                    }
                }
            }
            "player_blind" => {
                let (Some(a), Some(v)) = (resolve(e, "attacker"), resolve(e, "userid")) else { continue };
                let ent = e.int("entityid").unwrap_or(-2);
                let dur = e.float("blind_duration").unwrap_or(0.0);
                if !enemies(a, v, e.tick) || dur < 0.5 {
                    continue;
                }
                if let Some((_, g)) = out.iter_mut().find(|(id, g)| *id == ent && g.kind == GrenadeKind::Flash && (e.tick - g.detonate_tick).abs() <= secs(0.5)) {
                    g.blinded.push((v, dur));
                }
            }
            _ => {}
        }
    }

    let mut out: Vec<Grenade> = out.into_iter().map(|(_, g)| g).collect();
    for g in &mut out {
        let det = g.detonate_tick;
        g.kills = m
            .kills
            .iter()
            .filter(|k| match g.kind {
                GrenadeKind::He => k.weapon == "hegrenade" && k.attacker == g.thrower && k.tick >= det && k.tick - det <= secs(0.3),
                GrenadeKind::Fire => k.weapon == "inferno" && k.attacker == g.thrower && k.tick >= det && k.tick - det <= secs(8.0),
                // убит, пока был ослеплён этой флешкой
                GrenadeKind::Flash => g.blinded.iter().any(|&(v, d)| v == k.victim && k.tick >= det && k.tick - det <= secs(d + 0.3)),
                GrenadeKind::Smoke => false,
            })
            .map(|k| k.victim)
            .collect();
    }
    out.sort_by_key(|g| g.detonate_tick);
    out
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
    fn empty_player_name_becomes_player_with_userid() {
        let m = Match::build("de_mirage".into(), 64.0, vec![(7, 3, "\n\u{7}\u{202E}".into(), 1)], &[]);
        assert_eq!(m.players[0].name, "игрок 7");
    }

    #[test]
    fn build_cleans_hostile_name_and_map() {
        let m = Match::build("x;exec evil".into(), 64.0, vec![(1, 1, "a\nquit".into(), 1)], &[]);
        assert_eq!(m.map, "xexecevil");
        assert_eq!(m.players[0].name, "aquit");
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

    /// FACEIT: у `round_officially_ended` нет winner — победитель по бомбе, по «всех выбили», иначе CT.
    #[test]
    fn winners_without_winner_field() {
        let mut events: Vec<GameEvent> = (0..4).map(|p| ev(0, "player_team", &[("userid", p), ("team", if p < 2 { 2 } else { 3 })])).collect();
        events.extend([
            ev(10, "round_prestart", &[]),
            ev(50, "bomb_exploded", &[]),
            ev(60, "round_officially_ended", &[]),
            ev(70, "round_prestart", &[]),
            death(80, 2, 0),
            death(90, 3, 1),
            ev(100, "round_officially_ended", &[]),
            ev(110, "round_prestart", &[]),
            ev(150, "round_officially_ended", &[]),
            ev(160, "round_prestart", &[]),
        ]);
        let m = Match::build("de_nuke".into(), 64.0, players(), &events);
        let w: Vec<_> = m.rounds.iter().map(|r| r.winner_team).collect();
        assert_eq!(w, vec![Some(2), Some(2), Some(3), None]);
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
