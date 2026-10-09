use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, SystemTime},
};

use eframe::egui::{self, Color32, RichText};
use pipeline::{Analysis, PlanOptions, Source};

use crate::{launch, settings::Settings, update};

enum Msg {
    Log(String),
    Analyzed(Result<Analysis, String>),
    /// Анализ вернулся владельцу после планирования/записи.
    Finished { analysis: Analysis, result: Result<PathBuf, String> },
    UpdateFound(Option<update::Release>),
    Updated(Result<PathBuf, String>),
}

pub struct App {
    s: Settings,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    ctx: egui::Context,
    analysis: Option<Analysis>,
    checked: Vec<bool>,
    faceit_link: String,
    busy: Option<String>,
    log: Vec<String>,
    result: Option<PathBuf>,
    update: Option<update::Release>,
    show_settings: bool,
    #[cfg(windows)]
    _tray: Option<tray_icon::TrayIcon>,
}

/// Иконка приложения: тёмный квадрат, оранжевый круг и «play».
pub fn icon_rgba(n: u32) -> Vec<u8> {
    let mut px = vec![0u8; (n * n * 4) as usize];
    let c = n as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5 - c, y as f32 + 0.5 - c);
            let r = (fx * fx + fy * fy).sqrt();
            let i = ((y * n + x) * 4) as usize;
            let mut rgba = [24u8, 26, 32, 255];
            if r < c * 0.82 {
                rgba = [255, 140, 26, 255];
            }
            // треугольник «play»
            let (tx, ty) = (fx / c, fy / c);
            if tx > -0.25 && tx < 0.4 && ty.abs() < (0.4 - tx) * 0.6 {
                rgba = [255, 255, 255, 255];
            }
            px[i..i + 4].copy_from_slice(&rgba);
        }
    }
    px
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let s = Settings::load();
        cleanup_old();
        let mut app = Self {
            s,
            rx,
            tx,
            ctx: cc.egui_ctx.clone(),
            analysis: None,
            checked: vec![],
            faceit_link: String::new(),
            busy: None,
            log: vec![],
            result: None,
            update: None,
            show_settings: false,
            #[cfg(windows)]
            _tray: None,
        };
        #[cfg(windows)]
        {
            app._tray = make_tray(&cc.egui_ctx);
        }
        if app.s.auto_update && update::current().is_some() {
            let tx = app.tx.clone();
            let ctx = app.ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::UpdateFound(update::check().unwrap_or(None)));
                ctx.request_repaint();
            });
        }
        if app.s.hlae_dir.is_empty() || app.s.cs2_dir.is_empty() {
            app.show_settings = true;
        }
        app
    }

    fn say(&mut self, t: impl Into<String>) {
        self.log.push(t.into());
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn spawn<F: FnOnce(&Sender<Msg>) + Send + 'static>(&self, f: F) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            f(&tx);
            ctx.request_repaint();
        });
    }

    fn logger(tx: &Sender<Msg>, ctx: &egui::Context) -> impl Fn(&str) + use<> {
        let (tx, ctx) = (tx.clone(), ctx.clone());
        move |m: &str| {
            let _ = tx.send(Msg::Log(m.to_owned()));
            ctx.request_repaint();
        }
    }

    fn open_source(&mut self, src: Source) {
        self.busy = Some("Разбираю демку…".into());
        self.analysis = None;
        self.checked.clear();
        self.result = None;
        let ctx = self.ctx.clone();
        self.spawn(move |tx| {
            let log = Self::logger(tx, &ctx);
            let r = pipeline::analyze(&src, &log);
            let _ = tx.send(Msg::Analyzed(r));
        });
    }

    fn open_file(&mut self, p: PathBuf) {
        self.open_source(Source::File(p));
    }

    fn selected(&self) -> Vec<usize> {
        self.checked.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect()
    }

    fn out_dir(&self, a: &Analysis) -> PathBuf {
        Path::new(&self.s.out_root).join(pipeline::console_name(&a.label))
    }

    fn plan_opts(&self) -> PlanOptions {
        PlanOptions { fps: self.s.fps, chronological: self.s.chronological, flybys: self.s.flybys, walls: self.s.walls, ..Default::default() }
    }

    /// `record = false` — только план и скрипты; `true` — ещё запуск игры, запись, склейка и открытие.
    fn start(&mut self, record: bool) {
        let sel = self.selected();
        if sel.is_empty() {
            self.say("Отметьте хотя бы один момент");
            return;
        }
        if record {
            if let Err(e) = launch::check_paths(&self.s.hlae_dir, &self.s.cs2_dir) {
                self.say(e);
                self.show_settings = true;
                return;
            }
        }
        let Some(mut a) = self.analysis.take() else { return };
        self.s.save();
        self.busy = Some(if record { "Запись…".into() } else { "Строю план…".into() });
        let (s, o, out_dir, ctx) = (self.s.clone(), self.plan_opts(), self.out_dir(&a), self.ctx.clone());
        self.spawn(move |tx| {
            let log = Self::logger(tx, &ctx);
            let result = run(&mut a, &sel, &s, &o, &out_dir, record, &log);
            let _ = tx.send(Msg::Finished { analysis: a, result });
        });
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(t) => self.say(t),
                Msg::Analyzed(r) => {
                    self.busy = None;
                    match r {
                        Ok(a) => {
                            self.say(format!("{}: {} игроков, {} раундов, {} моментов", a.m.map, a.m.players.len(), a.m.rounds.len(), a.found.len()));
                            // по умолчанию отмечаем лучшие
                            self.checked = (0..a.found.len()).map(|i| i < 8).collect();
                            self.analysis = Some(a);
                        }
                        Err(e) => self.say(format!("Ошибка: {e}")),
                    }
                }
                Msg::Finished { analysis, result } => {
                    self.busy = None;
                    self.analysis = Some(analysis);
                    match result {
                        Ok(p) => {
                            self.say(format!("Готово: {}", p.display()));
                            self.result = Some(p);
                        }
                        Err(e) => self.say(format!("Ошибка: {e}")),
                    }
                }
                Msg::UpdateFound(r) => self.update = r,
                Msg::Updated(Ok(exe)) => {
                    let _ = Command::new(exe).spawn();
                    std::process::exit(0);
                }
                Msg::Updated(Err(e)) => {
                    self.busy = None;
                    self.say(format!("Обновление не удалось: {e}"));
                }
            }
        }
    }

    fn top(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("CS2 Cinema");
            ui.label(RichText::new(update::current().unwrap_or("dev")).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Настройки").clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
        });
        if let Some(r) = self.update.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(120, 200, 120), format!("Доступна версия {}", r.tag));
                if ui.add_enabled(self.busy.is_none(), egui::Button::new("Обновить и перезапустить")).clicked() {
                    self.busy = Some("Обновляю…".into());
                    self.spawn(move |tx| {
                        let _ = tx.send(Msg::Updated(update::install(&r)));
                    });
                }
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let idle = self.busy.is_none();
            if ui.add_enabled(idle, egui::Button::new("Открыть демку…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Демки CS2", &["dem", "zst", "gz"]).pick_file() {
                    self.open_file(p);
                }
            }
            ui.label("или перетащите .dem в окно, или ссылка FACEIT:");
            ui.add(egui::TextEdit::singleline(&mut self.faceit_link).desired_width(280.0).hint_text("https://www.faceit.com/…/room/1-…"));
            if ui.add_enabled(idle && !self.faceit_link.trim().is_empty(), egui::Button::new("Скачать")).clicked() {
                let dir = Path::new(&self.s.out_root).join("demos");
                let src = Source::Faceit { link: self.faceit_link.clone(), key: self.s.faceit_key.clone(), demo_index: 0, dir };
                self.open_source(src);
            }
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::Grid::new("settings").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                path_row(ui, "Папка HLAE (HLAE.exe)", &mut self.s.hlae_dir);
                path_row(ui, "Папка CS2 (Counter-Strike Global Offensive)", &mut self.s.cs2_dir);
                path_row(ui, "Папка результатов (без пробелов)", &mut self.s.out_root);
                ui.label("Ключ FACEIT Data API");
                ui.add(egui::TextEdit::singleline(&mut self.s.faceit_key).password(true).desired_width(320.0));
                ui.end_row();
                ui.label("Ожидание загрузки демки, c");
                ui.add(egui::DragValue::new(&mut self.s.load_wait_secs).range(5..=180));
                ui.end_row();
                ui.label("FPS записи");
                ui.add(egui::DragValue::new(&mut self.s.fps).range(24..=240));
                ui.end_row();
            });
            ui.checkbox(&mut self.s.flybys, "Пролёты и свободные камеры");
            ui.checkbox(&mut self.s.walls, "Учитывать стены карты (скачает геометрию)");
            ui.checkbox(&mut self.s.chronological, "Моменты по порядку матча (иначе «от слабого к лучшему»)");
            ui.checkbox(&mut self.s.minimize_to_tray, "Крестик сворачивает в трей");
            ui.checkbox(&mut self.s.auto_update, "Проверять обновления при запуске");
            if ui.button("Сохранить").clicked() {
                self.s.save();
                self.show_settings = false;
            }
        });
    }

    fn moments(&mut self, ui: &mut egui::Ui) {
        let Some(a) = &self.analysis else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Перетащите сюда демку (.dem, .dem.zst)").size(20.0).weak());
            });
            return;
        };
        ui.horizontal(|ui| {
            ui.label(format!("{} — {} моментов", a.m.map, a.found.len()));
            if ui.small_button("Лучшие 8").clicked() {
                self.checked.iter_mut().enumerate().for_each(|(i, c)| *c = i < 8);
            }
            if ui.small_button("Все").clicked() {
                self.checked.iter_mut().for_each(|c| *c = true);
            }
            if ui.small_button("Снять").clicked() {
                self.checked.iter_mut().for_each(|c| *c = false);
            }
        });
        let secs: f32 = self
            .checked
            .iter()
            .zip(&a.found)
            .filter(|(c, _)| **c)
            .map(|(_, h)| (h.last_tick() - h.first_tick()) as f32 / a.m.tick_rate + 6.0)
            .sum();
        ui.label(RichText::new(format!("выбрано {}, ролик ≈ {:.0} с", self.checked.iter().filter(|c| **c).count(), secs)).weak());
        egui::ScrollArea::vertical().max_height(ui.available_height() - 150.0).show(ui, |ui| {
            egui::Grid::new("moments").striped(true).num_columns(5).spacing([14.0, 4.0]).show(ui, |ui| {
                for (i, h) in a.found.iter().enumerate() {
                    ui.checkbox(&mut self.checked[i], "");
                    ui.label(format!("R{}", h.round));
                    ui.label(a.m.name(h.player));
                    ui.label(RichText::new(format!("{:.0}", h.score)).strong());
                    ui.label(h.tags.join(", "));
                    ui.end_row();
                }
            });
        });
    }

    fn bottom(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let ready = self.busy.is_none() && self.analysis.is_some() && self.checked.iter().any(|c| *c);
            if ui.add_enabled(ready, egui::Button::new(RichText::new("Записать и собрать ролик").size(16.0)).fill(Color32::from_rgb(200, 100, 20))).clicked() {
                self.start(true);
            }
            if ui.add_enabled(ready, egui::Button::new("Только план и скрипты")).clicked() {
                self.start(false);
            }
            if let Some(p) = self.result.clone() {
                if ui.button("Открыть результат").clicked() {
                    open_path(&p);
                }
            }
            if let Some(b) = &self.busy {
                ui.spinner();
                ui.label(b);
            }
        });
        egui::ScrollArea::vertical().stick_to_bottom(true).max_height(110.0).show(ui, |ui| {
            for l in &self.log {
                ui.label(RichText::new(l).monospace().small());
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(p) = dropped {
            if self.busy.is_none() {
                self.open_file(p);
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.s.minimize_to_tray && cfg!(windows) && self.busy.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| self.bottom(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            self.top(ui);
            if self.show_settings {
                self.settings_ui(ui);
            }
            self.moments(ui);
        });
        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(300));
        }
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(420.0));
    if ui.button("…").clicked() {
        if let Some(p) = rfd::FileDialog::new().pick_folder() {
            *value = p.to_string_lossy().into_owned();
        }
    }
    ui.end_row();
}

/// Весь путь от плана до готового ролика; исполняется в фоновом потоке.
fn run(a: &mut Analysis, sel: &[usize], s: &Settings, o: &PlanOptions, out_dir: &Path, record: bool, log: &dyn Fn(&str)) -> Result<PathBuf, String> {
    let plan = pipeline::plan(a, sel, o, out_dir, log)?;
    log(&format!("План: {} шотов, ~{:.0} с", plan.timeline.shots.len(), plan.timeline.total_secs()));
    if !record {
        return Ok(out_dir.to_path_buf());
    }
    if !renderer::console_safe(&pipeline::absolute_slash(out_dir)) {
        return Err("В пути результатов есть пробелы — HLAE не загрузит campath. Выберите папку без пробелов".into());
    }
    let last = plan.timeline.shots.iter().enumerate().max_by_key(|(_, sh)| sh.start_tick).map(|(i, _)| i).unwrap_or(0);
    let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
    launch::install_files(&s.cs2_dir, out_dir, &demo_name, |p| a.write_demo(p))?;

    let since = SystemTime::now();
    log("Запускаю CS2 через HLAE…");
    let mut child = launch::launch(&s.hlae_dir, &s.cs2_dir, &demo_name)?;
    log(&format!("Жду загрузки демки ({} с)…", s.load_wait_secs));
    std::thread::sleep(Duration::from_secs(s.load_wait_secs as u64));
    match launch::send_console(&["exec highlights"], Duration::from_secs(90)) {
        Ok(()) => log("Скрипт запущен, идёт запись…"),
        Err(e) => log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights")),
    }
    let root = launch::clips_root(&s.cs2_dir);
    launch::wait_for_clips(&root, last, since, &mut || child.try_wait().map(|r| r.is_none()).unwrap_or(false), log)?;

    log("Склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    let opts = renderer::assemble::Options { fps: s.fps, fade_secs: 0.35, out: out.clone() };
    let report = renderer::assemble::assemble(&root, plan.timeline.shots.len(), &opts)?;
    log(&format!("Ролик {:.0} с: {}", report.total_secs, out.display()));
    open_path(&out);
    Ok(out)
}

fn open_path(p: &Path) {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(p);
        let _ = launch::no_window(&mut c).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(p).spawn();
    }
}

fn cleanup_old() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(windows)]
fn make_tray(ctx: &egui::Context) -> Option<tray_icon::TrayIcon> {
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };
    let show = MenuItem::with_id("show", "Открыть", true, None);
    let quit = MenuItem::with_id("quit", "Выход", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit]).ok()?;
    let icon = Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("CS2 Cinema").with_icon(icon).build().ok()?;
    let reveal = |ctx: &egui::Context| {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    };
    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
        "show" => reveal(&c),
        "quit" => std::process::exit(0),
        _ => {}
    }));
    let c = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            reveal(&c);
        }
    }));
    Some(tray)
}
