//! Передача файлов кусками через настоящий HTTP на localhost (D-05, SRV-06): результат раннера грузится по 8 МиБ
//! с докачкой после обрыва, демка отдаётся по `Range`, а поведение `tiny_http`, на котором держится протокол
//! (`Expect: 100-continue`, обрыв тела, ответы больше 32 КиБ), закреплено тестами. Время двигают ручные часы.

mod common;

use common::{
    TestOptions, TestServer, agent, fake_demo, raw_put_head, raw_request, read_reply,
    read_status_line, result_bytes, sha256_hex,
};
use server::clock::ManualClock;
use std::io::Write;
use std::net::Shutdown;
use std::time::Duration;
use wire::{CHUNK_SIZE, JobState, Phase, ProgressRequest};

const T0: i64 = 1_800_000_000_000;
const MIB: usize = 1024 * 1024;

fn clocked() -> TestServer {
    TestServer::start_with(TestOptions {
        clock: Some(ManualClock::new(T0)),
        ..Default::default()
    })
}

fn record(epoch: i64, seq: u64) -> ProgressRequest {
    ProgressRequest {
        epoch,
        seq,
        phase: Phase::Record,
        take: None,
        status: String::new(),
    }
}

/// Задача в очереди, раннер с токеном, выдача (эпоха 1) и один прогресс `record`.
fn running_job(srv: &TestServer) -> (i64, String) {
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let token = srv.add_runner("a");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 1);
    assert_eq!(srv.progress(&token, id, &record(1, 1)).0, 200);
    (id, token)
}

/// Строки `files` задачи данного вида: размер и SHA-256.
fn file_rows(srv: &TestServer, id: i64, kind: &str) -> Vec<(String, i64, String)> {
    srv.running()
        .core()
        .db()
        .read(|c| {
            let mut st = c
                .prepare("SELECT rel_path, size, sha256 FROM files WHERE job_id = ?1 AND kind = ?2")
                .map_err(|e| e.to_string())?;
            let rows = st
                .query_map(rusqlite::params![id, kind], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())
        })
        .unwrap()
}

fn parts_in(srv: &TestServer, id: i64) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(srv.results_dir(id)) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect()
}

#[test]
fn result_uploads_in_chunks_and_closes_the_job() {
    let srv = clocked();
    let clock = srv.clock();
    let (id, token) = running_job(&srv);
    let data = result_bytes(17 * MIB);
    let sha = sha256_hex(&data);

    assert_eq!(srv.head_offset(&token, id, 1), (200, Some(0)));
    assert_eq!(srv.job(id).state, JobState::Running);

    let mut offset = 0u64;
    let mut last_lease = srv.job(id).lease_until.unwrap();
    for (n, len) in [8 * MIB, 8 * MIB, MIB].into_iter().enumerate() {
        clock.advance(Duration::from_secs(5));
        let chunk = &data[offset as usize..offset as usize + len];
        let (status, off, body) = srv.put_chunk(&token, id, 1, offset, chunk);
        assert_eq!(status, 204, "кусок {n}: {body}");
        offset += len as u64;
        assert_eq!(off, Some(offset), "кусок {n}");
        let job = srv.job(id);
        assert_eq!(job.state, JobState::Uploading, "после куска {n}");
        let lease = job.lease_until.unwrap();
        assert!(lease > last_lease, "кусок {n} продлевает аренду");
        last_lease = lease;
    }
    assert_eq!(offset, 17 * MIB as u64);
    assert_eq!(srv.head_offset(&token, id, 1), (200, Some(offset)));
    assert!(
        file_rows(&srv, id, "result").is_empty(),
        "до finish результата нет"
    );

    let (status, body) = srv.finish_done(&token, id, 1, offset, &sha);
    assert_eq!(
        status,
        200,
        "{body}
{}",
        srv.sink_text()
    );
    assert_eq!(body, r#"{"state":"done"}"#);

    let result = srv.results_dir(id).join("highlights.mp4");
    assert!(
        std::fs::read(&result).unwrap() == data,
        "файл результата не совпал побайтно"
    );
    assert!(parts_in(&srv, id).is_empty(), "файлов .part не осталось");
    let rows = file_rows(&srv, id, "result");
    assert_eq!(
        rows,
        vec![(
            format!("blobs/results/{id}/highlights.mp4"),
            offset as i64,
            sha.clone()
        )]
    );
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Done);
    assert!(job.finished_at.is_some());

    // Ответ мог потеряться: тот же итог повторяется и остаётся успехом.
    let (status, body) = srv.finish_done(&token, id, 1, offset, &sha);
    assert_eq!(status, 200, "{body}");
    assert_eq!(file_rows(&srv, id, "result").len(), 1);
}

#[test]
fn chunk_size_constant_matches_the_documented_eight_mebibytes() {
    assert_eq!(CHUNK_SIZE, 8 * MIB as u64);
}

// ---------------------------------------------------------------------------------------------------------
// Обрыв, докачка, отказы и ограждение эпохой (Task 2)
// ---------------------------------------------------------------------------------------------------------

/// Ждёт, пока условие станет истинным (опрос раз в 20 мс), но не дольше 10 секунд.
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(std::time::Instant::now() < until, "не дождались: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn aborted_chunk_keeps_its_bytes_and_upload_resumes() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);
    let total = 4 * MIB;
    let data = result_bytes(total);

    // Заявлено 4 МиБ, отправлено 2 МиБ, затем запись в сокет закрыта: для сервера это чистый EOF тела.
    let mut conn = raw_request(srv.addr());
    conn.write_all(
        raw_put_head(&token, id, 1, 0, &format!("Content-Length: {total}\r\n")).as_bytes(),
    )
    .unwrap();
    conn.write_all(&data[..2 * MIB]).unwrap();
    conn.shutdown(Shutdown::Write).unwrap();
    // Строка статуса пришла - обработчик закончил запись на диск и отвечает.
    let status = read_status_line(&mut conn);
    assert!(status.contains(" 400 "), "обрыв куска: {status}");

    let (code, off) = srv.head_offset(&token, id, 1);
    let off = off.expect("HEAD отдаёт смещение");
    assert_eq!(code, 200);
    assert!(
        off > 0 && off <= 2 * MIB as u64,
        "принятые байты остались на диске: смещение {off}"
    );
    assert!(
        srv.sink_text().contains("оборван"),
        "обрыв записан в журнал: {}",
        srv.sink_text()
    );

    // Докачка с настоящего смещения и итог с полным SHA-256.
    let (code, new_off, body) = srv.put_chunk(&token, id, 1, off, &data[off as usize..]);
    assert_eq!(code, 204, "{body}");
    assert_eq!(new_off, Some(total as u64));
    let (code, body) = srv.finish_done(&token, id, 1, total as u64, &sha256_hex(&data));
    assert_eq!(code, 200, "{body}");
    assert!(std::fs::read(srv.results_dir(id).join("highlights.mp4")).unwrap() == data);
    assert_eq!(srv.job(id).state, JobState::Done);
}

#[test]
fn wrong_offset_is_refused_with_the_real_one_and_the_connection_survives() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);

    // Через клиента с общим соединением.
    let shared = agent();
    let (code, off, body) = srv.put_chunk_on(&shared, &token, id, 1, 5, &vec![7u8; MIB]);
    assert_eq!(code, 409, "{body}");
    assert_eq!(
        off,
        Some(0),
        "заголовок Upload-Offset с настоящим смещением"
    );
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "offset_mismatch");
    assert_eq!(json["offset"], 0);
    assert_eq!(srv.head_offset_on(&shared, &token, id, 1), (200, Some(0)));

    // Сырое соединение: тело отправлено целиком, ответ 409, и на том же соединении работает следующий запрос.
    let mut conn = raw_request(srv.addr());
    let body = vec![9u8; MIB];
    conn.write_all(
        raw_put_head(
            &token,
            id,
            1,
            5,
            &format!("Content-Length: {}\r\n", body.len()),
        )
        .as_bytes(),
    )
    .unwrap();
    conn.write_all(&body).unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 409, "{}", reply.status_line);
    assert_eq!(reply.header("upload-offset"), Some("0"));
    conn.write_all(
        format!(
            "HEAD /v1/jobs/{id}/result HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {token}\r\nX-Epoch: 1\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let next = read_reply(&mut conn, true);
    assert_eq!(next.status(), 200, "{}", next.status_line);
    assert_eq!(next.header("upload-offset"), Some("0"));

    // Меньшее смещение тоже отказ: смещение не «перематывается».
    let data = result_bytes(2 * MIB);
    assert_eq!(srv.put_chunk(&token, id, 1, 0, &data[..MIB]).0, 204);
    let (code, off, _) = srv.put_chunk(&token, id, 1, 0, &data[..MIB]);
    assert_eq!((code, off), (409, Some(MIB as u64)));
    let (code, off, _) = srv.put_chunk(&token, id, 1, 2 * MIB as u64, &data[..MIB]);
    assert_eq!((code, off), (409, Some(MIB as u64)));
}

#[test]
fn chunk_without_length_or_too_big_is_refused() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);

    // chunked без Content-Length
    let mut conn = raw_request(srv.addr());
    conn.write_all(raw_put_head(&token, id, 1, 0, "Transfer-Encoding: chunked\r\n").as_bytes())
        .unwrap();
    conn.write_all(b"5\r\nhello\r\n0\r\n\r\n").unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 411, "{}", reply.status_line);
    assert!(String::from_utf8_lossy(&reply.body).contains("length_required"));

    // больше 8 МиБ: тело не отправляется, отказ приходит до него
    let mut conn = raw_request(srv.addr());
    let too_big = CHUNK_SIZE + 1;
    conn.write_all(
        raw_put_head(&token, id, 1, 0, &format!("Content-Length: {too_big}\r\n")).as_bytes(),
    )
    .unwrap();
    let status = read_status_line(&mut conn);
    assert!(status.contains(" 413 "), "{status}");
    drop(conn);

    // пустой кусок
    let mut conn = raw_request(srv.addr());
    conn.write_all(raw_put_head(&token, id, 1, 0, "Content-Length: 0\r\n").as_bytes())
        .unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 400, "{}", reply.status_line);

    // не число в заголовках протокола
    let mut conn = raw_request(srv.addr());
    conn.write_all(
        format!(
            "PUT /v1/jobs/{id}/result HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {token}\r\nX-Epoch: x\r\nUpload-Offset: 0\r\nContent-Length: 0\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 400, "{}", reply.status_line);

    assert_eq!(srv.head_offset(&token, id, 1), (200, Some(0)));
    assert!(
        parts_in(&srv, id).is_empty(),
        "отказанные куски ничего не записали"
    );
}

#[test]
fn size_or_sha_mismatch_is_not_a_result() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);
    let data = result_bytes(MIB);
    let sha = sha256_hex(&data);
    assert_eq!(srv.put_chunk(&token, id, 1, 0, &data).0, 204);
    let size = data.len() as u64;

    // Размер на 1 больше: настоящий размер в ответе, задача остаётся uploading, байты на месте.
    let (code, body) = srv.finish_done(&token, id, 1, size + 1, &sha);
    assert_eq!(code, 409, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "offset_mismatch");
    assert_eq!(json["offset"], size);
    assert_eq!(srv.job(id).state, JobState::Uploading);
    assert_eq!(srv.head_offset(&token, id, 1), (200, Some(size)));

    // Неверный SHA-256 при верном размере: 422, .part удалён, результата нет.
    let wrong = sha256_hex(b"other");
    let (code, body) = srv.finish_done(&token, id, 1, size, &wrong);
    assert_eq!(code, 422, "{body}");
    assert!(body.contains("sha256_mismatch"), "{body}");
    assert!(parts_in(&srv, id).is_empty(), ".part удалён");
    assert!(!srv.results_dir(id).join("highlights.mp4").exists());
    assert!(file_rows(&srv, id, "result").is_empty());
    assert_eq!(srv.head_offset(&token, id, 1), (200, Some(0)));
    assert_eq!(srv.job(id).state, JobState::Uploading);

    // Заведомо неверные описания итога - 400, а не 500.
    for (s, h) in [(0u64, sha.as_str()), (size, "abc"), (size, &"z".repeat(64))] {
        let (code, body) = srv.finish_done(&token, id, 1, s, h);
        assert_eq!(code, 400, "{body}");
    }

    // Загрузка заново с нуля и верный итог.
    assert_eq!(srv.put_chunk(&token, id, 1, 0, &data).0, 204);
    let (code, body) = srv.finish_done(&token, id, 1, size, &sha);
    assert_eq!(code, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Done);
}

#[test]
fn stale_epoch_cannot_upload_or_finish() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(4096), "s1mple");
    let old = srv.add_runner("old");
    let new = srv.add_runner("new");
    assert_eq!(srv.claim(&old, 0).0, 200);
    assert_eq!(srv.progress(&old, id, &record(1, 1)).0, 200);
    let first = result_bytes(MIB);
    assert_eq!(srv.put_chunk(&old, id, 1, 0, &first).0, 204);

    // Раннер замолчал, задачу взял другой: эпоха 2.
    clock.advance(Duration::from_secs(46));
    srv.sweep();
    let (status, got) = srv.claim(&new, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 2);

    assert_eq!(srv.head_offset(&old, id, 1).0, 409);
    let (code, _, body) = srv.put_chunk(&old, id, 1, MIB as u64, &first);
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("stale_epoch"), "{body}");
    let (code, body) = srv.finish_done(&old, id, 1, MIB as u64, &sha256_hex(&first));
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("stale_epoch"), "{body}");
    let demo = srv.get_range(&old, id, 1, None);
    assert_eq!(demo.status, 409);
    assert!(String::from_utf8_lossy(&demo.body).contains("stale_epoch"));
    // Чужая эпоха у правильного раннера - то же самое.
    assert_eq!(srv.head_offset(&new, id, 1).0, 409);

    // Новый раннер пишет в свою .part; кусок эпохи 1 не тронут.
    let second = result_bytes(2 * MIB);
    assert_eq!(srv.head_offset(&new, id, 2), (200, Some(0)));
    assert_eq!(srv.put_chunk(&new, id, 2, 0, &second).0, 204);
    let dir = srv.results_dir(id);
    assert!(std::fs::read(dir.join("1.part")).unwrap() == first);
    assert!(std::fs::read(dir.join("2.part")).unwrap() == second);
    assert_eq!(srv.job(id).state, JobState::Uploading);

    let (code, body) = srv.finish_done(&new, id, 2, second.len() as u64, &sha256_hex(&second));
    assert_eq!(code, 200, "{body}");
    assert!(std::fs::read(dir.join("highlights.mp4")).unwrap() == second);
    assert!(parts_in(&srv, id).is_empty(), "куски других эпох убраны");
}

#[test]
fn concurrent_upload_of_the_same_epoch_is_busy() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);
    let data = result_bytes(MIB);
    let half = MIB / 2;

    // Первый кусок принят наполовину: соединение держит PUT с недосланным телом.
    let mut first = raw_request(srv.addr());
    first
        .write_all(raw_put_head(&token, id, 1, 0, &format!("Content-Length: {MIB}\r\n")).as_bytes())
        .unwrap();
    first.write_all(&data[..half]).unwrap();
    let part = srv.results_dir(id).join("1.part");
    wait_until("первые байты на диске", || {
        std::fs::metadata(&part)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    });

    let mut second = raw_request(srv.addr());
    second
        .write_all(raw_put_head(&token, id, 1, 0, "Content-Length: 1024\r\n").as_bytes())
        .unwrap();
    second.write_all(&data[..1024]).unwrap();
    let busy = read_reply(&mut second, false);
    assert_eq!(busy.status(), 409, "{}", busy.status_line);
    assert_eq!(busy.header("retry-after"), Some("1"));
    assert!(String::from_utf8_lossy(&busy.body).contains("upload_busy"));

    // Первый дописан - занятость снята.
    first.write_all(&data[half..]).unwrap();
    let done = read_reply(&mut first, false);
    assert_eq!(done.status(), 204, "{}", done.status_line);
    assert_eq!(done.header("upload-offset"), Some(MIB.to_string().as_str()));
    let more = result_bytes(2 * MIB);
    let (code, off, body) = srv.put_chunk(&token, id, 1, MIB as u64, &more[MIB..]);
    assert_eq!((code, off), (204, Some(2 * MIB as u64)), "{body}");
}

#[test]
fn late_upload_after_cancel_is_refused_and_parts_are_deleted() {
    let srv = clocked();
    let clock = srv.clock();
    let (id, token) = running_job(&srv);
    let data = result_bytes(MIB);
    assert_eq!(srv.put_chunk(&token, id, 1, 0, &data).0, 204);
    assert_eq!(parts_in(&srv, id).len(), 1);

    // Оператор отменяет, раннер подтверждает отмену.
    srv.running().core().cancel(id, false).unwrap();
    let cancelled = wire::FinishRequest {
        epoch: 1,
        outcome: wire::Outcome::Cancelled,
    };
    let (code, body) = srv.finish(&token, id, &cancelled);
    assert_eq!(code, 200, "{body}");
    assert_eq!(srv.job(id).state, JobState::Cancelled);
    assert!(parts_in(&srv, id).is_empty(), "куски отменённой убраны");

    // Поздний кусок: отказ, а байты, успевшие появиться, удалены.
    std::fs::create_dir_all(srv.results_dir(id)).unwrap();
    std::fs::write(srv.results_dir(id).join("1.part"), b"late").unwrap();
    let (code, _, body) = srv.put_chunk(&token, id, 1, 0, &data);
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("job_terminal"), "{body}");
    assert!(parts_in(&srv, id).is_empty(), "поздние байты удалены");
    assert_eq!(srv.head_offset(&token, id, 1).0, 409);

    // Вторая задача: отмена запрошена, раннер молчит, аренда истекает - задача отменена.
    let second = srv.submit(&fake_demo(4096), "s1mple");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().job_id, second);
    assert_eq!(srv.put_chunk(&token, second, 1, 0, &data).0, 204);
    srv.running().core().cancel(second, false).unwrap();
    clock.advance(Duration::from_secs(46));
    srv.sweep();
    assert_eq!(srv.job(second).state, JobState::Cancelled);
    assert!(parts_in(&srv, second).is_empty());
    let (code, _, body) = srv.put_chunk(&token, second, 1, MIB as u64, &data);
    assert_eq!(code, 409, "{body}");
    assert!(parts_in(&srv, second).is_empty());
}

// ---------------------------------------------------------------------------------------------------------
// Демка по Range и поведение tiny_http (Task 3)
// ---------------------------------------------------------------------------------------------------------

/// Демка 20 МиБ, выданная раннеру (эпоха 1): id, токен и описание демки из выдачи.
fn demo_job(srv: &TestServer) -> (i64, String, wire::DemoInfo) {
    let id = srv.submit(&fake_demo(20 * MIB), "s1mple");
    let token = srv.add_runner("a");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let a = got.unwrap();
    assert_eq!((a.job_id, a.epoch), (id, 1));
    assert_eq!(a.demo.size, 20 * MIB as u64);
    (id, token, a.demo)
}

#[test]
fn demo_downloads_in_ranges_and_matches_its_sha256() {
    let srv = TestServer::start();
    let (id, token, demo) = demo_job(&srv);
    let size = demo.size;

    let (status, headers) = srv.head_demo(&token, id, 1);
    let h = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(status, 200);
    assert_eq!(h("content-length"), Some("20971520".to_owned()));
    assert_eq!(h("etag"), Some(format!("\"{}\"", demo.sha256)));
    assert_eq!(h("accept-ranges"), Some("bytes".to_owned()));
    assert_eq!(h("transfer-encoding"), None);

    // Раннер повторяет запросы с нового смещения, пока не получит всё.
    let mut whole = Vec::new();
    let mut lens = Vec::new();
    while (whole.len() as u64) < size {
        let off = whole.len() as u64;
        let r = srv.get_range(&token, id, 1, Some(&format!("bytes={off}-")));
        assert_eq!(r.status, 206, "смещение {off}");
        let last = off + r.body.len() as u64 - 1;
        assert_eq!(
            r.header("content-range"),
            Some(format!("bytes {off}-{last}/{size}").as_str())
        );
        assert_eq!(
            r.header("content-length"),
            Some(r.body.len().to_string().as_str()),
            "настоящий Content-Length"
        );
        assert_eq!(r.header("transfer-encoding"), None, "без chunked");
        assert_eq!(
            r.header("etag"),
            Some(format!("\"{}\"", demo.sha256).as_str())
        );
        assert!(r.body.len() as u64 <= CHUNK_SIZE);
        lens.push(r.body.len());
        whole.extend_from_slice(&r.body);
    }
    assert_eq!(lens, vec![8 * MIB, 8 * MIB, 4 * MIB]);
    assert_eq!(
        sha256_hex(&whole),
        demo.sha256,
        "склейка совпала с SHA-256 из выдачи"
    );
    assert!(whole == fake_demo(20 * MIB));
}

#[test]
fn small_and_open_ranges_follow_the_protocol() {
    let srv = TestServer::start();
    let (id, token, _) = demo_job(&srv);
    let all = fake_demo(20 * MIB);

    let r = srv.get_range(&token, id, 1, Some("bytes=0-99"));
    assert_eq!(r.status, 206);
    assert_eq!(r.body.len(), 100);
    assert_eq!(r.header("content-range"), Some("bytes 0-99/20971520"));
    assert!(r.body == all[..100]);

    let r = srv.get_range(&token, id, 1, Some("bytes=10-19"));
    assert_eq!((r.status, r.body.len()), (206, 10));
    assert!(r.body == all[10..20]);

    // Без Range - первый кусок; конец за пределом куска и файла обрезается.
    let r = srv.get_range(&token, id, 1, None);
    assert_eq!((r.status, r.body.len()), (206, 8 * MIB));
    assert_eq!(r.header("content-range"), Some("bytes 0-8388607/20971520"));
    let r = srv.get_range(&token, id, 1, Some("bytes=20971000-99999999999"));
    assert_eq!((r.status, r.body.len()), (206, 520));
    assert_eq!(
        r.header("content-range"),
        Some("bytes 20971000-20971519/20971520")
    );

    // Не те единицы и обратный отрезок - 400.
    for bad in [
        "items=1-2",
        "bytes=5-2",
        "bytes=-5",
        "bytes=1-2,5-6",
        "bytes=a-",
    ] {
        let r = srv.get_range(&token, id, 1, Some(bad));
        assert_eq!(r.status, 400, "{bad}");
        assert!(
            String::from_utf8_lossy(&r.body).contains("поддерживается только Range"),
            "{bad}"
        );
    }
}

#[test]
fn range_past_the_end_is_416() {
    let srv = TestServer::start();
    let (id, token, _) = demo_job(&srv);
    for range in ["bytes=20971520-", "bytes=99999999999-"] {
        let r = srv.get_range(&token, id, 1, Some(range));
        assert_eq!(r.status, 416, "{range}");
        assert_eq!(r.header("content-range"), Some("bytes */20971520"));
        assert!(String::from_utf8_lossy(&r.body).contains("range_not_satisfiable"));
    }
    assert_eq!(
        srv.get_range(&token, id, 1, Some("bytes=20971519-")).status,
        206
    );
}

#[test]
fn demo_needs_the_current_epoch() {
    let srv = clocked();
    let clock = srv.clock();
    let id = srv.submit(&fake_demo(MIB), "s1mple");
    let token = srv.add_runner("a");
    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    assert_eq!(got.unwrap().epoch, 1);

    for epoch in [0, 2] {
        let r = srv.get_range(&token, id, epoch, None);
        assert_eq!(r.status, 409, "эпоха {epoch}");
        assert!(String::from_utf8_lossy(&r.body).contains("stale_epoch"));
        assert_eq!(srv.head_demo(&token, id, epoch).0, 409);
    }
    // Чужой раннер с верной эпохой тоже не скачивает.
    let other = srv.add_runner("b");
    assert_eq!(srv.get_range(&other, id, 1, None).status, 409);

    // Скачивание продлевает аренду и не двигает состояние.
    clock.advance(Duration::from_secs(20));
    let r = srv.get_range(&token, id, 1, Some("bytes=0-9"));
    assert_eq!(r.status, 206);
    let job = srv.job(id);
    assert_eq!(job.state, JobState::Assigned);
    assert_eq!(job.lease_until, Some(T0 + 20_000 + 45_000));
}

#[test]
fn expect_continue_upload_works_and_a_rejected_one_gets_no_continue() {
    let srv = TestServer::start();
    let (id, token) = running_job(&srv);
    let data = result_bytes(1024);

    // Верное смещение: сервер присылает 100 Continue, после тела - 204.
    let mut conn = raw_request(srv.addr());
    conn.write_all(
        raw_put_head(
            &token,
            id,
            1,
            0,
            &format!("Expect: 100-continue\r\nContent-Length: {}\r\n", data.len()),
        )
        .as_bytes(),
    )
    .unwrap();
    let first = read_status_line(&mut conn);
    assert!(first.starts_with("HTTP/1.1 100"), "{first}");
    // Заголовки промежуточного ответа читаются до пустой строки: тело клиент шлёт только после них.
    let _ = common::read_headers(&mut conn);
    conn.write_all(&data).unwrap();
    let reply = read_reply(&mut conn, false);
    assert_eq!(reply.status(), 204, "{}", reply.status_line);
    assert_eq!(reply.header("upload-offset"), Some("1024"));

    // Неверное смещение: первая же строка - 409, 100 Continue не было.
    let mut conn = raw_request(srv.addr());
    conn.write_all(
        raw_put_head(
            &token,
            id,
            1,
            5,
            &format!("Expect: 100-continue\r\nContent-Length: {}\r\n", data.len()),
        )
        .as_bytes(),
    )
    .unwrap();
    let first = read_status_line(&mut conn);
    assert!(first.contains(" 409 "), "отказ без 100 Continue: {first}");
    drop(conn);

    // Слишком большой кусок с Expect - тоже сразу отказ.
    let mut conn = raw_request(srv.addr());
    conn.write_all(
        raw_put_head(
            &token,
            id,
            1,
            1024,
            &format!(
                "Expect: 100-continue\r\nContent-Length: {}\r\n",
                CHUNK_SIZE + 1
            ),
        )
        .as_bytes(),
    )
    .unwrap();
    let first = read_status_line(&mut conn);
    assert!(first.contains(" 413 "), "{first}");
}

#[test]
fn eight_mebibyte_put_and_a_response_over_32_kib_keep_their_content_length() {
    // Поведение tiny_http из спайка D-01 в одном месте: PUT ровно 8 МиБ принимается, а ответ больше порога
    // chunked (32 КиБ) всё равно идёт с настоящим Content-Length.
    let srv = TestServer::start();
    let (id, token, _) = demo_job(&srv);
    let r = srv.get_range(&token, id, 1, Some("bytes=0-65535"));
    assert_eq!((r.status, r.body.len()), (206, 65536));
    assert_eq!(r.header("content-length"), Some("65536"));
    assert_eq!(r.header("transfer-encoding"), None);
    let progress = srv.progress(&token, id, &record(1, 1));
    assert_eq!(progress.0, 200);
    let data = result_bytes(8 * MIB);
    let (code, off, body) = srv.put_chunk(&token, id, 1, 0, &data);
    assert_eq!((code, off), (204, Some(8 * MIB as u64)), "{body}");
}
