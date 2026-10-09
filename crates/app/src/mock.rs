//! Тестовые данные для страницы «Матчи» без ключа FACEIT (только debug: `CS2CINEMA_MOCK=1|details`).

use faceit::stats::{MapStats, MatchStats, MatchSummary, Player, PlayerStats, TeamStats};

use crate::app::App;

pub fn fill(app: &mut App) {
    app.matches.player = Some(Player { id: "me".into(), nickname: "Kamaliev".into(), avatar: String::new(), country: "ru".into(), elo: 2140, level: 8 });
    let now = chrono::Local::now().timestamp();
    app.matches.list = (0..20)
        .map(|i| MatchSummary {
            match_id: format!("1-{i:08}-0000-0000-0000-000000000000"),
            started_at: now - i * 5400,
            finished_at: now - i * 5400 + 2400 + i * 37,
            team1: "team_Kamaliev".into(),
            team2: format!("team_enemy{i}"),
            score1: 13,
            score2: (i * 3 % 14) as u32,
            won: Some(i % 3 != 0),
            competition: "5v5 RANKED".into(),
        })
        .collect();
    let id = app.matches.list[0].match_id.clone();
    let player = |n: &str, k: u32, d: u32, id: &str| PlayerStats {
        player_id: id.into(),
        nickname: n.into(),
        kills: k,
        deaths: d,
        assists: 4,
        headshots_pct: 55,
        kd: k as f32 / d.max(1) as f32,
        kr: k as f32 / 24.0,
        adr: 60.0 + k as f32 * 2.0,
        mvps: k / 6,
        triple: k / 10,
        quadro: (k > 25) as u32,
        penta: (k > 30) as u32,
    };
    let team = |name: &str, won: bool, base: u32, me: bool| TeamStats {
        name: name.into(),
        score: if won { 13 } else { 9 },
        won,
        players: (0..5).map(|i| player(&format!("{name}_p{i}"), base - i * 3, 12 + i * 2, if me && i == 1 { "me" } else { "x" })).collect(),
    };
    let stats = MatchStats {
        maps: vec![MapStats { map: "de_nuke".into(), score: "13 / 9".into(), rounds: 22, teams: vec![team("team_Kamaliev", true, 31, true), team("team_enemy0", false, 24, false)] }],
    };
    app.matches.stats.insert(id.clone(), Ok(stats));
    if std::env::var("CS2CINEMA_MOCK").as_deref() == Ok("details") {
        app.matches.selected = Some(id);
    }
}
