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
    manifest::{Manifest, RecordJson, TakeStatus},
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
        return resume_job(host, job, s, store, p);
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

/// «Повторить» (RET-02, RET-03, D-08): без скачивания, разбора демки и плана — по `record.json` из папки задачи.
/// Все дубли проверены — сразу склейка, без предпроверки и без игры (RET-03).
fn resume_job(host: &dyn Host, job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
    let cfg = job_config(s);
    let out_dir = job.out_dir.clone().ok_or_else(|| job::retry_no_data("папка задачи неизвестна"))?;
    let _ = store.set_out_dir(job.id, &out_dir);
    let rj = RecordJson::load(&out_dir).map_err(|e| job::retry_no_data(&e))?;
    if job::plan_hash(&out_dir, &rj.nonce) != rj.plan_hash {
        return Err(job::retry_no_data("скрипты записи в папке задачи изменились"));
    }
    let takes = rj.take_plans();
    let clips = out_dir.join("clips");
    let mut manifest = Manifest::load(&clips).filter(|m| m.plan_hash == rj.plan_hash);
    if let Some(m) = manifest.as_mut() {
        for line in m.reconcile(&clips) {
            p.log(&line);
        }
    }
    let outcome = |out: PathBuf| JobOutcome { out_dir: out_dir.clone(), result: out, shots: rj.shots as u32, secs: rj.secs };
    if manifest.as_ref().is_some_and(|m| job::all_takes_validated(m, &takes)) {
        p.log("все дубли проверены по манифесту — сразу склейка, без предпроверки и игры");
        p.stage(Stage::Assemble);
        let out = assemble_stage(host, &out_dir, &takes, &rj.ffmpeg_args, rj.video_secs, job::deadlines(&cfg).ffmpeg_for(rj.video_secs), p)?;
        return Ok(outcome(out));
    }
    // не все дубли проверены: HLAE нумерует новые дубли «наибольший + 1» и не заполняет пропуски, поэтому
    // дописать только недостающие нельзя — записываем все заново, прежние уходят в `clips_old_<ts>` (D-07, RET-06)
    let not_validated = match &manifest {
        None => "манифест дублей не найден".to_owned(),
        Some(m) => takes
            .iter()
            .filter(|t| m.entry(t.index).is_none_or(|e| e.status != TakeStatus::Validated))
            .map(|t| (t.index + 1).to_string())
            .collect::<Vec<_>>()
            .join(", "),
    };
    p.log(&format!("дубли не все проверены: {not_validated} — записываю все заново (новые дубли HLAE нумерует подряд)"));
    // демка — из record.json, не из скачивания: предпроверка скажет, если её нет (PRECHECK_FAILED)
    let inputs = precheck_inputs(s, Demo::File(rj.demo_path.clone()), store.last_good().ok().flatten());
    let mut report = job::precheck_stage(host, &inputs, p)?;
    job::space_pass(host, &inputs, space::estimate(&takes, rj.frame, rj.demo_len), &mut report, p)?;
    job::record_takes(host, &cfg, (job.id, p.attempt), &out_dir, &rj, Some(&report), p)?;
    p.stage(Stage::Assemble);
    let out = assemble_stage(host, &out_dir, &takes, &rj.ffmpeg_args, rj.video_secs, job::deadlines(&cfg).ffmpeg_for(rj.video_secs), p)?;
    job::last_good(Some(&report), p);
    Ok(outcome(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use pipeline::{Deadlines, FailCode, RecordRequest, host::LaunchSpec, manifest::EnvSnapshot};

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

    /// Настройки с временным деревом HLAE и CS2, где есть всё нужное предпроверке.
    fn healthy_settings(tag: &str) -> Settings {
        let root = std::env::temp_dir().join(format!("cs2cinema-precheck-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in ["hlae/HLAE.exe", "hlae/x64/AfxHookSource2.dll", "hlae/ffmpeg/bin/ffmpeg.exe", "cs2/game/bin/win64/cs2.exe"] {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let slash = |p: std::path::PathBuf| p.to_string_lossy().replace('\\', "/");
        Settings { hlae_dir: slash(root.join("hlae")), cs2_dir: slash(root.join("cs2")), out_root: slash(root.join("out")), ..Settings::default() }
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

    /// `record.json` на `n` дублей по 5 игровых секунд в кадре 1280x720, как у дублей фейка.
    fn sample_record_json(n: usize, demo: PathBuf, plan_hash: String) -> RecordJson {
        let takes: Vec<TakePlan> = (0..n as u32).map(|index| TakePlan { index, shot: index as usize, game_secs: 5.0, capture_fps: 60 }).collect();
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
            takes: takes.iter().map(pipeline::manifest::TakeSpec::from).collect(),
            ffmpeg_args: args,
            video_secs: 10.0,
            shots: takes.len(),
            secs: 5.0 * takes.len() as f32,
            plan_hash,
            created_unix: 1,
        }
    }

    use pipeline::record::verify::TakeFacts;

    fn facts() -> TakeFacts {
        TakeFacts { video_secs: 5.0, audio_secs: 5.0, width: 1280, height: 720, video_bytes: 10, audio_bytes: 4 }
    }

    /// Папка задачи, как её оставляет запись: скрипты, `record.json`, дубли на диске и манифест с заданными статусами.
    /// Возвращает задачу «Повторить» уже запущенной (`resume`, статус `running`).
    fn retry_job(tag: &str, statuses: &[TakeStatus]) -> (Store, Job, PathBuf, Settings) {
        let s = healthy_settings(tag);
        let dir = Path::new(&s.out_root).join("match_hero");
        let nonce = "ab12cd34";
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("highlights.cfg"), format!("echo CS2C {nonce} SCRIPT\nexec cs2c_01\n")).unwrap();
        std::fs::write(dir.join("cs2c_01.cfg"), format!("echo CS2C {nonce} TAKE_END 0\n")).unwrap();
        let hash = job::plan_hash(&dir, nonce);
        let takes: Vec<TakePlan> = (0..statuses.len() as u32).map(|index| TakePlan { index, shot: index as usize, game_secs: 5.0, capture_fps: 60 }).collect();
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
        let store = Store::in_memory().unwrap();
        let id = store.add_job(&new_job()).unwrap();
        store.set_out_dir(id, &dir).unwrap();
        store.requeue_resume(id).unwrap();
        store.start_job(id).unwrap();
        let job = store.job(id).unwrap().unwrap();
        assert!(job.resume);
        (store, job, dir, s)
    }

    fn reporter(store: &Store, job: &Job) -> JobReporter {
        JobReporter::new(store.clone(), job.id, Reporter::detached(), CancelToken::default(), job.attempt)
    }

    fn cleanup(s: &Settings) {
        let _ = std::fs::remove_dir_all(Path::new(&s.cs2_dir).parent().unwrap());
    }

    #[test]
    fn retry_with_every_take_validated_goes_straight_to_assembly() {
        let (store, job, dir, s) = retry_job("retry-ok", &[TakeStatus::Validated, TakeStatus::Validated]);
        // поддельный ffmpeg файлов не пишет: временный ролик кладёт сам тест
        std::fs::write(dir.join("highlights.part.mp4"), b"new").unwrap();
        std::fs::write(dir.join("highlights.mp4"), b"old").unwrap();
        let host = FakeHost::new(Scenario::happy(2));
        let out = run(&host, &job, &s, &store, &reporter(&store, &job)).expect("все дубли проверены — склейка без записи");

        assert_eq!(host.launches(), 0, "игра не запускалась");
        let runs = host.runs();
        let ffmpeg: Vec<_> = runs.iter().filter(|(_, prog, _)| prog.contains("ffmpeg")).collect();
        assert_eq!(ffmpeg.len(), 1, "{runs:?}");
        assert_eq!(ffmpeg[0].2.last().map(String::as_str), Some("highlights.part.mp4"));
        assert!(!runs.iter().any(|(_, _, a)| a.iter().any(|x| x == "-version")), "предпроверка пропущена: {runs:?}");
        assert_eq!(out.result, dir.join("highlights.mp4"));
        assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"new");
        assert!(!dir.join("highlights.part.mp4").exists());
        assert_eq!((out.shots, out.out_dir.as_path()), (2, dir.as_path()));
        cleanup(&s);
    }

    #[test]
    fn failed_assembly_keeps_the_previous_highlights_and_drops_the_part() {
        let (_store, _job, dir, s) = retry_job("retry-asm-fail", &[TakeStatus::Validated, TakeStatus::Validated]);
        std::fs::write(dir.join("highlights.mp4"), b"old").unwrap();
        std::fs::write(dir.join("highlights.part.mp4"), b"half").unwrap();
        let failed = pipeline::host::CmdOutput { code: Some(1), stderr_tail: vec!["Conversion failed".into()], elapsed: Duration::ZERO };
        let host = FakeHost::new(Scenario::happy(1).run_result(Ok(failed)));
        let rj = RecordJson::load(&dir).unwrap();
        let e = assemble_stage(&host, &dir, &rj.take_plans(), &rj.ffmpeg_args, 10.0, Duration::from_secs(60), &pipeline::Quiet).expect_err("ffmpeg упал");
        assert!(e.message.contains("ffmpeg завершился с ошибкой"), "{}", e.message);
        assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"old", "прежний ролик цел");
        assert!(!dir.join("highlights.part.mp4").exists(), "временный файл удалён");
        cleanup(&s);
    }

    #[test]
    fn assembly_refuses_a_take_that_is_not_validated() {
        let (_store, _job, dir, s) = retry_job("retry-asm-refuse", &[TakeStatus::Validated, TakeStatus::Recorded]);
        let host = FakeHost::new(Scenario::happy(1));
        let rj = RecordJson::load(&dir).unwrap();
        let e = assemble_stage(&host, &dir, &rj.take_plans(), &rj.ffmpeg_args, 10.0, Duration::from_secs(60), &pipeline::Quiet).expect_err("дубль не проверен");
        let f = e.fail.as_ref().expect("сбой типизирован");
        assert_eq!((f.code, f.state), (FailCode::TakeInvalid, RecordState::Take));
        assert!(e.message.contains("take0001"), "{}", e.message);
        assert!(f.hint.contains("take0001"), "{}", f.hint);
        assert!(host.runs().is_empty(), "ffmpeg не запускался: {:?}", host.runs());

        // файл изменился после проверки — тоже отказ
        let (_store, _job, dir2, s2) = retry_job("retry-asm-size", &[TakeStatus::Validated]);
        std::fs::write(dir2.join("clips/take0000/video.mp4"), b"xx").unwrap();
        let rj = RecordJson::load(&dir2).unwrap();
        let e = assemble_stage(&host, &dir2, &rj.take_plans(), &rj.ffmpeg_args, 10.0, Duration::from_secs(60), &pipeline::Quiet).expect_err("размер другой");
        assert!(e.message.contains("размер video.mp4 изменился"), "{}", e.message);
        assert!(host.runs().is_empty());
        cleanup(&s);
        cleanup(&s2);
    }

    #[test]
    fn retry_without_record_json_fails_with_retry_no_data() {
        let (store, job, dir, s) = retry_job("retry-nodata", &[TakeStatus::Validated]);
        RecordJson::remove(&dir).unwrap();
        let host = FakeHost::new(Scenario::happy(1));
        let e = run(&host, &job, &s, &store, &reporter(&store, &job)).expect_err("нет record.json");
        assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::RetryNoData));
        finish(&store, job.id, &Err(e)).unwrap();
        let j = store.job(job.id).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.fail_code.as_deref(), Some("RETRY_NO_DATA"));
        assert!(j.error.is_some_and(|m| m.contains("запустите задачу заново")));
        assert_eq!(host.launches(), 0);
        cleanup(&s);
    }

    #[test]
    fn retry_with_changed_scripts_fails_with_retry_no_data() {
        let (store, job, dir, s) = retry_job("retry-changed", &[TakeStatus::Validated]);
        std::fs::write(dir.join("cs2c_01.cfg"), "echo правка руками\n").unwrap();
        let host = FakeHost::new(Scenario::happy(1));
        let e = run(&host, &job, &s, &store, &reporter(&store, &job)).expect_err("скрипты изменились");
        assert_eq!(e.fail.as_ref().map(|f| f.code), Some(FailCode::RetryNoData));
        assert!(e.message.contains("скрипты записи в папке задачи изменились"), "{}", e.message);
        assert!(host.runs().is_empty());
        cleanup(&s);
    }

    /// Steam-раскладка вокруг CS2 в папке настроек: `<lib>/steamapps/common/<игра>` и манифест рядом,
    /// чтобы предпроверка нашла сборку CS2 (она попадает в снимок окружения).
    fn with_steam_layout(s: &mut Settings) {
        let lib = Path::new(&s.cs2_dir).parent().unwrap().join("lib");
        let game = lib.join("steamapps/common/Counter-Strike Global Offensive");
        let exe = game.join("game/bin/win64/cs2.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(exe, b"x").unwrap();
        std::fs::write(
            lib.join("steamapps/appmanifest_730.acf"),
            "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n\t\"TargetBuildID\"\t\t\"0\"\n}\n",
        )
        .unwrap();
        s.cs2_dir = game.to_string_lossy().replace('\\', "/");
    }

    #[test]
    fn retry_with_an_invalid_take_sets_clips_aside_and_records_again() {
        let (store, job, dir, mut s) = retry_job("retry-rec", &[TakeStatus::Validated, TakeStatus::Invalid]);
        with_steam_layout(&mut s);
        // текущая настройка — другой кадр: запись обязана идти в кадре из record.json (1280x720)
        s.resolution = 1080;
        // поддельный ffmpeg склейки файлов не пишет
        std::fs::write(dir.join("highlights.part.mp4"), b"new").unwrap();
        let ok = pipeline::host::CmdOutput { code: Some(0), stderr_tail: vec![], elapsed: Duration::ZERO };
        let host = FakeHost::new(
            Scenario::happy(2)
                .clips_dir(dir.join("clips"))
                .program_for("ffmpeg", "-version", &["ffmpeg version 7.1.1 Copyright (c) the FFmpeg developers"], Ok(ok.clone()))
                .program_for("ffprobe", "-version", &["ffprobe version 7.1.1 Copyright (c) the FFmpeg developers"], Ok(ok)),
        );
        let out = run(&host, &job, &s, &store, &reporter(&store, &job)).expect("повтор записывает дубли заново и склеивает");

        assert_eq!(host.launches(), 1, "игра запущена один раз");
        assert_eq!(out.result, dir.join("highlights.mp4"));
        assert_eq!(std::fs::read(dir.join("highlights.mp4")).unwrap(), b"new");
        assert!(!dir.join("installed.json").exists(), "папка игры убрана после записи");

        // прежние дубли и манифест — в clips_old_<ts> как улики
        let old: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("clips_old_"))
            .collect();
        assert_eq!(old.len(), 1, "{old:?}");
        assert!(old[0].join("take0000/video.mp4").is_file() && old[0].join("take0001/video.mp4").is_file());
        let old_manifest = Manifest::load(&old[0]).expect("прежний манифест");
        assert_eq!(old_manifest.entry(1).map(|e| e.status), Some(TakeStatus::Invalid));

        // оба дубля записаны заново и проверены с номером попытки задачи
        let m = Manifest::load(&dir.join("clips")).expect("новый манифест");
        assert_eq!(m.takes.len(), 2);
        for t in &m.takes {
            assert_eq!((t.status, t.attempt), (TakeStatus::Validated, job.attempt), "{t:?}");
        }

        // снимок окружения: env.json, манифест попытки и журнал
        let env: EnvSnapshot = serde_json::from_str(&std::fs::read_to_string(dir.join("env.json")).expect("env.json в папке задачи")).unwrap();
        assert_eq!(env.cs2_build.as_deref(), Some("25815307"));
        assert!(env.hlae_version.as_deref().is_some_and(|v| v.starts_with("2.192")), "{env:?}");
        assert!(env.ffmpeg.as_deref().is_some_and(|v| v.contains("7.1.1")), "{env:?}");
        assert_eq!(env.frame, (1280, 720));
        assert_eq!(m.attempts.last().and_then(|a| a.env.as_ref()), Some(&env), "тот же снимок у попытки в манифесте");
        let log = store.job(job.id).unwrap().unwrap().log;
        assert!(log.contains("окружение: CS2 25815307"), "{log}");
        assert!(log.contains("прошлые дубли отложены в clips_old_"), "{log}");
        assert!(log.contains("дубли не все проверены: 2"), "{log}");
        // успешная запись запоминает пару «сборка CS2 / версия HLAE»
        assert_eq!(store.last_good().unwrap().map(|g| g.cs2_build), Some("25815307".to_owned()));
        cleanup(&s);
    }
}
