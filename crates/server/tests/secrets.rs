//! SEC-04: токены не утекают из сервера. Подставные токены раннера и бота после принудительных сбоев не должны
//! находиться ни в журнале, ни в байтах базы, ни в ответах API.
//!
//! Правило против холостого теста (Pitfall 10): сначала доказывается, что подставной токен БЫЛ во входе
//! (в отправленных запросах), и только потом — что его нет на выходе. Сама проверка `assert_absent` падает
//! на буфере, где токен есть, и это тоже проверено отдельным тестом.

mod common;

use common::{TestServer, fake_demo};
use wire::{Failure, FinishReply, FinishRequest, JobState, Outcome, Phase, ProgressRequest, codes};

/// Паникует, если `needle` найден в `haystack`. Сообщение называет место, но не печатает соседний текст:
/// иначе провал теста сам выдал бы секрет в вывод `cargo test`.
fn assert_absent(haystack: &[u8], needle: &str, place: &str) {
    assert!(!needle.is_empty(), "пустая игла проверяла бы всё подряд");
    let found = haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes());
    assert!(!found, "подставной секрет найден: {place}");
}

fn progress(epoch: i64, seq: u64, phase: Phase) -> ProgressRequest {
    ProgressRequest {
        epoch,
        seq,
        phase,
        take: None,
        status: String::new(),
    }
}

#[test]
fn runner_token_in_a_failure_report_stays_out_of_logs_db_and_responses() {
    let mut srv = TestServer::start();
    let token = srv.add_runner("alpha");
    let id = srv.submit(&fake_demo(4096), "s1mple");

    let (status, got) = srv.claim(&token, 0);
    assert_eq!(status, 200);
    let epoch = got.expect("выдача").epoch;
    let (st, body) = srv.progress(&token, id, &progress(epoch, 1, Phase::Record));
    assert_eq!(st, 200, "{body}");

    let failed = FinishRequest {
        epoch,
        outcome: Outcome::Failed(Failure {
            code: "HLAE_ERROR".to_owned(),
            class: "окружение".to_owned(),
            state: "take".to_owned(),
            hint: "проверьте HLAE".to_owned(),
            message: format!("boom {token}"),
            tail: format!("строка\nAuthorization: Bearer {token}"),
            elapsed_ms: 1234,
        }),
    };
    let sent = serde_json::to_string(&failed).unwrap();
    // Против холостого теста: токен действительно ушёл серверу в теле запроса.
    assert!(sent.contains(&token), "токен должен быть во входе");

    let mut responses = String::new();
    let (st, body) = srv.finish(&token, id, &failed);
    assert_eq!(st, 200, "{body}");
    responses.push_str(&body);
    let reply: FinishReply = serde_json::from_str(&body).unwrap();
    assert_eq!(reply.state, JobState::Failed);

    let job = srv.job(id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.fail_code.as_deref(), Some("HLAE_ERROR"));
    assert_eq!(job.fail_state.as_deref(), Some("take"));
    let message = job.fail_message.clone().unwrap();
    assert!(
        message.contains("boom") && message.contains("cs2r_***"),
        "{message}"
    );
    assert_eq!(job.fail_elapsed_ms, Some(1234));
    assert!(job.finished_at.is_some());
    assert!(job.lease_until.is_none());

    // Повтор того же итога идемпотентен, другой итог после конца — отказ.
    let (st, body) = srv.finish(&token, id, &failed);
    assert_eq!(st, 200, "{body}");
    responses.push_str(&body);
    let cancelled = FinishRequest {
        epoch,
        outcome: Outcome::Cancelled,
    };
    let (st, body) = srv.finish(&token, id, &cancelled);
    assert_eq!(st, 409, "{body}");
    assert!(body.contains(codes::JOB_TERMINAL), "{body}");
    responses.push_str(&body);

    assert_absent(responses.as_bytes(), &token, "тела ответов API");
    assert_absent(srv.sink_text().as_bytes(), &token, "журнал сервера");
    assert_absent(&srv.db_bytes(), &token, "байты server.db и -wal");
    srv.stop();
    assert_absent(&srv.db_bytes(), &token, "байты server.db после остановки");
}
