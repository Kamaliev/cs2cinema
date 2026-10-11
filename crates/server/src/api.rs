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
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response};
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

/// Готовый ответ: статус и необязательное JSON-тело.
struct Reply {
    status: u16,
    body: Option<String>,
}

impl Reply {
    fn json(status: u16, value: &impl Serialize) -> Result<Reply, Reject> {
        let body = serde_json::to_string(value)
            .map_err(|e| Reject::from(format!("сериализация ответа: {e}")))?;
        Ok(Reply {
            status,
            body: Some(body),
        })
    }

    fn no_content() -> Reply {
        Reply {
            status: 204,
            body: None,
        }
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
        (Method::Post, p) if p.starts_with("/v1/jobs/") => job_action(ctx, rq, p, trace),
        _ => Err(Reject::new(codes::NOT_FOUND, "нет такого адреса")),
    }
}

/// `POST /v1/jobs/{id}/{действие}`: id — целое больше нуля, иначе 404; токен проверяется до чтения тела.
fn job_action(ctx: &Ctx, rq: &mut Request, path: &str, trace: &mut Trace) -> Result<Reply, Reject> {
    let not_found = || Reject::new(codes::NOT_FOUND, "нет такого адреса");
    let rest = path.strip_prefix("/v1/jobs/").ok_or_else(not_found)?;
    let (id, action) = rest.split_once('/').ok_or_else(not_found)?;
    let job_id = id
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(not_found)?;
    match action {
        "progress" => progress(ctx, rq, job_id, trace),
        "release" => release(ctx, rq, job_id, trace),
        "finish" => finish(ctx, rq, job_id, trace),
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
    let _ = match reply.body {
        Some(body) => {
            let mut r = Response::from_string(body).with_status_code(reply.status);
            if let Some(h) = header("Content-Type", "application/json") {
                r = r.with_header(h);
            }
            rq.respond(r)
        }
        None => rq.respond(Response::empty(reply.status)),
    };
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
