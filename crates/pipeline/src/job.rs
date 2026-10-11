//! Задача на монтаж целиком: предпроверка → демка → план → запись → склейка, и «Повторить». Один код для приложения
//! и `cs2-cli`; без базы и настроек приложения — входы приходят в `JobSpec`, итоги шагов уходят через
//! `Progress::artifact`, а вызывающий сам решает, куда их сохранить.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use renderer::TakePlan;

use crate::{
    Artifact, Deadlines, Event, FailCode, Progress, RecordFailure, RecordReport, RecordRequest,
    RecordState, RunError, Stage,
    assemble::assemble_stage,
    gamedir,
    host::{Host, LaunchSpec},
    manifest::{EnvSnapshot, Manifest, RecordJson, TakeStatus},
    precheck::{self, Demo, Inputs, Level, Report, ids, space},
    record::{Verify, failure},
};

/// Настройки задачи: пути и параметры, нужные шагам записи. Вызывающий собирает их из своих настроек.
#[derive(Clone)]
pub struct JobConfig {
    /// Папка с `HLAE.exe`.
    pub hlae_dir: PathBuf,
    /// Папка установки CS2 (внутри неё `game/bin/win64/cs2.exe`); пустой путь — «неизвестна».
    pub cs2_dir: PathBuf,
    /// Корневая папка результатов.
    pub out_root: PathBuf,
    /// Ключ FACEIT Data API; в `Debug` не выводится.
    pub faceit_key: String,
    pub fps: u32,
    pub chronological: bool,
    pub flybys: bool,
    pub walls: bool,
    /// Подробный ролик: весь KAST и все ракурсы.
    pub detailed: bool,
    /// Кодировать на видеокарте NVIDIA (NVENC), если она есть.
    pub gpu_encode: bool,
    /// Размер кадра (ширина, высота).
    pub frame: (u32, u32),
    /// Запас по времени: множитель всех дедлайнов записи и склейки.
    pub time_margin: f32,
    /// Подстроки строк `AFXERROR`, которые не останавливают запись.
    pub afxerror_ignore: Vec<String>,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl Default for JobConfig {
    fn default() -> Self {
        JobConfig {
            hlae_dir: PathBuf::new(),
            cs2_dir: PathBuf::new(),
            out_root: PathBuf::new(),
            faceit_key: String::new(),
            fps: 60,
            chronological: false,
            flybys: true,
            walls: true,
            detailed: false,
            gpu_encode: true,
            frame: (1280, 720),
            time_margin: 1.0,
            afxerror_ignore: Vec::new(),
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }
}

impl std::fmt::Debug for JobConfig {
    /// Ключ FACEIT не выводится: конфиг может попасть в журналы и сообщения об ошибках (T-04-10).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobConfig")
            .field("hlae_dir", &self.hlae_dir)
            .field("cs2_dir", &self.cs2_dir)
            .field("out_root", &self.out_root)
            .field(
                "faceit_key",
                &if self.faceit_key.trim().is_empty() {
                    "пусто"
                } else {
                    "задан"
                },
            )
            .field("fps", &self.fps)
            .field("chronological", &self.chronological)
            .field("flybys", &self.flybys)
            .field("walls", &self.walls)
            .field("detailed", &self.detailed)
            .field("gpu_encode", &self.gpu_encode)
            .field("frame", &self.frame)
            .field("time_margin", &self.time_margin)
            .field("afxerror_ignore", &self.afxerror_ignore)
            .field("ffmpeg", &self.ffmpeg)
            .field("ffprobe", &self.ffprobe)
            .finish()
    }
}

/// Откуда задача берёт демку.
#[derive(Debug, Clone, PartialEq)]
pub enum DemoSource {
    File(PathBuf),
    /// Ссылка или id матча FACEIT.
    Faceit(String),
}

/// Всё, что задаче нужно знать от вызывающего: что делать, про кого, и то, что он помнит с прошлых запусков.
#[derive(Debug, Clone)]
pub struct JobSpec {
    pub job_id: i64,
    /// Номер попытки (с единицы) — в манифест дублей.
    pub attempt: u32,
    /// Записывать ли в игре (иначе только план).
    pub record: bool,
    /// «Повторить»: по `record.json` из папки задачи, без скачивания, разбора и плана.
    pub resume: bool,
    pub source: DemoSource,
    pub hero_steam: u64,
    pub hero_name: String,
    /// Уже скачанная демка этого матча, если вызывающий её помнит.
    pub known_demo: Option<PathBuf>,
    /// Пара (сборка CS2, версия HLAE) последней удачной записи.
    pub last_good: Option<(String, String)>,
    /// Папка задачи, если она уже известна (при повторе).
    pub out_dir: Option<PathBuf>,
    pub cfg: JobConfig,
}

/// Итог задачи: папка, главный файл результата, число шотов и длина ролика по плану.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub out_dir: PathBuf,
    pub result: PathBuf,
    pub shots: u32,
    pub secs: f32,
}

/// Дедлайны записи и склейки с учётом «Запаса по времени».
pub fn deadlines(cfg: &JobConfig) -> Deadlines {
    Deadlines::default().scaled(cfg.time_margin)
}

/// Что проверять: пути из конфига, дедлайны с «Запасом по времени», пара последней удачной записи.
/// Один построитель для задачи и для кнопки проверки в настройках (D-01).
pub fn precheck_inputs(cfg: &JobConfig, demo: Demo, last_good: Option<(String, String)>) -> Inputs {
    Inputs {
        hlae_dir: cfg.hlae_dir.clone(),
        cs2_dir: cfg.cs2_dir.clone(),
        out_root: cfg.out_root.clone(),
        demo,
        last_good,
        deadlines: deadlines(cfg),
        ffmpeg: cfg.ffmpeg.clone(),
        ffprobe: cfg.ffprobe.clone(),
    }
}

/// Предпроверка первым шагом записи (PRE-02, D-01, D-02): отчёт уходит вызывающему через `Artifact::Precheck`,
/// `fail` останавливает задачу типизированным сбоем на ступени «предпроверка», `warn` остаются в журнале и запись идёт дальше.
pub fn precheck_stage(
    host: &dyn Host,
    inputs: &Inputs,
    p: &dyn Progress,
) -> Result<Report, RunError> {
    p.event(&Event::State {
        state: RecordState::Precheck,
        take: None,
    });
    let t0 = host.now();
    let report = precheck::run(host, inputs, p)?;
    let failure = report.failure(host.now().saturating_sub(t0));
    p.artifact(&Artifact::Precheck {
        json: report.to_json(),
        passed: failure.is_none(),
    });
    match failure {
        Some(f) => Err(RunError {
            message: f.message.clone(),
            fail: Some(f),
        }),
        None => Ok(report),
    }
}

/// Второй проход оценки места по плану (D-03, PRE-06): пункты места в отчёте заменяются, нехватка места
/// (`fail`) останавливает задачу до запуска игры, `warn` и `fail` попадают в журнал.
pub fn space_pass(
    host: &dyn Host,
    inputs: &Inputs,
    need: space::Need,
    report: &mut Report,
    p: &dyn Progress,
) -> Result<(), RunError> {
    precheck::check_space(host, inputs, need, report);
    let failure = report.failure(Duration::ZERO);
    p.artifact(&Artifact::Precheck {
        json: report.to_json(),
        passed: failure.is_none(),
    });
    for item in report
        .items
        .iter()
        .filter(|i| matches!(i.id.as_str(), ids::SPACE_RESULTS | ids::SPACE_CS2))
    {
        match item.level {
            Level::Ok => {}
            Level::Warn => p.log(&format!("предпроверка: ⚠ {}", item.message)),
            Level::Fail => p.log(&format!("предпроверка: ✖ {}", item.message)),
        }
    }
    match failure {
        Some(f) => Err(RunError {
            message: f.message.clone(),
            fail: Some(f),
        }),
        None => Ok(()),
    }
}

/// Запрос записи из конфига и `record.json`: кадр, метка, имя демки и дубли берутся из `record.json`, а не из
/// текущего конфига — старые скрипты и граф склейки нельзя смешивать с новым разрешением (Pattern 5).
pub fn record_request(
    cfg: &JobConfig,
    out_dir: &Path,
    rj: &RecordJson,
    job_id: i64,
    verify: Option<Verify>,
) -> RecordRequest {
    RecordRequest {
        out_dir: out_dir.to_path_buf(),
        launch: LaunchSpec {
            hlae_dir: cfg.hlae_dir.clone(),
            cs2_exe: gamedir::cs2_exe(&cfg.cs2_dir),
            demo_name: rj.demo_name.clone(),
            size: rj.frame,
            nonce: rj.nonce.clone(),
            console_log: Some(cfg.cs2_dir.join("game/csgo/console.log")),
        },
        takes: rj.take_plans(),
        deadlines: deadlines(cfg),
        header: format!(
            "задача #{job_id} · {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ),
        afxerror_ignore: cfg.afxerror_ignore.clone(),
        verify,
    }
}

/// Запись дублей через `record`. Отмена становится сентинелом `CANCELLED`, остальные
/// сбои несут типизированное описание до строки задачи.
pub fn record_stage(
    host: &dyn Host,
    req: &RecordRequest,
    p: &dyn Progress,
) -> Result<RecordReport, RunError> {
    crate::record(host, req, p).map_err(|f| {
        if f.code == FailCode::Cancelled {
            RunError::from(crate::CANCELLED)
        } else {
            RunError {
                message: f.message.clone(),
                fail: Some(f),
            }
        }
    })
}

/// Пара «сборка CS2 / версия HLAE» удачной записи — вызывающему (для предупреждения об обновлении, DIF-01).
pub fn last_good(report: Option<&Report>, p: &dyn Progress) {
    if let Some(r) = report {
        if let (Some(build), Some(hlae)) = (&r.cs2_build, &r.hlae_version) {
            p.artifact(&Artifact::LastGood {
                cs2_build: build.clone(),
                hlae_version: hlae.clone(),
            });
        }
    }
}

/// Начало плана: устаревший `record.json` удаляется (Pitfall 7). Иначе «Повторить» после упавшего на разборе или
/// плане первого запуска взяло бы план прошлой попытки, чьи скрипты уже перезаписаны или не дописаны.
pub fn begin_plan(out_dir: &Path) -> Result<(), String> {
    RecordJson::remove(out_dir)
        .map_err(|e| format!("не удалось убрать устаревший record.json: {e}"))
}

/// Всё, что нужно записи и склейке, из плана и конфига: кадр, fps, дубли, аргументы ffmpeg, отпечаток скриптов.
pub fn record_json_for(
    plan: &crate::Plan,
    demo_path: &Path,
    demo_len: u64,
    demo_name: &str,
    cfg: &JobConfig,
    take_nvenc: bool,
) -> Result<RecordJson, String> {
    let created_unix = std::time::UNIX_EPOCH
        .elapsed()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(RecordJson {
        version: 1,
        nonce: plan.nonce.clone(),
        demo_path: PathBuf::from(crate::absolute_slash(demo_path)),
        demo_name: demo_name.to_owned(),
        demo_len,
        frame: cfg.frame,
        fps: cfg.fps,
        take_nvenc,
        takes: plan
            .takes
            .iter()
            .map(crate::manifest::TakeSpec::from)
            .collect(),
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
pub fn set_aside_clips(out_dir: &Path) -> Result<Option<PathBuf>, String> {
    let clips = out_dir.join("clips");
    if !clips.is_dir() {
        return Ok(None);
    }
    let stamp = std::time::UNIX_EPOCH
        .elapsed()
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut to = out_dir.join(format!("clips_old_{stamp}"));
    let mut n = 2;
    while to.exists() {
        to = out_dir.join(format!("clips_old_{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&clips, &to).map_err(|e| format!("не удалось отложить прошлые дубли: {e}"))?;
    Ok(Some(to))
}

/// Обёртка `Progress` для записи: всё пересылает дальше, а на `Event::TakeDone` отмечает дубль записанным в
/// `installed.json` — по этому после падения приложения оборванные дубли отличаются от целых (D-08).
struct TakeMarker<'a> {
    inner: &'a dyn Progress,
    out_dir: &'a Path,
}

impl Progress for TakeMarker<'_> {
    fn stage(&self, s: Stage) {
        self.inner.stage(s)
    }
    fn log(&self, msg: &str) {
        self.inner.log(msg)
    }
    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
    fn force_cancelled(&self) -> bool {
        self.inner.force_cancelled()
    }
    fn event(&self, e: &Event) {
        if let Event::TakeDone { n, .. } = e {
            // номер дубля в событии с единицы, в `installed.json` и в имени `takeNNNN` — с нуля
            gamedir::note_take_done(self.out_dir, n.saturating_sub(1));
        }
        self.inner.event(e)
    }
    fn artifact(&self, a: &Artifact) {
        self.inner.artifact(a)
    }
}

/// Одна дорога записи для первого запуска, «Заново» и «Повторить»: прежние дубли в сторону, снимок окружения,
/// установка файлов в игру, запись с проверкой дублей, уборка папки игры при любом исходе (OBS-03, D-05, D-07).
/// `ids` — номер задачи и номер попытки.
pub fn record_takes(
    host: &dyn Host,
    cfg: &JobConfig,
    ids: (i64, u32),
    out_dir: &Path,
    rj: &RecordJson,
    report: Option<&Report>,
    p: &dyn Progress,
) -> Result<RecordReport, RunError> {
    let (job_id, attempt) = ids;
    let marker = TakeMarker { inner: p, out_dir };
    let p: &dyn Progress = &marker;
    p.stage(Stage::Record);
    if let Some(to) = set_aside_clips(out_dir)? {
        p.log(&format!(
            "прошлые дубли отложены в {}",
            to.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    // снимок окружения — в env.json, в манифест попытки и строкой в журнал
    let nvenc_assemble = rj.ffmpeg_args.iter().any(|a| a == "h264_nvenc");
    let env = match report {
        Some(r) => EnvSnapshot::from_report(r, rj.frame, rj.fps, nvenc_assemble, rj.take_nvenc),
        None => EnvSnapshot {
            frame: rj.frame,
            fps: rj.fps,
            nvenc_assemble,
            nvenc_takes: rj.take_nvenc,
            ..EnvSnapshot::default()
        },
    };
    if let Err(e) = env.save(out_dir) {
        p.log(&format!("env.json не записан: {e}"));
    }
    p.log(&env.one_line());
    let installed = gamedir::install_files(&cfg.cs2_dir, out_dir, &rj.demo_name, |path| {
        crate::write_demo_copy(&rj.demo_path, path)
    });
    // игру закрывает сама запись при любом выходе, в том числе при отмене
    let recorded_run = installed.map_err(RunError::from).and_then(|()| {
        // каждая попытка записи идёт с проверкой дублей и ведением манифеста (TAK-03, D-05)
        let req = RecordRequest {
            verify: Some(Verify {
                ffprobe: cfg.ffprobe.clone(),
                plan_hash: rj.plan_hash.clone(),
                attempt,
                env: Some(env),
            }),
            ..record_request(cfg, out_dir, rj, job_id, None)
        };
        p.log(&format!("запас по времени ×{:.2}", cfg.time_margin));
        record_stage(host, &req, p)
    });
    // папка игры чистая при любом исходе — успех, сбой, отмена (D-05, D-06, TMO-06): запись вернулась
    // только после подтверждённого закрытия игры, поэтому копия демки уже никем не занята
    for note in gamedir::cleanup_installed(&cfg.cs2_dir, out_dir) {
        p.log(&note);
    }
    let rep = recorded_run?;
    p.log(&format!("запись заняла {:.0} с", rep.elapsed.as_secs_f32()));
    Ok(rep)
}

/// Повтору нечего взять из папки задачи: типизированный сбой `RETRY_NO_DATA` (D-08).
pub fn retry_no_data(reason: &str) -> RunError {
    let f = RecordFailure::new(
        FailCode::RetryNoData,
        RecordState::Precheck,
        failure::msg_retry_no_data(reason),
        Duration::ZERO,
    );
    RunError {
        message: f.message.clone(),
        fail: Some(f),
    }
}

/// Все ли дубли плана проверены по манифесту (индексы дублей — из `record.json`).
pub fn all_takes_validated(m: &Manifest, takes: &[TakePlan]) -> bool {
    m.all_validated()
        && takes.iter().all(|t| {
            m.entry(t.index)
                .is_some_and(|e| e.status == TakeStatus::Validated)
        })
}

/// «Повторить» (RET-02, RET-03, D-07, D-08): без скачивания, разбора демки и плана — по `record.json` из папки задачи.
/// Все дубли проверены — сразу склейка, без предпроверки и без игры (RET-03). Иначе — предпроверка, запись всех дублей
/// заново (прежние уходят в `clips_old_<ts>`) и склейка. Нет данных для повтора — `RETRY_NO_DATA`.
pub fn resume(host: &dyn Host, spec: &JobSpec, p: &dyn Progress) -> Result<Outcome, RunError> {
    let cfg = &spec.cfg;
    let out_dir = spec
        .out_dir
        .clone()
        .ok_or_else(|| retry_no_data("папка задачи неизвестна"))?;
    p.artifact(&Artifact::OutDir(out_dir.clone()));
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
    let outcome = |out: PathBuf| Outcome {
        out_dir: out_dir.clone(),
        result: out,
        shots: rj.shots as u32,
        secs: rj.secs,
    };
    if manifest
        .as_ref()
        .is_some_and(|m| all_takes_validated(m, &takes))
    {
        p.log("все дубли проверены по манифесту — сразу склейка, без предпроверки и игры");
        p.stage(Stage::Assemble);
        let out = assemble_stage(
            host,
            &out_dir,
            &takes,
            &rj.ffmpeg_args,
            rj.video_secs,
            deadlines(cfg).ffmpeg_for(rj.video_secs),
            p,
        )?;
        return Ok(outcome(out));
    }
    // не все дубли проверены: HLAE нумерует новые дубли «наибольший + 1» и не заполняет пропуски, поэтому
    // дописать только недостающие нельзя — записываем все заново, прежние уходят в `clips_old_<ts>` (D-07, RET-06)
    let not_validated = match &manifest {
        None => "манифест дублей не найден".to_owned(),
        Some(m) => takes
            .iter()
            .filter(|t| {
                m.entry(t.index)
                    .is_none_or(|e| e.status != TakeStatus::Validated)
            })
            .map(|t| (t.index + 1).to_string())
            .collect::<Vec<_>>()
            .join(", "),
    };
    p.log(&format!("дубли не все проверены: {not_validated} — записываю все заново (новые дубли HLAE нумерует подряд)"));
    // демка — из record.json, не из скачивания: предпроверка скажет, если её нет (PRECHECK_FAILED)
    let inputs = precheck_inputs(
        cfg,
        Demo::File(rj.demo_path.clone()),
        spec.last_good.clone(),
    );
    let mut report = precheck_stage(host, &inputs, p)?;
    space_pass(
        host,
        &inputs,
        space::estimate(&takes, rj.frame, rj.demo_len),
        &mut report,
        p,
    )?;
    record_takes(
        host,
        cfg,
        (spec.job_id, spec.attempt),
        &out_dir,
        &rj,
        Some(&report),
        p,
    )?;
    p.stage(Stage::Assemble);
    let out = assemble_stage(
        host,
        &out_dir,
        &takes,
        &rj.ffmpeg_args,
        rj.video_secs,
        deadlines(cfg).ffmpeg_for(rj.video_secs),
        p,
    )?;
    last_good(Some(&report), p);
    Ok(outcome(out))
}

/// Отпечаток скриптов записи (`highlights.cfg`, `cs2c_*`): совпал — дубли в `clips/` от этого же плана.
/// `nonce` меняется на каждый план и стоит во всех маркерах консоли, поэтому перед хешированием
/// заменяется на `NONCE`: иначе «дубли этого плана уже записаны» никогда бы не срабатывало.
pub fn plan_hash(out_dir: &Path, nonce: &str) -> String {
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
        let body = if nonce.is_empty() {
            body.into_owned()
        } else {
            body.replace(nonce, "NONCE")
        };
        for b in n.bytes().chain(body.into_bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::host::{
        ProcInfo,
        fake::{FakeHost, Scenario},
    };

    /// `Progress`, который копит артефакты.
    #[derive(Default)]
    struct Capture {
        artifacts: Mutex<Vec<Artifact>>,
    }
    impl Progress for Capture {
        fn stage(&self, _: Stage) {}
        fn log(&self, _: &str) {}
        fn artifact(&self, a: &Artifact) {
            self.artifacts.lock().unwrap().push(a.clone());
        }
    }

    fn plan_dir(tag: &str, nonce: &str, tick: u32) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cs2cinema-jobmod-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("highlights.cfg"),
            format!("// шапка\necho CS2C {nonce} SCRIPT\nexec cs2c_01\n"),
        )
        .unwrap();
        std::fs::write(dir.join("cs2c_01.cfg"), format!("mirv_cmd addAtTick {tick} \"mirv_streams record end; echo CS2C {nonce} TAKE_END 0\"\n")).unwrap();
        dir
    }

    fn two_takes() -> Vec<TakePlan> {
        (0..2)
            .map(|index| TakePlan {
                index,
                shot: index as usize,
                game_secs: 5.0,
                capture_fps: 60,
            })
            .collect()
    }

    /// `record.json` на `n` дублей по 5 игровых секунд в кадре 1280x720.
    fn sample_record_json(n: usize, demo: PathBuf, plan_hash: String) -> RecordJson {
        let takes: Vec<TakePlan> = (0..n as u32)
            .map(|index| TakePlan {
                index,
                shot: index as usize,
                game_secs: 5.0,
                capture_fps: 60,
            })
            .collect();
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
            takes: takes.iter().map(crate::manifest::TakeSpec::from).collect(),
            ffmpeg_args: args,
            video_secs: 10.0,
            shots: takes.len(),
            secs: 5.0 * takes.len() as f32,
            plan_hash,
            created_unix: 1,
        }
    }

    /// Конфиг с временным деревом HLAE и CS2, где есть всё нужное предпроверке.
    fn healthy_config(tag: &str) -> JobConfig {
        let root =
            std::env::temp_dir().join(format!("cs2cinema-jobmod-env-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in [
            "hlae/HLAE.exe",
            "hlae/x64/AfxHookSource2.dll",
            "hlae/ffmpeg/bin/ffmpeg.exe",
            "cs2/game/bin/win64/cs2.exe",
        ] {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        JobConfig {
            hlae_dir: root.join("hlae"),
            cs2_dir: root.join("cs2"),
            out_root: root.join("out"),
            ..JobConfig::default()
        }
    }

    fn proc(pid: u32, exe: &str) -> ProcInfo {
        ProcInfo {
            pid,
            exe_name: exe.to_owned(),
        }
    }

    #[test]
    fn plan_hash_ignores_the_nonce() {
        let a = plan_dir("a", "aaaa1111", 1000);
        let b = plan_dir("b", "bbbb2222", 1000);
        let c = plan_dir("c", "cccc3333", 1001);
        assert_eq!(
            plan_hash(&a, "aaaa1111"),
            plan_hash(&b, "bbbb2222"),
            "nonce не должен менять отпечаток"
        );
        assert_ne!(
            plan_hash(&a, "aaaa1111"),
            plan_hash(&c, "cccc3333"),
            "другой тик — другой план"
        );
        for d in [a, b, c] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn set_aside_clips_never_overwrites_an_earlier_set_aside_folder() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            set_aside_clips(&dir).unwrap(),
            None,
            "нет clips — нечего откладывать"
        );
        let mut seen = std::collections::HashSet::new();
        for i in 0..3 {
            std::fs::create_dir_all(dir.join("clips")).unwrap();
            std::fs::write(dir.join("clips/marker.txt"), i.to_string()).unwrap();
            let to = set_aside_clips(&dir).unwrap().expect("clips отложены");
            assert!(
                to.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("clips_old_")
            );
            assert!(seen.insert(to.clone()), "имя {to:?} уже занято");
            assert_eq!(
                std::fs::read_to_string(to.join("marker.txt")).unwrap(),
                i.to_string()
            );
            assert!(!dir.join("clips").exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn time_margin_scales_the_assemble_deadline() {
        let mut cfg = JobConfig::default();
        let base = deadlines(&cfg).ffmpeg_for(100.0);
        cfg.time_margin = 2.0;
        assert_eq!(deadlines(&cfg).ffmpeg_for(100.0), base * 2);
        cfg.time_margin = f32::NAN;
        assert_eq!(
            deadlines(&cfg).ffmpeg_for(100.0),
            base,
            "мусор в настройках не ломает дедлайны"
        );
    }

    #[test]
    fn afxerror_ignore_from_config_reaches_the_record_request() {
        let mut cfg = JobConfig::default();
        let rj = sample_record_json(2, PathBuf::from("match.dem"), "h".into());
        assert!(
            record_request(&cfg, Path::new("out"), &rj, 7, None)
                .afxerror_ignore
                .is_empty(),
            "по умолчанию список пуст"
        );
        cfg.afxerror_ignore = vec!["shader cache".into()];
        assert_eq!(
            record_request(&cfg, Path::new("out"), &rj, 7, None).afxerror_ignore,
            vec!["shader cache".to_owned()]
        );
    }

    #[test]
    fn resume_launches_at_the_recorded_frame_not_the_current_setting() {
        let cfg = JobConfig {
            frame: (1920, 1080),
            ..JobConfig::default()
        };
        let rj = sample_record_json(2, PathBuf::from("match.dem"), "h".into());
        let req = record_request(&cfg, Path::new("out"), &rj, 7, None);
        assert_eq!(req.launch.size, (1280, 720), "кадр — из record.json");
        assert_eq!(
            (req.launch.nonce.as_str(), req.launch.demo_name.as_str()),
            ("ab12cd34", "cs2cinema_match_hero")
        );
        assert_eq!(req.takes, rj.take_plans());
    }

    #[test]
    fn failing_precheck_returns_a_typed_failure_and_reports_the_json() {
        let cfg = healthy_config("fail");
        let host = FakeHost::new(
            Scenario::happy(1).processes(vec![proc(100, "steam.exe"), proc(777, "cs2.exe")]),
        );
        let cap = Capture::default();
        let inputs = precheck_inputs(&cfg, Demo::NotApplicable, None);
        let e = precheck_stage(&host, &inputs, &cap).expect_err("чужой CS2 останавливает запись");
        let f = e.fail.as_ref().expect("сбой типизирован");
        assert_eq!(f.code, FailCode::ForeignGameRunning);
        assert_eq!(f.state, RecordState::Precheck);

        let artifacts = cap.artifacts.lock().unwrap();
        let [Artifact::Precheck { json, passed }] = artifacts.as_slice() else {
            panic!("ожидался один отчёт: {artifacts:?}")
        };
        assert!(!passed);
        let report = Report::from_json(json).expect("отчёт разбирается");
        let item = report
            .items
            .iter()
            .find(|i| i.id == ids::FOREIGN_GAME)
            .expect("пункт чужого CS2");
        assert_eq!(item.level, Level::Fail);
        let _ = std::fs::remove_dir_all(cfg.cs2_dir.parent().unwrap());
    }

    #[test]
    fn passing_precheck_reports_passed() {
        let cfg = healthy_config("ok");
        let ok = crate::host::CmdOutput {
            code: Some(0),
            stderr_tail: vec![],
            elapsed: Duration::ZERO,
        };
        let host = FakeHost::new(Scenario::happy(1).program(
            "ffmpeg",
            &["ffmpeg version 7.1.1 Copyright (c) the FFmpeg developers"],
            Ok(ok),
        ));
        let cap = Capture::default();
        let inputs = precheck_inputs(&cfg, Demo::NotApplicable, None);
        let report = precheck_stage(&host, &inputs, &cap).expect("окружение исправно");
        assert!(!report.blocks(), "{report:?}");

        let artifacts = cap.artifacts.lock().unwrap();
        assert_eq!(
            artifacts.as_slice(),
            [Artifact::Precheck {
                json: report.to_json(),
                passed: true
            }]
        );
        let _ = std::fs::remove_dir_all(cfg.cs2_dir.parent().unwrap());
    }

    #[test]
    fn job_config_debug_hides_the_faceit_key() {
        let cfg = JobConfig {
            faceit_key: "super-secret-key-123".into(),
            ..JobConfig::default()
        };
        let text = format!("{cfg:?}");
        assert!(!text.contains("super-secret-key-123"), "{text}");
        assert!(text.contains("задан"), "{text}");
        assert!(format!("{:?}", JobConfig::default()).contains("пусто"));
    }

    #[test]
    fn job_config_defaults_match_the_app_defaults() {
        let cfg = JobConfig::default();
        assert_eq!(
            (cfg.fps, cfg.frame, cfg.time_margin),
            (60, (1280, 720), 1.0)
        );
        assert!(cfg.flybys && cfg.walls && cfg.gpu_encode && !cfg.detailed && !cfg.chronological);
        assert_eq!(
            (cfg.ffmpeg, cfg.ffprobe),
            (PathBuf::from("ffmpeg"), PathBuf::from("ffprobe"))
        );
    }

    #[test]
    fn last_good_is_reported_only_with_a_report() {
        let cap = Capture::default();
        last_good(None, &cap);
        assert!(cap.artifacts.lock().unwrap().is_empty());
    }

    #[test]
    fn record_json_written_after_planning_passes_its_own_check() {
        let dir = plan_dir("record-json", "ab12cd34", 1000);
        let rj_sample = sample_record_json(2, PathBuf::from("match.dem"), String::new());
        let plan = crate::Plan {
            timeline: director::Timeline {
                map: "de_nuke".into(),
                tick_rate: 64.0,
                server_start_tick: 0,
                shots: vec![],
            },
            out_dir: dir.clone(),
            ffmpeg_args: rj_sample.ffmpeg_args.clone(),
            video_secs: 10.0,
            takes: two_takes(),
            nonce: "ab12cd34".into(),
        };
        let cfg = JobConfig {
            frame: (1920, 1080),
            ..JobConfig::default()
        };
        let rj = record_json_for(
            &plan,
            Path::new("C:/demos/match.dem"),
            4096,
            "cs2cinema_match_hero",
            &cfg,
            true,
        )
        .expect("record.json собирается из плана");
        rj.save(&dir).unwrap();

        let loaded = RecordJson::load(&dir).expect("record.json проходит собственную проверку");
        assert_eq!(loaded, rj);
        assert_eq!(loaded.nonce, "ab12cd34");
        assert_eq!(loaded.take_plans(), two_takes());
        assert_eq!(loaded.ffmpeg_args, plan.ffmpeg_args);
        assert_eq!(
            (loaded.frame, loaded.fps, loaded.take_nvenc, loaded.demo_len),
            ((1920, 1080), cfg.fps, true, 4096)
        );
        assert_eq!(loaded.plan_hash, plan_hash(&dir, "ab12cd34"));
        assert_eq!(loaded.demo_name, "cs2cinema_match_hero");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_record_json_is_removed_before_planning() {
        let dir = plan_dir("stale-record", "ab12cd34", 1000);
        sample_record_json(1, PathBuf::from("match.dem"), "h".into())
            .save(&dir)
            .unwrap();
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
