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
mod grenade;

pub use grenade::{grenade_victims, shoot_grenade};

use cs2::{Match, PlayerId};
use highlights::{Highlight, Utility, score_kill};
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
    /// Подпись в кадре: что именно показываем (пусто — без подписи).
    #[serde(default)]
    pub caption: String,
    /// Заставка перед шотом: (заголовок, подзаголовок) — например, начало раунда.
    #[serde(default)]
    pub card: Option<(String, String)>,
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
    /// Подача «как у эпика» для любого момента: пролёт-интро, смена ракурса на каждом
    /// убийстве, пролёт в финале и замедление на всех убийствах.
    pub cinematic: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { target_secs: 60.0, max_shots: 12, order: Order::BuildUp, slow: 0.4, slow_final: 0.18, cinematic: false }
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

/// Ролик про одного игрока, по раундам. В каждом раунде, где он что-то сделал:
/// 1. весь отрезок раунда его глазами — одним куском, нормальная скорость;
/// 2. режиссура — только на сильных моментах (серии, эффектные убийства) и важных гранатах;
/// 3. глазами противников: каждое убийство — нормальная скорость; взрыв гранаты глазами
///    ослеплённых/раненых — единственное место с замедлением (видно, сработала ли).
///
/// Шоты перекрываются по времени — запись отматывает демку назад.
pub fn direct_all(m: &Match, highlights: &[Highlight], utility: &[Utility], cfg: &Config, pos: &dyn PositionSource) -> Timeline {
    if !cfg.cinematic {
        let mut used = Vec::new();
        let mut shots: Vec<Shot> = highlights.iter().map(|h| shoot(m, h, cfg, pos, &mut used)).collect();
        shots.extend(utility.iter().filter_map(|u| shoot_grenade(m, u, cfg, pos)));
        shots.sort_by_key(|s| s.start_tick);
        return Timeline { map: m.map.clone(), tick_rate: m.tick_rate, shots };
    }
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let mut rounds: Vec<u32> = highlights.iter().map(|h| h.round).chain(utility.iter().map(|u| u.grenade.round)).collect();
    rounds.sort_unstable();
    rounds.dedup();

    let mut used: Vec<String> = Vec::new();
    let mut shots: Vec<Shot> = Vec::new();
    for round in rounds {
        let mut hs: Vec<&Highlight> = highlights.iter().filter(|h| h.round == round).collect();
        hs.sort_by_key(|h| h.first_tick());
        let mut us: Vec<&Utility> = utility.iter().filter(|u| u.grenade.round == round).collect();
        us.sort_by_key(|u| u.grenade.detonate_tick);
        let Some(hero) = hs.first().map(|h| h.player).or(us.first().map(|u| u.grenade.thrower)) else { continue };
        let name = m.name(hero);
        // гранаты залпом (две поп-флешки подряд) — один полёт и один набор вставок
        let volleys = volleys(&us, secs(3.0));

        // 1. раунд глазами игрока — кусками вокруг действий, пустой бег между ними вырезаем
        let floor = m.round(round).and_then(|r| r.freeze_end_tick.or(Some(r.start_tick))).unwrap_or(0);
        let mut spans: Vec<(i32, i32)> = hs
            .iter()
            .flat_map(|h| h.kills.iter().map(|k| (k.tick - secs(2.5), k.tick + secs(1.5))))
            .chain(us.iter().map(|u| {
                let g = &u.grenade;
                (g.throw_tick.unwrap_or(g.detonate_tick - secs(1.5)) - secs(1.0), g.detonate_tick + secs(1.2))
            }))
            .map(|(a, b)| (a.max(floor), b))
            .collect();
        spans.sort_unstable();
        let mut chunks: Vec<(i32, i32)> = Vec::new();
        for (a, b) in spans {
            match chunks.last_mut() {
                Some(last) if a - last.1 <= secs(5.0) => last.1 = last.1.max(b),
                _ => chunks.push((a, b)),
            }
        }
        let kills: usize = hs.iter().map(|h| h.kills.len()).sum();
        let mut summary: Vec<String> = Vec::new();
        if kills > 0 {
            summary.push(plural(kills, "убийство", "убийства", "убийств"));
        }
        let flashes = us.iter().filter(|u| u.grenade.kind == cs2::GrenadeKind::Flash).count();
        let other = us.len() - flashes;
        if flashes > 0 {
            summary.push(plural(flashes, "флешка", "флешки", "флешек"));
        }
        if other > 0 {
            summary.push(plural(other, "граната", "гранаты", "гранат"));
        }
        let tier = hs.iter().map(|h| tier_of(h.score)).max_by_key(|t| *t as u8).unwrap_or(Tier::Good);
        let score = hs.iter().map(|h| h.score).fold(0.0, f32::max);
        for (i, &(from, to)) in chunks.iter().enumerate() {
            shots.push(Shot {
                title: format!("{name} — раунд {round}, кусок {} из {}", i + 1, chunks.len()),
                tier,
                score,
                player: hero,
                round,
                start_tick: from,
                end_tick: to,
                segments: vec![Segment { start_tick: from, end_tick: to, camera: Camera::Pov { player: hero }, style: None, colorfulness: None }],
                timescale: vec![],
                zoom: vec![],
                caption: format!("Раунд {round} · глазами {name}"),
                card: (i == 0).then(|| (format!("Раунд {round}"), summary.join(" · "))),
            });
        }

        // 2. режиссура: только сильные моменты и гранаты, по порядку
        let mut director: Vec<Shot> = Vec::new();
        for h in hs.iter().filter(|h| is_strong(h)) {
            for k in &h.kills {
                director.extend(replay_shot(m, h, k, pos, &mut used));
            }
        }
        for v in &volleys {
            // в залпе летим за той гранатой, что сделала больше
            let best = v.iter().max_by(|a, b| a.score.total_cmp(&b.score)).expect("залп не пуст");
            director.extend(shoot_grenade(m, best, cfg, pos));
        }
        director.sort_by_key(|s| s.start_tick);
        shots.extend(director);

        // 3. глазами противников
        let mut enemies: Vec<Shot> = hs.iter().flat_map(|h| h.kills.iter().map(move |k| victim_shot(m, h, k, pos, cfg))).collect();
        enemies.extend(volleys.iter().flat_map(|v| grenade_victims(m, v, cfg)));
        enemies.sort_by_key(|s| s.start_tick);
        shots.extend(enemies);
    }
    Timeline { map: m.map.clone(), tick_rate: m.tick_rate, shots }
}

/// Гранаты одного игрока, взорвавшиеся с разницей не больше `gap` тиков, — один залп.
fn volleys<'a>(us: &[&'a Utility], gap: i32) -> Vec<Vec<&'a Utility>> {
    let mut out: Vec<Vec<&Utility>> = Vec::new();
    for &u in us {
        match out.last_mut() {
            Some(v) if u.grenade.detonate_tick - v.last().expect("залп не пуст").grenade.detonate_tick <= gap => v.push(u),
            _ => out.push(vec![u]),
        }
    }
    out
}

/// «1 убийство», «2 убийства», «5 убийств».
pub fn plural(n: usize, one: &str, few: &str, many: &str) -> String {
    let word = match (n % 10, n % 100) {
        (1, x) if x != 11 => one,
        (2..=4, x) if !(12..=14).contains(&x) => few,
        _ => many,
    };
    format!("{n} {word}")
}

/// Момент, достойный режиссуры: серия или эффектное убийство (дым, стена, без прицела, нож…).
fn is_strong(h: &Highlight) -> bool {
    h.kills.len() >= 2 || h.kills.iter().any(|k| score_kill(k).0 >= 16.0)
}

/// Повтор убийства со стороны: свободная камера, на которой видно и убийцу, и жертву.
fn replay_shot(m: &Match, h: &Highlight, k: &cs2::Kill, pos: &dyn PositionSource, used: &mut Vec<String>) -> Option<Shot> {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let (start, end) = (k.tick - secs(1.4), k.tick + secs(0.9));
    let scene = rig::Scene { killer: h.player, victim: k.victim, focus_tick: k.tick, tick_rate: tr };
    let pick = rig::best_replay(pos, &scene, start, end, used)?;
    used.push(pick.preset.name.to_owned());
    Some(Shot {
        title: format!("{} -> {}: повтор, {}", m.name(h.player), m.name(k.victim), pick.preset.name),
        tier: tier_of(h.score),
        score: h.score,
        player: h.player,
        round: h.round,
        start_tick: start,
        end_tick: end,
        segments: vec![Segment {
            start_tick: start,
            end_tick: end,
            camera: Camera::Free { keys: pick.keys },
            style: Some(pick.preset.name.to_owned()),
            colorfulness: Some(pick.score.total),
        }],
        timescale: vec![],
        zoom: vec![],
        caption: format!("Повтор · {} против {}", m.name(h.player), m.name(k.victim)),
        card: None,
    })
}

/// То же убийство глазами противника — до момента, когда его убивают.
fn victim_shot(m: &Match, h: &Highlight, k: &cs2::Kill, pos: &dyn PositionSource, cfg: &Config) -> Shot {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    // момент, когда игрок появляется у противника: в поле зрения и без стены между ними
    let seen = peek_tick(pos, k.victim, h.player, k.tick - secs(2.5), k.tick);
    let peek = seen.unwrap_or(k.tick - secs(0.3));
    // противник так и не увидел игрока — хватит короткой вставки с самой смертью
    let start = if seen.is_some() { (peek - secs(1.0)).min(k.tick - secs(1.6)) } else { k.tick - secs(1.2) };
    let end = k.tick + secs(0.3);
    // приближение и замедление на появлении, дальше обычная скорость
    let (a, b) = (peek - secs(0.1), (peek + secs(0.7)).min(k.tick));
    let curve = |v: f32| {
        let ramp = secs(0.15);
        vec![
            ScaleKey { tick: a - ramp, scale: 1.0 },
            ScaleKey { tick: a, scale: v },
            ScaleKey { tick: b, scale: v },
            ScaleKey { tick: b + ramp, scale: 1.0 },
        ]
    };
    Shot {
        title: format!("{} -> {}: глазами противника", m.name(h.player), m.name(k.victim)),
        tier: tier_of(h.score),
        score: h.score,
        player: k.victim,
        round: h.round,
        start_tick: start,
        end_tick: end,
        segments: vec![Segment { start_tick: start, end_tick: end, camera: Camera::Pov { player: k.victim }, style: Some("victim_pov".into()), colorfulness: None }],
        timescale: curve(cfg.slow),
        zoom: curve(0.6),
        caption: format!("Глазами {}", m.name(k.victim)),
        card: None,
    }
}

/// Первый тик на `[from, to]`, когда `target` виден глазами `viewer`: в пределах ~40° от взгляда
/// и без стены между ними.
fn peek_tick(pos: &dyn PositionSource, viewer: PlayerId, target: PlayerId, from: i32, to: i32) -> Option<i32> {
    (from..to).step_by(2).find(|&t| {
        let (Some(v), Some(p)) = (pos.eye_pose(viewer, t), pos.eye_pose(target, t)) else { return false };
        let a = look_at(v.pos, p.pos);
        let off = |x: f32| (x + 180.0).rem_euclid(360.0) - 180.0;
        off(a[1] - v.ang[1]).abs() <= 40.0 && off(a[0] - v.ang[0]).abs() <= 30.0 && pos.raycast(v.pos, [p.pos[0], p.pos[1], p.pos[2] - 10.0]).is_none()
    })
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
    // как подаём: в кинорежиме любой момент снимается как эпик (tier в плане остаётся честным)
    let look = if cfg.cinematic { Tier::Epic } else { tier };
    let (pre, post) = match look {
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
    match look {
        Tier::Epic => {
            let intro_end = (start + secs(1.8)).min(handoff);
            windows.push(Window { a: start, b: intro_end, role: rig::INTRO, scene: scene(&h.kills[0]), fallback: Some(Fallback::Pov) });
            // «нарезка» ракурсов на средних убийствах серии (в кинорежиме — и на первом)
            let mut cursor = intro_end + secs(0.4);
            let skip = if cfg.cinematic { 0 } else { 1 };
            for k in h.kills.iter().take(n.saturating_sub(1)).skip(skip) {
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
        let chosen = match rig::best(pos, &w.scene, w.a, w.b, w.role, look, used) {
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
    let wins = effects(h, look, cfg, tr, start, end);
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
        caption: String::new(),
        card: None,
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
