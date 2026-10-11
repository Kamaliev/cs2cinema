//! Outbox раннера на настоящем сервере: обрыв загрузки, перезапуск после записи и посреди неё, окончательные
//! отказы сервера. «Убитый» раннер изображается прямой записью `runner_state.json` после настоящего `claim`.

mod common;

use std::sync::atomic::Ordering;

use common::{Scenario, ScriptedExecutor, Stand, fake_demo, result_bytes, sha256_hex};
use runner::{HeldJob, Link, ResultFile, Runner, RunnerState, Stage, Tick, Token};
use wire::{Assignment, JobState};

const MIB: usize = 1024 * 1024;

/// Берёт задачу у сервера «руками», как это сделал бы умерший раннер.
fn claim_by_hand(stand: &Stand) -> Assignment {
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    link.claim(0).unwrap().expect("задача должна быть выдана")
}

/// Состояние «исполнитель отдал результат, загрузки ещё не было»: файл результата лежит в папке раннера.
fn finished_state(stand: &Stand, a: &Assignment, bytes: &[u8]) -> HeldJob {
    let dir = stand.runner_dir();
    let out_dir = dir.join("out").join(format!("srv{}", a.job_id));
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::create_dir_all(dir.join("remote")).unwrap();
    let path = out_dir.join("highlights.mp4");
    std::fs::write(&path, bytes).unwrap();
    let mut held = HeldJob::claimed(a, dir.join("remote").join(format!("srv{}.dem", a.job_id)));
    held.stage = Stage::Finished;
    held.attempt = 1;
    held.out_dir = Some(out_dir);
    held.result = Some(ResultFile {
        path,
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    });
    held
}

fn write_state(stand: &Stand, job: Option<HeldJob>) {
    std::fs::create_dir_all(stand.runner_dir()).unwrap();
    RunnerState {
        job,
        ..Default::default()
    }
    .save(&stand.runner_dir())
    .unwrap();
}

fn state_of(stand: &Stand) -> RunnerState {
    let (state, warning) = RunnerState::load(&stand.runner_dir());
    assert!(warning.is_none(), "{warning:?}");
    state
}

fn runner_log(stand: &Stand) -> String {
    std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap_or_default()
}

#[test]
fn interrupted_upload_resumes_after_restart_from_the_server_offset() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(20 * MIB)]);

    // Первый запуск останавливается после первого куска.
    let mut cfg = stand.runner_config();
    cfg.upload_chunk_limit = Some(1);
    let mut first = Runner::new(cfg, exec.clone()).unwrap();
    assert_eq!(first.tick().unwrap(), Tick::Stopped);
    drop(first);

    let job = state_of(&stand)
        .job
        .expect("задача должна остаться в outbox");
    assert_eq!(job.stage, Stage::Finished);
    assert_eq!(job.server_job_id, id);
    let result = job
        .result
        .clone()
        .expect("в состоянии есть запись о результате");
    assert!(result.path.ends_with("highlights.mp4"), "{:?}", result.path);
    assert_eq!(result.size, (20 * MIB) as u64);
    assert_eq!(result.sha256, sha256_hex(&result_bytes(20 * MIB)));

    // Сервер принял ровно один кусок.
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    assert_eq!(link.result_head(id, job.epoch).unwrap(), (8 * MIB) as u64);
    assert_eq!(stand.job(id).state, JobState::Uploading);

    // Второй запуск в той же папке без лимита докачивает и закрывает задачу.
    let mut second = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    assert_eq!(
        second.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(id).state, JobState::Done);
    let stored = std::fs::read(stand.results_dir(id).join("highlights.mp4")).unwrap();
    assert!(
        stored == result_bytes(20 * MIB),
        "результат на сервере отличается"
    );
    assert!(state_of(&stand).job.is_none(), "outbox должен очиститься");
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        1,
        "исполнитель вызван заново"
    );
}

#[test]
fn runner_killed_after_recording_uploads_on_restart_without_the_executor() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, id);

    let bytes = result_bytes(9 * MIB);
    write_state(&stand, Some(finished_state(&stand, &a, &bytes)));

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(id).state, JobState::Done);
    let stored = std::fs::read(stand.results_dir(id).join("highlights.mp4")).unwrap();
    assert!(stored == bytes, "результат на сервере отличается");
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        0,
        "исполнитель не должен вызываться"
    );
    assert!(state_of(&stand).job.is_none());
    assert!(runner_log(&stand).contains("сервер подтвердил выдачу"));
}

/// Демка задачи на месте раннера: та же, что отдал сервер.
fn put_demo(stand: &Stand, id: i64, demo: &[u8]) -> std::path::PathBuf {
    let path = stand
        .runner_dir()
        .join("remote")
        .join(format!("srv{id}.dem"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, demo).unwrap();
    path
}

#[test]
fn runner_killed_while_recording_resumes_the_executor_with_resume_flag() {
    let stand = Stand::start();
    let demo = fake_demo(MIB);
    let id = stand.submit(&demo, "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, id);

    // «Убит посреди записи»: демка скачана, исполнитель уже начал и знает свою папку.
    let demo_path = put_demo(&stand, id, &demo);
    let out_dir = stand.runner_dir().join("out").join(format!("srv{id}"));
    let mut held = HeldJob::claimed(&a, demo_path);
    held.stage = Stage::Executing;
    held.attempt = 1;
    held.out_dir = Some(out_dir.clone());
    write_state(&stand, Some(held));

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
    let task = exec.last_task();
    assert!(task.resume, "исполнитель должен идти с resume");
    assert_eq!(task.out_dir, Some(out_dir));
    assert_eq!(task.attempt, 2, "попытка должна вырасти на 1");
    assert_eq!(stand.job(id).state, JobState::Done);
}

#[test]
fn second_sha256_mismatch_drops_the_result_from_the_outbox() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);

    let mut held = finished_state(&stand, &a, &result_bytes(MIB));
    let result_path = held.result.as_ref().unwrap().path.clone();
    held.result.as_mut().unwrap().sha256 = "0".repeat(64);
    // Первый отказ сервера уже был (раннер перезапустили): перед первой загрузкой раннер сверяет файл сам и закрыл бы
    // задачу `RESULT_INVALID`, а второй отказ сервера обязан убрать результат из outbox.
    held.sha_retries = 1;
    write_state(&stand, Some(held));

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Dropped { job_id: id });

    assert!(!result_path.exists(), "файл результата должен быть удалён");
    assert!(state_of(&stand).job.is_none(), "outbox должен очиститься");
    assert_ne!(stand.job(id).state, JobState::Done);
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("sha256") && l.contains(&format!("#{id}"))),
        "в журнале нет причины:\n{log}"
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn job_cancelled_on_the_server_drops_the_result_from_the_outbox() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    let held = finished_state(&stand, &a, &result_bytes(MIB));
    let result_path = held.result.as_ref().unwrap().path.clone();
    write_state(&stand, Some(held));
    stand.cancel(id, true);

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    let tick = runner.tick().unwrap();
    assert!(
        matches!(
            tick,
            Tick::Settled {
                state: JobState::Cancelled,
                ..
            } | Tick::Dropped { .. }
        ),
        "{tick:?}"
    );
    assert_eq!(stand.job(id).state, JobState::Cancelled);
    assert!(!result_path.exists(), "файл результата должен быть удалён");
    assert!(state_of(&stand).job.is_none());
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains(&format!("#{id}")) && l.contains("отмен")),
        "в журнале нет причины:\n{log}"
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn corrupted_state_file_is_set_aside_and_the_runner_starts_empty() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let garbage: Vec<u8> = [b"{ not json ".as_slice(), &[0xff, 0xfe]].concat();
    std::fs::write(dir.join("runner_state.json"), &garbage).unwrap();

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();

    assert!(!dir.join("runner_state.json").exists());
    let aside: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("runner_state.bad-") && n.ends_with(".json"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read(dir.join(&aside[0])).unwrap(), garbage);
    let log = runner_log(&stand);
    assert!(log.contains("runner_state.bad-"), "{log}");
    assert_eq!(runner.tick().unwrap(), Tick::Idle);
}

#[test]
fn no_claim_while_a_job_is_held() {
    let stand = Stand::start();
    let first = stand.submit(&fake_demo(MIB), "s1mple");
    let second = stand.submit(&fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, first);
    write_state(&stand, Some(finished_state(&stand, &a, &result_bytes(MIB))));

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: first,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(second).state, JobState::Queued);
    assert_eq!(exec.calls.load(Ordering::SeqCst), 0);

    // Когда outbox пуст, следующая задача берётся.
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: second,
            state: JobState::Done
        }
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
}

mod scripted_http {
    //! Сценарный HTTP-ответчик: отвечает по списку, по одному запросу на соединение.

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    pub struct Reply {
        pub status: u16,
        pub headers: Vec<(&'static str, String)>,
        pub body: String,
    }

    impl Reply {
        pub fn new(status: u16, headers: &[(&'static str, &str)], body: &str) -> Reply {
            Reply {
                status,
                headers: headers.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
                body: body.to_owned(),
            }
        }
    }

    /// Что пришло на ответчик.
    #[derive(Debug, Clone)]
    pub struct Seen {
        pub method: String,
        pub offset: Option<u64>,
        pub body_len: usize,
        pub at: Instant,
    }

    pub fn start(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let handle = std::thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let method = line.split_whitespace().next().unwrap_or("").to_owned();
                let (mut offset, mut length) = (None, 0usize);
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim().to_ascii_lowercase();
                    if h.is_empty() {
                        break;
                    }
                    if let Some(v) = h.strip_prefix("upload-offset:") {
                        offset = v.trim().parse().ok();
                    }
                    if let Some(v) = h.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(Seen {
                    method,
                    offset,
                    body_len: length,
                    at: Instant::now(),
                });
                let mut out = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (k, v) in &reply.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&reply.body);
                let mut stream = stream;
                stream.write_all(out.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });
        (base, seen, handle)
    }
}

#[test]
fn upload_follows_the_server_offset_and_waits_when_busy() {
    use scripted_http::{Reply, start};
    use std::sync::Mutex;
    use std::time::Duration;

    let json = ("Content-Type", "application/json");
    let (base, seen, handle) = start(vec![
        Reply::new(200, &[("Upload-Offset", "0")], ""),
        Reply::new(
            409,
            &[json],
            r#"{"error":"offset_mismatch","message":"сервер принял 40","offset":40}"#,
        ),
        Reply::new(
            409,
            &[json, ("Retry-After", "1")],
            r#"{"error":"upload_busy","message":"занято"}"#,
        ),
        Reply::new(204, &[("Upload-Offset", "100")], ""),
        Reply::new(200, &[json], r#"{"state":"done"}"#),
    ]);

    let dir = std::env::temp_dir().join(format!("cs2up-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("highlights.mp4");
    let bytes = result_bytes(100);
    std::fs::write(&file, &bytes).unwrap();

    let a = Assignment {
        job_id: 9,
        epoch: 5,
        lease_until_ms: 0,
        demo: wire::DemoInfo {
            size: 1,
            sha256: "0".repeat(64),
            orig_name: String::new(),
        },
        faceit_link: None,
        player: wire::Player {
            steam64: None,
            name: "s1mple".to_owned(),
        },
        requested_by: String::new(),
        options: Default::default(),
    };
    let mut held = HeldJob::claimed(&a, dir.join("srv9.dem"));
    held.stage = Stage::Finished;
    held.result = Some(ResultFile {
        path: file.clone(),
        size: 100,
        sha256: sha256_hex(&bytes),
    });

    let link = Link::new(&base, Token::new("secret-token")).unwrap();
    let lines = Mutex::new(Vec::<String>::new());
    let end =
        runner::transfer::upload_result(&link, &mut held, &mut |_| {}, &|| None, None, &|m| {
            lines.lock().unwrap().push(m.to_owned())
        });
    handle.join().unwrap();

    assert_eq!(
        end,
        runner::TransferEnd::Done(JobState::Done),
        "{:?}",
        lines.lock().unwrap()
    );
    let seen = seen.lock().unwrap().clone();
    let shape: Vec<(&str, Option<u64>)> =
        seen.iter().map(|s| (s.method.as_str(), s.offset)).collect();
    assert_eq!(
        shape,
        vec![
            ("HEAD", None),
            ("PUT", Some(0)),
            ("PUT", Some(40)),
            ("PUT", Some(40)),
            ("POST", None),
        ]
    );
    // Тела кусков: весь файл с нуля, затем только хвост с присланного смещения.
    let lens: Vec<usize> = seen[1..4].iter().map(|s| s.body_len).collect();
    assert_eq!(lens, vec![100, 60, 60]);
    assert!(
        seen[3].at.duration_since(seen[2].at) >= Duration::from_secs(1),
        "повтор после upload_busy раньше Retry-After: {:?}",
        seen[3].at.duration_since(seen[2].at)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
