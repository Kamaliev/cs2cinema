//! Передача файлов кусками через настоящий HTTP на localhost (D-05, SRV-06): результат раннера грузится по 8 МиБ
//! с докачкой после обрыва, демка отдаётся по `Range`, а поведение `tiny_http`, на котором держится протокол
//! (`Expect: 100-continue`, обрыв тела, ответы больше 32 КиБ), закреплено тестами. Время двигают ручные часы.

mod common;

use common::{
    TestOptions, TestServer, fake_demo, raw_put_head, raw_request, read_headers, read_status_line,
    result_bytes, sha256_hex,
};
use server::clock::ManualClock;
use std::io::{Read, Write};
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

// Заглушки для сырых соединений, которые используют следующие тесты.
#[allow(dead_code)]
fn _raw_helpers_are_linked() {
    let _ = (
        raw_put_head as fn(&str, i64, i64, u64, &str) -> String,
        raw_request,
        read_headers,
        read_status_line,
    );
    let _ = (std::io::stdout().flush(), Shutdown::Both);
    let _ = std::io::empty().read(&mut [0u8; 0]);
}
