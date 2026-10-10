//! Страница «Матч»: таблица FACEIT, разбор своей игры по демке и ролик про себя.

use std::path::Path;

use cs2::{GrenadeKind, PlayerId, report::Report};
use eframe::egui::{self, RichText, Ui};
use faceit::stats::TeamStats;
use store::JobKind;

use crate::{
    app::{App, Page},
    launch,
    player::kd_color,
    ui::{self, BAD, MUTED, OK},
};

/// Шире этого разделы «Дуэли» и «Гранаты» идут в две колонки.
const TWO_COLUMNS_MIN: f32 = 900.0;

pub fn ui(app: &mut App, ui: &mut Ui) {
    header(app, ui);
    ui.add_space(6.0);
    egui::ScrollArea::vertical().id_salt("match").auto_shrink([false, false]).show(ui, |ui| {
        if let Some(id) = app.game.match_id.clone() {
            scoreboard(app, ui, &id);
        }
        analysis(app, ui);
        ui.add_space(12.0);
    });
}

/// Шапка: счёт, карта, дата, исход; справа — действия с демкой и кнопки ролика.
fn header(app: &mut App, ui: &mut Ui) {
    let summary = app.game.match_id.as_ref().and_then(|id| app.matches.list.iter().find(|m| &m.match_id == id)).cloned();
    let analyzed = app.game.analysis.is_some();
    ui::card(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button("‹ Назад").clicked() {
                app.page = Page::Player;
            }
            ui.add_space(6.0);
            match (&summary, &app.game.analysis) {
                (Some(m), a) => {
                    ui.label(ui::b(&m.team1).size(18.0));
                    ui.label(ui::b(format!("{}:{}", m.score1, m.score2)).size(22.0).color(ui::ACCENT));
                    ui.label(ui::b(&m.team2).size(18.0));
                    ui.add_space(8.0);
                    match m.won {
                        Some(true) => ui::pill(ui, "победа", OK),
                        Some(false) => ui::pill(ui, "поражение", BAD),
                        None => ui::pill(ui, "—", MUTED),
                    };
                    if let Some(a) = a {
                        ui.label(RichText::new(&a.m.map).color(MUTED));
                    }
                    ui.label(RichText::new(ui::date(m.started_at)).color(MUTED));
                }
                (None, Some(a)) => {
                    ui.label(ui::b(&a.label).size(18.0));
                    ui.label(RichText::new(&a.m.map).color(MUTED));
                }
                (None, None) => {
                    ui::heading(ui, "Матч");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if analyzed && app.game.hero.is_some() {
                    let ready = !(app.game.moments.is_empty() && app.game.utility.is_empty());
                    let mut kind = None;
                    if ui::primary(ui, ready, "Собрать ролик про меня") {
                        kind = Some(JobKind::Record);
                    }
                    if ui.add_enabled(ready, egui::Button::new("Только план")).clicked() {
                        kind = Some(JobKind::Plan);
                    }
                    if let Some(kind) = kind {
                        enqueue(app, kind);
                    }
                }
                if ui.add_enabled(!app.status.busy, egui::Button::new("Открыть демку с диска…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Демки CS2", &["dem", "zst", "gz"]).pick_file() {
                        app.open_source(pipeline::Source::File(p));
                    }
                }
                if let Some(id) = &app.game.match_id {
                    if ui.button("FACEIT ↗").clicked() {
                        ui::open_url(&format!("https://www.faceit.com/en/cs2/room/{id}"));
                    }
                }
            });
        });
    });
}

fn scoreboard(app: &mut App, ui: &mut Ui, id: &str) {
    let me = app.matches.player.as_ref().map(|p| p.id.clone()).unwrap_or_default();
    let mut retry = false;
    ui.add_space(6.0);
    egui::CollapsingHeader::new(ui::b("Таблица матча (FACEIT)").size(16.0)).default_open(app.game.analysis.is_none()).show(ui, |ui| {
        ui::card(ui, |ui| match app.matches.stats.get(id) {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Загружаю статистику…");
                });
            }
            Some(Err(e)) => {
                ui.colored_label(BAD, e);
                retry = ui.button("Повторить").clicked();
            }
            Some(Ok(stats)) => {
                for (i, map) in stats.maps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(ui::b(&map.map).size(16.0));
                        ui.label(RichText::new(&map.score).size(16.0));
                        ui.label(RichText::new(format!("{} раундов", map.rounds)).color(MUTED));
                    });
                    for (t, team) in map.teams.iter().enumerate() {
                        team_table(ui, team, &me, &format!("t{i}_{t}"));
                    }
                }
            }
        });
    });
    if retry {
        app.matches.requested.remove(id);
        app.matches.stats.remove(id);
        app.load_stats(&[id.to_owned()]);
    }
}

const COLS: [&str; 11] = ["Игрок", "K", "D", "A", "K/D", "K/R", "HS%", "ADR", "MVP", "3k/4k", "Ace"];

fn team_table(ui: &mut Ui, team: &TeamStats, me: &str, id: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(ui::b(&team.name));
        ui.label(ui::b(team.score.to_string()).color(ui::ACCENT));
        if team.won {
            ui::pill(ui, "победа", OK);
        }
    });
    egui::Grid::new(id).striped(true).num_columns(COLS.len()).spacing([14.0, 5.0]).show(ui, |ui| {
        ui::table_header(ui, &COLS);
        for p in &team.players {
            let mine = p.player_id == me;
            let cell = |s: String| if mine { ui::b(s).color(ui::ACCENT) } else { RichText::new(s) };
            ui.label(cell(p.nickname.clone()));
            ui.label(cell(p.kills.to_string()));
            ui.label(cell(p.deaths.to_string()));
            ui.label(cell(p.assists.to_string()));
            ui.colored_label(kd_color(p.kd), format!("{:.2}", p.kd));
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

fn analysis(app: &mut App, ui: &mut Ui) {
    ui.add_space(8.0);
    let Some(a) = &app.game.analysis else {
        if app.status.busy {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(28.0));
                ui.label(RichText::new(&app.status.last).size(16.0));
            });
        } else if app.game.match_id.is_some() {
            ui::empty_state(ui, "🎯", "Разбор по демке", "Дуэли, гранаты, раунды — и ролик про вас. Демка скачается с FACEIT (нужен ключ с доступом к Downloads API).");
            ui.vertical_centered(|ui| {
                if ui::primary(ui, true, "Скачать и разобрать демку") {
                    app.analyze_match();
                }
            });
        } else {
            ui::empty_state(ui, "🎯", "Матч не выбран", "Выберите матч на странице «Игрок» или перетащите демку (.dem, .dem.zst) в окно.");
        }
        return;
    };

    // про кого разбор
    let mut players: Vec<(PlayerId, String)> = a.m.players.iter().map(|p| (p.id, p.name.clone())).collect();
    players.sort_by_key(|(_, n)| n.to_lowercase());
    let mut hero = app.game.hero;
    ui.horizontal(|ui| {
        ui.label(ui::b(format!("Разбор · {}", a.m.map)).size(17.0));
        ui.add_space(12.0);
        ui.label(RichText::new("игрок:").color(MUTED));
        let current = hero.map_or("выберите игрока".to_owned(), |h| a.m.name(h));
        egui::ComboBox::from_id_salt("hero").selected_text(current).show_ui(ui, |ui| {
            for (id, name) in &players {
                ui.selectable_value(&mut hero, Some(*id), name);
            }
        });
        ui.label(RichText::new(format!("игроков {}, раундов {}", a.m.players.len(), a.m.rounds.len())).color(MUTED));
    });
    if hero != app.game.hero {
        app.game.set_hero(hero);
    }
    let (Some(_hero), Some(r)) = (app.game.hero, app.game.report.clone()) else {
        ui.label(RichText::new("Ваш ник в демке не нашёлся — выберите игрока в списке.").color(BAD));
        return;
    };

    hint(app, ui);
    if ui.available_width() >= TWO_COLUMNS_MIN {
        ui.columns(2, |cols| {
            duels(&mut cols[0], &r);
            nades(app, &mut cols[1], &r);
        });
    } else {
        duels(ui, &r);
        nades(app, ui, &r);
    }
    rounds(ui, &r);
    moments(app, ui);
}

fn hint(app: &mut App, ui: &mut Ui) {
    let queued = app.jobs.list.iter().filter(|j| j.is_active()).count();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Ролик: ваши раунды с убийствами от первого лица, повтор или взгляд противника на сильные моменты, полёт гранат и кто кого убил под них.")
                .color(MUTED)
                .size(12.5),
        );
        if queued > 0 {
            ui.label(RichText::new(format!("в работе задач: {queued}")).color(ui::ACCENT).size(12.5));
            if ui.small_button("К задачам").clicked() {
                app.page = Page::Jobs;
            }
        }
    });
}

/// Поставить задачу на монтаж; для записи путь к HLAE ищем сами, остальное проверяет предпроверка задачи.
fn enqueue(app: &mut App, kind: JobKind) {
    if kind == JobKind::Record {
        if !Path::new(&app.s.hlae_dir).join("HLAE.exe").is_file() {
            if let Some(p) = launch::find_hlae() {
                app.s.hlae_dir = p.to_string_lossy().into_owned();
                app.draft.hlae_dir = app.s.hlae_dir.clone();
                app.s.save();
            }
        }
        // пути HLAE и CS2 проверит предпроверка задачи: неисправное окружение остановит её за секунды с понятной причиной
    }
    match app.enqueue(kind) {
        Ok(id) => {
            app.status.push(&format!("задача #{id} поставлена в очередь"));
            app.page = Page::Jobs;
        }
        Err(e) => app.status.finish(Err(e)),
    }
}

fn duels(ui: &mut Ui, r: &Report) {
    let d = &r.duels;
    ui::section(ui, "Дуэли", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui::stat_tile(ui, "K-D-A", &format!("{}-{}-{}", d.kills, d.deaths, d.assists), None);
            ui::stat_tile(ui, "K/D", &format!("{:.2}", d.kd()), Some(kd_color(d.kd())));
            ui::stat_tile(ui, "HS%", &format!("{:.0}%", d.hs_pct()), None);
            ui::stat_tile(ui, "Раундов", &r.rounds.len().to_string(), None);
        });
        ui.add_space(4.0);
        egui::Grid::new("duels2").num_columns(4).spacing([14.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Открыл раунд").color(MUTED));
            ui.label(ui::b(d.opening_kills.to_string()));
            ui.label(RichText::new("Умер первым").color(MUTED));
            ui.label(ui::b(d.opening_deaths.to_string()));
            ui.end_row();
            ui.label(RichText::new("Отомстил").color(MUTED));
            ui.label(ui::b(d.trade_kills.to_string()));
            ui.label(RichText::new("За меня отомстили").color(MUTED));
            ui.label(ui::b(d.traded_deaths.to_string()));
            ui.end_row();
        });
        if !d.weapons.is_empty() {
            ui.add_space(4.0);
            let list: Vec<String> = d.weapons.iter().map(|(w, n)| format!("{w} {n}")).collect();
            ui.label(RichText::new(format!("Оружие: {}", list.join(" · "))).color(MUTED).size(12.5));
        }
    });
}

fn nade_title(k: GrenadeKind) -> &'static str {
    match k {
        GrenadeKind::He => "HE",
        GrenadeKind::Flash => "Флешки",
        GrenadeKind::Fire => "Молотовы",
        GrenadeKind::Smoke => "Дымы",
    }
}

fn nades(app: &App, ui: &mut Ui, r: &Report) {
    ui::section(ui, "Гранаты", |ui| {
        egui::Grid::new("nades").striped(true).num_columns(6).spacing([18.0, 5.0]).show(ui, |ui| {
            ui::table_header(ui, &["", "Брошено", "Ослеплено", "Ср. ослепл.", "Урон", "Убийства"]);
            let dash = || RichText::new("—").color(MUTED);
            for n in &r.nades {
                ui.label(ui::b(nade_title(n.kind)));
                ui.label(n.thrown.to_string());
                if n.kind == GrenadeKind::Flash {
                    ui.label(n.blinded.to_string());
                    ui.label(format!("{:.1} с", n.avg_blind_secs()));
                } else {
                    ui.label(dash());
                    ui.label(dash());
                }
                if matches!(n.kind, GrenadeKind::He | GrenadeKind::Fire) {
                    ui.label(n.damage.to_string());
                } else {
                    ui.label(dash());
                }
                // у флешки — убийства командой по ослеплённым
                ui.label(if n.kind == GrenadeKind::Smoke { dash() } else { RichText::new(n.kills.to_string()) });
                ui.end_row();
            }
        });
        let mut best = app.game.utility.clone();
        best.sort_by(|a, b| b.score.total_cmp(&a.score));
        if !best.is_empty() {
            ui.add_space(6.0);
            ui.label(ui::b("Лучшие гранаты"));
            for u in best.iter().take(6) {
                ui.horizontal(|ui| {
                    ui::pill(ui, &format!("R{}", u.grenade.round), MUTED);
                    ui.label(u.tags.join(", "));
                });
            }
        }
    });
}

/// Раунды: лента квадратов (исход цветом, убийства числом) и таблица под ней.
fn rounds(ui: &mut Ui, r: &Report) {
    ui::section(ui, "Раунды", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for row in &r.rounds {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                let color = match row.won {
                    Some(true) => OK,
                    Some(false) => BAD,
                    None => MUTED,
                };
                let fill = if row.kills > 0 { color.linear_multiply(0.35 + 0.15 * row.kills.min(3) as f32) } else { color.linear_multiply(0.12) };
                ui.painter().rect(rect, egui::CornerRadius::same(5), fill, egui::Stroke::new(1.0_f32, color.linear_multiply(0.7)), egui::StrokeKind::Inside);
                let text = if row.kills > 0 { row.kills.to_string() } else { String::new() };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::new(13.0, ui::bold()), ui::TEXT);
                if !row.died {
                    ui.painter().circle_filled(rect.right_top() + egui::vec2(-5.0, 5.0), 2.5, OK);
                }
                let mut tip = vec![format!("Раунд {}", row.number)];
                tip.push(match row.won {
                    Some(true) => "победа".into(),
                    Some(false) => "поражение".into(),
                    None => "исход неизвестен".into(),
                });
                tip.push(format!("убийств: {}", row.kills));
                tip.push(if row.died { "погиб".into() } else { "выжил".into() });
                if row.nades > 0 {
                    let mut eff = Vec::new();
                    if row.nade_damage > 0 {
                        eff.push(format!("{} урона", row.nade_damage));
                    }
                    if row.blinded > 0 {
                        eff.push(format!("ослепил {}", row.blinded));
                    }
                    tip.push(format!("гранат: {}{}", row.nades, if eff.is_empty() { String::new() } else { format!(" ({})", eff.join(", ")) }));
                }
                resp.on_hover_text(tip.join("\n"));
            }
        });
        ui.label(RichText::new("Цвет — исход раунда, число — убийства, точка — выжил.").color(MUTED).size(12.0));
        egui::CollapsingHeader::new(RichText::new("Таблица по раундам").color(MUTED)).default_open(false).show(ui, |ui| {
            egui::Grid::new("rounds").striped(true).num_columns(6).spacing([24.0, 3.0]).show(ui, |ui| {
                ui::table_header(ui, &["Раунд", "Исход", "Убийств", "Выжил", "Гранат", "Эффект гранат"]);
                for row in &r.rounds {
                    ui.label(format!("R{}", row.number));
                    match row.won {
                        Some(true) => ui.colored_label(OK, "победа"),
                        Some(false) => ui.colored_label(BAD, "поражение"),
                        None => ui.colored_label(MUTED, "—"),
                    };
                    let k = RichText::new(row.kills.to_string());
                    ui.label(if row.kills >= 3 { k.color(ui::ACCENT) } else if row.kills == 0 { k.color(MUTED) } else { k });
                    if row.died { ui.colored_label(MUTED, "нет") } else { ui.colored_label(OK, "да") };
                    ui.label(row.nades.to_string());
                    let mut eff = Vec::new();
                    if row.nade_damage > 0 {
                        eff.push(format!("{} урона", row.nade_damage));
                    }
                    if row.blinded > 0 {
                        eff.push(format!("ослепил {}", row.blinded));
                    }
                    ui.label(RichText::new(eff.join(", ")).color(MUTED));
                    ui.end_row();
                }
            });
        });
    });
}

fn moments(app: &App, ui: &mut Ui) {
    let Some(a) = &app.game.analysis else { return };
    if app.game.moments.is_empty() {
        return;
    }
    let max = app.game.moments.iter().map(|h| h.score).fold(1.0, f32::max);
    ui::section(ui, "Мои моменты (войдут в ролик)", |ui| {
        egui::Grid::new("moments").striped(true).num_columns(3).spacing([18.0, 5.0]).show(ui, |ui| {
            ui::table_header(ui, &["Раунд", "Score", "Что было"]);
            for h in &app.game.moments {
                ui.label(format!("R{}", h.round));
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(4), ui::BG);
                    let w = rect.width() * (h.score / max).clamp(0.05, 1.0);
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())), egui::CornerRadius::same(4), ui::ACCENT);
                    ui.label(ui::b(format!("{:.0}", h.score)));
                });
                let victims: Vec<String> = h.kills.iter().map(|k| format!("{} ({})", a.m.name(k.victim), k.weapon)).collect();
                let tags = if h.tags.is_empty() { String::new() } else { format!(" · {}", h.tags.join(", ")) };
                ui.label(format!("{}{tags}", victims.join(", ")));
                ui.end_row();
            }
        });
    });
}
