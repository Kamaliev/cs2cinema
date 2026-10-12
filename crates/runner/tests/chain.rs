//! Сквозная цепочка фазы 9 (OPS-03): друг присылает демку в чат, бот ставит задачу, настоящий сервер выдаёт её
//! раннеру, раннер на `FakeHost` «записывает» ролик и отдаёт его серверу, бот присылает ролик в чат друга.
//!
//! Что настоящее: сервер (`server::serve` в процессе теста, порт `127.0.0.1:0`), бот (`server::bot::Bot`),
//! раннер (`runner::Runner` с `PipelineExecutor`), база и файлы сервера. Что подменено: Telegram (`FakeTelegram`)
//! и внешние процессы (`FakeHost`: разбор демки дочерним `cs2-cli`, ffprobe, а запись CS2 и ffmpeg заменяет
//! `with_job_fn`). Тест проверяет транспорт, состояния, доставку и тексты, а не монтаж: монтаж охраняет
//! `crates/cli/tests/pipeline.rs`.
//!
//! Тест лежит в `runner`, потому что `server/tests/deps.rs` запрещает серверу зависеть от раннера даже в
//! dev-зависимостях. Бот ведётся шагами `bot.step()`, а не потоками: порядок детерминирован и без ожиданий
//! по времени. Каждый тест берёт свои временные папки; токены стенда подставные и в вывод не печатаются.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{Stand, TestDir, fake_demo};
use pipeline::host::CmdOutput;
use pipeline::host::fake::{FakeHost, Scenario};
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

const ADMIN: i64 = 100;
const FRIEND: i64 = 111;

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

/// Хост, у которого предпроверка демки и ffprobe отвечают «всё хорошо».
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
        ..JobConfig::default()
    }
}

/// Бот на ядре стенда: настоящий `Bot`, Telegram подставной, журнал в памяти с тем же `Redactor`, что у сервера.
fn chain_bot(stand: &Stand, tg: &FakeTelegram) -> (Bot, Arc<MemorySink>) {
    let core = stand.core();
    let sink = Arc::new(MemorySink::default());
    let log = Log::new(sink.clone(), core.redactor().clone());
    let bot = Bot::new(
        core,
        Arc::new(tg.clone()),
        log,
        BotSettings::new(&stand.dir.path.join("data"), tg.files_dir()),
    );
    (bot, sink)
}

/// Администратор и друг в пользователях сервера.
fn add_people(stand: &Stand) {
    let core = stand.core();
    let now = core.clock().now_ms();
    server::users::add_user(core.db(), "админ", Some(ADMIN), true, now).unwrap();
    server::users::add_user(core.db(), "друг", Some(FRIEND), false, now).unwrap();
}

/// Друг присылает демку; возвращает номер поставленной задачи.
fn friend_sends_demo(stand: &Stand, tg: &FakeTelegram, bot: &Bot) -> i64 {
    let bytes = fake_demo(4096);
    tg.put_document("file-1", "uniq-1", &bytes);
    let upd = tg.document_update(
        FRIEND,
        FRIEND,
        "file-1",
        "uniq-1",
        "match.dem",
        bytes.len() as u64,
        Some("s1mple"),
    );
    tg.push_update(upd);
    bot.step().expect("шаг бота");
    let jobs = store::tg_jobs(stand.core().db()).unwrap();
    assert_eq!(jobs.len(), 1, "после демки друга должна быть одна задача: {jobs:?}");
    jobs[0].job_id
}

/// Ничто из служебного не должно попадать другу: ни токен стенда, ни пути сервера и раннера.
fn assert_friend_texts_clean(stand: &Stand, tg: &FakeTelegram) {
    for t in tg.texts_to(FRIEND) {
        for bad in ["blobs", "out/", ":\\", stand.token.as_str()] {
            assert!(!t.contains(bad), "в сообщении другу служебная подробность: {t}");
        }
    }
}

#[test]
fn demo_from_a_friend_goes_through_the_server_and_the_runner_and_comes_back_as_a_video() {
    let stand = Stand::start();
    add_people(&stand);
    let tg_dir = TestDir::new("chain-tg");
    let tg = FakeTelegram::new(tg_dir.path.clone());
    let (bot, sink) = chain_bot(&stand, &tg);

    // Друг присылает демку: на сервере появляется задача в очереди, а друг получает номер.
    let id = friend_sends_demo(&stand, &tg, &bot);
    assert_eq!(stand.job(id).state, JobState::Queued);
    assert!(
        tg.texts_to(FRIEND).iter().any(|t| t.contains(&format!("#{id}"))),
        "друг не получил номер задачи: {:?}",
        tg.texts_to(FRIEND)
    );

    // Раннер: предпроверка и ffprobe в FakeHost, «запись» подменена файлом нужной формы.
    let dir = TestDir::new("chain-runner");
    let runner_dir = stand.runner_dir();
    let out_root = runner_dir.clone();
    let exec = PipelineExecutor::new(
        Arc::new(probing_host()),
        job_config(&dir),
        InspectSettings {
            exe: "cs2-cli".into(),
            limits: InspectLimits::default(),
        },
    )
    .with_job_fn(move |_, spec, _| {
        match &spec.source {
            DemoSource::File(p) => assert!(p.is_file(), "демка должна лежать на месте: {p:?}"),
            other => panic!("ждали файл, а не {other:?}"),
        }
        let out_dir = out_root.join("out").join(format!("srv{}", spec.job_id));
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
    assert_eq!(stand.job(id).state, JobState::Done);

    // Бот доставляет ролик: до пяти шагов, пока он не появится в чате друга.
    for _ in 0..5 {
        bot.step().expect("шаг бота");
        if !tg.videos().is_empty() {
            break;
        }
    }
    let videos = tg.videos();
    assert_eq!(videos.len(), 1, "ролик должен прийти ровно один раз: {videos:?}");
    assert_eq!(videos[0].chat, FRIEND, "ролик ушёл не в чат друга: {videos:?}");
    // Лишние шаги ролик не дублируют.
    bot.step().expect("шаг бота");
    bot.step().expect("шаг бота");
    assert_eq!(tg.videos().len(), 1, "повторные шаги продублировали ролик");

    let tg_job = store::tg_job(stand.core().db(), id).unwrap().expect("запись tg_jobs");
    assert_eq!(tg_job.delivery, "sent", "доставка: {tg_job:?}");
    assert!(
        stand.results_dir(id).join("highlights.mp4").is_file(),
        "на сервере нет принятого ролика"
    );

    // Раннер убрал за собой: ни папки дублей, ни скачанной демки.
    assert!(
        !runner_dir.join("out").join(format!("srv{id}")).exists(),
        "папка дублей раннера не удалена"
    );
    assert!(
        !runner_dir.join("remote").join(format!("srv{id}.dem")).exists(),
        "демка раннера не удалена"
    );

    // Другу не показывают служебного, а токен раннера не попадает ни в журнал бота, ни в чат.
    assert_friend_texts_clean(&stand, &tg);
    assert!(
        sink.lines().iter().all(|l| !l.contains(&stand.token)),
        "токен раннера в журнале бота"
    );
}
