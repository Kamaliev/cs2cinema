//! Защищённый исполнитель раннера на `FakeHost` и настоящем `server::serve`: дочерняя предпроверка закрывает
//! мусорную демку до запуска CS2, паника `run_job` не роняет раннер, продолжение откатывается к записи с нуля.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Stand, TestDir, fake_demo};
use pipeline::host::fake::{FakeHost, Scenario};
use pipeline::host::{CmdOutput, ProcInfo};
use pipeline::inspect::InspectLimits;
use pipeline::job::JobConfig;
use pipeline::{FailCode, Progress, Quiet, RunError};
use runner::{Executor, InspectSettings, PipelineExecutor, Runner, Task, Tick};
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
        attempt: 1,
        demo: PathBuf::from(demo),
        faceit: None,
        hero_steam: 0,
        hero_name: "s1mple".to_owned(),
        resume: false,
        out_dir: None,
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

#[test]
fn faceit_link_without_a_demo_file_fails_with_unsupported_source() {
    let dir = TestDir::new("exec-faceit");
    let host = FakeHost::new(Scenario::happy(1));
    let exec = executor(&host, &dir);
    let mut t = task("");
    t.faceit = Some("https://www.faceit.com/ru/cs2/room/1-abc".to_owned());

    let e = exec
        .run(&t, &Quiet)
        .expect_err("ссылка без файла не берётся");
    assert_eq!(fail_code(&e), FailCode::UnsupportedSource);
    assert_eq!(inspect_runs(&host), 0, "ребёнка не звали");
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
