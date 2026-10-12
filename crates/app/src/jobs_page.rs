//! Страница «Задачи»: очередь и история задач на монтаж — карточками с превью и прогрессом.

use eframe::egui::{self, RichText, Ui};
use pipeline::{
    RecordState, Stage,
    precheck::{Level, Report},
};
use store::{Job, JobKind, JobStatus};

use crate::{
    app::{App, Page, THUMB},
    queue::{self, PauseReason, QueueView},
    ui::{self, ACCENT, BAD, MUTED, OK},
};

enum Act {
    Cancel(i64),
    Retry(i64),
    Redo(i64),
    Delete(i64),
    Log(i64),
    Resume,
    DismissSummary,
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
            // ACC-01: пять задач на пяти картах одним действием; проверка серии — в консоли
            let accept = ui
                .add_enabled(!app.jobs.acceptance_busy, egui::Button::new("Поставить приёмочный набор").small())
                .on_hover_text("5 матчей из acceptance/matches.txt на 5 разных картах; проверка — cs2-cli acceptance check");
            if accept.clicked() {
                app.enqueue_acceptance();
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
    // итог очереди: тот же текст, что в тосте трея; остаётся, пока его не скроют
    if let Some(text) = app.jobs.summary.clone() {
        egui::Frame::new().stroke(egui::Stroke::new(1.0_f32, ui::BORDER)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).color(ui::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Скрыть").clicked() {
                            act = Some(Act::DismissSummary);
                        }
                    });
                });
            },
        );
        ui.add_space(8.0);
    }
    // очередь стоит после отмены или прерванной задачи, пока пользователь не нажмёт «Продолжить» (D-13, D-16)
    let waiting = app.jobs.list.iter().filter(|j| j.status == JobStatus::Queued).count();
    let paused = app.queue.view().paused;
    if let Some(reason) = paused.filter(|_| waiting > 0) {
        egui::Frame::new().stroke(egui::Stroke::new(1.0_f32, ACCENT)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(pause_banner(&reason, waiting)).color(ACCENT));
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
        Some(Act::DismissSummary) => app.dismiss_summary(),
        // «Повторить» (с записи, D-09) и «Заново» (с начала) — осознанные действия, пауза очереди снимается (D-13)
        Some(Act::Retry(id)) => requeue(app, id, true),
        Some(Act::Redo(id)) => requeue(app, id, false),
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

/// Ставит задачу в очередь заново: `resume` — с записи по `record.json`, иначе с самого начала.
fn requeue(app: &mut App, id: i64, resume: bool) {
    let queued = if resume { app.store.requeue_resume(id) } else { app.store.requeue(id) };
    if let Err(e) = queued {
        app.status.finish(Err(e));
    }
    app.queue.resume();
    app.refresh_jobs();
}

/// Какие кнопки показывает карточка задачи.
#[derive(Debug, PartialEq)]
pub struct CardButtons {
    /// «Повторить»: `None` — кнопки нет, `Some(активна)`.
    pub retry: Option<bool>,
    /// «Заново» показывается.
    pub redo: bool,
    /// «Открыть папку» показывается.
    pub open_dir: bool,
}

/// Кнопки карточки по статусу (D-09, D-13). «Повторить» активна, только если в папке есть `record.json` (`resumable`)
/// и окно не занято (`busy`); «Заново» доступна всегда, когда задача закончилась. У выполняемой и ждущей задачи нет ни той, ни другой.
pub fn card_buttons(status: JobStatus, resumable: bool, has_dir: bool, busy: bool) -> CardButtons {
    match status {
        JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => CardButtons { retry: Some(resumable && !busy), redo: true, open_dir: has_dir },
        JobStatus::Done => CardButtons { retry: None, redo: false, open_dir: has_dir },
        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => CardButtons { retry: None, redo: false, open_dir: false },
    }
}

/// Кнопки карточки задачи сервера (D-01, T-08-21): пока карточку держит раннер (запись в хабе или выдача в его состоянии),
/// кнопок нет — задачу друга нельзя ни потерять, ни записать дважды. «Заново» у карточки сервера нет никогда:
/// «Повторить» закрытой выдачи продолжает её локально по `record.json` без загрузки.
pub fn remote_card_buttons(status: JobStatus, resumable: bool, has_dir: bool, busy: bool, held: bool) -> CardButtons {
    if held {
        return CardButtons { retry: None, redo: false, open_dir: false };
    }
    CardButtons { redo: false, ..card_buttons(status, resumable, has_dir, busy) }
}

/// Заголовок карточки сервера, пока карта и игрок неизвестны: подпись «Задача с сервера #N», построенная из номера
/// в `external_ref` (текст от друга в неё не попадает). У карточки с картой или игроком и у локальной — `None`.
pub fn server_heading(j: &Job) -> Option<String> {
    let id = j.external_ref.as_deref()?.strip_prefix("srv:")?.split(':').next()?.parse::<i64>().ok()?;
    (j.map.is_empty() && j.hero_name.is_empty()).then(|| crate::remote::card_label(id))
}

/// Чего ждёт карточка в очереди приложения (D-04): потолка ожидания нет, но причина видна.
pub fn waiting_reason(view: &QueueView, card: i64) -> String {
    if view.paused.is_some() {
        "очередь приложения на паузе — нажмите «Продолжить»".to_owned()
    } else if let Some((id, _)) = view.running {
        format!("идёт задача #{id}")
    } else if let Some((id, _)) = view.hold {
        if id == card {
            "ждёт авто-повтора".to_owned()
        } else {
            "очередь ждёт авто-повтора другой задачи".to_owned()
        }
    } else {
        "начнётся на следующем такте очереди".to_owned()
    }
}

/// Пояснение под подписью карточки сервера: чего она ждёт, что продолжится сама и что стало с роликом (D-04, Pitfall 5).
/// `waiting` — причина из `waiting_reason`, `held` — карточку держит раннер, `result_exists` — файл ролика ещё на диске.
pub fn server_note(j: &Job, held: bool, waiting: Option<&str>, result_exists: bool) -> Option<String> {
    j.external_ref.as_ref()?;
    match j.status {
        JobStatus::Queued => Some(format!("ждёт очереди приложения: {}", waiting.unwrap_or("начнётся на следующем такте очереди"))),
        JobStatus::Interrupted if held => Some("продолжится сама, когда раннер подключится к серверу".to_owned()),
        JobStatus::Done if !result_exists => Some("ролик отправлен на сервер, локальная копия удалена".to_owned()),
        _ => None,
    }
}

fn job_card(app: &App, ui: &mut Ui, j: &Job, act: &mut Option<Act>) {
    let view = app.queue.view();
    let running = view.running.as_ref().is_some_and(|(id, _)| *id == j.id);
    let hold = view.hold.filter(|(id, _)| *id == j.id).map(|(_, until)| until);
    let server = j.external_ref.is_some();
    let held = server && app.remote.hub().is_held(j);
    let result_exists = j.result.as_ref().is_some_and(|p| p.is_file());
    ui::card(ui, |ui| {
        ui.horizontal_top(|ui| {
            thumb(app, ui, j);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                // заголовок: карта · игрок · вид · когда
                ui.horizontal(|ui| {
                    if let Some(title) = server_heading(j) {
                        ui.label(ui::b(title).size(16.0));
                    } else if server {
                        // у карточки сервера пустые части не выводятся
                        if !j.map.is_empty() {
                            ui.label(ui::b(&j.map).size(16.0));
                        }
                        if !j.hero_name.is_empty() {
                            ui.label(ui::b(&j.hero_name).size(16.0).color(ui::ACCENT));
                        }
                    } else {
                        ui.label(ui::b(&j.map).size(16.0));
                        ui.label(ui::b(&j.hero_name).size(16.0).color(ui::ACCENT));
                    }
                    let (kind, color) = match j.kind {
                        JobKind::Plan => ("план", MUTED),
                        JobKind::Record => ("ролик", ACCENT),
                    };
                    ui::pill(ui, kind, color);
                    if server {
                        ui::pill(ui, "с сервера", ACCENT);
                    }
                    ui.label(RichText::new(format!("#{} · {}", j.id, ui::date(j.created_at))).color(MUTED).size(12.5));
                });
                ui.add(egui::Label::new(RichText::new(&j.label).color(MUTED).size(12.5)).truncate());
                if server {
                    let waiting = (j.status == JobStatus::Queued).then(|| waiting_reason(&view, j.id));
                    if let Some(note) = server_note(j, held, waiting.as_deref(), result_exists) {
                        ui.add(egui::Label::new(RichText::new(note).color(MUTED).size(12.5)).wrap());
                    }
                }
                ui.add_space(2.0);
                // статус и прогресс
                ui.horizontal(|ui| match j.status {
                    JobStatus::Queued => {
                        ui::pill(ui, "в очереди", MUTED);
                        // «пауза для машины» перед авто-повтором: сколько осталось (DC-4); карточка перерисовывается раз в секунду
                        if let Some(until) = hold {
                            ui.label(RichText::new(hold_text(until, std::time::Instant::now())).color(ACCENT));
                            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                        }
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
                    JobStatus::Interrupted => {
                        ui::pill(ui, "прервана", ACCENT);
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
                    JobStatus::Failed | JobStatus::Interrupted if j.fail_state.is_some() => ladder(ui, j, None),
                    _ => {}
                }
                if matches!(j.status, JobStatus::Failed | JobStatus::Interrupted) && j.fail_code.is_some() {
                    error_block(ui, j);
                }
                // у задач, которые не выполняются: сводка длительности этапов и журналы прошлых попыток (D-11, D-13, RET-06)
                if !matches!(j.status, JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling) {
                    if let Some(text) = stage_summary_text(&j.log) {
                        ui.add(egui::Label::new(RichText::new(text).color(MUTED).size(12.5)).wrap());
                    }
                    if let Some(logs) = app.jobs.old_logs.get(&j.id).filter(|l| !l.is_empty()) {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("прошлые попытки:").color(MUTED).size(12.5));
                            for p in logs {
                                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                if ui.small_button(&name).on_hover_text(p.display().to_string()).clicked() {
                                    ui::open_path(p);
                                }
                            }
                        });
                    }
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
        let (resumable, has_dir) = (app.jobs.resumable.contains(&j.id), j.out_dir.is_some());
        let buttons = if server {
            remote_card_buttons(j.status, resumable, has_dir, app.status.busy, held)
        } else {
            card_buttons(j.status, resumable, has_dir, app.status.busy)
        };
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
                        // второе нажатие (D-12): не ждать `quit`, если игра зависла на выходе
                        if ui.small_button("Закрыть игру сразу").on_hover_text("Завершить свой job без ожидания quit — если игра зависла на выходе").clicked()
                        {
                            *act = Some(Act::Cancel(j.id));
                        }
                    }
                    JobStatus::Done => {
                        // ролик карточки сервера после подтверждения сервером удаляется: открывать нечего
                        if let Some(p) = j.result.as_ref().filter(|_| !server || result_exists) {
                            if ui::primary(ui, true, "Открыть") {
                                ui::open_path(p);
                            }
                        }
                        if buttons.open_dir && ui.button("Открыть папку").clicked() {
                            if let Some(d) = &j.out_dir {
                                ui::open_path(d);
                            }
                        }
                    }
                    JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted => {
                        // справа налево: «Повторить» крайняя, левее «Заново», затем «Открыть папку»
                        if let Some(enabled) = buttons.retry {
                            let retry = ui
                                .add_enabled(enabled, egui::Button::new("Повторить"))
                                .on_hover_text("С записи, без скачивания и плана; если все дубли проверены — сразу склейка")
                                .on_disabled_hover_text("Нет данных записи (record.json) — нажмите «Заново»");
                            if retry.clicked() {
                                *act = Some(Act::Retry(j.id));
                            }
                        }
                        if buttons.redo
                            && ui
                                .add_enabled(!app.status.busy, egui::Button::new("Заново"))
                                .on_hover_text("С самого начала: скачивание, план и запись всех дублей")
                                .clicked()
                        {
                            *act = Some(Act::Redo(j.id));
                        }
                        if buttons.open_dir && ui.button("Открыть папку").clicked() {
                            if let Some(d) = &j.out_dir {
                                ui::open_path(d);
                            }
                        }
                    }
                }
                if !matches!(j.status, JobStatus::Running | JobStatus::Cancelling) && !held && ui.small_button("Удалить").clicked() {
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

/// Отметка на ждущей карточке: сколько секунд осталось до авто-повтора (округление вверх, не меньше 0).
pub fn hold_text(until: std::time::Instant, now: std::time::Instant) -> String {
    let left = until.saturating_duration_since(now);
    format!("авто-повтор через {} с", left.as_millis().div_ceil(1000))
}

/// Текст баннера паузы очереди: называет причину (D-16).
pub fn pause_banner(reason: &PauseReason, waiting: usize) -> String {
    let n = ui::plural(waiting, "задача", "задачи", "задач");
    let sep = if matches!(reason, PauseReason::Cancelled) { ":" } else { " —" };
    format!("{}{sep} {n} ждут", queue::pause_text(reason))
}

/// Отчёт предпроверки: свёрнутый заголовок с итогом и строки ✔ / ⚠ / ✖ с сообщением; у `warn` и `fail` под строкой подсказка (PRE-01).
/// `open` — раскрыт ли отчёт сразу (у задачи, упавшей на предпроверке).
pub fn precheck_view(ui: &mut Ui, salt: impl std::hash::Hash, r: &Report, open: bool) {
    let count = |level: Level| r.items.iter().filter(|i| i.level == level).count();
    let (fails, warns) = (count(Level::Fail), count(Level::Warn));
    let title = if fails > 0 {
        format!("Предпроверка: {}", ui::plural(fails, "ошибка", "ошибки", "ошибок"))
    } else if warns > 0 {
        format!("Предпроверка: {}", ui::plural(warns, "предупреждение", "предупреждения", "предупреждений"))
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
        JobStatus::Failed | JobStatus::Interrupted => (j.fail_state.as_deref().and_then(RecordState::from_code), true),
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

/// Строка над сообщением об ошибке: «Попытка N · этап X · КОД · состояние». Пустые части пропускаются (D-13, OBS-02).
fn attempt_line(j: &Job, code: &str, state: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if j.attempt > 0 {
        parts.push(format!("Попытка {}", j.attempt));
    }
    if !j.stage.is_empty() {
        parts.push(format!("этап {}", j.stage));
    }
    parts.extend([code, state].into_iter().filter(|p| !p.is_empty()).map(str::to_owned));
    parts.join(" · ")
}

/// Сводка «по этапам» последней попытки из журнала задачи: «по этапам: Скачивание 12 с · Разбор 4 с». Нет строк этапов — `None` (D-11).
pub fn stage_summary_text(log: &str) -> Option<String> {
    let stages = store::journal::stage_summary(log);
    if stages.is_empty() {
        return None;
    }
    Some(format!("по этапам: {}", stages.iter().map(|(name, dur)| format!("{name} {dur}")).collect::<Vec<_>>().join(" · ")))
}

/// Блок типизированной ошибки: попытка, этап, код и состояние, сообщение, подсказка, время, путь к `record.log` и хвост консоли (D-07).
fn error_block(ui: &mut Ui, j: &Job) {
    let code = j.fail_code.as_deref().unwrap_or_default();
    let state = j.fail_state.as_deref().and_then(RecordState::from_code).map_or("", RecordState::title);
    ui.label(RichText::new(attempt_line(j, code, state)).color(MUTED).size(12.5));
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

#[cfg(test)]
mod tests {
    use super::*;
    use store::{JobSource, NewJob, Store};

    #[test]
    fn pause_banner_explains_a_repeated_environment_failure() {
        let reason = PauseReason::Repeated {
            code: "PRECHECK_FAILED".into(),
            class: pipeline::record::failure::FailClass::Environment,
            jobs: 2,
            message: "нет ffmpeg".into(),
            hint: "установите ffmpeg".into(),
        };
        assert_eq!(
            pause_banner(&reason, 1),
            "Очередь на паузе: 2 задачи подряд упали с кодом PRECHECK_FAILED (окружение). Последняя: нет ffmpeg. установите ffmpeg Исправьте причину и нажмите «Продолжить» — 1 задача ждут"
        );
        let no_hint = PauseReason::Repeated {
            code: "HLAE_ERROR".into(),
            class: pipeline::record::failure::FailClass::Environment,
            jobs: 5,
            message: "сбой.".into(),
            hint: String::new(),
        };
        assert_eq!(
            pause_banner(&no_hint, 3),
            "Очередь на паузе: 5 задач подряд упали с кодом HLAE_ERROR (окружение). Последняя: сбой. Исправьте причину и нажмите «Продолжить» — 3 задачи ждут"
        );
    }

    #[test]
    fn pause_banner_names_the_reason() {
        assert_eq!(pause_banner(&PauseReason::Cancelled, 1), "Очередь на паузе после отмены: 1 задача ждут");
        assert_eq!(pause_banner(&PauseReason::Cancelled, 2), "Очередь на паузе после отмены: 2 задачи ждут");
        assert_eq!(pause_banner(&PauseReason::Interrupted, 1), "Очередь на паузе: задача прервана закрытием приложения — 1 задача ждут");
        assert_eq!(pause_banner(&PauseReason::Interrupted, 2), "Очередь на паузе: задача прервана закрытием приложения — 2 задачи ждут");
    }

    #[test]
    fn hold_mark_counts_seconds_up_and_never_below_zero() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        assert_eq!(hold_text(now + Duration::from_millis(19_200), now), "авто-повтор через 20 с");
        assert_eq!(hold_text(now + Duration::from_secs(1), now), "авто-повтор через 1 с");
        assert_eq!(hold_text(now, now + Duration::from_secs(3)), "авто-повтор через 0 с");
    }

    #[test]
    fn card_offers_retry_only_with_record_json_and_redo_for_finished_jobs() {
        use JobStatus::*;
        for status in [Failed, Interrupted, Cancelled] {
            assert_eq!(card_buttons(status, true, true, false), CardButtons { retry: Some(true), redo: true, open_dir: true }, "{status:?}");
            assert_eq!(card_buttons(status, false, true, false).retry, Some(false), "без record.json «Повторить» видна, но неактивна: {status:?}");
            assert_eq!(card_buttons(status, true, true, true).retry, Some(false), "окно занято: {status:?}");
            assert!(!card_buttons(status, true, false, false).open_dir, "без папки открывать нечего");
        }
        for status in [Running, Cancelling, Queued] {
            assert_eq!(card_buttons(status, true, true, false), CardButtons { retry: None, redo: false, open_dir: false }, "{status:?}");
        }
        assert_eq!(card_buttons(Done, false, true, false), CardButtons { retry: None, redo: false, open_dir: true });
    }

    #[test]
    fn stage_summary_text_lists_the_last_attempt() {
        use std::time::Duration;
        use store::journal::{StartKind, outcome_line, separator, stage_line};
        let log = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            separator(1, "10:00", StartKind::First),
            stage_line("Скачивание", Duration::from_secs(99)),
            outcome_line(1, "сбой"),
            separator(2, "10:30", StartKind::Retry),
            stage_line("Скачивание", Duration::from_secs(12)),
            stage_line("Разбор", Duration::from_secs(4)),
            stage_line("Запись", Duration::from_secs(252)),
            outcome_line(2, "готово"),
        );
        assert_eq!(stage_summary_text(&log).as_deref(), Some("по этапам: Скачивание 12 с · Разбор 4 с · Запись 4 мин 12 с"));
        assert_eq!(stage_summary_text("просто строка\nдругая"), None);
        assert_eq!(stage_summary_text(""), None);
    }

    /// Карточка в базе: локальная или с выдачей сервера.
    fn card(server: bool, map: &str, hero: &str) -> Job {
        let s = Store::in_memory().unwrap();
        let new = NewJob {
            kind: JobKind::Record,
            source: JobSource::File("d.dem".into()),
            label: "Задача с сервера #7".into(),
            hero_name: hero.into(),
            hero_steam: 0,
            map: map.into(),
        };
        let id = if server { s.adopt_remote(&new, "srv:7:1", false, None).unwrap() } else { s.add_job(&new).unwrap() };
        s.job(id).unwrap().unwrap()
    }

    #[test]
    fn remote_card_offers_retry_without_redo_and_nothing_while_held() {
        use JobStatus::*;
        for status in [Failed, Cancelled, Interrupted] {
            assert_eq!(remote_card_buttons(status, true, true, false, false), CardButtons { retry: Some(true), redo: false, open_dir: true }, "{status:?}");
            assert_eq!(remote_card_buttons(status, false, true, false, false).retry, Some(false), "без record.json «Повторить» неактивна: {status:?}");
            assert_eq!(remote_card_buttons(status, true, true, true, false).retry, Some(false), "окно занято: {status:?}");
            assert_eq!(
                remote_card_buttons(status, true, true, false, true),
                CardButtons { retry: None, redo: false, open_dir: false },
                "удержанная карточка без кнопок: {status:?}"
            );
        }
        assert_eq!(remote_card_buttons(Done, true, true, false, false), card_buttons(Done, true, true, false));
        for status in [Running, Cancelling, Queued] {
            assert_eq!(remote_card_buttons(status, true, true, false, false), CardButtons { retry: None, redo: false, open_dir: false }, "{status:?}");
        }
    }

    #[test]
    fn server_card_without_map_and_player_is_titled_by_its_number() {
        assert_eq!(server_heading(&card(true, "", "")).as_deref(), Some("Задача с сервера #7"));
        assert_eq!(server_heading(&card(true, "", "s1mple")), None, "игрок известен — заголовок из карты и игрока");
        assert_eq!(server_heading(&card(true, "de_mirage", "")), None);
        assert_eq!(server_heading(&card(false, "", "")), None, "локальная карточка заголовок не меняет");
    }

    #[test]
    fn server_note_explains_waiting_and_the_deleted_copy() {
        let paused = QueueView { running: None, paused: Some(PauseReason::Cancelled), hold: None };
        let other = QueueView { running: Some((3, crate::jobs::CancelToken::default())), paused: None, hold: None };
        let hold = QueueView { running: None, paused: None, hold: Some((5, std::time::Instant::now())) };
        let free = QueueView { running: None, paused: None, hold: None };
        let reasons = [waiting_reason(&paused, 5), waiting_reason(&other, 5), waiting_reason(&hold, 5), waiting_reason(&free, 5)];
        for (i, a) in reasons.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &reasons[i + 1..] {
                assert_ne!(a, b, "четыре разных причины ожидания");
            }
        }
        assert!(reasons[0].contains("Продолжить"), "{}", reasons[0]);
        assert!(reasons[1].contains("#3"), "{}", reasons[1]);
        assert!(waiting_reason(&hold, 6).contains("другой задачи"), "{}", waiting_reason(&hold, 6));

        let mut j = card(true, "", "");
        j.status = JobStatus::Queued;
        assert_eq!(server_note(&j, true, Some("идёт задача #3"), false).as_deref(), Some("ждёт очереди приложения: идёт задача #3"));
        j.status = JobStatus::Interrupted;
        let note = server_note(&j, true, None, false).unwrap();
        assert!(note.starts_with("продолжится сама"), "{note}");
        assert_eq!(server_note(&j, false, None, false), None, "не удержана — раннер её не продолжит");
        j.status = JobStatus::Done;
        assert_eq!(server_note(&j, false, None, false).as_deref(), Some("ролик отправлен на сервер, локальная копия удалена"));
        assert_eq!(server_note(&j, false, None, true), None, "файл на месте — объяснять нечего");
        let mut local = card(false, "", "");
        local.status = JobStatus::Done;
        assert_eq!(server_note(&local, false, None, false), None, "локальная карточка без пояснений");
    }
}
