//! Строка состояния: этапы конвейера, текущее действие, журнал.

use eframe::egui::{self, RichText, Ui};
use pipeline::Stage;

use crate::ui::{ACCENT, BAD, MUTED, OK};

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
}

impl Status {
    pub fn start(&mut self, what: &str) {
        self.busy = true;
        self.error = None;
        self.stage = None;
        self.passed.clear();
        self.last = what.to_owned();
        self.push(what);
    }

    pub fn set_stage(&mut self, s: Stage) {
        if let Some(prev) = self.stage.replace(s) {
            if prev != s && !self.passed.contains(&prev) {
                self.passed.push(prev);
            }
        }
    }

    pub fn push(&mut self, m: &str) {
        self.last = m.to_owned();
        self.log.push(format!("{}  {m}", chrono::Local::now().format("%H:%M:%S")));
        if self.log.len() > 500 {
            self.log.remove(0);
        }
    }

    pub fn finish(&mut self, r: Result<String, String>) {
        self.busy = false;
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

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (i, st) in Stage::ALL.iter().enumerate() {
                let (color, text) = match (self.stage, self.error.is_some()) {
                    (Some(cur), _) if *st == cur && cur == Stage::Done => (OK, RichText::new(st.title()).strong()),
                    (Some(cur), true) if *st == cur => (BAD, RichText::new(st.title()).strong()),
                    (Some(cur), _) if *st == cur => (ACCENT, RichText::new(st.title()).strong()),
                    _ if self.passed.contains(st) => (OK, RichText::new(st.title())),
                    _ => (MUTED, RichText::new(st.title())),
                };
                if i > 0 {
                    ui.label(RichText::new("›").color(MUTED));
                }
                ui.label(text.color(color));
            }
            ui.separator();
            if self.busy {
                ui.spinner();
            }
            let msg = match &self.error {
                Some(e) => RichText::new(e).color(BAD),
                None => RichText::new(&self.last),
            };
            ui.add(egui::Label::new(msg).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_log, "Журнал");
            });
        });
        if self.show_log {
            egui::ScrollArea::vertical().stick_to_bottom(true).max_height(140.0).show(ui, |ui| {
                for l in &self.log {
                    ui.label(RichText::new(l).monospace().small());
                }
            });
        }
    }
}
