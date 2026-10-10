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

    /// Настоящий полёт снаряда гранаты: точки `(тик, позиция)` от броска до взрыва.
    /// По умолчанию неизвестен.
    fn grenade_path(&self, _g: &cs2::Grenade) -> Option<Vec<(i32, [f32; 3])>> {
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

/// Плотность ролика про игрока (`direct_all`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Density {
    /// Весь KAST: каждый раунд с ассистом или разменом, все ракурсы на каждое убийство,
    /// гранаты и глазами всех пострадавших. В ~2 раза длиннее.
    Full,
    /// Только раунды с убийствами (гранаты — если под них убили), не больше одного
    /// дополнительного ракурса на убийство и двух на раунд, короткие отрезки от первого лица.
    #[default]
    Balanced,
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
    pub density: Density,
}

impl Default for Config {
    fn default() -> Self {
        Self { target_secs: 60.0, max_shots: 12, order: Order::BuildUp, slow: 0.4, slow_final: 0.18, cinematic: false, density: Density::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub map: String,
    pub tick_rate: f32,
    /// Игровое время = (тик демки + server_start_tick) / tick_rate; нужно для путей камеры HLAE.
    #[serde(default)]
    pub server_start_tick: i32,
    /// В порядке показа в итоговом ролике.
    pub shots: Vec<Shot>,
}

impl Timeline {
    pub fn total_secs(&self) -> f32 {
        self.shots.iter().map(|s| s.output_secs(self.tick_rate)).sum()
    }
}

/// За сколько секунд после смерти тиммейта ответное убийство — размен.
const TRADE_SECS: f32 = 5.0;
/// Длиннее этого отрезок от первого лица режем по самой долгой паузе между действиями (сбалансированный монтаж), с.
const MAX_CHUNK_SECS: f32 = 9.0;
/// Дополнительных ракурсов на убийства в раунде (сбалансированный монтаж); гранаты и размен — сверх того.
const MAX_ALT_PER_ROUND: usize = 2;
/// Одиночное убийство с оценкой ниже этой — рядовое: без дополнительного ракурса.
const ALT_MIN_SCORE: f32 = 12.0;

/// Дополнительный шот раунда (повтор, глазами противника, граната, размен) и к какому моменту
/// он относится — после отрезка от первого лица с этим тиком он и встанет.
struct Extra {
    anchor: i32,
    priority: f32,
    shot: Shot,
}

/// Ролик про одного игрока (`hero`) по раундам. В каждом раунде:
/// 1. отрезок раунда его глазами — кусками вокруг действий, нормальная скорость;
/// 2. сразу после куска — дополнительные ракурсы на то, что в нём было: повтор со стороны
///    (серии, эффектные убийства), глазами противника (с момента, когда игрок у него появился),
///    полёт гранаты и её взрыв глазами того, кого она решила, размен — глазами тиммейта.
///
/// `Density::Full` — весь KAST (раунды с ассистом или разменом тоже) и все ракурсы;
/// `Density::Balanced` — только раунды с убийствами, не больше одного ракурса на убийство и двух на раунд,
/// лучший раунд — в финале (если порядок не хронологический).
///
/// Шоты перекрываются по времени — запись отматывает демку назад.
pub fn direct_all(m: &Match, hero: PlayerId, highlights: &[Highlight], utility: &[Utility], cfg: &Config, pos: &dyn PositionSource) -> Timeline {
    let mut used: Vec<String> = Vec::new();
    if !cfg.cinematic {
        let mut shots: Vec<Shot> = highlights.iter().map(|h| shoot(m, h, cfg, pos, &mut used)).collect();
        shots.extend(utility.iter().filter_map(|u| shoot_grenade(m, u, cfg, pos)));
        shots.sort_by_key(|s| s.start_tick);
        return Timeline { map: m.map.clone(), tick_rate: m.tick_rate, server_start_tick: m.server_start_tick, shots };
    }
    let full = cfg.density == Density::Full;
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let name = m.name(hero);
    // тиммейт — по командам из демки; без них размены не ищем
    let mate = |p: PlayerId, tick: i32| p != hero && matches!((m.team_at(hero, tick), m.team_at(p, tick)), (Some(a), Some(b)) if a == b);
    // ассисты по урону (гранатные — отдельно, через utility)
    let assists: Vec<&cs2::Kill> = m.kills.iter().filter(|k| k.assister == Some(hero) && k.attacker != hero && !k.assisted_flash).collect();
    // размены: моя смерть и ответное убийство тиммейта в ближайшие секунды
    let trades: Vec<(&cs2::Kill, &cs2::Kill)> = m
        .kills
        .iter()
        .filter(|k| k.victim == hero)
        .filter_map(|death| {
            let trade = m
                .kills
                .iter()
                .filter(|x| x.victim == death.attacker && mate(x.attacker, x.tick) && x.tick >= death.tick && x.tick - death.tick <= secs(TRADE_SECS))
                .min_by_key(|x| x.tick)?;
            Some((death, trade))
        })
        .collect();

    // раунд попадает в ролик: с убийством — всегда; с гранатой — если под неё убили (Full: любая важная);
    // ассист и размен сами по себе — только в Full
    let mut rounds: Vec<u32> = highlights
        .iter()
        .map(|h| h.round)
        .chain(utility.iter().filter(|u| full || !u.grenade.kills.is_empty()).map(|u| u.grenade.round))
        .chain(assists.iter().filter(|_| full).map(|k| k.round))
        .chain(trades.iter().filter(|_| full).map(|(d, _)| d.round))
        .collect();
    rounds.sort_unstable();
    rounds.dedup();

    let (pre, post) = if full { (2.5, 1.5) } else { (1.8, 1.2) };
    let gap = secs(if full { 5.0 } else { 3.0 });
    let mut blocks: Vec<(f32, Vec<Shot>)> = Vec::new();
    for round in rounds {
        let mut hs: Vec<&Highlight> = highlights.iter().filter(|h| h.round == round).collect();
        hs.sort_by_key(|h| h.first_tick());
        let mut us: Vec<&Utility> = utility.iter().filter(|u| u.grenade.round == round).collect();
        us.sort_by_key(|u| u.grenade.detonate_tick);
        let assists_here: Vec<&cs2::Kill> = assists.iter().copied().filter(|k| k.round == round).collect();
        let trades_here: Vec<(&cs2::Kill, &cs2::Kill)> = trades.iter().copied().filter(|(d, _)| d.round == round).collect();
        // гранаты залпом (две поп-флешки подряд) — один полёт и один набор вставок
        let volleys = volleys(&us, secs(3.0));

        // 1. раунд глазами игрока — кусками вокруг действий, пустой бег между ними вырезаем;
        // раньше конца фриза (с запасом: на старте раунда экран выходит из затемнения) не начинаем
        let floor = m.round(round).map_or(0, |r| r.freeze_end_tick.map_or(r.start_tick + secs(1.5), |f| f + secs(0.5)));
        let mut spans: Vec<(i32, i32)> = hs
            .iter()
            .flat_map(|h| h.kills.iter().map(|k| (k.tick - secs(pre), k.tick + secs(post))))
            .chain(us.iter().map(|u| {
                let g = &u.grenade;
                let throw = g.throw_tick.unwrap_or(g.detonate_tick - secs(1.5));
                if full { (throw - secs(1.0), g.detonate_tick + secs(1.2)) } else { (throw - secs(0.5), g.detonate_tick + secs(1.0)) }
            }))
            .chain(assists_here.iter().map(|k| (k.tick - secs(pre), k.tick + secs(post))))
            .chain(trades_here.iter().map(|(d, _)| (d.tick - secs(pre), d.tick + secs(0.6))))
            .map(|(a, b)| (a.max(floor), b))
            .collect();
        spans.sort_unstable();
        let chunks = chunk_spans(&spans, gap, if full { i32::MAX } else { secs(MAX_CHUNK_SECS) });
        let kills: usize = hs.iter().map(|h| h.kills.len()).sum();
        let mut summary: Vec<String> = Vec::new();
        if kills > 0 {
            summary.push(plural(kills, "убийство", "убийства", "убийств"));
        }
        if !assists_here.is_empty() {
            summary.push(plural(assists_here.len(), "ассист", "ассиста", "ассистов"));
        }
        let flashes = us.iter().filter(|u| u.grenade.kind == cs2::GrenadeKind::Flash).count();
        let other = us.len() - flashes;
        if flashes > 0 {
            summary.push(plural(flashes, "флешка", "флешки", "флешек"));
        }
        if other > 0 {
            summary.push(plural(other, "граната", "гранаты", "гранат"));
        }
        if !trades_here.is_empty() {
            summary.push("размен".to_owned());
        }
        let tier = hs.iter().map(|h| tier_of(h.score)).max_by_key(|t| *t as u8).unwrap_or(Tier::Good);
        let score = hs.iter().map(|h| h.score).fold(0.0, f32::max);
        let pov_chunks: Vec<Shot> = chunks
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| Shot {
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
                // плашка раунда на первом куске говорит, чей это раунд; подпись не дублируем
                caption: if full { format!("Раунд {round} · глазами {name}") } else { String::new() },
                card: (i == 0).then(|| (round_title(m, hero, round), summary.join(" · "))),
            })
            .collect();

        // 2. дополнительные ракурсы: на убийство — повтор со стороны (серии, эффектные) или глазами противника
        let mut alts: Vec<Extra> = Vec::new();
        for h in &hs {
            for k in &h.kills {
                let (ks, _) = score_kill(k);
                let priority = ks + if h.kills.len() >= 2 { 10.0 } else { 0.0 };
                let replay = is_strong(h).then(|| replay_shot(m, h, k, cfg, pos, &mut used)).flatten();
                let victim = || victim_shot(m, h, k, pos, cfg, full);
                if full {
                    alts.extend(replay.map(|shot| Extra { anchor: k.tick, priority, shot }));
                    alts.extend(victim().map(|shot| Extra { anchor: k.tick, priority, shot }));
                } else if let Some(shot) = replay.or_else(|| (ks >= ALT_MIN_SCORE || h.kills.len() >= 2).then(victim).flatten()) {
                    alts.push(Extra { anchor: k.tick, priority, shot });
                }
            }
        }
        if !full {
            alts.sort_by(|a, b| b.priority.total_cmp(&a.priority));
            alts.truncate(MAX_ALT_PER_ROUND);
        }
        // гранаты: полёт за той, что сделала больше, и взрыв глазами того, кого она решила;
        // в сбалансированном монтаже флешка без убийства под неё полёта не заслуживает
        for v in &volleys {
            let best = v.iter().max_by(|a, b| a.score.total_cmp(&b.score)).expect("залп не пуст");
            let det = best.grenade.detonate_tick;
            let flight = full || !best.grenade.kills.is_empty() || matches!(best.grenade.kind, cs2::GrenadeKind::He | cs2::GrenadeKind::Fire);
            if flight {
                alts.extend(shoot_grenade(m, best, cfg, pos).map(|shot| Extra { anchor: det, priority: best.score, shot }));
            }
            alts.extend(grenade_victims(m, v, cfg, pos, &mut used).into_iter().map(|shot| Extra { anchor: det, priority: best.score, shot }));
        }
        // размен — глазами тиммейта, который отомстил
        alts.extend(trades_here.iter().map(|(d, x)| {
            let (a, b) = (x.tick - secs(1.5), x.tick + secs(0.5));
            let shot = Shot {
                title: format!("{} разменивает {} за {name}", m.name(x.attacker), m.name(x.victim)),
                tier: Tier::Good,
                score: 0.0,
                player: x.attacker,
                round,
                start_tick: a,
                end_tick: b,
                segments: vec![Segment { start_tick: a, end_tick: b, camera: Camera::Pov { player: x.attacker }, style: Some("trade_pov".into()), colorfulness: None }],
                timescale: vec![],
                zoom: vec![],
                caption: format!("Размен · {} за {name}", m.name(x.attacker)),
                card: None,
            };
            Extra { anchor: d.tick, priority: 0.0, shot }
        }));
        blocks.push((score, interleave(pov_chunks, alts)));
    }
    // финал — лучший раунд (в хронологическом порядке — как есть)
    if !full && cfg.order != Order::Chronological && blocks.len() > 1 {
        if let Some(best) = blocks.iter().enumerate().max_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(i, _)| i) {
            let b = blocks.remove(best);
            blocks.push(b);
        }
    }
    let shots = blocks.into_iter().flat_map(|(_, s)| s).collect();
    Timeline { map: m.map.clone(), tick_rate: m.tick_rate, server_start_tick: m.server_start_tick, shots }
}

/// Отрезки действий -> куски от первого лица: соседние (пауза ≤ `gap`) сливаются, кусок длиннее
/// `max_len` режется по самой долгой паузе внутри (пока режется).
fn chunk_spans(spans: &[(i32, i32)], gap: i32, max_len: i32) -> Vec<(i32, i32)> {
    let mut groups: Vec<Vec<(i32, i32)>> = Vec::new();
    for &(a, b) in spans {
        match groups.last_mut() {
            Some(g) if a - g.last().map_or(a, |l| l.1) <= gap => g.push((a, b)),
            _ => groups.push(vec![(a, b)]),
        }
    }
    let mut out = Vec::new();
    let mut queue: Vec<Vec<(i32, i32)>> = groups.into_iter().rev().collect();
    while let Some(g) = queue.pop() {
        let (from, to) = (g[0].0, g.iter().map(|s| s.1).max().unwrap_or(g[0].1));
        // самая долгая пауза между соседними отрезками (конец предыдущего — начало следующего)
        let cut = g
            .windows(2)
            .enumerate()
            .map(|(i, w)| (w[1].0 - w[0].1, i))
            .filter(|(d, _)| *d > 0)
            .max();
        match cut {
            Some((_, i)) if to - from > max_len => {
                let (left, right) = g.split_at(i + 1);
                queue.push(right.to_vec());
                queue.push(left.to_vec());
            }
            _ => out.push((from, to)),
        }
    }
    out
}

/// Куски от первого лица по порядку, после каждого — ракурсы на то, что в нём было (по тику).
/// Ракурс между кусками встаёт после предыдущего, раньше первого — после первого.
fn interleave(chunks: Vec<Shot>, mut extras: Vec<Extra>) -> Vec<Shot> {
    extras.sort_by_key(|e| e.anchor);
    let mut slots: Vec<Vec<Shot>> = (0..chunks.len()).map(|_| Vec::new()).collect();
    for e in extras {
        let i = chunks.iter().rposition(|c| c.start_tick <= e.anchor).unwrap_or(0);
        slots[i].push(e.shot);
    }
    chunks.into_iter().zip(slots).flat_map(|(c, s)| std::iter::once(c).chain(s)).collect()
}

/// Заставки ролика про игрока: интро «Sihnel / de_dust2 · 23 убийства · победа 13:11»
/// и итоги «Sihnel / 23-15-4 · HS 48 % · K/D 1.53».
pub fn cards(m: &Match, hero: PlayerId) -> ((String, String), (String, String)) {
    let r = cs2::report::player_report(m, hero);
    let name = m.name(hero);
    let (won, lost) = r.rounds.iter().fold((0, 0), |(w, l), row| match row.won {
        Some(true) => (w + 1, l),
        Some(false) => (w, l + 1),
        None => (w, l),
    });
    let result = match (won + lost > 0, won.cmp(&lost)) {
        (false, _) => None,
        (_, std::cmp::Ordering::Greater) => Some(format!("победа {won}:{lost}")),
        (_, std::cmp::Ordering::Less) => Some(format!("поражение {won}:{lost}")),
        (_, std::cmp::Ordering::Equal) => Some(format!("ничья {won}:{lost}")),
    };
    let mut intro = vec![m.map.clone(), plural(r.duels.kills as usize, "убийство", "убийства", "убийств")];
    intro.extend(result.clone());
    let mut outro = vec![format!("{}-{}-{}", r.duels.kills, r.duels.deaths, r.duels.assists), format!("HS {:.0} %", r.duels.hs_pct()), format!("K/D {:.2}", r.duels.kd())];
    outro.extend(result);
    ((name.clone(), intro.join(" · ")), (name, outro.join(" · ")))
}

/// «Раунд 12 · CT · 7:4»: сторона и счёт героя перед раундом, если демка сообщила команды и победителей.
fn round_title(m: &Match, hero: PlayerId, round: u32) -> String {
    let Some(r) = m.round(round) else { return format!("Раунд {round}") };
    let Some(team) = m.team_at(hero, r.start_tick) else { return format!("Раунд {round}") };
    let side = match team {
        2 => "T",
        3 => "CT",
        _ => return format!("Раунд {round}"),
    };
    let (mut mine, mut theirs) = (0, 0);
    for prev in m.rounds.iter().filter(|p| p.number < round) {
        match (prev.winner_team, m.team_at(hero, prev.start_tick)) {
            (Some(w), Some(t)) if w == t => mine += 1,
            (Some(_), Some(_)) => theirs += 1,
            _ => {}
        }
    }
    format!("Раунд {round} · {side} · {mine}:{theirs}")
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

/// Окно `[a, b]` на скорости `v` с плавным входом и выходом за `ramp` тиков.
fn slow_window(a: i32, b: i32, v: f32, ramp: i32) -> Vec<ScaleKey> {
    slow_windows(&[(a, b)], v, ramp)
}

/// Несколько окон замедления; соседние (ближе чем две рампы) сливаются в одно.
fn slow_windows(windows: &[(i32, i32)], v: f32, ramp: i32) -> Vec<ScaleKey> {
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for &(a, b) in windows {
        match merged.last_mut() {
            Some(last) if a <= last.1 + 2 * ramp => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
        .into_iter()
        .flat_map(|(a, b)| {
            [
                ScaleKey { tick: a - ramp, scale: 1.0 },
                ScaleKey { tick: a, scale: v },
                ScaleKey { tick: b, scale: v },
                ScaleKey { tick: b + ramp, scale: 1.0 },
            ]
        })
        .collect()
}

/// Повтор убийства со стороны: свободная камера, на которой видно и убийцу, и жертву; сам выстрел — в замедлении.
fn replay_shot(m: &Match, h: &Highlight, k: &cs2::Kill, cfg: &Config, pos: &dyn PositionSource, used: &mut Vec<String>) -> Option<Shot> {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let (start, end) = (k.tick - secs(1.4), k.tick + secs(0.9));
    let scene = rig::Scene { killer: h.player, victim: k.victim, focus_tick: k.tick, tick_rate: tr };
    // приближение — только на дальних дуэлях, иначе оно мешает
    let pick = rig::best_replay(pos, &scene, start, end, used, k.distance >= FAR_DUEL_METERS)?;
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
        timescale: slow_window(k.tick - secs(0.25), k.tick + secs(0.2), cfg.slow, secs(0.15)),
        zoom: vec![],
        caption: format!("Повтор · {} против {}", m.name(h.player), m.name(k.victim)),
        card: None,
    })
}

/// Дуэль дальше этого (метры) — дальняя: там приближение помогает разглядеть игрока.
const FAR_DUEL_METERS: f32 = 25.0;
/// Сколько держим кадр глазами противника после попадания: смерть видна, камера ещё не упала в пол, с.
const VICTIM_AFTER_SECS: f32 = 0.3;

/// Мой пик глазами противника: с момента, когда я у него появляюсь, и до его смерти (чуть после
/// попадания, пока камера не упала в пол); замедление — на появлении и на самом попадании.
/// Если он меня так и не увидел и `shoulder` — камера из-за его плеча, развёрнутая туда, откуда
/// я вышел. `None` — показать нечем.
fn victim_shot(m: &Match, h: &Highlight, k: &cs2::Kill, pos: &dyn PositionSource, cfg: &Config, shoulder: bool) -> Option<Shot> {
    let tr = m.tick_rate;
    let secs = |s: f32| (s * tr).round() as i32;
    let ramp = secs(0.15);
    let base = |start: i32, end: i32, segment: Segment, timescale: Vec<ScaleKey>, caption: String| Shot {
        title: format!("{} -> {}: глазами противника", m.name(h.player), m.name(k.victim)),
        tier: tier_of(h.score),
        score: h.score,
        player: k.victim,
        round: h.round,
        start_tick: start,
        end_tick: end,
        segments: vec![segment],
        timescale,
        zoom: vec![],
        caption,
        card: None,
    };
    let end = k.tick + secs(VICTIM_AFTER_SECS);
    // попадание — в замедлении, чтобы смерть читалась
    let hit = (k.tick - secs(0.15), k.tick + secs(0.2));
    // момент, когда я появляюсь у противника: в поле зрения и без стены между нами
    if let Some(peek) = peek_tick(pos, k.victim, h.player, k.tick - secs(2.5), k.tick) {
        let start = (peek - secs(1.0)).min(k.tick - secs(1.6));
        // замедление на появлении; без приближения — на дальних дуэлях противник и так в прицеле
        let reveal = (peek - secs(0.1), (peek + secs(0.7)).min(hit.0));
        let seg = Segment { start_tick: start, end_tick: end, camera: Camera::Pov { player: k.victim }, style: Some("victim_pov".into()), colorfulness: None };
        return Some(base(start, end, seg, slow_windows(&[reveal, hit], cfg.slow, ramp), format!("Глазами {}", m.name(k.victim))));
    }
    if !shoulder {
        return None;
    }
    // противник меня не увидел: из-за его плеча, камера смотрит на меня
    let start = k.tick - secs(1.6);
    let keys = shoulder_keys(pos, k.victim, h.player, start, end, k.tick, tr)?;
    let seg = Segment { start_tick: start, end_tick: end, camera: Camera::Free { keys }, style: Some("victim_shoulder".into()), colorfulness: None };
    Some(base(start, end, seg, slow_window(k.tick - secs(0.45), hit.1, cfg.slow, ramp), format!("Из-за плеча {}", m.name(k.victim))))
}

/// Камера из-за плеча `viewer`, повёрнутая на `target`, на `[t0, t1]`; после `freeze`
/// (смерти) зритель не двигается. `None`, если в момент `freeze` цель из-за плеча не видна.
fn shoulder_keys(pos: &dyn PositionSource, viewer: PlayerId, target: PlayerId, t0: i32, t1: i32, freeze: i32, tr: f32) -> Option<Vec<CamKey>> {
    let step = ((0.1 * tr) as usize).max(1);
    let mut ticks: Vec<i32> = (t0..t1).step_by(step).collect();
    ticks.extend([t1, freeze]);
    ticks.sort_unstable();
    ticks.dedup();
    let chest = |p: Pose| [p.pos[0], p.pos[1], p.pos[2] - 10.0];
    let mut keys = Vec::with_capacity(ticks.len());
    let mut prev: Option<[f32; 3]> = None;
    for t in ticks {
        let v = pos.eye_pose(viewer, t.min(freeze)).or_else(|| pos.eye_pose(viewer, freeze))?;
        let p = pos.eye_pose(target, t).or_else(|| pos.eye_pose(target, freeze))?;
        let want = grenade::shoulder(pos, &v, [p.pos[0] - v.pos[0], p.pos[1] - v.pos[1], 0.0]);
        // не проходим сквозь стены между соседними ключами
        let cam = match prev {
            Some(c) if pos.raycast(c, want).is_some() => c,
            _ => want,
        };
        if t == freeze && pos.raycast(cam, chest(p)).is_some() {
            return None;
        }
        keys.push(CamKey { tick: t, pos: cam, ang: look_at(cam, chest(p)), fov: 90.0 });
        prev = Some(cam);
    }
    Some(keys)
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
    Timeline { map: m.map.clone(), tick_rate: m.tick_rate, server_start_tick: m.server_start_tick, shots }
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

    // 4. вид «глазами игрока» и «из-за спины» — тоже пути камеры по позициям: так запись не зависит
    // от команд наблюдателя, а зум вшит прямо в FOV ключей. Без позиций остаётся Pov/Chase.
    let mut segments = segments;
    let mut all_free = true;
    for seg in &mut segments {
        let (mode, name) = match seg.camera {
            Camera::Pov { .. } => (Follow::Pov, "pov"),
            Camera::Chase { .. } => (Follow::Chase, "chase"),
            _ => continue,
        };
        match follow_keys(pos, h.player, seg.start_tick, seg.end_tick, mode, tr, &zoom) {
            Some(keys) => {
                seg.camera = Camera::Free { keys };
                seg.style = Some(name.to_owned());
            }
            None => all_free = false,
        }
    }
    let zoom = if all_free { Vec::new() } else { zoom };

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

/// Обычный FOV, от которого считается зум (множитель из кривой `zoom`).
pub const BASE_FOV: f32 = 90.0;
/// Шаг ключей пути «за игроком»: 2 тика = 32 ключа в секунду, камера повторяет движения мыши.
const FOLLOW_STEP: i32 = 2;
/// Камеру «из глаз» чуть выносим вперёд, чтобы не видеть изнутри собственную голову.
const POV_FORWARD: f32 = 5.0;
const CHASE_BACK: f32 = 110.0;
const CHASE_UP: f32 = 22.0;
const CHASE_WALL_MARGIN: f32 = 16.0;

#[derive(Clone, Copy)]
enum Follow {
    Pov,
    Chase,
}

/// Единичный вектор взгляда Source: pitch положителен вниз.
fn forward(ang: [f32; 3]) -> [f32; 3] {
    let (p, y) = (ang[0].to_radians(), ang[1].to_radians());
    [p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()]
}

/// Путь камеры, повторяющий взгляд игрока (`Pov`) или идущий за его плечом (`Chase`).
/// `None`, если для какого-то кадра нет позиции игрока.
fn follow_keys(
    pos: &dyn PositionSource,
    player: PlayerId,
    a: i32,
    b: i32,
    mode: Follow,
    tick_rate: f32,
    zoom: &[ScaleKey],
) -> Option<Vec<CamKey>> {
    let _ = tick_rate;
    if b <= a {
        return None;
    }
    let mut ticks: Vec<i32> = (a..b).step_by(FOLLOW_STEP as usize).collect();
    ticks.push(b);
    let mut keys = Vec::with_capacity(ticks.len());
    for t in ticks {
        let eye = pos.eye_pose(player, t)?;
        let dir = forward(eye.ang);
        let cam = match mode {
            Follow::Pov => [
                eye.pos[0] + dir[0] * POV_FORWARD,
                eye.pos[1] + dir[1] * POV_FORWARD,
                eye.pos[2] + dir[2] * POV_FORWARD,
            ],
            Follow::Chase => {
                let want = [
                    eye.pos[0] - dir[0] * CHASE_BACK,
                    eye.pos[1] - dir[1] * CHASE_BACK,
                    eye.pos[2] - dir[2] * CHASE_BACK + CHASE_UP,
                ];
                // не заезжаем за стену: подтягиваемся к игроку
                match pos.raycast(eye.pos, want) {
                    Some(f) => {
                        let len = ((want[0] - eye.pos[0]).powi(2) + (want[1] - eye.pos[1]).powi(2) + (want[2] - eye.pos[2]).powi(2)).sqrt().max(1.0);
                        let keep = ((f * len - CHASE_WALL_MARGIN) / len).max(0.0);
                        [
                            eye.pos[0] + (want[0] - eye.pos[0]) * keep,
                            eye.pos[1] + (want[1] - eye.pos[1]) * keep,
                            eye.pos[2] + (want[2] - eye.pos[2]) * keep,
                        ]
                    }
                    None => want,
                }
            }
        };
        keys.push(CamKey { tick: t, pos: cam, ang: [eye.ang[0], eye.ang[1], 0.0], fov: BASE_FOV * scale_at(zoom, t) });
    }
    Some(keys)
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
            assisted_flash: false,
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
        let presets: Vec<_> = free.iter().filter(|s| s.colorfulness.is_some()).collect();
        assert!(presets.len() >= 3 && presets.iter().all(|s| s.colorfulness.unwrap() > 0.0));
        // разные ракурсы подряд не повторяются
        let styles: Vec<_> = presets.iter().map(|s| s.style.clone().unwrap()).collect();
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
        let styles: Vec<String> = t.shots.iter().flat_map(|s| s.segments.iter().filter(|x| x.colorfulness.is_some()).filter_map(|x| x.style.clone())).collect();
        let mut uniq = styles.clone();
        uniq.sort();
        uniq.dedup();
        assert!(uniq.len() * 10 >= styles.len() * 8, "слишком много повторов: {styles:?}");
    }

    #[test]
    fn pov_and_chase_become_free_paths_with_zoom_baked_into_fov() {
        let m = mat();
        let shot = shoot(&m, &hl(200.0, &[1000, 1100, 1200]), &Config::default(), &Duel, &mut vec![]);
        assert!(shot.segments.iter().all(|s| matches!(s.camera, Camera::Free { .. })));
        assert!(shot.zoom.is_empty(), "зум вшит в ключи, mirv_fov не нужен");
        assert_contiguous(&shot);

        // «глазами игрока»: камера в точке глаз + небольшой вынос вперёд, углы игрока
        let pov = shot.segments.iter().find(|s| s.style.as_deref() == Some("pov")).expect("есть POV-сегмент");
        let Camera::Free { keys } = &pov.camera else { unreachable!() };
        let k = keys[keys.len() / 2];
        let eye = Duel.eye_pose(PlayerId(0), k.tick).unwrap();
        let d = forward(eye.ang);
        for i in 0..3 {
            assert!((k.pos[i] - (eye.pos[i] + d[i] * POV_FORWARD)).abs() < 1e-3);
        }
        assert_eq!(k.ang, [eye.ang[0], eye.ang[1], 0.0]);

        // на первом убийстве (оно в виде от первого лица) FOV уменьшен приближением
        let fovs: Vec<f32> = shot
            .segments
            .iter()
            .filter(|s| s.style.as_deref() == Some("pov"))
            .flat_map(|s| match &s.camera { Camera::Free { keys } => keys.iter().filter(|k| (k.tick - 1000).abs() <= 4).map(|k| k.fov).collect::<Vec<_>>(), _ => vec![] })
            .collect();
        assert!(fovs.iter().any(|f| *f < BASE_FOV * 0.85), "{fovs:?}");
        // а до убийств FOV обычный
        assert!(keys.first().unwrap().fov > BASE_FOV * 0.99);
    }

    #[test]
    fn without_positions_pov_stays_a_spectator_camera_and_zoom_is_kept() {
        let m = mat();
        let shot = shoot(&m, &hl(60.0, &[1000, 1100]), &Config::default(), &NoPositions, &mut vec![]);
        assert!(shot.segments.iter().any(|s| matches!(s.camera, Camera::Pov { .. })));
        assert!(!shot.zoom.is_empty());
    }

    /// Бесконечная стена x = -50: цепляет и лучи камеры за спиной.
    struct Wall;
    impl PositionSource for Wall {
        fn eye_pose(&self, p: PlayerId, t: i32) -> Option<Pose> {
            Duel.eye_pose(p, t)
        }
        fn raycast(&self, a: [f32; 3], b: [f32; 3]) -> Option<f32> {
            let (da, db) = (a[0] + 50.0, b[0] + 50.0);
            (da * db < 0.0).then(|| da / (da - db))
        }
    }

    fn kill_in(tick: i32, round: u32, victim: i32, headshot: bool) -> Kill {
        Kill { tick, round, victim: PlayerId(victim), headshot, ..kill(tick, victim) }
    }

    /// Герой 0: раунд 1 — серия из двух (хедшот + обычное), раунд 2 — рядовое убийство,
    /// раунд 3 — только ассист, раунд 4 — одно эффектное (сквозь дым) далеко от первого.
    fn player_match() -> Match {
        let mut m = Match::build("de_test".into(), 64.0, (0..6).map(|i| (i, i, format!("p{i}"), 1)).collect(), &[]);
        m.rounds = (1..=4)
            .map(|n| cs2::Round { number: n, start_tick: n as i32 * 10_000, freeze_end_tick: Some(n as i32 * 10_000 + 500), end_tick: None, winner_team: None })
            .collect();
        let mut assist = kill_in(31_000, 3, 4, false);
        assist.attacker = PlayerId(5);
        assist.assister = Some(PlayerId(0));
        let mut smoke = kill_in(41_000, 4, 2, false);
        smoke.through_smoke = true;
        m.kills = vec![kill_in(11_000, 1, 1, true), kill_in(11_100, 1, 2, false), kill_in(21_000, 2, 3, false), assist, smoke];
        m
    }

    fn player_timeline(density: Density, order: Order) -> Timeline {
        let m = player_match();
        let hs = highlights::for_player(&m, PlayerId(0), &highlights::Config::default());
        let cfg = Config { cinematic: true, density, order, ..Config::default() };
        direct_all(&m, PlayerId(0), &hs, &[], &cfg, &Duel)
    }

    fn is_hero_pov(s: &Shot) -> bool {
        s.player == PlayerId(0) && matches!(s.segments[0].camera, Camera::Pov { .. }) && s.segments[0].style.is_none()
    }

    #[test]
    fn balanced_montage_keeps_kill_rounds_and_interleaves_angles() {
        let t = player_timeline(Density::Balanced, Order::BuildUp);
        let rounds: Vec<u32> = t.shots.iter().map(|s| s.round).collect();
        assert!(!rounds.contains(&3), "раунд из одного ассиста — не момент: {rounds:?}");
        // лучший раунд (серия) — в финале, остальные по порядку
        let mut seen = Vec::new();
        for r in &rounds {
            if !seen.contains(r) {
                seen.push(*r);
            }
        }
        assert_eq!(seen, vec![2, 4, 1], "{rounds:?}");
        // в каждом раунде: сначала от первого лица с плашкой, потом ракурсы на то, что в нём было
        for r in [1, 2, 4] {
            let block: Vec<&Shot> = t.shots.iter().filter(|s| s.round == r).collect();
            assert!(is_hero_pov(block[0]) && block[0].card.is_some(), "раунд {r}: {:?}", block[0].title);
            assert!(block[0].caption.is_empty(), "подпись дублирует плашку");
            let alts = block.iter().filter(|s| !is_hero_pov(s)).count();
            assert!(alts <= MAX_ALT_PER_ROUND, "раунд {r}: ракурсов {alts}");
            let last_pov = block.iter().rposition(|s| is_hero_pov(s)).unwrap();
            for s in &block[last_pov + 1..] {
                assert!(s.start_tick >= block[last_pov].start_tick - 64 * 3, "ракурс относится к своему куску: {}", s.title);
            }
        }
        // рядовое убийство (раунд 2) — без дополнительного ракурса, серия (раунд 1) — с ними
        assert_eq!(t.shots.iter().filter(|s| s.round == 2 && !is_hero_pov(s)).count(), 0);
        assert!(t.shots.iter().any(|s| s.round == 1 && !is_hero_pov(s)));
        // глазами противника — смерть в кадре и в замедлении, но без долгой камеры смерти
        for s in t.shots.iter().filter(|s| s.segments[0].style.as_deref() == Some("victim_pov")) {
            let k = player_match().kills.into_iter().find(|k| k.victim == s.player).unwrap();
            assert!(s.end_tick > k.tick && s.end_tick <= k.tick + 64 / 2, "{}: {} vs {}", s.title, s.end_tick, k.tick);
            assert!(s.scale_at(k.tick) < 1.0, "попадание в замедлении: {:?}", s.timescale);
            assert!(s.timescale.windows(2).all(|w| w[0].tick <= w[1].tick), "{:?}", s.timescale);
            assert!(s.zoom.is_empty());
        }
        // повтор — с замедлением на выстреле
        if let Some(r) = t.shots.iter().find(|s| s.caption.starts_with("Повтор")) {
            assert!(r.timescale.iter().any(|k| k.scale < 1.0), "{:?}", r.timescale);
        }
        assert_eq!(t.shots.iter().find(|s| s.round == 1).unwrap().card.as_ref().unwrap().0, "Раунд 1");
    }

    #[test]
    fn full_montage_keeps_everything_in_match_order() {
        let t = player_timeline(Density::Full, Order::BuildUp);
        let rounds: Vec<u32> = t.shots.iter().map(|s| s.round).collect();
        assert!(rounds.contains(&3), "ассист — тоже KAST: {rounds:?}");
        assert!(rounds.windows(2).all(|w| w[0] <= w[1]), "по порядку матча: {rounds:?}");
        let alts = t.shots.iter().filter(|s| s.round == 1 && !is_hero_pov(s)).count();
        assert!(alts > MAX_ALT_PER_ROUND, "все ракурсы: {alts}");
        let chrono = player_timeline(Density::Balanced, Order::Chronological);
        let rounds: Vec<u32> = chrono.shots.iter().map(|s| s.round).collect();
        assert!(rounds.windows(2).all(|w| w[0] <= w[1]), "{rounds:?}");
    }

    #[test]
    fn long_pov_chunks_are_cut_at_the_longest_pause() {
        // три отрезка с паузами 100 и 200 тиков: при пороге 300 сливаются в один (900 тиков),
        // длиннее 576 — режем по самой долгой паузе
        let spans = [(0, 200), (300, 500), (700, 900)];
        assert_eq!(chunk_spans(&spans, 300, i32::MAX), vec![(0, 900)]);
        assert_eq!(chunk_spans(&spans, 300, 576), vec![(0, 500), (700, 900)]);
        // пауза больше порога — отдельные куски и без ограничения
        assert_eq!(chunk_spans(&spans, 150, i32::MAX), vec![(0, 500), (700, 900)]);
        // перекрывающиеся отрезки резать негде
        assert_eq!(chunk_spans(&[(0, 400), (100, 800)], 10, 100), vec![(0, 800)]);
    }

    #[test]
    fn chase_camera_does_not_go_behind_a_wall() {
        // игрок смотрит вдоль +X, «сзади» — отрицательные x; стена на x = -50
        let keys = follow_keys(&Wall, PlayerId(0), 100, 140, Follow::Chase, 64.0, &[]).unwrap();
        // игрок на x ≈ 50..70, без стены камера была бы на x ≈ -60..-40 и ниже; со стеной — не дальше стены
        assert!(keys.iter().all(|k| k.pos[0] >= -50.0), "{:?}", keys.iter().map(|k| k.pos[0]).collect::<Vec<_>>());
        let free = follow_keys(&Duel, PlayerId(0), 100, 140, Follow::Chase, 64.0, &[]).unwrap();
        assert!(free.iter().any(|k| k.pos[0] < -30.0));
    }
}
