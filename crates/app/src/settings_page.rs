//! Страница «Настройки»: всё в одном месте, с проверкой путей и подсказкой при первом запуске.

use std::path::Path;

use eframe::egui::{self, RichText, Ui};

use crate::{
    app::App,
    remote::control::{RemoteStatus, StatusLevel},
    ui::{self, ACCENT, BAD, MUTED, OK},
};

/// Какие кнопки показывать в разделе «Задачи с сервера».
#[derive(Debug, PartialEq)]
pub struct RemoteButtons {
    /// `None` — кнопки нет (адрес не задан); `Some(активна)` — «Переподключить», неактивна, пока идёт задача сервера.
    pub reconnect: Option<bool>,
    /// Показывать «Продолжить»: раннер на паузе после повторяющихся неисправностей.
    pub resume: bool,
}

/// Кнопки раздела по состоянию раннера: чистая функция, чтобы правила проверялись без окна.
pub fn remote_buttons(status: &RemoteStatus, busy: bool, has_server: bool) -> RemoteButtons {
    RemoteButtons { reconnect: has_server.then_some(!busy), resume: matches!(status, RemoteStatus::Paused { .. }) }
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui::heading(ui, "Настройки");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let changed = app.draft != app.s;
            if ui::primary(ui, changed, "Сохранить") {
                let server_changed = app.s.runner_server != app.draft.runner_server;
                app.s = app.draft.clone();
                app.s.save();
                if server_changed {
                    app.remote.apply(&app.s.runner_server);
                }
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
            ui.label(
                RichText::new("Ключ нужен для списка матчей и статистики; для скачивания демок у ключа должен быть доступ к Downloads API.")
                    .color(MUTED)
                    .size(12.5),
            );
        });

        let mut status_note: Option<&str> = None;
        ui::section(ui, "Пути", |ui| {
            let field_w = (ui.available_width() - 360.0).max(240.0);
            egui::Grid::new("paths").num_columns(4).spacing([12.0, 10.0]).show(ui, |ui| {
                path_row(ui, field_w, "Папка HLAE", &mut d.hlae_dir, |p| p.join("HLAE.exe").is_file(), "нет HLAE.exe");
                path_row(ui, field_w, "Папка CS2", &mut d.cs2_dir, |p| pipeline::gamedir::cs2_exe(p).is_file(), "нет game\\bin\\win64\\cs2.exe");
                path_row(ui, field_w, "Результаты", &mut d.out_root, |p| !p.to_string_lossy().contains(' '), "в пути есть пробел — HLAE не найдёт campath");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.small_button("Найти CS2 в Steam").clicked() {
                    match crate::settings::find_cs2() {
                        Some(p) => d.cs2_dir = p.to_string_lossy().into_owned(),
                        None => status_note = Some("CS2 в библиотеках Steam не найден"),
                    }
                }
                if ui.small_button("Найти HLAE на компьютере").clicked() {
                    match crate::settings::find_hlae() {
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
                ui.label("Запас по времени");
                ui.add(egui::DragValue::new(&mut d.time_margin).range(0.5..=5.0).speed(0.05).fixed_decimals(2).prefix("×"))
                    .on_hover_text("Умножает все ожидания записи и склейки: игра, консоль, демка, дубли, ffmpeg");
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.checkbox(&mut d.gpu_encode, "Кодировать на видеокарте NVIDIA (NVENC) — запись и склейка в разы быстрее");
        });

        remote_section(app, ui);

        // действие откладывается флагом: `d` занимает `app.draft`, а запуск проверки берёт `&mut app`
        let mut run_precheck = false;
        let (busy, job_running) = (app.precheck_busy, app.queue.view().running.is_some());
        ui::section(ui, "Проверка окружения", |ui| {
            ui.horizontal(|ui| {
                let btn = ui.add_enabled(!busy && !job_running, egui::Button::new("Проверить окружение"));
                run_precheck = btn.on_disabled_hover_text("Во время задачи проверка недоступна: её игра выглядела бы чужим CS2").clicked();
                if busy {
                    ui.spinner();
                    ui.label(RichText::new("проверяю…").color(MUTED));
                }
            });
            ui.label(RichText::new("проверяются значения на этой странице, даже несохранённые").color(MUTED).size(12.5));
            match &app.precheck {
                Some(Ok(r)) => crate::jobs_page::precheck_view(ui, "settings-precheck", r, true),
                Some(Err(e)) => {
                    ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
                }
                None => {}
            }
        });
        if run_precheck {
            app.run_precheck();
        }

        let d = &mut app.draft;
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
            ui.checkbox(&mut d.auto_update, "Обновляться автоматически (проверка при запуске и раз в 6 часов; ставится, когда нет задач)");
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

/// Раздел «Задачи с сервера»: адрес, токен (скрытый ввод), состояние подключения и кнопки раннера.
/// Введённый токен уходит в файл с суженным доступом и стирается из окна при любом исходе; из файла в окно он не читается.
fn remote_section(app: &mut App, ui: &mut Ui) {
    let has_server = !app.s.runner_server.trim().is_empty();
    let status = app.remote.status();
    let (busy, pending, has_token) = (app.remote.busy(), app.remote.pending(), app.remote.has_token());
    let buttons = remote_buttons(&status, busy, has_server);
    let (mut save_token, mut clear_token, mut reconnect, mut resume) = (false, false, false, false);
    ui::section(ui, "Задачи с сервера", |ui| {
        egui::Grid::new("remote").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label("Адрес сервера");
            let w = (ui.available_width() - 20.0).max(240.0);
            ui.add_sized(
                [w, 24.0],
                egui::TextEdit::singleline(&mut app.draft.runner_server).hint_text("https://… или http://адрес в локальной сети; пусто — режим выключен"),
            );
            ui.end_row();
            ui.label("Токен раннера");
            ui.horizontal(|ui| {
                ui.add_sized([260.0, 24.0], egui::TextEdit::singleline(&mut app.token_draft).password(true));
                save_token = ui.add_enabled(!app.token_draft.trim().is_empty(), egui::Button::new("Сохранить токен")).clicked();
                if has_token {
                    clear_token = ui.button("Удалить токен").clicked();
                }
                let (text, color) = if has_token { ("токен сохранён", OK) } else { ("токен не задан", MUTED) };
                ui.label(RichText::new(text).color(color).size(12.5));
            });
            ui.end_row();
        });
        ui.add_space(4.0);
        let color = match status.level() {
            StatusLevel::Ok => OK,
            StatusLevel::Warn => ACCENT,
            StatusLevel::Bad => BAD,
            StatusLevel::Off => MUTED,
        };
        ui.add(egui::Label::new(RichText::new(status.text()).color(color)).wrap());
        if pending {
            ui.label(RichText::new("адрес применится, когда закончится задача с сервера").color(ACCENT).size(12.5));
        }
        ui.horizontal(|ui| {
            if let Some(active) = buttons.reconnect {
                let btn = ui.add_enabled(active, egui::Button::new("Переподключить"));
                reconnect = btn.on_disabled_hover_text("Дождитесь конца задачи с сервера — переподключение её отменило бы").clicked();
            }
            if buttons.resume {
                resume = ui.button("Продолжить").clicked();
            }
        });
        ui.label(
            RichText::new(
                "Приложение само забирает задачи с сервера, когда своя очередь пуста и не на паузе. Не запускайте одновременно cs2-cli runner в той же папке.",
            )
            .color(MUTED)
            .size(12.5),
        );
    });
    if save_token {
        // черновик стирается при любом исходе; ошибка не содержит значения токена
        let draft = std::mem::take(&mut app.token_draft);
        match app.remote.save_token(draft.trim()) {
            Ok(()) => app.status.push("токен раннера сохранён"),
            Err(e) => app.status.push(&format!("токен не сохранён: {e}")),
        }
    }
    if clear_token {
        match app.remote.clear_token() {
            Ok(()) => app.status.push("токен раннера удалён"),
            Err(e) => app.status.push(&format!("токен не удалён: {e}")),
        }
    }
    if reconnect {
        app.remote.restart();
    }
    if resume {
        app.resume_queue();
    }
}

/// Подсказка при первом запуске: три шага до первого ролика, с галочками по мере заполнения.
fn first_run(app: &App, ui: &mut Ui) {
    let d = &app.draft;
    let key = !d.faceit_key.trim().is_empty();
    let nick = !d.nickname.trim().is_empty();
    let hlae = Path::new(&d.hlae_dir).join("HLAE.exe").is_file();
    let cs2 = pipeline::gamedir::cs2_exe(Path::new(&d.cs2_dir)).is_file();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_buttons_follow_the_status() {
        let online = RemoteStatus::Online { server_version: "1".into() };
        assert_eq!(
            remote_buttons(&RemoteStatus::Off, false, false),
            RemoteButtons { reconnect: None, resume: false },
            "без адреса кнопки «Переподключить» нет"
        );
        assert_eq!(remote_buttons(&online, false, true), RemoteButtons { reconnect: Some(true), resume: false });
        assert_eq!(remote_buttons(&online, true, true), RemoteButtons { reconnect: Some(false), resume: false }, "пока идёт задача сервера, кнопка неактивна");
        let paused = RemoteStatus::Paused { code: "X".into(), message: "m".into() };
        assert!(remote_buttons(&paused, false, true).resume, "при паузе раннера есть «Продолжить»");
        assert!(!remote_buttons(&online, false, true).resume);
    }
}
