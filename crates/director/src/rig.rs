//! Каталог камер и оценка «красочности» кадра.
//!
//! Пресет — это строка таблицы: вокруг какой точки летит камера (`Anchor`), от чего считается
//! азимут (`Frame`), куда смотрит (`Look`), откуда и куда движется (`from`/`to`), как меняется
//! FOV (зум) и крен. Один и тот же генератор строит ключи для всех ~40 пресетов, а `evaluate`
//! выбирает из них лучший для конкретной сцены.

use cs2::PlayerId;
use serde::{Deserialize, Serialize};

use crate::{CamKey, Pose, PositionSource, Tier, look_at};

pub const INTRO: u8 = 1;
pub const CUT: u8 = 2;
pub const OUTRO: u8 = 4;
pub const ANY: u8 = INTRO | CUT | OUTRO;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Orbit,
    Dolly,
    Crane,
    Angle,
    Reveal,
    Wide,
    Whip,
    Zoom,
}

#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    Killer,
    Victim,
    /// Середина между убийцей и жертвой.
    Mid,
    /// Центр всех игроков в кадре сцены.
    Group,
}

#[derive(Debug, Clone, Copy)]
pub enum Frame {
    /// Азимут 0 = туда, куда смотрит убийца.
    KillerYaw,
    /// Азимут 0 = направление от убийцы к жертве.
    Axis,
}

#[derive(Debug, Clone, Copy)]
pub enum Look {
    Killer,
    Victim,
    Mid,
    Group,
    /// Взгляд перелетает с убийцы на жертву (whip pan).
    SweepKV,
    /// ...и наоборот.
    SweepVK,
}

#[derive(Debug, Clone, Copy)]
pub enum Ease {
    Linear,
    Smooth,
    /// Медленный старт, резкий финал (snap).
    In,
    Out,
}

/// Точка траектории: азимут (°), радиус и высота (юниты) относительно якоря.
#[derive(Debug, Clone, Copy)]
pub struct Pt(pub f32, pub f32, pub f32);

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    pub category: Category,
    /// Насколько «громкая» подача, 1..=5. Сопоставляется с tier хайлайта.
    pub drama: u8,
    /// Где пресет уместен (маски `INTRO | CUT | OUTRO`).
    pub fits: u8,
    pub anchor: Anchor,
    pub frame: Frame,
    pub look: Look,
    pub from: Pt,
    pub to: Pt,
    pub fov: (f32, f32),
    pub roll: (f32, f32),
    pub ease: Ease,
}

#[allow(clippy::too_many_arguments)]
const fn p(
    name: &'static str,
    category: Category,
    drama: u8,
    fits: u8,
    anchor: Anchor,
    frame: Frame,
    look: Look,
    from: Pt,
    to: Pt,
    fov: (f32, f32),
    roll: (f32, f32),
    ease: Ease,
) -> Preset {
    Preset { name, category, drama, fits, anchor, frame, look, from, to, fov, roll, ease }
}

use Anchor::{Group, Killer as K, Mid, Victim as V};
use Category::*;
use Ease::{In, Linear, Out, Smooth};
use Frame::{Axis, KillerYaw as Yaw};
use Look as L;

const NR: (f32, f32) = (0.0, 0.0);

/// 41 камера. Азимуты в градусах, радиусы/высоты в юнитах Source (глаза ≈ 64 над полом).
pub static PRESETS: &[Preset] = &[
    // — орбиты вокруг убийцы —
    p("orbit_cw_slow", Orbit, 2, ANY, K, Yaw, L::Killer, Pt(200., 180., 30.), Pt(280., 150., 40.), (85., 85.), NR, Smooth),
    p("orbit_ccw_slow", Orbit, 2, ANY, K, Yaw, L::Killer, Pt(160., 180., 30.), Pt(80., 150., 40.), (85., 85.), NR, Smooth),
    p("orbit_cw_fast", Orbit, 4, ANY, K, Yaw, L::Killer, Pt(200., 150., 30.), Pt(380., 130., 40.), (85., 90.), NR, Smooth),
    p("orbit_ccw_fast", Orbit, 4, ANY, K, Yaw, L::Killer, Pt(160., 150., 30.), Pt(-20., 130., 40.), (85., 90.), NR, Smooth),
    p("spiral_in", Orbit, 5, INTRO | CUT, K, Yaw, L::Killer, Pt(180., 420., 120.), Pt(340., 90., 20.), (70., 95.), NR, Smooth),
    p("spiral_out", Orbit, 4, CUT | OUTRO, K, Yaw, L::Killer, Pt(90., 70., 10.), Pt(270., 380., 140.), (95., 70.), NR, Smooth),
    p("low_orbit", Orbit, 4, ANY, K, Yaw, L::Killer, Pt(200., 140., -30.), Pt(300., 140., -10.), (90., 90.), NR, Smooth),
    p("high_orbit", Orbit, 3, ANY, K, Yaw, L::Killer, Pt(180., 220., 160.), Pt(270., 220., 150.), (80., 80.), NR, Smooth),
    p("ground_skim", Orbit, 4, ANY, K, Yaw, L::Killer, Pt(200., 200., -40.), Pt(300., 200., -40.), (95., 95.), NR, Linear),
    p("dutch_orbit", Orbit, 4, ANY, K, Yaw, L::Killer, Pt(200., 160., 30.), Pt(300., 140., 40.), (85., 85.), (0., 12.), Smooth),
    // — наезды/отъезды —
    p("push_in_behind", Dolly, 3, ANY, K, Yaw, L::Killer, Pt(180., 300., 40.), Pt(180., 110., 20.), (80., 90.), NR, Smooth),
    p("pull_out_behind", Dolly, 3, ANY, K, Yaw, L::Killer, Pt(180., 80., 10.), Pt(180., 320., 90.), (90., 80.), NR, Smooth),
    p("push_in_front", Dolly, 4, ANY, K, Yaw, L::Killer, Pt(0., 360., 20.), Pt(0., 120., 20.), (80., 90.), NR, Smooth),
    p("pull_out_front", Dolly, 3, CUT | OUTRO, K, Yaw, L::Killer, Pt(0., 100., 20.), Pt(0., 350., 60.), (90., 80.), NR, Smooth),
    p("push_in_left", Dolly, 3, ANY, K, Yaw, L::Killer, Pt(90., 320., 30.), Pt(90., 120., 30.), (80., 90.), NR, Smooth),
    p("push_in_right", Dolly, 3, ANY, K, Yaw, L::Killer, Pt(270., 320., 30.), Pt(270., 120., 30.), (80., 90.), NR, Smooth),
    p("dolly_zoom_in", Dolly, 5, INTRO | OUTRO, K, Yaw, L::Killer, Pt(180., 100., 20.), Pt(180., 400., 40.), (100., 40.), NR, Smooth),
    p("dolly_zoom_out", Dolly, 5, INTRO | OUTRO, K, Yaw, L::Killer, Pt(180., 400., 40.), Pt(180., 100., 20.), (40., 100.), NR, Smooth),
    p("dutch_push", Dolly, 4, ANY, K, Yaw, L::Killer, Pt(0., 340., 20.), Pt(0., 130., 20.), (80., 90.), (10., -8.), Smooth),
    // — краны —
    p("crane_up_behind", Crane, 3, ANY, K, Yaw, L::Killer, Pt(180., 160., 10.), Pt(180., 190., 230.), (85., 80.), NR, Smooth),
    p("crane_down_behind", Crane, 3, ANY, K, Yaw, L::Killer, Pt(180., 190., 260.), Pt(180., 150., 30.), (80., 90.), NR, Smooth),
    p("crane_up_front", Crane, 3, ANY, K, Yaw, L::Killer, Pt(0., 170., 10.), Pt(0., 200., 230.), (85., 80.), NR, Smooth),
    p("top_down", Crane, 4, INTRO | OUTRO, K, Yaw, L::Killer, Pt(180., 20., 420.), Pt(200., 40., 380.), (80., 80.), NR, Smooth),
    // — ракурсы —
    p("hero_low_front", Angle, 4, ANY, K, Yaw, L::Killer, Pt(20., 130., -35.), Pt(-20., 130., -35.), (88., 88.), NR, Smooth),
    p("over_shoulder", Angle, 2, ANY, K, Axis, L::Victim, Pt(180., 70., 25.), Pt(180., 70., 25.), (85., 85.), NR, Linear),
    p("over_shoulder_push", Angle, 3, ANY, K, Axis, L::Victim, Pt(170., 60., 25.), Pt(190., 130., 35.), (85., 90.), NR, Smooth),
    p("kill_axis_wide", Angle, 3, ANY, K, Axis, L::Mid, Pt(180., 220., 70.), Pt(180., 220., 70.), (95., 95.), NR, Linear),
    p("kill_axis_push", Angle, 3, ANY, K, Axis, L::Mid, Pt(180., 320., 90.), Pt(180., 160., 60.), (100., 80.), NR, Smooth),
    // — широкие планы с двумя участниками —
    p("flank_left_wide", Wide, 3, ANY, Mid, Axis, L::Mid, Pt(90., 380., 80.), Pt(90., 380., 80.), (90., 90.), NR, Linear),
    p("flank_right_wide", Wide, 3, ANY, Mid, Axis, L::Mid, Pt(270., 380., 80.), Pt(270., 380., 80.), (90., 90.), NR, Linear),
    p("flank_sweep", Wide, 4, ANY, Mid, Axis, L::Mid, Pt(60., 350., 70.), Pt(120., 300., 90.), (90., 90.), NR, Smooth),
    // — раскрытия через жертву —
    p("victim_reveal", Reveal, 4, ANY, V, Axis, L::Killer, Pt(0., 90., 20.), Pt(0., 200., 30.), (85., 80.), NR, Smooth),
    p("victim_orbit", Reveal, 3, CUT | OUTRO, V, Axis, L::Victim, Pt(20., 150., 30.), Pt(200., 150., 30.), (85., 85.), NR, Smooth),
    // — группа: видно как можно больше людей —
    p("group_crane", Wide, 3, INTRO | OUTRO, Group, Axis, L::Group, Pt(180., 500., 250.), Pt(180., 700., 420.), (90., 90.), NR, Smooth),
    p("group_orbit", Wide, 3, INTRO | OUTRO, Group, Axis, L::Group, Pt(0., 650., 300.), Pt(180., 650., 300.), (90., 90.), NR, Smooth),
    p("group_push", Wide, 3, INTRO | OUTRO, Group, Axis, L::Group, Pt(180., 900., 400.), Pt(180., 400., 200.), (90., 85.), NR, Smooth),
    // — вайпы взглядом —
    p("whip_to_victim", Whip, 5, CUT | OUTRO, K, Axis, L::SweepKV, Pt(180., 120., 40.), Pt(180., 120., 40.), (90., 90.), NR, Linear),
    p("whip_to_killer", Whip, 5, CUT | OUTRO, V, Axis, L::SweepVK, Pt(20., 140., 40.), Pt(20., 140., 40.), (90., 90.), NR, Linear),
    // — зумы —
    p("punch_in", Zoom, 4, ANY, K, Axis, L::Killer, Pt(180., 260., 50.), Pt(180., 260., 50.), (100., 45.), NR, Smooth),
    p("snap_zoom", Zoom, 5, CUT | OUTRO, K, Axis, L::Killer, Pt(180., 300., 40.), Pt(180., 300., 40.), (95., 38.), NR, In),
    p("zoom_out_reveal", Zoom, 3, ANY, K, Axis, L::Mid, Pt(180., 200., 50.), Pt(180., 200., 50.), (40., 95.), NR, Out),
];

/// Что снимаем: убийца, жертва и тик, по которому берётся поза жертвы.
#[derive(Debug, Clone, Copy)]
pub struct Scene {
    pub killer: PlayerId,
    pub victim: PlayerId,
    /// Момент убийства; после него жертва «замирает» на последней позе.
    pub focus_tick: i32,
    pub tick_rate: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Breakdown {
    /// Видны ли убийца и жертва (особенно в момент киллa).
    pub kill_visible: f32,
    /// Сколько ещё людей попадает в кадр.
    pub crowd: f32,
    /// Композиция: герой ближе к центру, комфортная дистанция.
    pub framing: f32,
    pub smooth: f32,
    /// Соответствие громкости подачи уровню хайлайта.
    pub drama_fit: f32,
    pub variety: f32,
    /// Доля кадров, где убийца закрыт стеной (информативно: уже учтено в `kill_visible`).
    #[serde(default)]
    pub occluded: f32,
    /// Камера пролетает сквозь стену — оценка режется.
    #[serde(default)]
    pub through_wall: bool,
    pub total: f32,
}

#[derive(Debug, Clone)]
pub struct Pick {
    pub preset: &'static Preset,
    pub keys: Vec<CamKey>,
    pub score: Breakdown,
}

const KEY_EVERY_SECS: f32 = 0.2;
/// Насколько камера не доезжает до стены, юниты.
const ARM_MARGIN: f32 = 24.0;

fn ease(e: Ease, u: f32) -> f32 {
    match e {
        Ease::Linear => u,
        Ease::Smooth => u * u * (3.0 - 2.0 * u),
        Ease::In => u * u * u,
        Ease::Out => 1.0 - (1.0 - u).powi(3),
    }
}

fn lerp(a: f32, b: f32, e: f32) -> f32 {
    a + (b - a) * e
}

fn lerp3(a: [f32; 3], b: [f32; 3], e: f32) -> [f32; 3] {
    [lerp(a[0], b[0], e), lerp(a[1], b[1], e), lerp(a[2], b[2], e)]
}

fn wrap(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn chest(mut p: [f32; 3]) -> [f32; 3] {
    p[2] -= 10.0;
    p
}

/// Позиции участников сцены в тик `t`.
struct Actors {
    killer: [f32; 3],
    killer_yaw: f32,
    victim: [f32; 3],
    group: [f32; 3],
}

fn actors(pos: &dyn PositionSource, s: &Scene, t: i32) -> Option<Actors> {
    let k = pos.eye_pose(s.killer, t)?;
    let v = pos.eye_pose(s.victim, t.min(s.focus_tick)).or_else(|| pos.eye_pose(s.victim, s.focus_tick))?;
    let all = pos.players(t);
    let group = if all.is_empty() {
        lerp3(k.pos, v.pos, 0.5)
    } else {
        let n = all.len() as f32;
        let sum = all.iter().fold([0.0; 3], |a, (_, p)| [a[0] + p.pos[0], a[1] + p.pos[1], a[2] + p.pos[2]]);
        [sum[0] / n, sum[1] / n, sum[2] / n]
    };
    Some(Actors { killer: k.pos, killer_yaw: k.ang[1], victim: v.pos, group })
}

/// Ключевые кадры пресета на отрезке `[t0, t1]`; `None`, если не хватает позиций.
pub fn build(preset: &Preset, pos: &dyn PositionSource, s: &Scene, t0: i32, t1: i32) -> Option<Vec<CamKey>> {
    if t1 <= t0 {
        return None;
    }
    let step = ((KEY_EVERY_SECS * s.tick_rate) as usize).max(1);
    let mut ticks: Vec<i32> = (t0..t1).step_by(step).collect();
    ticks.push(t1);

    let mut keys = Vec::with_capacity(ticks.len());
    for t in ticks {
        let u = (t - t0) as f32 / (t1 - t0) as f32;
        let e = ease(preset.ease, u);
        let a = actors(pos, s, t)?;

        let mid = lerp3(a.killer, a.victim, 0.5);
        let center = match preset.anchor {
            Anchor::Killer => a.killer,
            Anchor::Victim => a.victim,
            Anchor::Mid => mid,
            Anchor::Group => a.group,
        };
        let base = match preset.frame {
            Frame::KillerYaw => a.killer_yaw,
            Frame::Axis => (a.victim[1] - a.killer[1]).atan2(a.victim[0] - a.killer[0]).to_degrees(),
        };
        let target = match preset.look {
            Look::Killer => chest(a.killer),
            Look::Victim => chest(a.victim),
            Look::Mid => chest(mid),
            Look::Group => chest(a.group),
            // перелёт взгляда — быстро в середине клипа, до и после держим героя
            Look::SweepKV | Look::SweepVK => {
                let w = ease(Ease::Smooth, ((u - 0.35) / 0.3).clamp(0.0, 1.0));
                let (from, to) = match preset.look {
                    Look::SweepKV => (a.killer, a.victim),
                    _ => (a.victim, a.killer),
                };
                chest(lerp3(from, to, w))
            }
        };

        let az = (base + lerp(preset.from.0, preset.to.0, e)).to_radians();
        let (r, h) = (lerp(preset.from.1, preset.to.1, e), lerp(preset.from.2, preset.to.2, e));
        let mut cam = [center[0] + r * az.cos(), center[1] + r * az.sin(), center[2] + h];
        // «пружинный подвес»: если между якорем и камерой стена, камера подтягивается к якорю
        let arm = chest(center);
        if let Some(f) = pos.raycast(arm, cam) {
            let len = dist(arm, cam).max(1.0);
            let keep = ((f * len - ARM_MARGIN) / len).max(0.0);
            cam = lerp3(arm, cam, keep);
        }
        let mut ang = look_at(cam, target);
        ang[2] = lerp(preset.roll.0, preset.roll.1, e);
        keys.push(CamKey { tick: t, pos: cam, ang, fov: lerp(preset.fov.0, preset.fov.1, e) });
    }
    Some(keys)
}

/// Насколько точка видна в кадре: 1 — внутри (с полями), дальше спад до 0.
fn visibility(cam: &CamKey, point: [f32; 3]) -> f32 {
    let a = look_at(cam.pos, point);
    let dyaw = wrap(a[1] - cam.ang[1]).abs();
    let dpitch = wrap(a[0] - cam.ang[0]).abs();
    let half_h = (cam.fov / 2.0).max(1.0);
    let half_v = ((half_h.to_radians().tan()) * 9.0 / 16.0).atan().to_degrees();
    let excess = (dyaw - 0.9 * half_h).max(0.0) / half_h + (dpitch - 0.9 * half_v).max(0.0) / half_v;
    (1.0 - excess).clamp(0.0, 1.0)
}

fn tier_drama(tier: Tier) -> f32 {
    match tier {
        Tier::Good => 2.0,
        Tier::Great => 3.5,
        Tier::Epic => 4.5,
    }
}

/// Красочность готового пути камеры (0..1).
pub fn evaluate(
    preset: &Preset,
    keys: &[CamKey],
    pos: &dyn PositionSource,
    s: &Scene,
    tier: Tier,
    used: &[String],
) -> Breakdown {
    let near_kill = (0.5 * s.tick_rate) as i32;
    let (mut kv, mut crowd, mut framing, mut weights) = (0.0, 0.0, 0.0, 0.0);
    let mut clipped = 0usize;
    let mut hidden = 0.0f32;

    for key in keys {
        let Some(a) = actors(pos, s, key.tick) else { continue };
        let w = if (key.tick - s.focus_tick).abs() <= near_kill { 3.0 } else { 1.0 };
        weights += w;

        let seen = |p: [f32; 3]| {
            let target = chest(p);
            if pos.raycast(key.pos, target).is_some() { 0.0 } else { visibility(key, target) }
        };
        let killer_seen = seen(a.killer);
        if pos.raycast(key.pos, chest(a.killer)).is_some() {
            hidden += w;
        }
        kv += w * (0.6 * killer_seen + 0.4 * seen(a.victim));

        let others: Vec<_> = pos
            .players(key.tick)
            .into_iter()
            .filter(|(id, _)| *id != s.killer && *id != s.victim)
            .collect();
        let seen = others
            .iter()
            .filter(|(_, p)| dist(key.pos, p.pos) < 1800.0 && visibility(key, chest(p.pos)) >= 0.99 && pos.raycast(key.pos, chest(p.pos)).is_none())
            .count();
        crowd += w * (seen.min(4) as f32 / 4.0);

        let d = dist(key.pos, a.killer);
        let size = if d < 90.0 {
            ((d - 30.0) / 60.0).clamp(0.0, 1.0)
        } else {
            (1.0 - (d - 350.0).max(0.0) / 850.0).clamp(0.0, 1.0)
        };
        let ang = look_at(key.pos, chest(a.killer));
        let off = (wrap(ang[1] - key.ang[1]).abs() / (key.fov / 2.0)).min(1.0);
        framing += w * (0.5 * (1.0 - off) + 0.5 * size);

        // камера внутри чьей-то головы
        let all = pos.players(key.tick);
        if all.iter().any(|(_, p)| dist(key.pos, p.pos) < 35.0) || dist(key.pos, a.killer) < 35.0 {
            clipped += 1;
        }
    }
    let n = weights.max(1.0);
    let (kill_visible, crowd, framing) = (kv / n, crowd / n, framing / n);

    // угловая скорость между ключами, °/с
    let dt = KEY_EVERY_SECS;
    let fastest = keys
        .windows(2)
        .map(|w| {
            let secs = ((w[1].tick - w[0].tick) as f32 / s.tick_rate).max(dt * 0.25);
            (wrap(w[1].ang[1] - w[0].ang[1]).abs().max(wrap(w[1].ang[0] - w[0].ang[0]).abs())) / secs
        })
        .fold(0.0, f32::max);
    let limit = if matches!(preset.category, Category::Whip) { 700.0 } else { 160.0 };
    let smooth = (1.0 - (fastest - limit).max(0.0) / 300.0).clamp(0.0, 1.0);

    let drama_fit = 1.0 - ((preset.drama as f32 - tier_drama(tier)).abs() / 4.0);

    let repeats = used.iter().filter(|u| *u == preset.name).count() as f32;
    let same_category_as_last = used
        .last()
        .and_then(|last| PRESETS.iter().find(|x| x.name == last))
        .is_some_and(|x| x.category == preset.category);
    let variety = (1.0 - 0.5 * repeats).max(0.0) * if same_category_as_last { 0.7 } else { 1.0 };

    let clip = clipped as f32 / keys.len().max(1) as f32;
    let through_wall = keys.windows(2).any(|w| pos.raycast(w[0].pos, w[1].pos).is_some());
    let occluded = hidden / n;
    let total = (0.30 * kill_visible + 0.16 * crowd + 0.12 * framing + 0.10 * smooth + 0.12 * drama_fit + 0.20 * variety)
        * (1.0 - clip)
        * if through_wall { 0.3 } else { 1.0 };
    Breakdown { kill_visible, crowd, framing, smooth, drama_fit, variety, occluded, through_wall, total }
}

/// Все подходящие пресеты, лучший первым.
#[allow(clippy::too_many_arguments)]
pub fn rank(
    pos: &dyn PositionSource,
    s: &Scene,
    t0: i32,
    t1: i32,
    role: u8,
    tier: Tier,
    used: &[String],
) -> Vec<Pick> {
    let mut picks: Vec<Pick> = PRESETS
        .iter()
        .filter(|pr| pr.fits & role != 0)
        .filter_map(|pr| {
            let keys = build(pr, pos, s, t0, t1)?;
            let score = evaluate(pr, &keys, pos, s, tier, used);
            Some(Pick { preset: pr, keys, score })
        })
        .collect();
    picks.sort_by(|a, b| b.score.total.total_cmp(&a.score.total));
    picks
}

pub fn best(
    pos: &dyn PositionSource,
    s: &Scene,
    t0: i32,
    t1: i32,
    role: u8,
    tier: Tier,
    used: &[String],
) -> Option<Pick> {
    rank(pos, s, t0, t1, role, tier, used).into_iter().next()
}

/// Синтетическая сцена для превью каталога и тестов: игрок 0 бежит вдоль +X,
/// впереди него стоят игроки 1..5 (игрок 1 — жертва).
pub struct SyntheticDuel;

impl SyntheticDuel {
    pub fn scene(tick_rate: f32) -> Scene {
        Scene { killer: PlayerId(0), victim: PlayerId(1), focus_tick: 100, tick_rate }
    }
}

impl PositionSource for SyntheticDuel {
    fn eye_pose(&self, player: PlayerId, tick: i32) -> Option<Pose> {
        let run = tick as f32 * 0.5;
        Some(match player.0 {
            0 => Pose { pos: [run, 0.0, 64.0], ang: [0.0, 0.0, 0.0] },
            id => Pose {
                pos: [run + 300.0 + 80.0 * id as f32, 60.0 * id as f32 - 90.0, 64.0],
                ang: [0.0, 180.0, 0.0],
            },
        })
    }

    fn players(&self, tick: i32) -> Vec<(PlayerId, Pose)> {
        (0..6).filter_map(|i| Some((PlayerId(i), self.eye_pose(PlayerId(i), tick)?))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TR: f32 = 64.0;

    #[test]
    fn catalog_has_forty_distinct_cameras_across_categories() {
        let mut names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        names.sort();
        names.dedup();
        assert!(names.len() >= 40, "{}", names.len());
        assert_eq!(names.len(), PRESETS.len());
        let mut cats: Vec<_> = PRESETS.iter().map(|p| p.category).collect();
        cats.dedup();
        assert!(PRESETS.iter().all(|p| (1..=5).contains(&p.drama) && p.fits & ANY != 0));
        for role in [INTRO, CUT, OUTRO] {
            assert!(PRESETS.iter().filter(|p| p.fits & role != 0).count() >= 15, "мало камер для роли {role}");
        }
    }

    #[test]
    fn every_preset_builds_valid_keys_and_scores_in_range() {
        let s = SyntheticDuel::scene(TR);
        for pr in PRESETS {
            let keys = build(pr, &SyntheticDuel, &s, 40, 140).unwrap_or_else(|| panic!("{} не построился", pr.name));
            assert!(keys.len() >= 5 && keys.first().unwrap().tick == 40 && keys.last().unwrap().tick == 140);
            assert!(keys.iter().all(|k| k.pos.iter().chain(&k.ang).all(|v| v.is_finite()) && (10.0..170.0).contains(&k.fov)), "{}", pr.name);
            let b = evaluate(pr, &keys, &SyntheticDuel, &s, Tier::Epic, &[]);
            for v in [b.kill_visible, b.crowd, b.framing, b.smooth, b.drama_fit, b.variety, b.total] {
                assert!((0.0..=1.0).contains(&v), "{}: {b:?}", pr.name);
            }
        }
    }

    #[test]
    fn camera_that_sees_both_beats_one_that_looks_away() {
        let s = SyntheticDuel::scene(TR);
        let by_name = |n: &str| PRESETS.iter().find(|p| p.name == n).unwrap();
        let score = |n: &str| {
            let pr = by_name(n);
            let keys = build(pr, &SyntheticDuel, &s, 70, 130).unwrap();
            evaluate(pr, &keys, &SyntheticDuel, &s, Tier::Great, &[])
        };
        let wide = score("kill_axis_wide");
        // камера спереди смотрит на убийцу, а жертва остаётся у неё за спиной
        let front = score("hero_low_front");
        assert!(wide.kill_visible > 0.9, "{wide:?}");
        assert!(wide.kill_visible > front.kill_visible && wide.crowd > front.crowd);
        assert!(wide.total > front.total);
        assert!(score("group_push").crowd > front.crowd);
    }

    #[test]
    fn used_presets_are_penalised_for_variety() {
        let s = SyntheticDuel::scene(TR);
        let pr = &PRESETS[0];
        let keys = build(pr, &SyntheticDuel, &s, 70, 130).unwrap();
        let fresh = evaluate(pr, &keys, &SyntheticDuel, &s, Tier::Great, &[]);
        let again = evaluate(pr, &keys, &SyntheticDuel, &s, Tier::Great, &[pr.name.to_owned()]);
        assert!(again.variety < fresh.variety && again.total < fresh.total);
    }

    #[test]
    fn missing_positions_give_no_camera() {
        let s = SyntheticDuel::scene(TR);
        assert!(best(&crate::NoPositions, &s, 0, 100, ANY, Tier::Epic, &[]).is_none());
    }

    #[test]
    fn dramatic_tier_prefers_dramatic_cameras() {
        let s = SyntheticDuel::scene(TR);
        let avg = |tier| {
            let r = rank(&SyntheticDuel, &s, 60, 140, ANY, tier, &[]);
            r.iter().take(5).map(|p| p.preset.drama as f32).sum::<f32>() / 5.0
        };
        assert!(avg(Tier::Epic) > avg(Tier::Good));
    }

    /// Сцена со стеной x = -100 позади убийцы (бесконечная плоскость, достаточно для теста).
    struct Walled;
    impl PositionSource for Walled {
        fn eye_pose(&self, p: PlayerId, t: i32) -> Option<Pose> {
            SyntheticDuel.eye_pose(p, t)
        }
        fn players(&self, t: i32) -> Vec<(PlayerId, Pose)> {
            SyntheticDuel.players(t)
        }
        fn raycast(&self, a: [f32; 3], b: [f32; 3]) -> Option<f32> {
            let wall = -100.0;
            let (da, db) = (a[0] - wall, b[0] - wall);
            (da * db < 0.0).then(|| da / (da - db))
        }
    }

    #[test]
    fn spring_arm_keeps_camera_in_front_of_wall() {
        let s = SyntheticDuel::scene(TR);
        let pr = PRESETS.iter().find(|p| p.name == "pull_out_behind").unwrap();
        // без стены камера уезжает на x ≈ 70 - 320 = -250
        let free = build(pr, &SyntheticDuel, &s, 60, 140).unwrap();
        assert!(free.iter().any(|k| k.pos[0] < -150.0));
        let arm = build(pr, &Walled, &s, 60, 140).unwrap();
        assert!(arm.iter().all(|k| k.pos[0] > -100.0), "{:?}", arm.iter().map(|k| k.pos[0]).collect::<Vec<_>>());
    }

    #[test]
    fn flying_through_a_wall_is_punished() {
        let s = SyntheticDuel::scene(TR);
        let pr = PRESETS.iter().find(|p| p.name == "orbit_cw_slow").unwrap();
        let key = |tick, x| CamKey { tick, pos: [x, 0.0, 100.0], ang: [0.0, 0.0, 0.0], fov: 90.0 };
        let inside = vec![key(60, 0.0), key(100, 10.0), key(140, 20.0)];
        let crossing = vec![key(60, 0.0), key(100, -300.0), key(140, 20.0)];
        let a = evaluate(pr, &inside, &Walled, &s, Tier::Great, &[]);
        let b = evaluate(pr, &crossing, &Walled, &s, Tier::Great, &[]);
        assert!(!a.through_wall && b.through_wall);
        assert!(b.total < a.total * 0.5);
    }

    #[test]
    fn hidden_killer_scores_zero_visibility() {
        let s = SyntheticDuel::scene(TR);
        let pr = PRESETS.iter().find(|p| p.name == "kill_axis_wide").unwrap();
        // камера по ту сторону стены от убийцы и жертвы
        let behind = (60..=140).step_by(20).map(|t| CamKey { tick: t, pos: [-400.0, 0.0, 100.0], ang: [0.0, 0.0, 0.0], fov: 90.0 }).collect::<Vec<_>>();
        let b = evaluate(pr, &behind, &Walled, &s, Tier::Great, &[]);
        assert!(b.kill_visible < 0.01 && b.occluded > 0.99, "{b:?}");
    }
}
