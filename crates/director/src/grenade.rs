//! Шоты гранаты:
//! * режиссёрский — nade-cam: камера цепляется за настоящий снаряд (полёт из демки) сзади-сверху
//!   и несётся за ним, перед взрывом тормозит — граната улетает вперёд в кадр, взрыв в замедлении,
//!   затем облёт на взрыв и пострадавших;
//! * взрыв глазами тех, кого граната решила (видно, сработала ли), и, для флешки, убийства под неё —
//!   повтором со стороны, где видно и убийцу, и ослеплённого.
//!
//! Камера не проходит сквозь стены: каждый шаг проверяется лучом по геометрии карты, а за гранатой
//! она держится на «пружинной руке» — у стены подтягивается к снаряду, а не замирает.

use cs2::{GrenadeKind, Match, PlayerId};
use highlights::{MIN_BLIND_SECS, Utility};

use crate::{Camera, CamKey, Config, Density, Pose, PositionSource, ScaleKey, Segment, Shot, look_at, plural, rig, tier_of};

/// Насколько камера не доезжает до стены, юниты.
const MARGIN: f32 = 32.0;
/// Шаг ключей nade-cam, тики: снаряд быстрый.
const CHASE_STEP: i32 = 2;
/// Где камера за снарядом: позади по ходу полёта и выше.
const CHASE_BACK: f32 = 70.0;
const CHASE_UP: f32 = 18.0;
/// Быстрее камера не разворачивается (отскок — плавный разворот, а не рывок), °/с.
const CHASE_TURN: f32 = 220.0;
/// Рука, освободившаяся от стены, отрастает не быстрее, юнитов/с.
const ARM_GROW: f32 = 300.0;
/// Камера смотрит на точку полёта на столько вперёд, с.
const AHEAD_SECS: f32 = 0.15;
/// После взрыва камера ещё столько стоит на нём (наезд), потом облёт, с.
const SETTLE_SECS: f32 = 0.5;
/// Флешка ближе этого засвечивает весь кадр — камера встаёт дальше, юниты.
const FLASH_KEEP: f32 = 300.0;

/// Полёт снаряда: `(тик, позиция)` по возрастанию тиков.
type Path = [(i32, [f32; 3])];

/// Режиссёрский шот гранаты: бросок от первого лица -> nade-cam -> встреча со взрывом -> облёт.
/// Длинный полёт чуть ускорен, взрыв — в замедлении.
pub fn shoot_grenade(m: &Match, u: &Utility, cfg: &Config, pos: &dyn PositionSource) -> Option<Shot> {
    let g = &u.grenade;
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let det = g.detonate_tick;
    let throw = g.throw_tick.unwrap_or(det - secs(1.6)).clamp(det - secs(4.0), det - secs(0.2));
    // настоящий полёт, иначе — расчётная дуга, если она не задевает стен; без них показывать нечего
    let path = pos.grenade_path(g).filter(|p| p.len() >= 2).or_else(|| ballistic(g, pos, throw, det))?;
    let release = (throw + secs(0.15)).max(path[0].0).min(det - 1);
    // бросок от первого лица уже был в отрезке раунда — в сбалансированном монтаже сразу летим за снарядом
    let start = match cfg.density {
        Density::Full => (throw - secs(0.3)).max(0),
        Density::Balanced => release,
    };
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

    let mut segments = Vec::new();
    if start < release {
        segments.push(seg(start, release, Camera::Pov { player: g.thrower }, None));
    }
    let NadeCam { keys, cut } = nade_cam(&path, pos, g.kind, release, det, end, wide.as_ref(), tr);
    match cut {
        // облёт упёрся в стену — склейка на общий план отдельным участком со своим путём камеры
        Some((at, wide_keys)) => {
            segments.push(seg(release, at, Camera::Free { keys }, Some("nade_cam")));
            segments.push(seg(at, end, Camera::Free { keys: wide_keys }, Some("grenade_wide")));
        }
        None => segments.push(seg(release, end, Camera::Free { keys }, Some("nade_cam"))),
    }

    Some(Shot {
        title: format!("{} — {} (R{})", m.name(g.thrower), u.tags.join(", "), g.round),
        tier: tier_of(u.score),
        score: u.score,
        player: g.thrower,
        round: g.round,
        start_tick: start,
        end_tick: end,
        segments,
        timescale: nade_speed(release, det, g.kind, cfg, tr),
        zoom: vec![],
        caption: grenade_caption(g),
        card: None,
    })
}

/// Что сделала граната — подписью в кадре: «Флешка · ослепил 3 · до 2.4 с», «HE · 87 урона · 1 убийство».
fn grenade_caption(g: &cs2::Grenade) -> String {
    let mut parts: Vec<String> = Vec::new();
    match g.kind {
        GrenadeKind::Flash => {
            parts.push("Флешка".into());
            let blind: Vec<f32> = g.blinded.iter().map(|b| b.1).filter(|&d| d >= MIN_BLIND_SECS).collect();
            if !blind.is_empty() {
                parts.push(format!("ослепил {}", blind.len()));
                parts.push(format!("до {:.1} с", blind.iter().copied().fold(0.0, f32::max)));
            }
            if !g.kills.is_empty() {
                parts.push(format!("{} под неё", plural(g.kills.len(), "убийство", "убийства", "убийств")));
            }
        }
        kind => {
            parts.push(match kind {
                GrenadeKind::He => "HE",
                GrenadeKind::Fire => "Молотов",
                _ => "Дым",
            }.into());
            if g.damage > 0 {
                parts.push(format!("{} урона", g.damage));
            }
            if !g.kills.is_empty() {
                parts.push(plural(g.kills.len(), "убийство", "убийства", "убийств"));
            }
        }
    }
    parts.join(" · ")
}

/// Скорость режиссёрского шота: длинный полёт ускорен (1.4×) и рампой уходит в замедление на взрыве.
fn nade_speed(release: i32, det: i32, kind: GrenadeKind, cfg: &Config, tr: f32) -> Vec<ScaleKey> {
    let secs = |s: f32| (s * tr).round() as i32;
    let slow = if kind == GrenadeKind::Fire { cfg.slow.max(0.5) } else { cfg.slow.min(0.3) };
    let (a, b) = (det - secs(0.1), det + secs(0.45));
    let (fast_in, fast_full, brake) = (release + secs(0.2), release + secs(0.45), a - secs(0.35));
    let mut keys = Vec::new();
    if (det - release) as f32 / tr > 1.4 && brake > fast_full {
        keys.extend([ScaleKey { tick: fast_in, scale: 1.0 }, ScaleKey { tick: fast_full, scale: 1.4 }, ScaleKey { tick: brake, scale: 1.4 }]);
    } else {
        keys.push(ScaleKey { tick: a - secs(0.22), scale: 1.0 });
    }
    keys.extend([ScaleKey { tick: a, scale: slow }, ScaleKey { tick: b, scale: slow }, ScaleKey { tick: b + secs(0.2), scale: 1.0 }]);
    keys
}

/// Позиция снаряда в тик (между точками — линейно, за краями — крайняя точка).
fn path_at(p: &Path, t: i32) -> [f32; 3] {
    let i = p.partition_point(|x| x.0 <= t);
    match (i.checked_sub(1).map(|i| p[i]), p.get(i)) {
        (Some(a), Some(b)) => lerp3(a.1, b.1, (t - a.0) as f32 / (b.0 - a.0).max(1) as f32),
        (Some(a), None) => a.1,
        (None, Some(b)) => b.1,
        (None, None) => [0.0; 3],
    }
}

/// Полёта в демке нет — дуга от глаз бросающего до взрыва; `None`, если она задевает стену.
fn ballistic(g: &cs2::Grenade, pos: &dyn PositionSource, throw: i32, det: i32) -> Option<Vec<(i32, [f32; 3])>> {
    let eye = pos.eye_pose(g.thrower, throw)?;
    let p0 = add(eye.pos, scale(dir(eye.ang), 16.0));
    let p1 = add(g.pos, [0.0, 0.0, 8.0]);
    let apex = (0.25 * dist(p0, p1)).clamp(20.0, 220.0);
    let mut path: Vec<(i32, [f32; 3])> = (throw..det).step_by(CHASE_STEP as usize).chain([det]).map(|t| {
        let u = (t - throw) as f32 / (det - throw).max(1) as f32;
        (t, add(lerp3(p0, p1, u), [0.0, 0.0, 4.0 * apex * u * (1.0 - u)]))
    }).collect();
    path.dedup_by_key(|p| p.0);
    path.windows(2).all(|w| pos.raycast(w[0].1, w[1].1).is_none()).then_some(path)
}

/// Повернуть единичный `from` к `to` не больше чем на `max` радиан.
fn turn_toward(from: [f32; 3], to: [f32; 3], max: f32) -> [f32; 3] {
    let cos = (from[0] * to[0] + from[1] * to[1] + from[2] * to[2]).clamp(-1.0, 1.0);
    let angle = cos.acos();
    if angle <= max {
        return to;
    }
    normalize(lerp3(from, to, max / angle)).unwrap_or(to)
}

struct NadeCam {
    keys: Vec<CamKey>,
    /// Склейка на общий план: с какого тика и его ключи.
    cut: Option<(i32, Vec<CamKey>)>,
}

/// Камера режиссёрского шота на `[t0, t1]`:
/// 1. nade-cam: позади снаряда по ходу полёта и выше, взгляд — на полёт чуть впереди, FOV растёт (скорость);
/// 2. встреча: за `lead` до взрыва камера тормозит (скорость плавно до нуля), снаряд уходит в кадр,
///    взгляд переходит на точку взрыва; после взрыва — наезд FOV;
/// 3. облёт дугой к общему плану `wide` (если дуга свободна от стен, иначе склейка).
fn nade_cam(path: &Path, pos: &dyn PositionSource, kind: GrenadeKind, t0: i32, det: i32, t1: i32, wide: Option<&Wide>, tr: f32) -> NadeCam {
    let secs = |s: f32| (s * tr).round() as i32;
    // ближе этого камера к взрыву не подходит
    let keep = if kind == GrenadeKind::Flash { FLASH_KEEP } else { 0.0 };
    let p1 = add(path_at(path, det), [0.0, 0.0, 8.0]);
    let ahead = |t: i32| path_at(path, (t + secs(AHEAD_SECS)).min(det));
    let lead = secs(0.45).min(((det - t0) as f32 * 0.4) as i32).max(1);
    let settle = (det + secs(SETTLE_SECS)).min(t1);
    let dt = CHASE_STEP as f32 / tr;
    let arm_full = CHASE_BACK.hypot(CHASE_UP);

    // 1. за снарядом — до взрыва целиком: из него берётся и разгон для торможения
    let mut chase: Vec<CamKey> = Vec::new();
    let (mut d_prev, mut arm_prev): (Option<[f32; 3]>, Option<f32>) = (None, None);
    let mut ticks: Vec<i32> = (t0..det).step_by(CHASE_STEP as usize).collect();
    ticks.push(det);
    for &t in &ticks {
        let g = path_at(path, t);
        let v = sub(path_at(path, t + 6), path_at(path, t - 6));
        // по вертикали направление мягче: на взлёте камера не уходит под гранату, на падении — не над ней
        let want = normalize([v[0], v[1], 0.5 * v[2]]).or(d_prev).unwrap_or([1.0, 0.0, 0.0]);
        let d = d_prev.map_or(want, |p| turn_toward(p, want, CHASE_TURN.to_radians() * dt));
        d_prev = Some(d);
        let off = normalize(add(scale(d, -CHASE_BACK), [0.0, 0.0, CHASE_UP])).unwrap_or([0.0, 0.0, 1.0]);
        // пружинная рука: у стены — сразу короче, отошли от стены — отрастает плавно
        let avail = dist(pull(pos, g, add(g, scale(off, arm_full))), g);
        let arm = match arm_prev {
            Some(a) if avail > a => (a + ARM_GROW * dt).min(avail),
            _ => avail,
        };
        arm_prev = Some(arm);
        let cam = add(g, scale(off, arm));
        let u = (t - t0) as f32 / (det - t0).max(1) as f32;
        // взгляд вперёд по полёту на отскоке поворачивает резко — тоже ограничиваем скорость поворота
        let mut ang = look_at(cam, ahead(t));
        if let Some(prev) = chase.last() {
            let max = CHASE_TURN * dt;
            for i in 0..2 {
                let d = (ang[i] - prev.ang[i] + 540.0).rem_euclid(360.0) - 180.0;
                ang[i] = prev.ang[i] + d.clamp(-max, max);
            }
        }
        chase.push(CamKey { tick: t, pos: cam, ang, fov: 80.0 + 15.0 * smooth(2.0 * u) });
    }

    // 2. встреча: от позиции на `meet` камера тормозит до нуля за `lead` (x = c0 + Δ·(2s − s²) —
    //    скорость в начале как у nade-cam, в конце ноль); если с точки остановки взрыва не видно —
    //    летим за снарядом до самого взрыва и встаём там
    let meet = det - lead;
    let mut i_meet = chase.iter().position(|k| k.tick >= meet).unwrap_or(chase.len() - 1);
    // флешка засвечивает кадр вблизи — тормозим заранее, пока камера ещё далеко от хлопка
    if keep > 0.0 {
        i_meet = chase[..=i_meet].iter().rposition(|k| dist(k.pos, p1) >= keep + 60.0).unwrap_or(0);
    }
    let c0 = chase[i_meet].pos;
    let v0 = match i_meet.checked_sub(1).map(|i| &chase[i]) {
        Some(prev) => scale(sub(c0, prev.pos), 1.0 / (chase[i_meet].tick - prev.tick).max(1) as f32),
        None => [0.0; 3],
    };
    let span = (det - chase[i_meet].tick).max(1);
    let mut travel = scale(v0, span as f32 / 2.0);
    // тормозной путь — не ближе `keep` к взрыву
    for _ in 0..12 {
        if keep <= 0.0 || dist(add(c0, travel), p1) >= keep {
            break;
        }
        travel = scale(travel, 0.7);
    }
    let stop = pull(pos, c0, add(c0, travel));
    let brake = pos.raycast(stop, p1).is_none() && pos.raycast(p1, stop).is_none();
    let mut keys: Vec<CamKey> = Vec::new();
    let hold = if brake {
        keys.extend_from_slice(&chase[..i_meet]);
        let m0 = chase[i_meet].tick;
        for t in (m0..det).step_by(CHASE_STEP as usize) {
            let s = (t - m0) as f32 / span as f32;
            let cam = lerp3(c0, stop, 2.0 * s - s * s);
            keys.push(CamKey { tick: t, pos: cam, ang: look_at(cam, lerp3(ahead(t), p1, smooth(s))), fov: 95.0 });
        }
        stop
    } else {
        keys.extend(chase.iter().filter(|k| k.tick < det).cloned());
        chase.last().map_or(stop, |k| k.pos)
    };
    // на взрыве и сразу после: стоим и наезжаем
    for t in (det..settle).step_by(CHASE_STEP as usize).chain([settle]) {
        let e = smooth((t - det) as f32 / (settle - det).max(1) as f32);
        keys.push(CamKey { tick: t, pos: hold, ang: look_at(hold, p1), fov: lerp(95.0, 75.0, e) });
    }
    keys.dedup_by_key(|k| k.tick);
    if settle >= t1 {
        return NadeCam { keys, cut: None };
    }

    // 3. облёт от точки остановки к общему плану
    let step = CHASE_STEP as usize * 2;
    let mut tail: Vec<i32> = (settle + step as i32..t1).step_by(step).collect();
    tail.push(t1);
    let Some(w) = wide else {
        // общего плана нет: стоим и медленно наезжаем на взрыв
        for &t in &tail {
            let e = smooth((t - settle) as f32 / (t1 - settle).max(1) as f32);
            keys.push(CamKey { tick: t, pos: hold, ang: look_at(hold, p1), fov: lerp(75.0, 65.0, e) });
        }
        return NadeCam { keys, cut: None };
    };
    let (o0, o1) = (sub(hold, w.target), sub(w.cam, w.target));
    let (az0, az1) = (o0[1].atan2(o0[0]), o1[1].atan2(o1[0]));
    let daz = (az1 - az0 + std::f32::consts::PI).rem_euclid(2.0 * std::f32::consts::PI) - std::f32::consts::PI;
    let (r0, r1) = (o0[0].hypot(o0[1]), o1[0].hypot(o1[1]));
    let mut orbit = Vec::with_capacity(tail.len());
    let mut prev = hold;
    let mut clear = r0 >= 40.0 && daz.abs() <= 60f32.to_radians();
    for &t in &tail {
        if !clear {
            break;
        }
        let e = smooth((t - settle) as f32 / (t1 - settle).max(1) as f32);
        let (az, r) = (az0 + daz * e, lerp(r0, r1, e));
        let cam = add(w.target, [r * az.cos(), r * az.sin(), lerp(o0[2], o1[2], e)]);
        let target = lerp3(p1, w.target, e);
        let ang = look_at(cam, target);
        // дуга свободна от стен и камера не задирается в небо (иначе в кадре только небо и пальмы)
        clear = pos.raycast(prev, cam).is_none() && pos.raycast(cam, target).is_none() && ang[0] > -20.0;
        prev = cam;
        orbit.push(CamKey { tick: t, pos: cam, ang, fov: lerp(75.0, w.fov, e) });
    }
    if clear {
        keys.extend(orbit);
        return NadeCam { keys, cut: None };
    }
    // склейка: общий план с лёгким дрейфом вокруг группы
    let r = o1[0].hypot(o1[1]);
    let wide_keys = std::iter::once(settle)
        .chain(tail)
        .map(|t| {
            let drift = smooth((t - settle) as f32 / (t1 - settle).max(1) as f32);
            let az = az1 + 8f32.to_radians() * drift;
            let cam = pull(pos, w.target, [w.target[0] + r * az.cos(), w.target[1] + r * az.sin(), w.cam[2]]);
            CamKey { tick: t, pos: cam, ang: look_at(cam, w.target), fov: w.fov }
        })
        .collect();
    NadeCam { keys, cut: Some((settle, wide_keys)) }
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
    // сбалансированный монтаж: белый экран ослеплённого или вспышка у раненого без убийства — пустые секунды
    if victims.is_empty() && cfg.density == Density::Balanced {
        return Vec::new();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const THROW: i32 = 1000;
    const BOUNCE: i32 = 1064;
    const DET: i32 = 1128;

    /// Флешка: 1 с дугой вдоль +x до (800, 0), отскок, ещё 1 с вдоль +y до (800, 300).
    /// Бросающий (1) в начале координат, ослеплённый (2) — у точки взрыва.
    fn flight() -> Vec<(i32, [f32; 3])> {
        (THROW..=DET)
            .step_by(2)
            .map(|t| {
                let p = if t <= BOUNCE {
                    let u = (t - THROW) as f32 / (BOUNCE - THROW) as f32;
                    [800.0 * u, 0.0, 64.0 * (1.0 - u) + 300.0 * u * (1.0 - u)]
                } else {
                    let u = (t - BOUNCE) as f32 / (DET - BOUNCE) as f32;
                    [800.0, 300.0 * u, 60.0 * u * (1.0 - u)]
                };
                (t, p)
            })
            .collect()
    }

    /// `wall_x` — бесконечная стена-плоскость x = const.
    struct Scene {
        path: Option<Vec<(i32, [f32; 3])>>,
        wall_x: Option<f32>,
    }

    impl PositionSource for Scene {
        fn eye_pose(&self, p: PlayerId, _: i32) -> Option<Pose> {
            match p.0 {
                1 => Some(Pose { pos: [0.0, 0.0, 64.0], ang: [0.0, 0.0, 0.0] }),
                2 => Some(Pose { pos: [900.0, 320.0, 64.0], ang: [0.0, 180.0, 0.0] }),
                _ => None,
            }
        }
        fn raycast(&self, a: [f32; 3], b: [f32; 3]) -> Option<f32> {
            let x = self.wall_x?;
            ((a[0] - x) * (b[0] - x) < 0.0).then(|| (x - a[0]) / (b[0] - a[0]))
        }
        fn grenade_path(&self, _: &cs2::Grenade) -> Option<Vec<(i32, [f32; 3])>> {
            self.path.clone()
        }
    }

    fn setup(kind: GrenadeKind) -> (Match, Utility) {
        let m = Match::build("de_test".into(), 64.0, vec![(1, 0, "ace".into(), 1), (2, 1, "bob".into(), 2)], &[]);
        let grenade = cs2::Grenade {
            kind,
            thrower: PlayerId(1),
            round: 3,
            throw_tick: Some(THROW),
            detonate_tick: DET,
            pos: [800.0, 300.0, 0.0],
            damage: if kind == GrenadeKind::Flash { 0 } else { 87 },
            hurt: if kind == GrenadeKind::Flash { vec![] } else { vec![PlayerId(2)] },
            blinded: if kind == GrenadeKind::Flash { vec![(PlayerId(2), 2.43)] } else { vec![] },
            kills: vec![],
        };
        (m, Utility { grenade, score: 20.0, tags: vec!["флешка".into()] })
    }

    /// Подробный монтаж: бросок от первого лица отдельным участком перед nade-cam.
    fn full() -> Config {
        Config { density: Density::Full, ..Config::default() }
    }

    fn free_keys(shot: &Shot) -> Vec<CamKey> {
        shot.segments.iter().flat_map(|s| match &s.camera {
            Camera::Free { keys } => keys.clone(),
            _ => vec![],
        }).collect()
    }

    #[test]
    fn camera_chases_the_real_grenade_then_brakes_before_blast() {
        let (m, u) = setup(GrenadeKind::He);
        let pos = Scene { path: Some(flight()), wall_x: None };
        let shot = shoot_grenade(&m, &u, &full(), &pos).unwrap();
        assert!(matches!(shot.segments[0].camera, Camera::Pov { .. }), "бросок от первого лица");
        assert_eq!(shot.segments[1].style.as_deref(), Some("nade_cam"));
        let keys = free_keys(&shot);
        let path = flight();
        let release = THROW + 10;
        let lead = (0.45f32 * 64.0).round() as i32;
        // nade-cam: позади снаряда по ходу полёта, рядом, снаряд в кадре
        for k in keys.iter().filter(|k| k.tick >= release && k.tick < DET - lead) {
            let g = path_at(&path, k.tick);
            let d = dist(k.pos, g);
            assert!((30.0..=80.0).contains(&d), "тик {}: до гранаты {d}", k.tick);
            assert!(rig::visibility(k, g) > 0.5, "тик {}: граната вне кадра", k.tick);
        }
        // до отскока камера позади по +x
        let k = keys.iter().find(|k| k.tick == 1040).unwrap();
        assert!(k.pos[0] < path_at(&path, 1040)[0] - 30.0, "{k:?}");
        // отскок: разворот камеры не быстрее CHASE_TURN (+ запас на взгляд вперёд)
        for w in keys.windows(2).filter(|w| w[0].tick >= release && w[1].tick < DET - lead) {
            let dyaw = (w[1].ang[1] - w[0].ang[1] + 540.0).rem_euclid(360.0) - 180.0;
            let rate = dyaw.abs() / ((w[1].tick - w[0].tick) as f32 / 64.0);
            assert!(rate < 2.0 * CHASE_TURN, "{rate}°/с на тике {}", w[1].tick);
        }
        // встреча: к взрыву камера останавливается, а граната — у взрыва в кадре
        // скорость камеры (юнитов за тик) у первого ключа с тиком ≥ t
        let speed = |t: i32| {
            let i = keys.iter().position(|k| k.tick >= t).unwrap();
            dist(keys[i - 1].pos, keys[i].pos) / (keys[i].tick - keys[i - 1].tick) as f32
        };
        assert!(speed(DET - lead + 4) > 3.0 * speed(DET - 2), "{} -> {}", speed(DET - lead + 4), speed(DET - 2));
        let at_det = keys.iter().find(|k| k.tick == DET).unwrap();
        assert!(rig::visibility(at_det, [800.0, 300.0, 8.0]) > 0.9);
        assert!(at_det.fov > 90.0, "на взрыве широкий кадр, дальше наезд");
        for s in &shot.segments {
            if let Camera::Free { keys } = &s.camera {
                assert!(keys.windows(2).all(|w| w[0].tick < w[1].tick), "тики по возрастанию: {:?}", keys.iter().map(|k| k.tick).collect::<Vec<_>>());
            }
        }
        assert_eq!(keys.last().unwrap().tick, shot.end_tick);
    }

    #[test]
    fn flash_is_watched_from_a_distance() {
        // вблизи хлопок засвечивает весь кадр: камера встаёт не ближе FLASH_KEEP и видит его
        let (m, u) = setup(GrenadeKind::Flash);
        let pos = Scene { path: Some(flight()), wall_x: None };
        let shot = shoot_grenade(&m, &u, &full(), &pos).unwrap();
        let keys = free_keys(&shot);
        let blast = [800.0, 300.0, 8.0];
        let at_det = keys.iter().find(|k| k.tick == DET).unwrap();
        assert!(dist(at_det.pos, blast) >= FLASH_KEEP - 1.0, "{}", dist(at_det.pos, blast));
        assert!(rig::visibility(at_det, blast) > 0.9);
        // пока хлопок яркий (до конца наезда) — не ближе; облёт к ослеплённым потом может подойти
        for k in keys.iter().filter(|k| (DET - 20..DET + 32).contains(&k.tick)) {
            assert!(dist(k.pos, blast) >= FLASH_KEEP - 1.0, "тик {}: {}", k.tick, dist(k.pos, blast));
        }
    }

    #[test]
    fn long_flight_is_sped_up_and_blast_is_slow() {
        let (m, u) = setup(GrenadeKind::Flash);
        let pos = Scene { path: Some(flight()), wall_x: None };
        let shot = shoot_grenade(&m, &u, &full(), &pos).unwrap();
        let ts = &shot.timescale;
        assert!(ts.iter().any(|k| k.scale > 1.3), "{ts:?}");
        let slow = ts.iter().filter(|k| k.tick >= DET - 10 && k.tick <= DET + 30).map(|k| k.scale).fold(1.0, f32::min);
        assert!(slow <= 0.3, "{ts:?}");
        assert!(ts.windows(2).all(|w| w[0].tick < w[1].tick));
        // короткий полёт — без ускорения
        let mut short = u.clone();
        short.grenade.throw_tick = Some(DET - 50);
        let path: Vec<_> = flight().into_iter().filter(|p| p.0 >= DET - 50).collect();
        let shot = shoot_grenade(&m, &short, &Config::default(), &Scene { path: Some(path), wall_x: None }).unwrap();
        assert!(shot.timescale.iter().all(|k| k.scale <= 1.0), "{:?}", shot.timescale);
    }

    #[test]
    fn camera_never_goes_through_walls() {
        let (m, u) = setup(GrenadeKind::He);
        // стена за точкой отскока (пострадавший — за ней): внутри участка камера её не пересекает,
        // на ту сторону — только склейкой
        let pos = Scene { path: Some(flight()), wall_x: Some(830.0) };
        let shot = shoot_grenade(&m, &u, &full(), &pos).unwrap();
        for s in &shot.segments {
            if let Camera::Free { keys } = &s.camera {
                for w in keys.windows(2) {
                    assert!(pos.raycast(w[0].pos, w[1].pos).is_none(), "сквозь стену: {:?} -> {:?}", w[0], w[1]);
                }
            }
        }
        let chase = match &shot.segments[1].camera {
            Camera::Free { keys } => keys,
            _ => unreachable!(),
        };
        assert!(chase.iter().all(|k| k.pos[0] < 830.0), "за гранатой камера не заходит за стену");
    }

    #[test]
    fn falls_back_to_ballistic_arc_without_flight_data() {
        let (m, u) = setup(GrenadeKind::He);
        let shot = shoot_grenade(&m, &u, &full(), &Scene { path: None, wall_x: None }).unwrap();
        assert_eq!(shot.segments[1].style.as_deref(), Some("nade_cam"));
        // дуга упирается в стену — полёта нет, шота нет (пострадавших покажут их глазами)
        assert!(shoot_grenade(&m, &u, &Config::default(), &Scene { path: None, wall_x: Some(400.0) }).is_none());
    }

    #[test]
    fn blinded_without_a_kill_is_shown_only_in_full_montage() {
        let (m, u) = setup(GrenadeKind::Flash);
        let pos = Scene { path: None, wall_x: None };
        assert!(grenade_victims(&m, &[&u], &Config::default(), &pos, &mut vec![]).is_empty(), "белый экран без убийства — пустые секунды");
        assert_eq!(grenade_victims(&m, &[&u], &full(), &pos, &mut vec![]).len(), 1);
        let mut killed = u.clone();
        killed.grenade.kills.push(PlayerId(2));
        assert_eq!(grenade_victims(&m, &[&killed], &Config::default(), &pos, &mut vec![]).len(), 1);
        // сбалансированный монтаж: без броска от первого лица, сразу за снарядом
        let shot = shoot_grenade(&m, &u, &Config::default(), &Scene { path: Some(flight()), wall_x: None }).unwrap();
        assert_eq!(shot.segments[0].style.as_deref(), Some("nade_cam"));
    }

    #[test]
    fn caption_says_what_the_grenade_did() {
        let (_, mut u) = setup(GrenadeKind::Flash);
        u.grenade.blinded.push((PlayerId(3), 1.6));
        u.grenade.blinded.push((PlayerId(4), 0.7)); // едва мигнуло — не считается
        u.grenade.kills.push(PlayerId(2));
        assert_eq!(grenade_caption(&u.grenade), "Флешка · ослепил 2 · до 2.4 с · 1 убийство под неё");
        let (_, mut u) = setup(GrenadeKind::He);
        u.grenade.kills = vec![PlayerId(2), PlayerId(3)];
        assert_eq!(grenade_caption(&u.grenade), "HE · 87 урона · 2 убийства");
        let (_, u) = setup(GrenadeKind::Fire);
        assert_eq!(grenade_caption(&u.grenade), "Молотов · 87 урона");
    }
}
