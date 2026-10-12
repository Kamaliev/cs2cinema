//! Тесты режима раннера: задача настоящего сервера проходит через поток очереди приложения карточкой.

use std::sync::Arc;

use pipeline::{Artifact, Stage, job::Outcome};
use store::{JobKind, JobOutcome, JobSource, JobStatus, NewJob, Store};
use wire::JobState;

use super::testkit::{
    JobFnCalls, Rig, RigOpts, Stand, TestDir, after_kill, fake_demo, faststart_mp4, inspect_runs, pipeline_factory, probing_host, wait_until,
};
use super::{card_label, external_ref};
use crate::{jobs, settings::Settings};

const MIB: usize = 1024 * 1024;

#[test]
fn server_job_becomes_a_card_and_its_video_reaches_the_server() {
    let stand = Stand::start();
    let id = stand.submit(&fake_demo(MIB), "s1mple");
    let tools = TestDir::new("tools");
    let host = probing_host();
    let runner_dir = stand.runner_dir();
    // вместо `run_job`: этап записи, папка задачи через artifact и годный ролик
    let factory = pipeline_factory(&host, &tools, runner_dir.clone(), move |_, spec, p| {
        p.stage(Stage::Record);
        let out_dir = runner_dir.join("out").join(format!("srv{}", spec.job_id));
        p.artifact(&Artifact::OutDir(out_dir.clone()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let result = out_dir.join("highlights.mp4");
        std::fs::write(&result, faststart_mp4()).unwrap();
        Ok(Outcome { out_dir, result, shots: 1, secs: 5.0 })
    });
    let local = jobs::runner(Arc::new(Settings::default), Arc::new(host.clone()));
    let mut rig = Rig::start(&stand, factory, local);

    let done = wait_until(20, || stand.job(id).state == JobState::Done);
    let card_log = rig.card_for(id).map(|c| c.log).unwrap_or_default();
    assert!(done, "за 20 с задача сервера не стала done: {:?}\nжурнал карточки:\n{card_log}", stand.job(id).state);

    let uploaded = std::fs::read(stand.results_dir(id).join("highlights.mp4")).expect("ролик принят сервером");
    assert_eq!(uploaded, faststart_mp4(), "байты результата на сервере совпадают с записанными");

    let cards: Vec<_> = rig.store.jobs().unwrap().into_iter().filter(|j| j.external_ref.is_some()).collect();
    assert_eq!(cards.len(), 1, "одна выдача — одна карточка");
    let card = &cards[0];
    assert_eq!(card.external_ref, Some(external_ref(id, stand.job(id).epoch)));
    assert_eq!(card.status, JobStatus::Done, "{}", card.log);
    assert_eq!(card.label, card_label(id));
    assert_eq!(card.label, format!("Задача с сервера #{id}"));
    assert_eq!(card.hero_name, "s1mple");
    assert!(card.log.contains(&format!("Задача с сервера #{id}")), "{}", card.log);
    assert!(card.log.contains("[Запись #1]"), "в журнале карточки нет этапа записи:\n{}", card.log);

    assert_eq!(inspect_runs(&host), 1, "чужая демка обязана пройти дочернюю предпроверку: {:?}", host.runs());
    assert_eq!(host.launches(), 0, "игра в этом тесте не запускалась");
    assert_eq!(rig.queue.view().paused, None, "удачная задача очередь не останавливает");
    assert!(!rig.hub.has_slots(), "после итога запись карточки в хабе закрыта");

    assert!(rig.stop(), "поток раннера завершился без паники");
}

#[test]
fn card_reference_and_label_are_built_from_numbers_only() {
    assert_eq!(external_ref(7, 3), "srv:7:3");
    assert_eq!(card_label(7), "Задача с сервера #7");
}

/// Исполнитель локальных задач: сразу «готово» без записи.
fn instant_local() -> crate::queue::Runner {
    Arc::new(|job, _| Ok(JobOutcome { out_dir: "out/local".into(), result: format!("out/local/{}.mp4", job.id).into(), shots: 1, secs: 1.0 }))
}

fn local_job() -> NewJob {
    NewJob {
        kind: JobKind::Record,
        source: JobSource::File("local.dem".into()),
        label: "своя".into(),
        hero_name: String::new(),
        hero_steam: 0,
        map: String::new(),
    }
}

#[test]
fn interrupted_server_job_resumes_after_restart_without_a_pause() {
    let stand = Stand::start();
    let tools = TestDir::new("tools");
    let db_dir = TestDir::new("db");
    let db = db_dir.path.join("cs2cinema.db");
    let runner_dir = stand.runner_dir();

    // приложение убито посреди записи: выдача у раннера, карточка запущена
    let dead = after_kill(&stand, &runner_dir, &db);
    assert_eq!(stand.job(dead.job_id).state, JobState::Assigned, "сервер считает задачу выданной раннеру");

    // новый запуск: уборка и сброс не ставят паузу
    let store = Store::open(&db).unwrap();
    let host = probing_host();
    let reset = jobs::recover_after_crash(&store, &host, &tools.path.join("tools/cs2").to_string_lossy()).unwrap();
    assert_eq!((reset.interrupted, reset.interrupted_remote), (0, 1), "карточка сервера считается отдельно: {reset:?}");
    let card = store.job(dead.card).unwrap().unwrap();
    assert_eq!(card.status, JobStatus::Interrupted);
    assert!(card.error.as_deref().is_some_and(|e| e.contains("продолжится сама")), "{:?}", card.error);

    let calls = JobFnCalls::default();
    let factory = {
        let (calls, runner_dir) = (calls.clone(), runner_dir.clone());
        pipeline_factory(&host, &tools, runner_dir, move |_, spec, p| {
            calls.record(spec);
            p.stage(Stage::Record);
            let out_dir = spec.out_dir.clone().expect("при продолжении папка известна");
            std::fs::create_dir_all(&out_dir).unwrap();
            let result = out_dir.join("highlights.mp4");
            std::fs::write(&result, faststart_mp4()).unwrap();
            Ok(Outcome { out_dir, result, shots: 1, secs: 5.0 })
        })
    };
    let mut rig = Rig::start_with(&stand, factory, RigOpts { store: Some(store.clone()), paused: None, local: Some(instant_local()) });

    let mut paused_seen = false;
    let done = wait_until(20, || {
        paused_seen |= rig.queue.view().paused.is_some();
        stand.job(dead.job_id).state == JobState::Done
    });
    let log = store.job(dead.card).unwrap().unwrap().log;
    assert!(done, "за 20 с задача на сервере не стала done: {:?}\nжурнал карточки:\n{log}", stand.job(dead.job_id).state);
    assert!(!paused_seen, "очередь не должна вставать на паузу из-за прерванной задачи сервера");

    let uploaded = std::fs::read(stand.results_dir(dead.job_id).join("highlights.mp4")).expect("ролик принят сервером");
    assert_eq!(uploaded, faststart_mp4());
    assert!(wait_until(5, || store.job(dead.card).unwrap().unwrap().status == JobStatus::Done), "карточка не стала Done:\n{log}");
    let card = store.job(dead.card).unwrap().unwrap();
    assert!(card.log.contains("продолжение после перезапуска"), "{}", card.log);
    assert_eq!(store.jobs().unwrap().iter().filter(|j| j.external_ref.is_some()).count(), 1, "карточка усыновлена, а не создана заново");

    assert_eq!(calls.count(), 1, "исполнитель вызван ровно один раз");
    assert_eq!(calls.resumes(), vec![true], "и с продолжением по record.json");
    assert_eq!(calls.out_dirs(), vec![Some(dead.out_dir.clone())], "по папке, которую раннер сохранил");
    assert_eq!(inspect_runs(&host), 0, "повторной предпроверки демки при продолжении нет");

    // очередь жива: следующая локальная задача доходит до Done
    let local = store.add_job(&local_job()).unwrap();
    rig.queue.resume();
    assert!(wait_until(10, || store.job(local).unwrap().unwrap().status == JobStatus::Done), "локальная задача после продолжения не дошла до Done");
    assert_eq!(rig.queue.view().paused, None);
    assert!(rig.stop(), "поток раннера завершился без паники");
}
