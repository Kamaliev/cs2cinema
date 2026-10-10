//! Страница «Задачи»: очередь и история задач на монтаж — карточками с превью и прогрессом.

use eframe::egui::{self, RichText, Ui};
use pipeline::{
    RecordState, Stage,
    precheck::{Level, Report},
};
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
    Resume,
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
    // очередь стоит после отмены, пока пользователь не нажмёт «Продолжить» (D-13)
    let waiting = app.jobs.list.iter().filter(|j| j.status == JobStatus::Queued).count();
    if app.jobs.paused && waiting > 0 {
        egui::Frame::new().stroke(egui::Stroke::new(1.0_f32, ACCENT)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Очередь на паузе после отмены: {} ждут", plural(waiting, "задача", "задачи", "задач"))).color(ACCENT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::primary(ui, true, "Продолжить") {
                            act = Some(Act::Resume);
                        }
                    });
                });
            },
        );
        ui.add_space(8.0);
    }
    let jobs = app.jobs.list.clone();
    egui::ScrollArea::vertical().id_salt("jobs").auto_shrink([false, false]).show(ui, |ui| {
        for j in &jobs {
            job_card(app, ui, j, &mut act);
            ui.add_space(8.0);
        }
    });
    match act {
        Some(Act::Cancel(id)) => app.cancel_job(id),
        Some(Act::Resume) => app.resume_queue(),
        Some(Act::Retry(id)) => {
            // «Повторить» — осознанное действие, пауза после отмены снимается (D-13)
            app.jobs.paused = false;
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
                    JobStatus::Cancelling => {
                        ui.spinner();
                        ui::pill(ui, "отменяется", MUTED);
                    }
                    JobStatus::Done => {
                        ui::pill(ui, "готово", OK);
                    }
                    JobStatus::Failed => {
                        ui::pill(ui, "ошибка", BAD);
                        // типизированный сбой показывается блоком ниже, прежняя строка — только для остальных
                        if j.fail_code.is_none() {
                            let e = j.error.clone().unwrap_or_default();
                            ui.add(egui::Label::new(RichText::new(&e).color(BAD)).truncate()).on_hover_text(e);
                        }
                    }
                    JobStatus::Cancelled => {
                        ui::pill(ui, "отменена", MUTED);
                    }
                });
                // отчёт предпроверки: у упавшей на ней задачи раскрыт
                if j.kind == JobKind::Record {
                    if let Some(r) = j.precheck.as_deref().and_then(Report::from_json) {
                        precheck_view(ui, ("precheck", j.id), &r, j.fail_state.as_deref() == Some("precheck"));
                    }
                }
                match j.status {
                    JobStatus::Running | JobStatus::Cancelling if !j.phase.is_empty() => ladder(ui, j, if running { app.status.take_now } else { None }),
                    JobStatus::Failed if j.fail_state.is_some() => ladder(ui, j, None),
                    _ => {}
                }
                if j.status == JobStatus::Failed && j.fail_code.is_some() {
                    error_block(ui, j);
                }
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
                    JobStatus::Cancelling => {
                        ui.add_enabled(false, egui::Button::new("Отменяется…"));
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
                if !matches!(j.status, JobStatus::Running | JobStatus::Cancelling) && ui.small_button("Удалить").clicked() {
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

/// Число с существительным в нужной форме: 1 ошибка, 2 ошибки, 5 ошибок.
fn plural(n: usize, one: &str, few: &str, many: &str) -> String {
    let word = match (n % 100, n % 10) {
        (11..=14, _) => many,
        (_, 1) => one,
        (_, 2..=4) => few,
        _ => many,
    };
    format!("{n} {word}")
}

/// Отчёт предпроверки: свёрнутый заголовок с итогом и строки ✔ / ⚠ / ✖ с сообщением; у `warn` и `fail` под строкой подсказка (PRE-01).
/// `open` — раскрыт ли отчёт сразу (у задачи, упавшей на предпроверке).
pub fn precheck_view(ui: &mut Ui, salt: impl std::hash::Hash, r: &Report, open: bool) {
    let count = |level: Level| r.items.iter().filter(|i| i.level == level).count();
    let (fails, warns) = (count(Level::Fail), count(Level::Warn));
    let title = if fails > 0 {
        format!("Предпроверка: {}", plural(fails, "ошибка", "ошибки", "ошибок"))
    } else if warns > 0 {
        format!("Предпроверка: {}", plural(warns, "предупреждение", "предупреждения", "предупреждений"))
    } else {
        "Предпроверка: всё в порядке".to_owned()
    };
    egui::CollapsingHeader::new(title).id_salt(salt).default_open(open).show(ui, |ui| {
        for item in &r.items {
            let (glyph, color) = match item.level {
                Level::Ok => ("✔", OK),
                Level::Warn => ("⚠", ACCENT),
                Level::Fail => ("✖", BAD),
            };
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(glyph).color(color));
                ui.add(egui::Label::new(RichText::new(&item.message).color(color)).wrap());
            });
            if item.level != Level::Ok && !item.hint.is_empty() {
                ui.horizontal_top(|ui| {
                    ui.add_space(18.0);
                    ui.add(egui::Label::new(RichText::new(&item.hint).color(MUTED).size(12.5)).wrap());
                });
            }
        }
    });
}

/// Лестница состояний записи: пройденные шаги, текущий у выполняемой задачи и упавший у неудачной (D-06).
/// `takes` — «дубль a из b», известный только выполняемой задаче.
fn ladder(ui: &mut Ui, j: &Job, takes: Option<(u32, u32)>) {
    if j.kind != JobKind::Record {
        return;
    }
    let (current, failed) = match j.status {
        JobStatus::Running | JobStatus::Cancelling => (RecordState::from_code(&j.phase), false),
        JobStatus::Failed => (j.fail_state.as_deref().and_then(RecordState::from_code), true),
        _ => return,
    };
    let Some(current) = current else { return };
    ui.horizontal_wrapped(|ui| {
        for (i, step) in RecordState::LADDER.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("→").color(MUTED).size(12.5));
            }
            let title = match (step, takes) {
                (RecordState::Take, Some((n, m))) => format!("дубль {n}/{m}"),
                _ => step.title().to_owned(),
            };
            let text = if *step < current {
                RichText::new(format!("✓ {title}")).color(OK)
            } else if *step == current && failed {
                RichText::new(format!("✕ {title}")).color(BAD)
            } else if *step == current {
                ui::b(title).color(ACCENT)
            } else {
                RichText::new(title).color(MUTED)
            };
            ui.label(text.size(12.5));
        }
    });
}

/// Блок типизированной ошибки: код и состояние, сообщение, подсказка, время, путь к `record.log` и хвост консоли (D-07).
fn error_block(ui: &mut Ui, j: &Job) {
    let code = j.fail_code.as_deref().unwrap_or_default();
    let state = j.fail_state.as_deref().and_then(RecordState::from_code).map_or("", RecordState::title);
    ui.label(RichText::new(format!("{code} · {state}")).color(MUTED).size(12.5));
    if let Some(e) = j.error.as_deref().filter(|e| !e.is_empty()) {
        ui.add(egui::Label::new(RichText::new(e).color(BAD)).wrap());
    }
    if let Some(h) = j.fail_hint.as_deref().filter(|h| !h.is_empty()) {
        ui.add(egui::Label::new(RichText::new(h).color(MUTED)).wrap());
    }
    if let Some(ms) = j.fail_elapsed_ms {
        ui.label(RichText::new(format!("прошло {} с", (ms as f64 / 1000.0).round() as i64)).color(MUTED).size(12.5));
    }
    if let Some(d) = &j.out_dir {
        ui.add(egui::Label::new(RichText::new(format!("record.log: {}", d.join("record.log").display())).color(MUTED).size(12.5)).wrap());
    }
    if let Some(tail) = j.fail_tail.as_deref().filter(|t| !t.is_empty()) {
        egui::CollapsingHeader::new("Последние строки консоли").id_salt(("tail", j.id)).show(ui, |ui| {
            egui::Frame::new().fill(ui::BG).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().id_salt(("tail-scroll", j.id)).max_height(200.0).show(ui, |ui| {
                    ui.label(RichText::new(tail).monospace().small());
                });
            });
        });
    }
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
                (_, JobStatus::Running | JobStatus::Cancelling) => "⏺",
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
