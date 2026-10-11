//! Вертикальный срез ссылок FACEIT (BOT-05, D-01): друг присылает ссылку боту, сервер ставит задачу с канонической
//! ссылкой без `getFile`, раннер скачивает демку своим ключом, записывает ролик, а бот присылает его в чат друга.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Stand, TestDir, fake_demo};
use pipeline::host::fake::{FakeHost, Scenario};
use pipeline::host::CmdOutput;
use pipeline::inspect::InspectLimits;
use pipeline::job::{DemoSource, JobConfig, Outcome};
use runner::{InspectSettings, PipelineExecutor, Runner, Tick};
use server::bot::store;
use server::bot::{Bot, BotSettings};
use server::log::{Log, MemorySink};
use server::tg::fake::FakeTelegram;
use wire::JobState;

const OK_REPLY: &str = r#"{"ok":true,"player_name":"s1mple","player_steam":76561198034202275,"moments":3,"utility":1,"peak_mem_bytes":1048576}"#;
const GOOD_PROBE: &str = r#"{"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"5.000000"}}"#;

const MATCH_ID: &str = "1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
const AUTHOR_KEY: &str = "ключ-автора";

fn exit0() -> Result<CmdOutput, pipeline::host::HostError> {
    Ok(CmdOutput {
        code: Some(0),
        stderr_tail: Vec::new(),
        elapsed: Duration::ZERO,
    })
}

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

fn probing_host() -> FakeHost {
    FakeHost::new(
        Scenario::happy(1)
            .program("cs2-cli", &[OK_REPLY], exit0())
            .program("ffprobe", &[GOOD_PROBE], exit0()),
    )
}

/// Конфиг задачи с пустыми папками инструментов (до CS2 дело не доходит).
fn job_config(dir: &TestDir) -> JobConfig {
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
        faceit_key: AUTHOR_KEY.to_owned(),
        ..JobConfig::default()
    }
}

/// Все файлы под `root` (рекурсивно).
fn files_under(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn faceit_link_from_a_friend_is_downloaded_by_the_runner_and_delivered() {
    let stand = Stand::start();
    let core = stand.core();
    server::users::add_user(core.db(), "Друг", Some(111), false, core.clock().now_ms()).unwrap();

    let tg_dir = TestDir::new("faceit-tg");
    let tg = FakeTelegram::new(tg_dir.path.clone());
    let sink = Arc::new(MemorySink::default());
    let log = Log::new(sink.clone(), core.redactor().clone());
    let bot = Bot::new(
        core.clone(),
        Arc::new(tg.clone()),
        log,
        BotSettings::new(&stand.dir.path.join("data"), tg.files_dir()),
    );

    // Друг присылает ссылку с ником после неё.
    tg.push_update(tg.text_update(
        111,
        111,
        &format!("https://www.faceit.com/ru/cs2/room/{MATCH_ID} s1mple"),
    ));
    bot.step().expect("шаг бота");
    assert_eq!(tg.get_file_calls(), 0, "по ссылке getFile не вызывается");
    let tg_jobs = store::tg_jobs(core.db()).unwrap();
    assert_eq!(tg_jobs.len(), 1, "{tg_jobs:?}");
    let id = tg_jobs[0].job_id;
    let job = stand.job(id);
    assert_eq!(job.state, JobState::Queued);
    assert_eq!(job.source_kind, "faceit");
    assert_eq!(job.player_name, "s1mple");
    assert_eq!(
        job.faceit_link.as_deref(),
        Some(format!("https://www.faceit.com/en/cs2/room/{MATCH_ID}").as_str()),
        "в базе только канонический вид"
    );

    // Раннер: скачивание подменено, ролик «записан» заглушкой, ключ приходит из настроек раннера.
    let dir = TestDir::new("faceit-runner");
    let host = probing_host();
    let calls: Arc<Mutex<Vec<(String, String, PathBuf)>>> = Arc::default();
    let seen = calls.clone();
    let runner_dir = stand.runner_dir();
    let exec = PipelineExecutor::new(
        Arc::new(host.clone()),
        job_config(&dir),
        InspectSettings {
            exe: PathBuf::from("cs2-cli"),
            limits: InspectLimits::default(),
        },
    )
    .with_fetch_fn(move |match_id, key, dir| {
        seen.lock()
            .unwrap()
            .push((match_id.to_owned(), key.to_owned(), dir.to_path_buf()));
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("demo.dem.zst");
        std::fs::write(&path, fake_demo(4096)).unwrap();
        Ok(path)
    })
    .with_job_fn(move |_, spec, _| {
        match &spec.source {
            DemoSource::File(p) => assert!(p.is_file(), "демка должна лежать на месте: {p:?}"),
            other => panic!("ждали файл, а не {other:?}"),
        }
        let out_dir = runner_dir.join("out").join(format!("srv{}", spec.job_id));
        std::fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        std::fs::write(&result, mp4(&[b"ftyp", b"moov", b"mdat"])).unwrap();
        Ok(Outcome {
            out_dir,
            result,
            shots: 1,
            secs: 5.0,
        })
    });
    let mut runner = Runner::new(stand.runner_config(), Arc::new(exec)).unwrap();
    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "скачивание вызвано один раз: {calls:?}");
    assert_eq!(calls[0].0, MATCH_ID);
    assert_eq!(calls[0].1, AUTHOR_KEY);
    assert!(
        !calls[0].2.exists(),
        "временная папка скачивания убрана: {:?}",
        calls[0].2
    );

    // Бот присылает ролик в чат друга.
    for _ in 0..5 {
        bot.step().expect("шаг бота");
        if !tg.videos().is_empty() {
            break;
        }
    }
    let videos = tg.videos();
    assert!(videos.iter().any(|v| v.chat == 111), "ролик не доставлен: {videos:?}");

    // Ключ автора не попадает ни в журнал бота, ни в базу и файлы сервера, ни в текст сбоя.
    let needle = AUTHOR_KEY.as_bytes();
    assert!(
        sink.lines().iter().all(|l| !l.contains(AUTHOR_KEY)),
        "ключ в журнале: {:?}",
        sink.lines()
    );
    let mut files = Vec::new();
    files_under(&stand.dir.path.join("data"), &mut files);
    assert!(!files.is_empty());
    for f in files {
        let bytes = std::fs::read(&f).unwrap_or_default();
        assert!(!contains(&bytes, needle), "ключ найден в {f:?}");
    }
    for t in tg.texts_to(111) {
        assert!(!t.contains(AUTHOR_KEY), "ключ в чате: {t}");
    }
}
