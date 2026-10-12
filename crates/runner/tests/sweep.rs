//! Уборка рабочей папки раннера: сиротские дубли, скачанные демки, журнал, подтверждённые задачи.
//! Возраст файлов задаётся `File::set_modified`, сервер — настоящий (`Stand`).

mod common;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use common::{Scenario, ScriptedExecutor, Stand, TestDir, result_bytes, sha256_hex};
use pipeline::job::Outcome as Produced;
use pipeline::{Progress, RunError};
use runner::{
    Executor, HeldJob, Link, ResultFile, Runner, RunnerState, Stage, Task, Tick, Token, sweep,
};
use wire::{Assignment, FinishRequest, JobState, Outcome};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// Создаёт файл с содержимым и ставит ему изменение `age` назад.
fn put(path: &Path, age: Duration) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
    path.to_path_buf()
}

fn days(n: u64) -> Duration {
    Duration::from_secs(n * DAY)
}

fn runner_log(stand: &Stand) -> String {
    fs::read_to_string(stand.runner_dir().join("runner.log")).unwrap_or_default()
}

#[test]
fn idle_runner_sweeps_old_orphan_take_folders_and_keeps_fresh_ones() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    // Имя как у настоящего `run_job`: метка демки и герой.
    put(&dir.join("out/srv7_s1mple/take1.mp4"), days(4));
    put(&dir.join("out/srv8/take1.mp4"), Duration::ZERO);

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    assert!(
        !dir.join("out/srv7_s1mple").exists(),
        "старая сиротская папка осталась"
    );
    assert!(dir.join("out/srv8").exists(), "свежая папка удалена");
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("уборка:") && l.contains("#7")),
        "в журнале нет итога уборки:\n{log}"
    );
}

#[test]
fn old_remote_demos_parts_and_faceit_folders_are_swept() {
    let tmp = TestDir::new("sweep-remote");
    let dir = &tmp.path;
    let old = days(2);
    put(&dir.join("remote/srv9.dem"), old);
    put(&dir.join("remote/srv9.dem.part"), old);
    put(&dir.join("remote/srv9.faceit/demo.dem.zst"), old);
    put(&dir.join("remote/srv10.dem"), Duration::ZERO);

    let swept = sweep::sweep(dir, None, SystemTime::now());

    assert!(!dir.join("remote/srv9.dem").exists());
    assert!(!dir.join("remote/srv9.dem.part").exists());
    assert!(!dir.join("remote/srv9.faceit").exists());
    assert!(
        dir.join("remote/srv10.dem").exists(),
        "свежая демка удалена"
    );
    assert_eq!(swept.remote_items, 3, "{swept:?}");
    assert_eq!(swept.errors, 0);
}

#[test]
fn held_job_is_never_swept() {
    let tmp = TestDir::new("sweep-held");
    let dir = &tmp.path;
    let ancient = days(10);
    put(&dir.join("out/srv5_s1mple/take.mp4"), ancient);
    put(&dir.join("remote/srv5.dem"), ancient);
    put(&dir.join("out/srv6_s1mple/take.mp4"), ancient);

    let swept = sweep::sweep(dir, Some(5), SystemTime::now());

    assert!(dir.join("out/srv5_s1mple/take.mp4").exists());
    assert!(dir.join("remote/srv5.dem").exists());
    assert!(
        !dir.join("out/srv6_s1mple").exists(),
        "чужая старая папка осталась"
    );
    assert_eq!(swept.out_dirs, vec![6]);
}

#[test]
fn foreign_names_and_links_are_left_alone() {
    let tmp = TestDir::new("sweep-foreign");
    let dir = &tmp.path;
    let ancient = days(30);
    let keep = [
        "out/notes.txt",
        "out/srvX/a.bin",
        "out/srv12abc/a.bin",
        "out/srv_12/a.bin",
        "out/srv/a.bin",
        "out/srv012/a.bin",
        "remote/other.dem",
        "remote/srv3.mp4",
        "remote/srv.dem",
        "remote/srv3x.dem",
        "remote/srv3.dem.bak",
    ];
    for name in keep {
        put(&dir.join(name), ancient);
    }
    // Файл с именем папки дублей — не папка.
    put(&dir.join("out/srv14"), ancient);
    // Файл вне out/ и remote/ рабочей папки.
    put(&dir.join("srv15/take.mp4"), ancient);
    put(&dir.join("runner.log"), ancient);

    #[cfg(unix)]
    let outside = {
        let outside = TestDir::new("sweep-outside");
        put(&outside.path.join("take.mp4"), ancient);
        std::os::unix::fs::symlink(&outside.path, dir.join("out/srv13")).unwrap();
        outside
    };

    let swept = sweep::sweep(dir, None, SystemTime::now());

    for name in keep {
        assert!(dir.join(name).exists(), "удалено чужое: {name}");
    }
    assert!(dir.join("out/srv14").is_file());
    assert!(dir.join("srv15/take.mp4").exists());
    assert!(dir.join("runner.log").exists());
    assert!(swept.is_empty(), "{swept:?}");
    #[cfg(unix)]
    {
        assert!(dir.join("out/srv13").is_symlink(), "ссылка удалена");
        assert!(
            outside.path.join("take.mp4").exists(),
            "цель ссылки удалена"
        );
    }
}

/// Заполняет журнал строками «СТАРОЕ …» так, что он больше `bytes`.
fn fill_log(path: &Path, bytes: u64) {
    let line = "СТАРОЕ строка журнала раннера\n";
    let mut out = std::io::BufWriter::new(File::create(path).unwrap());
    let mut written = 0u64;
    while written <= bytes {
        out.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
    }
    out.flush().unwrap();
}

#[test]
fn runner_log_is_rotated_when_it_grows_too_large() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    fs::create_dir_all(&dir).unwrap();
    fill_log(&dir.join("runner.log"), sweep::LOG_MAX_BYTES);

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    let old = fs::read_to_string(dir.join("runner.log.1")).unwrap();
    assert!(old.contains("СТАРОЕ"), "прежний журнал не перенесён в .1");
    let new = fs::read_to_string(dir.join("runner.log")).unwrap();
    assert!(
        new.len() < 1 << 20,
        "новый журнал велик: {} байт",
        new.len()
    );
    assert!(
        !new.contains("СТАРОЕ"),
        "старые строки остались в новом журнале"
    );
    assert!(
        new.contains("на связи с сервером"),
        "в новом журнале нет строк:\n{new}"
    );
    assert!(
        !old.contains(&stand.token) && !new.contains(&stand.token),
        "токен в журнале"
    );
}

#[test]
fn runner_log_is_rotated_on_the_sweep_pass() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    // Журнал разросся, пока раннер работал: его хэндл открыт, запись идёт в конец.
    {
        let mut f = File::options()
            .append(true)
            .open(dir.join("runner.log"))
            .unwrap();
        let line = "СТАРОЕ строка журнала раннера\n";
        let mut written = 0u64;
        while written <= sweep::LOG_MAX_BYTES {
            f.write_all(line.as_bytes()).unwrap();
            written += line.len() as u64;
        }
    }
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    let old = fs::read_to_string(dir.join("runner.log.1")).unwrap();
    assert!(old.contains("СТАРОЕ"));
    let new = fs::read_to_string(dir.join("runner.log")).unwrap_or_default();
    assert!(!new.contains("СТАРОЕ"));
    // Новые строки идут уже в новый файл.
    let before = new.len();
    runner.tick().unwrap();
    let after = fs::read_to_string(dir.join("runner.log")).unwrap().len();
    assert!(after >= before);
}

#[test]
fn sweep_runs_at_most_once_per_interval() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    put(&dir.join("out/srv21/take1.mp4"), days(4));
    assert_eq!(runner.tick().unwrap(), Tick::Idle);
    assert!(
        dir.join("out/srv21").exists(),
        "уборка прошла раньше, чем через SWEEP_EVERY"
    );
}

// ---------------------------------------------------------------------------------------------------------
// Подтверждённые задачи: метка принятия, повтор `finish`, инвариант «Finished на диске до загрузки».
// ---------------------------------------------------------------------------------------------------------

const MIB: usize = 1024 * 1024;

/// Берёт задачу у сервера «руками», как это сделал бы умерший раннер.
fn claim_by_hand(stand: &Stand) -> Assignment {
    link(stand)
        .claim(0)
        .unwrap()
        .expect("задача должна быть выдана")
}

fn link(stand: &Stand) -> Link {
    Link::new(&stand.base, Token::new(stand.token.clone())).unwrap()
}

/// Состояние «исполнитель отдал результат, загрузки ещё не было»; папка названа так, как её называет `run_job`.
fn finished_state(stand: &Stand, a: &Assignment, bytes: &[u8]) -> HeldJob {
    let dir = stand.runner_dir();
    let out_dir = dir.join("out").join(format!("srv{}_s1mple", a.job_id));
    fs::create_dir_all(&out_dir).unwrap();
    fs::create_dir_all(dir.join("remote")).unwrap();
    let path = out_dir.join("highlights.mp4");
    fs::write(&path, bytes).unwrap();
    fs::write(out_dir.join("take1.mp4"), b"raw take").unwrap();
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
    fs::create_dir_all(stand.runner_dir()).unwrap();
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

/// Закрывает задачу на сервере итогом `Done` с размером и SHA-256 всего файла.
fn finish_done(link: &Link, id: i64, epoch: i64, bytes: &[u8]) {
    let reply = link
        .finish(
            id,
            &FinishRequest {
                epoch,
                outcome: Outcome::Done {
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                },
            },
        )
        .unwrap();
    assert_eq!(reply.state, JobState::Done);
}

/// Результат загружен и принят «руками»: куски не больше `CHUNK_SIZE`, затем `finish`.
fn accept_by_hand(stand: &Stand, id: i64, epoch: i64, bytes: &[u8]) {
    let link = link(stand);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + wire::CHUNK_SIZE as usize).min(bytes.len());
        offset = link
            .result_put(id, epoch, offset as u64, &bytes[offset..end])
            .unwrap() as usize;
    }
    finish_done(&link, id, epoch, bytes);
    assert_eq!(stand.job(id).state, JobState::Done);
}

#[test]
fn confirmed_takes_are_swept_at_once_even_when_fresh() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    put(&dir.join("out/srv31_s1mple/take1.mp4"), Duration::ZERO);
    put(&dir.join("remote/srv31.dem"), Duration::ZERO);
    put(&dir.join("out/srv31.accepted"), Duration::ZERO);
    put(&dir.join("out/srv32_s1mple/take1.mp4"), Duration::ZERO);

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(1)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    assert_eq!(runner.tick().unwrap(), Tick::Idle);

    assert!(
        !dir.join("out/srv31_s1mple").exists(),
        "подтверждённые дубли остались"
    );
    assert!(
        !dir.join("remote/srv31.dem").exists(),
        "демка подтверждённой задачи осталась"
    );
    assert!(!dir.join("out/srv31.accepted").exists(), "метка осталась");
    assert!(
        dir.join("out/srv32_s1mple/take1.mp4").exists(),
        "свежая неподтверждённая папка удалена"
    );
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("уборка:") && l.contains("#31")),
        "в журнале нет итога уборки:\n{log}"
    );
}

#[test]
fn restart_after_the_server_accepted_the_result_removes_the_takes() {
    let stand = Stand::start();
    let id = stand.submit(&common::fake_demo(MIB), "s1mple");
    let a = claim_by_hand(&stand);
    assert_eq!(a.job_id, id);
    // Сервер принял результат, раннер упал, не получив ответа.
    let bytes = result_bytes(MIB);
    accept_by_hand(&stand, id, a.epoch, &bytes);
    write_state(&stand, Some(finished_state(&stand, &a, &bytes)));

    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    runner.tick().unwrap();

    let dir = stand.runner_dir();
    assert!(
        state_of(&stand).job.is_none(),
        "задача осталась в состоянии"
    );
    assert!(
        !dir.join(format!("out/srv{id}_s1mple")).exists(),
        "дубли подтверждённой задачи остались"
    );
    assert!(
        !dir.join(format!("out/srv{id}.accepted")).exists(),
        "метка осталась"
    );
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("сервер уже принял результат") && l.contains(&format!("#{id}"))),
        "в журнале нет подтверждения:\n{log}"
    );
    assert_eq!(exec.calls.load(Ordering::SeqCst), 0, "исполнитель вызван");
    assert_eq!(stand.job(id).state, JobState::Done);
}

/// Исполнитель-заглушка: пишет результат, ломает запись `runner_state.json` и отдаёт результат.
struct BreakStateWrites {
    dir: PathBuf,
    calls: AtomicUsize,
}

impl Executor for BreakStateWrites {
    fn run(&self, task: &Task, _p: &dyn Progress) -> Result<Produced, RunError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let out_dir = self
            .dir
            .join("out")
            .join(format!("srv{}_s1mple", task.job_id));
        fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        fs::write(&result, result_bytes(MIB)).unwrap();
        // Каталог с именем временного файла состояния: `RunnerState::save` не сможет его создать.
        fs::create_dir_all(self.dir.join("runner_state.json.tmp")).unwrap();
        Ok(Produced {
            out_dir,
            result,
            shots: 1,
            secs: 1.0,
        })
    }
}

#[test]
fn upload_never_starts_before_the_finished_state_is_on_disk() {
    let stand = Stand::start();
    let id = stand.submit(&common::fake_demo(MIB), "s1mple");
    let exec = Arc::new(BreakStateWrites {
        dir: stand.runner_dir(),
        calls: AtomicUsize::new(0),
    });
    let mut runner = Runner::new(stand.runner_config(), exec.clone()).unwrap();

    let err = runner
        .tick()
        .expect_err("запись состояния сломана, тик обязан вернуть ошибку");
    assert!(err.contains("runner_state.json"), "{err}");

    let epoch = stand.job(id).epoch;
    assert_eq!(
        link(&stand).result_head(id, epoch).unwrap(),
        0,
        "сервер получил байты до записи Finished"
    );
    let state = stand.job(id).state;
    assert!(
        state != JobState::Uploading && state != JobState::Done,
        "загрузка началась: {state:?}"
    );
    let result = stand
        .runner_dir()
        .join(format!("out/srv{id}_s1mple/highlights.mp4"));
    assert!(result.exists(), "файл результата удалён");
    assert_eq!(exec.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn runner_stopped_mid_upload_restarts_from_finished_and_settles_the_accepted_result() {
    let stand = Stand::start();
    let id = stand.submit(&common::fake_demo(MIB), "s1mple");
    let exec = ScriptedExecutor::new(stand.runner_dir(), vec![Scenario::Result(20 * MIB)]);

    let mut cfg = stand.runner_config();
    cfg.upload_chunk_limit = Some(1);
    let mut first = Runner::new(cfg, exec.clone()).unwrap();
    assert_eq!(first.tick().unwrap(), Tick::Stopped);
    drop(first);

    // Состояние написал сам раннер: этап `Finished` и запись о результате.
    let bytes = result_bytes(20 * MIB);
    let job = state_of(&stand)
        .job
        .expect("задача должна остаться в состоянии");
    assert_eq!(job.server_job_id, id);
    assert_eq!(job.stage, Stage::Finished);
    let result = job.result.clone().expect("записи о результате нет");
    assert_eq!(result.size, (20 * MIB) as u64);
    assert_eq!(result.sha256, sha256_hex(&bytes));
    let link = link(&stand);
    assert_eq!(link.result_head(id, job.epoch).unwrap(), (8 * MIB) as u64);
    assert_eq!(stand.job(id).state, JobState::Uploading);

    // Остаток загружен и задача закрыта «руками»: сервер принял результат, раннер ответа не получил.
    for from in [8 * MIB, 16 * MIB] {
        let to = (from + 8 * MIB).min(bytes.len());
        link.result_put(id, job.epoch, from as u64, &bytes[from..to])
            .unwrap();
    }
    finish_done(&link, id, job.epoch, &bytes);
    assert_eq!(stand.job(id).state, JobState::Done);

    let mut second = Runner::new(stand.runner_config(), exec.clone()).unwrap();
    second.tick().unwrap();

    let dir = stand.runner_dir();
    assert!(
        state_of(&stand).job.is_none(),
        "задача осталась в состоянии"
    );
    assert!(!dir.join(format!("out/srv{id}")).exists(), "дубли остались");
    assert!(
        !dir.join(format!("remote/srv{id}.dem")).exists(),
        "демка осталась"
    );
    assert!(
        !dir.join(format!("out/srv{id}.accepted")).exists(),
        "метка осталась"
    );
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("сервер уже принял результат") && l.contains(&format!("#{id}"))),
        "в журнале нет подтверждения:\n{log}"
    );
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        1,
        "исполнитель вызван заново"
    );
    assert_eq!(stand.job(id).state, JobState::Done);
}

/// Исполнитель-заглушка: сам загружает и закрывает результат на сервере, а раннеру отдаёт его как обычный.
struct AcceptBehindBack {
    dir: PathBuf,
    base: String,
    token: String,
}

impl Executor for AcceptBehindBack {
    fn run(&self, task: &Task, _p: &dyn Progress) -> Result<Produced, RunError> {
        let out_dir = self
            .dir
            .join("out")
            .join(format!("srv{}_s1mple", task.job_id));
        fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        let bytes = result_bytes(MIB);
        fs::write(&result, &bytes).unwrap();
        fs::write(out_dir.join("take1.mp4"), b"raw take").unwrap();
        let link = Link::new(&self.base, Token::new(self.token.clone())).unwrap();
        link.result_put(task.job_id, task.epoch, 0, &bytes).unwrap();
        finish_done(&link, task.job_id, task.epoch, &bytes);
        Ok(Produced {
            out_dir,
            result,
            shots: 1,
            secs: 1.0,
        })
    }
}

#[test]
fn result_accepted_behind_the_runners_back_is_settled_not_dropped() {
    let stand = Stand::start();
    let id = stand.submit(&common::fake_demo(MIB), "s1mple");
    let exec = Arc::new(AcceptBehindBack {
        dir: stand.runner_dir(),
        base: stand.base.clone(),
        token: stand.token.clone(),
    });
    let mut cfg = stand.runner_config();
    // Пульсы отключены: о закрытии раннер узнаёт только из отказа `HEAD`.
    cfg.heartbeat = Some(Duration::from_secs(3600));
    cfg.debounce = Duration::from_secs(3600);
    let mut runner = Runner::new(cfg, exec).unwrap();

    assert_eq!(
        runner.tick().unwrap(),
        Tick::Settled {
            job_id: id,
            state: JobState::Done
        }
    );

    let dir = stand.runner_dir();
    assert!(
        !dir.join(format!("out/srv{id}_s1mple")).exists(),
        "дубли остались"
    );
    assert!(
        !dir.join(format!("out/srv{id}.accepted")).exists(),
        "метка осталась"
    );
    assert!(state_of(&stand).job.is_none());
    let log = runner_log(&stand);
    assert!(log.contains("сервер уже принял результат"), "{log}");
}

#[test]
fn unconfirmed_dropped_job_keeps_its_takes_until_the_age_limit() {
    let stand = Stand::start();
    let dir = stand.runner_dir();
    // Задачу сервер не знает: на `hello` он отвечает `abandon`, повтор `finish` получает 404.
    let a = Assignment {
        job_id: 4242,
        epoch: 1,
        lease_until_ms: 0,
        demo: wire::DemoInfo::default(),
        faceit_link: None,
        player: wire::Player {
            steam64: None,
            name: "s1mple".to_owned(),
        },
        requested_by: String::new(),
        options: Default::default(),
    };
    write_state(&stand, Some(finished_state(&stand, &a, &result_bytes(MIB))));

    let exec = ScriptedExecutor::new(dir.clone(), vec![Scenario::Result(MIB)]);
    let mut runner = Runner::new(stand.runner_config(), exec).unwrap();
    runner.tick().unwrap();

    assert!(state_of(&stand).job.is_none());
    let log = runner_log(&stand);
    assert!(
        log.lines()
            .any(|l| l.contains("задача #4242: сервер велел бросить задачу")),
        "в журнале нет причины:\n{log}"
    );
    assert!(
        dir.join("out/srv4242_s1mple/take1.mp4").exists(),
        "неподтверждённые дубли удалены"
    );
    assert!(
        !dir.join("out/srv4242.accepted").exists(),
        "метка появилась без подтверждения сервера"
    );
}
