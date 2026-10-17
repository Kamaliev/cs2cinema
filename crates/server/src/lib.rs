//! Сервер очереди монтажа: своя SQLite, автомат задач с арендой и эпохой, файлы кусками с SHA-256.
//!
//! Крейт не зависит от Windows-крейтов воркспейса (только от `wire`) и собирается на Linux.
//! Правило: ядро (`core`) работает без HTTP и с внедрёнными часами (`clock`), HTTP — тонкая обёртка над ним.
//! Секреты хранятся только в `secret::Secret`, а любой текст, уходящий из процесса, проходит `secret::Redactor`.

pub mod api;
pub mod auth;
pub mod blobs;
pub mod bot;
pub mod clean;
pub mod cli;
pub mod clock;
pub mod config;
pub mod core;
pub mod db;
pub mod http;
pub mod log;
pub mod rules;
pub mod secret;
pub mod tg;
pub mod users;
pub mod webapp;

pub type Result<T> = std::result::Result<T, String>;

/// Строчное шестнадцатеричное представление байтов.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Запущенный сервер: адрес, ядро и журнал; `stop` останавливает бота и HTTP.
pub struct Running {
    http: http::HttpServer,
    core: core::Core,
    log: log::Log,
    sweeper: Option<std::thread::JoinHandle<()>>,
    cleaner: Option<std::thread::JoinHandle<()>>,
    bot: Option<bot::BotHandle>,
}

impl Running {
    pub fn addr(&self) -> std::net::SocketAddr {
        self.http.addr()
    }

    pub fn core(&self) -> &core::Core {
        &self.core
    }

    pub fn log(&self) -> &log::Log {
        &self.log
    }

    /// Бот, если он запущен (`serve_with_bot` с клиентом Telegram).
    pub fn bot(&self) -> Option<&bot::Bot> {
        self.bot.as_ref().map(bot::BotHandle::bot)
    }

    /// Останавливает бота (ждёт потоки не дольше трёх секунд), ставит флаг остановки, будит long-poll
    /// и ждёт обработчики не дольше трёх секунд.
    pub fn stop(mut self) {
        if let Some(b) = self.bot.take() {
            b.stop();
        }
        self.http
            .shutdown_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.core.wake();
        if let Some(h) = self.sweeper.take() {
            let _ = h.join();
        }
        if let Some(h) = self.cleaner.take() {
            let _ = h.join();
        }
        self.http.stop();
    }
}

/// Открывает ядро в папке данных, занимает адрес из настроек и запускает HTTP.
/// Бот запускается, если заданы `bot_api_url` и токен (клиент — `HttpTg` на локальный `telegram-bot-api`).
pub fn serve(
    cfg: &config::Config,
    clock: std::sync::Arc<dyn clock::Clock>,
    sink: std::sync::Arc<dyn log::LogSink>,
) -> Result<Running> {
    let tg: Option<std::sync::Arc<dyn tg::Tg>> = match (&cfg.bot_api_url, &cfg.telegram_bot_token) {
        (Some(url), Some(token)) => Some(std::sync::Arc::new(tg::http::HttpTg::new(
            url,
            token,
            cfg.redactor(),
            cfg.http_timeouts(),
        )?)),
        (Some(_), None) => {
            return Err(
                "бот не запустился: задан bot_api_url, а токена бота нет (TELEGRAM_BOT_TOKEN)"
                    .to_owned(),
            );
        }
        _ => None,
    };
    serve_with_bot(cfg, clock, sink, tg)
}

/// То же, что `serve`, но клиент Telegram внедряется: с `Some` бот запускается (в тестах — `FakeTelegram`),
/// с `None` сервер работает без бота.
pub fn serve_with_bot(
    cfg: &config::Config,
    clock: std::sync::Arc<dyn clock::Clock>,
    sink: std::sync::Arc<dyn log::LogSink>,
    tg: Option<std::sync::Arc<dyn tg::Tg>>,
) -> Result<Running> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let redactor = cfg.redactor();
    let core = core::Core::open(core::CoreOptions {
        data_dir: cfg.data_dir.clone(),
        limits: cfg.limits(),
        clock,
        redactor: redactor.clone(),
    })?;
    let log = log::Log::new(sink, redactor.clone());
    // Восстановление до открытия порта: раннеры ещё не могли вернуться и увидеть полузавершённое состояние.
    let rec = core.recover()?;
    log.info(&format!(
        "восстановление: упавших приёмов {}, продлённых аренд {}",
        rec.receive_failed.len(),
        rec.grace_extended
    ));
    // Бот проверяется до порта: недоступный bot-api не должен оставлять сервер наполовину запущенным.
    let bot = match tg {
        Some(tg) => {
            if cfg.bot_api_files_root.is_none() {
                return Err("бот не запустился: не задан bot_api_files_root".to_owned());
            }
            let bot = bot::Bot::new(core.clone(), tg, log.clone(), cfg.bot_settings());
            let me = bot.startup().map_err(|e| {
                redactor.redact(&format!(
                    "бот не запустился: локальный telegram-bot-api не отвечает (getMe) — проверьте bot_api_url и что telegram-bot-api запущен с --local ({e})"
                ))
            })?;
            Some((bot, me))
        }
        None => None,
    };
    let listener = std::net::TcpListener::bind(&cfg.bind)
        .map_err(|e| format!("не удалось занять адрес {}: {e}", cfg.bind))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let ctx = api::Ctx {
        core: core.clone(),
        log: log.clone(),
        shutdown: shutdown.clone(),
        webapp: cfg.telegram_bot_token.clone().map(webapp::Auth::new),
    };
    let handler: http::Handler = Arc::new(move |rq| api::handle(&ctx, rq));
    let shutdown_flag = shutdown.clone();
    let http = http::start_with_flag(listener, cfg.threads, log.clone(), handler, shutdown)?;
    log.info(&format!(
        "cs2-server {} слушает http://{}, данные {}",
        env!("CARGO_PKG_VERSION"),
        http.addr(),
        cfg.data_dir.display()
    ));
    // Рабочую папку bot-api чистим, только когда бот запущен: иначе корень в настройках может быть не задан.
    let botapi_root = bot.as_ref().map(|_| cfg.bot_settings().files_root_local);
    let bot = bot.map(|(bot, me)| {
        let name = me.username.unwrap_or_else(|| "без имени".to_owned());
        log.info(&format!("бот @{name} запущен через локальный Bot API"));
        bot::spawn(bot, shutdown_flag.clone())
    });
    let sweeper = spawn_sweeper(core.clone(), log.clone(), shutdown_flag.clone());
    let cleaner = spawn_cleaner(
        core.clone(),
        log.clone(),
        shutdown_flag,
        cfg.retention(),
        botapi_root,
        cfg.clean_interval_secs,
    );
    Ok(Running {
        http,
        core,
        log,
        sweeper: Some(sweeper),
        cleaner: Some(cleaner),
        bot,
    })
}

/// Поток, раз в 5 секунд возвращающий в очередь задачи с истёкшей арендой; выходит по флагу остановки.
fn spawn_sweeper(
    core: core::Core,
    log: log::Log,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    std::thread::spawn(move || {
        'outer: loop {
            // Шаги по 200 мс: остановка не ждёт целых 5 секунд.
            for _ in 0..25 {
                if stop.load(Ordering::SeqCst) {
                    break 'outer;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            match core.sweep() {
                Ok(s) => {
                    for id in &s.requeued {
                        log.info(&format!(
                            "аренда задачи #{id} истекла — задача снова в очереди"
                        ));
                    }
                    for id in &s.cancelled {
                        log.info(&format!("аренда задачи #{id} истекла — задача отменена"));
                    }
                    for id in &s.receive_failed {
                        log.info(&format!("приём демки задачи #{id} завис — задача упала"));
                    }
                }
                Err(e) => log.warn(&format!("sweep: {e}")),
            }
        }
    })
}

/// Поток уборки: раз в `interval_secs` один проход `Core::clean`; в журнал уходят только числа. Выходит по флагу
/// остановки, ожидая шагами по 200 мс, как `spawn_sweeper`.
fn spawn_cleaner(
    core: core::Core,
    log: log::Log,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    retention: clean::Retention,
    botapi_root: Option<std::path::PathBuf>,
    interval_secs: u64,
) -> std::thread::JoinHandle<()> {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    std::thread::spawn(move || {
        'outer: loop {
            for _ in 0..interval_secs.saturating_mul(5) {
                if stop.load(Ordering::SeqCst) {
                    break 'outer;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            match core.clean(&retention, botapi_root.as_deref(), clean::CleanMode::Apply) {
                Ok(c) if !c.is_empty() => log.info(&format!("уборка: {}", c.summary())),
                Ok(_) => {}
                Err(e) => log.warn(&format!("уборка: {e}")),
            }
        }
    })
}
