//! Страница «Задачи»: очередь и история задач на монтаж.

use eframe::egui::{self, RichText, Ui};
use store::{JobKind, JobStatus};

use crate::{
    app::App,
    jobs,
    ui::{self, ACCENT, BAD, MUTED, OK},
};

pub fn ui(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.heading("Задачи");
        ui.add_space(12.0);
        let active = app.jobs.list.iter().filter(|j| j.is_active()).count();
        ui.label(RichText::new(format!("{} всего · {active} в работе", app.jobs.list.len())).weak());
        if ui.small_button("Обновить").clicked() {
            app.refresh_jobs();
        }
    });
    ui.separator();
    if app.jobs.list.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Задач пока нет: откройте матч и нажмите «Собрать ролик про меня».").weak().size(15.0));
        });
        return;
    }

    enum Act {
        Cancel(i64),
        Retry(i64),
        Delete(i64),
        Log(i64),
    }
    let mut act: Option<Act> = None;
    let jobs = app.jobs.list.clone();
    egui::ScrollArea::vertical().id_salt("jobs").auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("jobs").striped(true).num_columns(8).spacing([14.0, 6.0]).show(ui, |ui| {
            for h in ["#", "Создана", "Матч", "Игрок", "Что", "Статус", "Ролик", ""] {
                ui.label(RichText::new(h).weak().small());
            }
            ui.end_row();
            for j in &jobs {
                ui.label(RichText::new(format!("{}", j.id)).weak());
                ui.label(ui::date(j.created_at));
                ui.label(RichText::new(format!("{} · {}", j.map, j.label)).strong());
                ui.label(&j.hero_name);
                ui.label(match j.kind {
                    JobKind::Plan => "план",
                    JobKind::Record => "ролик",
                });
                match j.status {
                    JobStatus::Queued => ui.colored_label(MUTED, "в очереди"),
                    JobStatus::Running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.colored_label(ACCENT, if j.stage.is_empty() { "выполняется".to_owned() } else { j.stage.clone() });
                        })
                        .response
                    }
                    JobStatus::Done => ui.colored_label(OK, "готово"),
                    JobStatus::Failed => ui.colored_label(BAD, j.error.clone().unwrap_or_else(|| "ошибка".into())).on_hover_text(j.error.clone().unwrap_or_default()),
                    JobStatus::Cancelled => ui.colored_label(MUTED, "отменена"),
                };
                match (j.shots, j.secs) {
                    (Some(n), Some(s)) => ui.label(format!("{n} шотов · {s:.0} с")),
                    _ => ui.label(RichText::new("—").weak()),
                };
                ui.horizontal(|ui| {
                    match j.status {
                        JobStatus::Queued | JobStatus::Running => {
                            if ui.button("Отменить").clicked() {
                                act = Some(Act::Cancel(j.id));
                            }
                        }
                        JobStatus::Done => {
                            if let Some(p) = &j.result {
                                if ui.button("Открыть").clicked() {
                                    ui::open_path(p);
                                }
                            }
                            if let Some(d) = &j.out_dir {
                                if ui.small_button("Папка").clicked() {
                                    ui::open_path(d);
                                }
                            }
                        }
                        JobStatus::Failed | JobStatus::Cancelled => {
                            if ui.add_enabled(!app.status.busy, egui::Button::new("Повторить")).clicked() {
                                act = Some(Act::Retry(j.id));
                            }
                        }
                    }
                    if ui.small_button("Журнал").clicked() {
                        act = Some(Act::Log(j.id));
                    }
                    if j.status != JobStatus::Running && ui.small_button("Удалить").clicked() {
                        act = Some(Act::Delete(j.id));
                    }
                });
                ui.end_row();
                if app.jobs.open_log == Some(j.id) {
                    ui.label("");
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt(("log", j.id)).max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                            let text = if j.log.is_empty() { "журнал пуст" } else { j.log.as_str() };
                            ui.label(RichText::new(text).monospace().small());
                        });
                    });
                    ui.end_row();
                }
            }
        });
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
            app.refresh_jobs();
        }
        Some(Act::Log(id)) => app.jobs.open_log = if app.jobs.open_log == Some(id) { None } else { Some(id) },
        None => {}
    }
}
