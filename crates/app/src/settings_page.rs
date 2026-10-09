//! Страница «Настройки»: всё в одном месте, с проверкой путей.

use std::path::Path;

use eframe::egui::{self, RichText, Ui};

use crate::{
    app::App,
    launch,
    ui::{self, BAD, OK},
};

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("Настройки");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let changed = app.draft != app.s;
            if ui::primary(ui, changed, "Сохранить") {
                app.s = app.draft.clone();
                app.s.save();
                app.matches.nickname = app.s.nickname.clone();
                app.status.push("настройки сохранены");
            }
            if ui.add_enabled(changed, egui::Button::new("Отменить")).clicked() {
                app.draft = app.s.clone();
            }
        });
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        let d = &mut app.draft;
        ui::section(ui, "FACEIT", |ui| {
            egui::Grid::new("faceit").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Ключ Data API");
                ui.horizontal(|ui| {
                    ui.add_sized([360.0, 22.0], egui::TextEdit::singleline(&mut d.faceit_key).password(true));
                    if ui.small_button("Получить ключ").clicked() {
                        ui::open_url("https://developers.faceit.com/");
                    }
                });
                ui.end_row();
                ui.label("Ник");
                ui.add_sized([200.0, 22.0], egui::TextEdit::singleline(&mut d.nickname));
                ui.end_row();
            });
            ui.label(RichText::new("Ключ нужен для списка матчей и статистики; для скачивания демок у ключа должен быть доступ к Downloads API.").weak().small());
        });

        let mut status_note: Option<&str> = None;
        ui::section(ui, "Пути", |ui| {
            egui::Grid::new("paths").num_columns(4).spacing([10.0, 8.0]).show(ui, |ui| {
                path_row(ui, "Папка HLAE", &mut d.hlae_dir, |p| p.join("HLAE.exe").is_file(), "нет HLAE.exe");
                path_row(ui, "Папка CS2", &mut d.cs2_dir, |p| launch::cs2_exe(&p.to_string_lossy()).is_file(), "нет game\\bin\\win64\\cs2.exe");
                path_row(ui, "Результаты", &mut d.out_root, |p| !p.to_string_lossy().contains(' '), "в пути есть пробел — HLAE не найдёт campath");
            });
            ui.horizontal(|ui| {
                if ui.small_button("Найти CS2 в Steam").clicked() {
                    match launch::find_cs2() {
                        Some(p) => d.cs2_dir = p.to_string_lossy().into_owned(),
                        None => status_note = Some("CS2 в библиотеках Steam не найден"),
                    }
                }
                if ui.small_button("Скачать HLAE").clicked() {
                    ui::open_url("https://github.com/advancedfx/advancedfx/releases");
                }
            });
        });

        if let Some(n) = status_note {
            app.status.push(n);
        }
        let d = &mut app.draft;
        ui::section(ui, "Запись", |ui| {
            egui::Grid::new("rec").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("FPS записи");
                ui.add(egui::DragValue::new(&mut d.fps).range(24..=240));
                ui.end_row();
                ui.label("Ожидание загрузки демки, с");
                ui.add(egui::DragValue::new(&mut d.load_wait_secs).range(5..=180));
                ui.end_row();
            });
        });

        ui::section(ui, "Монтаж", |ui| {
            ui.checkbox(&mut d.flybys, "Пролёты и свободные камеры (разбор позиций игроков, дольше)");
            ui.checkbox(&mut d.walls, "Учитывать стены карты (скачает геометрию карт)");
            ui.checkbox(&mut d.chronological, "Моменты по порядку матча (иначе «от слабого к лучшему»)");
        });

        let mut check = false;
        let note = app.update_note.clone();
        ui::section(ui, "Приложение", |ui| {
            ui.checkbox(&mut d.minimize_to_tray, "Крестик сворачивает в трей");
            ui.checkbox(&mut d.auto_update, "Проверять обновления при запуске");
            ui.horizontal(|ui| {
                ui.label(format!("Версия: {}", crate::update::current().unwrap_or("dev")));
                check = ui.small_button("Проверить обновления").clicked();
                if let Some(n) = &note {
                    ui.label(RichText::new(n).weak());
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Файл настроек: {}", crate::settings::data_dir().join("settings.json").display())).weak().small());
                if ui.small_button("Открыть папку").clicked() {
                    ui::open_path(&crate::settings::data_dir());
                }
            });
        });
        if check {
            app.check_updates();
        }
    });
}

fn path_row(ui: &mut Ui, label: &str, value: &mut String, ok: impl Fn(&Path) -> bool, bad_hint: &str) {
    ui.label(label);
    ui.add_sized([420.0, 22.0], egui::TextEdit::singleline(value));
    if ui.button("…").clicked() {
        if let Some(p) = rfd::FileDialog::new().pick_folder() {
            *value = p.to_string_lossy().into_owned();
        }
    }
    if value.is_empty() {
        ui.label(RichText::new("не задано").color(BAD));
    } else if ok(Path::new(value)) {
        ui.label(RichText::new("найдено").color(OK));
    } else {
        ui.label(RichText::new(bad_hint).color(BAD));
    }
    ui.end_row();
}
