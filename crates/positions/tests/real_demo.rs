//! Проверки на настоящей демке FACEIT. Включаются переменной окружения:
//!   CS2_TEST_DEMO=/path/to/match.dem cargo test -p positions --release -- --nocapture
//! Без неё (или если файл — LFS-заглушка) тесты молча пропускаются.
//!
//! Эталонные числа получены независимым парсером demoparser2 на демке
//! 1-3e7db9e3-8e92-4761-a1db-3729fb7de11c-1-1.dem (de_nuke).

fn load() -> Option<Vec<u8>> {
    let path = std::env::var_os("CS2_TEST_DEMO")?;
    let data = demo::read_demo(path).ok()?;
    data.starts_with(b"PBDEMS2\0").then_some(data)
}

fn wrap(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

#[test]
fn match_model_agrees_with_reference_parser() {
    let Some(data) = load() else { return };
    let parsed = demo::parse(&data).unwrap();
    let players = parsed.players.iter().map(|p| (p.userid, p.slot, p.name.clone(), p.xuid)).collect();
    let m = cs2::Match::build(parsed.map_name.clone(), parsed.tick_rate(), players, &parsed.events);
    assert_eq!(m.map, "de_nuke");
    assert_eq!(m.kills.len(), 173, "demoparser2: 173 убийств после begin_new_match");
    assert_eq!(m.rounds.len(), 24);
}

#[test]
fn shooters_look_at_their_victims() {
    let Some(data) = load() else { return };
    let parsed = demo::parse(&data).unwrap();
    let players = parsed.players.iter().map(|p| (p.userid, p.slot, p.name.clone(), p.xuid)).collect();
    let m = cs2::Match::build(parsed.map_name.clone(), parsed.tick_rate(), players, &parsed.events);
    let track = positions::track(&data, 4).unwrap();

    let (mut checked, mut aimed) = (0, 0);
    for k in &m.kills {
        let (Some(a), Some(v)) = (track.sample_at(k.attacker, k.tick - 1), track.sample_at(k.victim, k.tick - 1)) else { continue };
        let bearing = (v.pos[1] - a.pos[1]).atan2(v.pos[0] - a.pos[0]).to_degrees();
        checked += 1;
        aimed += (wrap(a.ang[1] - bearing).abs() < 15.0) as usize;
    }
    // проверено: 170 из 173; если соглашение об углах сломается, будет ~0
    assert!(checked >= 170 && aimed * 100 >= checked * 95, "{aimed}/{checked}");
}

#[test]
fn positions_match_reference_samples() {
    let Some(data) = load() else { return };
    let track = positions::track(&data, 4).unwrap();
    // userid 3 = VOVA_EBAKIN; demoparser2 на тике 20000: X=1764.152 Y=-1645.419 Z=-415.969, жив, команда CT
    let s = track.sample_at(cs2::PlayerId(3), 20000).unwrap();
    assert!((s.pos[0] - 1764.15).abs() < 1.0 && (s.pos[1] + 1645.42).abs() < 1.0 && (s.pos[2] + 415.97).abs() < 1.0, "{s:?}");
    assert!(s.alive && s.team == 3);
    assert!((s.ang[1] - 178.53).abs() < 2.0 && (s.ang[0] + 13.36).abs() < 2.0, "{s:?}");
}

/// Геометрия карты (awpy-data) и позиции из демки должны быть в одной системе координат:
/// на обычных убийствах (не вабанг, не через смок) стрелок видит жертву.
/// Нужны CS2_TEST_DEMO и CS2_TEST_MAPS (папка с *.mesh).
#[test]
fn map_geometry_is_aligned_with_demo_coordinates() {
    let (Some(data), Some(dir)) = (load(), std::env::var_os("CS2_TEST_MAPS")) else { return };
    let mesh = geometry::load_map(std::path::Path::new(&dir), "de_nuke", &geometry::Options { offline: true, ..Default::default() }).unwrap().mesh;
    let parsed = demo::parse(&data).unwrap();
    let players = parsed.players.iter().map(|p| (p.userid, p.slot, p.name.clone(), p.xuid)).collect();
    let m = cs2::Match::build(parsed.map_name.clone(), parsed.tick_rate(), players, &parsed.events);
    let track = positions::track(&data, 4).unwrap();
    use director::PositionSource;

    let (mut checked, mut clear) = (0, 0);
    for k in m.kills.iter().filter(|k| !k.wallbang && !k.through_smoke) {
        let (Some(a), Some(v)) = (track.eye_pose(k.attacker, k.tick - 1), track.eye_pose(k.victim, k.tick - 1)) else { continue };
        checked += 1;
        let target = [v.pos[0], v.pos[1], v.pos[2] - 10.0];
        clear += mesh.segment_clear(a.pos, target) as usize;
    }
    println!("{} triangles; clear line of sight on {clear}/{checked} ordinary kills", mesh.triangle_count());
    assert!(checked > 100 && clear * 100 >= checked * 90, "{clear}/{checked}");
}

/// Со стенами в плане нет кадров, где герой закрыт, и пролётов камеры сквозь стены
/// (без геометрии на этой демке было 9 закрытых кадров из 20 и 5 пролётов сквозь стены).
#[test]
fn planned_cameras_respect_walls() {
    let (Some(data), Some(dir)) = (load(), std::env::var_os("CS2_TEST_MAPS")) else { return };
    use director::{Camera, PositionSource};
    let mesh = geometry::load_map(std::path::Path::new(&dir), "de_nuke", &geometry::Options { offline: true, ..Default::default() }).unwrap().mesh;
    let parsed = demo::parse(&data).unwrap();
    let players = parsed.players.iter().map(|p| (p.userid, p.slot, p.name.clone(), p.xuid)).collect();
    let m = cs2::Match::build(parsed.map_name.clone(), parsed.tick_rate(), players, &parsed.events);
    let track = positions::track(&data, 4).unwrap();
    let scene = positions::WithGeometry { track: &track, mesh: &mesh };

    let found = highlights::find(&m, &highlights::Config::default());
    let plan = director::direct(&m, &found, &director::Config::default(), &scene);

    let (mut frames, mut hidden, mut crossings) = (0, 0, 0);
    for shot in &plan.shots {
        for seg in &shot.segments {
            let Camera::Free { keys } = &seg.camera else { continue };
            for k in keys {
                let p = track.eye_pose(shot.player, k.tick).unwrap();
                frames += 1;
                hidden += mesh.raycast(k.pos, [p.pos[0], p.pos[1], p.pos[2] - 10.0]).is_some() as usize;
            }
            crossings += keys.windows(2).filter(|w| mesh.raycast(w[0].pos, w[1].pos).is_some()).count();
        }
    }
    assert!(frames > 0, "ожидались свободные камеры");
    assert_eq!((hidden, crossings), (0, 0));
}
