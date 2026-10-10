//! Строка состояния: что происходит сейчас (этап и прогресс), последнее сообщение, журнал.

use std::path::PathBuf;

use eframe::egui::{self, RichText, Ui};
use pipeline::Stage;

use crate::ui::{self, ACCENT, BAD, MUTED, OK};

#[derive(Default)]
pub struct Status {
    /// Текущий этап; `None` — ничего не делаем.
    pub stage: Option<Stage>,
    /// Этапы, которые прошли в текущем запуске (пропущенные не подсвечиваем).
    pub passed: Vec<Stage>,
    /// Конвейер работает (кнопки заблокированы).
    pub busy: bool,
    pub error: Option<String>,
    pub last: String,
    pub log: Vec<String>,
    pub show_log: bool,
    /// Готовый ролик (или папка плана) последней задачи — для кнопок «Открыть» / «Папка».
    pub result: Option<PathBuf>,
    /// Запись: записано дублей из скольких.
    pub takes: Option<(u32, u32)>,
    /// Склейка: доля готового ролика.
    pub assemble: Option<f32>,
}

impl Status {
    pub fn start(&mut self, what: &str) {
        self.busy = true;
        self.error = None;
        self.stage = None;
        self.passed.clear();
        self.result = None;
        self.takes = None;
        self.assemble = None;
        self.last = what.to_owned();
        self.push(what);
    }

    pub fn set_stage(&mut self, s: Stage) {
        if let Some(prev) = self.stage.replace(s) {
            if prev != s && !self.passed.contains(&prev) {
                self.passed.push(prev);
            }
        }
        if s != Stage::Record {
            self.takes = None;
        }
        if s != Stage::Assemble {
            self.assemble = None;
        }
    }

    pub fn push(&mut self, m: &str) {
        self.last = m.to_owned();
        self.log.push(format!("{}  {m}", chrono::Local::now().format("%H:%M:%S")));
        if self.log.len() > 500 {
            self.log.remove(0);
        }
        // прогресс записи и склейки — из сообщений конвейера
        if let Some(rest) = m.strip_prefix("записан дубль ") {
            if let Some((a, b)) = rest.split_once('/') {
                if let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse()) {
                    self.takes = Some((a, b));
                }
            }
        }
        if let Some(rest) = m.strip_prefix("склейка: ") {
            if let Some(p) = rest.trim_end_matches('%').trim().parse::<f32>().ok() {
                self.assemble = Some(p / 100.0);
            }
        }
    }

    pub fn finish(&mut self, r: Result<String, String>) {
        self.busy = false;
        self.takes = None;
        self.assemble = None;
        match r {
            Ok(m) => {
                self.set_stage(Stage::Done);
                self.push(&m);
            }
            Err(e) => {
                self.push(&format!("Ошибка: {e}"));
                self.error = Some(e);
            }
        }
    }

    /// Доля пройденного пути по этапам (внутри записи и склейки — по их прогрессу).
    fn fraction(&self) -> f32 {
        let Some(cur) = self.stage else { return 0.0 };
        let i = Stage::ALL.iter().position(|s| *s == cur).unwrap_or(0) as f32;
        let n = (Stage::ALL.len() - 1) as f32;
        let inner = match cur {
            Stage::Record => self.takes.map_or(0.0, |(a, b)| a as f32 / b.max(1) as f32),
            Stage::Assemble => self.assemble.unwrap_or(0.0),
            _ => 0.0,
        };
        ((i + inner) / n).clamp(0.0, 1.0)
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if self.busy {
                ui.spinner();
                let stage = self.stage.map_or("Запуск", Stage::title);
                let detail = match (self.stage, self.takes, self.assemble) {
                    (Some(Stage::Record), Some((a, b)), _) => format!("{stage} · дубль {a}/{b}"),
                    (Some(Stage::Assemble), _, Some(p)) => format!("{stage} · {:.0} %", p * 100.0),
                    _ => stage.to_owned(),
                };
                ui.label(ui::b(detail).color(ACCENT));
                ui.add(egui::ProgressBar::new(self.fraction()).desired_width(180.0).desired_height(10.0).animate(true).fill(ACCENT));
                ui.separator();
                ui.add(egui::Label::new(RichText::new(&self.last).color(MUTED)).truncate());
            } else {
                let (color, glyph) = if self.error.is_some() { (BAD, "●") } else { (OK, "●") };
                ui.label(RichText::new(glyph).color(color));
                let msg = match &self.error {
                    Some(e) => RichText::new(e).color(BAD),
                    None => RichText::new(&self.last),
                };
                let w = ui.available_width() - if self.result.is_some() { 240.0 } else { 90.0 };
                ui.scope(|ui| {
                    ui.set_max_width(w.max(80.0));
                    ui.add(egui::Label::new(msg).truncate());
                });
                if let Some(p) = self.result.clone() {
                    if ui.small_button("Открыть").clicked() {
                        ui::open_path(&p);
                    }
                    if ui.small_button("Папка").clicked() {
                        ui::open_path(p.parent().unwrap_or(&p));
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_log, "Журнал");
            });
        });
        if self.show_log {
            // цепочка этапов — только в развёрнутом журнале
            ui.horizontal(|ui| {
                for (i, st) in Stage::ALL.iter().enumerate() {
                    let (color, text) = match (self.stage, self.error.is_some()) {
                        (Some(cur), _) if *st == cur && cur == Stage::Done => (OK, ui::b(st.title())),
                        (Some(cur), true) if *st == cur => (BAD, ui::b(st.title())),
                        (Some(cur), _) if *st == cur => (ACCENT, ui::b(st.title())),
                        _ if self.passed.contains(st) => (OK, RichText::new(st.title())),
                        _ => (MUTED, RichText::new(st.title())),
                    };
                    if i > 0 {
                        ui.label(RichText::new("›").color(MUTED));
                    }
                    ui.label(text.color(color));
                }
            });
            egui::ScrollArea::vertical().stick_to_bottom(true).max_height(140.0).show(ui, |ui| {
                for l in &self.log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        }
        ui.add_space(2.0);
    }
}
