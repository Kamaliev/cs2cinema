//! Страница «Матчи»: последние матчи игрока на FACEIT и статистика каждого.

use eframe::egui::{self, Color32, RichText, Ui};
use faceit::stats::{MatchStats, TeamStats};

use crate::{
    app::App,
    ui::{self, BAD, MUTED, OK},
};

pub fn ui(app: &mut App, ui: &mut Ui) {
    if let Some(id) = app.matches.selected.clone() {
        details(app, ui, &id);
        return;
    }
    ui.horizontal(|ui| {
        ui.heading("Матчи");
        ui.add_space(12.0);
        let resp = ui.add(egui::TextEdit::singleline(&mut app.matches.nickname).hint_text("ник на FACEIT").desired_width(200.0));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(!app.matches.loading, egui::Button::new("Загрузить")).clicked() || enter {
            app.load_matches();
        }
        if app.matches.loading {
            ui.spinner();
        }
    });
    if let Some(e) = &app.matches.error {
        ui.colored_label(BAD, e);
    }
    if let Some(p) = &app.matches.player {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.nickname).strong().size(16.0));
            ui.label(RichText::new(format!("уровень {}", p.level)).color(ui::ACCENT));
            ui.label(format!("{} elo", p.elo));
            if !p.country.is_empty() {
                ui.label(RichText::new(p.country.to_uppercase()).weak());
            }
        });
    }
    ui.separator();
    if app.matches.list.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Введите ник и нажмите «Загрузить» — покажу последние 20 матчей").weak().size(15.0));
        });
        return;
    }

    let mut open: Option<String> = None;
    let mut montage: Option<String> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("matches").striped(true).num_columns(7).spacing([16.0, 8.0]).min_col_width(40.0).show(ui, |ui| {
            for h in ["Дата", "Карта", "Счёт", "Исход", "Длит.", "", ""] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            for m in &app.matches.list {
                ui.label(ui::date(m.started_at));
                let map = app.matches.stats.get(&m.match_id).and_then(|s| s.as_ref().ok()).and_then(|s| s.maps.first()).map(|x| x.map.clone());
                ui.label(RichText::new(map.unwrap_or_else(|| "—".into())).weak());
                ui.label(format!("{} {}:{} {}", m.team1, m.score1, m.score2, m.team2));
                match m.won {
                    Some(true) => ui.colored_label(OK, "Победа"),
                    Some(false) => ui.colored_label(BAD, "Поражение"),
                    None => ui.colored_label(MUTED, "—"),
                };
                ui.label(ui::duration(m.duration_secs()));
                if ui.button("Статистика").clicked() {
                    open = Some(m.match_id.clone());
                }
                if ui.add_enabled(!app.status.busy, egui::Button::new("Смонтировать")).clicked() {
                    montage = Some(m.match_id.clone());
                }
                ui.end_row();
            }
        });
    });
    if let Some(id) = open {
        app.load_stats(std::slice::from_ref(&id));
        app.matches.selected = Some(id);
    }
    if let Some(id) = montage {
        app.open_faceit(&id);
    }
}

fn details(app: &mut App, ui: &mut Ui, id: &str) {
    let Some(m) = app.matches.list.iter().find(|m| m.match_id == id).cloned() else {
        app.matches.selected = None;
        return;
    };
    ui.horizontal(|ui| {
        if ui.button("‹ Назад").clicked() {
            app.matches.selected = None;
        }
        ui.heading(format!("{} {}:{} {}", m.team1, m.score1, m.score2, m.team2));
        ui.label(RichText::new(ui::date(m.started_at)).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::primary(ui, !app.status.busy, "Смонтировать хайлайты") {
                app.open_faceit(id);
            }
            if ui.button("Открыть на FACEIT").clicked() {
                ui::open_url(&format!("https://www.faceit.com/en/cs2/room/{id}"));
            }
        });
    });
    if !m.competition.is_empty() {
        ui.label(RichText::new(&m.competition).weak());
    }
    ui.separator();
    let me = app.matches.player.as_ref().map(|p| p.id.clone()).unwrap_or_default();
    match app.matches.stats.get(id) {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Загружаю статистику…");
            });
        }
        Some(Err(e)) => {
            ui.colored_label(BAD, e);
            if ui.button("Повторить").clicked() {
                app.matches.requested.remove(id);
                app.matches.stats.remove(id);
                app.load_stats(std::slice::from_ref(&id.to_owned()));
            }
        }
        Some(Ok(stats)) => stats_ui(ui, stats, &me),
    }
}

fn stats_ui(ui: &mut Ui, stats: &MatchStats, me: &str) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, map) in stats.maps.iter().enumerate() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&map.map).strong().size(17.0));
                ui.label(RichText::new(&map.score).size(17.0));
                ui.label(RichText::new(format!("{} раундов", map.rounds)).weak());
            });
            for (t, team) in map.teams.iter().enumerate() {
                team_table(ui, team, me, &format!("t{i}_{t}"));
            }
        }
    });
}

const COLS: [&str; 11] = ["Игрок", "K", "D", "A", "K/D", "K/R", "HS%", "ADR", "MVP", "3k/4k", "Ace"];

fn team_table(ui: &mut Ui, team: &TeamStats, me: &str, id: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&team.name).strong());
        ui.label(RichText::new(team.score.to_string()).strong());
        if team.won {
            ui.colored_label(OK, "победа");
        }
    });
    egui::Grid::new(id).striped(true).num_columns(COLS.len()).spacing([14.0, 4.0]).show(ui, |ui| {
        for c in COLS {
            ui.label(RichText::new(c).weak().small());
        }
        ui.end_row();
        for p in &team.players {
            let mine = p.player_id == me;
            let cell = |s: String| if mine { RichText::new(s).color(ui::ACCENT).strong() } else { RichText::new(s) };
            ui.label(cell(p.nickname.clone()));
            ui.label(cell(p.kills.to_string()));
            ui.label(cell(p.deaths.to_string()));
            ui.label(cell(p.assists.to_string()));
            let kd = RichText::new(format!("{:.2}", p.kd)).color(if p.kd >= 1.0 { OK } else { Color32::from_rgb(220, 160, 100) });
            ui.label(kd);
            ui.label(cell(format!("{:.2}", p.kr)));
            ui.label(cell(format!("{}%", p.headshots_pct)));
            ui.label(cell(format!("{:.0}", p.adr)));
            ui.label(cell(p.mvps.to_string()));
            ui.label(cell(format!("{}/{}", p.triple, p.quadro)));
            ui.label(cell(p.penta.to_string()));
            ui.end_row();
        }
    });
}
