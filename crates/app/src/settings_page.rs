//! Страница «Настройки»: всё в одном месте, с проверкой путей и подсказкой при первом запуске.

use std::path::Path;

use eframe::egui::{self, RichText, Ui};

use crate::{
    app::App,
    launch,
    ui::{self, BAD, MUTED, OK},
};

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui::heading(ui, "Настройки");
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
            if changed {
                ui.label(RichText::new("есть несохранённые изменения").color(ui::ACCENT).size(12.5));
            }
        });
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        first_run(app, ui);
        let d = &mut app.draft;
        ui::section(ui, "FACEIT", |ui| {
            egui::Grid::new("faceit").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("Ключ Data API");
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 140.0).max(200.0);
                    ui.add_sized([w, 24.0], egui::TextEdit::singleline(&mut d.faceit_key).password(true));
                    if ui.small_button("Получить ключ").clicked() {
                        ui::open_url("https://developers.faceit.com/");
                    }
                });
                ui.end_row();
                ui.label("Ник");
                ui.add_sized([240.0, 24.0], egui::TextEdit::singleline(&mut d.nickname));
                ui.end_row();
            });
            ui.label(RichText::new("Ключ нужен для списка матчей и статистики; для скачивания демок у ключа должен быть доступ к Downloads API.").color(MUTED).size(12.5));
        });

        let mut status_note: Option<&str> = None;
        ui::section(ui, "Пути", |ui| {
            let field_w = (ui.available_width() - 360.0).max(240.0);
            egui::Grid::new("paths").num_columns(4).spacing([12.0, 10.0]).show(ui, |ui| {
                path_row(ui, field_w, "Папка HLAE", &mut d.hlae_dir, |p| p.join("HLAE.exe").is_file(), "нет HLAE.exe");
                path_row(ui, field_w, "Папка CS2", &mut d.cs2_dir, |p| launch::cs2_exe(&p.to_string_lossy()).is_file(), "нет game\\bin\\win64\\cs2.exe");
                path_row(ui, field_w, "Результаты", &mut d.out_root, |p| !p.to_string_lossy().contains(' '), "в пути есть пробел — HLAE не найдёт campath");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.small_button("Найти CS2 в Steam").clicked() {
                    match launch::find_cs2() {
                        Some(p) => d.cs2_dir = p.to_string_lossy().into_owned(),
                        None => status_note = Some("CS2 в библиотеках Steam не найден"),
                    }
                }
                if ui.small_button("Найти HLAE на компьютере").clicked() {
                    match launch::find_hlae() {
                        Some(p) => d.hlae_dir = p.to_string_lossy().into_owned(),
                        None => status_note = Some("HLAE.exe не найден — укажите папку вручную или скачайте HLAE"),
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
            egui::Grid::new("rec").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("FPS записи");
                ui.add(egui::DragValue::new(&mut d.fps).range(24..=240));
                ui.end_row();
                ui.label("Разрешение");
                egui::ComboBox::from_id_salt("resolution").selected_text(format!("{}p", d.resolution)).show_ui(ui, |ui| {
                    for h in [720, 1080, 1440] {
                        ui.selectable_value(&mut d.resolution, h, format!("{h}p"));
                    }
                });
                ui.end_row();
                ui.label("Ожидание загрузки демки, с");
                ui.add(egui::DragValue::new(&mut d.load_wait_secs).range(5..=180));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.checkbox(&mut d.gpu_encode, "Кодировать на видеокарте NVIDIA (NVENC) — запись и склейка в разы быстрее");
        });

        ui::section(ui, "Монтаж", |ui| {
            ui.checkbox(&mut d.detailed, "Подробный ролик: весь KAST (ассисты, размены) и все ракурсы на каждое убийство — в ~2 раза длиннее");
            ui.checkbox(&mut d.chronological, "Строго по порядку матча (иначе лучший раунд — в финале)");
            ui.checkbox(&mut d.flybys, "Пролёты и свободные камеры (разбор позиций игроков, дольше)");
            ui.checkbox(&mut d.walls, "Учитывать стены карты (скачает геометрию карт)");
        });

        let mut check = false;
        let note = app.update_note.clone();
        ui::section(ui, "Приложение", |ui| {
            ui.checkbox(&mut d.minimize_to_tray, "Крестик сворачивает в трей");
            ui.checkbox(&mut d.auto_update, "Проверять обновления при запуске");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(format!("Версия: {}", crate::update::current().unwrap_or("dev")));
                check = ui.small_button("Проверить обновления").clicked();
                if let Some(n) = &note {
                    ui.label(RichText::new(n).color(MUTED));
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Файл настроек: {}", crate::settings::data_dir().join("settings.json").display())).color(MUTED).size(12.5));
                if ui.small_button("Открыть папку").clicked() {
                    ui::open_path(&crate::settings::data_dir());
                }
            });
        });
        if check {
            app.check_updates();
        }
        ui.add_space(12.0);
    });
}

/// Подсказка при первом запуске: три шага до первого ролика, с галочками по мере заполнения.
fn first_run(app: &App, ui: &mut Ui) {
    let d = &app.draft;
    let key = !d.faceit_key.trim().is_empty();
    let nick = !d.nickname.trim().is_empty();
    let hlae = Path::new(&d.hlae_dir).join("HLAE.exe").is_file();
    let cs2 = launch::cs2_exe(&d.cs2_dir).is_file();
    if key && nick && hlae && cs2 {
        return;
    }
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(ui::ACCENT.linear_multiply(0.12))
        .stroke(egui::Stroke::new(1.0_f32, ui::ACCENT.linear_multiply(0.5)))
        .corner_radius(egui::CornerRadius::same(ui::RADIUS))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(ui::b("Три шага до первого ролика").size(16.0));
            ui.add_space(4.0);
            let step = |ui: &mut Ui, done: bool, text: &str| {
                ui.horizontal(|ui| {
                    let (glyph, color) = if done { ("✔", OK) } else { ("○", MUTED) };
                    ui.label(RichText::new(glyph).color(color).size(16.0));
                    ui.label(RichText::new(text).color(if done { MUTED } else { ui::TEXT }));
                });
            };
            step(ui, key, "Ключ FACEIT Data API — чтобы видеть матчи и скачивать демки");
            step(ui, nick, "Ваш ник на FACEIT — чтобы находить вас в демке");
            step(ui, hlae && cs2, "Папки HLAE и CS2 — чтобы записывать ролик в игре (ищутся автоматически)");
            ui.label(RichText::new("Не забудьте нажать «Сохранить».").color(MUTED).size(12.5));
        });
}

fn path_row(ui: &mut Ui, w: f32, label: &str, value: &mut String, ok: impl Fn(&Path) -> bool, bad_hint: &str) {
    ui.label(label);
    ui.add_sized([w, 24.0], egui::TextEdit::singleline(value));
    if ui.button("…").clicked() {
        if let Some(p) = rfd::FileDialog::new().pick_folder() {
            *value = p.to_string_lossy().into_owned();
        }
    }
    if value.is_empty() {
        ui.label(RichText::new("○ не задано").color(BAD));
    } else if ok(Path::new(value)) {
        ui.label(RichText::new("✔ найдено").color(OK));
    } else {
        ui.label(RichText::new(format!("✖ {bad_hint}")).color(BAD));
    }
    ui.end_row();
}
