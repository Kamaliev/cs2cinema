//! Позиции, взгляд, команды и «живость» игроков по тикам — из состояния сущностей демки.
//!
//! Разбор сущностей (string tables, serializers, packet entities) делает крейт `source2-demo`
//! (MIT/Apache-2.0); здесь он спрятан за `Track`, который реализует `director::PositionSource`.
//! Контейнер, события и модель матча остаются на собственных крейтах проекта.

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
};

use cs2::{GrenadeKind, PlayerId};
use director::{Pose, PositionSource};
use source2_demo::prelude::*;

/// Масштаб ячейки координат Source 2: `world = cell * 512 - 16384 + offset`.
const CELL: f32 = 512.0;
const CELL_ORIGIN: f32 = 16384.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub tick: i32,
    /// Позиция ног.
    pub pos: [f32; 3],
    /// Высота глаз над позицией (приседание меняет).
    pub eye_height: f32,
    /// pitch, yaw, roll в градусах.
    pub ang: [f32; 3],
    pub alive: bool,
    pub team: u8,
}

/// Полёт снаряда гранаты: позиции от появления сущности до её исчезновения.
#[derive(Debug, Clone, PartialEq)]
pub struct Flight {
    pub kind: GrenadeKind,
    pub thrower: Option<PlayerId>,
    /// `(тик, позиция)` по возрастанию тиков.
    pub samples: Vec<(i32, [f32; 3])>,
}

#[derive(Debug, Default, Clone)]
pub struct Track {
    samples: BTreeMap<PlayerId, Vec<Sample>>,
    flights: Vec<Flight>,
}

/// Снаряды летят быстро — снимаем их чаще игроков.
const FLIGHT_STEP: i32 = 2;

struct Sampler {
    step: i32,
    next: i32,
    out: BTreeMap<PlayerId, Vec<Sample>>,
    /// Пешка (handle) -> игрок: из контроллеров, для `m_hThrower` снарядов.
    pawns: HashMap<u32, PlayerId>,
    flights: Vec<Flight>,
    /// Летящие сейчас: handle сущности -> индекс в `flights`.
    live: HashMap<u32, usize>,
}

impl Sampler {
    fn new(step: i32) -> Self {
        Self { step, next: i32::MIN, out: BTreeMap::new(), pawns: HashMap::new(), flights: Vec::new(), live: HashMap::new() }
    }

    fn sample_flights(&mut self, tick: i32, entities: &source2_demo::Entities) {
        let mut seen = Vec::new();
        for e in entities.iter() {
            let kind = match e.class().name() {
                "CFlashbangProjectile" => GrenadeKind::Flash,
                "CHEGrenadeProjectile" => GrenadeKind::He,
                "CMolotovProjectile" => GrenadeKind::Fire,
                "CSmokeGrenadeProjectile" => GrenadeKind::Smoke,
                _ => continue,
            };
            let Some(pos) = origin(e) else { continue };
            let h = e.handle();
            seen.push(h);
            let i = *self.live.entry(h).or_insert_with(|| {
                let thrower = handle(e, "m_hThrower").and_then(|t| self.pawns.get(&t).copied());
                self.flights.push(Flight { kind, thrower, samples: Vec::new() });
                self.flights.len() - 1
            });
            self.flights[i].samples.push((tick, pos));
        }
        // исчезнувшие сущности — полёт окончен (handle с тем же индексом будет уже другим снарядом)
        self.live.retain(|h, _| seen.contains(h));
    }
}

fn f(e: &Entity, name: &str) -> Option<f32> {
    match e.get_property(name).ok()? {
        FieldValue::Float(v) => Some(*v),
        _ => None,
    }
}

fn cell(e: &Entity, name: &str) -> Option<f32> {
    match e.get_property(name).ok()? {
        FieldValue::Unsigned16(v) => Some(*v as f32),
        FieldValue::Unsigned32(v) => Some(*v as f32),
        FieldValue::Unsigned8(v) => Some(*v as f32),
        _ => None,
    }
}

fn handle(e: &Entity, name: &str) -> Option<u32> {
    match e.get_property(name).ok()? {
        FieldValue::Unsigned32(v) => Some(*v),
        _ => None,
    }
}

fn wrap180(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// Сырые `m_angEyeAngles` из source2-demo сдвинуты на 180° по pitch и yaw
/// (проверено на убийствах: после сдвига стрелок смотрит на жертву, медианная ошибка < 1°,
/// и совпадает с demoparser2). Невозможный pitch (|p| > 90, бывает у ещё не обновлённых
/// сущностей) заменяем горизонтом.
pub fn eye_angles(raw: [f32; 3]) -> [f32; 3] {
    let pitch = wrap180(raw[0] - 180.0);
    [if pitch.abs() > 90.0 { 0.0 } else { pitch }, wrap180(raw[1] - 180.0), 0.0]
}

const ORIGIN: &str = "CBodyComponent.m_skeletonInstance.m_vecOrigin";

/// Позиция сущности (пешки или снаряда — у них один и тот же путь свойства).
fn origin(e: &Entity) -> Option<[f32; 3]> {
    let axis = |c: &str| Some(cell(e, &format!("{ORIGIN}.m_cell{c}"))? * CELL - CELL_ORIGIN + f(e, &format!("{ORIGIN}.m_vec{c}"))?);
    Some([axis("X")?, axis("Y")?, axis("Z")?])
}

fn read_pawn(pawn: &Entity) -> Option<([f32; 3], f32, [f32; 3])> {
    let pos = origin(pawn)?;
    let eye_height = f(pawn, "m_vecViewOffset.m_vecZ").unwrap_or(64.0);
    let raw = match pawn.get_property("m_angEyeAngles").ok()? {
        FieldValue::Vector3D(v) => *v,
        _ => return None,
    };
    Some((pos, eye_height, eye_angles(raw)))
}

impl Observer for Sampler {
    fn interests(&self) -> Interests {
        Interests::TICK_END | Interests::ENTITY_STATE | Interests::ENTITY_EVENTS
    }

    fn on_tick_end(&mut self, ctx: &Context) -> ObserverResult {
        let tick = ctx.tick() as i32;
        let entities = ctx.entities();
        if tick % FLIGHT_STEP == 0 {
            self.sample_flights(tick, entities);
        }
        if tick < self.next {
            return Ok(());
        }
        self.next = tick + self.step;

        for controller in entities.iter().filter(|e| e.class().name() == "CCSPlayerController") {
            // в CS2 userid события = индекс контроллера - 1
            let id = PlayerId(controller.index() as i32 - 1);
            let team = match controller.get_property("m_iTeamNum") {
                Ok(FieldValue::Unsigned8(t)) => *t,
                _ => 0,
            };
            let alive = matches!(controller.get_property("m_bPawnAlive"), Ok(FieldValue::Boolean(true)))
                || matches!(controller.get_property("m_bPawnIsAlive"), Ok(FieldValue::Boolean(true)));
            let Some(h) = handle(controller, "m_hPlayerPawn") else { continue };
            self.pawns.insert(h, id);
            let Ok(pawn) = entities.get_by_handle(h as usize) else { continue };
            let Some((pos, eye_height, ang)) = read_pawn(pawn) else { continue };
            self.out.entry(id).or_default().push(Sample { tick, pos, eye_height, ang, alive, team });
        }
        Ok(())
    }
}

/// Проходит демку и снимает позиции каждые `step` тиков.
pub fn track(demo: &[u8], step: i32) -> Result<Track, Box<dyn Error>> {
    let mut parser = Parser::from_slice(demo)?;
    let sampler = parser.add_observer(Sampler::new(step.max(1)));
    parser.run_to_end()?;
    let mut s = sampler.borrow_mut();
    Ok(Track { samples: std::mem::take(&mut s.out), flights: std::mem::take(&mut s.flights) })
}

fn lerp_angle(a: f32, b: f32, u: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    a + d * u
}

impl Track {
    pub fn from_samples(samples: BTreeMap<PlayerId, Vec<Sample>>) -> Self {
        Self { samples, flights: Vec::new() }
    }

    pub fn flights(&self) -> &[Flight] {
        &self.flights
    }

    /// Полёт снаряда гранаты `g`: тот же вид и бросающий, появился около броска,
    /// к взрыву оказался у точки взрыва. Путь обрезан по взрыв (последняя точка — на тике взрыва).
    pub fn grenade_flight(&self, g: &cs2::Grenade) -> Option<Vec<(i32, [f32; 3])>> {
        // тики CS2: 64 в секунду
        let det = g.detonate_tick;
        let at = |s: &[(i32, [f32; 3])], t: i32| -> [f32; 3] {
            let i = s.partition_point(|p| p.0 <= t);
            match (i.checked_sub(1).map(|i| s[i]), s.get(i)) {
                (Some(a), Some(b)) => {
                    let u = (t - a.0) as f32 / (b.0 - a.0).max(1) as f32;
                    [a.1[0] + (b.1[0] - a.1[0]) * u, a.1[1] + (b.1[1] - a.1[1]) * u, a.1[2] + (b.1[2] - a.1[2]) * u]
                }
                (Some(a), None) => a.1,
                (None, Some(b)) => b.1,
                (None, None) => [0.0; 3],
            }
        };
        let off = |p: [f32; 3]| ((p[0] - g.pos[0]).powi(2) + (p[1] - g.pos[1]).powi(2) + (p[2] - g.pos[2]).powi(2)).sqrt();
        self.flights
            .iter()
            .filter(|f| f.kind == g.kind && f.thrower.is_none_or(|t| t == g.thrower))
            .filter(|f| {
                let (Some(first), Some(last)) = (f.samples.first(), f.samples.last()) else { return false };
                // появился около броска (или хотя бы до взрыва), дожил почти до взрыва
                let spawned = match g.throw_tick {
                    Some(t) => (t - 13..=t + 32).contains(&first.0),
                    None => first.0 < det && first.0 >= det - 8 * 64,
                };
                spawned && last.0 >= det - 8
            })
            .map(|f| (off(at(&f.samples, det)), f))
            .filter(|(d, _)| *d <= 150.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, f)| {
                let mut path: Vec<(i32, [f32; 3])> = f.samples.iter().copied().filter(|p| p.0 < det).collect();
                path.push((det, at(&f.samples, det)));
                path
            })
    }

    pub fn players(&self) -> impl Iterator<Item = PlayerId> + '_ {
        self.samples.keys().copied()
    }

    pub fn samples(&self, player: PlayerId) -> &[Sample] {
        self.samples.get(&player).map_or(&[], Vec::as_slice)
    }

    /// Состояние игрока в тик: интерполяция между соседними живыми замерами.
    pub fn sample_at(&self, player: PlayerId, tick: i32) -> Option<Sample> {
        let s = self.samples.get(&player)?;
        let i = s.partition_point(|x| x.tick <= tick);
        let prev = i.checked_sub(1).map(|i| s[i]);
        let next = s.get(i).copied();
        match (prev, next) {
            (Some(a), Some(b)) if a.alive && b.alive && b.tick > a.tick => {
                let u = (tick - a.tick) as f32 / (b.tick - a.tick) as f32;
                let mix = |p: usize| a.pos[p] + (b.pos[p] - a.pos[p]) * u;
                Some(Sample {
                    tick,
                    pos: [mix(0), mix(1), mix(2)],
                    eye_height: a.eye_height + (b.eye_height - a.eye_height) * u,
                    ang: [lerp_angle(a.ang[0], b.ang[0], u), lerp_angle(a.ang[1], b.ang[1], u), 0.0],
                    alive: true,
                    team: a.team,
                })
            }
            (Some(a), _) => Some(Sample { tick, ..a }),
            (None, Some(b)) => Some(Sample { tick, ..b }),
            (None, None) => None,
        }
    }

    pub fn team_at(&self, player: PlayerId, tick: i32) -> Option<u8> {
        self.sample_at(player, tick).map(|s| s.team).filter(|t| *t >= 2)
    }

    pub fn is_alive(&self, player: PlayerId, tick: i32) -> bool {
        self.sample_at(player, tick).is_some_and(|s| s.alive)
    }
}

impl PositionSource for Track {
    fn eye_pose(&self, player: PlayerId, tick: i32) -> Option<Pose> {
        let s = self.sample_at(player, tick)?;
        Some(Pose { pos: [s.pos[0], s.pos[1], s.pos[2] + s.eye_height], ang: s.ang })
    }

    fn players(&self, tick: i32) -> Vec<(PlayerId, Pose)> {
        self.samples
            .keys()
            .filter(|p| self.is_alive(**p, tick))
            .filter_map(|p| Some((*p, self.eye_pose(*p, tick)?)))
            .collect()
    }

    fn grenade_path(&self, g: &cs2::Grenade) -> Option<Vec<(i32, [f32; 3])>> {
        self.grenade_flight(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_angles_are_unshifted() {
        assert_eq!(eye_angles([166.5, 358.5, 0.0]), [-13.5, 178.5, 0.0]);
        // ещё не обновлённый pitch = 0 -> невозможное -180 -> горизонт
        assert_eq!(eye_angles([0.0, 341.0, 0.0]), [0.0, 161.0, 0.0]);
        assert_eq!(eye_angles([193.4, 175.7, 0.0])[0].round(), 13.0);
    }
}

/// Позиции игроков + геометрия карты: источник для режиссёра, который знает про стены.
pub struct WithGeometry<'a> {
    pub track: &'a Track,
    pub mesh: &'a geometry::Mesh,
}

impl PositionSource for WithGeometry<'_> {
    fn eye_pose(&self, player: PlayerId, tick: i32) -> Option<Pose> {
        self.track.eye_pose(player, tick)
    }

    fn players(&self, tick: i32) -> Vec<(PlayerId, Pose)> {
        PositionSource::players(self.track, tick)
    }

    fn raycast(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        self.mesh.raycast(from, to)
    }

    fn grenade_path(&self, g: &cs2::Grenade) -> Option<Vec<(i32, [f32; 3])>> {
        self.track.grenade_flight(g)
    }
}
