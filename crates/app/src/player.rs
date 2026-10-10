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
        ui::heading(ui, "Игрок");
        ui.add_space(12.0);
        let resp = ui.add(egui::TextEdit::singleline(&mut app.matches.nickname).hint_text("ник на FACEIT").desired_width(220.0));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui::primary(ui, !app.matches.loading, "Загрузить") || enter {
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
        ui::empty_state(ui, "👤", "Введите свой ник на FACEIT", "Покажу статистику за последние 20 матчей; из матча можно разобрать демку и собрать ролик про себя.");
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Демку с диска можно просто перетащить в окно.").color(MUTED));
        });
        return;
    };
    ui.add_space(6.0);
    ui::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui::level_badge(ui, p.level);
            ui.label(ui::b(&p.nickname).size(20.0));
            ui.label(RichText::new(format!("{} elo", p.elo)).color(ui::ACCENT).size(16.0));
            ui.label(RichText::new(format!("уровень {}", p.level)).color(MUTED));
            if !p.country.is_empty() {
                ui::pill(ui, &p.country.to_uppercase(), MUTED);
            }
        });
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
                    ui.label(RichText::new(format!("загружаю статистику, осталось {pending}")).color(MUTED));
                });
            }
            tiles(ui, &summary.total);
        });
        if !summary.maps.is_empty() {
            ui::section(ui, "По картам", |ui| {
                egui::Grid::new("maps").striped(true).num_columns(6).spacing([24.0, 6.0]).show(ui, |ui| {
                    ui::table_header(ui, &["Карта", "Матчей", "Винрейт", "K/D", "ADR", "HS%"]);
                    for (map, t) in &summary.maps {
                        ui.label(ui::b(map));
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
            ui.label(RichText::new("Нажмите на матч, чтобы открыть таблицу и разобрать демку.").color(MUTED).size(12.5));
            ui.add_space(4.0);
            egui::Grid::new("matches").striped(true).num_columns(8).spacing([16.0, 7.0]).show(ui, |ui| {
                ui::table_header(ui, &["Дата", "Карта", "Счёт", "Исход", "K-D-A", "K/D", "ADR", ""]);
                for m in &app.matches.list {
                    let row_top = ui.cursor().min;
                    let mine = app.matches.stats.get(&m.match_id).and_then(|s| s.as_ref().ok()).and_then(|s| s.find(&p.id));
                    ui.label(ui::date_short(m.started_at));
                    ui.label(ui::b(mine.map_or("—".into(), |(map, _, _)| map.map.clone())));
                    // счёт со своей стороны: у победителя очков больше
                    let (hi, lo) = (m.score1.max(m.score2), m.score1.min(m.score2));
                    ui.label(match m.won {
                        Some(true) => format!("{hi}:{lo}"),
                        Some(false) => format!("{lo}:{hi}"),
                        None => format!("{}:{}", m.score1, m.score2),
                    });
                    match m.won {
                        Some(true) => ui::pill(ui, "победа", OK),
                        Some(false) => ui::pill(ui, "поражение", BAD),
                        None => ui::pill(ui, "—", MUTED),
                    };
                    match mine {
                        Some((_, s, _)) => {
                            ui.label(format!("{}-{}-{}", s.kills, s.deaths, s.assists));
                            ui.colored_label(kd_color(s.kd), format!("{:.2}", s.kd));
                            ui.label(format!("{:.0}", s.adr));
                        }
                        None => {
                            for _ in 0..3 {
                                ui.label(RichText::new("—").color(MUTED));
                            }
                        }
                    }
                    if ui.small_button("Открыть").clicked() {
                        open = Some(m.match_id.clone());
                    }
                    let row = egui::Rect::from_min_max(row_top, egui::pos2(ui.max_rect().right(), ui.cursor().min.y + 22.0));
                    if ui::row_hover(ui, row, egui::Id::new(("match-row", &m.match_id))).clicked() {
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
fn tiles(ui: &mut Ui, t: &Totals) {
    ui.horizontal_wrapped(|ui| {
        ui::stat_tile(ui, "Винрейт", &format!("{:.0}%", t.winrate()), Some(if t.winrate() >= 50.0 { OK } else { BAD }));
        ui::stat_tile(ui, "K/D", &format!("{:.2}", t.kd), Some(kd_color(t.kd)));
        ui::stat_tile(ui, "ADR", &format!("{:.0}", t.adr), None);
        ui::stat_tile(ui, "HS%", &format!("{:.0}%", t.hs_pct), None);
        ui::stat_tile(ui, "K/R", &format!("{:.2}", t.kr), None);
        ui::stat_tile(ui, "Убийств", &t.kills.to_string(), None);
    });
}
