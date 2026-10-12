//! Тесты режима раннера: задача настоящего сервера проходит через поток очереди приложения карточкой.

use std::sync::Arc;

use pipeline::{Artifact, Stage, job::Outcome};
use store::JobStatus;
use wire::JobState;

use super::testkit::{Rig, Stand, TestDir, fake_demo, faststart_mp4, inspect_runs, pipeline_factory, probing_host, wait_until};
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
