//! Позиции, взгляд, команды и «живость» игроков по тикам — из состояния сущностей демки.
//!
//! Разбор сущностей (string tables, serializers, packet entities) делает крейт `source2-demo`
//! (MIT/Apache-2.0); здесь он спрятан за `Track`, который реализует `director::PositionSource`.
//! Контейнер, события и модель матча остаются на собственных крейтах проекта.

use std::{collections::BTreeMap, error::Error};

use cs2::PlayerId;
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

#[derive(Debug, Default, Clone)]
pub struct Track {
    samples: BTreeMap<PlayerId, Vec<Sample>>,
}

struct Sampler {
    step: i32,
    next: i32,
    out: BTreeMap<PlayerId, Vec<Sample>>,
}

impl Sampler {
    fn new(step: i32) -> Self {
        Self { step, next: i32::MIN, out: BTreeMap::new() }
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

fn read_pawn(pawn: &Entity) -> Option<([f32; 3], f32, [f32; 3])> {
    let axis = |c: &str, v: &str| Some(cell(pawn, &format!("{ORIGIN}.m_cell{c}"))? * CELL - CELL_ORIGIN + f(pawn, &format!("{ORIGIN}.m_vec{v}"))?);
    let pos = [axis("X", "X")?, axis("Y", "Y")?, axis("Z", "Z")?];
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
        if tick < self.next {
            return Ok(());
        }
        self.next = tick + self.step;

        let entities = ctx.entities();
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
    let samples = std::mem::take(&mut sampler.borrow_mut().out);
    Ok(Track { samples })
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
        Self { samples }
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
}
