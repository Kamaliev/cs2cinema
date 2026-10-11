//! Выполнение задач на монтаж: очередь из базы, по одной (CS2 пишет только одну), с отменой.
//!
//! Задача самодостаточна: откуда демка и про кого ролик она знает сама, поэтому разбор
//! делается заново в фоновом потоке — и задачу можно повторить после перезапуска.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use pipeline::{
    Artifact, PlanOptions, Progress, RecordFailure, RecordState, RunError, Source, Stage,
    assemble::assemble_stage,
    host::Host,
    job::{self, DemoSource, JobConfig, JobSpec},
    precheck::{Demo, Inputs, space},
};
use store::{
    Job, JobFailure, JobKind, JobOutcome, JobSource, Store,
    journal::{self, StartKind},
};

use crate::{
    app::{App, Msg, Reporter},
    launch,
    settings::Settings,
    ui,
};

/// Токен отмены: кнопка в окне ставит флаг, исполнитель проверяет его между этапами
/// и во всех ожиданиях (загрузка игры, запись, ffmpeg).
#[derive(Clone, Default)]
pub struct CancelToken {
    cancel: Arc<AtomicBool>,
    /// Второе нажатие (D-12): закрыть свой job сразу, без `quit` и ожидания.
    force: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    /// «Закрыть игру сразу»: подразумевает обычную отмену.
    pub fn force(&self) {
        self.cancel();
        self.force.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    pub fn is_forced(&self) -> bool {
        self.force.load(Ordering::SeqCst)
    }
}

/// Отчёт задачи: в базу (переживает перезапуск) и в строку состояния окна.
struct JobReporter {
    store: Store,
    id: i64,
    ui: Reporter,
    cancel: CancelToken,
    /// Номер попытки — в метке каждой строки журнала.
    attempt: u32,
    /// Текущий этап для метки строк журнала и время входа в него — по нему считается длительность (OBS-01, D-11).
    cur: Mutex<Option<(String, Instant)>>,
}

impl JobReporter {
    fn new(store: Store, id: i64, ui: Reporter, cancel: CancelToken, attempt: u32) -> Self {
        JobReporter { store, id, ui, cancel, attempt, cur: Mutex::new(None) }
    }

    /// Строка в журнал задачи в базе: время, метка `[этап #попытка]` и текст.
    fn write(&self, label: &str, text: &str) {
        let line = format!("{}  {} {text}", chrono::Local::now().format("%H:%M:%S"), journal::prefix(label, self.attempt));
        let _ = self.store.append_log(self.id, &line);
    }

    /// Перейти на этап `label`: длительность предыдущего этапа пишется в журнал. Тот же этап — ничего не меняет.
    fn switch(&self, label: &str) {
        let Ok(mut cur) = self.cur.lock() else { return };
        match cur.as_ref() {
            Some((prev, _)) if prev == label => return,
            Some((prev, since)) => self.write(prev, &journal::stage_line(prev, since.elapsed())),
            None => {}
        }
        *cur = Some((label.to_owned(), Instant::now()));
    }

    /// Закрыть текущий этап: записать его длительность. Повторный вызов ничего не пишет.
    pub fn close_stage(&self) {
        let Ok(mut cur) = self.cur.lock() else { return };
        if let Some((prev, since)) = cur.take() {
            self.write(&prev, &journal::stage_line(&prev, since.elapsed()));
        }
    }
}

impl Progress for JobReporter {
    fn stage(&self, s: Stage) {
        self.switch(s.title());
        let _ = self.store.set_stage(self.id, s.title());
        self.ui.send(Msg::Stage(s));
    }
    fn log(&self, m: &str) {
        let label = self.cur.lock().ok().and_then(|c| c.as_ref().map(|(l, _)| l.clone())).unwrap_or_else(|| "Старт".to_owned());
        self.write(&label, m);
        // в окно уходит сырой текст: status.rs разбирает его префиксы
        self.ui.send(Msg::Log(m.to_owned()));
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    fn force_cancelled(&self) -> bool {
        self.cancel.is_forced()
    }
    /// Состояние записи — в строку задачи (для лестницы в карточке), любое событие — в журнал.
    /// Подсостояние не пишем в `stage`: полоска прогресса сопоставляет `stage` с `Stage::ALL`.
    fn event(&self, e: &pipeline::Event) {
        if let pipeline::Event::State { state, .. } = e {
            let _ = self.store.set_phase(self.id, state.code());
            if *state == RecordState::Precheck {
                self.switch("Предпроверка");
            }
        }
        self.log(&e.to_string());
    }
    /// Итоги шагов конвейера — в строки задачи и в базу демок: `pipeline` сам базы не знает.
    fn artifact(&self, a: &Artifact) {
        match a {
            Artifact::Demo { match_id, path } => {
                let _ = self.store.remember_demo(match_id, path);
            }
            Artifact::OutDir(dir) => {
                let _ = self.store.set_out_dir(self.id, dir);
            }
            Artifact::Precheck { json, passed } => {
                let _ = self.store.set_precheck(self.id, json);
                // лестница прячется до начала записи
                if *passed {
                    let _ = self.store.set_phase(self.id, "");
                }
            }
            Artifact::LastGood { cs2_build, hlae_version } => {
                let _ = self.store.set_last_good(cs2_build, hlae_version);
            }
        }
    }
}

/// Сбой записи строками для базы: store не знает типов pipeline.
pub fn job_failure(f: &RecordFailure) -> JobFailure {
    JobFailure {
        message: f.message.clone(),
        code: f.code.as_str().to_owned(),
        state: f.state.code().to_owned(),
        hint: f.hint.clone(),
        tail: f.console_tail.join("\n"),
        elapsed_ms: Some(f.elapsed.as_millis() as i64),
    }
}

/// Итог попытки одной строкой для журнала: «готово», «отменена», «сбой КОД · состояние · этап X — сообщение».
fn outcome_text(result: &Result<JobOutcome, RunError>, stage: &str) -> String {
    let stage = if stage.is_empty() { "Старт" } else { stage };
    let short = |m: &str| m.chars().take(200).collect::<String>();
    match result {
        Ok(_) => "готово".to_owned(),
        Err(e) if e.message == pipeline::CANCELLED => "отменена".to_owned(),
        Err(RunError { fail: Some(f), .. }) => format!("сбой {} · {} · этап {stage} — {}", f.code.as_str(), f.state.title(), short(&f.message)),
        Err(e) => format!("ошибка · этап {stage} — {}", short(&e.message)),
    }
}

/// Итог задачи в строку базы: отмена остаётся отменой, типизированный сбой — со всеми полями.
/// Итог попытки пишется в журнал раньше, чем статус (RET-01, D-10).
pub fn finish(store: &Store, id: i64, result: &Result<JobOutcome, RunError>) -> store::Result<()> {
    let (attempt, stage) = store.job(id).ok().flatten().map(|j| (j.attempt, j.stage)).unwrap_or_default();
    let _ = store.append_log(id, &journal::outcome_line(attempt, &outcome_text(result, &stage)));
    match result {
        Ok(o) => store.finish_job(id, Ok(o)),
        Err(e) if e.message == pipeline::CANCELLED => store.cancel_job(id),
        Err(RunError { fail: Some(f), .. }) => store.fail_job(id, &job_failure(f)),
        Err(e) => store.finish_job(id, Err(&e.message)),
    }
}

/// Нужна ли задаче предпроверка окружения: «только план» ничего не пишет, поэтому её пропускает (D-01).
pub fn needs_precheck(kind: JobKind) -> bool {
    kind != JobKind::Plan
}

/// Настройки приложения в настройки задачи `pipeline`: пути, кадр и остальное поле в поле.
pub fn job_config(s: &Settings) -> JobConfig {
    JobConfig {
        hlae_dir: PathBuf::from(&s.hlae_dir),
        cs2_dir: PathBuf::from(&s.cs2_dir),
        out_root: PathBuf::from(&s.out_root),
        faceit_key: s.faceit_key.clone(),
        fps: s.fps,
        chronological: s.chronological,
        flybys: s.flybys,
        walls: s.walls,
        detailed: s.detailed,
        gpu_encode: s.gpu_encode,
        frame: s.frame_size(),
        time_margin: s.time_margin,
        afxerror_ignore: s.afxerror_ignore.clone(),
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
    }
}

/// Задача из базы в описание для `pipeline::job`: всё, что приложение помнит (скачанная демка, пара последней
/// удачной записи, папка задачи), приходит сюда, а не берётся из базы внутри `pipeline`.
pub fn job_spec(job: &Job, store: &Store, s: &Settings, attempt: u32) -> JobSpec {
    let (source, known_demo) = match &job.source {
        JobSource::File(path) => (DemoSource::File(path.clone()), None),
        JobSource::Faceit(id) => (DemoSource::Faceit(id.clone()), store.demo_path(id).ok().flatten()),
    };
    JobSpec {
        job_id: job.id,
        attempt,
        record: job.kind != JobKind::Plan,
        resume: job.resume,
        source,
        hero_steam: job.hero_steam,
        hero_name: job.hero_name.clone(),
        known_demo,
        last_good: store.last_good().ok().flatten().map(|g| (g.cs2_build, g.hlae_version)),
        out_dir: job.out_dir.clone(),
        cfg: job_config(s),
    }
}

/// Что проверять: пути из настроек, дедлайны с «Запасом по времени», пара последней удачной записи.
/// Один построитель для задачи и для кнопки в настройках (D-01).
pub fn precheck_inputs(s: &Settings, demo: Demo, last_good: Option<store::LastGood>) -> Inputs {
    job::precheck_inputs(&job_config(s), demo, last_good.map(|g| (g.cs2_build, g.hlae_version)))
}

/// Демка задачи для предпроверки: файл, готовая скачанная копия из реестра или «ещё скачать».
fn job_demo(job: &Job, store: &Store) -> Demo {
    match &job.source {
        JobSource::File(p) => Demo::File(p.clone()),
        JobSource::Faceit(id) => match store.demo_path(id) {
            Ok(Some(path)) => Demo::File(path),
            _ => Demo::Download,
        },
    }
}

/// Останавливает ли итог задачи очередь: только осознанная отмена (D-13); сбой и успех — очередь идёт дальше.
pub fn pauses_queue(result: &Result<PathBuf, String>) -> bool {
    matches!(result, Err(e) if e == pipeline::CANCELLED)
}

/// Запускает следующую задачу из очереди, если сейчас ничего не выполняется и очередь не на паузе.
pub fn run_next(app: &mut App) {
    if app.jobs.running.is_some() || app.status.busy || app.jobs.paused {
        return;
    }
    let Ok(Some(job)) = app.store.next_queued() else {
        return;
    };
    let attempt = match app.store.start_job(job.id) {
        Ok(n) => n,
        Err(e) => {
            app.status.finish(Err(e));
            return;
        }
    };
    // журнал не стирается: каждая попытка открывается разделителем с номером, датой и способом запуска (D-10)
    let kind = if job.resume {
        StartKind::Retry
    } else if attempt <= 1 {
        StartKind::First
    } else {
        StartKind::Redo
    };
    let _ = app.store.append_log(job.id, &journal::separator(attempt, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), kind));
    let cancel = CancelToken::default();
    app.jobs.running = Some((job.id, cancel.clone()));
    app.status.start(&format!("Задача #{}: {} · {}", job.id, job.label, job.kind.title()));
    let (store, s, ui) = (app.store.clone(), app.s.clone(), app.reporter());
    std::thread::spawn(move || {
        let host = pipeline::host::default_host();
        let rep = JobReporter::new(store.clone(), job.id, ui, cancel, attempt);
        let result = run(&*host, &job, &s, &store, &rep);
        // длительность последнего этапа — до итога попытки (успех, сбой и отмена)
        rep.close_stage();
        let _ = finish(&store, job.id, &result);
        // готовый ролик открываем в плеере здесь, а не в `run`: тесты `run` плеер не запускают
        if let Ok(o) = &result {
            if o.result.extension().is_some_and(|e| e == "mp4") {
                ui::open_path(&o.result);
            }
        }
        rep.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result).map_err(|e| e.message) });
    });
    app.refresh_jobs();
}

/// Весь путь задачи: демка -> план -> (запись -> склейка); исполняется в фоновом потоке.
fn run(host: &dyn Host, job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
    // «Повторить»: всё берётся из record.json в папке задачи, без скачивания, разбора и плана (RET-02, D-08)
    if job.resume {
        return job::resume(host, &job_spec(job, store, s, p.attempt), p).map(store_outcome);
    }
    let cfg = job_config(s);
    // предпроверка окружения — до скачивания, разбора и плана (PRE-02): неисправное окружение видно за секунды
    let inputs = precheck_inputs(s, job_demo(job, store), store.last_good().ok().flatten());
    let mut report = if needs_precheck(job.kind) { Some(job::precheck_stage(host, &inputs, p)?) } else { None };
    let src = match &job.source {
        // скачанную демку берём из реестра, иначе качаем
        JobSource::Faceit(id) => match store.demo_path(id)? {
            Some(path) => Source::File(path),
            None => Source::Faceit { link: id.clone(), key: s.faceit_key.clone(), demo_index: 0, dir: Path::new(&s.out_root).join("demos") },
        },
        JobSource::File(path) => Source::File(path.clone()),
    };
    let mut a = pipeline::analyze(&src, p)?;
    if let JobSource::Faceit(id) = &job.source {
        let _ = store.remember_demo(id, &a.demo_path);
    }
    let hero = pipeline::find_player(&a.m, Some(job.hero_steam), &job.hero_name).ok_or_else(|| format!("игрок {} в демке не найден", job.hero_name))?;
    let out_dir = Path::new(&s.out_root).join(pipeline::console_name(&format!("{}_{}", a.label, a.m.name(hero))));
    // папка задачи известна карточке до записи: путь к record.log виден и при сбое
    let _ = store.set_out_dir(job.id, &out_dir);
    // NVENC: для склейки — у ffmpeg из PATH, для дублей — у ffmpeg, которым пишет HLAE
    let (mut nvenc, mut take_nvenc) = (false, false);
    if s.gpu_encode && job.kind != JobKind::Plan {
        // проба с дедлайном и отменой задачи (TMO-03): зависший ffmpeg не вешает задачу
        let probe_for = job::deadlines(&cfg).nvenc_probe;
        let cancel = || p.cancelled();
        let check = launch::nvenc_check(host, Path::new("ffmpeg"), probe_for, &cancel);
        take_nvenc = launch::hlae_ffmpeg(&s.hlae_dir).is_some_and(|f| launch::has_nvenc(host, &f, probe_for, &cancel));
        // отмена во время пробы прерывает задачу, а не превращается в «NVENC недоступен»
        pipeline::check(p)?;
        nvenc = check.is_ok();
        p.log(&format!("кодирование: дубли — {}, склейка — {}", if take_nvenc { "NVENC" } else { "процессор" }, if nvenc { "NVENC" } else { "процессор" }));
        if let Err(e) = check {
            p.log(&format!("NVENC недоступен: {e}"));
        }
    }
    let o = PlanOptions {
        fps: s.fps,
        chronological: s.chronological,
        flybys: s.flybys,
        walls: s.walls,
        size: s.frame_size(),
        encoder: if nvenc { renderer::Encoder::Nvenc } else { renderer::Encoder::X264 },
        take_nvenc,
        detailed: s.detailed,
        ..Default::default()
    };
    // record.json прошлого плана устарел, как только скрипты пишутся заново: «Повторить» не должен взять чужой план (Pitfall 7)
    job::begin_plan(&out_dir)?;
    let plan = pipeline::plan_player(&mut a, hero, &o, &out_dir, p)?;
    let (shots, secs) = (plan.timeline.shots.len() as u32, plan.timeline.total_secs());
    p.log(&format!("план: {shots} шотов, ~{secs:.0} с"));
    if job.kind == JobKind::Plan {
        return Ok(JobOutcome { out_dir: out_dir.clone(), result: out_dir, shots, secs });
    }
    pipeline::check(p)?;
    // пути HLAE и CS2 проверила предпроверка; путь результатов должен годиться для консоли HLAE
    if !renderer::console_safe(&pipeline::absolute_slash(&out_dir)) {
        return Err("в пути результатов есть пробелы — HLAE не загрузит campath; выберите папку без пробелов".into());
    }

    // скрипты плана готовы: всё, что нужно записи и склейке, — в `RecordJson` (повтор берёт его же, D-08)
    let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
    let rj = job::record_json_for(&plan, &a.demo_path, a.demo_len(), &demo_name, &cfg, take_nvenc)?;
    // без record.json «Повторить» невозможен, поэтому ошибка сохранения — ошибка задачи; файл ложится сразу после плана
    rj.save(&out_dir).map_err(|e| format!("не удалось сохранить record.json: {e} — без него «Повторить» невозможен"))?;
    // второй проход места по плану (D-03): сумма игровых секунд дублей, размер кадра и демки; до запуска игры
    if let Some(report) = report.as_mut() {
        job::space_pass(host, &inputs, space::estimate(&plan.takes, s.frame_size(), a.demo_len()), report, p)?;
    }
    // первый запуск и «Заново» всегда пишут дубли с проверкой, прежние не переиспользуют (решение 2)
    job::record_takes(host, &cfg, (job.id, p.attempt), &out_dir, &rj, report.as_ref(), p)?;

    p.stage(Stage::Assemble);
    let out = assemble_stage(host, &out_dir, &plan.takes, &plan.ffmpeg_args, plan.video_secs, job::deadlines(&cfg).ffmpeg_for(plan.video_secs), p)?;
    // пара «сборка CS2 / версия HLAE» после удачной задачи, в которой шла запись (D-04, DIF-01)
    job::last_good(report.as_ref(), p);
    Ok(JobOutcome { out_dir, result: out, shots, secs })
}

/// Итог общей задачи `pipeline` в строку базы: поля перекладываются как есть.
fn store_outcome(o: job::Outcome) -> JobOutcome {
    JobOutcome { out_dir: o.out_dir, result: o.result, shots: o.shots, secs: o.secs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use pipeline::{Deadlines, RecordRequest, host::LaunchSpec};

    #[test]
    fn force_implies_cancel_and_is_seen_by_the_reporter() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled() && !token.is_forced());
        token.cancel();
        assert!(token.is_cancelled() && !token.is_forced(), "обычная отмена не закрывает сразу");
        let token = CancelToken::default();
        let rep = JobReporter::new(Store::in_memory().unwrap(), 1, Reporter::detached(), token.clone(), 1);
        assert!(!rep.force_cancelled());
        token.force();
        assert!(token.is_cancelled(), "force включает и отмену");
        assert!(rep.cancelled() && rep.force_cancelled());
    }

    #[test]
    fn only_a_cancelled_job_pauses_the_queue() {
        assert!(pauses_queue(&Err(pipeline::CANCELLED.to_owned())));
        assert!(!pauses_queue(&Err("сбой записи".to_owned())));
        assert!(!pauses_queue(&Ok(PathBuf::from("highlights.mp4"))));
    }

    use pipeline::host::fake::{FakeHost, Scenario};
    use renderer::TakePlan;
    use store::JobStatus;

    fn new_job() -> store::NewJob {
        store::NewJob {
            kind: JobKind::Record,
            source: JobSource::Faceit("1-abc".into()),
            label: "матч".into(),
            hero_name: "me".into(),
            hero_steam: 42,
            map: "de_nuke".into(),
        }
    }

    fn two_takes() -> Vec<TakePlan> {
        (0..2).map(|index| TakePlan { index, shot: index as usize, game_secs: 5.0, capture_fps: 60 }).collect()
    }

    /// Задача в базе, запущенная и с папкой результата, как её оставляет `run` перед записью.
    fn running_job(tag: &str) -> (Store, i64, std::path::PathBuf) {
        let store = Store::in_memory().unwrap();
        let id = store.add_job(&new_job()).unwrap();
        store.start_job(id).unwrap();
        let dir = std::env::temp_dir().join(format!("cs2cinema-app-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store.set_out_dir(id, &dir).unwrap();
        (store, id, dir)
    }

    fn request(dir: &Path) -> RecordRequest {
        RecordRequest {
            out_dir: dir.to_path_buf(),
            launch: LaunchSpec { nonce: "ab12cd34".into(), ..Default::default() },
            takes: two_takes(),
            deadlines: Deadlines::default(),
            header: "задача #1 · тест".into(),
            afxerror_ignore: Vec::new(),
            verify: None,
        }
    }

    /// Отчёт, который отменён с самого начала.
    struct Cancelled;
    impl Progress for Cancelled {
        fn stage(&self, _: Stage) {}
        fn log(&self, _: &str) {}
        fn cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn typed_failure_reaches_the_job_row() {
        let (store, id, dir) = running_job("fail");
        let host = FakeHost::new(Scenario::happy(2).console_never_up());
        let e = job::record_stage(&host, &request(&dir), &pipeline::Quiet).expect_err("консоль не открывается — запись падает");
        finish(&store, id, &Err(e)).unwrap();

        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.fail_code.as_deref(), Some("CONSOLE_NOT_OPEN"));
        assert_eq!(j.fail_state.as_deref(), Some("console"));
        let error = j.error.unwrap();
        assert!(error.contains("консольный порт не открылся"), "{error}");
        assert!(error.contains("cs2.exe:"), "в сообщении строка фактов о двух процессах: {error}");
        assert!(j.fail_hint.is_some_and(|h| !h.is_empty()));
        assert!(j.fail_tail.is_some(), "хвост консоли записан (может быть пустым)");
        assert!(j.fail_elapsed_ms.is_some_and(|ms| ms >= 90_000), "{:?}", j.fail_elapsed_ms);
        assert_eq!(j.out_dir.as_deref(), Some(dir.as_path()));
        let log = std::fs::read_to_string(dir.join("record.log")).expect("record.log лежит в папке задачи");
        assert!(log.contains("CONSOLE_NOT_OPEN"), "{log}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancelled_recording_becomes_a_cancelled_job() {
        let (store, id, dir) = running_job("cancel");
        let host = FakeHost::new(Scenario::happy(2));
        let e = job::record_stage(&host, &request(&dir), &Cancelled).expect_err("отмена прерывает запись");
        assert_eq!(e.message, pipeline::CANCELLED);
        assert!(e.fail.is_none(), "отмена не несёт типизированного сбоя");
        finish(&store, id, &Err(e)).unwrap();

        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Cancelled);
        assert_eq!(j.fail_code, None);
        assert!(host.terminated() > 0, "игра закрыта и при отмене");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn finished_attempt_leaves_an_outcome_line_and_the_next_one_a_separator() {
        let (store, id, dir) = running_job("outcome");
        store.append_log(id, "строка первой попытки").unwrap();
        store.set_stage(id, "Запись").unwrap();
        let host = FakeHost::new(Scenario::happy(2).console_never_up());
        let e = job::record_stage(&host, &request(&dir), &pipeline::Quiet).expect_err("консоль не открывается — запись падает");
        finish(&store, id, &Err(e)).unwrap();

        store.requeue(id).unwrap();
        let attempt = store.start_job(id).unwrap();
        assert_eq!(attempt, 2);
        store.append_log(id, &journal::separator(attempt, "2026-10-11 10:30", StartKind::Redo)).unwrap();
        let log = store.job(id).unwrap().unwrap().log;
        assert!(log.contains("строка первой попытки"), "{log}");
        assert!(log.contains("итог попытки 1: сбой CONSOLE_NOT_OPEN"), "{log}");
        assert!(log.contains("этап Запись"), "{log}");
        assert!(log.contains("──── попытка 2"), "{log}");
        assert!(log.find("итог попытки 1").unwrap() < log.find("──── попытка 2").unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn journal_lines_carry_stage_and_attempt_and_stage_durations() {
        let (store, id, _dir) = running_job("journal");
        let rep = JobReporter::new(store.clone(), id, Reporter::detached(), CancelToken::default(), 2);
        rep.log("а");
        rep.event(&pipeline::Event::State { state: RecordState::Precheck, take: None });
        rep.log("б");
        rep.stage(Stage::Download);
        rep.log("в");
        rep.stage(Stage::Parse);
        rep.close_stage();
        rep.close_stage();

        let log = store.job(id).unwrap().unwrap().log;
        assert!(log.contains("[Старт #2] а"), "{log}");
        assert!(log.contains("[Предпроверка #2] б"), "{log}");
        assert!(log.contains("[Скачивание #2] в"), "{log}");
        assert_eq!(log.matches("⏱ этап Разбор").count(), 1, "{log}");
        assert_eq!(log.matches("⏱ этап Скачивание").count(), 1, "{log}");
        let names: Vec<String> = journal::stage_summary(&log).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Предпроверка", "Скачивание", "Разбор"]);
    }

    #[test]
    fn plain_error_keeps_the_old_failed_row() {
        let (store, id, _dir) = running_job("plain");
        finish(&store, id, &Err(RunError::from("игрок не найден"))).unwrap();
        let j = store.job(id).unwrap().unwrap();
        assert_eq!((j.status, j.error.as_deref(), j.fail_code), (JobStatus::Failed, Some("игрок не найден"), None));
    }

    #[test]
    fn plan_job_skips_the_precheck() {
        assert!(!needs_precheck(JobKind::Plan));
        assert!(needs_precheck(JobKind::Record));
    }

    #[test]
    fn job_reporter_turns_artifacts_into_store_rows() {
        let (store, id, dir) = running_job("artifacts");
        let rep = reporter_for(&store, id);
        rep.event(&pipeline::Event::State { state: RecordState::Precheck, take: None });
        assert_eq!(store.job(id).unwrap().unwrap().phase, "precheck");

        // реестр отдаёт только существующие файлы
        let demo = std::env::temp_dir().join(format!("cs2cinema-artifact-demo-{}.dem", std::process::id()));
        std::fs::write(&demo, b"PBDEMS2\0").unwrap();
        rep.artifact(&Artifact::Demo { match_id: "1-abc".into(), path: demo.clone() });
        assert_eq!(store.demo_path("1-abc").unwrap(), Some(demo.clone()));
        let _ = std::fs::remove_file(&demo);

        let other = std::env::temp_dir().join("cs2cinema-artifact-out");
        rep.artifact(&Artifact::OutDir(other.clone()));
        assert_eq!(store.job(id).unwrap().unwrap().out_dir.as_deref(), Some(other.as_path()));

        // сбой предпроверки: отчёт в базе, лестница не спрятана
        rep.artifact(&Artifact::Precheck { json: "{\"items\":[]}".into(), passed: false });
        let j = store.job(id).unwrap().unwrap();
        assert_eq!((j.precheck.as_deref(), j.phase.as_str()), (Some("{\"items\":[]}"), "precheck"));

        // успех: отчёт остаётся, лестница прячется до начала записи
        rep.artifact(&Artifact::Precheck { json: "{\"items\":[1]}".into(), passed: true });
        let j = store.job(id).unwrap().unwrap();
        assert_eq!((j.precheck.as_deref(), j.phase.as_str()), (Some("{\"items\":[1]}"), ""));

        rep.artifact(&Artifact::LastGood { cs2_build: "25815307".into(), hlae_version: "2.192.7".into() });
        let g = store.last_good().unwrap().expect("пара запомнена");
        assert_eq!((g.cs2_build.as_str(), g.hlae_version.as_str()), ("25815307", "2.192.7"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn reporter_for(store: &Store, id: i64) -> JobReporter {
        JobReporter::new(store.clone(), id, Reporter::detached(), CancelToken::default(), 1)
    }

    #[test]
    fn nvenc_probe_that_hangs_is_cut_by_its_deadline() {
        let host = FakeHost::new(Scenario::happy(1).run_result(Err(pipeline::host::HostError::Timeout)));
        let e = launch::nvenc_check(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| false).unwrap_err();
        assert!(e.contains("не уложилась"), "{e}");
        assert!(e.contains("20 с"), "{e}");
        assert!(!launch::has_nvenc(&host, Path::new("ffmpeg"), Duration::from_secs(20), &|| false));
    }
}
