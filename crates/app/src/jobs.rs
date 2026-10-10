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
    precheck::{self, Demo, Inputs, Level, Report, ids, space},
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

/// Запрос записи из настроек и плана. Ограничения путей и имя демки проверяет вызывающий.
pub fn record_request(s: &Settings, out_dir: &Path, demo_name: &str, plan: &pipeline::Plan, job_id: i64) -> RecordRequest {
    RecordRequest {
        out_dir: out_dir.to_path_buf(),
        launch: LaunchSpec {
            hlae_dir: s.hlae_dir.clone().into(),
            cs2_exe: launch::cs2_exe(&s.cs2_dir),
            demo_name: demo_name.to_owned(),
            size: s.frame_size(),
            nonce: plan.nonce.clone(),
            console_log: Some(Path::new(&s.cs2_dir).join("game/csgo/console.log")),
        },
        takes: plan.takes.clone(),
        deadlines: deadlines(s),
        header: format!("задача #{job_id} · {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")),
        afxerror_ignore: s.afxerror_ignore.clone(),
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
        let rep = JobReporter::new(store.clone(), job.id, ui, cancel, attempt);
        let result = run(&job, &s, &store, &rep);
        // длительность последнего этапа — до итога попытки (успех, сбой и отмена)
        rep.close_stage();
        let _ = finish(&store, job.id, &result);
        rep.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result).map_err(|e| e.message) });
    });
    app.refresh_jobs();
}

/// Весь путь задачи: демка -> план -> (запись -> склейка); исполняется в фоновом потоке.
fn run(job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
    let host = pipeline::host::default_host();
    // предпроверка окружения — до скачивания, разбора и плана (PRE-02): неисправное окружение видно за секунды
    let inputs = precheck_inputs(s, job_demo(job, store), store.last_good().ok().flatten());
    let mut report = if needs_precheck(job.kind) { Some(precheck_stage(&*host, &inputs, store, job.id, p)?) } else { None };
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
        let check = launch::nvenc_check(&*host, Path::new("ffmpeg"), probe_for, &cancel);
        take_nvenc = launch::hlae_ffmpeg(&s.hlae_dir).is_some_and(|f| launch::has_nvenc(&*host, &f, probe_for, &cancel));
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

    p.stage(Stage::Record);
    let clips = out_dir.join("clips");
    // дубли этого же плана уже есть (например, упала склейка) — не пишем заново;
    // ожидаемый набор дублей берём из плана, а не считаем номера сами
    let hash = plan_hash(&out_dir, &plan.nonce);
    let recorded = std::fs::read_to_string(clips.join("plan.hash")).is_ok_and(|h| h == hash)
        && plan.takes.iter().all(|t| renderer::TakePlan::FILES.iter().all(|f| clips.join(t.dir_name()).join(f).is_file()));
    if recorded {
        p.log("дубли этого плана уже записаны — сразу склеиваю");
    } else {
        // второй проход места по плану (D-03): сумма игровых секунд дублей, размер кадра и демки; до запуска игры
        if let Some(report) = report.as_mut() {
            space_pass(&*host, &inputs, space::estimate(&plan.takes, s.frame_size(), a.demo_len()), report, store, job.id, p)?;
        }
        // старые дубли сбили бы нумерацию takeNNNN — откладываем в сторону (запись дорогая, не удаляем)
        if clips.is_dir() {
            let stamp = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            std::fs::rename(&clips, out_dir.join(format!("clips_old_{stamp}"))).map_err(|e| format!("не удалось отложить прошлые дубли: {e}"))?;
        }
        let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
        let installed = launch::install_files(&s.cs2_dir, &out_dir, &demo_name, |path| a.write_demo(path));
        // игру закрывает сама запись при любом выходе, в том числе при отмене
        let recorded_run = installed.map_err(RunError::from).and_then(|()| {
            let req = record_request(s, &out_dir, &demo_name, &plan, job.id);
            p.log(&format!("запас по времени ×{:.2}", s.time_margin));
            record_stage(&*host, &req, p)
        });
        // папка игры чистая при любом исходе — успех, сбой, отмена (D-05, D-06, TMO-06): запись вернулась
        // только после подтверждённого закрытия игры, поэтому копия демки уже никем не занята
        for note in launch::cleanup_installed(&s.cs2_dir, &out_dir) {
            p.log(&note);
        }
        let rep = recorded_run?;
        let _ = std::fs::write(clips.join("plan.hash"), &hash);
        p.log(&format!("запись заняла {:.0} с", rep.elapsed.as_secs_f32()));
    }

    p.stage(Stage::Assemble);
    p.log("склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    if let Err(e) = run_ffmpeg(&*host, &out_dir, &plan.ffmpeg_args, plan.video_secs, deadlines(s).ffmpeg_for(plan.video_secs), p) {
        // недописанный ролик не должен выглядеть результатом (Pitfall 11)
        let _ = std::fs::remove_file(&out);
        return Err(e.into());
    }
    p.log(&format!("ролик готов: {}", out.display()));
    // пара «сборка CS2 / версия HLAE» после удачной задачи, в которой шла запись (D-04, DIF-01)
    if let (false, Some(r)) = (recorded, report.as_ref()) {
        if let (Some(build), Some(hlae)) = (&r.cs2_build, &r.hlae_version) {
            let _ = store.set_last_good(build, hlae);
        }
    }
    ui::open_path(&out);
    Ok(JobOutcome { out_dir, result: out, shots, secs })
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

    #[test]
    fn afxerror_ignore_from_settings_reaches_the_record_request() {
        let mut s = Settings::default();
        let plan = pipeline::Plan {
            timeline: director::Timeline { map: "de_nuke".into(), tick_rate: 64.0, server_start_tick: 0, shots: vec![] },
            out_dir: std::path::PathBuf::new(),
            ffmpeg_args: vec![],
            video_secs: 0.0,
            takes: two_takes(),
            nonce: "ab12cd34".into(),
        };
        assert!(record_request(&s, Path::new("out"), "cs2cinema_m", &plan, 7).afxerror_ignore.is_empty(), "по умолчанию список пуст");
        s.afxerror_ignore = vec!["shader cache".into()];
        assert_eq!(record_request(&s, Path::new("out"), "cs2cinema_m", &plan, 7).afxerror_ignore, vec!["shader cache".to_owned()]);
    }
}
