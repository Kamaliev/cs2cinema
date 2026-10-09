//! Разбор игры одного игрока по демке: дуэли, гранаты, раунды — то, чего нет в статистике FACEIT.

use crate::{GrenadeKind, Kill, Match, PlayerId};

/// За сколько секунд после смерти тиммейта ответное убийство считается трейдом.
const TRADE_SECS: f32 = 5.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Duels {
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub headshots: u32,
    /// Первое убийство раунда — моё.
    pub opening_kills: u32,
    /// Первая смерть раунда — моя.
    pub opening_deaths: u32,
    /// Я убил того, кто только что убил тиммейта.
    pub trade_kills: u32,
    /// Моего убийцу быстро убил тиммейт.
    pub traded_deaths: u32,
    /// Убийства по оружию, больше сверху.
    pub weapons: Vec<(String, u32)>,
}

impl Duels {
    pub fn hs_pct(&self) -> f32 {
        100.0 * self.headshots as f32 / self.kills.max(1) as f32
    }
    pub fn kd(&self) -> f32 {
        self.kills as f32 / self.deaths.max(1) as f32
    }
}

/// Гранаты одного вида.
#[derive(Debug, Clone, PartialEq)]
pub struct Nades {
    pub kind: GrenadeKind,
    pub thrown: u32,
    /// Флешки: ослеплено противников (суммарно по всем броскам).
    pub blinded: u32,
    pub blind_secs: f32,
    /// Урон противникам (HE, огонь).
    pub damage: i32,
    /// Убито гранатой, а для флешки — убито командой, пока противник был ослеплён.
    pub kills: u32,
}

impl Nades {
    pub fn avg_blind_secs(&self) -> f32 {
        self.blind_secs / self.blinded.max(1) as f32
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoundRow {
    pub number: u32,
    pub kills: u32,
    pub died: bool,
    pub nades: u32,
    pub nade_damage: i32,
    pub blinded: u32,
    /// `None` — команда игрока или победитель неизвестны.
    pub won: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub duels: Duels,
    /// HE, флешки, огонь, дым — всегда в этом порядке.
    pub nades: Vec<Nades>,
    pub rounds: Vec<RoundRow>,
}

pub fn player_report(m: &Match, me: PlayerId) -> Report {
    let trade = (TRADE_SECS * m.tick_rate) as i32;
    // противники: по командам из демки, а если их нет — те, с кем были убийства в любую сторону
    let enemy_of_me: Vec<PlayerId> =
        m.kills.iter().filter_map(|k| if k.attacker == me { Some(k.victim) } else if k.victim == me { Some(k.attacker) } else { None }).collect();
    let mate = |p: PlayerId, tick: i32| {
        p != me
            && match (m.team_at(me, tick), m.team_at(p, tick)) {
                (Some(a), Some(b)) => a == b,
                _ => !enemy_of_me.contains(&p),
            }
    };

    let mut d = Duels::default();
    for k in &m.kills {
        if k.attacker == me {
            d.kills += 1;
            d.headshots += u32::from(k.headshot);
            match d.weapons.iter_mut().find(|(w, _)| *w == k.weapon) {
                Some((_, n)) => *n += 1,
                None => d.weapons.push((k.weapon.clone(), 1)),
            }
            // жертва за последние секунды убила моего тиммейта
            let avenged = m.kills.iter().any(|x| x.attacker == k.victim && mate(x.victim, x.tick) && x.tick <= k.tick && k.tick - x.tick <= trade);
            d.trade_kills += u32::from(avenged);
        }
        if k.victim == me {
            d.deaths += 1;
            let traded = m.kills.iter().any(|x| x.victim == k.attacker && mate(x.attacker, x.tick) && x.tick >= k.tick && x.tick - k.tick <= trade);
            d.traded_deaths += u32::from(traded);
        }
        d.assists += u32::from(k.assister == Some(me));
    }
    d.weapons.sort_by_key(|(_, n)| std::cmp::Reverse(*n));

    let mut rounds: Vec<u32> = m.rounds.iter().map(|r| r.number).chain(m.kills.iter().map(|k| k.round)).collect();
    rounds.sort();
    rounds.dedup();
    let first_kill = |r: u32| -> Option<&Kill> { m.kills.iter().filter(|k| k.round == r).min_by_key(|k| k.tick) };
    for &r in &rounds {
        if let Some(k) = first_kill(r) {
            d.opening_kills += u32::from(k.attacker == me);
            d.opening_deaths += u32::from(k.victim == me);
        }
    }

    let mine: Vec<_> = m.grenades.iter().filter(|g| g.thrower == me).collect();
    let nades = [GrenadeKind::He, GrenadeKind::Flash, GrenadeKind::Fire, GrenadeKind::Smoke]
        .into_iter()
        .map(|kind| {
            let gs: Vec<_> = mine.iter().filter(|g| g.kind == kind).collect();
            Nades {
                kind,
                thrown: gs.len() as u32,
                blinded: gs.iter().map(|g| g.blinded.len() as u32).sum(),
                blind_secs: gs.iter().flat_map(|g| g.blinded.iter().map(|b| b.1)).sum(),
                damage: gs.iter().map(|g| g.damage).sum(),
                kills: gs.iter().map(|g| g.kills.len() as u32).sum(),
            }
        })
        .collect();

    let rows = rounds
        .iter()
        .map(|&number| {
            let gs: Vec<_> = mine.iter().filter(|g| g.round == number).collect();
            let round = m.round(number);
            let won = round.and_then(|r| {
                // команда — на старте раунда: к концу раунда перед сменой сторон она уже другая
                let team = m.team_at(me, r.start_tick)?;
                Some(r.winner_team? == team)
            });
            RoundRow {
                number,
                kills: m.kills.iter().filter(|k| k.round == number && k.attacker == me).count() as u32,
                died: m.kills.iter().any(|k| k.round == number && k.victim == me),
                nades: gs.len() as u32,
                nade_damage: gs.iter().map(|g| g.damage).sum(),
                blinded: gs.iter().map(|g| g.blinded.len() as u32).sum(),
                won,
            }
        })
        .collect();

    Report { duels: d, nades, rounds: rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Grenade, Round};

    fn kill(tick: i32, round: u32, attacker: i32, victim: i32) -> Kill {
        Kill {
            tick,
            round,
            attacker: PlayerId(attacker),
            victim: PlayerId(victim),
            assister: None,
            weapon: "ak47".into(),
            headshot: tick % 2 == 0,
            wallbang: false,
            through_smoke: false,
            no_scope: false,
            attacker_blind: false,
            attacker_in_air: false,
            distance: 10.0,
        }
    }

    /// Я — 1, тиммейт — 2 (команда 3); противники 10, 11 (команда 2).
    fn sample() -> Match {
        let mut m = Match::build("de_test".into(), 64.0, vec![], &[]);
        for (p, t) in [(1, 3), (2, 3), (10, 2), (11, 2)] {
            m.teams.insert(PlayerId(p), vec![(0, t)]);
        }
        m.rounds = vec![
            Round { number: 1, start_tick: 0, freeze_end_tick: None, end_tick: Some(5000), winner_team: Some(3) },
            Round { number: 2, start_tick: 6000, freeze_end_tick: None, end_tick: Some(9000), winner_team: Some(2) },
        ];
        m.kills = vec![
            kill(1000, 1, 10, 2), // противник открывает раунд, убив тиммейта
            kill(1100, 1, 1, 10), // я трейжу через ~1.5 с
            kill(7000, 2, 1, 11), // открывающее убийство моё
            kill(7500, 2, 10, 1), // меня убили…
            kill(7600, 2, 2, 10), // …и сразу отомстили
        ];
        m.grenades = vec![Grenade {
            kind: GrenadeKind::Flash,
            thrower: PlayerId(1),
            round: 2,
            throw_tick: Some(6900),
            detonate_tick: 6950,
            pos: [0.0; 3],
            damage: 0,
            hurt: vec![],
            blinded: vec![(PlayerId(10), 2.0), (PlayerId(11), 3.0)],
            kills: vec![PlayerId(11)],
        }];
        m
    }

    #[test]
    fn duels_openings_and_trades() {
        let r = player_report(&sample(), PlayerId(1));
        let d = &r.duels;
        assert_eq!((d.kills, d.deaths), (2, 1));
        assert_eq!((d.opening_kills, d.opening_deaths), (1, 0));
        assert_eq!((d.trade_kills, d.traded_deaths), (1, 1));
        assert_eq!(d.weapons, vec![("ak47".to_owned(), 2)]);
    }

    #[test]
    fn flashes_and_rounds() {
        let r = player_report(&sample(), PlayerId(1));
        let flash = r.nades.iter().find(|n| n.kind == GrenadeKind::Flash).unwrap();
        assert_eq!((flash.thrown, flash.blinded, flash.kills), (1, 2, 1));
        assert!((flash.avg_blind_secs() - 2.5).abs() < 1e-4);
        assert_eq!(r.rounds.len(), 2);
        assert_eq!((r.rounds[0].kills, r.rounds[0].died, r.rounds[0].won), (1, false, Some(true)));
        assert_eq!((r.rounds[1].died, r.rounds[1].nades, r.rounds[1].won), (true, 1, Some(false)));
    }
}
