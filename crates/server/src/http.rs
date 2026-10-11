//! HTTP-слой на `tiny_http` 0.12 (D-01): поток-диспетчер, фиксированный пул обработчиков, 503 при переполнении (D-02).
//!
//! У `tiny_http` три слабости, найденные спайком исследования, и вот как слой их обходит:
//! 1. Нет таймаутов сокета: клиент, который замолчал, держит поток. Поэтому число обработчиков ограничено,
//!    при переполнении сервер отвечает 503 `Retry-After: 2`, а таймаутами соединения занимается прокси перед сервером.
//! 2. Недочитанное тело запроса дочитывается в `Drop` запроса. Поэтому отказ (401, 413, 503) отправляется
//!    до `as_reader`, а 503 уходит из отдельного короткого потока и никогда не из диспетчера: сброс тела блокирует.
//! 3. Обрыв тела клиентом выглядит как чистый EOF. Поэтому обработчики загрузки сверяют число принятых байтов
//!    с `Content-Length` (это делают обработчики PUT, не этот слой).
//!
//! Слой ничего не знает о маршрутах: обработчик приходит замыканием.

use crate::Result;
use crate::log::Log;
use std::net::{SocketAddr, TcpListener};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};
use wire::ApiError;

/// Сколько `stop` ждёт, пока обработчики закончат текущие запросы.
const STOP_GRACE: Duration = Duration::from_secs(3);

pub type Handler = Arc<dyn Fn(Request) + Send + Sync>;

pub struct HttpServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    server: Arc<Server>,
    busy: Arc<AtomicUsize>,
    dispatcher: Option<JoinHandle<()>>,
}

impl HttpServer {
    /// Адрес, на котором слушает сервер (с настоящим портом, если просили порт 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Общий флаг остановки: долгие обработчики (long-poll) смотрят на него.
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    /// Останавливает приём, ждёт диспетчер и до трёх секунд — обработчики. Потоки обработчиков не `join`-ятся:
    /// застрявший клиент может держать поток (слабость 1), и остановка из-за него не должна зависать.
    pub fn stop(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.server.unblock();
        if let Some(d) = self.dispatcher.take() {
            let _ = d.join();
        }
        let until = Instant::now() + STOP_GRACE;
        while self.busy.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Запускает сервер с собственным флагом остановки.
pub fn start(
    listener: TcpListener,
    workers: usize,
    log: Log,
    handler: Handler,
) -> Result<HttpServer> {
    start_with_flag(
        listener,
        workers,
        log,
        handler,
        Arc::new(AtomicBool::new(false)),
    )
}

/// То же с готовым флагом остановки: его заранее кладут в контекст обработчиков.
pub fn start_with_flag(
    listener: TcpListener,
    workers: usize,
    log: Log,
    handler: Handler,
    shutdown: Arc<AtomicBool>,
) -> Result<HttpServer> {
    let workers = workers.max(1);
    let addr = listener
        .local_addr()
        .map_err(|e| format!("адрес слушателя: {e}"))?;
    let server = Arc::new(
        Server::from_listener(listener, None).map_err(|e| format!("запуск HTTP-сервера: {e}"))?,
    );
    let busy = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<Request>();
    let rx = Arc::new(Mutex::new(rx));

    for n in 0..workers {
        let (rx, busy, log, handler) = (rx.clone(), busy.clone(), log.clone(), handler.clone());
        std::thread::Builder::new()
            .name(format!("http-{n}"))
            .spawn(move || {
                loop {
                    let rq = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok(rq) = rq else { return };
                    // Паника обработчика: запрос брошен и tiny_http сам отвечает 500, сервер живёт дальше.
                    if let Err(p) = std::panic::catch_unwind(AssertUnwindSafe(|| handler(rq))) {
                        log.warn(&format!("обработчик упал: {}", panic_text(&*p)));
                    }
                    busy.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .map_err(|e| format!("поток обработчика: {e}"))?;
    }

    let dispatcher = {
        let (server, busy, shutdown, log) =
            (server.clone(), busy.clone(), shutdown.clone(), log.clone());
        std::thread::Builder::new()
            .name("http-dispatcher".to_owned())
            .spawn(move || {
                // `tx` живёт в диспетчере: когда он выходит, канал закрывается и свободные обработчики завершаются.
                loop {
                    if shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    match server.recv_timeout(Duration::from_millis(500)) {
                        Ok(Some(rq)) => {
                            if busy.fetch_add(1, Ordering::SeqCst) >= workers {
                                busy.fetch_sub(1, Ordering::SeqCst);
                                std::thread::spawn(move || overloaded(rq));
                            } else if tx.send(rq).is_err() {
                                busy.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log.warn(&format!("приём соединения: {e}"));
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            })
            .map_err(|e| format!("поток диспетчера: {e}"))?
    };

    Ok(HttpServer {
        addr,
        shutdown,
        server,
        busy,
        dispatcher: Some(dispatcher),
    })
}

/// Ответ 503: сервер занят. Тело запроса не читается, а дочитывается в `Drop` — поэтому отдельный поток.
fn overloaded(rq: Request) {
    let body = serde_json::to_string(&ApiError {
        error: wire::codes::OVERLOADED.to_owned(),
        message: "сервер занят, повторите позже".to_owned(),
        offset: None,
    })
    .unwrap_or_default();
    let mut resp = Response::from_string(body).with_status_code(503);
    for (k, v) in [
        (wire::headers::RETRY_AFTER, "2"),
        ("Content-Type", "application/json"),
    ] {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp = resp.with_header(h);
        }
    }
    let _ = rq.respond(resp);
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "без текста".to_owned()
    }
}
