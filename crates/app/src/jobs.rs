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
    // NVENC: для склейки — у ffmpeg из PATH, для дублей — у ffmpeg, которым пишет HLAE
    let (mut nvenc, mut take_nvenc) = (false, false);
    if s.gpu_encode && job.kind != JobKind::Plan {
        let check = launch::nvenc_check(Path::new("ffmpeg"));
        nvenc = check.is_ok();
        take_nvenc = launch::hlae_ffmpeg(&s.hlae_dir).is_some_and(|f| launch::has_nvenc(&f));
        p.log(&format!(
            "кодирование: дубли — {}, склейка — {}",
            if take_nvenc { "NVENC" } else { "процессор" },
            if nvenc { "NVENC" } else { "процессор" }
        ));
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
    // дубли HLAE нумерует по порядку записи: последний записанный — take{shots-1}
    let last = plan.timeline.shots.len().saturating_sub(1);
    let clips = out_dir.join("clips");
    // дубли этого же плана уже есть (например, упала склейка) — не пишем заново
    let hash = plan_hash(&out_dir);
    let recorded = std::fs::read_to_string(clips.join("plan.hash")).is_ok_and(|h| h == hash)
        && (0..=last).all(|i| ["video.mp4", "audio.wav"].iter().all(|f| clips.join(format!("take{i:04}")).join(f).is_file()));
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
        let started = std::time::Instant::now();
        // отмена во время записи — закрываем игру, иначе она так и будет писать дубли
        let r = record(s, &out_dir, &demo_name, last, &plan.nonce, p);
        if r.as_ref().is_err_and(|e| e == pipeline::CANCELLED) {
            launch::kill_game();
        }
        r?;
        let _ = std::fs::write(clips.join("plan.hash"), &hash);
        p.log(&format!("запись заняла {:.0} с", started.elapsed().as_secs_f32()));
    }

    p.stage(Stage::Assemble);
    p.log("склеиваю ролик через ffmpeg…");
    let out = out_dir.join("highlights.mp4");
    run_ffmpeg(&out_dir, &plan.ffmpeg_args, plan.video_secs, p)?;
    p.log(&format!("ролик готов: {}", out.display()));
    ui::open_path(&out);
    Ok(JobOutcome { out_dir, result: out, shots, secs })
}

/// Потолок ожидания загрузки демки (большие демки грузятся дольше минуты).
const LOAD_CAP: Duration = Duration::from_secs(150);
/// Если за это время после `exec highlights` не появился первый дубль — повторяем команду.
const EXEC_RETRY: Duration = Duration::from_secs(30);
const EXEC_RETRIES: u32 = 4;
/// Как часто пробовать подключиться к консоли, если с первого раза не вышло.
const RECONNECT_EVERY: Duration = Duration::from_secs(10);

/// Запись: CS2 через HLAE, скрипт `highlights` как только демка пошла, ждём последний дубль и закрываем игру.
fn record(s: &Settings, out_dir: &Path, demo_name: &str, last: usize, nonce: &str, p: &JobReporter) -> Result<(), String> {
    let since = SystemTime::now();
    p.log("запускаю CS2 через HLAE…");
    // свой порт на каждый запуск: фиксированный может держать «мёртвый» сокет прошлой игры
    let port = launch::free_port();
    let _launcher = launch::launch(&s.hlae_dir, &s.cs2_dir, demo_name, s.frame_size(), port)?;
    let cancelled = || p.cancelled();
    let started = std::time::Instant::now();
    let load_wait = Duration::from_secs(s.load_wait_secs as u64);
    // свои строки консоли (не эхо отправленной команды)
    let marker = |l: &str, m: &str| l.contains(m) && !l.contains("echo") && !l.contains("addAtTick");
    // метка конца дубля с nonce этого плана: чужие запуски и старые строки консоли не совпадут
    let take_end = renderer::marker(nonce, "TAKE_END ");
    // большая демка может грузиться дольше настройки — маркер ждём до жёсткого потолка,
    // а настройка лишь говорит, когда перестать молчать
    let load_cap = load_wait.max(LOAD_CAP);
    let console = match launch::Console::connect(port, load_cap, &cancelled) {
        Ok(mut c) => {
            // mirv_cmd срабатывает только на тиках идущей демки — маркер значит «демка загрузилась»
            let _ = c.send("mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C_READY\"");
            let mut ready = false;
            let mut warned = false;
            while !ready && started.elapsed() < load_cap {
                if cancelled() {
                    return Err(pipeline::CANCELLED.to_owned());
                }
                ready = c.drain().iter().any(|l| marker(l, "CS2C_READY"));
                if !ready && !warned && started.elapsed() >= load_wait {
                    warned = true;
                    p.log(&format!("демка не загрузилась за {} с — жду ещё…", load_wait.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            p.log(&format!("демка {} за {:.0} с", if ready { "загрузилась" } else { "должна была загрузиться" }, started.elapsed().as_secs_f32()));
            match c.send("exec highlights") {
                Ok(()) => p.log("скрипт запущен, идёт запись…"),
                Err(e) => p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights")),
            }
            Some(c)
        }
        Err(e) if e == pipeline::CANCELLED => return Err(e),
        Err(e) => {
            p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights"));
            None
        }
    };
    let console = std::cell::RefCell::new(console);
    let console_ref = &console;
    // прогресс по маркерам дублей из консоли; если первый дубль так и не начался (скрипт ушёл
    // до загрузки демки — игра просто играет её с начала), повторяем exec highlights
    let done = std::cell::Cell::new(false);
    let any_take = std::cell::Cell::new(false);
    let mut exec_at = std::time::Instant::now();
    let mut retries = 0u32;
    let first_take = out_dir.join("clips").join("take0000");
    let mut reconnect_at = std::time::Instant::now();
    let mut finished = move || {
        // консоль не открылась с первого раза — пробуем ещё, пока игра жива и дубли не пошли
        if console_ref.borrow().is_none() && !any_take.get() && reconnect_at.elapsed() > RECONNECT_EVERY {
            reconnect_at = std::time::Instant::now();
            if let Some(mut c) = launch::Console::try_connect(port) {
                match c.send("exec highlights") {
                    Ok(()) => p.log("консоль игры открылась — отправил exec highlights"),
                    Err(e) => p.log(&format!("консоль игры: {e}")),
                }
                exec_at = std::time::Instant::now();
                *console_ref.borrow_mut() = Some(c);
            }
        }
        if let Some(c) = console_ref.borrow().as_ref() {
            for l in c.drain() {
                let take = l.split(&take_end).nth(1).and_then(|r| r.split_whitespace().next()?.parse::<usize>().ok());
                if let Some(n) = take.filter(|_| marker(&l, take_end.trim())) {
                    any_take.set(true);
                    p.log(&format!("записан дубль {}/{}", n + 1, last + 1));
                    done.set(done.get() || n >= last);
                }
            }
        }
        if !any_take.get() && first_take.is_dir() {
            any_take.set(true);
        }
        if !any_take.get() && exec_at.elapsed() > EXEC_RETRY && retries < EXEC_RETRIES {
            if let Some(c) = console_ref.borrow_mut().as_mut() {
                retries += 1;
                exec_at = std::time::Instant::now();
                match c.send("exec highlights") {
                    Ok(()) => p.log(&format!("запись не началась — повторяю exec highlights ({retries}/{EXEC_RETRIES})")),
                    Err(e) => p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights")),
                }
            }
        }
        done.get()
    };
    let r = launch::wait_for_clips(out_dir, last, since, &mut finished, &launch::game_alive, &cancelled);
    // игра больше не нужна — освобождаем видеокарту для склейки
    if let Some(mut c) = console.into_inner() {
        let _ = c.send("quit");
    }
    r
}

/// Отпечаток скриптов записи (`highlights.cfg`, `cs2c_*`): совпал — дубли в `clips/` от этого же плана.
fn plan_hash(out_dir: &Path) -> String {
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
        for b in n.bytes().chain(std::fs::read(out_dir.join(&n)).unwrap_or_default()) {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Склейка: ffmpeg в `dir` с прогрессом в лог. stdout (прогресс) и stderr (ошибки) читаются
/// в своих потоках — непрочитанный pipe заполняется, и ffmpeg встаёт намертво.
fn run_ffmpeg(dir: &Path, args: &[String], secs: f32, p: &JobReporter) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("ffmpeg");
    cmd.current_dir(dir)
        .args(["-hide_banner", "-nostats", "-loglevel", "error", "-progress", "pipe:1"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    launch::no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("не удалось запустить ffmpeg (он должен быть в PATH): {e}"))?;
    // прогресс: `out_time_us=…` в микросекундах готового ролика
    let done_us = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let stdout = child.stdout.take().map(|o| {
        let done_us = done_us.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(o).lines().map_while(Result::ok) {
                if let Some(v) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    done_us.store(v, Ordering::Relaxed);
                }
            }
        })
    });
    let stderr = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            s
        })
    });
    let started = std::time::Instant::now();
    let mut logged = 0u32;
    let status = loop {
        if p.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(pipeline::CANCELLED.to_owned());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            Err(e) => return Err(format!("ffmpeg: {e}")),
        }
        // раз в 10 с — сколько склеено
        if started.elapsed().as_secs() as u32 / 10 > logged && secs > 0.0 {
            logged = started.elapsed().as_secs() as u32 / 10;
            let pct = (done_us.load(Ordering::Relaxed) as f32 / 1e6 / secs * 100.0).min(100.0);
            p.log(&format!("склейка: {pct:.0}%"));
        }
    };
    if let Some(t) = stdout {
        let _ = t.join();
    }
    let err = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    if !status.success() {
        let tail: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(format!("ffmpeg завершился с ошибкой: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")));
    }
    p.log(&format!("склейка заняла {:.0} с", started.elapsed().as_secs_f32()));
    Ok(())
}
