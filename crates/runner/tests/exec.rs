//! Защищённый исполнитель раннера на `FakeHost` и настоящем `server::serve`: дочерняя предпроверка закрывает
//! мусорную демку до запуска CS2, паника `run_job` не роняет раннер, продолжение откатывается к записи с нуля.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Stand, TestDir, fake_demo};
use pipeline::host::fake::{FakeHost, Scenario};
use pipeline::host::{CmdOutput, ProcIdentity, ProcInfo};
use pipeline::inspect::InspectLimits;
use pipeline::job::{JobConfig, Outcome};
use pipeline::{FailCode, Progress, Quiet, RunError};
use runner::{
    Executor, HeldJob, InspectSettings, Link, PipelineExecutor, ResultFile, Runner, RunnerState,
    Stage, Task, Tick, Token, sha256_file,
};
use wire::JobState;

const MIB: usize = 1024 * 1024;

const OK_REPLY: &str = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"moments":3,"utility":1,"peak_mem_bytes":1048576}"#;
const BAD_DEMO_REPLY: &str = r#"{"ok":false,"code":"BAD_DEMO","message":"мусор"}"#;
const NOT_FOUND_REPLY: &str = r#"{"ok":false,"code":"PLAYER_NOT_FOUND","message":"нет","candidates":["ZywOo (76561198000000003)"]}"#;

fn exit0() -> Result<CmdOutput, pipeline::host::HostError> {
    Ok(CmdOutput {
        code: Some(0),
        stderr_tail: Vec::new(),
        elapsed: Duration::ZERO,
    })
}

/// Конфиг задачи с пустыми временными папками: до CS2 дело не доходит или он целиком в `FakeHost`.
fn healthy_config(dir: &TestDir) -> JobConfig {
    for f in [
        "hlae/HLAE.exe",
        "hlae/x64/AfxHookSource2.dll",
        "hlae/ffmpeg/bin/ffmpeg.exe",
        "cs2/game/bin/win64/cs2.exe",
    ] {
        let path = dir.path.join("tools").join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
    JobConfig {
        hlae_dir: dir.path.join("tools/hlae"),
        cs2_dir: dir.path.join("tools/cs2"),
        out_root: dir.path.join("tools/out"),
        ..JobConfig::default()
    }
}

fn settings() -> InspectSettings {
    InspectSettings {
        exe: PathBuf::from("cs2-cli"),
        limits: InspectLimits::default(),
    }
}

fn executor(host: &FakeHost, dir: &TestDir) -> PipelineExecutor {
    PipelineExecutor::new(Arc::new(host.clone()), healthy_config(dir), settings())
}

fn task(demo: &str) -> Task {
    Task {
        job_id: 7,
        epoch: 1,
        attempt: 1,
        demo: PathBuf::from(demo),
        faceit: None,
        hero_steam: 0,
        hero_name: "s1mple".to_owned(),
        resume: false,
        out_dir: None,
        options: Default::default(),
    }
}

fn inspect_runs(host: &FakeHost) -> usize {
    host.runs()
        .iter()
        .filter(|(_, program, args)| {
            program.contains("cs2-cli") && args.first().is_some_and(|a| a == "inspect")
        })
        .count()
}

fn fail_code(e: &RunError) -> FailCode {
    e.fail.as_ref().expect("сбой типизирован").code
}

#[test]
fn garbage_demo_fails_with_bad_demo_before_cs2_and_the_runner_takes_the_next_job() {
    let stand = Stand::start();
    let first = stand.submit(&fake_demo(MIB), "s1mple");
    let second = stand.submit(&fake_demo(MIB), "ZywOo");
    let dir = TestDir::new("exec-tracer");
    let host = FakeHost::new(
        Scenario::happy(1)
            .program_for(
                "cs2-cli",
                &format!("srv{first}."),
                &[BAD_DEMO_REPLY],
                exit0(),
            )
            .program_for(
                "cs2-cli",
                &format!("srv{second}."),
                &[NOT_FOUND_REPLY],
                exit0(),
            ),
    );
    let exec = Arc::new(executor(&host, &dir));
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: first,
            state: JobState::Failed
        }
    );
    // Раннер не на паузе: контентный отказ берёт следующую задачу.
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: second,
            state: JobState::Failed
        }
    );

    assert_eq!(stand.job(first).fail_code.as_deref(), Some("BAD_DEMO"));
    assert_eq!(
        stand.job(second).fail_code.as_deref(),
        Some("PLAYER_NOT_FOUND")
    );
    assert_eq!(host.launches(), 0, "CS2 не должен был запускаться");
    assert_eq!(inspect_runs(&host), 2, "{:?}", host.runs());
}

#[test]
fn environment_failure_after_a_good_preflight_returns_the_job_to_the_queue() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let dir = TestDir::new("exec-env");
    let proc = |pid, exe: &str| ProcInfo {
        pid,
        exe_name: exe.to_owned(),
    };
    let host = FakeHost::new(
        Scenario::happy(1)
            .processes(vec![proc(100, "steam.exe"), proc(777, "cs2.exe")])
            .program("cs2-cli", &[OK_REPLY], exit0()),
    );
    let exec = Arc::new(executor(&host, &dir));
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();

    assert_eq!(runner.tick().unwrap(), Tick::Released { job_id: id });

    let job = stand.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.grants, 1, "release засчитывает выдачу (D-02)");
    assert_eq!(job.fail_code, None);
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    println!("журнал раннера:\n{log}");
    assert!(
        log.contains("демка проверена: игрок s1mple, моментов 3"),
        "{log}"
    );
    assert_eq!(host.launches(), 0, "чужой CS2 — игра не запускалась");
}

const FACEIT_LINK: &str = "https://www.faceit.com/ru/cs2/room/1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
const FACEIT_ID: &str = "1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";

/// Исполнитель с ключом FACEIT раннера и подменённым скачиванием: вызовы `(match_id, ключ)` копятся в `calls`.
fn faceit_executor(
    host: &FakeHost,
    dir: &TestDir,
    key: &str,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    fetch: impl Fn(&Path) -> Result<PathBuf, String> + Send + Sync + 'static,
) -> PipelineExecutor {
    let cfg = JobConfig {
        faceit_key: key.to_owned(),
        ..healthy_config(dir)
    };
    PipelineExecutor::new(Arc::new(host.clone()), cfg, settings()).with_fetch_fn(
        move |id, key, folder| {
            calls.lock().unwrap().push((id.to_owned(), key.to_owned()));
            fetch(folder)
        },
    )
}

/// Задача FACEIT: демка должна оказаться в `<dir>/remote/srv7.dem`.
fn faceit_task(dir: &TestDir, link: &str) -> Task {
    let mut t = task(dir.path.join("remote/srv7.dem").to_str().unwrap());
    t.faceit = Some(link.to_owned());
    t
}

#[test]
fn faceit_link_is_downloaded_by_match_id_with_the_runner_key() {
    let dir = TestDir::new("exec-faceit");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen_source = Arc::new(Mutex::new(None));
    let source = seen_source.clone();
    let exec = faceit_executor(&host, &dir, "k-1", calls.clone(), |folder| {
        std::fs::create_dir_all(folder).unwrap();
        let path = folder.join("1-abc.dem.zst");
        std::fs::write(&path, fake_demo(4096)).unwrap();
        Ok(path)
    })
    .with_job_fn(move |_, spec, _| {
        *source.lock().unwrap() = Some(format!("{:?}", spec.source));
        Err(RunError::from("стоп после проверки источника"))
    });
    let t = faceit_task(&dir, FACEIT_LINK);

    let e = exec.run(&t, &Quiet).expect_err("job_fn остановил задачу");
    assert_eq!(e.message, "стоп после проверки источника");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![(FACEIT_ID.to_owned(), "k-1".to_owned())],
        "скачивание по id матча ключом раннера, один раз"
    );
    assert_eq!(inspect_runs(&host), 1);
    let runs = host.runs();
    let inspect = runs
        .iter()
        .find(|(_, program, _)| program.contains("cs2-cli"))
        .expect("разбор звали");
    assert!(
        inspect.2.iter().any(|a| a.contains("srv7.dem")),
        "разбор идёт по скачанному файлу: {:?}",
        inspect.2
    );
    let source = seen_source.lock().unwrap().clone().unwrap();
    assert!(
        source.starts_with("File(") && source.contains("srv7.dem"),
        "run_job получил файл: {source}"
    );
    assert!(t.demo.is_file(), "демка лежит на месте задачи");
    assert!(
        !t.demo.with_extension("faceit").exists(),
        "временная папка убрана"
    );
}

#[test]
fn link_that_fails_the_strict_check_is_unsupported_source() {
    let dir = TestDir::new("exec-faceit-strict");
    let host = FakeHost::new(Scenario::happy(1));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let exec = faceit_executor(&host, &dir, "k-1", calls.clone(), |_| {
        panic!("скачивание не должно вызываться")
    });
    let t = faceit_task(&dir, "https://evil.com/x");

    let e = exec.run(&t, &Quiet).expect_err("чужая ссылка не берётся");
    assert_eq!(fail_code(&e), FailCode::UnsupportedSource);
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(inspect_runs(&host), 0, "ребёнка не звали");
}

#[test]
fn missing_key_is_faceit_fetch_failed() {
    let dir = TestDir::new("exec-faceit-nokey");
    let host = FakeHost::new(Scenario::happy(1));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let exec = faceit_executor(&host, &dir, "  ", calls.clone(), |_| {
        panic!("скачивание не должно вызываться")
    });
    let t = faceit_task(&dir, FACEIT_LINK);

    let e = exec.run(&t, &Quiet).expect_err("без ключа не скачать");
    assert_eq!(fail_code(&e), FailCode::FaceitFetchFailed);
    assert!(e.message.contains("FACEIT_API_KEY"), "{}", e.message);
    assert!(e.message.contains("настройках"), "{}", e.message);
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(inspect_runs(&host), 0);
}

#[test]
fn fetch_failure_is_faceit_fetch_failed_without_urls() {
    let cases = [
        (
            "у матча нет демки (ещё не доиграли или демка удалена)",
            "у матча FACEIT нет демки",
        ),
        (
            "FACEIT отказал в доступе (ссылка на скачивание (нужен ключ с доступом к Downloads API)): проверьте FACEIT_API_KEY",
            "FACEIT отказал в доступе к демке",
        ),
        (
            "io: connection reset https://cdn.example/demo.dem.zst?token=abc k-1",
            "не удалось скачать демку FACEIT",
        ),
    ];
    for (raw, phrase) in cases {
        let dir = TestDir::new("exec-faceit-fail");
        let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let raw_owned = raw.to_owned();
        let exec = faceit_executor(&host, &dir, "k-1", calls.clone(), move |folder| {
            // Сбой посреди скачивания: недокачанный файл остаётся во временной папке.
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("half.part"), b"x").unwrap();
            Err(raw_owned.clone())
        });
        let t = faceit_task(&dir, FACEIT_LINK);

        let e = exec.run(&t, &Quiet).expect_err("скачивание упало");
        assert_eq!(fail_code(&e), FailCode::FaceitFetchFailed, "{raw}");
        assert!(e.message.contains(phrase), "{raw}: {}", e.message);
        for secret in ["https://", "token=", "k-1", "cdn.example"] {
            assert!(!e.message.contains(secret), "{secret} в {}", e.message);
        }
        assert!(
            !t.demo.with_extension("faceit").exists(),
            "временная папка убрана"
        );
        assert!(!t.demo.exists());
        assert_eq!(inspect_runs(&host), 0, "разбор не вызывался");
    }
}

#[test]
fn panic_inside_run_job_becomes_a_plain_error_and_does_not_kill_the_process() {
    let dir = TestDir::new("exec-panic");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
    let exec = executor(&host, &dir).with_job_fn(|_, _, _| panic!("бум"));

    let e = exec
        .run(&task("match.dem"), &Quiet)
        .expect_err("паника — ошибка");
    assert!(e.fail.is_none(), "раннер закроет её JOB_ERROR: {e:?}");
    assert!(
        e.message.contains("исполнитель задачи упал: бум"),
        "{}",
        e.message
    );
}

#[test]
fn resume_skips_the_preflight_and_falls_back_to_a_fresh_start_on_retry_no_data() {
    let dir = TestDir::new("exec-resume");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
    let seen: Arc<Mutex<Vec<(bool, usize)>>> = Arc::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let exec = {
        let (seen, calls, probe) = (seen.clone(), calls.clone(), host.clone());
        executor(&host, &dir).with_job_fn(move |_, spec, _| {
            seen.lock()
                .unwrap()
                .push((spec.resume, inspect_runs(&probe)));
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(pipeline::job::retry_no_data("манифест не найден"))
            } else {
                Err(RunError::from("стоп"))
            }
        })
    };
    let mut t = task("match.dem");
    t.resume = true;
    t.out_dir = Some(dir.path.join("old"));

    let e = exec
        .run(&t, &Quiet)
        .expect_err("второй вызов вернул «стоп»");
    assert_eq!(e.message, "стоп");
    // Первый вызов: продолжение без предпроверки; второй: с нуля и после повторной предпроверки.
    assert_eq!(*seen.lock().unwrap(), vec![(true, 0), (false, 1)]);
}

#[test]
fn log_lines_of_the_executor_are_reported_to_progress() {
    struct Lines(Mutex<Vec<String>>);
    impl Progress for Lines {
        fn stage(&self, _: pipeline::Stage) {}
        fn log(&self, msg: &str) {
            self.0.lock().unwrap().push(msg.to_owned());
        }
    }
    let dir = TestDir::new("exec-lines");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
    let exec = executor(&host, &dir).with_job_fn(|_, _, _| Err(RunError::from("стоп")));
    let lines = Lines(Mutex::default());

    let _ = exec.run(&task("match.dem"), &lines);
    let text = lines.0.lock().unwrap().join("\n");
    assert!(text.contains("проверяю демку"), "{text}");
    assert!(
        text.contains("пик памяти разбора 1 МиБ"),
        "пик нужен для калибровки лимитов: {text}"
    );
}

/// Ответ ffprobe для годного ролика: H.264, yuv420p, звук, mp4, длительность.
const GOOD_PROBE: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"5.000000"}}"#;

/// Бокс MP4 верхнего уровня: размер, тип, полезная нагрузка.
fn mp4_box(kind: &[u8; 4], payload: usize) -> Vec<u8> {
    let mut b = ((8 + payload) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend(std::iter::repeat_n(0xAB, payload));
    b
}

/// Файл MP4 из боксов в заданном порядке.
fn mp4(order: &[&[u8; 4]]) -> Vec<u8> {
    order.iter().flat_map(|k| mp4_box(k, 64)).collect()
}

/// Исполнитель, который «записал» ролик с заданными боксами в `out/srv{id}/highlights.mp4`.
fn exec_writing(
    host: &FakeHost,
    dir: &TestDir,
    runner_dir: PathBuf,
    bytes: Vec<u8>,
) -> PipelineExecutor {
    executor(host, dir).with_job_fn(move |_, spec, _| {
        let out_dir = runner_dir.join("out").join(format!("srv{}", spec.job_id));
        std::fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        std::fs::write(&result, &bytes).unwrap();
        Ok(Outcome {
            out_dir,
            result,
            shots: 1,
            secs: 5.0,
        })
    })
}

fn probing_host() -> FakeHost {
    FakeHost::new(
        Scenario::happy(1)
            .program("cs2-cli", &[OK_REPLY], exit0())
            .program("ffprobe", &[GOOD_PROBE], exit0()),
    )
}

/// Папка принятых результатов задачи на сервере пуста или не создана.
fn nothing_uploaded(stand: &Stand, id: i64) -> bool {
    std::fs::read_dir(stand.results_dir(id)).map_or(true, |mut d| d.next().is_none())
}

#[test]
fn invalid_result_is_not_uploaded_and_fails_with_result_invalid() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let dir = TestDir::new("exec-invalid");
    let host = probing_host();
    // moov после mdat: ролик не начнёт играть в Telegram до полной загрузки.
    let exec = Arc::new(exec_writing(
        &host,
        &dir,
        stand.runner_dir(),
        mp4(&[b"ftyp", b"mdat", b"moov"]),
    ));
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Failed
        }
    );
    let job = stand.job(id);
    assert_eq!(job.fail_code.as_deref(), Some("RESULT_INVALID"));
    assert!(
        nothing_uploaded(&stand, id),
        "негодный ролик не должен быть загружен"
    );
    let kept = stand
        .runner_dir()
        .join("out")
        .join(format!("srv{id}"))
        .join("highlights.mp4");
    assert!(kept.is_file(), "файл остаётся у раннера уликой");
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    assert!(log.contains("faststart"), "{log}");
}

#[test]
fn valid_result_is_uploaded_and_the_job_is_done() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let dir = TestDir::new("exec-valid");
    let host = probing_host();
    let exec = Arc::new(exec_writing(
        &host,
        &dir,
        stand.runner_dir(),
        mp4(&[b"ftyp", b"moov", b"mdat"]),
    ));
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    assert_eq!(stand.job(id).state, JobState::Done);
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    assert!(log.contains("ролик проверен: H.264, 1280x720"), "{log}");
}

#[test]
fn demo_over_the_limit_is_refused_before_download() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let dir = TestDir::new("exec-big");
    let host = probing_host();
    let exec = Arc::new(executor(&host, &dir));
    let mut cfg = stand.runner_config();
    cfg.max_demo_bytes = (MIB / 2) as u64;
    let mut runner = Runner::new(cfg, exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Failed
        }
    );
    assert_eq!(stand.job(id).fail_code.as_deref(), Some("DEMO_TOO_LARGE"));
    let remote = stand.runner_dir().join("remote");
    assert!(!remote.join(format!("srv{id}.dem")).exists());
    assert!(!remote.join(format!("srv{id}.dem.part")).exists());
    assert!(host.runs().is_empty(), "{:?}", host.runs());
}

#[test]
fn result_changed_after_verification_is_not_uploaded() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");

    // «Убитый» раннер: задача выдана по-настоящему, ролик проверен и записан в состояние, но уйти не успел.
    let link = Link::new(&stand.base, Token::new(stand.token.clone())).unwrap();
    let a = link.claim(0).unwrap().expect("задача должна быть выдана");
    assert_eq!(a.job_id, id);
    let remote = stand.runner_dir().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let out_dir = stand.runner_dir().join("out").join(format!("srv{id}"));
    std::fs::create_dir_all(&out_dir).unwrap();
    let result = out_dir.join("highlights.mp4");
    std::fs::write(&result, mp4(&[b"ftyp", b"moov", b"mdat"])).unwrap();
    let (size, sha256) = sha256_file(&result).unwrap();
    let mut held = HeldJob::claimed(&a, remote.join(format!("srv{id}.dem")));
    held.stage = Stage::Finished;
    held.attempt = 1;
    held.out_dir = Some(out_dir);
    held.result = Some(ResultFile {
        path: result.clone(),
        size,
        sha256,
    });
    RunnerState {
        job: Some(held),
        ..Default::default()
    }
    .save(&stand.runner_dir())
    .unwrap();
    // Файл изменили уже после проверки, размер тот же.
    std::fs::write(&result, mp4(&[b"ftyp", b"mdat", b"moov"])).unwrap();

    let host = probing_host();
    let dir = TestDir::new("exec-changed");
    let mut runner = Runner::new(stand.runner_config(), Arc::new(executor(&host, &dir))).unwrap();
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Failed
        }
    );
    let job = stand.job(id);
    assert_eq!(job.fail_code.as_deref(), Some("RESULT_INVALID"));
    let log = std::fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap();
    assert!(
        log.contains("файл результата изменился после проверки"),
        "{log}"
    );
    assert!(result.is_file(), "улика остаётся у раннера");
    assert!(
        nothing_uploaded(&stand, id),
        "изменённый ролик не должен быть загружен"
    );
}

#[test]
fn recover_kills_own_orphans_before_cleaning_the_game_dir() {
    let dir = TestDir::new("exec-recover");
    let host = FakeHost::new(Scenario::happy(1));
    let exec = executor(&host, &dir);

    // Пустая папка: ни процессов, ни файлов — пустой журнал, без паники.
    let empty = dir.path.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(exec.recover(&empty).is_empty());
    assert!(host.orphan_kills().is_empty());

    // Папка задачи с `processes.json` своей игры: процесс завершается по полной идентичности.
    let job = dir.path.join("job");
    std::fs::create_dir_all(&job).unwrap();
    let game = ProcIdentity {
        pid: 4242,
        created: 1234,
        image: "cs2.exe".to_owned(),
    };
    pipeline::record::processes::save(&job, std::slice::from_ref(&game)).unwrap();
    let notes = exec.recover(&job);
    assert_eq!(host.orphan_kills(), vec![game]);
    assert!(
        notes.iter().any(|n| n.contains("4242")),
        "строка для журнала: {notes:?}"
    );
}

#[test]
fn run_returns_an_error_when_the_server_rejects_the_token() {
    let stand = Stand::start();
    let dir = TestDir::new("exec-token");
    let host = probing_host();
    let mut cfg = stand.runner_config();
    cfg.token = Token::new("wrong-token");
    let mut runner = Runner::new(cfg, Arc::new(executor(&host, &dir))).unwrap();

    let e = runner
        .run()
        .expect_err("раннер с чужим токеном должен остановиться");
    assert!(e.contains("токен"), "{e}");
}

const MIRAGE_REPLY: &str = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"map":"de_mirage","moments":3,"utility":1,"peak_mem_bytes":1048576}"#;

/// `Progress`, который складывает по порядку артефакты `Match` и строки журнала.
struct Recorder(Mutex<Vec<String>>);

impl Progress for Recorder {
    fn stage(&self, _: pipeline::Stage) {}
    fn log(&self, msg: &str) {
        self.0.lock().unwrap().push(format!("лог: {msg}"));
    }
    fn artifact(&self, a: &pipeline::Artifact) {
        if let pipeline::Artifact::Match { map, player } = a {
            self.0
                .lock()
                .unwrap()
                .push(format!("карта {map}, игрок {player}"));
        }
    }
}

#[test]
fn inspected_demo_reports_its_map_and_player() {
    let dir = TestDir::new("exec-map");
    let host = FakeHost::new(
        Scenario::happy(1)
            .program("cs2-cli", &[MIRAGE_REPLY], exit0())
            .program("ffprobe", &[GOOD_PROBE], exit0()),
    );
    let exec = executor(&host, &dir).with_job_fn({
        let out = dir.path.join("out").join("srv7");
        move |_, _, p| {
            p.log("job_fn");
            std::fs::create_dir_all(&out).unwrap();
            let result = out.join("highlights.mp4");
            std::fs::write(&result, mp4(&[b"ftyp", b"moov", b"mdat"])).unwrap();
            Ok(Outcome {
                out_dir: out.clone(),
                result,
                shots: 1,
                secs: 5.0,
            })
        }
    });
    let rec = Recorder(Mutex::default());

    exec.run(&task("match.dem"), &rec).expect("ролик записан");

    let seen = rec.0.lock().unwrap().clone();
    let match_at = seen
        .iter()
        .position(|l| l == "карта de_mirage, игрок s1mple")
        .unwrap_or_else(|| panic!("нет Artifact::Match: {seen:?}"));
    let job_at = seen
        .iter()
        .position(|l| l == "лог: job_fn")
        .expect("job_fn вызван");
    assert!(match_at < job_at, "карта сообщается до записи: {seen:?}");
    assert!(
        seen.iter()
            .any(|l| l.starts_with("лог: демка проверена: карта de_mirage, игрок s1mple")),
        "{seen:?}"
    );
}

#[test]
fn resumed_task_has_no_preflight_and_reports_no_map() {
    let dir = TestDir::new("exec-map-resume");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[MIRAGE_REPLY], exit0()));
    let exec = executor(&host, &dir).with_job_fn(|_, _, _| Err(RunError::from("стоп")));
    let rec = Recorder(Mutex::default());
    let mut t = task("match.dem");
    t.resume = true;
    t.out_dir = Some(dir.path.join("old"));

    let _ = exec.run(&t, &rec);

    assert_eq!(inspect_runs(&host), 0, "при продолжении предпроверки нет");
    let seen = rec.0.lock().unwrap().clone();
    assert!(!seen.iter().any(|l| l.starts_with("карта ")), "{seen:?}");
}

#[test]
fn user_options_override_montage_settings_but_keep_machine_ones() {
    let dir = TestDir::new("exec-options");
    let host = FakeHost::new(Scenario::happy(1).program("cs2-cli", &[OK_REPLY], exit0()));
    let seen = Arc::new(Mutex::new(None));
    let sink = seen.clone();
    let exec = executor(&host, &dir).with_job_fn(move |_, spec, _| {
        *sink.lock().unwrap() = Some(spec.cfg.clone());
        Err("стоп".into())
    });
    let mut t = task(dir.path.join("remote/srv7.dem").to_str().unwrap());
    t.options = serde_json::json!({"detailed": true, "flybys": false, "fps": 30, "resolution": 1080})
        .as_object()
        .cloned()
        .unwrap();
    let _ = exec.run(&t, &Quiet);
    let cfg = seen.lock().unwrap().clone().expect("run_job был вызван");
    assert!(cfg.detailed && !cfg.flybys && cfg.walls);
    assert_eq!((cfg.fps, cfg.frame), (30, (1920, 1080)));
    assert_eq!(cfg.hlae_dir, healthy_config(&dir).hlae_dir, "пути раннера остались его");
}
