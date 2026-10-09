//! Состояние окна: страницы, фоновые задачи, сообщения от них.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

use eframe::egui::{self, RichText};
use cs2::{PlayerId, report::Report};
use faceit::stats::{MatchStats, MatchSummary, Player};
use highlights::{Highlight, Utility};
use pipeline::{Analysis, Stage};
use store::{Job, JobKind, JobSource, NewJob, Store};

use crate::{
    jobs::{self, CancelToken},
    settings::{self, Settings},
    status::Status,
    ui, update,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Player,
    Match,
    Jobs,
    Settings,
}

pub enum Msg {
    Log(String),
    Stage(Stage),
    Analyzed(Result<Analysis, String>),
    /// Задача на монтаж завершилась (успех — путь к ролику или папке плана; `CANCELLED` — отмена).
    JobDone { id: i64, result: Result<PathBuf, String> },
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
    pub stats: HashMap<String, Result<MatchStats, String>>,
    /// Матчи, статистика которых уже запрошена (чтобы не ходить дважды).
    pub requested: std::collections::HashSet<String>,
}

/// Открытый матч: FACEIT-матч из списка или перетащенная демка (`match_id` = `None`).
#[derive(Default)]
pub struct MatchState {
    pub match_id: Option<String>,
    pub analysis: Option<Analysis>,
    /// Про кого разбор и ролик.
    pub hero: Option<PlayerId>,
    pub report: Option<Report>,
    pub moments: Vec<Highlight>,
    pub utility: Vec<Utility>,
}

/// Очередь и история задач на монтаж (сами задачи живут в базе).
#[derive(Default)]
pub struct JobsState {
    pub list: Vec<Job>,
    /// Выполняемая сейчас задача и её токен отмены.
    pub running: Option<(i64, CancelToken)>,
    /// Чей журнал развёрнут на странице задач.
    pub open_log: Option<i64>,
    /// Список надо перечитать из базы (пришли новые строки журнала).
    pub dirty: bool,
}

impl MatchState {
    /// Пересчитать разбор под выбранного игрока.
    pub fn set_hero(&mut self, hero: Option<PlayerId>) {
        self.hero = hero;
        let (Some(a), Some(id)) = (&self.analysis, hero) else {
            self.report = None;
            self.moments.clear();
            self.utility.clear();
            return;
        };
        self.report = Some(cs2::report::player_report(&a.m, id));
        self.moments = highlights::for_player(&a.m, id, &highlights::Config::default());
        self.utility = highlights::utility_for(&a.m, id);
    }
}

pub struct App {
    pub s: Settings,
    /// Черновик настроек на странице «Настройки».
    pub draft: Settings,
    pub page: Page,
    pub status: Status,
    pub matches: MatchesState,
    pub game: MatchState,
    pub jobs: JobsState,
    pub store: Store,
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
        // база: задачи, кэш FACEIT, реестр демок; если не открылась — работаем в памяти
        let (store, store_note) = match Store::open(&settings::data_dir().join("cs2cinema.db")) {
            Ok(st) => (st, None),
            Err(e) => (Store::in_memory().expect("sqlite в памяти"), Some(e)),
        };
        let interrupted = store.reset_running().unwrap_or(0);
        let mut app = Self {
            draft: s.clone(),
            matches: MatchesState { nickname: s.nickname.clone(), ..Default::default() },
            s,
            page: if first_run { Page::Settings } else { Page::Player },
            status: Status::default(),
            game: MatchState::default(),
            jobs: JobsState::default(),
            store,
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
        if let Some(e) = store_note {
            app.status.push(&format!("база не открылась ({e}) — задачи не сохранятся"));
        }
        if interrupted > 0 {
            app.status.push(&format!("задач прервано при прошлом запуске: {interrupted}"));
        }
        app.refresh_jobs();
        // `cs2cinema.exe demo.dem` — открыть демку сразу (и для «Открыть с помощью»)
        if let Some(p) = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_file()) {
            app.open_source(pipeline::Source::File(p));
        } else if !first_run {
            // задачи, оставшиеся в очереди, продолжаем
            jobs::run_next(&mut app);
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("CS2CINEMA_MOCK").is_some() {
            crate::mock::fill(&mut app);
        }
        // для отладки: CS2CINEMA_PAGE=player|match|settings
        app.page = match std::env::var("CS2CINEMA_PAGE").as_deref() {
            Ok("player") => Page::Player,
            Ok("match") => Page::Match,
            Ok("jobs") => Page::Jobs,
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
        let store = self.store.clone();
        self.spawn(move |r| {
            let client = faceit::Client::new(key);
            for id in ids {
                // статистика сыгранного матча не меняется — после первого запроса живёт в базе
                let cache_key = format!("stats:{id}");
                let res = match store.cache_get(&cache_key) {
                    Ok(Some(v)) => Ok(MatchStats::from_json(&v)),
                    _ => client.match_stats_json(&id).map_err(|e| e.to_string()).map(|v| {
                        let _ = store.cache_put(&cache_key, &v);
                        MatchStats::from_json(&v)
                    }),
                };
                r.send(Msg::Stats(id, res));
            }
        });
    }

    pub fn refresh_jobs(&mut self) {
        match self.store.jobs() {
            Ok(list) => self.jobs.list = list,
            Err(e) => self.status.push(&e),
        }
        self.jobs.dirty = false;
    }

    /// Поставить задачу на открытый матч и выбранного игрока; возвращает её id.
    pub fn enqueue(&mut self, kind: JobKind) -> Result<i64, String> {
        let (Some(a), Some(hero)) = (&self.game.analysis, self.game.hero) else { return Err("сначала разберите демку и выберите игрока".into()) };
        let source = match &self.game.match_id {
            Some(id) => {
                let _ = self.store.remember_demo(id, &a.demo_path);
                JobSource::Faceit(id.clone())
            }
            None => JobSource::File(a.demo_path.clone()),
        };
        let label = self
            .game
            .match_id
            .as_ref()
            .and_then(|id| self.matches.list.iter().find(|m| &m.match_id == id))
            .map(|m| ui::date(m.started_at))
            .unwrap_or_else(|| a.label.clone());
        let job = NewJob {
            kind,
            source,
            label,
            hero_name: a.m.name(hero),
            hero_steam: a.m.player(hero).map_or(0, |p| p.xuid),
            map: a.m.map.clone(),
        };
        let id = self.store.add_job(&job)?;
        self.refresh_jobs();
        jobs::run_next(self);
        Ok(id)
    }

    /// Отменить задачу: из очереди — сразу, выполняемую — через токен (поток сам остановится).
    pub fn cancel_job(&mut self, id: i64) {
        match &self.jobs.running {
            Some((rid, token)) if *rid == id => {
                token.cancel();
                self.status.push("отменяю задачу…");
            }
            _ => {
                if let Err(e) = self.store.cancel_job(id) {
                    self.status.push(&e);
                }
            }
        }
        self.refresh_jobs();
    }

    /// Разобрать демку; для файла страница матча открывается без FACEIT-шапки.
    pub fn open_source(&mut self, src: pipeline::Source) {
        self.status.start("Открываю демку…");
        let match_id = match &src {
            pipeline::Source::Faceit { link, .. } => faceit::parse_match_id(link),
            pipeline::Source::File(_) => None,
        };
        self.game = MatchState { match_id, ..Default::default() };
        self.page = Page::Match;
        self.spawn(move |r| {
            let res = pipeline::analyze(&src, r);
            r.send(Msg::Analyzed(res));
        });
    }

    /// Страница FACEIT-матча; демка качается по кнопке (если уже разобрана — остаётся).
    pub fn open_match(&mut self, id: &str) {
        if self.game.match_id.as_deref() != Some(id) {
            self.game = MatchState { match_id: Some(id.to_owned()), ..Default::default() };
        }
        self.load_stats(&[id.to_owned()]);
        self.page = Page::Match;
    }

    /// Скачать и разобрать демку открытого матча.
    pub fn analyze_match(&mut self) {
        let Some(id) = self.game.match_id.clone() else { return };
        let dir = Path::new(&self.s.out_root).join("demos");
        self.open_source(pipeline::Source::Faceit { link: id, key: self.s.faceit_key.clone(), demo_index: 0, dir });
    }

    /// Кого разбирать по умолчанию: игрок с FACEIT-аккаунтом из настроек (по SteamID, затем по нику).
    fn default_hero(&self, a: &Analysis) -> Option<PlayerId> {
        let steam = self.matches.player.as_ref().map(|p| p.steam_id);
        let nick = self.matches.player.as_ref().map_or(self.s.nickname.as_str(), |p| p.nickname.as_str());
        pipeline::find_player(&a.m, steam, nick)
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Log(t) => {
                    self.status.push(&t);
                    self.jobs.dirty = self.jobs.running.is_some();
                }
                Msg::Stage(s) => {
                    self.status.set_stage(s);
                    self.jobs.dirty = self.jobs.running.is_some();
                }
                Msg::Analyzed(r) => match r {
                    Ok(a) => {
                        self.status.finish(Ok(format!("{}: игроков {}, раундов {}", a.m.map, a.m.players.len(), a.m.rounds.len())));
                        let hero = self.default_hero(&a);
                        self.game.analysis = Some(a);
                        self.game.set_hero(hero);
                    }
                    Err(e) => self.status.finish(Err(e)),
                },
                Msg::JobDone { id, result } => {
                    if self.jobs.running.as_ref().is_some_and(|(rid, _)| *rid == id) {
                        self.jobs.running = None;
                    }
                    match result {
                        Ok(p) => self.status.finish(Ok(format!("Задача #{id} готова: {}", p.display()))),
                        Err(e) if e == pipeline::CANCELLED => {
                            self.status.busy = false;
                            self.status.stage = None;
                            self.status.push(&format!("задача #{id} отменена"));
                        }
                        Err(e) => self.status.finish(Err(format!("задача #{id}: {e}"))),
                    }
                    self.refresh_jobs();
                    jobs::run_next(self);
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
        let active = self.jobs.list.iter().filter(|j| j.is_active()).count();
        let jobs_title = if active > 0 { format!("Задачи · {active}") } else { "Задачи".to_owned() };
        for (page, title) in [(Page::Player, "Игрок".to_owned()), (Page::Match, "Матч".to_owned()), (Page::Jobs, jobs_title), (Page::Settings, "Настройки".to_owned())] {
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
        if self.jobs.dirty {
            self.refresh_jobs();
        }
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
            Page::Player => crate::player::ui(self, ui),
            Page::Match => crate::match_page::ui(self, ui),
            Page::Jobs => crate::jobs_page::ui(self, ui),
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
