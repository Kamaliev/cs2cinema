//! Шоты гранаты: камера из-за плеча бросающего (саму гранату игра рендерит — траектория настоящая),
//! с подъездом по земле к точке взрыва, если путь свободен, -> общий план на взрыв и пострадавших
//! вблизи и невысоко; затем взрыв глазами тех, кого она решила (видно, сработала ли), и, для флешки,
//! убийства под неё — повтором со стороны, где видно и убийцу, и ослеплённого.
//!
//! Камера нигде не отрывается от земли выше роста и не проходит сквозь стены: каждый шаг
//! проверяется лучом по геометрии карты.

use cs2::{GrenadeKind, Match, PlayerId};
use highlights::Utility;

use crate::{Camera, CamKey, Config, Pose, PositionSource, ScaleKey, Segment, Shot, look_at, rig, tier_of};

/// Шаг ключей камеры: полёт быстрый, ключи чаще, чем у пресетов.
const KEY_EVERY_SECS: f32 = 0.1;
/// Насколько камера не доезжает до стены, юниты.
const MARGIN: f32 = 32.0;
/// Ослепление короче этого — не считается (экран едва мигнул).
const MIN_BLIND_SECS: f32 = 1.2;


/// Режиссёрский шот гранаты: камера за гранатой от броска -> общий план на взрыв и пострадавших.
/// Замедление — только на самом взрыве. Бросок от первого лица уже есть в отрезке раунда.
pub fn shoot_grenade(m: &Match, u: &Utility, cfg: &Config, pos: &dyn PositionSource) -> Option<Shot> {
    let g = &u.grenade;
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let det = g.detonate_tick;
    let throw = g.throw_tick.unwrap_or(det - secs(1.6)).clamp(det - secs(4.0), det - secs(0.2));
    let start = (throw - secs(0.3)).max(0);
    let release = (throw + secs(0.15)).min(det - 1);
    let seg = |a, b, camera, style: Option<&str>| Segment { start_tick: a, end_tick: b, camera, style: style.map(str::to_owned), colorfulness: None };

    let affected = affected(g);
    let points: Vec<[f32; 3]> = {
        let seen_at = if g.kind == GrenadeKind::Fire { det + secs(0.6) } else { det };
        let mut p = vec![add(g.pos, [0.0, 0.0, 8.0])];
        p.extend(affected.iter().filter_map(|&v| pos.eye_pose(v, seen_at).or_else(|| pos.eye_pose(v, det)).map(|e| e.pos)));
        p
    };
    let thrower_at_det = pos.eye_pose(g.thrower, det).map(|e| e.pos);
    let wide = wide_view(pos, &points, affected.len(), thrower_at_det);
    let after = match (g.kind, wide.is_some()) {
        (GrenadeKind::Fire | GrenadeKind::Smoke, true) => 2.4,
        (_, true) => 1.6,
        (_, false) => 0.6,
    };
    let end = det + secs(after);

    let mut segments = vec![seg(start, release, Camera::Pov { player: g.thrower }, None)];
    match flight(m, g, pos, release, det, end, wide.as_ref()) {
        Some(mut keys) => match keys.windows(2).position(|w| w[0].tick == det && w[1].tick == det + 1) {
            // склейка на общий план — отдельным участком со своим путём камеры
            Some(i) => {
                let mut after = keys.split_off(i + 1);
                after[0].tick = det;
                segments.push(seg(release, det, Camera::Free { keys }, Some("grenade_chase")));
                segments.push(seg(det, end, Camera::Free { keys: after }, Some("grenade_wide")));
            }
            None => segments.push(seg(release, end, Camera::Free { keys }, Some("grenade_chase"))),
        },
        // нет позиции бросающего — полёт показать не из чего
        None => return None,
    }

    let what = match g.kind {
        GrenadeKind::Flash => "Флешка",
        GrenadeKind::He => "HE",
        GrenadeKind::Fire => "Молотов",
        GrenadeKind::Smoke => "Дым",
    };
    Some(Shot {
        title: format!("{} — {} (R{}): полёт", m.name(g.thrower), u.tags.join(", "), g.round),
        tier: tier_of(u.score),
        score: u.score,
        player: g.thrower,
        round: g.round,
        start_tick: start,
        end_tick: end,
        segments,
        timescale: effect_slow(&[det], g.kind, cfg, tr),
        zoom: vec![],
        caption: format!("{what} · полёт и общий план"),
        card: None,
    })
}

/// Взрыв залпа глазами тех, кого он решил (убитых ослеплёнными, иначе — самого «наглухо»
/// ослеплённого; раненых и убитых HE/огнём), с замедлением, чтобы было видно, сработала ли граната.
/// Для флешек — ещё убийства под неё: повтор со стороны, где видно и убийцу, и ослеплённого
/// (если чистого ракурса нет — глазами убийцы). `used` — уже занятые пресеты камер.
pub fn grenade_victims(m: &Match, volley: &[&Utility], cfg: &Config, pos: &dyn PositionSource, used: &mut Vec<String>) -> Vec<Shot> {
    let (Some(first), Some(last)) = (volley.first(), volley.last()) else { return Vec::new() };
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let kind = first.grenade.kind;
    let (det0, det1) = (first.grenade.detonate_tick, last.grenade.detonate_tick);
    let dets: Vec<i32> = volley.iter().map(|u| u.grenade.detonate_tick).collect();
    let (pre, post) = match kind {
        GrenadeKind::Flash => (0.4, 0.9),
        GrenadeKind::He => (0.6, 0.6),
        GrenadeKind::Fire | GrenadeKind::Smoke => (0.5, 1.4),
    };
    let what = match kind {
        GrenadeKind::Flash => "Флешка",
        GrenadeKind::He => "HE",
        GrenadeKind::Fire => "Молотов",
        GrenadeKind::Smoke => "Дым",
    };
    // самое долгое ослепление игрока в залпе
    let blind_for = |v: PlayerId| {
        volley
            .iter()
            .flat_map(|u| u.grenade.blinded.iter())
            .filter(|b| b.0 == v)
            .map(|b| b.1)
            .fold(None, |acc: Option<f32>, d| Some(acc.map_or(d, |a| a.max(d))))
    };
    let base = |player: PlayerId| Shot {
        title: String::new(),
        tier: tier_of(first.score),
        score: volley.iter().map(|u| u.score).fold(0.0, f32::max),
        player,
        round: first.grenade.round,
        start_tick: 0,
        end_tick: 0,
        segments: vec![],
        timescale: vec![],
        zoom: vec![],
        caption: String::new(),
        card: None,
    };
    let pov = |a, b, player, style: &str| Segment { start_tick: a, end_tick: b, camera: Camera::Pov { player }, style: Some(style.to_owned()), colorfulness: None };

    // чьими глазами: белый экран на 5 секунд у каждого ослеплённого — скучно, берём только тех,
    // кого это решило: убитых под гранатой, иначе одного самого «наглухо» ослеплённого / раненых
    let killed: Vec<PlayerId> = volley.iter().flat_map(|u| u.grenade.kills.iter().copied()).collect();
    let mut victims: Vec<PlayerId> = volley.iter().flat_map(|u| affected(&u.grenade)).filter(|p| killed.contains(p)).collect();
    if victims.is_empty() {
        let mut rest: Vec<PlayerId> = volley.iter().flat_map(|u| affected(&u.grenade)).collect();
        if kind == GrenadeKind::Flash {
            rest.sort_by(|a, b| blind_for(*b).unwrap_or(0.0).total_cmp(&blind_for(*a).unwrap_or(0.0)));
            rest.truncate(1);
        } else {
            rest.truncate(2);
        }
        victims = rest;
    }
    let mut seen = Vec::new();
    victims.retain(|p| {
        let new = !seen.contains(p);
        seen.push(*p);
        new
    });
    let mut shots: Vec<Shot> = victims
        .into_iter()
        .map(|v| {
            let (a, b) = (det0 - secs(pre), det1 + secs(post));
            let how = match blind_for(v) {
                Some(d) => format!("ослеплён на {d:.1} с"),
                None => "под гранатой".to_owned(),
            };
            Shot {
                title: format!("{what} {} (R{}): глазами {}, {how}", m.name(first.grenade.thrower), first.grenade.round, m.name(v)),
                start_tick: a,
                end_tick: b,
                segments: vec![pov(a, b, v, "victim_pov")],
                timescale: effect_slow(&dets, kind, cfg, tr),
                caption: format!("Глазами {} · {how}", m.name(v)),
                ..base(v)
            }
        })
        .collect();

    // убийства тиммейтов под флешку — их глазами (свои уже есть в отрезке раунда)
    if kind == GrenadeKind::Flash {
        let thrower = first.grenade.thrower;
        let mut assists: Vec<&cs2::Kill> = m
            .kills
            .iter()
            .filter(|k| k.attacker != thrower && k.tick >= det0)
            .filter(|k| volley.iter().any(|u| u.grenade.kills.contains(&k.victim)))
            .filter(|k| blind_for(k.victim).is_some_and(|d| k.tick - det1 <= secs(d + 0.3)))
            .collect();
        assists.dedup_by_key(|k| k.tick);
        for k in assists {
            let (a, b) = (k.tick - secs(1.6), k.tick + secs(0.7));
            // кто и как: ракурс со стороны, на котором видно и убийцу, и ослеплённого
            let scene = rig::Scene { killer: k.attacker, victim: k.victim, focus_tick: k.tick, tick_rate: tr };
            let segment = match rig::best_replay(pos, &scene, a, b, used, false) {
                Some(pick) => {
                    used.push(pick.preset.name.to_owned());
                    Segment { start_tick: a, end_tick: b, camera: Camera::Free { keys: pick.keys }, style: Some(pick.preset.name.to_owned()), colorfulness: Some(pick.score.total) }
                }
                None => pov(a, b, k.attacker, "assist_pov"),
            };
            let ramp = secs(0.15);
            shots.push(Shot {
                title: format!("{what} {} (R{}): {} убивает ослеплённого {}", m.name(thrower), first.grenade.round, m.name(k.attacker), m.name(k.victim)),
                start_tick: a,
                end_tick: b,
                segments: vec![segment],
                // само убийство — с замедлением, чтобы прочитать, кто кого
                timescale: vec![
                    ScaleKey { tick: k.tick - secs(0.35) - ramp, scale: 1.0 },
                    ScaleKey { tick: k.tick - secs(0.35), scale: cfg.slow },
                    ScaleKey { tick: k.tick + secs(0.25), scale: cfg.slow },
                    ScaleKey { tick: k.tick + secs(0.25) + ramp, scale: 1.0 },
                ],
                caption: format!("Под флешку {} · {} убивает {}", m.name(thrower), m.name(k.attacker), m.name(k.victim)),
                ..base(k.attacker)
            });
        }
    }
    shots
}

/// Замедление только на эффекте гранат: чуть до каждого взрыва и сразу после (окна сливаются).
fn effect_slow(dets: &[i32], kind: GrenadeKind, cfg: &Config, tr: f32) -> Vec<ScaleKey> {
    let secs = |s: f32| (s * tr).round() as i32;
    let slow = if kind == GrenadeKind::Fire { cfg.slow.max(0.5) } else { cfg.slow.min(0.35) };
    let ramp = secs(0.22);
    let mut windows: Vec<(i32, i32)> = Vec::new();
    for &d in dets {
        let (a, b) = (d - secs(0.08), d + secs(0.5));
        match windows.last_mut() {
            Some(w) if a <= w.1 + 2 * ramp => w.1 = w.1.max(b),
            _ => windows.push((a, b)),
        }
    }
    windows
        .into_iter()
        .flat_map(|(a, b)| {
            [
                ScaleKey { tick: a - ramp, scale: 1.0 },
                ScaleKey { tick: a, scale: slow },
                ScaleKey { tick: b, scale: slow },
                ScaleKey { tick: b + ramp, scale: 1.0 },
            ]
        })
        .collect()
}

/// Кого граната задела: урон, ослепление (не меньше `MIN_BLIND_SECS`), убийство.
fn affected(g: &cs2::Grenade) -> Vec<PlayerId> {
    let mut v: Vec<PlayerId> = g.hurt.iter().chain(g.kills.iter()).copied().collect();
    // ослеплённые — по убыванию длительности: самые «наглухо» первыми
    let mut blind: Vec<(PlayerId, f32)> = g.blinded.iter().copied().filter(|b| b.1 >= MIN_BLIND_SECS).collect();
    blind.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.extend(blind.into_iter().map(|b| b.0));
    let mut seen = Vec::new();
    v.retain(|p| {
        let new = !seen.contains(p);
        seen.push(*p);
        new
    });
    v
}

/// Камера из-за плеча `eye`, развёрнутая в сторону `toward` (по горизонтали): позади, чуть сбоку
/// и выше глаз. К стене не прижимается: если сзади стена — пробуем другое плечо, иначе встаём
/// вплотную к игроку.
pub(crate) fn shoulder(pos: &dyn PositionSource, eye: &Pose, toward: [f32; 3]) -> [f32; 3] {
    let f = normalize([toward[0], toward[1], 0.0]).unwrap_or_else(|| {
        let d = dir(eye.ang);
        normalize([d[0], d[1], 0.0]).unwrap_or([1.0, 0.0, 0.0])
    });
    let right = [f[1], -f[0], 0.0];
    let want = |side: f32| add(add(eye.pos, scale(f, -72.0)), add(scale(right, 24.0 * side), [0.0, 0.0, 20.0]));
    let a = pull(pos, eye.pos, want(1.0));
    if dist(a, eye.pos) >= 40.0 {
        return a;
    }
    let b = pull(pos, eye.pos, want(-1.0));
    if dist(b, eye.pos) > dist(a, eye.pos) { b } else { a }
}

/// Ключи камеры на `[t0, t1]`: до взрыва — из-за плеча бросающего, с подъездом по земле к точке
/// взрыва, пока путь свободен (саму гранату рендерит игра); после — общий план.
/// `None` — нет позиции бросающего.
fn flight(m: &Match, g: &cs2::Grenade, pos: &dyn PositionSource, t0: i32, det: i32, t1: i32, wide: Option<&Wide>) -> Option<Vec<CamKey>> {
    let eye0 = pos.eye_pose(g.thrower, t0)?;
    let p1 = add(g.pos, [0.0, 0.0, 8.0]);
    // куда смотрит камера: от направления броска к точке взрыва по пологой дуге —
    // примерно так и летит граната, а на рикошете взгляд просто приходит к взрыву
    let aim = add(eye0.pos, scale(dir(eye0.ang), 400.0));
    let look = |u: f32| {
        let u = u.clamp(0.0, 1.0);
        let p = lerp3(aim, p1, smooth(u));
        [p[0], p[1], p[2] + 60.0 * u * (1.0 - u)]
    };
    let ground = sub(p1, eye0.pos);
    let base = shoulder(pos, &eye0, ground);
    // докуда можно подъехать: по земле к взрыву, не ближе 140 юнитов и не дальше первой стены
    let reach = {
        let far = ground[0].hypot(ground[1]);
        let f = normalize([ground[0], ground[1], 0.0]).unwrap_or([0.0; 3]);
        let want = add(base, scale(f, (far - 140.0).clamp(0.0, 0.6 * far)));
        pull(pos, base, want)
    };

    let step = ((KEY_EVERY_SECS * m.tick_rate) as usize).max(1);
    let mut ticks: Vec<i32> = (t0..t1).step_by(step).collect();
    ticks.extend([t1, det]);
    ticks.sort_unstable();
    ticks.dedup();

    let mut keys = Vec::with_capacity(ticks.len() + 1);
    let mut cam_prev = base;
    for &t in ticks.iter().filter(|&&t| t <= det) {
        let u = (t - t0) as f32 / (det - t0).max(1) as f32;
        // бросающий может двигаться — держимся за его плечом, постепенно подъезжая к взрыву
        let e = pos.eye_pose(g.thrower, t).unwrap_or(eye0);
        let want = lerp3(shoulder(pos, &e, ground), reach, smooth(u));
        // шаг делаем, только если на пути и до точки взгляда нет стены; иначе стоим
        let cam = if pos.raycast(cam_prev, want).is_none() && pos.raycast(want, look(u)).is_none() { want } else { cam_prev };
        keys.push(CamKey { tick: t, pos: cam, ang: look_at(cam, look(u)), fov: 90.0 });
        cam_prev = cam;
    }
    let last_flight = cam_prev;
    // плавный переезд к общему плану — только если по пути нет стен, иначе склейка
    let glide = wide.is_some_and(|w| pos.raycast(last_flight, w.cam).is_none() && pos.raycast(w.cam, last_flight).is_none());
    let reveal = (0.6 * m.tick_rate).max(1.0);

    for t in ticks.into_iter().filter(|&t| t > det) {
        let Some(w) = wide else {
            // общего плана нет: держим последний ракурс на точке взрыва
            keys.push(CamKey { tick: t, pos: last_flight, ang: look_at(last_flight, p1), fov: 85.0 });
            continue;
        };
        let e = if glide { smooth((t - det) as f32 / reveal) } else { 1.0 };
        if !glide && keys.last().is_some_and(|k: &CamKey| k.tick == det) {
            // склейка: следующий тик уже на общем плане
            keys.push(CamKey { tick: det + 1, pos: w.cam, ang: look_at(w.cam, w.target), fov: w.fov });
            if t == det + 1 {
                continue;
            }
        }
        // лёгкий дрейф вокруг группы
        let drift = ((t - det) as f32 / (t1 - det).max(1) as f32).clamp(0.0, 1.0);
        let off = sub(w.cam, w.target);
        let az = off[1].atan2(off[0]) + (8.0f32).to_radians() * smooth(drift);
        let r = off[0].hypot(off[1]);
        let orbit = pull(pos, w.target, [w.target[0] + r * az.cos(), w.target[1] + r * az.sin(), w.cam[2]]);
        let cam = lerp3(last_flight, orbit, e);
        let target = lerp3(p1, w.target, e);
        keys.push(CamKey { tick: t, pos: cam, ang: look_at(cam, target), fov: lerp(85.0, w.fov, e) });
    }
    Some(keys)
}

struct Wide {
    cam: [f32; 3],
    target: [f32; 3],
    fov: f32,
}

/// Общий план на взрыв и пострадавших: камера близко (≤ 320), на высоте чуть выше роста,
/// без стен до каждого; при прочих равных — со стороны бросавшего (`from`), чтобы было видно,
/// как граната пришла. `None` — чистого ракурса нет (тогда показываем глазами пострадавших).
fn wide_view(pos: &dyn PositionSource, points: &[[f32; 3]], victims: usize, from: Option<[f32; 3]>) -> Option<Wide> {
    let n = points.len().max(1) as f32;
    let mut target = scale(points.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / n);
    target[2] -= 10.0;
    let spread = points.iter().map(|p| dist(*p, target)).fold(0.0, f32::max) + 50.0;
    let fov: f32 = 90.0;
    // вертикальный полуугол кадра 16:9 при горизонтальном FOV
    let half_v = ((fov / 2.0).to_radians().tan() * 9.0 / 16.0).atan();
    let fit = (spread / (0.8 * half_v).tan()).clamp(140.0, 320.0);
    let from_az = from.map(|f| (f[1] - target[1]).atan2(f[0] - target[0]));

    let mut best: Option<(f32, Wide)> = None;
    for step in 0..16 {
        let az = (step as f32 * 22.5).to_radians();
        for lift in [0.08, 0.16, 0.26] {
            let want = [target[0] + fit * az.cos(), target[1] + fit * az.sin(), target[2] + fit * lift];
            let cam = pull(pos, target, want);
            let reach = dist(cam, target);
            if reach < 120.0 {
                continue; // упёрлись в стену вплотную — ракурса нет
            }
            // видно ли каждого: лучи в обе стороны (камера не должна сидеть внутри геометрии)
            let clear = |p: &[f32; 3]| {
                let c = [p[0], p[1], p[2] - 10.0];
                pos.raycast(cam, c).is_none() && pos.raycast(c, cam).is_none()
            };
            let framed = |p: &[f32; 3]| dist(*p, target) <= reach * (0.9 * half_v).tan() + 20.0;
            let seen = points.iter().skip(1).filter(|p| clear(p) && framed(p)).count();
            let blast = clear(&points[0]) as usize;
            // со стороны бросавшего (±60°) — небольшой бонус, ближе — тоже
            let side = from_az.map_or(0.0, |f| if ((az - f + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() <= 1.05 { 0.5 } else { 0.0 });
            let score = 3.0 * seen as f32 + blast as f32 + side + (320.0 - reach) / 1000.0;
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                best = Some((score, Wide { cam, target, fov }));
            }
        }
    }
    let (score, w) = best?;
    // нужен хотя бы половина пострадавших в кадре (или сам взрыв, если пострадавших не нашли)
    let need = if victims == 0 { 1.0 } else { 3.0 * ((victims + 1) / 2) as f32 };
    (score >= need).then_some(w)
}

/// Точка `to`, подтянутая к `from`, если между ними стена.
fn pull(pos: &dyn PositionSource, from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    match pos.raycast(from, to) {
        Some(f) => {
            let len = dist(from, to).max(1.0);
            lerp3(from, to, ((f * len - MARGIN) / len).max(0.0))
        }
        None => to,
    }
}

fn lerp(a: f32, b: f32, e: f32) -> f32 {
    a + (b - a) * e
}

fn smooth(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// Единичный вектор взгляда по углам Source (pitch вниз положителен).
fn dir(ang: [f32; 3]) -> [f32; 3] {
    let (p, y) = (ang[0].to_radians(), ang[1].to_radians());
    [p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dist(a, [0.0; 3]);
    (l > 1e-3).then(|| scale(a, 1.0 / l))
}

fn lerp3(a: [f32; 3], b: [f32; 3], e: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * e, a[1] + (b[1] - a[1]) * e, a[2] + (b[2] - a[2]) * e]
}
