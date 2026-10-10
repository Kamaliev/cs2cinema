//! Выполнение задач на монтаж: очередь из базы, по одной (CS2 пишет только одну), с отменой.
//!
//! Задача самодостаточна: откуда демка и про кого ролик она знает сама, поэтому разбор
//! делается заново в фоновом потоке — и задачу можно повторить после перезапуска.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use pipeline::{
    Deadlines, FailCode, PlanOptions, Progress, RecordFailure, RecordReport, RecordRequest, RecordState, Source, Stage,
    host::{CmdSpec, Host, HostError, LaunchSpec},
    manifest::{EnvSnapshot, Manifest, RecordJson, TakeStatus},
    precheck::{self, Demo, Inputs, Level, Report, ids, space},
    record::{Verify, failure},
};
use renderer::TakePlan;
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

/// Ошибка задачи: текст для строки задачи и, если это сбой записи, его типизированное описание.
#[derive(Debug)]
pub struct RunError {
    pub message: String,
    pub fail: Option<RecordFailure>,
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        RunError { message, fail: None }
    }
}

impl From<&str> for RunError {
    fn from(message: &str) -> Self {
        RunError::from(message.to_owned())
    }
}

/// Отчёт задачи: в базу (переживает перезапуск) и в строку состояния окна.
struct JobReporter {
    store: Store,
    id: i64,
    ui: Reporter,
    cancel: CancelToken,
    /// Папка задачи: известна после разбора демки; сюда `TakeDone` дописывает номера записанных дублей (D-08).
    out_dir: OnceLock<PathBuf>,
    /// Номер попытки — в метке каждой строки журнала.
    attempt: u32,
    /// Текущий этап для метки строк журнала и время входа в него — по нему считается длительность (OBS-01, D-11).
    cur: Mutex<Option<(String, Instant)>>,
}

impl JobReporter {
    fn new(store: Store, id: i64, ui: Reporter, cancel: CancelToken, attempt: u32) -> Self {
        JobReporter { store, id, ui, cancel, out_dir: OnceLock::new(), attempt, cur: Mutex::new(None) }
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
        if let (pipeline::Event::TakeDone { n, .. }, Some(dir)) = (e, self.out_dir.get()) {
            // номер дубля в событии с единицы, в `installed.json` и в имени `takeNNNN` — с нуля
            launch::note_take_done(dir, n.saturating_sub(1));
        }
        self.log(&e.to_string());
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

/// Запрос записи из настроек и `record.json`: кадр, метка, имя демки и дубли берутся из `record.json`, а не из
/// текущих настроек — старые скрипты и граф склейки нельзя смешивать с новым разрешением (Pattern 5).
pub fn record_request(s: &Settings, out_dir: &Path, rj: &RecordJson, job_id: i64, verify: Option<Verify>) -> RecordRequest {
    RecordRequest {
        out_dir: out_dir.to_path_buf(),
        launch: LaunchSpec {
            hlae_dir: s.hlae_dir.clone().into(),
            cs2_exe: launch::cs2_exe(&s.cs2_dir),
            demo_name: rj.demo_name.clone(),
            size: rj.frame,
            nonce: rj.nonce.clone(),
            console_log: Some(Path::new(&s.cs2_dir).join("game/csgo/console.log")),
        },
        takes: rj.take_plans(),
        deadlines: deadlines(s),
        header: format!("задача #{job_id} · {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")),
        afxerror_ignore: s.afxerror_ignore.clone(),
        verify,
    }
}

/// Запись дублей через `pipeline::record`. Отмена становится сентинелом `CANCELLED`, остальные
/// сбои несут типизированное описание до строки задачи.
pub fn record_stage(host: &dyn Host, req: &RecordRequest, p: &dyn Progress) -> Result<RecordReport, RunError> {
    pipeline::record(host, req, p)
        .map_err(|f| if f.code == FailCode::Cancelled { RunError::from(pipeline::CANCELLED) } else { RunError { message: f.message.clone(), fail: Some(f) } })
}

/// Нужна ли задаче предпроверка окружения: «только план» ничего не пишет, поэтому её пропускает (D-01).
pub fn needs_precheck(kind: JobKind) -> bool {
    kind != JobKind::Plan
}

/// Что проверять: пути из настроек, дедлайны с «Запасом по времени», пара последней удачной записи.
/// Один построитель для задачи и для кнопки в настройках (D-01).
pub fn precheck_inputs(s: &Settings, demo: Demo, last_good: Option<store::LastGood>) -> Inputs {
    Inputs {
        hlae_dir: s.hlae_dir.clone().into(),
        cs2_dir: s.cs2_dir.clone().into(),
        out_root: s.out_root.clone().into(),
        demo,
        last_good: last_good.map(|g| (g.cs2_build, g.hlae_version)),
        deadlines: deadlines(s),
        ffmpeg: "ffmpeg".into(),
        ffprobe: "ffprobe".into(),
    }
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

/// Предпроверка первым шагом записи (PRE-02, D-01, D-02): отчёт уходит в базу, `fail` останавливает задачу
/// типизированным сбоем на ступени «предпроверка», `warn` остаются в журнале и запись идёт дальше.
pub fn precheck_stage(host: &dyn Host, inputs: &Inputs, store: &Store, id: i64, p: &dyn Progress) -> Result<Report, RunError> {
    p.event(&pipeline::Event::State { state: RecordState::Precheck, take: None });
    let t0 = host.now();
    let report = precheck::run(host, inputs, p)?;
    let _ = store.set_precheck(id, &report.to_json());
    if let Some(f) = report.failure(host.now().saturating_sub(t0)) {
        return Err(RunError { message: f.message.clone(), fail: Some(f) });
    }
    // лестница прячется до начала записи
    let _ = store.set_phase(id, "");
    Ok(report)
}

/// Второй проход оценки места по плану (D-03, PRE-06): пункты места в отчёте заменяются, нехватка места
/// (`fail`) останавливает задачу до запуска игры, `warn` и `fail` попадают в журнал.
fn space_pass(host: &dyn Host, inputs: &Inputs, need: space::Need, report: &mut Report, store: &Store, id: i64, p: &dyn Progress) -> Result<(), RunError> {
    precheck::check_space(host, inputs, need, report);
    let _ = store.set_precheck(id, &report.to_json());
    for item in report.items.iter().filter(|i| matches!(i.id.as_str(), ids::SPACE_RESULTS | ids::SPACE_CS2)) {
        match item.level {
            Level::Ok => {}
            Level::Warn => p.log(&format!("предпроверка: ⚠ {}", item.message)),
            Level::Fail => p.log(&format!("предпроверка: ✖ {}", item.message)),
        }
    }
    match report.failure(Duration::ZERO) {
        Some(f) => Err(RunError { message: f.message.clone(), fail: Some(f) }),
        None => Ok(()),
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
    // предпроверка окружения — до скачивания, разбора и плана (PRE-02): неисправное окружение видно за секунды
    let inputs = precheck_inputs(s, job_demo(job, store), store.last_good().ok().flatten());
    let mut report = if needs_precheck(job.kind) { Some(precheck_stage(host, &inputs, store, job.id, p)?) } else { None };
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
    let _ = p.out_dir.set(out_dir.clone());
    // NVENC: для склейки — у ffmpeg из PATH, для дублей — у ffmpeg, которым пишет HLAE
    let (mut nvenc, mut take_nvenc) = (false, false);
    if s.gpu_encode && job.kind != JobKind::Plan {
        // проба с дедлайном и отменой задачи (TMO-03): зависший ffmpeg не вешает задачу
        let probe_for = deadlines(s).nvenc_probe;
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
    begin_plan(&out_dir)?;
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
    let rj = record_json_for(&plan, &a.demo_path, a.demo_len(), &demo_name, s, take_nvenc)?;
    // без record.json «Повторить» невозможен, поэтому ошибка сохранения — ошибка задачи; файл ложится сразу после плана
    rj.save(&out_dir).map_err(|e| format!("не удалось сохранить record.json: {e} — без него «Повторить» невозможен"))?;
    // второй проход места по плану (D-03): сумма игровых секунд дублей, размер кадра и демки; до запуска игры
    if let Some(report) = report.as_mut() {
        space_pass(host, &inputs, space::estimate(&plan.takes, s.frame_size(), a.demo_len()), report, store, job.id, p)?;
    }
    // первый запуск и «Заново» всегда пишут дубли с проверкой, прежние не переиспользуют (решение 2)
    record_takes(host, s, job, &out_dir, &rj, report.as_ref(), p)?;

    p.stage(Stage::Assemble);
    let out = assemble_stage(host, &out_dir, &plan.takes, &plan.ffmpeg_args, plan.video_secs, deadlines(s).ffmpeg_for(plan.video_secs), p)?;
    // пара «сборка CS2 / версия HLAE» после удачной задачи, в которой шла запись (D-04, DIF-01)
    remember_last_good(store, report.as_ref());
    Ok(JobOutcome { out_dir, result: out, shots, secs })
}

/// Пара «сборка CS2 / версия HLAE» удачной записи — в базу (для предупреждения об обновлении, DIF-01).
fn remember_last_good(store: &Store, report: Option<&Report>) {
    if let Some(r) = report {
        if let (Some(build), Some(hlae)) = (&r.cs2_build, &r.hlae_version) {
            let _ = store.set_last_good(build, hlae);
        }
    }
}

/// Начало плана: устаревший `record.json` удаляется (Pitfall 7). Иначе «Повторить» после упавшего на разборе или
/// плане первого запуска взяло бы план прошлой попытки, чьи скрипты уже перезаписаны или не дописаны.
fn begin_plan(out_dir: &Path) -> Result<(), String> {
    RecordJson::remove(out_dir).map_err(|e| format!("не удалось убрать устаревший record.json: {e}"))
}

/// Всё, что нужно записи и склейке, из плана и настроек: кадр, fps, дубли, аргументы ffmpeg, отпечаток скриптов.
/// `secs` — длина ролика по плану (то, что показывает карточка задачи).
fn record_json_for(plan: &pipeline::Plan, demo_path: &Path, demo_len: u64, demo_name: &str, s: &Settings, take_nvenc: bool) -> Result<RecordJson, String> {
    let created_unix = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs() as i64).unwrap_or(0);
    Ok(RecordJson {
        version: 1,
        nonce: plan.nonce.clone(),
        demo_path: PathBuf::from(pipeline::absolute_slash(demo_path)),
        demo_name: demo_name.to_owned(),
        demo_len,
        frame: s.frame_size(),
        fps: s.fps,
        take_nvenc,
        takes: plan.takes.iter().map(pipeline::manifest::TakeSpec::from).collect(),
        ffmpeg_args: plan.ffmpeg_args.clone(),
        video_secs: plan.video_secs,
        shots: plan.timeline.shots.len(),
        secs: plan.timeline.total_secs(),
        plan_hash: plan_hash(&plan.out_dir, &plan.nonce),
        created_unix,
    })
}

/// Прежние дубли целиком — в `clips_old_<секунды>` (при совпадении имени — суффикс `-2`, `-3`, …): HLAE нумерует
/// новые дубли «наибольший + 1», старые сбили бы нумерацию `takeNNNN`, а запись дорогая — не удаляем (D-07, D-12, RET-06).
fn set_aside_clips(out_dir: &Path) -> Result<Option<PathBuf>, String> {
    let clips = out_dir.join("clips");
    if !clips.is_dir() {
        return Ok(None);
    }
    let stamp = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    let mut to = out_dir.join(format!("clips_old_{stamp}"));
    let mut n = 2;
    while to.exists() {
        to = out_dir.join(format!("clips_old_{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&clips, &to).map_err(|e| format!("не удалось отложить прошлые дубли: {e}"))?;
    Ok(Some(to))
}

/// Одна дорога записи для первого запуска, «Заново» и «Повторить»: прежние дубли в сторону, снимок окружения,
/// установка файлов в игру, запись с проверкой дублей, уборка папки игры при любом исходе (OBS-03, D-05, D-07).
fn record_takes(
    host: &dyn Host,
    s: &Settings,
    job: &Job,
    out_dir: &Path,
    rj: &RecordJson,
    report: Option<&Report>,
    p: &JobReporter,
) -> Result<RecordReport, RunError> {
    p.stage(Stage::Record);
    if let Some(to) = set_aside_clips(out_dir)? {
        p.log(&format!("прошлые дубли отложены в {}", to.file_name().unwrap_or_default().to_string_lossy()));
    }
    // снимок окружения — в env.json, в манифест попытки и строкой в журнал
    let nvenc_assemble = rj.ffmpeg_args.iter().any(|a| a == "h264_nvenc");
    let env = match report {
        Some(r) => EnvSnapshot::from_report(r, rj.frame, rj.fps, nvenc_assemble, rj.take_nvenc),
        None => EnvSnapshot { frame: rj.frame, fps: rj.fps, nvenc_assemble, nvenc_takes: rj.take_nvenc, ..EnvSnapshot::default() },
    };
    if let Err(e) = env.save(out_dir) {
        p.log(&format!("env.json не записан: {e}"));
    }
    p.log(&env.one_line());
    let installed = launch::install_files(&s.cs2_dir, out_dir, &rj.demo_name, |path| pipeline::write_demo_copy(&rj.demo_path, path));
    // игру закрывает сама запись при любом выходе, в том числе при отмене
    let recorded_run = installed.map_err(RunError::from).and_then(|()| {
        let verify = Verify { ffprobe: "ffprobe".into(), plan_hash: rj.plan_hash.clone(), attempt: p.attempt, env: Some(env) };
        let req = record_request(s, out_dir, rj, job.id, Some(verify));
        p.log(&format!("запас по времени ×{:.2}", s.time_margin));
        record_stage(host, &req, p)
    });
    // папка игры чистая при любом исходе — успех, сбой, отмена (D-05, D-06, TMO-06): запись вернулась
    // только после подтверждённого закрытия игры, поэтому копия демки уже никем не занята
    for note in launch::cleanup_installed(&s.cs2_dir, out_dir) {
        p.log(&note);
    }
    let rep = recorded_run?;
    p.log(&format!("запись заняла {:.0} с", rep.elapsed.as_secs_f32()));
    Ok(rep)
}

/// Повтору нечего взять из папки задачи: типизированный сбой `RETRY_NO_DATA` (D-08).
fn retry_no_data(reason: &str) -> RunError {
    let f = RecordFailure::new(FailCode::RetryNoData, RecordState::Precheck, failure::msg_retry_no_data(reason), Duration::ZERO);
    RunError { message: f.message.clone(), fail: Some(f) }
}

/// Все ли дубли плана проверены по манифесту (индексы дублей — из `record.json`).
fn all_takes_validated(m: &Manifest, takes: &[TakePlan]) -> bool {
    m.all_validated() && takes.iter().all(|t| m.entry(t.index).is_some_and(|e| e.status == TakeStatus::Validated))
}

/// «Повторить» (RET-02, RET-03, D-08): без скачивания, разбора демки и плана — по `record.json` из папки задачи.
/// Все дубли проверены — сразу склейка, без предпроверки и без игры (RET-03).
fn resume_job(host: &dyn Host, job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
    let out_dir = job.out_dir.clone().ok_or_else(|| retry_no_data("папка задачи неизвестна"))?;
    let _ = store.set_out_dir(job.id, &out_dir);
    let _ = p.out_dir.set(out_dir.clone());
    let rj = RecordJson::load(&out_dir).map_err(|e| retry_no_data(&e))?;
    if plan_hash(&out_dir, &rj.nonce) != rj.plan_hash {
        return Err(retry_no_data("скрипты записи в папке задачи изменились"));
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
    if manifest.as_ref().is_some_and(|m| all_takes_validated(m, &takes)) {
        p.log("все дубли проверены по манифесту — сразу склейка, без предпроверки и игры");
        p.stage(Stage::Assemble);
        let out = assemble_stage(host, &out_dir, &takes, &rj.ffmpeg_args, rj.video_secs, deadlines(s).ffmpeg_for(rj.video_secs), p)?;
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
    let mut report = precheck_stage(host, &inputs, store, job.id, p)?;
    space_pass(host, &inputs, space::estimate(&takes, rj.frame, rj.demo_len), &mut report, store, job.id, p)?;
    record_takes(host, s, job, &out_dir, &rj, Some(&report), p)?;
    p.stage(Stage::Assemble);
    let out = assemble_stage(host, &out_dir, &takes, &rj.ffmpeg_args, rj.video_secs, deadlines(s).ffmpeg_for(rj.video_secs), p)?;
    remember_last_good(store, Some(&report));
    Ok(outcome(out))
}

/// Последний аргумент склейки — `highlights.mp4`; ролик пишется во временный `highlights.part.mp4` (D-06).
fn part_args(args: &[String]) -> Result<Vec<String>, String> {
    match args.last() {
        Some(last) if last == "highlights.mp4" => {
            let mut v = args.to_vec();
            if let Some(l) = v.last_mut() {
                *l = PART_NAME.to_owned();
            }
            Ok(v)
        }
        _ => Err(format!("неожиданные аргументы склейки: {}", args.join(" "))),
    }
}

/// Временное имя ролика: расширение `.mp4` сохранено — по нему ffmpeg выбирает формат.
const PART_NAME: &str = "highlights.part.mp4";

/// Дубль `take` годен к склейке, только если манифест называет его `validated`, оба файла на месте и их размеры
/// равны записанным (TAK-04, TAK-05, T-03-16).
fn check_take_for_assembly(m: &Manifest, clips: &Path, take: &TakePlan) -> Result<(), String> {
    let dir = take.dir_name();
    let Some(e) = m.entry(take.index) else {
        return Err("дубля нет в манифесте".to_owned());
    };
    if e.status != TakeStatus::Validated {
        let status = match e.status {
            TakeStatus::Pending => "не записан",
            TakeStatus::Recorded => "записан, но не проверен",
            TakeStatus::Validated => "проверен",
            TakeStatus::Invalid => "негоден",
        };
        return Err(match &e.error {
            Some(why) => format!("{status} ({why})"),
            None => status.to_owned(),
        });
    }
    for (file, want) in TakePlan::FILES.iter().zip([e.video_bytes, e.audio_bytes]) {
        let have = std::fs::metadata(clips.join(&dir).join(file)).ok().filter(|m| m.is_file()).map(|m| m.len());
        match (have, want) {
            (None, _) => return Err(format!("файл {file} пропал")),
            (Some(h), Some(w)) if h != w => return Err(format!("размер {file} изменился: {h} байт вместо {w}")),
            (Some(_), None) => return Err(format!("размер {file} в манифесте неизвестен")),
            _ => {}
        }
    }
    Ok(())
}

/// Склейка только из проверенных дублей и через временное имя (TAK-04..06, D-06): ffmpeg пишет `highlights.part.mp4`,
/// переименование в `highlights.mp4` — после кода 0. При сбое удаляется только `.part`, прежний ролик остаётся.
pub fn assemble_stage(
    host: &dyn Host,
    out_dir: &Path,
    takes: &[TakePlan],
    args: &[String],
    secs: f32,
    deadline: Duration,
    p: &dyn Progress,
) -> Result<PathBuf, RunError> {
    let clips = out_dir.join("clips");
    let Some(manifest) = Manifest::load(&clips) else {
        let f = take_invalid_failure(
            1,
            takes.len() as u32,
            &takes.first().map(TakePlan::dir_name).unwrap_or_default(),
            "манифест дублей не найден — запишите задачу заново",
        );
        return Err(RunError { message: f.message.clone(), fail: Some(f) });
    };
    let total = takes.len() as u32;
    for take in takes {
        if let Err(why) = check_take_for_assembly(&manifest, &clips, take) {
            let f = take_invalid_failure(take.index + 1, total, &take.dir_name(), &format!("не проверен по манифесту: {why}"));
            return Err(RunError { message: f.message.clone(), fail: Some(f) });
        }
    }
    let part_args = part_args(args)?;
    let (part, out) = (out_dir.join(PART_NAME), out_dir.join("highlights.mp4"));
    p.log("склеиваю ролик через ffmpeg…");
    if let Err(e) = run_ffmpeg(host, out_dir, &part_args, secs, deadline, p) {
        // недописанный ролик не должен выглядеть результатом (Pitfall 11); прежний highlights.mp4 не трогаем
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    if !part.is_file() {
        return Err(format!("ffmpeg не создал {PART_NAME}").into());
    }
    std::fs::rename(&part, &out).map_err(|e| format!("не удалось заменить highlights.mp4: {e} — закройте плеер с прошлым роликом и нажмите «Повторить»"))?;
    p.log(&format!("ролик готов: {}", out.display()));
    Ok(out)
}

/// Типизированный сбой «дубль негоден»: состояние `Take`, сообщение и подсказка с номером дубля.
fn take_invalid_failure(n: u32, total: u32, dir: &str, reason: &str) -> RecordFailure {
    let mut f = RecordFailure::new(FailCode::TakeInvalid, RecordState::Take, failure::msg_take_invalid(n, total, dir, reason), Duration::ZERO);
    f.hint = failure::hint_take_invalid(n, dir);
    f
}

/// Отпечаток скриптов записи (`highlights.cfg`, `cs2c_*`): совпал — дубли в `clips/` от этого же плана.
/// `nonce` меняется на каждый план и стоит во всех маркерах консоли, поэтому перед хешированием
/// заменяется на `NONCE`: иначе «дубли этого плана уже записаны» никогда бы не срабатывало.
fn plan_hash(out_dir: &Path, nonce: &str) -> String {
    let mut names: Vec<_> = std::fs::read_dir(out_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n == "highlights.cfg" || n.starts_with("cs2c_"))
        .collect();
    names.sort();
    // FNV-1a: стабилен между версиями Rust, в отличие от DefaultHasher
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for n in names {
        let body = std::fs::read(out_dir.join(&n)).unwrap_or_default();
        let body = String::from_utf8_lossy(&body);
        let body = if nonce.is_empty() { body.into_owned() } else { body.replace(nonce, "NONCE") };
        for b in n.bytes().chain(body.into_bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Дедлайны записи и склейки с учётом «Запаса по времени» из настроек.
fn deadlines(s: &Settings) -> Deadlines {
    Deadlines::default().scaled(s.time_margin)
}

/// Текст ошибки запуска утилиты склейки по-русски.
fn ffmpeg_error(e: &HostError, deadline: Duration) -> String {
    match e {
        HostError::Timeout => format!("ffmpeg не уложился в {} с — склейка прервана; увеличьте «Запас по времени» в настройках", deadline.as_secs()),
        HostError::Cancelled => pipeline::CANCELLED.to_owned(),
        HostError::Spawn(e) => format!("не удалось запустить ffmpeg (он должен быть в PATH): {e}"),
        e => format!("ffmpeg: {e}"),
    }
}

/// Склейка: ffmpeg в `dir` через `Host::run` — с дедлайном и отменой; прогресс берётся из `-progress pipe:1`
/// (`out_time_us` в микросекундах готового ролика) и раз в 10 с пишется в журнал.
fn run_ffmpeg(host: &dyn Host, dir: &Path, args: &[String], secs: f32, deadline: Duration, p: &dyn Progress) -> Result<(), String> {
    let mut all: Vec<String> = ["-hide_banner", "-nostats", "-loglevel", "error", "-progress", "pipe:1"].map(String::from).into();
    all.extend_from_slice(args);
    let cmd = CmdSpec { program: "ffmpeg".into(), args: all, cwd: Some(dir.to_path_buf()), deadline };
    let started = std::time::Instant::now();
    let mut logged = 0u64;
    let mut on_line = |line: &str| {
        let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok()) else { return };
        let tick = started.elapsed().as_secs() / 10;
        if tick > logged && secs > 0.0 {
            logged = tick;
            let pct = (us as f32 / 1e6 / secs * 100.0).min(100.0);
            p.log(&format!("склейка: {pct:.0}%"));
        }
    };
    let out = host.run(&cmd, &|| p.cancelled(), &mut on_line).map_err(|e| ffmpeg_error(&e, deadline))?;
    if out.code != Some(0) {
        let tail: Vec<&str> = out.stderr_tail.iter().rev().take(6).map(String::as_str).collect();
        return Err(format!("ffmpeg завершился с ошибкой: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    p.log(&format!("склейка заняла {:.0} с", started.elapsed().as_secs_f32()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_dir(tag: &str, nonce: &str, tick: u32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2cinema-jobs-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("highlights.cfg"), format!("// шапка\necho CS2C {nonce} SCRIPT\nexec cs2c_01\n")).unwrap();
        std::fs::write(dir.join("cs2c_01.cfg"), format!("mirv_cmd addAtTick {tick} \"mirv_streams record end; echo CS2C {nonce} TAKE_END 0\"\n")).unwrap();
        dir
    }

    #[test]
    fn plan_hash_ignores_the_nonce() {
        let a = plan_dir("a", "aaaa1111", 1000);
        let b = plan_dir("b", "bbbb2222", 1000);
        let c = plan_dir("c", "cccc3333", 1001);
        assert_eq!(plan_hash(&a, "aaaa1111"), plan_hash(&b, "bbbb2222"), "nonce не должен менять отпечаток");
        assert_ne!(plan_hash(&a, "aaaa1111"), plan_hash(&c, "cccc3333"), "другой тик — другой план");
        for d in [a, b, c] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

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
        let e = record_stage(&host, &request(&dir), &pipeline::Quiet).expect_err("консоль не открывается — запись падает");
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
        let e = record_stage(&host, &request(&dir), &Cancelled).expect_err("отмена прерывает запись");
        assert_eq!(e.message, pipeline::CANCELLED);
        assert!(e.fail.is_none(), "отмена не несёт типизированного сбоя");
        finish(&store, id, &Err(e)).unwrap();

        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Cancelled);
        assert_eq!(j.fail_code, None);
        assert!(host.terminated() > 0, "игра закрыта и при отмене");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn assemble_with(r: Result<pipeline::host::CmdOutput, HostError>) -> Result<(), String> {
        let host = FakeHost::new(Scenario::happy(1).run_result(r));
        run_ffmpeg(&host, Path::new("."), &[], 10.0, Duration::from_secs(600), &pipeline::Quiet)
    }

    #[test]
    fn ffmpeg_failures_are_explained_in_russian() {
        let timeout = assemble_with(Err(HostError::Timeout)).unwrap_err();
        assert!(timeout.starts_with("ffmpeg не уложился в 600 с"), "{timeout}");
        assert!(timeout.contains("Запас по времени"), "{timeout}");
        assert_eq!(assemble_with(Err(HostError::Cancelled)).unwrap_err(), pipeline::CANCELLED);
        let spawn = assemble_with(Err(HostError::Spawn("нет файла".into()))).unwrap_err();
        assert!(spawn.contains("должен быть в PATH"), "{spawn}");
        let failed = pipeline::host::CmdOutput { code: Some(1), stderr_tail: (1..=8).map(|i| format!("строка {i}")).collect(), elapsed: Duration::ZERO };
        let msg = assemble_with(Ok(failed)).unwrap_err();
        assert_eq!(msg, "ffmpeg завершился с ошибкой: строка 3 | строка 4 | строка 5 | строка 6 | строка 7 | строка 8");
        let ok = pipeline::host::CmdOutput { code: Some(0), stderr_tail: vec![], elapsed: Duration::ZERO };
        assert!(assemble_with(Ok(ok)).is_ok());
    }

    #[test]
    fn time_margin_scales_the_assemble_deadline() {
        let mut s = Settings::default();
        let base = deadlines(&s).ffmpeg_for(100.0);
        s.time_margin = 2.0;
        assert_eq!(deadlines(&s).ffmpeg_for(100.0), base * 2);
        s.time_margin = f32::NAN;
        assert_eq!(deadlines(&s).ffmpeg_for(100.0), base, "мусор в настройках не ломает дедлайны");
    }

    #[test]
    fn finished_attempt_leaves_an_outcome_line_and_the_next_one_a_separator() {
        let (store, id, dir) = running_job("outcome");
        store.append_log(id, "строка первой попытки").unwrap();
        store.set_stage(id, "Запись").unwrap();
        let host = FakeHost::new(Scenario::happy(2).console_never_up());
        let e = record_stage(&host, &request(&dir), &pipeline::Quiet).expect_err("консоль не открывается — запись падает");
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

    use pipeline::host::ProcInfo;

    fn proc(pid: u32, exe: &str) -> ProcInfo {
        ProcInfo { pid, exe_name: exe.to_owned() }
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
    fn precheck_failure_reaches_the_job_row_with_its_report() {
        let (store, id, _dir) = running_job("precheck");
        let s = healthy_settings("fail");
        let host = FakeHost::new(Scenario::happy(1).processes(vec![proc(100, "steam.exe"), proc(777, "cs2.exe")]));
        let host_ref: &dyn Host = &host;
        let inputs = precheck_inputs(&s, Demo::NotApplicable, None);
        let e = precheck_stage(host_ref, &inputs, &store, id, &pipeline::Quiet).expect_err("чужой CS2 останавливает запись");
        let f = e.fail.as_ref().expect("сбой типизирован");
        assert_eq!(f.code, FailCode::ForeignGameRunning);
        assert_eq!(f.state, RecordState::Precheck);
        finish(&store, id, &Err(e)).unwrap();

        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert_eq!(j.fail_code.as_deref(), Some("FOREIGN_GAME_RUNNING"));
        assert_eq!(j.fail_state.as_deref(), Some("precheck"));
        let report = Report::from_json(j.precheck.as_deref().expect("отчёт сохранён")).expect("отчёт разбирается");
        let item = report.items.iter().find(|i| i.id == ids::FOREIGN_GAME).expect("пункт чужого CS2");
        assert_eq!(item.level, Level::Fail);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&s.cs2_dir).parent().unwrap());
    }

    #[test]
    fn passing_precheck_clears_the_phase_and_keeps_the_report() {
        let (store, id, _dir) = running_job("precheck-ok");
        store.set_phase(id, "precheck").unwrap();
        let s = healthy_settings("ok");
        let ok = pipeline::host::CmdOutput { code: Some(0), stderr_tail: vec![], elapsed: Duration::ZERO };
        let host = FakeHost::new(Scenario::happy(1).program("ffmpeg", &["ffmpeg version 7.1.1 Copyright (c) the FFmpeg developers"], Ok(ok)));
        let host_ref: &dyn Host = &host;
        let inputs = precheck_inputs(&s, Demo::NotApplicable, None);
        let report = precheck_stage(host_ref, &inputs, &store, id, &pipeline::Quiet).expect("окружение исправно");
        assert!(!report.blocks(), "{report:?}");

        let j = store.job(id).unwrap().unwrap();
        assert_eq!(j.phase, "", "лестница спрятана до начала записи");
        assert_eq!(Report::from_json(j.precheck.as_deref().unwrap()), Some(report));
        let _ = std::fs::remove_dir_all(std::path::Path::new(&s.cs2_dir).parent().unwrap());
    }

    #[test]
    fn nvenc_probe_that_hangs_is_cut_by_its_deadline() {
        let host = FakeHost::new(Scenario::happy(1).run_result(Err(HostError::Timeout)));
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

    #[test]
    fn afxerror_ignore_from_settings_reaches_the_record_request() {
        let mut s = Settings::default();
        let rj = sample_record_json(2, PathBuf::from("match.dem"), "h".into());
        assert!(record_request(&s, Path::new("out"), &rj, 7, None).afxerror_ignore.is_empty(), "по умолчанию список пуст");
        s.afxerror_ignore = vec!["shader cache".into()];
        assert_eq!(record_request(&s, Path::new("out"), &rj, 7, None).afxerror_ignore, vec!["shader cache".to_owned()]);
    }

    #[test]
    fn resume_launches_at_the_recorded_frame_not_the_current_setting() {
        let mut s = Settings::default();
        s.resolution = 1080;
        assert_eq!(s.frame_size(), (1920, 1080));
        let rj = sample_record_json(2, PathBuf::from("match.dem"), "h".into());
        let req = record_request(&s, Path::new("out"), &rj, 7, None);
        assert_eq!(req.launch.size, (1280, 720), "кадр — из record.json");
        assert_eq!((req.launch.nonce.as_str(), req.launch.demo_name.as_str()), ("ab12cd34", "cs2cinema_match_hero"));
        assert_eq!(req.takes, rj.take_plans());
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
        let hash = plan_hash(&dir, nonce);
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

    #[test]
    fn set_aside_clips_never_overwrites_an_earlier_set_aside_folder() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(set_aside_clips(&dir).unwrap(), None, "нет clips — нечего откладывать");
        let mut seen = std::collections::HashSet::new();
        for i in 0..3 {
            std::fs::create_dir_all(dir.join("clips")).unwrap();
            std::fs::write(dir.join("clips/marker.txt"), i.to_string()).unwrap();
            let to = set_aside_clips(&dir).unwrap().expect("clips отложены");
            assert!(to.file_name().unwrap().to_string_lossy().starts_with("clips_old_"));
            assert!(seen.insert(to.clone()), "имя {to:?} уже занято");
            assert_eq!(std::fs::read_to_string(to.join("marker.txt")).unwrap(), i.to_string());
            assert!(!dir.join("clips").exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_json_written_after_planning_passes_its_own_check() {
        let dir = plan_dir("record-json", "ab12cd34", 1000);
        let rj_sample = sample_record_json(2, PathBuf::from("match.dem"), String::new());
        let plan = pipeline::Plan {
            timeline: director::Timeline { map: "de_nuke".into(), tick_rate: 64.0, server_start_tick: 0, shots: vec![] },
            out_dir: dir.clone(),
            ffmpeg_args: rj_sample.ffmpeg_args.clone(),
            video_secs: 10.0,
            takes: two_takes(),
            nonce: "ab12cd34".into(),
        };
        let mut s = Settings::default();
        s.resolution = 1080;
        let rj = record_json_for(&plan, Path::new("C:/demos/match.dem"), 4096, "cs2cinema_match_hero", &s, true).expect("record.json собирается из плана");
        rj.save(&dir).unwrap();

        let loaded = RecordJson::load(&dir).expect("record.json проходит собственную проверку");
        assert_eq!(loaded, rj);
        assert_eq!(loaded.nonce, "ab12cd34");
        assert_eq!(loaded.take_plans(), two_takes());
        assert_eq!(loaded.ffmpeg_args, plan.ffmpeg_args);
        assert_eq!((loaded.frame, loaded.fps, loaded.take_nvenc, loaded.demo_len), ((1920, 1080), s.fps, true, 4096));
        assert_eq!(loaded.plan_hash, plan_hash(&dir, "ab12cd34"));
        assert_eq!(loaded.demo_name, "cs2cinema_match_hero");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_record_json_is_removed_before_planning() {
        let dir = plan_dir("stale-record", "ab12cd34", 1000);
        sample_record_json(1, PathBuf::from("match.dem"), "h".into()).save(&dir).unwrap();
        assert!(RecordJson::load(&dir).is_ok());
        begin_plan(&dir).unwrap();
        let e = RecordJson::load(&dir).expect_err("устаревшего record.json больше нет");
        assert!(e.contains("record.json"), "{e}");
        // повторный вызов и несуществующая папка — не ошибки
        begin_plan(&dir).unwrap();
        begin_plan(&dir.join("нет такой папки")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
