//! Поиск хайлайтов в матче и их оценка (score).
//!
//! Score нужен режиссёру: чем он выше, тем эффектнее подача
//! (замедление, пролёты камеры) и тем больше шанс попасть в ролик.

use cs2::{Grenade, GrenadeKind, Kill, Match, PlayerId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Одиночное убийство, но с эффектными условиями.
    Showpiece,
    /// Серия из N убийств за раунд (N >= 2); 5 — эйс.
    MultiKill(u8),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Highlight {
    pub kind: Kind,
    pub round: u32,
    pub player: PlayerId,
    pub kills: Vec<Kill>,
    pub score: f32,
    /// Человекочитаемые причины оценки: `["3k", "wallbang", ...]`.
    pub tags: Vec<String>,
}

impl Highlight {
    pub fn first_tick(&self) -> i32 {
        self.kills.first().map_or(0, |k| k.tick)
    }
    pub fn last_tick(&self) -> i32 {
        self.kills.last().map_or(0, |k| k.tick)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Максимальная пауза между убийствами внутри одной серии, секунды.
    pub chain_gap_secs: f32,
    /// Одиночные убийства с score ниже порога не считаются хайлайтами.
    pub min_single_score: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { chain_gap_secs: 8.0, min_single_score: 14.0 }
    }
}

/// Оценка одного убийства и его теги.
pub fn score_kill(k: &Kill) -> (f32, Vec<&'static str>) {
    let mut score = 10.0;
    let mut tags = Vec::new();
    let mut bonus = |points: f32, tag: &'static str, on: bool| {
        if on {
            score += points;
            tags.push(tag);
        }
    };
    bonus(3.0, "headshot", k.headshot);
    bonus(4.0, "wallbang", k.wallbang);
    bonus(6.0, "through smoke", k.through_smoke);
    bonus(5.0, "blind", k.attacker_blind);
    bonus(3.0, "airborne", k.attacker_in_air);
    bonus(8.0, "no-scope", k.no_scope);
    bonus(3.0, "long range", k.distance >= 40.0);
    bonus(10.0, "knife", k.weapon.contains("knife") || k.weapon.contains("bayonet"));
    bonus(10.0, "zeus", k.weapon == "taser");
    bonus(4.0, "grenade", matches!(k.weapon.as_str(), "hegrenade" | "inferno" | "molotov" | "incgrenade"));
    (score, tags)
}

pub fn find(m: &Match, cfg: &Config) -> Vec<Highlight> {
    let gap = (cfg.chain_gap_secs * m.tick_rate) as i32;
    let mut out = Vec::new();

    let mut players: Vec<PlayerId> = m.kills.iter().map(|k| k.attacker).collect();
    players.sort();
    players.dedup();
    let mut rounds: Vec<u32> = m.kills.iter().map(|k| k.round).collect();
    rounds.sort();
    rounds.dedup();

    for &round in &rounds {
        for &player in &players {
            let mut kills: Vec<&Kill> = m
                .kills
                .iter()
                .filter(|k| k.round == round && k.attacker == player)
                .collect();
            kills.sort_by_key(|k| k.tick);

            let mut chain: Vec<&Kill> = Vec::new();
            for k in kills {
                if chain.last().is_some_and(|last| k.tick - last.tick > gap) {
                    out.extend(make(round, player, std::mem::take(&mut chain), cfg));
                }
                chain.push(k);
            }
            out.extend(make(round, player, chain, cfg));
        }
    }

    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// Граната, которая что-то решила: урон, ослепление, убийство.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utility {
    pub grenade: Grenade,
    pub score: f32,
    pub tags: Vec<String>,
}

/// Оценка гранаты; `None` — ничего важного не сделала.
pub fn score_grenade(g: &Grenade) -> Option<Utility> {
    let kills = g.kills.len() as f32;
    let (score, tags, important) = match g.kind {
        GrenadeKind::He => {
            let s = 8.0 + g.damage as f32 / 4.0 + 15.0 * kills;
            (s, vec![format!("HE {} урона", g.damage)], g.damage >= 40 || kills > 0.0)
        }
        GrenadeKind::Fire => {
            let s = 8.0 + g.damage as f32 / 5.0 + 15.0 * kills;
            (s, vec![format!("молотов {} урона", g.damage)], g.damage >= 30 || kills > 0.0)
        }
        GrenadeKind::Flash => {
            let n = g.blinded.len();
            let longest = g.blinded.iter().map(|b| b.1).fold(0.0, f32::max);
            let s = 6.0 + 7.0 * n as f32 + 2.0 * longest + 12.0 * kills;
            (s, vec![format!("флешка: ослепила {n}")], n >= 2 || kills > 0.0)
        }
        GrenadeKind::Smoke => return None,
    };
    let mut tags = tags;
    if kills > 0.0 {
        tags.push(format!("{} убит(о)", g.kills.len()));
    }
    important.then(|| Utility { grenade: g.clone(), score, tags })
}

/// Важные гранаты игрока по порядку матча.
pub fn utility_for(m: &Match, player: PlayerId) -> Vec<Utility> {
    m.grenades.iter().filter(|g| g.thrower == player).filter_map(score_grenade).collect()
}

/// Все убийства одного игрока — каждое попадает в какой-нибудь момент (серии склеиваются),
/// по порядку матча.
pub fn for_player(m: &Match, player: PlayerId, cfg: &Config) -> Vec<Highlight> {
    let all = Config { min_single_score: f32::NEG_INFINITY, ..cfg.clone() };
    let mut out: Vec<Highlight> = find(m, &all).into_iter().filter(|h| h.player == player).collect();
    out.sort_by_key(|h| h.first_tick());
    out
}

fn make(round: u32, player: PlayerId, chain: Vec<&Kill>, cfg: &Config) -> Option<Highlight> {
    if chain.is_empty() {
        return None;
    }
    let n = chain.len();
    let mut total = 0.0;
    let mut tags: Vec<String> = Vec::new();
    for k in &chain {
        let (s, t) = score_kill(k);
        total += s;
        for tag in t {
            if !tags.iter().any(|x| x == tag) {
                tags.push(tag.to_owned());
            }
        }
    }

    if n == 1 {
        if total < cfg.min_single_score {
            return None;
        }
        return Some(Highlight { kind: Kind::Showpiece, round, player, kills: vec![chain[0].clone()], score: total, tags });
    }

    // серии растут быстрее суммы: каждое следующее убийство ценнее предыдущего
    let multiplier = 1.0 + 0.6 * (n as f32 - 1.0);
    let ace_bonus = if n >= 5 { 60.0 } else { 0.0 };
    tags.insert(0, format!("{n}k"));
    Some(Highlight {
        kind: Kind::MultiKill(n.min(255) as u8),
        round,
        player,
        kills: chain.into_iter().cloned().collect(),
        score: total * multiplier + ace_bonus,
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kill(tick: i32, round: u32, attacker: i32, victim: i32) -> Kill {
        Kill {
            tick,
            round,
            attacker: PlayerId(attacker),
            victim: PlayerId(victim),
            assister: None,
            weapon: "ak47".into(),
            headshot: false,
            wallbang: false,
            through_smoke: false,
            no_scope: false,
            attacker_blind: false,
            attacker_in_air: false,
            assisted_flash: false,
            distance: 10.0,
        }
    }

    fn match_with(kills: Vec<Kill>) -> Match {
        // Match собирается через публичный build; проще десериализовать минимальный JSON-подобный вид через build.
        let mut m = Match::build("de_test".into(), 64.0, vec![], &[]);
        m.kills = kills;
        m
    }

    #[test]
    fn chain_splits_by_gap_and_ace_dominates() {
        let mut kills: Vec<Kill> = (0..5).map(|i| kill(1000 + i * 100, 1, 0, 10 + i)).collect();
        kills.push(kill(5000, 1, 0, 20)); // одиночное убийство позже
        kills.push(kill(1100, 1, 1, 21)); // чужое
        let hl = find(&match_with(kills), &Config::default());
        assert_eq!(hl[0].kind, Kind::MultiKill(5));
        assert!(hl[0].tags.contains(&"5k".to_string()));
        assert!(hl.iter().all(|h| h.kind != Kind::Showpiece), "обычные одиночные не нужны");
    }

    #[test]
    fn fancy_single_kill_is_showpiece() {
        let mut k = kill(100, 3, 2, 7);
        k.weapon = "awp".into();
        k.no_scope = true;
        k.through_smoke = true;
        let hl = find(&match_with(vec![k]), &Config::default());
        assert_eq!(hl.len(), 1);
        assert_eq!(hl[0].kind, Kind::Showpiece);
        assert!(hl[0].tags.contains(&"no-scope".to_string()));
    }

    #[test]
    fn plain_single_kill_is_ignored() {
        assert!(find(&match_with(vec![kill(100, 1, 0, 1)]), &Config::default()).is_empty());
    }
}
