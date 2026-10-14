//! Тонкая обёртка над ядром: маршруты `/v1/...`, вход по токену, JSON и журнал запросов (D-05).
//!
//! Вся логика живёт в `core`; здесь только разбор запроса, ответ и строка журнала. Токен читается
//! из заголовка `Authorization` до чтения тела; в журнал идут метод, путь без query, статус и имя раннера,
//! заголовки не пишутся.

use crate::auth::{self, AuthError, Runner};
use crate::core::{Core, Reject};
use crate::log::Log;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::{Read, Take};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, StatusCode};
use wire::{
    ApiError, ClaimRequest, FinishRequest, HelloRequest, Outcome, ProgressRequest, ReleaseRequest,
    codes,
};

/// Самое большое JSON-тело запроса: 64 КиБ.
pub const MAX_JSON: usize = 64 * 1024;

/// Всё, что нужно обработчикам: ядро, журнал и флаг остановки (его же смотрит HTTP-слой).
#[derive(Clone)]
pub struct Ctx {
    pub core: Core,
    pub log: Log,
    pub shutdown: Arc<AtomicBool>,
}

/// Готовый ответ: статус, заголовки и необязательное тело (JSON, кусок файла или только длина для `HEAD`).
struct Reply {
    status: u16,
    body: Option<String>,
    headers: Vec<(&'static str, String)>,
    /// Кусок файла: читатель и его длина (ответ 206 на скачивание демки).
    file: Option<(Take<File>, usize)>,
    /// `HEAD`: пустое тело с `Content-Length` настоящего файла.
    head_len: Option<usize>,
}

impl Reply {
    fn json(status: u16, value: &impl Serialize) -> Result<Reply, Reject> {
        let body = serde_json::to_string(value)
            .map_err(|e| Reject::from(format!("сериализация ответа: {e}")))?;
        Ok(Reply {
            body: Some(body),
            ..Reply::bare(status)
        })
    }

    fn bare(status: u16) -> Reply {
        Reply {
            status,
            body: None,
            headers: Vec::new(),
            file: None,
            head_len: None,
        }
    }

    fn no_content() -> Reply {
        Reply::bare(204)
    }

    fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Reply {
        self.headers.push((name, value.into()));
        self
    }
}

/// Кто делает запрос: заполняется, когда токен опознан, и попадает в строку журнала.
#[derive(Default)]
struct Trace {
    runner: Option<String>,
}

pub fn handle(ctx: &Ctx, mut rq: Request) {
    let started = Instant::now();
    let method = rq.method().clone();
    let path = rq.url().split('?').next().unwrap_or("").to_owned();
    let mut trace = Trace::default();

    let result = route(ctx, &mut rq, &method, &path, &mut trace);
    let (status, detail) = match result {
        Ok(reply) => {
            let status = reply.status;
            respond(rq, reply);
            (status, None)
        }
        Err(rej) => {
            let status = status_of(rej.code);
            respond_error(rq, &rej);
            (status, rej.detail)
        }
    };

    ctx.log.info(&format!(
        "{} {} → {} за {} мс, раннер {}",
        method,
        shorten(&path),
        status,
        started.elapsed().as_millis(),
        trace.runner.as_deref().unwrap_or("—")
    ));
    if let Some(d) = detail {
        ctx.log.warn(&format!("{method} {}: {d}", shorten(&path)));
    }
}

fn shorten(path: &str) -> String {
    path.chars().take(200).collect()
}

fn route(
    ctx: &Ctx,
    rq: &mut Request,
    method: &Method,
    path: &str,
    trace: &mut Trace,
) -> Result<Reply, Reject> {
    match (method, path) {
        (Method::Post, wire::paths::CLAIM) => claim(ctx, rq, trace),
        (Method::Post, wire::paths::HELLO) => hello(ctx, rq, trace),
        (Method::Post, wire::paths::PAIR) => pair(ctx, rq),
        (_, p) if p.starts_with("/v1/jobs/") => job_action(ctx, rq, method, p, trace),
        _ => Err(Reject::new(codes::NOT_FOUND, "нет такого адреса")),
    }
}

/// `/v1/jobs/{id}/{действие}`: id — целое больше нуля, иначе 404; токен проверяется до чтения тела.
fn job_action(
    ctx: &Ctx,
    rq: &mut Request,
    method: &Method,
    path: &str,
    trace: &mut Trace,
) -> Result<Reply, Reject> {
    let not_found = || Reject::new(codes::NOT_FOUND, "нет такого адреса");
    let rest = path.strip_prefix("/v1/jobs/").ok_or_else(not_found)?;
    let (id, action) = rest.split_once('/').ok_or_else(not_found)?;
    let job_id = id
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(not_found)?;
    match (method, action) {
        (Method::Post, "progress") => progress(ctx, rq, job_id, trace),
        (Method::Post, "release") => release(ctx, rq, job_id, trace),
        (Method::Post, "finish") => finish(ctx, rq, job_id, trace),
        (Method::Head, "result") => head_result(ctx, rq, job_id, trace),
        (Method::Put, "result") => put_result(ctx, rq, job_id, trace),
        (Method::Head, "demo") => head_demo(ctx, rq, job_id, trace),
        (Method::Get, "demo") => get_demo(ctx, rq, job_id, trace),
        _ => Err(not_found()),
    }
}

/// `POST /v1/jobs/{id}/progress`: прогресс раннера, он же heartbeat.
fn progress(ctx: &Ctx, rq: &mut Request, job_id: i64, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let req: ProgressRequest = read_json(rq, MAX_JSON)?;
    let (reply, reattached) = ctx.core.progress(runner.id, job_id, &req)?;
    if reattached {
        ctx.log.info(&format!(
            "раннер {} вернул себе задачу #{job_id} (эпоха {})",
            runner.name, req.epoch
        ));
    }
    Reply::json(200, &reply)
}

/// `POST /v1/jobs/{id}/release`: раннер возвращает задачу в очередь.
fn release(ctx: &Ctx, rq: &mut Request, job_id: i64, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let req: ReleaseRequest = read_json(rq, MAX_JSON)?;
    ctx.core.release(runner.id, job_id, &req)?;
    let reason = ctx.core.redactor().clean(&req.reason, 200);
    ctx.log.info(&format!(
        "раннер {} вернул задачу #{job_id}: {}",
        runner.name,
        if reason.is_empty() {
            "без причины"
        } else {
            &reason
        }
    ));
    Ok(Reply::no_content())
}

/// `POST /v1/jobs/{id}/finish`: раннер сообщает итог задачи (сбой или отмену).
fn finish(ctx: &Ctx, rq: &mut Request, job_id: i64, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let req: FinishRequest = read_json(rq, MAX_JSON)?;
    let done = ctx.core.finish_report(runner.id, job_id, &req)?;
    if let Some(e) = &done.discard_error {
        ctx.log.warn(e);
    }
    // В журнал идёт только код сбоя после очистки (из базы), а не текст раннера.
    let code = match (&req.outcome, ctx.core.job(job_id)) {
        (Outcome::Failed(_), Ok(Some(job))) => job.fail_code.unwrap_or_else(|| "—".to_owned()),
        _ => "—".to_owned(),
    };
    ctx.log.info(&format!(
        "задача #{job_id}: {} ({code}), раннер {}",
        done.reply.state.as_str(),
        runner.name
    ));
    Reply::json(200, &done.reply)
}

/// Заголовок как целое число; нет или не число — 400.
fn header_number<T: std::str::FromStr>(rq: &Request, name: &str) -> Result<T, Reject> {
    rq.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .and_then(|h| h.value.as_str().trim().parse::<T>().ok())
        .ok_or_else(|| {
            Reject::new(
                codes::BAD_REQUEST,
                format!("нужен заголовок {name} с целым числом"),
            )
        })
}

/// `HEAD /v1/jobs/{id}/result`: сколько байт результата сервер уже принял в эту эпоху.
fn head_result(
    ctx: &Ctx,
    rq: &mut Request,
    job_id: i64,
    trace: &mut Trace,
) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let epoch: i64 = header_number(rq, wire::headers::EPOCH)?;
    let offset = ctx.core.result_offset(runner.id, job_id, epoch)?;
    Ok(Reply::bare(200).with_header(wire::headers::UPLOAD_OFFSET, offset.to_string()))
}

/// `PUT /v1/jobs/{id}/result`: один кусок результата не больше `CHUNK_SIZE`, дописывается в `.part` эпохи.
///
/// Порядок проверок важен (Pitfall 3 исследования): всё, что отвергает запрос, идёт до чтения тела, поэтому
/// `100 Continue` отвергнутому запросу не отправляется, а тело не принимается зря.
fn put_result(
    ctx: &Ctx,
    rq: &mut Request,
    job_id: i64,
    trace: &mut Trace,
) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let epoch: i64 = header_number(rq, wire::headers::EPOCH)?;
    let offset: u64 = header_number(rq, wire::headers::UPLOAD_OFFSET)?;
    let len = match rq.body_length() {
        None => {
            return Err(Reject::new(
                codes::LENGTH_REQUIRED,
                "нужен Content-Length: кусок без длины или chunked не принимается",
            ));
        }
        Some(0) => return Err(Reject::new(codes::BAD_REQUEST, "пустой кусок")),
        Some(n) if n as u64 > wire::CHUNK_SIZE => {
            return Err(Reject::new(
                codes::TOO_LARGE,
                format!("кусок больше {} МиБ", wire::CHUNK_SIZE / (1024 * 1024)),
            ));
        }
        Some(n) => n as u64,
    };
    // Ограждение эпохой, `-> uploading` и продление аренды.
    ctx.core.touch_upload(runner.id, job_id, epoch)?;
    let Some(guard) = ctx.core.blobs().lock_upload(job_id, epoch) else {
        let mut busy = Reject::new(
            codes::UPLOAD_BUSY,
            "кусок этой задачи уже принимается: повторите через секунду",
        );
        busy.headers
            .push((wire::headers::RETRY_AFTER, "1".to_owned()));
        return Err(busy);
    };
    let actual = ctx
        .core
        .blobs()
        .part_len(job_id, epoch)
        .map_err(|e| Reject::from(format!("задача #{job_id}: длина .part: {e}")))?;
    if offset != actual {
        // Смещение никогда не «перематывается»: отказ и для меньшего, и для большего, тело не читается.
        let mut r = Reject::new(
            codes::OFFSET_MISMATCH,
            format!("сервер принял {actual} байт, а не {offset}: продолжайте с {actual}"),
        );
        r.offset = Some(actual);
        return Err(r);
    }
    let max = ctx.core.limits().max_result_bytes;
    if offset.checked_add(len).is_none_or(|end| end > max) {
        return Err(Reject::new(
            codes::TOO_LARGE,
            format!("результат больше {} МиБ", max / (1024 * 1024)),
        ));
    }
    // Только теперь читается тело: `100 Continue` уйдёт лишь принятому запросу.
    let got = ctx
        .core
        .blobs()
        .append_chunk(&guard, offset, len, rq.as_reader());
    if got.got != len {
        // Обрыв выглядит как чистый EOF (Pitfall 1) или как ошибка чтения: принятые байты остались на диске,
        // следующий `HEAD` покажет настоящее смещение.
        ctx.log.warn(&format!(
            "кусок задачи #{job_id} оборван: принято {} из {len} байт",
            got.got
        ));
        let mut r = Reject::new(
            codes::BAD_REQUEST,
            "кусок оборван: узнайте смещение через HEAD",
        );
        r.detail = got.error;
        return Err(r);
    }
    if let Some(e) = got.error {
        return Err(Reject::from(format!("задача #{job_id}: запись куска: {e}")));
    }
    ctx.core.touch_upload(runner.id, job_id, epoch)?;
    Ok(Reply::no_content().with_header(wire::headers::UPLOAD_OFFSET, (offset + len).to_string()))
}

/// `HEAD /v1/jobs/{id}/demo`: размер и SHA-256 демки; аренда продлевается.
fn head_demo(ctx: &Ctx, rq: &mut Request, job_id: i64, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let epoch: i64 = header_number(rq, wire::headers::EPOCH)?;
    let demo = ctx.core.demo_for(runner.id, job_id, epoch)?;
    let mut reply = Reply::bare(200)
        .with_header("ETag", format!("\"{}\"", demo.sha256))
        .with_header("Accept-Ranges", "bytes")
        .with_header("Content-Type", "application/octet-stream");
    reply.head_len = Some(usize::try_from(demo.size).unwrap_or(usize::MAX));
    Ok(reply)
}

/// Разбор `Range`: только `bytes=N-` и `bytes=N-M` (`M >= N`); без заголовка — с начала файла.
fn parse_range(rq: &Request) -> Result<(u64, Option<u64>), Reject> {
    let bad = || {
        Reject::new(
            codes::BAD_REQUEST,
            "поддерживается только Range: bytes=N- или bytes=N-M",
        )
    };
    let Some(h) = rq.headers().iter().find(|h| h.field.equiv("Range")) else {
        return Ok((0, None));
    };
    let spec = h
        .value
        .as_str()
        .trim()
        .strip_prefix("bytes=")
        .ok_or_else(bad)?;
    let (a, b) = spec.split_once('-').ok_or_else(bad)?;
    let start = a.parse::<u64>().map_err(|_| bad())?;
    if b.is_empty() {
        return Ok((start, None));
    }
    let end = b.parse::<u64>().map_err(|_| bad())?;
    if end < start {
        return Err(bad());
    }
    Ok((start, Some(end)))
}

/// `GET /v1/jobs/{id}/demo`: кусок демки по `Range`. На один ответ приходится не больше `CHUNK_SIZE` байт,
/// раннер повторяет запросы с нового смещения; `Content-Length` настоящий (без chunked, Pitfall 5).
fn get_demo(ctx: &Ctx, rq: &mut Request, job_id: i64, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let epoch: i64 = header_number(rq, wire::headers::EPOCH)?;
    let (start, end) = parse_range(rq)?;
    let demo = ctx.core.demo_for(runner.id, job_id, epoch)?;
    if start >= demo.size {
        let mut r = Reject::new(
            codes::RANGE_NOT_SATISFIABLE,
            format!("смещение {start} за концом файла ({} байт)", demo.size),
        );
        r.headers
            .push(("Content-Range", format!("bytes */{}", demo.size)));
        return Err(r);
    }
    let (reader, len, size) = ctx
        .core
        .blobs()
        .open_demo_range(job_id, start, end)
        .map_err(|e| Reject::from(format!("задача #{job_id}: чтение демки: {e}")))?;
    let mut reply = Reply::bare(206)
        .with_header(
            "Content-Range",
            format!("bytes {start}-{}/{size}", start + len - 1),
        )
        .with_header("ETag", format!("\"{}\"", demo.sha256))
        .with_header("Accept-Ranges", "bytes")
        .with_header("Content-Type", "application/octet-stream");
    reply.file = Some((reader, usize::try_from(len).unwrap_or(usize::MAX)));
    Ok(reply)
}

/// `POST /v1/runner/hello`: раннер представляется и сверяет удерживаемые задачи. Версия протокола проверяется
/// до любых записей хэндлера: чужая версия — 426.
fn hello(ctx: &Ctx, rq: &mut Request, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let req: HelloRequest = read_json(rq, MAX_JSON)?;
    if req.protocol != wire::PROTOCOL_VERSION {
        return Err(Reject::new(
            codes::PROTOCOL_MISMATCH,
            format!(
                "версия протокола раннера {}, сервера {}: обновите раннер",
                req.protocol,
                wire::PROTOCOL_VERSION
            ),
        ));
    }
    let reply = ctx.core.hello(runner.id, &req)?;
    ctx.log.info(&format!(
        "раннер {} на связи ({}), задач: {}",
        runner.name,
        ctx.core.redactor().clean(&req.app_version, 64),
        req.holding.len()
    ));
    for d in &reply.directives {
        if d.action == wire::DirectiveAction::Abandon {
            ctx.log.info(&format!(
                "раннер {}: задача #{} ему больше не принадлежит (abandon)",
                runner.name, d.job_id
            ));
        }
    }
    Reply::json(200, &reply)
}

/// `POST /v1/runner/pair`: заявка на подключение через Telegram, без токена. Серверу приходит только хеш
/// придуманного раннером токена; в ответ — код, который админ называет боту (`/runner код`).
fn pair(ctx: &Ctx, rq: &mut Request) -> Result<Reply, Reject> {
    let req: wire::PairRequest = read_json(rq, MAX_JSON)?;
    let name = req.name.trim();
    let name = if name.is_empty() { "runner" } else { name };
    match auth::request_pairing(ctx.core.db(), name, &req.token_hash, ctx.core.clock().now_ms()) {
        Ok(p) => {
            ctx.log.info(&format!(
                "заявка раннера «{}» на подключение через Telegram принята, ждёт кода у админа",
                p.name
            ));
            Reply::json(
                200,
                &wire::PairReply {
                    code: p.code,
                    expires_in_secs: (auth::PAIR_TTL_MS / 1000) as u64,
                },
            )
        }
        Err(auth::PairError::TooMany) => Err(Reject::new(
            codes::OVERLOADED,
            auth::PairError::TooMany.to_string(),
        )),
        Err(auth::PairError::Invalid(why)) => Err(Reject::new(codes::BAD_REQUEST, why)),
        Err(auth::PairError::Db(e)) => Err(Reject::from(e)),
    }
}

/// `POST /v1/runner/claim?wait=N`: раннер просит задачу; сервер ждёт её до `min(N, longpoll_secs)` секунд.
fn claim(ctx: &Ctx, rq: &mut Request, trace: &mut Trace) -> Result<Reply, Reject> {
    let runner = authenticate(ctx, rq)?;
    trace.runner = Some(runner.name.clone());
    let wait = wait_secs(rq.url(), ctx.core.limits().longpoll_secs)?;
    let _req: ClaimRequest = read_json(rq, MAX_JSON)?;

    // Ожидание идёт по реальному времени (`Instant`), не по `Clock`: ядро не ждёт, виртуальные часы не участвуют.
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        // Поколение читается до проверки очереди: пробуждение между проверкой и ожиданием не теряется.
        let seen = ctx.core.work_generation();
        let got = ctx.core.claim_report(runner.id)?;
        for id in &got.failed {
            ctx.log
                .warn(&format!("задача #{id}: превышено число выдач"));
        }
        if let Some(a) = got.assignment {
            return Reply::json(200, &a);
        }
        let now = Instant::now();
        if now >= deadline || ctx.shutdown.load(Ordering::SeqCst) {
            return Ok(Reply::no_content());
        }
        // Шаг в 1 с нужен и для `submit` из другого процесса: он не может разбудить `Condvar` этого процесса.
        ctx.core
            .wait_for_work_since(seen, (deadline - now).min(Duration::from_secs(1)));
    }
}

/// Значение `wait` из query: без параметра — предел сервера, иначе не больше предела.
fn wait_secs(url: &str, limit: u64) -> Result<u64, Reject> {
    let Some((_, query)) = url.split_once('?') else {
        return Ok(limit);
    };
    for pair in query.split('&') {
        if let Some(("wait", v)) = pair.split_once('=') {
            return v.parse::<u64>().map(|n| n.min(limit)).map_err(|_| {
                Reject::new(
                    codes::BAD_REQUEST,
                    "параметр wait должен быть целым числом секунд",
                )
            });
        }
    }
    Ok(limit)
}

/// Узнаёт раннера по `Authorization: Bearer {токен}`; тело не трогает.
pub fn authenticate(ctx: &Ctx, rq: &Request) -> Result<Runner, Reject> {
    let header = rq
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_owned());
    let token = header.as_deref().and_then(|v| {
        let (scheme, rest) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("Bearer").then(|| rest.trim())
    });
    let Some(token) = token else {
        return Err(unauthorized(
            "нужен заголовок Authorization: Bearer и токен раннера",
        ));
    };
    match auth::authenticate(ctx.core.db(), token, ctx.core.clock().now_ms()) {
        Ok(runner) => Ok(runner),
        Err(AuthError::Malformed) => Err(unauthorized(
            "нужен заголовок Authorization: Bearer и токен раннера",
        )),
        Err(AuthError::Unknown { fingerprint }) => {
            ctx.log
                .warn(&format!("вход отклонён: токен {fingerprint} неизвестен"));
            Err(unauthorized("токен раннера неизвестен или отозван"))
        }
        Err(AuthError::Revoked { fingerprint }) => {
            ctx.log
                .warn(&format!("вход отклонён: токен {fingerprint} отозван"));
            Err(unauthorized("токен раннера неизвестен или отозван"))
        }
        Err(AuthError::Db(e)) => Err(Reject::from(e)),
    }
}

fn unauthorized(message: &str) -> Reject {
    Reject::new(codes::UNAUTHORIZED, message)
}

/// Читает JSON-тело не больше `max` байт. Пустое тело читается как `{}`.
/// Текст ошибки `serde_json` не выводится (в нём бывают значения), только строка и столбец.
pub fn read_json<T: DeserializeOwned>(rq: &mut Request, max: usize) -> Result<T, Reject> {
    if rq.body_length().is_some_and(|n| n > max) {
        return Err(too_large(max));
    }
    let mut bytes = Vec::new();
    rq.as_reader()
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Reject::from(format!("чтение тела запроса: {e}")))?;
    if bytes.len() > max {
        return Err(too_large(max));
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        bytes = b"{}".to_vec();
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        Reject::new(
            codes::BAD_REQUEST,
            format!("неверный JSON: строка {}, столбец {}", e.line(), e.column()),
        )
    })
}

fn too_large(max: usize) -> Reject {
    Reject::new(
        codes::TOO_LARGE,
        format!("тело запроса больше {} КиБ", max / 1024),
    )
}

fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

fn respond(rq: Request, reply: Reply) {
    let extra: Vec<Header> = reply
        .headers
        .iter()
        .filter_map(|(k, v)| header(k, v))
        .collect();
    // Ответы с файлом и на `HEAD` идут с настоящим `Content-Length`: без порога chunked ответ больше 32 КиБ
    // ушёл бы частями и длина пропала бы (Pitfall 5).
    if let Some((reader, len)) = reply.file {
        let r = Response::new(StatusCode(reply.status), extra, reader, Some(len), None)
            .with_chunked_threshold(usize::MAX);
        let _ = rq.respond(r);
    } else if let Some(len) = reply.head_len {
        let r = Response::new(
            StatusCode(reply.status),
            extra,
            std::io::empty(),
            Some(len),
            None,
        )
        .with_chunked_threshold(usize::MAX);
        let _ = rq.respond(r);
    } else if let Some(body) = reply.body {
        let mut r = Response::from_string(body).with_status_code(reply.status);
        if let Some(h) = header("Content-Type", "application/json") {
            r = r.with_header(h);
        }
        for h in extra {
            r = r.with_header(h);
        }
        let _ = rq.respond(r);
    } else {
        let mut r = Response::empty(reply.status);
        for h in extra {
            r = r.with_header(h);
        }
        let _ = rq.respond(r);
    }
}

/// Ответ 200/201/… с JSON-телом.
pub fn respond_json(rq: Request, status: u16, value: &impl Serialize) {
    match Reply::json(status, value) {
        Ok(reply) => respond(rq, reply),
        Err(rej) => respond_error(rq, &rej),
    }
}

/// Ответ с ошибкой: `ApiError` в JSON; при `offset` ещё заголовок `Upload-Offset`.
pub fn respond_error(rq: Request, rej: &Reject) {
    let body = serde_json::to_string(&ApiError {
        error: rej.code.to_owned(),
        message: rej.message.clone(),
        offset: rej.offset,
    })
    .unwrap_or_default();
    let mut r = Response::from_string(body).with_status_code(status_of(rej.code));
    if let Some(h) = header("Content-Type", "application/json") {
        r = r.with_header(h);
    }
    if let Some(off) = rej.offset
        && let Some(h) = header(wire::headers::UPLOAD_OFFSET, &off.to_string())
    {
        r = r.with_header(h);
    }
    for (name, value) in &rej.headers {
        if let Some(h) = header(name, value) {
            r = r.with_header(h);
        }
    }
    let _ = rq.respond(r);
}

/// HTTP-статус для кода ошибки API.
pub fn status_of(code: &str) -> u16 {
    match code {
        codes::UNAUTHORIZED => 401,
        codes::BAD_REQUEST => 400,
        codes::NOT_FOUND => 404,
        codes::LENGTH_REQUIRED => 411,
        codes::TOO_LARGE => 413,
        codes::RANGE_NOT_SATISFIABLE => 416,
        codes::SHA256_MISMATCH => 422,
        codes::PROTOCOL_MISMATCH => 426,
        codes::OVERLOADED => 503,
        codes::STALE_EPOCH
        | codes::JOB_TERMINAL
        | codes::ALREADY_HOLDING
        | codes::OFFSET_MISMATCH
        | codes::UPLOAD_BUSY => 409,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_is_clamped_to_the_server_limit() {
        assert_eq!(wait_secs("/v1/runner/claim", 25).unwrap(), 25);
        assert_eq!(wait_secs("/v1/runner/claim?wait=0", 25).unwrap(), 0);
        assert_eq!(wait_secs("/v1/runner/claim?wait=999", 25).unwrap(), 25);
        assert_eq!(wait_secs("/v1/runner/claim?x=1&wait=7", 25).unwrap(), 7);
        assert!(wait_secs("/v1/runner/claim?wait=abc", 25).is_err());
    }

    #[test]
    fn every_error_code_has_the_documented_status() {
        for (code, status) in [
            (codes::UNAUTHORIZED, 401),
            (codes::BAD_REQUEST, 400),
            (codes::NOT_FOUND, 404),
            (codes::LENGTH_REQUIRED, 411),
            (codes::TOO_LARGE, 413),
            (codes::RANGE_NOT_SATISFIABLE, 416),
            (codes::SHA256_MISMATCH, 422),
            (codes::PROTOCOL_MISMATCH, 426),
            (codes::OVERLOADED, 503),
            (codes::INTERNAL, 500),
            (codes::STALE_EPOCH, 409),
            (codes::JOB_TERMINAL, 409),
            (codes::ALREADY_HOLDING, 409),
            (codes::OFFSET_MISMATCH, 409),
            (codes::UPLOAD_BUSY, 409),
        ] {
            assert_eq!(status_of(code), status, "{code}");
        }
    }
}
