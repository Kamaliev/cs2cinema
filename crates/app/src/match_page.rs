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

pub fn ui(app: &mut App, ui: &mut Ui) {
    header(app, ui);
    ui.separator();
    egui::ScrollArea::vertical().id_salt("match").auto_shrink([false, false]).show(ui, |ui| {
        if let Some(id) = app.game.match_id.clone() {
            scoreboard(app, ui, &id);
        }
        analysis(app, ui);
    });
}

fn header(app: &mut App, ui: &mut Ui) {
    let summary = app.game.match_id.as_ref().and_then(|id| app.matches.list.iter().find(|m| &m.match_id == id)).cloned();
    ui.horizontal(|ui| {
        if ui.button("‹ Назад").clicked() {
            app.page = Page::Player;
        }
        match (&summary, &app.game.analysis) {
            (Some(m), _) => {
                ui.heading(format!("{} {}:{} {}", m.team1, m.score1, m.score2, m.team2));
                ui.label(RichText::new(ui::date(m.started_at)).weak());
            }
            (None, Some(a)) => {
                ui.heading(&a.label);
            }
            (None, None) => {
                ui.heading("Матч");
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(!app.status.busy, egui::Button::new("Открыть демку с диска…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Демки CS2", &["dem", "zst", "gz"]).pick_file() {
                    app.open_source(pipeline::Source::File(p));
                }
            }
            if let Some(id) = &app.game.match_id {
                if ui.button("Открыть на FACEIT").clicked() {
                    ui::open_url(&format!("https://www.faceit.com/en/cs2/room/{id}"));
                }
            }
        });
    });
}

fn scoreboard(app: &mut App, ui: &mut Ui, id: &str) {
    let me = app.matches.player.as_ref().map(|p| p.id.clone()).unwrap_or_default();
    let mut retry = false;
    egui::CollapsingHeader::new(RichText::new("Таблица матча (FACEIT)").strong().size(15.0)).default_open(app.game.analysis.is_none()).show(ui, |ui| {
        match app.matches.stats.get(id) {
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
                        ui.label(RichText::new(&map.map).strong().size(16.0));
                        ui.label(RichText::new(&map.score).size(16.0));
                        ui.label(RichText::new(format!("{} раундов", map.rounds)).weak());
                    });
                    for (t, team) in map.teams.iter().enumerate() {
                        team_table(ui, team, &me, &format!("t{i}_{t}"));
                    }
                }
            }
        }
    });
    if retry {
        app.matches.requested.remove(id);
        app.matches.stats.remove(id);
        app.load_stats(&[id.to_owned()]);
    }
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
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            if app.status.busy {
                ui.add(egui::Spinner::new().size(28.0));
                ui.label(RichText::new(&app.status.last).size(16.0));
            } else if app.game.match_id.is_some() {
                ui.label(RichText::new("Разбор по демке: дуэли, гранаты, раунды — и ролик про вас").weak().size(15.0));
                ui.add_space(6.0);
                if ui::primary(ui, true, "Скачать и разобрать демку") {
                    app.analyze_match();
                }
            } else {
                ui.label(RichText::new("Выберите матч на странице «Игрок» или перетащите демку (.dem, .dem.zst) в окно").weak().size(15.0));
            }
        });
        return;
    };

    // про кого разбор
    let mut players: Vec<(PlayerId, String)> = a.m.players.iter().map(|p| (p.id, p.name.clone())).collect();
    players.sort_by_key(|(_, n)| n.to_lowercase());
    let mut hero = app.game.hero;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Разбор · {}", a.m.map)).strong().size(17.0));
        ui.add_space(12.0);
        ui.label("игрок:");
        let current = hero.map_or("выберите игрока".to_owned(), |h| a.m.name(h));
        egui::ComboBox::from_id_salt("hero").selected_text(current).show_ui(ui, |ui| {
            for (id, name) in &players {
                ui.selectable_value(&mut hero, Some(*id), name);
            }
        });
    });
    if hero != app.game.hero {
        app.game.set_hero(hero);
    }
    let (Some(hero), Some(r)) = (app.game.hero, app.game.report.clone()) else {
        ui.label(RichText::new("Ваш ник в демке не нашёлся — выберите игрока в списке.").color(BAD));
        return;
    };

    actions(app, ui, hero);
    duels(ui, &r);
    nades(app, ui, &r);
    rounds(ui, &r);
    moments(app, ui);
}

fn actions(app: &mut App, ui: &mut Ui, hero: PlayerId) {
    ui.add_space(6.0);
    let _ = hero;
    ui.horizontal(|ui| {
        let ready = !(app.game.moments.is_empty() && app.game.utility.is_empty());
        let mut kind = None;
        if ui::primary(ui, ready, "Собрать ролик про меня") {
            kind = Some(JobKind::Record);
        }
        if ui.add_enabled(ready, egui::Button::new("Только план и скрипты")).clicked() {
            kind = Some(JobKind::Plan);
        }
        if let Some(kind) = kind {
            enqueue(app, kind);
        }
        let queued = app.jobs.list.iter().filter(|j| j.is_active()).count();
        if queued > 0 {
            ui.label(RichText::new(format!("в работе задач: {queued}")).weak());
            if ui.small_button("К задачам").clicked() {
                app.page = Page::Jobs;
            }
        }
    });
    ui.label(
        RichText::new("Весь ваш полезный KAST по раундам: убийства, ассисты, гранаты и размены вашими глазами; пики — глазами противника; гранаты — полёт из-за плеча, общий план и кто кого убил под флешку.")
            .weak()
            .small(),
    );
}

/// Поставить задачу на монтаж; для записи сначала проверяем HLAE и CS2 (путь к HLAE ищем сами).
fn enqueue(app: &mut App, kind: JobKind) {
    if kind == JobKind::Record {
        if !Path::new(&app.s.hlae_dir).join("HLAE.exe").is_file() {
            if let Some(p) = launch::find_hlae() {
                app.s.hlae_dir = p.to_string_lossy().into_owned();
                app.draft.hlae_dir = app.s.hlae_dir.clone();
                app.s.save();
            }
        }
        if let Err(e) = launch::check_paths(&app.s.hlae_dir, &app.s.cs2_dir) {
            app.status.finish(Err(e));
            app.page = Page::Settings;
            return;
        }
    }
    match app.enqueue(kind) {
        Ok(id) => {
            app.status.push(&format!("задача #{id} поставлена в очередь"));
            app.page = Page::Jobs;
        }
        Err(e) => app.status.finish(Err(e)),
    }
}

/// Сетка «название — значение» в несколько колонок.
fn facts(ui: &mut Ui, id: &str, items: &[(&str, String)]) {
    egui::Grid::new(id).num_columns(8).spacing([12.0, 6.0]).show(ui, |ui| {
        for (i, (k, v)) in items.iter().enumerate() {
            ui.label(RichText::new(*k).weak());
            ui.label(RichText::new(v).strong().size(16.0));
            if i % 4 == 3 {
                ui.end_row();
            }
        }
    });
}

fn duels(ui: &mut Ui, r: &Report) {
    let d = &r.duels;
    ui::section(ui, "Дуэли", |ui| {
        facts(
            ui,
            "duels",
            &[
                ("K-D-A", format!("{}-{}-{}", d.kills, d.deaths, d.assists)),
                ("K/D", format!("{:.2}", d.kd())),
                ("HS%", format!("{:.0}%", d.hs_pct())),
                ("Раундов", r.rounds.len().to_string()),
                ("Открыл раунд", d.opening_kills.to_string()),
                ("Умер первым", d.opening_deaths.to_string()),
                ("Отомстил", d.trade_kills.to_string()),
                ("За меня отомстили", d.traded_deaths.to_string()),
            ],
        );
        if !d.weapons.is_empty() {
            ui.add_space(4.0);
            let list: Vec<String> = d.weapons.iter().map(|(w, n)| format!("{w} {n}")).collect();
            ui.label(RichText::new(format!("Убийства по оружию: {}", list.join(" · "))).weak());
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
        egui::Grid::new("nades").striped(true).num_columns(6).spacing([24.0, 4.0]).show(ui, |ui| {
            for h in ["", "Брошено", "Ослеплено врагов", "Ср. ослепление", "Урон", "Убийства"] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            let dash = || RichText::new("—").color(MUTED);
            for n in &r.nades {
                ui.label(RichText::new(nade_title(n.kind)).strong());
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
            ui.label(RichText::new("Лучшие гранаты").strong());
            for u in best.iter().take(6) {
                ui.label(format!("R{:<3} {}", u.grenade.round, u.tags.join(", ")));
            }
        }
    });
}

fn rounds(ui: &mut Ui, r: &Report) {
    ui::section(ui, "Раунды", |ui| {
        egui::Grid::new("rounds").striped(true).num_columns(6).spacing([24.0, 3.0]).show(ui, |ui| {
            for h in ["Раунд", "Исход", "Убийств", "Выжил", "Гранат", "Эффект гранат"] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            for row in &r.rounds {
                ui.label(format!("R{}", row.number));
                match row.won {
                    Some(true) => ui.colored_label(OK, "победа"),
                    Some(false) => ui.colored_label(BAD, "поражение"),
                    None => ui.colored_label(MUTED, "—"),
                };
                let k = RichText::new(row.kills.to_string());
                ui.label(if row.kills >= 3 { k.color(ui::ACCENT).strong() } else if row.kills == 0 { k.color(MUTED) } else { k });
                if row.died { ui.colored_label(MUTED, "нет") } else { ui.colored_label(OK, "да") };
                ui.label(row.nades.to_string());
                let mut eff = Vec::new();
                if row.nade_damage > 0 {
                    eff.push(format!("{} урона", row.nade_damage));
                }
                if row.blinded > 0 {
                    eff.push(format!("ослепил {}", row.blinded));
                }
                ui.label(RichText::new(eff.join(", ")).weak());
                ui.end_row();
            }
        });
    });
}

fn moments(app: &App, ui: &mut Ui) {
    let Some(a) = &app.game.analysis else { return };
    if app.game.moments.is_empty() {
        return;
    }
    ui::section(ui, "Мои моменты (войдут в ролик)", |ui| {
        egui::Grid::new("moments").striped(true).num_columns(3).spacing([18.0, 3.0]).show(ui, |ui| {
            for h in ["Раунд", "Score", "Что было"] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            for h in &app.game.moments {
                ui.label(format!("R{}", h.round));
                ui.label(RichText::new(format!("{:.0}", h.score)).strong());
                let victims: Vec<String> = h.kills.iter().map(|k| format!("{} ({})", a.m.name(k.victim), k.weapon)).collect();
                let tags = if h.tags.is_empty() { String::new() } else { format!(" · {}", h.tags.join(", ")) };
                ui.label(format!("{}{tags}", victims.join(", ")));
                ui.end_row();
            }
        });
    });
}

