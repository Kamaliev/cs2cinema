//! Предпроверка на фейковом хосте и временных папках: без CS2, HLAE и ffmpeg.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pipeline::host::fake::{FakeHost, Scenario};
use pipeline::host::{CmdOutput, Host, HostError, ProcInfo};
use pipeline::precheck::{self, Demo, Inputs, Level, Report, ids};
use pipeline::{CANCELLED, Deadlines, FailCode, Progress, Quiet, Stage};

/// Временное окружение: папка HLAE, дерево Steam с CS2, папка результатов и демка.
struct Env {
    root: PathBuf,
    inputs: Inputs,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn touch(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Здоровое окружение; `name` делает папку уникальной для теста.
fn env(name: &str) -> Env {
    let root =
        std::env::temp_dir().join(format!("cs2cinema-precheck-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let hlae = root.join("hlae");
    touch(&hlae.join("HLAE.exe"), b"");
    touch(&hlae.join("x64/AfxHookSource2.dll"), b"");
    let hlae_ffmpeg = hlae.join("hlae-ffmpeg.exe");
    touch(&hlae_ffmpeg, b"");
    touch(
        &hlae.join("ffmpeg/ffmpeg.ini"),
        format!("[Ffmpeg]\nPath=\"{}\"\n", hlae_ffmpeg.display()).as_bytes(),
    );
    let lib = root.join("lib");
    let cs2 = lib.join("steamapps/common/Counter-Strike Global Offensive");
    touch(&cs2.join("game/bin/win64/cs2.exe"), b"");
    touch(
        &lib.join("steamapps/appmanifest_730.acf"),
        b"\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n}\n",
    );
    touch(
        &cs2.join("game/csgo/steam.inf"),
        b"ClientVersion=2000\nPatchVersion=1.41.9.0\n",
    );
    let out_root = root.join("out");
    let demo = root.join("match.dem");
    touch(&demo, b"PBDEMS2\0rest of the demo");
    let inputs = Inputs {
        hlae_dir: hlae,
        cs2_dir: cs2,
        out_root,
        demo: Demo::File(demo),
        last_good: Some(("25815307".to_owned(), "2.192.7.0".to_owned())),
        deadlines: Deadlines::default(),
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
    };
    Env { root, inputs }
}

fn answered() -> Result<CmdOutput, HostError> {
    Ok(CmdOutput {
        code: Some(0),
        stderr_tail: vec![],
        elapsed: Duration::ZERO,
    })
}

/// Хост без ответа ffmpeg: ffprobe и ffmpeg HLAE отвечают на `-version`. Имя `hlae-ffmpeg` стоит
/// первым, потому что фейк ищет утилиту по вхождению имени.
fn without_ffmpeg() -> Scenario {
    Scenario::happy(1)
        .program(
            "hlae-ffmpeg",
            &["ffmpeg version 7.1-full_build"],
            answered(),
        )
        .program("ffprobe", &["ffprobe version 7.1-full_build"], answered())
}

/// Хост со здоровыми утилитами: ffmpeg, ffprobe и ffmpeg HLAE отвечают на `-version`.
fn healthy() -> Scenario {
    without_ffmpeg().program("ffmpeg", &["ffmpeg version 7.1-full_build"], answered())
}

fn proc(pid: u32, name: &str) -> ProcInfo {
    ProcInfo {
        pid,
        exe_name: name.to_owned(),
    }
}

fn item<'a>(r: &'a Report, id: &str) -> &'a precheck::Item {
    r.items
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("нет пункта {id}: {:?}", r.items))
}

/// Прогресс с флагом отмены.
struct Flag(AtomicBool);

impl Progress for Flag {
    fn stage(&self, _: Stage) {}
    fn log(&self, _: &str) {}
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn foreign_cs2_is_reported_and_left_alive() {
    let e = env("foreign");
    let fake =
        FakeHost::new(healthy().processes(vec![proc(100, "steam.exe"), proc(777, "cs2.exe")]));
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let f = r
        .failure(Duration::ZERO)
        .expect("чужой CS2 блокирует старт");
    assert_eq!(f.code, FailCode::ForeignGameRunning);
    let it = item(&r, ids::FOREIGN_GAME);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("777"), "{}", it.message);
    assert_eq!(fake.terminated(), 0, "чужой процесс не завершается");
    assert_eq!(fake.launches(), 0, "предпроверка ничего не запускает");
}

#[test]
fn previous_own_game_that_never_exits_gives_prev_game_stuck() {
    let e = env("stuck");
    // 4242 - PID игры в фейке: она же видна в снимке процессов, но своя, а не чужая
    let fake = FakeHost::new(
        healthy()
            .previous_game(None)
            .processes(vec![proc(100, "steam.exe"), proc(4242, "cs2.exe")]),
    );
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let f = r
        .failure(Duration::ZERO)
        .expect("зависшая игра блокирует старт");
    assert_eq!(f.code, FailCode::PrevGameStuck);
    assert!(
        fake.now() >= Duration::from_secs(30),
        "ждали {:?}",
        fake.now()
    );
    assert_ne!(
        item(&r, ids::FOREIGN_GAME).level,
        Level::Fail,
        "свой PID не чужой"
    );
}

#[test]
fn previous_own_game_that_exits_lets_the_job_start_after_the_settle_pause() {
    let e = env("exits");
    let fake = FakeHost::new(healthy().previous_game(Some(Duration::from_secs(5))));
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(item(&r, ids::PREV_GAME).level, Level::Ok);
    assert!(
        fake.now() >= Duration::from_secs(8),
        "ждали {:?}",
        fake.now()
    );
}

#[test]
fn no_steam_blocks_with_precheck_failed() {
    let e = env("nosteam");
    let fake = FakeHost::new(healthy().processes(vec![]));
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let f = r.failure(Duration::ZERO).expect("без Steam старта нет");
    assert_eq!(f.code, FailCode::PrecheckFailed);
    assert_eq!(item(&r, ids::STEAM).level, Level::Fail);
}

#[test]
fn cancel_during_the_wait_returns_the_cancelled_sentinel() {
    let e = env("cancel");
    let fake = FakeHost::new(healthy().previous_game(Some(Duration::from_secs(5))));
    let p = Flag(AtomicBool::new(true));
    let err = precheck::run(&fake, &e.inputs, &p).unwrap_err();
    assert_eq!(err, CANCELLED);
}

#[test]
fn healthy_environment_gives_an_all_ok_report() {
    let e = env("healthy");
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(r.worst(), Level::Ok, "{:#?}", r.items);
    for id in [
        ids::PREV_GAME,
        ids::FOREIGN_GAME,
        ids::STEAM,
        ids::STEAM_UPDATE,
        ids::CS2_BUILD,
        ids::HLAE,
        ids::HOOK,
        ids::CS2_EXE,
        ids::FFMPEG,
        ids::FFPROBE,
        ids::HLAE_FFMPEG,
        ids::DEMO,
        ids::OUT_DIR,
        ids::SPACE_RESULTS,
        ids::SPACE_CS2,
    ] {
        item(&r, id);
    }
    assert_eq!(r.items.len(), 15);
    assert_eq!(r.cs2_build.as_deref(), Some("25815307"));
    assert_eq!(r.hlae_version.as_deref(), Some("2.192.7.0"));
    assert!(!r.blocks());
}

#[test]
fn missing_hook_dll_fails_with_the_settings_hint() {
    let e = env("nohook");
    std::fs::remove_file(e.inputs.hlae_dir.join("x64/AfxHookSource2.dll")).unwrap();
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::HOOK);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("AfxHookSource2"), "{}", it.message);
    assert!(r.blocks());
    assert_eq!(
        r.failure(Duration::ZERO).unwrap().code,
        FailCode::PrecheckFailed
    );
}

#[test]
fn ffmpeg_that_times_out_fails_the_report() {
    let e = env("timeout");
    let fake = FakeHost::new(without_ffmpeg().program("ffmpeg", &[], Err(HostError::Timeout)));
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::FFMPEG);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("нет ответа"), "{}", it.message);
    assert!(r.blocks());
    assert_eq!(
        item(&r, ids::FFPROBE).level,
        Level::Ok,
        "остальные утилиты не пострадали"
    );
}

#[test]
fn ffmpeg_that_cannot_start_fails_with_the_reason() {
    let e = env("nospawn");
    let fake = FakeHost::new(without_ffmpeg().program(
        "ffmpeg",
        &[],
        Err(HostError::Spawn("файл не найден".into())),
    ));
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::FFMPEG);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("не запускается"), "{}", it.message);
}

#[test]
fn cancelled_tool_probe_returns_the_cancelled_sentinel() {
    let e = env("probecancel");
    let fake = FakeHost::new(without_ffmpeg().program("ffmpeg", &[], Err(HostError::Cancelled)));
    assert_eq!(
        precheck::run(&fake, &e.inputs, &Quiet).unwrap_err(),
        CANCELLED
    );
}

#[test]
fn lfs_pointer_demo_is_caught_before_download() {
    let e = env("lfs");
    let Demo::File(demo) = &e.inputs.demo else {
        unreachable!()
    };
    std::fs::write(
        demo,
        "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 123\n",
    )
    .unwrap();
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::DEMO);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("LFS"), "{}", it.message);
}

#[test]
fn missing_demo_and_download_are_told_apart() {
    let mut e = env("demomissing");
    e.inputs.demo = Demo::File(e.root.join("нет.dem"));
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(item(&r, ids::DEMO).level, Level::Fail);

    e.inputs.demo = Demo::Download;
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(item(&r, ids::DEMO).level, Level::Ok);

    e.inputs.demo = Demo::NotApplicable;
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert!(
        r.items.iter().all(|i| i.id != ids::DEMO),
        "без задачи демки нет"
    );
}

#[test]
fn steam_updating_cs2_blocks_the_start() {
    let e = env("updating");
    let manifest = e
        .inputs
        .cs2_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("appmanifest_730.acf");
    std::fs::write(
        manifest,
        "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"1026\"\n\t\"buildid\"\t\t\"25815307\"\n}\n",
    )
    .unwrap();
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(item(&r, ids::STEAM_UPDATE).level, Level::Fail);
    assert!(r.blocks());
}

#[test]
fn changed_build_since_last_good_only_warns() {
    let mut e = env("newbuild");
    e.inputs.last_good = Some(("25000000".to_owned(), "2.192.7.0".to_owned()));
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::CS2_BUILD);
    assert_eq!(it.level, Level::Warn);
    assert!(it.message.contains("25000000") && it.message.contains("25815307"));
    assert_eq!(r.worst(), Level::Warn);
    assert!(!r.blocks(), "предупреждение старт не блокирует (D-02)");
}

#[test]
fn low_space_by_plan_fails_after_the_second_pass() {
    let e = env("space");
    // 5 ГиБ хватает на минимум до плана (4 ГиБ на одном томе), но не на оценку по плану
    let fake = FakeHost::new(healthy().free_space(Ok(5 << 30)));
    let mut r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert!(!r.blocks(), "до плана места хватает: {:#?}", r.items);

    let need = precheck::space::Need {
        results: 5 << 30,
        cs2: 1 << 30,
    };
    precheck::check_space(&fake, &e.inputs, need, &mut r);
    assert_eq!(item(&r, ids::SPACE_RESULTS).level, Level::Fail);
    assert!(item(&r, ids::SPACE_RESULTS).message.contains("по плану"));
    assert!(r.blocks());
    assert_eq!(r.items.len(), 15, "пункты места заменены, а не дописаны");
}

#[test]
fn results_path_with_spaces_fails() {
    let mut e = env("spaces");
    e.inputs.out_root = e.root.join("my results");
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let it = item(&r, ids::OUT_DIR);
    assert_eq!(it.level, Level::Fail);
    assert!(it.message.contains("пробел"), "{}", it.message);
}

#[test]
fn results_probe_file_is_removed() {
    let e = env("probefile");
    let fake = FakeHost::new(healthy());
    precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    let left: Vec<_> = std::fs::read_dir(&e.inputs.out_root)
        .unwrap()
        .flatten()
        .collect();
    assert!(left.is_empty(), "пробный файл остался: {left:?}");
}

#[test]
fn hlae_ffmpeg_follows_the_ini_then_the_bundled_binary() {
    let e = env("hlaeffmpeg");
    let from_ini = precheck::hlae_ffmpeg(&e.inputs.hlae_dir).unwrap();
    assert!(from_ini.ends_with("hlae-ffmpeg.exe"), "{from_ini:?}");

    std::fs::remove_file(e.inputs.hlae_dir.join("ffmpeg/ffmpeg.ini")).unwrap();
    assert_eq!(precheck::hlae_ffmpeg(&e.inputs.hlae_dir), None);

    touch(&e.inputs.hlae_dir.join("ffmpeg/bin/ffmpeg.exe"), b"");
    let bundled = precheck::hlae_ffmpeg(&e.inputs.hlae_dir).unwrap();
    assert!(bundled.ends_with("ffmpeg/bin/ffmpeg.exe"), "{bundled:?}");
}

#[test]
fn hlae_without_any_ffmpeg_fails() {
    let e = env("hlaenone");
    std::fs::remove_file(e.inputs.hlae_dir.join("ffmpeg/ffmpeg.ini")).unwrap();
    let fake = FakeHost::new(healthy());
    let r = precheck::run(&fake, &e.inputs, &Quiet).unwrap();
    assert_eq!(item(&r, ids::HLAE_FFMPEG).level, Level::Fail);
}
