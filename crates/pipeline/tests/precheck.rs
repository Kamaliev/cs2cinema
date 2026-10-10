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

/// Хост со здоровыми утилитами: ffmpeg, ffprobe и ffmpeg HLAE отвечают на `-version`.
fn healthy() -> Scenario {
    Scenario::happy(1)
        .program(
            "hlae-ffmpeg",
            &["ffmpeg version 7.1-full_build"],
            answered(),
        )
        .program("ffmpeg", &["ffmpeg version 7.1-full_build"], answered())
        .program("ffprobe", &["ffprobe version 7.1-full_build"], answered())
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
