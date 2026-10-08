//! «Режиссёр»: превращает хайлайты в монтажный план — какие моменты брать,
//! чьими глазами смотреть, где замедлять время и где пускать свободную камеру.
//!
//! Подача зависит от score хайлайта:
//! * `Epic`  — пролёт-интро, замедление на каждом убийстве и глубокий слоумо на последнем,
//!   финальный пролёт вокруг игрока;
//! * `Great` — от первого лица, слоумо на последнем и «ярких» убийствах, затем вид от третьего лица;
//! * `Good`  — от первого лица, слоумо только на «ярком» убийстве.
//!
//! Свободная камера (`Camera::Free`) строится по позициям игроков (`PositionSource`).
//! Пока источника позиций нет, режиссёр честно деградирует до POV/chase.

pub mod rig;

use cs2::{Match, PlayerId};
use highlights::{Highlight, score_kill};
use serde::{Deserialize, Serialize};

pub const EPIC_SCORE: f32 = 120.0;
pub const GREAT_SCORE: f32 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub pos: [f32; 3],
    /// pitch, yaw, roll в градусах (как в Source).
    pub ang: [f32; 3],
}

/// Откуда режиссёр берёт позиции глаз игроков.
pub trait PositionSource {
    fn eye_pose(&self, player: PlayerId, tick: i32) -> Option<Pose>;

    /// Все живые игроки в тик — нужны, чтобы оценивать «людность» кадра.
    fn players(&self, _tick: i32) -> Vec<(PlayerId, Pose)> {
        Vec::new()
    }

    /// Геометрия карты: доля отрезка `from -> to` до первой стены (0..1], `None` — путь свободен.
    /// Без геометрии (по умолчанию) стен нет.
    fn raycast(&self, _from: [f32; 3], _to: [f32; 3]) -> Option<f32> {
        None
    }
}

/// Заглушка: позиций нет, свободные пролёты отключены.
pub struct NoPositions;

impl PositionSource for NoPositions {
    fn eye_pose(&self, _: PlayerId, _: i32) -> Option<Pose> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    Good,
    Great,
    Epic,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CamKey {
    pub tick: i32,
    pub pos: [f32; 3],
    pub ang: [f32; 3],
    pub fov: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Camera {
    /// От первого лица.
    Pov { player: PlayerId },
    /// От третьего лица (chase cam).
    Chase { player: PlayerId },
    /// Свободная камера по ключевым кадрам.
    Free { keys: Vec<CamKey> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_tick: i32,
    pub end_tick: i32,
    pub camera: Camera,
    /// Имя пресета из каталога камер (для свободных камер).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Красочность выбранной камеры, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colorfulness: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScaleKey {
    pub tick: i32,
    pub scale: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    pub title: String,
    pub tier: Tier,
    pub score: f32,
    pub player: PlayerId,
    pub round: u32,
    pub start_tick: i32,
    pub end_tick: i32,
    pub segments: Vec<Segment>,
    /// Кусочно-линейная кривая скорости времени; вне ключей скорость 1.0.
    pub timescale: Vec<ScaleKey>,
    /// Множитель FOV для POV/chase-сегментов (1.0 — обычный, <1 — приближение).
    #[serde(default)]
    pub zoom: Vec<ScaleKey>,
}

impl Shot {
    pub fn scale_at(&self, tick: i32) -> f32 {
        scale_at(&self.timescale, tick)
    }

    /// Длительность готового клипа в секундах (с учётом замедления).
    pub fn output_secs(&self, tick_rate: f32) -> f32 {
        (self.start_tick..self.end_tick)
            .map(|t| 1.0 / (self.scale_at(t) * tick_rate))
            .sum()
    }
}

pub fn scale_at(keys: &[ScaleKey], tick: i32) -> f32 {
    match keys {
        [] => 1.0,
        [first, ..] if tick <= first.tick => first.scale,
        [.., last] if tick >= last.tick => last.scale,
        _ => {
            let i = keys.partition_point(|k| k.tick <= tick);
            let (a, b) = (keys[i - 1], keys[i]);
            let u = (tick - a.tick) as f32 / (b.tick - a.tick).max(1) as f32;
            a.scale + (b.scale - a.scale) * u
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// От слабых к сильным: лучший момент — в финале.
    BuildUp,
    Chronological,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Целевая длина ролика, секунды.
    pub target_secs: f32,
    pub max_shots: usize,
    pub order: Order,
    pub slow: f32,
    pub slow_final: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { target_secs: 60.0, max_shots: 12, order: Order::BuildUp, slow: 0.4, slow_final: 0.18 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub map: String,
    pub tick_rate: f32,
    /// В порядке показа в итоговом ролике.
    pub shots: Vec<Shot>,
}

impl Timeline {
    pub fn total_secs(&self) -> f32 {
        self.shots.iter().map(|s| s.output_secs(self.tick_rate)).sum()
    }
}

pub fn direct(m: &Match, highlights: &[Highlight], cfg: &Config, pos: &dyn PositionSource) -> Timeline {
    let mut ranked: Vec<&Highlight> = highlights.iter().collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));

    let mut shots: Vec<Shot> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut total = 0.0;
    for h in ranked {
        if shots.len() >= cfg.max_shots {
            break;
        }
        // пресеты «занимаем» только если шот реально попал в монтаж
        let mut trial = used.clone();
        let shot = shoot(m, h, cfg, pos, &mut trial);
        let len = shot.output_secs(m.tick_rate);
        if !shots.is_empty() && total + len > cfg.target_secs {
            continue; // не лезет — пробуем более короткий момент
        }
        total += len;
        used = trial;
        shots.push(shot);
    }

    match cfg.order {
        Order::BuildUp => shots.sort_by(|a, b| a.score.total_cmp(&b.score)),
        Order::Chronological => shots.sort_by_key(|s| s.start_tick),
    }
    Timeline { map: m.map.clone(), tick_rate: m.tick_rate, shots }
}

fn tier_of(score: f32) -> Tier {
    if score >= EPIC_SCORE {
        Tier::Epic
    } else if score >= GREAT_SCORE {
        Tier::Great
    } else {
        Tier::Good
    }
}

/// Где камера покидает POV и что подставить, если свободную камеру построить не из чего.
struct Window {
    a: i32,
    b: i32,
    role: u8,
    scene: rig::Scene,
    fallback: Option<Fallback>,
}

#[derive(Clone, Copy)]
enum Fallback {
    Pov,
    Chase,
}

/// `used` — имена уже выбранных пресетов (для разнообразия); пополняется.
pub fn shoot(m: &Match, h: &Highlight, cfg: &Config, pos: &dyn PositionSource, used: &mut Vec<String>) -> Shot {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let tier = tier_of(h.score);
    let (pre, post) = match tier {
        Tier::Epic => (3.0, 2.8),
        Tier::Great => (2.5, 2.0),
        Tier::Good => (2.2, 1.5),
    };

    let round = m.round(h.round);
    let floor = round.and_then(|r| r.freeze_end_tick.or(Some(r.start_tick))).unwrap_or(0);
    let start = (h.first_tick() - secs(pre)).max(floor);
    let end = h.last_tick() + secs(post);
    let handoff = (h.last_tick() - secs(0.7)).max(start + 1);
    let n = h.kills.len();

    let scene = |k: &cs2::Kill| rig::Scene { killer: h.player, victim: k.victim, focus_tick: k.tick, tick_rate: tr };

    // 1. где хотим свободную камеру
    let mut windows: Vec<Window> = Vec::new();
    match tier {
        Tier::Epic => {
            let intro_end = (start + secs(1.8)).min(handoff);
            windows.push(Window { a: start, b: intro_end, role: rig::INTRO, scene: scene(&h.kills[0]), fallback: Some(Fallback::Pov) });
            // «нарезка» ракурсов на средних убийствах серии
            let mut cursor = intro_end + secs(0.4);
            for k in h.kills.iter().take(n.saturating_sub(1)).skip(1) {
                let (a, b) = ((k.tick - secs(0.5)).max(cursor), (k.tick + secs(0.6)).min(handoff - secs(0.3)));
                if b - a >= secs(0.6) {
                    windows.push(Window { a, b, role: rig::CUT, scene: scene(k), fallback: None });
                    cursor = b + secs(0.4);
                }
            }
            windows.push(Window { a: handoff, b: end, role: rig::OUTRO, scene: scene(&h.kills[n - 1]), fallback: Some(Fallback::Chase) });
        }
        Tier::Great => {
            let a = h.last_tick() + secs(0.4);
            windows.push(Window { a, b: end, role: rig::OUTRO, scene: scene(&h.kills[n - 1]), fallback: Some(Fallback::Chase) });
        }
        Tier::Good => {}
    }

    // 2. выбираем лучшую камеру на каждое окно и склеиваем с POV
    let seg = |a, b, camera, style: Option<String>, colorfulness| Segment { start_tick: a, end_tick: b, camera, style, colorfulness };
    let pov = |a, b| seg(a, b, Camera::Pov { player: h.player }, None, None);
    let mut segments: Vec<Segment> = Vec::new();
    let mut cursor = start;
    for w in windows {
        if w.b <= w.a {
            continue;
        }
        let chosen = match rig::best(pos, &w.scene, w.a, w.b, w.role, tier, used) {
            Some(pick) => {
                used.push(pick.preset.name.to_owned());
                Some(seg(w.a, w.b, Camera::Free { keys: pick.keys }, Some(pick.preset.name.to_owned()), Some(pick.score.total)))
            }
            None => w.fallback.map(|f| match f {
                Fallback::Pov => pov(w.a, w.b),
                Fallback::Chase => seg(w.a, w.b, Camera::Chase { player: h.player }, None, None),
            }),
        };
        if let Some(c) = chosen {
            if w.a > cursor {
                segments.push(pov(cursor, w.a));
            }
            cursor = w.b;
            segments.push(c);
        }
    }
    if cursor < end {
        segments.push(pov(cursor, end));
    }

    // 3. время и зум; зум не трогаем там, где летит свободная камера (у неё свой FOV)
    let wins = effects(h, tier, cfg, tr, start, end);
    let free: Vec<(i32, i32)> = segments
        .iter()
        .filter(|s| matches!(s.camera, Camera::Free { .. }))
        .map(|s| (s.start_tick, s.end_tick))
        .collect();
    let pad = secs(0.25);
    let timescale = curve(wins.iter().map(|w| (w.from, w.to, w.slow)), pad, start, end);
    let zoom = curve(
        wins.iter()
            .filter(|w| !free.iter().any(|(a, b)| w.from - pad < *b && w.to + pad > *a))
            .map(|w| (w.from, w.to, w.zoom)),
        pad,
        start,
        end,
    );

    Shot {
        title: format!("{} — {} (R{})", m.name(h.player), h.tags.join(", "), h.round),
        tier,
        score: h.score,
        player: h.player,
        round: h.round,
        start_tick: start,
        end_tick: end,
        segments,
        timescale,
        zoom,
    }
}

/// Окно «акцента» вокруг убийства: замедление времени и приближение.
struct Effect {
    from: i32,
    to: i32,
    slow: f32,
    zoom: f32,
}

fn effects(h: &Highlight, tier: Tier, cfg: &Config, tr: f32, _start: i32, _end: i32) -> Vec<Effect> {
    let secs = |s: f32| (s * tr).round() as i32;
    let n = h.kills.len();

    let mut raw: Vec<Effect> = Vec::new();
    for (i, k) in h.kills.iter().enumerate() {
        let last = i + 1 == n;
        let special = score_kill(k).0 >= 16.0;
        // (замедление, зум)
        let depth: Option<(f32, f32)> = match tier {
            Tier::Epic if last => Some((cfg.slow_final, 0.55)),
            Tier::Epic => Some((cfg.slow, 0.75)),
            Tier::Great if last => Some((cfg.slow_final.max(0.25), 0.65)),
            Tier::Great | Tier::Good if special => Some((cfg.slow, 0.8)),
            _ => None,
        };
        if let Some((slow, mut zoom)) = depth {
            if matches!(k.weapon.as_str(), "awp" | "ssg08" | "scar20" | "g3sg1") {
                zoom = zoom.min(0.5); // снайперская «оптика»
            }
            let hold = if last { 0.9 } else { 0.45 };
            raw.push(Effect { from: k.tick - secs(0.3), to: k.tick + secs(hold), slow, zoom });
        }
    }

    // склеиваем перекрывающиеся окна, берём самые глубокие значения
    let ramp = secs(0.25);
    let mut merged: Vec<Effect> = Vec::new();
    for w in raw {
        match merged.last_mut() {
            Some(prev) if w.from <= prev.to + 2 * ramp => {
                prev.to = prev.to.max(w.to);
                prev.slow = prev.slow.min(w.slow);
                prev.zoom = prev.zoom.min(w.zoom);
            }
            _ => merged.push(w),
        }
    }
    merged
}

/// Окна `(from, to, значение)` -> кусочно-линейная кривая: плавно входим, держим, плавно выходим.
fn curve(windows: impl Iterator<Item = (i32, i32, f32)>, ramp: i32, start: i32, end: i32) -> Vec<ScaleKey> {
    let mut keys = Vec::new();
    for (from, to, value) in windows {
        let from = from.max(start + ramp);
        let to = to.min(end - ramp);
        if to <= from {
            continue;
        }
        keys.push(ScaleKey { tick: from - ramp, scale: 1.0 });
        keys.push(ScaleKey { tick: from, scale: value });
        keys.push(ScaleKey { tick: to, scale: value });
        keys.push(ScaleKey { tick: to + ramp, scale: 1.0 });
    }
    keys
}

/// Углы Source (pitch вниз положителен) для взгляда из `from` в `to`.
pub fn look_at(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    let yaw = dy.atan2(dx).to_degrees();
    let pitch = (-dz).atan2(dx.hypot(dy)).to_degrees();
    [pitch, yaw, 0.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2::Kill;
    use highlights::Kind;

    fn kill(tick: i32, victim: i32) -> Kill {
        Kill {
            tick,
            round: 1,
            attacker: PlayerId(0),
            victim: PlayerId(victim),
            assister: None,
            weapon: "ak47".into(),
            headshot: false,
            wallbang: false,
            through_smoke: false,
            no_scope: false,
            attacker_blind: false,
            attacker_in_air: false,
            distance: 5.0,
        }
    }

    fn hl(score: f32, ticks: &[i32]) -> Highlight {
        Highlight {
            kind: Kind::MultiKill(ticks.len() as u8),
            round: 1,
            player: PlayerId(0),
            kills: ticks.iter().enumerate().map(|(i, t)| kill(*t, 1 + i as i32 % 3)).collect(),
            score,
            tags: vec![format!("{}k", ticks.len())],
        }
    }

    fn mat() -> Match {
        Match::build("de_test".into(), 64.0, vec![(0, 0, "ace".into(), 1)], &[])
    }

    use rig::SyntheticDuel as Duel;

    #[test]
    fn scale_curve_interpolates() {
        let keys = [ScaleKey { tick: 0, scale: 1.0 }, ScaleKey { tick: 10, scale: 0.5 }];
        assert_eq!(scale_at(&keys, -5), 1.0);
        assert!((scale_at(&keys, 5) - 0.75).abs() < 1e-6);
        assert_eq!(scale_at(&keys, 50), 0.5);
    }

    #[test]
    fn slowmo_makes_clip_longer_than_realtime() {
        let m = mat();
        let shot = shoot(&m, &hl(200.0, &[1000, 1100, 1200]), &Config::default(), &NoPositions, &mut vec![]);
        assert_eq!(shot.tier, Tier::Epic);
        let realtime = (shot.end_tick - shot.start_tick) as f32 / 64.0;
        assert!(shot.output_secs(64.0) > realtime * 1.1);
        assert!(shot.scale_at(1200) < 0.25);
        assert!(shot.timescale.windows(2).all(|w| w[0].tick <= w[1].tick));
    }

    #[test]
    fn zoom_punches_in_on_kills_without_free_camera() {
        let m = mat();
        let shot = shoot(&m, &hl(200.0, &[1000, 1100, 1200]), &Config::default(), &NoPositions, &mut vec![]);
        let z = scale_at(&shot.zoom, 1200);
        assert!(z < 0.6, "на последнем убийстве приближаемся, а не {z}");
        assert_eq!(scale_at(&shot.zoom, shot.start_tick + 1), 1.0);
    }

    #[test]
    fn without_positions_segments_stay_contiguous_and_not_free() {
        let m = mat();
        let shot = shoot(&m, &hl(200.0, &[1000, 1100]), &Config::default(), &NoPositions, &mut vec![]);
        assert!(shot.segments.iter().all(|s| !matches!(s.camera, Camera::Free { .. })));
        assert_contiguous(&shot);
    }

    fn assert_contiguous(shot: &Shot) {
        for w in shot.segments.windows(2) {
            assert_eq!(w[0].end_tick, w[1].start_tick, "{:?}", shot.segments.iter().map(|s| (s.start_tick, s.end_tick)).collect::<Vec<_>>());
        }
        assert_eq!(shot.segments.first().unwrap().start_tick, shot.start_tick);
        assert_eq!(shot.segments.last().unwrap().end_tick, shot.end_tick);
    }

    #[test]
    fn epic_with_positions_cuts_between_named_cameras() {
        let m = mat();
        let mut used = vec![];
        let shot = shoot(&m, &hl(200.0, &[1000, 1150, 1300, 1450]), &Config::default(), &Duel, &mut used);
        assert_contiguous(&shot);
        let free: Vec<_> = shot.segments.iter().filter(|s| matches!(s.camera, Camera::Free { .. })).collect();
        assert!(free.len() >= 3, "интро, нарезка на средних киллах и аутро: {}", free.len());
        assert!(free.iter().all(|s| s.style.is_some() && s.colorfulness.unwrap() > 0.0));
        // разные ракурсы подряд не повторяются
        let styles: Vec<_> = free.iter().map(|s| s.style.clone().unwrap()).collect();
        let mut uniq = styles.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), styles.len(), "повтор пресета в одном шоте: {styles:?}");
        assert_eq!(used, styles);
        // зум не накладывается на свободную камеру
        for s in &free {
            for t in (s.start_tick..s.end_tick).step_by(8) {
                assert_eq!(scale_at(&shot.zoom, t), 1.0, "зум внутри свободной камеры на {t}");
            }
        }
    }

    #[test]
    fn great_tier_ends_with_free_camera_when_possible() {
        let m = mat();
        let shot = shoot(&m, &hl(80.0, &[1000, 1100]), &Config::default(), &Duel, &mut vec![]);
        assert_eq!(shot.tier, Tier::Great);
        assert!(matches!(shot.segments.last().unwrap().camera, Camera::Free { .. }));
        assert_contiguous(&shot);
    }

    #[test]
    fn look_at_points_at_target() {
        let a = look_at([0.0, 0.0, 100.0], [100.0, 0.0, 0.0]);
        assert!(a[1].abs() < 1e-4 && (a[0] - 45.0).abs() < 1e-3);
        let b = look_at([0.0, 0.0, 0.0], [0.0, 10.0, 0.0]);
        assert!((b[1] - 90.0).abs() < 1e-4);
    }

    #[test]
    fn selection_respects_budget_and_order() {
        let m = mat();
        let hs: Vec<_> = (0..10).map(|i| hl(10.0 + i as f32 * 20.0, &[10_000 * (i + 1), 10_000 * (i + 1) + 100])).collect();
        let cfg = Config { target_secs: 25.0, ..Config::default() };
        let t = direct(&m, &hs, &cfg, &NoPositions);
        assert!(t.shots.len() >= 2 && t.shots.len() < 10);
        assert!(t.total_secs() <= 25.0 + 1e-3 || t.shots.len() == 1);
        assert!(t.shots.windows(2).all(|w| w[0].score <= w[1].score), "build-up: лучший в конце");
        assert!(t.shots.last().unwrap().score >= 190.0);
    }

    #[test]
    fn variety_spreads_presets_across_shots() {
        let m = mat();
        let hs: Vec<_> = (0..4).map(|i| hl(200.0 - i as f32, &[2000 * (i + 1), 2000 * (i + 1) + 120, 2000 * (i + 1) + 240])).collect();
        let cfg = Config { target_secs: 600.0, max_shots: 4, ..Config::default() };
        let t = direct(&m, &hs, &cfg, &Duel);
        let styles: Vec<String> = t.shots.iter().flat_map(|s| s.segments.iter().filter_map(|x| x.style.clone())).collect();
        let mut uniq = styles.clone();
        uniq.sort();
        uniq.dedup();
        assert!(uniq.len() * 10 >= styles.len() * 8, "слишком много повторов: {styles:?}");
    }
}
