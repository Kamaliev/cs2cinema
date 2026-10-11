//! Сервер очереди монтажа: своя SQLite, автомат задач с арендой и эпохой, файлы кусками с SHA-256.
//!
//! Крейт не зависит от Windows-крейтов воркспейса (только от `wire`) и собирается на Linux.
//! Правило: ядро (`core`) работает без HTTP и с внедрёнными часами (`clock`), HTTP — тонкая обёртка над ним.
//! Секреты хранятся только в `secret::Secret`, а любой текст, уходящий из процесса, проходит `secret::Redactor`.

pub mod api;
pub mod auth;
pub mod blobs;
pub mod cli;
pub mod clock;
pub mod config;
pub mod core;
pub mod db;
pub mod http;
pub mod log;
pub mod secret;
pub mod users;

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

/// Запущенный сервер: адрес, ядро и журнал; `stop` останавливает HTTP.
pub struct Running {
    http: http::HttpServer,
    core: core::Core,
    log: log::Log,
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

    /// Ставит флаг остановки, будит long-poll и ждёт обработчики не дольше трёх секунд.
    pub fn stop(self) {
        self.http
            .shutdown_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.core.wake();
        self.http.stop();
    }
}

/// Открывает ядро в папке данных, занимает адрес из настроек и запускает HTTP.
pub fn serve(
    cfg: &config::Config,
    clock: std::sync::Arc<dyn clock::Clock>,
    sink: std::sync::Arc<dyn log::LogSink>,
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
    let log = log::Log::new(sink, redactor);
    let listener = std::net::TcpListener::bind(&cfg.bind)
        .map_err(|e| format!("не удалось занять адрес {}: {e}", cfg.bind))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let ctx = api::Ctx {
        core: core.clone(),
        log: log.clone(),
        shutdown: shutdown.clone(),
    };
    let handler: http::Handler = Arc::new(move |rq| api::handle(&ctx, rq));
    let http = http::start_with_flag(listener, cfg.threads, log.clone(), handler, shutdown)?;
    log.info(&format!(
        "cs2-server {} слушает http://{}, данные {}",
        env!("CARGO_PKG_VERSION"),
        http.addr(),
        cfg.data_dir.display()
    ));
    Ok(Running { http, core, log })
}
