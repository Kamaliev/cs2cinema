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
    time::{Duration, SystemTime},
};

use pipeline::{PlanOptions, Progress, Source, Stage};
use store::{Job, JobKind, JobOutcome, JobSource, Store};

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
}

/// Запускает следующую задачу из очереди, если сейчас ничего не выполняется.
pub fn run_next(app: &mut App) {
    if app.jobs.running.is_some() || app.status.busy {
        return;
    }
    let Ok(Some(job)) = app.store.next_queued() else { return };
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
        let _ = match &result {
            Ok(o) => store.finish_job(job.id, Ok(o)),
            Err(e) if e == pipeline::CANCELLED => store.cancel_job(job.id),
            Err(e) => store.finish_job(job.id, Err(e)),
        };
        rep.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result) });
    });
    app.refresh_jobs();
}

/// Весь путь задачи: демка -> план -> (запись -> склейка); исполняется в фоновом потоке.
fn run(job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, String> {
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
    let o = PlanOptions { fps: s.fps, chronological: s.chronological, flybys: s.flybys, walls: s.walls, ..Default::default() };
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
    // дубли HLAE нумерует по порядку записи, а шоты пишутся в порядке показа: последний дубль — последний шот
    let last = plan.timeline.shots.len().saturating_sub(1);
    // старые дубли сбили бы нумерацию takeNNNN — откладываем в сторону (запись дорогая, не удаляем)
    let clips = out_dir.join("clips");
    if clips.is_dir() {
        let stamp = std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        std::fs::rename(&clips, out_dir.join(format!("clips_old_{stamp}"))).map_err(|e| format!("не удалось отложить прошлые дубли: {e}"))?;
    }
    let demo_name = format!("cs2cinema_{}", pipeline::console_name(&a.label));
    launch::install_files(&s.cs2_dir, &out_dir, &demo_name, |path| a.write_demo(path))?;

    let since = SystemTime::now();
    p.log("запускаю CS2 через HLAE…");
    let _launcher = launch::launch(&s.hlae_dir, &s.cs2_dir, &demo_name)?;
    let cancelled = || p.cancelled();
    // отмена во время записи — закрываем игру, иначе она так и будет писать дубли
    let recording = |r: Result<(), String>| {
        if r.as_ref().is_err_and(|e| e == pipeline::CANCELLED) {
            launch::kill_game();
        }
        r
    };
    p.log(&format!("жду загрузки демки ({} с)…", s.load_wait_secs));
    recording(sleep_unless(Duration::from_secs(s.load_wait_secs as u64), &cancelled))?;
    match launch::send_console(&["exec highlights"], Duration::from_secs(90)) {
        Ok(()) => p.log("скрипт запущен, идёт запись…"),
        Err(e) => p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights")),
    }
    let log = |m: &str| p.log(m);
    recording(launch::wait_for_clips(&out_dir, last, since, &launch::game_alive, &cancelled, &log))?;

    p.stage(Stage::Assemble);
    p.log("склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.current_dir(&out_dir).args(&plan.ffmpeg_args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    launch::no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("не удалось запустить ffmpeg (он должен быть в PATH): {e}"))?;
    let status = loop {
        if cancelled() {
            let _ = child.kill();
            return Err(pipeline::CANCELLED.to_owned());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            Err(e) => return Err(format!("ffmpeg: {e}")),
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        let tail: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(format!("ffmpeg завершился с ошибкой: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    p.log(&format!("ролик готов: {}", out.display()));
    ui::open_path(&out);
    Ok(JobOutcome { out_dir, result: out, shots, secs })
}

/// Спит `d`, просыпаясь на отмену.
fn sleep_unless(d: Duration, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let until = std::time::Instant::now() + d;
    while std::time::Instant::now() < until {
        if cancelled() {
            return Err(pipeline::CANCELLED.to_owned());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}
