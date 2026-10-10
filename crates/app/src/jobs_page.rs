//! Страница «Задачи»: очередь и история задач на монтаж — карточками с превью и прогрессом.

use eframe::egui::{self, RichText, Ui};
use pipeline::Stage;
use store::{Job, JobKind, JobStatus};

use crate::{
    app::{App, Page, THUMB},
    jobs,
    ui::{self, ACCENT, BAD, MUTED, OK},
};

enum Act {
    Cancel(i64),
    Retry(i64),
    Delete(i64),
    Log(i64),
}

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui::heading(ui, "Задачи");
        ui.add_space(8.0);
        let active = app.jobs.list.iter().filter(|j| j.is_active()).count();
        ui.label(RichText::new(format!("{} всего · {active} в работе", app.jobs.list.len())).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Обновить").clicked() {
                app.refresh_jobs();
            }
        });
    });
    ui.add_space(6.0);
    if app.jobs.list.is_empty() {
        ui::empty_state(ui, "🎬", "Задач пока нет", "Откройте матч и нажмите «Собрать ролик про меня» — задача появится здесь.");
        ui.vertical_centered(|ui| {
            if ui::primary(ui, true, "К игроку") {
                app.page = Page::Player;
            }
        });
        return;
    }

    let mut act: Option<Act> = None;
    let jobs = app.jobs.list.clone();
    egui::ScrollArea::vertical().id_salt("jobs").auto_shrink([false, false]).show(ui, |ui| {
        for j in &jobs {
            job_card(app, ui, j, &mut act);
            ui.add_space(8.0);
        }
    });
    match act {
        Some(Act::Cancel(id)) => app.cancel_job(id),
        Some(Act::Retry(id)) => {
            if let Err(e) = app.store.requeue(id) {
                app.status.finish(Err(e));
            }
            app.refresh_jobs();
            jobs::run_next(app);
        }
        Some(Act::Delete(id)) => {
            if let Err(e) = app.store.delete_job(id) {
                app.status.finish(Err(e));
            }
            app.jobs.thumbs.remove(&id);
            app.refresh_jobs();
        }
        Some(Act::Log(id)) => app.jobs.open_log = if app.jobs.open_log == Some(id) { None } else { Some(id) },
        None => {}
    }
}

fn job_card(app: &App, ui: &mut Ui, j: &Job, act: &mut Option<Act>) {
    let running = app.jobs.running.as_ref().is_some_and(|(id, _)| *id == j.id);
    ui::card(ui, |ui| {
        ui.horizontal_top(|ui| {
            thumb(app, ui, j);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                // заголовок: карта · игрок · вид · когда
                ui.horizontal(|ui| {
                    ui.label(ui::b(&j.map).size(16.0));
                    ui.label(ui::b(&j.hero_name).size(16.0).color(ui::ACCENT));
                    let (kind, color) = match j.kind {
                        JobKind::Plan => ("план", MUTED),
                        JobKind::Record => ("ролик", ACCENT),
                    };
                    ui::pill(ui, kind, color);
                    ui.label(RichText::new(format!("#{} · {}", j.id, ui::date(j.created_at))).color(MUTED).size(12.5));
                });
                ui.add(egui::Label::new(RichText::new(&j.label).color(MUTED).size(12.5)).truncate());
                ui.add_space(2.0);
                // статус и прогресс
                ui.horizontal(|ui| match j.status {
                    JobStatus::Queued => {
                        ui::pill(ui, "в очереди", MUTED);
                    }
                    JobStatus::Running => {
                        ui.spinner();
                        let stage = if j.stage.is_empty() { "выполняется".to_owned() } else { j.stage.clone() };
                        ui::pill(ui, &stage, ACCENT);
                        if running {
                            let (frac, text) = progress(app, &j.stage);
                            ui.add(egui::ProgressBar::new(frac).desired_width(220.0).desired_height(12.0).animate(true).fill(ACCENT));
                            if let Some(t) = text {
                                ui.label(RichText::new(t).color(MUTED));
                            }
                        }
                    }
                    JobStatus::Done => {
                        ui::pill(ui, "готово", OK);
                    }
                    JobStatus::Failed => {
                        ui::pill(ui, "ошибка", BAD);
                        let e = j.error.clone().unwrap_or_default();
                        ui.add(egui::Label::new(RichText::new(&e).color(BAD)).truncate()).on_hover_text(e);
                    }
                    JobStatus::Cancelled => {
                        ui::pill(ui, "отменена", MUTED);
                    }
                });
                // мета: шоты, длина, размер файла
                let mut meta: Vec<String> = Vec::new();
                if let Some(n) = j.shots {
                    meta.push(format!("{n} шотов"));
                }
                if let Some(s) = j.secs {
                    meta.push(ui::mmss(s));
                }
                if let Some(b) = app.jobs.sizes.get(&j.id) {
                    meta.push(format!("{:.0} МБ", *b as f64 / 1e6));
                }
                if !meta.is_empty() {
                    ui.label(RichText::new(meta.join(" · ")).color(MUTED).size(13.0));
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match j.status {
                    JobStatus::Queued | JobStatus::Running => {
                        if ui.button("Отменить").clicked() {
                            *act = Some(Act::Cancel(j.id));
                        }
                    }
                    JobStatus::Done => {
                        if let Some(p) = &j.result {
                            if ui::primary(ui, true, "Открыть") {
                                ui::open_path(p);
                            }
                        }
                        if let Some(d) = &j.out_dir {
                            if ui.button("Папка").clicked() {
                                ui::open_path(d);
                            }
                        }
                    }
                    JobStatus::Failed | JobStatus::Cancelled => {
                        if ui.add_enabled(!app.status.busy, egui::Button::new("Повторить")).clicked() {
                            *act = Some(Act::Retry(j.id));
                        }
                    }
                }
                if j.status != JobStatus::Running && ui.small_button("Удалить").clicked() {
                    *act = Some(Act::Delete(j.id));
                }
                let open = app.jobs.open_log == Some(j.id);
                if ui.small_button(if open { "Скрыть журнал" } else { "Журнал" }).clicked() {
                    *act = Some(Act::Log(j.id));
                }
            });
        });
        if app.jobs.open_log == Some(j.id) {
            ui.add_space(6.0);
            egui::Frame::new().fill(ui::BG).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().id_salt(("log", j.id)).max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                    let text = if j.log.is_empty() { "журнал пуст" } else { j.log.as_str() };
                    ui.label(RichText::new(text).monospace().small());
                });
            });
        }
    });
}

/// Превью ролика (кадр с 3-й секунды) или заглушка с глифом.
fn thumb(app: &App, ui: &mut Ui, j: &Job) {
    let size = egui::vec2(THUMB[0] as f32 * 0.75, THUMB[1] as f32 * 0.75);
    match app.jobs.thumbs.get(&j.id) {
        Some(tex) => {
            ui.add(egui::Image::new((tex.id(), size)).corner_radius(egui::CornerRadius::same(6)));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, egui::CornerRadius::same(6), ui::BG);
            let glyph = match (j.kind, j.status) {
                (_, JobStatus::Running) => "⏺",
                (JobKind::Plan, _) => "📄",
                _ => "🎬",
            };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(26.0), MUTED);
        }
    }
}

/// Прогресс выполняемой задачи по этапу и счётчикам из строки состояния.
fn progress(app: &App, stage: &str) -> (f32, Option<String>) {
    let idx = Stage::ALL.iter().position(|s| s.title() == stage).unwrap_or(0) as f32;
    let n = (Stage::ALL.len() - 1) as f32;
    let (inner, text) = match (app.status.stage, app.status.takes, app.status.assemble) {
        (Some(Stage::Record), Some((a, b)), _) => (a as f32 / b.max(1) as f32, Some(format!("дубль {a} из {b}"))),
        (Some(Stage::Assemble), _, Some(p)) => (p, Some(format!("{:.0} %", p * 100.0))),
        _ => (0.0, None),
    };
    (((idx + inner) / n).clamp(0.0, 1.0), text)
}
