//! Разбор ответов Data API в плоские структуры для окна: ничего, кроме того, что показываем.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub id: String,
    pub nickname: String,
    pub avatar: String,
    pub country: String,
    pub elo: u32,
    pub level: u32,
}

impl Player {
    pub fn from_json(v: &Value) -> Option<Self> {
        let cs2 = &v["games"]["cs2"];
        Some(Self {
            id: v["player_id"].as_str()?.to_owned(),
            nickname: v["nickname"].as_str().unwrap_or("").to_owned(),
            avatar: v["avatar"].as_str().unwrap_or("").to_owned(),
            country: v["country"].as_str().unwrap_or("").to_owned(),
            elo: cs2["faceit_elo"].as_u64().unwrap_or(0) as u32,
            level: cs2["skill_level"].as_u64().unwrap_or(0) as u32,
        })
    }
}

/// Строка в списке матчей.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchSummary {
    pub match_id: String,
    /// Unix-время начала.
    pub started_at: i64,
    pub finished_at: i64,
    pub team1: String,
    pub team2: String,
    pub score1: u32,
    pub score2: u32,
    /// Выиграла ли команда игрока, чья история запрошена (`None` — исход неизвестен).
    pub won: Option<bool>,
    pub competition: String,
}

impl MatchSummary {
    pub fn list_from_json(v: &Value, player_id: &str) -> Vec<Self> {
        v["items"].as_array().map(|a| a.iter().filter_map(|m| Self::from_json(m, player_id)).collect()).unwrap_or_default()
    }

    fn from_json(m: &Value, player_id: &str) -> Option<Self> {
        let teams = &m["teams"];
        let name = |f: &str| teams[f]["nickname"].as_str().unwrap_or(f).to_owned();
        let has_player = |f: &str| {
            teams[f]["players"].as_array().is_some_and(|ps| ps.iter().any(|p| p["player_id"].as_str() == Some(player_id)))
        };
        let my_faction = ["faction1", "faction2"].into_iter().find(|f| has_player(f));
        let winner = m["results"]["winner"].as_str();
        let score = |f: &str| m["results"]["score"][f].as_u64().unwrap_or(0) as u32;
        Some(Self {
            match_id: m["match_id"].as_str()?.to_owned(),
            started_at: m["started_at"].as_i64().unwrap_or(0),
            finished_at: m["finished_at"].as_i64().unwrap_or(0),
            team1: name("faction1"),
            team2: name("faction2"),
            score1: score("faction1"),
            score2: score("faction2"),
            won: match (my_faction, winner) {
                (Some(me), Some(w)) => Some(me == w),
                _ => None,
            },
            competition: m["competition_name"].as_str().unwrap_or("").to_owned(),
        })
    }

    pub fn duration_secs(&self) -> i64 {
        (self.finished_at - self.started_at).max(0)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerStats {
    pub player_id: String,
    pub nickname: String,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub headshots_pct: u32,
    pub kd: f32,
    pub kr: f32,
    pub adr: f32,
    pub mvps: u32,
    pub triple: u32,
    pub quadro: u32,
    pub penta: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamStats {
    pub name: String,
    pub score: u32,
    pub won: bool,
    pub players: Vec<PlayerStats>,
}

/// Одна карта матча.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapStats {
    pub map: String,
    pub score: String,
    pub rounds: u32,
    pub teams: Vec<TeamStats>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchStats {
    pub maps: Vec<MapStats>,
}

fn num(v: &Value) -> f32 {
    v.as_f64().map(|f| f as f32).or_else(|| v.as_str()?.trim().parse().ok()).unwrap_or(0.0)
}

impl MatchStats {
    pub fn from_json(v: &Value) -> Self {
        let maps = v["rounds"]
            .as_array()
            .map(|rounds| {
                rounds
                    .iter()
                    .map(|r| {
                        let rs = &r["round_stats"];
                        let winner = r["round_stats"]["Winner"].as_str().unwrap_or("");
                        let teams = r["teams"]
                            .as_array()
                            .map(|ts| {
                                ts.iter()
                                    .map(|t| TeamStats {
                                        name: t["team_stats"]["Team"].as_str().unwrap_or("").to_owned(),
                                        score: num(&t["team_stats"]["Final Score"]) as u32,
                                        won: t["team_id"].as_str() == Some(winner),
                                        players: t["players"]
                                            .as_array()
                                            .map(|ps| {
                                                ps.iter()
                                                    .map(|p| {
                                                        let s = &p["player_stats"];
                                                        PlayerStats {
                                                            player_id: p["player_id"].as_str().unwrap_or("").to_owned(),
                                                            nickname: p["nickname"].as_str().unwrap_or("").to_owned(),
                                                            kills: num(&s["Kills"]) as u32,
                                                            deaths: num(&s["Deaths"]) as u32,
                                                            assists: num(&s["Assists"]) as u32,
                                                            headshots_pct: num(&s["Headshots %"]) as u32,
                                                            kd: num(&s["K/D Ratio"]),
                                                            kr: num(&s["K/R Ratio"]),
                                                            adr: num(&s["ADR"]),
                                                            mvps: num(&s["MVPs"]) as u32,
                                                            triple: num(&s["Triple Kills"]) as u32,
                                                            quadro: num(&s["Quadro Kills"]) as u32,
                                                            penta: num(&s["Penta Kills"]) as u32,
                                                        }
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut m = MapStats {
                            map: rs["Map"].as_str().unwrap_or("").to_owned(),
                            score: rs["Score"].as_str().unwrap_or("").to_owned(),
                            rounds: num(&rs["Rounds"]) as u32,
                            teams,
                        };
                        for t in &mut m.teams {
                            t.players.sort_by_key(|p| std::cmp::Reverse(p.kills));
                        }
                        m
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { maps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_marks_my_result() {
        let v = json!({"items": [{
            "match_id": "1-aa", "started_at": 100, "finished_at": 2500, "competition_name": "5v5",
            "teams": {"faction1": {"nickname": "A", "players": [{"player_id": "me"}]}, "faction2": {"nickname": "B", "players": [{"player_id": "x"}]}},
            "results": {"winner": "faction2", "score": {"faction1": 10, "faction2": 13}}
        }]});
        let l = MatchSummary::list_from_json(&v, "me");
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].score1, l[0].score2, l[0].won), (10, 13, Some(false)));
        assert_eq!(l[0].duration_secs(), 2400);
    }

    #[test]
    fn stats_parse_strings_and_sort_by_kills() {
        let v = json!({"rounds": [{
            "round_stats": {"Map": "de_mirage", "Score": "13 / 7", "Rounds": "20", "Winner": "t1"},
            "teams": [{"team_id": "t1", "team_stats": {"Team": "A", "Final Score": "13"},
                "players": [
                    {"player_id": "p1", "nickname": "low", "player_stats": {"Kills": "5", "Deaths": "15", "K/D Ratio": "0.33", "Headshots %": "40"}},
                    {"player_id": "p2", "nickname": "top", "player_stats": {"Kills": "25", "Deaths": "10", "K/D Ratio": "2.5", "ADR": "101.4", "Penta Kills": "1"}}
                ]}]
        }]});
        let s = MatchStats::from_json(&v);
        let m = &s.maps[0];
        assert_eq!((m.map.as_str(), m.rounds, m.teams[0].won), ("de_mirage", 20, true));
        assert_eq!(m.teams[0].players[0].nickname, "top");
        assert_eq!(m.teams[0].players[0].penta, 1);
        assert!((m.teams[0].players[0].adr - 101.4).abs() < 0.01);
    }
}
