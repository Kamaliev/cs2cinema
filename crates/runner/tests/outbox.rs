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
