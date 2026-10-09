//! Состояние окна: страницы, фоновые задачи, сообщения от них.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

use eframe::egui::{self, RichText};
use faceit::stats::{MatchStats, MatchSummary, Player};
use pipeline::{Analysis, Stage};

use crate::{settings::Settings, status::Status, ui, update};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Matches,
    Montage,
    Settings,
}

pub enum Msg {
    Log(String),
    Stage(Stage),
    Analyzed(Result<Analysis, String>),
    /// Анализ возвращается владельцу после планирования/записи.
    Finished { analysis: Analysis, result: Result<PathBuf, String> },
    Player(Result<(Player, Vec<MatchSummary>), String>),
    Stats(String, Result<MatchStats, String>),
    UpdateFound(Result<Option<update::Release>, String>),
    Updated(Result<PathBuf, String>),
}

/// Отчёт конвейера из фонового потока в окно.
pub struct Reporter {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Reporter {
    pub fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }
}

impl pipeline::Progress for Reporter {
    fn stage(&self, s: Stage) {
        self.send(Msg::Stage(s));
    }
    fn log(&self, m: &str) {
        self.send(Msg::Log(m.to_owned()));
    }
}

#[derive(Default)]
pub struct MatchesState {
    pub nickname: String,
    pub player: Option<Player>,
    pub list: Vec<MatchSummary>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected: Option<String>,
    pub stats: HashMap<String, Result<MatchStats, String>>,
    /// Матчи, статистика которых уже запрошена (чтобы не ходить дважды).
    pub requested: std::collections::HashSet<String>,
}

#[derive(Default)]
pub struct MontageState {
    pub analysis: Option<Analysis>,
    pub checked: Vec<bool>,
    pub link: String,
    pub result: Option<PathBuf>,
}

pub struct App {
    pub s: Settings,
    /// Черновик настроек на странице «Настройки».
    pub draft: Settings,
    pub page: Page,
    pub status: Status,
    pub matches: MatchesState,
    pub montage: MontageState,
    pub update: Option<update::Release>,
    pub update_note: Option<String>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    ctx: egui::Context,
    #[cfg(windows)]
    _tray: Option<tray_icon::TrayIcon>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ui::apply_theme(&cc.egui_ctx);
        let (tx, rx) = channel();
        let s = Settings::load();
        cleanup_old();
        let first_run = s.hlae_dir.is_empty() || s.faceit_key.is_empty();
        let mut app = Self {
            draft: s.clone(),
            matches: MatchesState { nickname: s.nickname.clone(), ..Default::default() },
            s,
            page: if first_run { Page::Settings } else { Page::Matches },
            status: Status::default(),
            montage: MontageState::default(),
            update: None,
            update_note: None,
            rx,
            tx,
            ctx: cc.egui_ctx.clone(),
            #[cfg(windows)]
            _tray: None,
        };
        #[cfg(windows)]
        {
            app._tray = crate::tray(&cc.egui_ctx);
        }
        if app.s.auto_update {
            app.check_updates();
        }
        if !app.s.nickname.is_empty() && !app.s.faceit_key.is_empty() {
            app.load_matches();
        }
        app.status.last = "Готов к работе".into();
        // `cs2cinema.exe demo.dem` — открыть демку сразу (и для «Открыть с помощью»)
        if let Some(p) = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_file()) {
            app.open_source(pipeline::Source::File(p));
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("CS2CINEMA_MOCK").is_some() {
            crate::mock::fill(&mut app);
        }
        // для отладки: CS2CINEMA_PAGE=matches|montage|settings
        app.page = match std::env::var("CS2CINEMA_PAGE").as_deref() {
            Ok("matches") => Page::Matches,
            Ok("montage") => Page::Montage,
            Ok("settings") => Page::Settings,
            _ => app.page,
        };
        app
    }

    pub fn reporter(&self) -> Reporter {
        Reporter { tx: self.tx.clone(), ctx: self.ctx.clone() }
    }

    pub fn spawn<F: FnOnce(&Reporter) + Send + 'static>(&self, f: F) {
        let r = self.reporter();
        std::thread::spawn(move || f(&r));
    }

    pub fn check_updates(&mut self) {
        if update::current().is_none() {
            self.update_note = Some("dev-сборка: обновления недоступны".into());
            return;
        }
        self.update_note = Some("проверяю…".into());
        self.spawn(|r| r.send(Msg::UpdateFound(update::check())));
    }

    pub fn load_matches(&mut self) {
        let nick = self.matches.nickname.trim().to_owned();
        if nick.is_empty() || self.matches.loading {
            return;
        }
        if self.s.faceit_key.trim().is_empty() {
            self.matches.error = Some("Укажите ключ FACEIT Data API в настройках".into());
            return;
        }
        if self.s.nickname != nick {
            self.s.nickname = nick.clone();
            self.draft.nickname = nick.clone();
            self.s.save();
        }
        self.matches.loading = true;
        self.matches.error = None;
        let key = self.s.faceit_key.clone();
        self.spawn(move |r| {
            let client = faceit::Client::new(key);
            let res = client.player(&nick).and_then(|p| client.history(&p.id, 20).map(|h| (p, h))).map_err(|e| e.to_string());
            r.send(Msg::Player(res));
        });
    }

    /// Статистика матчей по очереди в одном потоке: для колонки «Карта» и страницы матча.
    pub fn load_stats(&mut self, ids: &[String]) {
        let ids: Vec<String> = ids.iter().filter(|id| self.matches.requested.insert((*id).clone())).cloned().collect();
        if ids.is_empty() {
            return;
        }
        let key = self.s.faceit_key.clone();
        self.spawn(move |r| {
            let client = faceit::Client::new(key);
            for id in ids {
                let res = client.match_stats(&id).map_err(|e| e.to_string());
                r.send(Msg::Stats(id, res));
            }
        });
    }

    pub fn open_source(&mut self, src: pipeline::Source) {
        self.status.start("Открываю демку…");
        self.montage.analysis = None;
        self.montage.checked.clear();
        self.montage.result = None;
        self.page = Page::Montage;
        self.spawn(move |r| {
            let res = pipeline::analyze(&src, r);
            r.send(Msg::Analyzed(res));
        });
    }

    pub fn open_faceit(&mut self, link: &str) {
        let dir = Path::new(&self.s.out_root).join("demos");
        self.open_source(pipeline::Source::Faceit { link: link.to_owned(), key: self.s.faceit_key.clone(), demo_index: 0, dir });
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(t) => self.status.push(&t),
                Msg::Stage(s) => self.status.set_stage(s),
                Msg::Analyzed(r) => match r {
                    Ok(a) => {
                        self.status.finish(Ok(format!("{}: игроков {}, раундов {}, моментов {}", a.m.map, a.m.players.len(), a.m.rounds.len(), a.found.len())));
                        self.montage.checked = (0..a.found.len()).map(|i| i < 8).collect();
                        self.montage.analysis = Some(a);
                    }
                    Err(e) => self.status.finish(Err(e)),
                },
                Msg::Finished { analysis, result } => {
                    self.montage.analysis = Some(analysis);
                    match result {
                        Ok(p) => {
                            self.status.finish(Ok(format!("Готово: {}", p.display())));
                            self.montage.result = Some(p);
                        }
                        Err(e) => self.status.finish(Err(e)),
                    }
                }
                Msg::Player(r) => {
                    self.matches.loading = false;
                    match r {
                        Ok((p, list)) => {
                            self.matches.player = Some(p);
                            let ids: Vec<String> = list.iter().map(|m| m.match_id.clone()).collect();
                            self.matches.list = list;
                            self.load_stats(&ids);
                        }
                        Err(e) => self.matches.error = Some(e),
                    }
                }
                Msg::Stats(id, r) => {
                    self.matches.stats.insert(id, r);
                }
                Msg::UpdateFound(r) => match r {
                    Ok(Some(rel)) => {
                        self.update_note = Some(format!("доступна {}", rel.tag));
                        self.update = Some(rel);
                    }
                    Ok(None) => self.update_note = Some("у вас последняя версия".into()),
                    Err(e) => self.update_note = Some(format!("не удалось проверить: {e}")),
                },
                Msg::Updated(Ok(exe)) => {
                    let _ = std::process::Command::new(exe).spawn();
                    std::process::exit(0);
                }
                Msg::Updated(Err(e)) => self.status.finish(Err(format!("обновление не удалось: {e}"))),
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("CS2 Cinema").strong().size(18.0).color(ui::ACCENT));
        });
        ui.label(RichText::new(update::current().unwrap_or("dev")).weak().small());
        ui.add_space(12.0);
        for (page, title) in [(Page::Matches, "Матчи"), (Page::Montage, "Монтаж"), (Page::Settings, "Настройки")] {
            let sel = self.page == page;
            if ui.add_sized([ui.available_width(), 32.0], egui::SelectableLabel::new(sel, RichText::new(title).size(15.0))).clicked() {
                self.page = page;
            }
        }
        if let Some(r) = self.update.clone() {
            ui.add_space(16.0);
            ui.label(RichText::new(format!("Новая версия {}", r.tag)).color(ui::OK));
            if ui.add_enabled(!self.status.busy, egui::Button::new("Обновить")).clicked() {
                self.status.start("Скачиваю обновление…");
                self.spawn(move |r2| r2.send(Msg::Updated(update::install(&r))));
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(8.0);
            if let Some(p) = &self.matches.player {
                ui.label(RichText::new(format!("{} · lvl {} · {} elo", p.nickname, p.level, p.elo)).small().weak());
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(p) = dropped {
            if !self.status.busy {
                self.open_source(pipeline::Source::File(p));
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.s.minimize_to_tray && cfg!(windows) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status.ui(ui));
        egui::SidePanel::left("nav").exact_width(170.0).show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Matches => crate::matches::ui(self, ui),
            Page::Montage => crate::montage::ui(self, ui),
            Page::Settings => crate::settings_page::ui(self, ui),
        });
        if self.status.busy || self.matches.loading || self.matches.requested.len() > self.matches.stats.len() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
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
