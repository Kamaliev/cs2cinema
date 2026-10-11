//! «Повторить» и склейка на фейковом хосте: без CS2, без базы и без настроек приложения.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use pipeline::assemble::assemble_stage;
use pipeline::host::fake::{FakeHost, Scenario};
use pipeline::host::{CmdOutput, ProcInfo};
use pipeline::job::{self, DemoSource, JobConfig, JobSpec};
use pipeline::manifest::{EnvSnapshot, Manifest, RecordJson, TakeSpec, TakeStatus};
use pipeline::record::verify::TakeFacts;
use pipeline::{Artifact, FailCode, Progress, RecordState, Stage};
use renderer::TakePlan;

/// `Progress`, который копит строки журнала и артефакты.
#[derive(Default)]
struct Capture {
    lines: Mutex<Vec<String>>,
    artifacts: Mutex<Vec<Artifact>>,
}

impl Capture {
    fn log_text(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

impl Progress for Capture {
    fn stage(&self, _: Stage) {}
    fn log(&self, msg: &str) {
        self.lines.lock().unwrap().push(msg.to_owned());
    }
    fn artifact(&self, a: &Artifact) {
        self.artifacts.lock().unwrap().push(a.clone());
    }
}

/// Конфиг с временным деревом HLAE и CS2, где есть всё нужное предпроверке.
fn healthy_config(tag: &str) -> JobConfig {
    let root = std::env::temp_dir().join(format!("cs2cinema-job-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for f in [
        "hlae/HLAE.exe",
        "hlae/x64/AfxHookSource2.dll",
        "hlae/ffmpeg/bin/ffmpeg.exe",
        "cs2/game/bin/win64/cs2.exe",
    ] {
        let path = root.join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
    JobConfig {
        hlae_dir: root.join("hlae"),
        cs2_dir: root.join("cs2"),
        out_root: root.join("out"),
        ..JobConfig::default()
    }
}

fn cleanup(cfg: &JobConfig) {
    let _ = std::fs::remove_dir_all(cfg.cs2_dir.parent().unwrap());
}

/// `record.json` на `n` дублей по 5 игровых секунд в кадре 1280x720, как у дублей фейка.
fn sample_record_json(n: usize, demo: PathBuf, plan_hash: String) -> RecordJson {
    let takes: Vec<TakePlan> = (0..n as u32)
        .map(|index| TakePlan {
            index,
            shot: index as usize,
            game_secs: 5.0,
            capture_fps: 60,
        })
        .collect();
    let mut args: Vec<String> = vec!["-y".into()];
    for t in &takes {
        for f in TakePlan::FILES {
            args.extend(["-i".to_owned(), format!("clips/{}/{f}", t.dir_name())]);
        }
    }
    args.extend(["-map", "[v]", "highlights.mp4"].map(String::from));
    RecordJson {
        version: 1,
        nonce: "ab12cd34".into(),
        demo_path: demo,
        demo_name: "cs2cinema_match_hero".into(),
        demo_len: 12,
        frame: (1280, 720),
        fps: 60,
        take_nvenc: false,
        takes: takes.iter().map(TakeSpec::from).collect(),
        ffmpeg_args: args,
        video_secs: 10.0,
        shots: takes.len(),
        secs: 5.0 * takes.len() as f32,
        plan_hash,
        created_unix: 1,
    }
}

fn facts() -> TakeFacts {
    TakeFacts {
        video_secs: 5.0,
        audio_secs: 5.0,
        width: 1280,
        height: 720,
        video_bytes: 10,
        audio_bytes: 4,
    }
}

/// Папка задачи, как её оставляет запись: скрипты, `record.json`, дубли на диске и манифест с заданными статусами.
/// Возвращает описание задачи «Повторить» (вторая попытка) и папку задачи.
fn retry_spec(tag: &str, statuses: &[TakeStatus]) -> (JobSpec, PathBuf) {
    let cfg = healthy_config(tag);
    let dir = cfg.out_root.join("match_hero");
    let nonce = "ab12cd34";
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("highlights.cfg"),
        format!("echo CS2C {nonce} SCRIPT\nexec cs2c_01\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("cs2c_01.cfg"),
        format!("echo CS2C {nonce} TAKE_END 0\n"),
    )
    .unwrap();
    let hash = job::plan_hash(&dir, nonce);
    let takes: Vec<TakePlan> = (0..statuses.len() as u32)
        .map(|index| TakePlan {
            index,
            shot: index as usize,
            game_secs: 5.0,
            capture_fps: 60,
        })
        .collect();
    let demo = dir.join("match.dem");
    std::fs::write(&demo, b"PBDEMS2\0demo").unwrap();
    let rj = sample_record_json(statuses.len(), demo, hash.clone());
    rj.save(&dir).unwrap();
    let clips = dir.join("clips");
    let mut m = Manifest::new(hash, nonce, (1280, 720), &takes);
    for (t, st) in takes.iter().zip(statuses) {
        let d = clips.join(t.dir_name());
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("video.mp4"), [b'v'; 10]).unwrap();
        std::fs::write(d.join("audio.wav"), [b'a'; 4]).unwrap();
        match st {
            TakeStatus::Pending => {}
            TakeStatus::Recorded => m.mark_recorded(t.index, (10, 4)),
            TakeStatus::Validated => m.mark_validated(t.index, &facts(), 1),
            TakeStatus::Invalid => m.mark_invalid(t.index, "обрезан", 1),
        }
    }
    m.save(&clips).unwrap();
    let spec = JobSpec {
        job_id: 1,
        attempt: 2,
        record: true,
        resume: true,
        source: DemoSource::Faceit("1-abc".into()),
        hero_steam: 42,
        hero_name: "me".into(),
        known_demo: None,
        last_good: None,
        out_dir: Some(dir.clone()),
        cfg,
    };
    (spec, dir)
}

/// Steam-раскладка вокруг CS2 в конфиге: `<lib>/steamapps/common/<игра>` и манифест рядом,
/// чтобы предпроверка нашла сборку CS2 (она попадает в снимок окружения).
fn with_steam_layout(cfg: &mut JobConfig) {
    let lib = cfg.cs2_dir.parent().unwrap().join("lib");
    let game = lib.join("steamapps/common/Counter-Strike Global Offensive");
    let exe = game.join("game/bin/win64/cs2.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(exe, b"x").unwrap();
    std::fs::write(
        lib.join("steamapps/appmanifest_730.acf"),
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n\t\"TargetBuildID\"\t\t\"0\"\n}\n",
    )
    .unwrap();
    cfg.cs2_dir = game;
}

#[test]
fn retry_with_every_take_validated_goes_straight_to_assembly() {
    let (spec, dir) = retry_spec("retry-ok", &[TakeStatus::Validated, TakeStatus::Validated]);
    // поддельный ffmpeg файлов не пишет: временный ролик кладёт сам тест
    std::fs::write(dir.join("highlights.part.mp4"), b"new").unwrap();
    std::fs::write(dir.join("highlights.mp4"), b"old").unwrap();
    let host = FakeHost::new(Scenario::happy(2));
    let out = job::resume(&host, &spec, &Capture::default())
        .expect("все дубли проверены — склейка без записи");

    assert_eq!(host.launches(), 0, "игра не запускалась");
    let runs = host.runs();
    let ffmpeg: Vec<_> = runs
        .iter()
        .filter(|(_, prog, _)| prog.contains("ffmpeg"))
        .collect();
    assert_eq!(ffmpeg.len(), 1, "{runs:?}");
    assert_eq!(
        ffmpeg[0].2.last().map(String::as_str),
        Some("highlights.part.mp4")
    );
    assert!(
        !runs
            .iter()
            .any(|(_, _, a)| a.iter().any(|x| x == "-version")),
        "предпроверка пропущена: {runs:?}"
    );
    assert_eq!(out.result, dir.join("highlights.mp4"));
    assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"new");
    assert!(!dir.join("highlights.part.mp4").exists());
    assert_eq!((out.shots, out.out_dir.as_path()), (2, dir.as_path()));
    cleanup(&spec.cfg);
}

#[test]
fn failed_assembly_keeps_the_previous_highlights_and_drops_the_part() {
    let (spec, dir) = retry_spec(
        "retry-asm-fail",
        &[TakeStatus::Validated, TakeStatus::Validated],
    );
    std::fs::write(dir.join("highlights.mp4"), b"old").unwrap();
    std::fs::write(dir.join("highlights.part.mp4"), b"half").unwrap();
    let failed = CmdOutput {
        code: Some(1),
        stderr_tail: vec!["Conversion failed".into()],
        elapsed: Duration::ZERO,
    };
    let host = FakeHost::new(Scenario::happy(1).run_result(Ok(failed)));
    let rj = RecordJson::load(&dir).unwrap();
    let e = assemble_stage(
        &host,
        &dir,
        &rj.take_plans(),
        &rj.ffmpeg_args,
        10.0,
        Duration::from_secs(60),
        &pipeline::Quiet,
    )
    .expect_err("ffmpeg упал");
    assert!(
        e.message.contains("ffmpeg завершился с ошибкой"),
        "{}",
        e.message
    );
    assert_eq!(
        std::fs::read(dir.join("highlights.mp4")).unwrap(),
        b"old",
        "прежний ролик цел"
    );
    assert!(
        !dir.join("highlights.part.mp4").exists(),
        "временный файл удалён"
    );
    cleanup(&spec.cfg);
}

#[test]
fn assembly_refuses_a_take_that_is_not_validated() {
    let (spec, dir) = retry_spec(
        "retry-asm-refuse",
        &[TakeStatus::Validated, TakeStatus::Recorded],
    );
    let host = FakeHost::new(Scenario::happy(1));
    let rj = RecordJson::load(&dir).unwrap();
    let e = assemble_stage(
        &host,
        &dir,
        &rj.take_plans(),
        &rj.ffmpeg_args,
        10.0,
        Duration::from_secs(60),
        &pipeline::Quiet,
    )
    .expect_err("дубль не проверен");
    let f = e.fail.as_ref().expect("сбой типизирован");
    assert_eq!(
        (f.code, f.state),
        (FailCode::TakeInvalid, RecordState::Take)
    );
    assert!(e.message.contains("take0001"), "{}", e.message);
    assert!(f.hint.contains("take0001"), "{}", f.hint);
    assert!(
        host.runs().is_empty(),
        "ffmpeg не запускался: {:?}",
        host.runs()
    );

    // файл изменился после проверки — тоже отказ
    let (spec2, dir2) = retry_spec("retry-asm-size", &[TakeStatus::Validated]);
    std::fs::write(dir2.join("clips/take0000/video.mp4"), b"xx").unwrap();
    let rj = RecordJson::load(&dir2).unwrap();
    let e = assemble_stage(
        &host,
        &dir2,
        &rj.take_plans(),
        &rj.ffmpeg_args,
        10.0,
        Duration::from_secs(60),
        &pipeline::Quiet,
    )
    .expect_err("размер другой");
    assert!(
        e.message.contains("размер video.mp4 изменился"),
        "{}",
        e.message
    );
    assert!(host.runs().is_empty());
    cleanup(&spec.cfg);
    cleanup(&spec2.cfg);
}

#[test]
fn retry_without_record_json_fails_with_retry_no_data() {
    let (spec, dir) = retry_spec("retry-nodata", &[TakeStatus::Validated]);
    RecordJson::remove(&dir).unwrap();
    let host = FakeHost::new(Scenario::happy(1));
    let e = job::resume(&host, &spec, &Capture::default()).expect_err("нет record.json");
    let f = e.fail.as_ref().expect("сбой типизирован");
    assert_eq!(f.code, FailCode::RetryNoData);
    assert_eq!(f.code.as_str(), "RETRY_NO_DATA");
    assert!(
        e.message.contains("запустите задачу заново"),
        "{}",
        e.message
    );
    assert_eq!(host.launches(), 0);
    cleanup(&spec.cfg);
}

#[test]
fn retry_without_a_known_job_folder_fails_with_retry_no_data() {
    let (mut spec, _dir) = retry_spec("retry-nofolder", &[TakeStatus::Validated]);
    spec.out_dir = None;
    let host = FakeHost::new(Scenario::happy(1));
    let e = job::resume(&host, &spec, &Capture::default()).expect_err("папка неизвестна");
    assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::RetryNoData));
    assert!(
        e.message.contains("папка задачи неизвестна"),
        "{}",
        e.message
    );
    cleanup(&spec.cfg);
}

#[test]
fn retry_with_changed_scripts_fails_with_retry_no_data() {
    let (spec, dir) = retry_spec("retry-changed", &[TakeStatus::Validated]);
    std::fs::write(dir.join("cs2c_01.cfg"), "echo правка руками\n").unwrap();
    let host = FakeHost::new(Scenario::happy(1));
    let e = job::resume(&host, &spec, &Capture::default()).expect_err("скрипты изменились");
    assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::RetryNoData));
    assert!(
        e.message
            .contains("скрипты записи в папке задачи изменились"),
        "{}",
        e.message
    );
    assert!(host.runs().is_empty());
    cleanup(&spec.cfg);
}

#[test]
fn retry_with_an_invalid_take_sets_clips_aside_and_records_again() {
    let (mut spec, dir) = retry_spec("retry-rec", &[TakeStatus::Validated, TakeStatus::Invalid]);
    let root = spec.cfg.cs2_dir.parent().unwrap().to_path_buf();
    with_steam_layout(&mut spec.cfg);
    // текущая настройка — другой кадр: запись обязана идти в кадре из record.json (1280x720)
    spec.cfg.frame = (1920, 1080);
    // поддельный ffmpeg склейки файлов не пишет
    std::fs::write(dir.join("highlights.part.mp4"), b"new").unwrap();
    let ok = CmdOutput {
        code: Some(0),
        stderr_tail: vec![],
        elapsed: Duration::ZERO,
    };
    let host = FakeHost::new(
        Scenario::happy(2)
            .clips_dir(dir.join("clips"))
            .program_for(
                "ffmpeg",
                "-version",
                &["ffmpeg version 7.1.1 Copyright (c) the FFmpeg developers"],
                Ok(ok.clone()),
            )
            .program_for(
                "ffprobe",
                "-version",
                &["ffprobe version 7.1.1 Copyright (c) the FFmpeg developers"],
                Ok(ok),
            ),
    );
    let cap = Capture::default();
    let out = job::resume(&host, &spec, &cap).expect("повтор записывает дубли заново и склеивает");

    assert_eq!(host.launches(), 1, "игра запущена один раз");
    assert_eq!(out.result, dir.join("highlights.mp4"));
    assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"new");
    assert!(
        !dir.join("installed.json").exists(),
        "папка игры убрана после записи"
    );

    // прежние дубли и манифест — в clips_old_<ts> как улики
    let old: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("clips_old_")
        })
        .collect();
    assert_eq!(old.len(), 1, "{old:?}");
    assert!(
        old[0].join("take0000/video.mp4").is_file() && old[0].join("take0001/video.mp4").is_file()
    );
    let old_manifest = Manifest::load(&old[0]).expect("прежний манифест");
    assert_eq!(
        old_manifest.entry(1).map(|e| e.status),
        Some(TakeStatus::Invalid)
    );

    // оба дубля записаны заново и проверены с номером попытки задачи
    let m = Manifest::load(&dir.join("clips")).expect("новый манифест");
    assert_eq!(m.takes.len(), 2);
    for t in &m.takes {
        assert_eq!(
            (t.status, t.attempt),
            (TakeStatus::Validated, spec.attempt),
            "{t:?}"
        );
    }

    // снимок окружения: env.json, манифест попытки и журнал
    let env: EnvSnapshot = serde_json::from_str(
        &std::fs::read_to_string(dir.join("env.json")).expect("env.json в папке задачи"),
    )
    .unwrap();
    assert_eq!(env.cs2_build.as_deref(), Some("25815307"));
    assert!(
        env.hlae_version
            .as_deref()
            .is_some_and(|v| v.starts_with("2.192")),
        "{env:?}"
    );
    assert!(
        env.ffmpeg.as_deref().is_some_and(|v| v.contains("7.1.1")),
        "{env:?}"
    );
    assert_eq!(env.frame, (1280, 720));
    assert_eq!(
        m.attempts.last().and_then(|a| a.env.as_ref()),
        Some(&env),
        "тот же снимок у попытки в манифесте"
    );
    let log = cap.log_text();
    assert!(log.contains("окружение: CS2 25815307"), "{log}");
    assert!(log.contains("прошлые дубли отложены в clips_old_"), "{log}");
    assert!(log.contains("дубли не все проверены: 2"), "{log}");

    // вызывающему ушли папка задачи и пара «сборка CS2 / версия HLAE» удачной записи
    let artifacts = cap.artifacts.lock().unwrap();
    assert!(
        artifacts.contains(&Artifact::OutDir(dir.clone())),
        "{artifacts:?}"
    );
    assert!(
        artifacts
            .iter()
            .any(|a| matches!(a, Artifact::LastGood { cs2_build, .. } if cs2_build == "25815307")),
        "{artifacts:?}"
    );
    assert!(
        artifacts
            .iter()
            .any(|a| matches!(a, Artifact::Precheck { passed: true, .. })),
        "{artifacts:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Описание первого запуска задачи (не «Повторить») на временном дереве.
fn first_run_spec(tag: &str, record: bool, source: DemoSource) -> JobSpec {
    JobSpec {
        job_id: 1,
        attempt: 1,
        record,
        resume: false,
        source,
        hero_steam: 42,
        hero_name: "me".into(),
        known_demo: None,
        last_good: None,
        out_dir: None,
        cfg: healthy_config(tag),
    }
}

#[test]
fn record_job_with_a_failing_precheck_stops_before_the_demo() {
    let mut spec = first_run_spec("first-foreign", true, DemoSource::Faceit("1-abc".into()));
    with_steam_layout(&mut spec.cfg);
    // чужой cs2.exe: запись невозможна, а скачивание демки ещё не началось бы
    let host = FakeHost::new(Scenario::happy(1).processes(vec![ProcInfo {
        pid: 777,
        exe_name: "cs2.exe".into(),
    }]));
    let cap = Capture::default();
    let e = job::run_job(&host, &spec, &cap).expect_err("чужая игра блокирует задачу");

    let f = e.fail.as_ref().expect("сбой типизирован");
    assert_eq!(f.code, FailCode::ForeignGameRunning);
    assert_eq!(f.code.as_str(), "FOREIGN_GAME_RUNNING");
    let artifacts = cap.artifacts.lock().unwrap();
    assert!(
        artifacts
            .iter()
            .any(|a| matches!(a, Artifact::Precheck { passed: false, .. })),
        "{artifacts:?}"
    );
    assert!(
        !artifacts.iter().any(|a| matches!(a, Artifact::Demo { .. })),
        "демку не скачивали: {artifacts:?}"
    );
    let log = cap.log_text();
    assert!(!log.contains("качаю демку"), "{log}");
    assert_eq!(host.launches(), 0);
    cleanup(&spec.cfg);
}

#[test]
fn plan_only_job_skips_the_precheck_and_reports_a_missing_demo() {
    let missing = std::env::temp_dir().join(format!(
        "cs2cinema-job-{}-no-such-demo.dem",
        std::process::id()
    ));
    let spec = first_run_spec("first-plan", false, DemoSource::File(missing.clone()));
    let host = FakeHost::new(Scenario::happy(1));
    let cap = Capture::default();
    let e = job::run_job(&host, &spec, &cap).expect_err("демки нет");

    assert!(
        e.message.contains(&*missing.to_string_lossy()),
        "в ошибке путь к файлу: {}",
        e.message
    );
    assert!(e.fail.is_none(), "это не сбой записи: {e:?}");
    let runs = host.runs();
    assert!(
        !runs
            .iter()
            .any(|(_, _, args)| args.iter().any(|a| a == "-version")),
        "предпроверка пропущена, `-version` не запускали: {runs:?}"
    );
    assert!(
        !cap.artifacts
            .lock()
            .unwrap()
            .iter()
            .any(|a| matches!(a, Artifact::Precheck { .. }))
    );
    cleanup(&spec.cfg);
}
