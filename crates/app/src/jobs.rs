//! Выполнение задач на монтаж: очередь из базы, по одной (CS2 пишет только одну), с отменой.
//!
//! Задача самодостаточна: откуда демка и про кого ролик она знает сама, поэтому разбор
//! делается заново в фоновом потоке — и задачу можно повторить после перезапуска.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use pipeline::{
    Deadlines, FailCode, PlanOptions, Progress, RecordFailure, RecordReport, RecordRequest, Source, Stage,
    host::{CmdSpec, Host, HostError, LaunchSpec},
};
use store::{Job, JobFailure, JobKind, JobOutcome, JobSource, Store};

use crate::{
    app::{App, Msg, Reporter},
    launch,
    settings::Settings,
    ui,
};

/// Токен отмены: кнопка в окне ставит флаг, исполнитель проверяет его между этапами
/// и во всех ожиданиях (загрузка игры, запись, ffmpeg).
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
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
}

impl Progress for JobReporter {
    fn stage(&self, s: Stage) {
        let _ = self.store.set_stage(self.id, s.title());
        self.ui.send(Msg::Stage(s));
    }
    fn log(&self, m: &str) {
        let _ = self.store.append_log(self.id, &format!("{}  {m}", chrono::Local::now().format("%H:%M:%S")));
        self.ui.send(Msg::Log(m.to_owned()));
    }
    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    /// Состояние записи — в строку задачи (для лестницы в карточке), любое событие — в журнал.
    /// Подсостояние не пишем в `stage`: полоска прогресса сопоставляет `stage` с `Stage::ALL`.
    fn event(&self, e: &pipeline::Event) {
        if let pipeline::Event::State { state, .. } = e {
            let _ = self.store.set_phase(self.id, state.code());
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

/// Итог задачи в строку базы: отмена остаётся отменой, типизированный сбой — со всеми полями.
pub fn finish(store: &Store, id: i64, result: &Result<JobOutcome, RunError>) -> store::Result<()> {
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
        afxerror_ignore: Vec::new(),
    }
}

/// Запись дублей через `pipeline::record`. Отмена становится сентинелом `CANCELLED`, остальные
/// сбои несут типизированное описание до строки задачи.
pub fn record_stage(host: &dyn Host, req: &RecordRequest, p: &dyn Progress) -> Result<RecordReport, RunError> {
    pipeline::record(host, req, p)
        .map_err(|f| if f.code == FailCode::Cancelled { RunError::from(pipeline::CANCELLED) } else { RunError { message: f.message.clone(), fail: Some(f) } })
}

/// Запускает следующую задачу из очереди, если сейчас ничего не выполняется.
pub fn run_next(app: &mut App) {
    if app.jobs.running.is_some() || app.status.busy {
        return;
    }
    let Ok(Some(job)) = app.store.next_queued() else {
        return;
    };
    if let Err(e) = app.store.start_job(job.id) {
        app.status.finish(Err(e));
        return;
    }
    let cancel = CancelToken::default();
    app.jobs.running = Some((job.id, cancel.clone()));
    app.status.start(&format!("Задача #{}: {} · {}", job.id, job.label, job.kind.title()));
    let (store, s, ui) = (app.store.clone(), app.s.clone(), app.reporter());
    std::thread::spawn(move || {
        let rep = JobReporter { store: store.clone(), id: job.id, ui, cancel };
        let result = run(&job, &s, &store, &rep);
        let _ = finish(&store, job.id, &result);
        rep.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result).map_err(|e| e.message) });
    });
    app.refresh_jobs();
}

/// Весь путь задачи: демка -> план -> (запись -> склейка); исполняется в фоновом потоке.
fn run(job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
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
        let check = launch::nvenc_check(Path::new("ffmpeg"));
        nvenc = check.is_ok();
        take_nvenc = launch::hlae_ffmpeg(&s.hlae_dir).is_some_and(|f| launch::has_nvenc(&f));
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
    launch::check_paths(&s.hlae_dir, &s.cs2_dir)?;
    if !renderer::console_safe(&pipeline::absolute_slash(&out_dir)) {
        return Err("в пути результатов есть пробелы — HLAE не загрузит campath; выберите папку без пробелов".into());
    }

    p.stage(Stage::Record);
    let host = pipeline::host::default_host();
    let clips = out_dir.join("clips");
    // дубли этого же плана уже есть (например, упала склейка) — не пишем заново;
    // ожидаемый набор дублей берём из плана, а не считаем номера сами
    let hash = plan_hash(&out_dir, &plan.nonce);
    let recorded = std::fs::read_to_string(clips.join("plan.hash")).is_ok_and(|h| h == hash)
        && plan.takes.iter().all(|t| renderer::TakePlan::FILES.iter().all(|f| clips.join(t.dir_name()).join(f).is_file()));
    if recorded {
        p.log("дубли этого плана уже записаны — сразу склеиваю");
    } else {
        // старые дубли сбили бы нумерацию takeNNNN — откладываем в сторону (запись дорогая, не удаляем)
        if clips.is_dir() {
            let stamp = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            std::fs::rename(&clips, out_dir.join(format!("clips_old_{stamp}"))).map_err(|e| format!("не удалось отложить прошлые дубли: {e}"))?;
        }
        let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
        launch::install_files(&s.cs2_dir, &out_dir, &demo_name, |path| a.write_demo(path))?;
        // игру закрывает сама запись при любом выходе, в том числе при отмене
        let req = record_request(s, &out_dir, &demo_name, &plan, job.id);
        p.log(&format!("запас по времени ×{:.2}", s.time_margin));
        let rep = record_stage(&*host, &req, p)?;
        let _ = std::fs::write(clips.join("plan.hash"), &hash);
        p.log(&format!("запись заняла {:.0} с", rep.elapsed.as_secs_f32()));
    }

    p.stage(Stage::Assemble);
    p.log("склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    run_ffmpeg(&*host, &out_dir, &plan.ffmpeg_args, plan.video_secs, deadlines(s).ffmpeg_for(plan.video_secs), p)?;
    p.log(&format!("ролик готов: {}", out.display()));
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
    fn plain_error_keeps_the_old_failed_row() {
        let (store, id, _dir) = running_job("plain");
        finish(&store, id, &Err(RunError::from("игрок не найден"))).unwrap();
        let j = store.job(id).unwrap().unwrap();
        assert_eq!((j.status, j.error.as_deref(), j.fail_code), (JobStatus::Failed, Some("игрок не найден"), None));
    }
}
