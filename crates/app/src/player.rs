//! Страница «Игрок»: ник на FACEIT, сводка за последние матчи и список матчей.

use eframe::egui::{self, Color32, RichText, Ui};
use faceit::stats::{MatchStats, Totals};

use crate::{
    app::App,
    ui::{self, BAD, MUTED, OK},
};

/// Цвет K/D: выше единицы — хорошо.
pub fn kd_color(kd: f32) -> Color32 {
    if kd >= 1.0 { OK } else { Color32::from_rgb(220, 160, 100) }
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("Игрок");
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
    let Some(p) = app.matches.player.clone() else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Введите свой ник на FACEIT — покажу статистику за последние 20 матчей.").weak().size(15.0));
            ui.label(RichText::new("Откройте матч, чтобы разобрать демку и собрать ролик про себя.").weak());
            ui.label(RichText::new("Демку с диска можно просто перетащить в окно.").weak());
        });
        return;
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(&p.nickname).strong().size(18.0));
        ui.label(RichText::new(format!("уровень {}", p.level)).color(ui::ACCENT));
        ui.label(format!("{} elo", p.elo));
        if !p.country.is_empty() {
            ui.label(RichText::new(p.country.to_uppercase()).weak());
        }
    });

    let loaded: Vec<&MatchStats> = app.matches.list.iter().filter_map(|m| app.matches.stats.get(&m.match_id)?.as_ref().ok()).collect();
    let summary = faceit::stats::summarize(&p.id, &loaded);
    let pending = app.matches.list.len().saturating_sub(app.matches.stats.len());

    let mut open: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("player").auto_shrink([false, false]).show(ui, |ui| {
        ui::section(ui, &format!("Сводка за {} матчей", summary.total.matches), |ui| {
            if pending > 0 {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("загружаю статистику, осталось {pending}")).weak());
                });
            }
            cards(ui, &summary.total);
        });
        if !summary.maps.is_empty() {
            ui::section(ui, "По картам", |ui| {
                egui::Grid::new("maps").striped(true).num_columns(6).spacing([24.0, 4.0]).show(ui, |ui| {
                    for h in ["Карта", "Матчей", "Винрейт", "K/D", "ADR", "HS%"] {
                        ui.label(RichText::new(h).weak().small());
                    }
                    ui.end_row();
                    for (map, t) in &summary.maps {
                        ui.label(RichText::new(map).strong());
                        ui.label(t.matches.to_string());
                        ui.colored_label(if t.winrate() >= 50.0 { OK } else { BAD }, format!("{:.0}%", t.winrate()));
                        ui.colored_label(kd_color(t.kd), format!("{:.2}", t.kd));
                        ui.label(format!("{:.0}", t.adr));
                        ui.label(format!("{:.0}%", t.hs_pct));
                        ui.end_row();
                    }
                });
            });
        }
        ui::section(ui, "Матчи", |ui| {
            egui::Grid::new("matches").striped(true).num_columns(8).spacing([16.0, 6.0]).show(ui, |ui| {
                for h in ["Дата", "Карта", "Счёт", "Исход", "K-D-A", "K/D", "ADR", ""] {
                    ui.label(RichText::new(h).weak().small());
                }
                ui.end_row();
                for m in &app.matches.list {
                    let mine = app.matches.stats.get(&m.match_id).and_then(|s| s.as_ref().ok()).and_then(|s| s.find(&p.id));
                    ui.label(ui::date(m.started_at));
                    ui.label(mine.map_or("—".into(), |(map, _, _)| map.map.clone()));
                    // счёт со своей стороны: у победителя очков больше
                    let (hi, lo) = (m.score1.max(m.score2), m.score1.min(m.score2));
                    ui.label(match m.won {
                        Some(true) => format!("{hi}:{lo}"),
                        Some(false) => format!("{lo}:{hi}"),
                        None => format!("{}:{}", m.score1, m.score2),
                    });
                    match m.won {
                        Some(true) => ui.colored_label(OK, "Победа"),
                        Some(false) => ui.colored_label(BAD, "Поражение"),
                        None => ui.colored_label(MUTED, "—"),
                    };
                    match mine {
                        Some((_, s, _)) => {
                            ui.label(format!("{}-{}-{}", s.kills, s.deaths, s.assists));
                            ui.colored_label(kd_color(s.kd), format!("{:.2}", s.kd));
                            ui.label(format!("{:.0}", s.adr));
                        }
                        None => {
                            for _ in 0..3 {
                                ui.label(RichText::new("—").weak());
                            }
                        }
                    }
                    if ui.button("Открыть").clicked() {
                        open = Some(m.match_id.clone());
                    }
                    ui.end_row();
                }
            });
        });
    });
    if let Some(id) = open {
        app.open_match(&id);
    }
}

/// Крупные цифры в ряд.
fn cards(ui: &mut Ui, t: &Totals) {
    ui.horizontal_wrapped(|ui| {
        let card = |ui: &mut Ui, title: &str, value: String, color: Option<Color32>| {
            egui::Frame::group(ui.style()).inner_margin(egui::vec2(14.0, 8.0)).show(ui, |ui| {
                ui.set_min_width(90.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).weak().small());
                    let v = RichText::new(value).size(22.0).strong();
                    ui.label(match color {
                        Some(c) => v.color(c),
                        None => v,
                    });
                });
            });
        };
        card(ui, "Винрейт", format!("{:.0}%", t.winrate()), Some(if t.winrate() >= 50.0 { OK } else { BAD }));
        card(ui, "K/D", format!("{:.2}", t.kd), Some(kd_color(t.kd)));
        card(ui, "ADR", format!("{:.0}", t.adr), None);
        card(ui, "HS%", format!("{:.0}%", t.hs_pct), None);
        card(ui, "K/R", format!("{:.2}", t.kr), None);
        card(ui, "Убийств", t.kills.to_string(), None);
    });
}
