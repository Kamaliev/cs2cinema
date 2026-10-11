//! Машинная приёмка: база приложения только на чтение, ffprobe роликов, снимок процессов;
//! ничего не пишет в базу и не трогает процессы (ACC-02, ACC-03, DC-8, DC-9, DC-10).
//!
//! `cs2-cli acceptance check` отвечает «приёмка пройдена или нет и почему» по партии задач, поставленной кнопкой
//! «Поставить приёмочный набор»: ровно матчи набора на разных картах, все «готово», каждый ролик прочитан ffprobe
//! (видео и звук, длительность в допуске от плана), игра не запущена, ручных действий не было. Проверка — чистая
//! функция [`evaluate`] над снятым состоянием; файлы и процессы читают только обёртки вокруг неё.

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

use pipeline::{
    acceptance as set_file,
    host::{CmdSpec, Host, ProcInfo},
    manifest::RecordJson,
    record::{
        Deadlines,
        verify::{PROBE_STDOUT_MAX, Probe, parse_probe, probe_args, tolerance, within_tolerance},
    },
};
use serde_json::json;
use store::{
    Job, JobSource, JobStatus, Store,
    journal::{self, StartKind},
};

/// Использование подкоманд `acceptance`; печатается и по `--help`.
pub const USAGE_ACCEPTANCE: &str = "\
cs2-cli acceptance — машинная приёмка очереди (база приложения открывается только на чтение)

Использование:
  cs2-cli acceptance check [--db <файл>] [--batch <имя> | --last] [--expect-restart-from <партия A>] [--set <файл>]

check: подтверждает партию, поставленную кнопкой «Поставить приёмочный набор»: в партии ровно матчи набора,
  5 разных карт, все задачи «готово», каждый highlights.mp4 прошёл ffprobe (видео и звук, длительность в допуске
  от плана), cs2.exe и HLAE.exe не запущены, ручных действий не было. Авто-повторы перечисляются отдельно.
  Код выхода: 0 — пройдена, 1 — не пройдена, 2 — ошибка аргументов или базы.
  Отчёт пишется в acceptance-<партия>.json рядом с базой.
  --db <файл>                      база приложения         (по умолчанию %LOCALAPPDATA%\\cs2cinema\\cs2cinema.db)
  --batch <имя> | --last           какая партия            (по умолчанию --last: самая новая)
  --expect-restart-from <партия A> серия B шла в другом запуске приложения, начатом после конца серии A
  --set <файл>                     набор вместо встроенного (или переменная CS2CINEMA_ACCEPTANCE)";

/// Имя файла базы приложения в папке данных.
const DB_FILE: &str = "cs2cinema.db";
/// Имя готового ролика.
const OUTPUT_NAME: &str = "highlights.mp4";
/// Не читать `plan.json` больше этого размера: он нужен только ради поля `map`.
const PLAN_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Папка данных приложения (копия логики `app::settings::data_dir`): база, настройки, отчёты.
pub fn data_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("LOCALAPPDATA")
        .or_else(|| env("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.unwrap_or_else(|| PathBuf::from(".")).join("cs2cinema")
}

/// Что известно о партии на момент проверки: всё, что читают файлы и процессы, собрано заранее.
pub struct CheckInput {
    /// Матчи набора (id).
    pub set: Vec<String>,
    pub batch: String,
    pub jobs: Vec<Job>,
    /// Карта задачи из `plan.json` (`Timeline.map`); нет записи — берётся `job.map`.
    pub maps: HashMap<i64, String>,
    /// Ожидаемая длительность ролика из `record.json` (`video_secs`) или причина, почему её нет.
    pub expected: HashMap<i64, Result<f32, String>>,
    /// Снимок процессов системы.
    pub processes: Result<Vec<ProcInfo>, String>,
    /// Партия серии A и её задачи — для `--expect-restart-from`.
    pub restart_from: Option<(String, Vec<Job>)>,
}

/// Строка отчёта: одна задача партии.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    pub match_id: String,
    pub map: String,
    pub status: &'static str,
    /// Измеренная длительность ролика, секунды.
    pub measured: Option<f32>,
    /// Ожидаемая по плану длительность, секунды.
    pub expected: Option<f32>,
    pub attempt: u32,
    pub auto_retries: u32,
    /// Итоги упавших попыток, которые закрыл авто-повтор: «попытка 1: сбой CONSOLE_NOT_OPEN …».
    pub failed_attempts: Vec<String>,
    pub problems: Vec<String>,
}

/// Итог проверки партии.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub batch: String,
    pub rows: Vec<Row>,
    /// Все нарушения: общие по партии и по задачам.
    pub problems: Vec<String>,
    /// Сколько всего авто-повторов в партии.
    pub auto_retries: u32,
    pub passed: bool,
}

impl Report {
    /// Отчёт об ошибке аргументов или базы: проверка не состоялась.
    fn failed(batch: &str, problem: String) -> Self {
        Report {
            batch: batch.to_owned(),
            rows: Vec::new(),
            problems: vec![problem],
            auto_retries: 0,
            passed: false,
        }
    }
}

fn source_id(job: &Job) -> String {
    match &job.source {
        JobSource::Faceit(id) => id.clone(),
        JobSource::File(p) => format!("файл {}", p.display()),
    }
}

/// Проверка партии по всем критериям приёмки. Чистая: файлы читает `probe`, остальное уже снято в `input`.
pub fn evaluate(input: &CheckInput, probe: &dyn Fn(&Path) -> Result<Probe, String>) -> Report {
    let mut problems: Vec<String> = Vec::new();

    // набор и состав партии
    if let Err(e) = set_file::require_full(&input.set) {
        problems.push(format!("набор: {e}"));
    }
    let sources: Vec<String> = input.jobs.iter().map(source_id).collect();
    if input.jobs.len() != input.set.len() {
        problems.push(format!(
            "в партии {} задач, в наборе {} матчей",
            input.jobs.len(),
            input.set.len()
        ));
    }
    for id in &input.set {
        if !sources.contains(id) {
            problems.push(format!("в партии нет матча {id} из набора"));
        }
    }
    let in_set: HashSet<&String> = input.set.iter().collect();
    let mut seen: HashSet<&String> = HashSet::new();
    for s in &sources {
        if !in_set.contains(s) {
            problems.push(format!("в партии лишний матч {s}, которого нет в наборе"));
        } else if !seen.insert(s) {
            problems.push(format!("матч {s} стоит в партии дважды"));
        }
    }

    // карты
    let mut by_map: HashMap<String, i64> = HashMap::new();
    let mut maps: HashMap<i64, String> = HashMap::new();
    for j in &input.jobs {
        let map = input
            .maps
            .get(&j.id)
            .unwrap_or(&j.map)
            .trim()
            .to_lowercase();
        if map.is_empty() {
            problems.push(format!("карта задачи #{} неизвестна", j.id));
            continue;
        }
        if let Some(first) = by_map.get(&map) {
            problems.push(format!(
                "две одинаковые карты: {map} (задачи #{first} и #{}): нужны {} разных",
                j.id,
                set_file::SET_SIZE
            ));
        } else {
            by_map.insert(map.clone(), j.id);
        }
        maps.insert(j.id, map);
    }

    // процессы
    match &input.processes {
        Err(e) => problems.push(format!("снимок процессов недоступен: {e}")),
        Ok(list) => {
            for p in list {
                if pipeline::host::ORPHAN_IMAGES
                    .iter()
                    .any(|img| p.exe_name.eq_ignore_ascii_case(img))
                {
                    problems.push(format!(
                        "запущен {} (PID {}): после приёмки игры быть не должно",
                        p.exe_name, p.pid
                    ));
                }
            }
        }
    }

    // серии
    if let Some((a_batch, a_jobs)) = &input.restart_from {
        match (
            set_file::batch_pid(&input.batch),
            set_file::batch_pid(a_batch),
        ) {
            (Some(b), Some(a)) if a == b => {
                problems.push(format!("серия B шла в том же запуске приложения, что и серия A: PID {b} в обеих партиях"));
            }
            (Some(_), Some(_)) => {}
            _ => problems.push(format!(
                "PID приложения не разобран из имён партий {a_batch} и {}",
                input.batch
            )),
        }
        let a_end = a_jobs.iter().map(|j| j.updated_at).max();
        let b_start = input.jobs.iter().map(|j| j.created_at).min();
        match (a_end, b_start) {
            (Some(end), Some(start)) if start < end => {
                problems.push(format!("серия B поставлена (t={start}) раньше конца серии A (t={end}): перезапуск приложения не доказан"))
            }
            (None, _) => problems.push(format!("в партии {a_batch} нет задач: конец серии A неизвестен")),
            _ => {}
        }
    }

    let rows: Vec<Row> = input
        .jobs
        .iter()
        .map(|j| check_job(j, input, maps.get(&j.id), probe))
        .collect();
    for r in &rows {
        problems.extend(r.problems.iter().cloned());
    }
    let auto_retries = rows.iter().map(|r| r.auto_retries).sum();
    let passed = problems.is_empty();
    Report {
        batch: input.batch.clone(),
        rows,
        problems,
        auto_retries,
        passed,
    }
}

/// Критерии по одной задаче; тексты нарушений самодостаточны (называют задачу).
fn check_job(
    j: &Job,
    input: &CheckInput,
    map: Option<&String>,
    probe: &dyn Fn(&Path) -> Result<Probe, String>,
) -> Row {
    let id = j.id;
    let mut problems: Vec<String> = Vec::new();
    let mut measured = None;
    let expected = match input.expected.get(&id) {
        Some(Ok(v)) => Some(*v),
        Some(Err(e)) => {
            problems.push(format!(
                "задача #{id}: ожидаемая длительность неизвестна: {e}"
            ));
            None
        }
        None => {
            problems.push(format!(
                "задача #{id}: ожидаемая длительность неизвестна: нет данных record.json"
            ));
            None
        }
    };

    if j.status != JobStatus::Done {
        problems.push(format!(
            "задача #{id}: статус «{}», нужен «{}»",
            j.status.title(),
            JobStatus::Done.title()
        ));
    } else {
        match j.result.as_deref() {
            None => problems.push(format!("задача #{id}: ролика нет — результат не записан")),
            Some(path) if path.file_name().is_none_or(|n| n != OUTPUT_NAME) => {
                problems.push(format!(
                    "задача #{id}: ролика нет — результат {} не {OUTPUT_NAME}",
                    path.display()
                ))
            }
            Some(path) => match probe(path) {
                Err(e) => {
                    problems.push(format!("задача #{id}: ролика нет или он не читается: {e}"))
                }
                Ok(p) => {
                    let has = |kind: &str| p.streams.iter().any(|s| s.codec_type == kind);
                    if !has("video") {
                        problems.push(format!("задача #{id}: в ролике нет видео"));
                    }
                    if !has("audio") {
                        problems.push(format!("задача #{id}: в ролике нет звука"));
                    }
                    measured = p.duration.or_else(|| {
                        p.streams
                            .iter()
                            .filter_map(|s| s.duration)
                            .fold(None, |m: Option<f32>, d| Some(m.map_or(d, |m| m.max(d))))
                    });
                    match (measured, expected) {
                        (None, _) => problems.push(format!("задача #{id}: длительность ролика не определена")),
                        (Some(m), Some(e)) if !within_tolerance(e, m) => {
                            problems.push(format!("задача #{id}: длительность {m:.1} с вне допуска от плана {e:.1} с (±{:.1} с)", tolerance(e)))
                        }
                        _ => {}
                    }
                }
            },
        }
    }

    // ручные действия — только по сохранённым данным: разделители попыток в журнале и счётчики задачи
    let attempts = journal::attempts(&j.log);
    for (n, kind) in &attempts {
        if !matches!(kind, StartKind::First | StartKind::Auto) {
            problems.push(format!(
                "ручное действие: задача #{id}, попытка {n} — {}",
                kind.title()
            ));
        }
    }
    if attempts.len() as u32 != j.attempt {
        problems.push(format!(
            "задача #{id}: в журнале {} попыток, а у задачи попытка {}",
            attempts.len(),
            j.attempt
        ));
    }
    if j.attempt != 1 + j.auto_retries {
        problems.push(format!("задача #{id}: попытка {} при {} авто-повторах (должно быть 1 + авто-повторы): был ручной запуск", j.attempt, j.auto_retries));
    }
    let failed_attempts: Vec<String> = journal::outcomes(&j.log)
        .into_iter()
        .filter(|(n, _)| *n < j.attempt)
        .map(|(n, t)| format!("попытка {n}: {t}"))
        .collect();

    Row {
        id,
        match_id: source_id(j),
        map: map.cloned().unwrap_or_default(),
        status: j.status.title(),
        measured,
        expected,
        attempt: j.attempt,
        auto_retries: j.auto_retries,
        failed_attempts,
        problems,
    }
}

/// Разобранные аргументы `check`.
#[derive(Debug, Default, PartialEq)]
pub struct CheckArgs {
    pub db: Option<PathBuf>,
    pub batch: Option<String>,
    pub restart_from: Option<String>,
    pub set: Option<String>,
}

pub fn parse_check(args: &[String]) -> Result<CheckArgs, String> {
    let mut out = CheckArgs::default();
    let mut last = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} требует значение"))
        };
        match a.as_str() {
            "--db" => out.db = Some(value("--db")?.into()),
            "--batch" => out.batch = Some(value("--batch")?),
            "--last" => last = true,
            "--expect-restart-from" => out.restart_from = Some(value("--expect-restart-from")?),
            "--set" => out.set = Some(value("--set")?),
            s if s.starts_with("--") => return Err(format!("неизвестный параметр {s}")),
            s => return Err(format!("лишний аргумент {s}")),
        }
    }
    if last && out.batch.is_some() {
        return Err("--batch и --last вместе не нужны".into());
    }
    Ok(out)
}

/// Длительность `plan.json` не нужна, только карта; файл больше предела не читается.
fn plan_map(out_dir: &Path) -> Option<String> {
    let path = out_dir.join("plan.json");
    if std::fs::metadata(&path).ok()?.len() > PLAN_MAX_BYTES {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("map")?.as_str().map(str::to_owned)
}

/// Проверка партии по файлу базы (только чтение). Код выхода: 0 — пройдена, 1 — нет, 2 — ошибка аргументов или базы.
/// `probe` и `processes` внедряются: боевые — ffprobe через хост и снимок процессов, в тестах — подставные.
pub fn check_with(
    db: &Path,
    batch: Option<&str>,
    restart_from: Option<&str>,
    set: Result<Vec<String>, String>,
    probe: &dyn Fn(&Path) -> Result<Probe, String>,
    processes: Result<Vec<ProcInfo>, String>,
) -> (Report, i32) {
    let set = match set {
        Ok(s) => s,
        Err(e) => {
            return (
                Report::failed(batch.unwrap_or(""), format!("набор: {e}")),
                2,
            );
        }
    };
    let store = match Store::open_read_only(db) {
        Ok(s) => s,
        Err(e) => return (Report::failed(batch.unwrap_or(""), e), 2),
    };
    let batch = match batch {
        Some(b) => b.to_owned(),
        None => match store.last_batch() {
            Ok(Some(b)) => b,
            Ok(None) => {
                return (
                    Report::failed(
                        "",
                        "в базе нет приёмочных партий: поставьте набор кнопкой в приложении".into(),
                    ),
                    2,
                );
            }
            Err(e) => return (Report::failed("", e), 2),
        },
    };
    let jobs = match store.jobs_in_batch(&batch) {
        Ok(j) if !j.is_empty() => j,
        Ok(_) => {
            return (
                Report::failed(&batch, format!("партии {batch} в базе нет")),
                2,
            );
        }
        Err(e) => return (Report::failed(&batch, e), 2),
    };
    let restart_from = match restart_from {
        None => None,
        Some(a) => match store.jobs_in_batch(a) {
            Ok(j) if !j.is_empty() => Some((a.to_owned(), j)),
            Ok(_) => return (Report::failed(&batch, format!("партии {a} в базе нет")), 2),
            Err(e) => return (Report::failed(&batch, e), 2),
        },
    };
    let mut maps = HashMap::new();
    let mut expected = HashMap::new();
    for j in &jobs {
        match &j.out_dir {
            Some(dir) => {
                if let Some(m) = plan_map(dir) {
                    maps.insert(j.id, m);
                }
                expected.insert(j.id, RecordJson::load(dir).map(|r| r.video_secs));
            }
            None => {
                expected.insert(j.id, Err("у задачи нет папки".to_owned()));
            }
        }
    }
    let input = CheckInput {
        set,
        batch,
        jobs,
        maps,
        expected,
        processes,
        restart_from,
    };
    let report = evaluate(&input, probe);
    let code = if report.passed { 0 } else { 1 };
    (report, code)
}

/// ffprobe ролика через хост: дедлайн, вектор аргументов, stdout ограничен.
fn probe_with_host(host: &dyn Host, path: &Path) -> Result<Probe, String> {
    if !path.is_file() {
        return Err(format!("файла нет: {}", path.display()));
    }
    let spec = CmdSpec {
        program: "ffprobe".into(),
        args: probe_args(path),
        cwd: None,
        deadline: Deadlines::default().ffprobe,
    };
    let mut text = String::new();
    let mut too_long = false;
    let out = host
        .run(&spec, &|| false, &mut |line| {
            if text.len() + line.len() + 1 > PROBE_STDOUT_MAX {
                too_long = true;
            } else {
                text.push_str(line);
                text.push('\n');
            }
        })
        .map_err(|e| format!("ffprobe не отработал: {e}"))?;
    if out.code != Some(0) {
        let tail = out
            .stderr_tail
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>();
        let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
        return Err(format!(
            "ffprobe завершился с кодом {}: {tail}",
            out.code.map_or("?".to_owned(), |c| c.to_string())
        ));
    }
    if too_long {
        return Err(format!(
            "ответ ffprobe длиннее {} КиБ",
            PROBE_STDOUT_MAX / 1024
        ));
    }
    parse_probe(&text)
}

fn secs(v: Option<f32>) -> String {
    v.map_or("—".to_owned(), |v| format!("{v:.1} с"))
}

/// Печать отчёта: строка на задачу, затем итог и нарушения.
pub fn print_report(r: &Report) {
    println!("Партия {}", r.batch);
    for row in &r.rows {
        println!(
            "  #{} · {} · {} · {} · {} (план {}) · попыток {} · авто-повторов {}{}",
            row.id,
            row.match_id,
            if row.map.is_empty() { "?" } else { &row.map },
            row.status,
            secs(row.measured),
            secs(row.expected),
            row.attempt,
            row.auto_retries,
            if row.problems.is_empty() {
                String::new()
            } else {
                format!(" · НАРУШЕНИЙ {}", row.problems.len())
            },
        );
        for f in &row.failed_attempts {
            println!("      упавшая попытка, закрытая авто-повтором — {f}");
        }
    }
    if r.passed {
        println!(
            "ПРИЁМКА ПРОЙДЕНА: {0}/{0} готово, авто-повторов: {1}",
            r.rows.len(),
            r.auto_retries
        );
    } else {
        println!("ПРИЁМКА НЕ ПРОЙДЕНА:");
        for p in &r.problems {
            println!("  - {p}");
        }
    }
}

/// Безопасное имя файла из имени партии (имя пришло из базы или аргумента).
fn file_part(batch: &str) -> String {
    let s: String = batch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if s.is_empty() {
        "partiya".to_owned()
    } else {
        s
    }
}

/// Отчёт в JSON: только id, карты, статусы, длительности, коды и итоги попыток (ни настроек, ни ключей).
pub fn report_json(r: &Report) -> serde_json::Value {
    json!({
        "batch": r.batch,
        "passed": r.passed,
        "auto_retries": r.auto_retries,
        "problems": r.problems,
        "jobs": r.rows.iter().map(|row| json!({
            "id": row.id,
            "match": row.match_id,
            "map": row.map,
            "status": row.status,
            "measured_secs": row.measured,
            "expected_secs": row.expected,
            "attempt": row.attempt,
            "auto_retries": row.auto_retries,
            "failed_attempts": row.failed_attempts,
            "problems": row.problems,
        })).collect::<Vec<_>>(),
    })
}

/// Пишет `acceptance-<партия>.json` в папку `dir` (рядом с базой), не в саму базу.
pub fn write_report(dir: &Path, r: &Report) -> Result<PathBuf, String> {
    let path = dir.join(format!("acceptance-{}.json", file_part(&r.batch)));
    let text = serde_json::to_string_pretty(&report_json(r)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn real_env(k: &str) -> Option<String> {
    env::var(k).ok()
}

/// `cs2-cli acceptance <check|…>`.
pub fn acceptance_cmd() -> ExitCode {
    let args: Vec<String> = env::args().skip(2).collect();
    match args.first().map(String::as_str) {
        Some("check") => check_cmd(&args[1..]),
        Some("-h" | "--help") => {
            println!("{USAGE_ACCEPTANCE}");
            ExitCode::SUCCESS
        }
        other => {
            if let Some(o) = other {
                eprintln!("ошибка: неизвестная подкоманда acceptance {o}\n");
            }
            eprintln!("{USAGE_ACCEPTANCE}");
            ExitCode::from(2)
        }
    }
}

/// `cs2-cli acceptance check …`.
pub fn check_cmd(args: &[String]) -> ExitCode {
    let a = match parse_check(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ошибка: {e}\n\n{USAGE_ACCEPTANCE}");
            return ExitCode::from(2);
        }
    };
    let db =
        a.db.clone()
            .unwrap_or_else(|| data_dir(&real_env).join(DB_FILE));
    let set = match &a.set {
        Some(path) => set_file::load_set(&|k| (k == set_file::SET_ENV).then(|| path.clone())),
        None => set_file::load_set(&real_env),
    }
    .map(|(ids, _source)| ids);
    let host = pipeline::host::default_host();
    let processes = host.processes().map_err(|e| e.to_string());
    let probe = |p: &Path| probe_with_host(&*host, p);
    let (report, code) = check_with(
        &db,
        a.batch.as_deref(),
        a.restart_from.as_deref(),
        set,
        &probe,
        processes,
    );
    if code == 2 {
        // проверка не состоялась: ошибка аргументов или базы, а не результат приёмки
        for p in &report.problems {
            eprintln!("ошибка: {p}");
        }
        return ExitCode::from(2);
    }
    print_report(&report);
    {
        let dir = db
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        match write_report(dir, &report) {
            Ok(path) => println!("Отчёт: {}", path.display()),
            Err(e) => eprintln!("отчёт не записан: {e}"),
        }
    }
    ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipeline::record::verify::Stream;
    use store::{JobKind, NewJob};

    const IDS: [&str; 5] = [
        "1-00000000-0000-0000-0000-000000000001",
        "1-00000000-0000-0000-0000-000000000002",
        "1-00000000-0000-0000-0000-000000000003",
        "1-00000000-0000-0000-0000-000000000004",
        "1-00000000-0000-0000-0000-000000000005",
    ];
    const MAPS: [&str; 5] = [
        "de_mirage",
        "de_inferno",
        "de_nuke",
        "de_dust2",
        "de_ancient",
    ];

    fn first_log() -> String {
        format!(
            "{}\nитог попытки 1: готово",
            journal::separator(1, "2026-01-01 10:00:00", StartKind::First)
        )
    }

    fn job(i: usize) -> Job {
        let id = i as i64 + 1;
        Job {
            id,
            created_at: 1000 + id,
            updated_at: 2000 + id,
            kind: JobKind::Record,
            source: JobSource::Faceit(IDS[i].to_owned()),
            label: format!("приёмка {id}/5"),
            hero_name: "me".into(),
            hero_steam: 1,
            map: MAPS[i].to_owned(),
            status: JobStatus::Done,
            stage: String::new(),
            out_dir: None,
            result: Some(PathBuf::from(format!("out/{id}/highlights.mp4"))),
            error: None,
            shots: Some(3),
            secs: Some(60.0),
            log: first_log(),
            fail_code: None,
            fail_state: None,
            fail_hint: None,
            fail_tail: None,
            fail_elapsed_ms: None,
            phase: String::new(),
            precheck: None,
            attempt: 1,
            resume: false,
            auto_retries: 0,
            batch: Some("acc-1700000000-100".into()),
        }
    }

    fn good_probe() -> Probe {
        let stream = |t: &str| Stream {
            codec_type: t.to_owned(),
            duration: Some(60.0),
            ..Stream::default()
        };
        Probe {
            streams: vec![stream("video"), stream("audio")],
            duration: Some(60.2),
        }
    }

    fn input() -> CheckInput {
        let jobs: Vec<Job> = (0..5).map(job).collect();
        CheckInput {
            set: IDS.iter().map(|s| (*s).to_owned()).collect(),
            batch: "acc-1700000000-100".into(),
            expected: jobs.iter().map(|j| (j.id, Ok(60.0))).collect(),
            jobs,
            maps: HashMap::new(),
            processes: Ok(vec![
                ProcInfo {
                    pid: 4,
                    exe_name: "System".into(),
                },
                ProcInfo {
                    pid: 9,
                    exe_name: "steam.exe".into(),
                },
            ]),
            restart_from: None,
        }
    }

    fn run(i: &CheckInput) -> Report {
        evaluate(i, &|_| Ok(good_probe()))
    }

    fn assert_fails(i: &CheckInput, probe: &dyn Fn(&Path) -> Result<Probe, String>, needle: &str) {
        let r = evaluate(i, probe);
        assert!(!r.passed, "должно не пройти: {needle}");
        assert!(
            r.problems.iter().any(|p| p.contains(needle)),
            "нет «{needle}» в {:#?}",
            r.problems
        );
    }

    #[test]
    fn five_good_jobs_pass_without_auto_retries() {
        let r = run(&input());
        assert!(r.passed, "{:#?}", r.problems);
        assert_eq!((r.rows.len(), r.auto_retries), (5, 0));
        assert!(
            r.rows
                .iter()
                .all(|row| row.measured == Some(60.2) && row.expected == Some(60.0))
        );
    }

    #[test]
    fn a_job_that_is_not_done_fails() {
        let mut i = input();
        i.jobs[2].status = JobStatus::Failed;
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            "задача #3: статус «ошибка», нужен «готово»",
        );
    }

    #[test]
    fn a_missing_video_file_fails() {
        let i = input();
        assert_fails(
            &i,
            &|p| Err(format!("файла нет: {}", p.display())),
            "ролика нет",
        );
        let mut i = input();
        i.jobs[0].result = None;
        assert_fails(&i, &|_| Ok(good_probe()), "задача #1: ролика нет");
    }

    #[test]
    fn a_video_without_sound_fails() {
        let silent = Probe {
            streams: vec![Stream {
                codec_type: "video".into(),
                ..Stream::default()
            }],
            duration: Some(60.0),
        };
        let r = evaluate(&input(), &|_| Ok(silent.clone()));
        assert!(
            r.problems.iter().any(|p| p.contains("нет звука")),
            "{:#?}",
            r.problems
        );
    }

    #[test]
    fn a_duration_outside_the_tolerance_names_the_numbers() {
        let long = Probe {
            duration: Some(75.0),
            ..good_probe()
        };
        let r = evaluate(&input(), &|_| Ok(long.clone()));
        let p = r
            .problems
            .iter()
            .find(|p| p.contains("вне допуска"))
            .expect("нарушение длительности");
        assert!(
            p.contains("75.0") && p.contains("60.0") && p.contains("±6.0"),
            "{p}"
        );
        // внутри допуска (±10%) проходит
        let ok = Probe {
            duration: Some(65.9),
            ..good_probe()
        };
        assert!(evaluate(&input(), &|_| Ok(ok.clone())).passed);
    }

    #[test]
    fn two_identical_maps_fail() {
        let mut i = input();
        i.jobs[3].map = "De_Mirage".into();
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            "две одинаковые карты: de_mirage (задачи #1 и #4)",
        );
    }

    #[test]
    fn the_plan_map_wins_over_the_job_map() {
        let mut i = input();
        i.jobs[3].map = "de_mirage".into();
        i.maps.insert(4, "de_dust2".into());
        assert!(run(&i).passed);
        let mut i = input();
        i.maps.insert(4, "de_mirage".into());
        assert_fails(&i, &|_| Ok(good_probe()), "две одинаковые карты");
    }

    #[test]
    fn a_running_game_fails_case_insensitively() {
        let mut i = input();
        i.processes = Ok(vec![ProcInfo {
            pid: 77,
            exe_name: "CS2.EXE".into(),
        }]);
        assert_fails(&i, &|_| Ok(good_probe()), "запущен CS2.EXE (PID 77)");
        i.processes = Ok(vec![ProcInfo {
            pid: 78,
            exe_name: "hlae.exe".into(),
        }]);
        assert_fails(&i, &|_| Ok(good_probe()), "запущен hlae.exe (PID 78)");
        i.processes = Err("access denied".into());
        assert_fails(&i, &|_| Ok(good_probe()), "снимок процессов недоступен");
    }

    #[test]
    fn manual_retry_or_redo_in_the_journal_fails() {
        for kind in [StartKind::Retry, StartKind::Redo] {
            let mut i = input();
            i.jobs[1].attempt = 2;
            i.jobs[1].auto_retries = 1; // счётчики сходятся, выдаёт только журнал
            i.jobs[1].log = format!(
                "{}\n{}",
                first_log(),
                journal::separator(2, "2026-01-01 10:05:00", kind)
            );
            assert_fails(
                &i,
                &|_| Ok(good_probe()),
                &format!("ручное действие: задача #2, попытка 2 — {}", kind.title()),
            );
        }
    }

    #[test]
    fn attempt_counters_must_agree_with_the_journal_and_auto_retries() {
        let mut i = input();
        i.jobs[0].attempt = 2;
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            "в журнале 1 попыток, а у задачи попытка 2",
        );
        assert_fails(&i, &|_| Ok(good_probe()), "попытка 2 при 0 авто-повторах");
    }

    #[test]
    fn the_batch_must_hold_exactly_the_set() {
        let mut i = input();
        i.jobs.pop();
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            "в партии 4 задач, в наборе 5 матчей",
        );
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            &format!("в партии нет матча {} из набора", IDS[4]),
        );
        let mut i = input();
        i.jobs[4].source = JobSource::Faceit("1-ffffffff-ffff-ffff-ffff-ffffffffffff".into());
        assert_fails(&i, &|_| Ok(good_probe()), "в партии лишний матч 1-ffffffff");
        let mut i = input();
        i.set.pop();
        assert_fails(&i, &|_| Ok(good_probe()), "4 матчей из 5");
    }

    #[test]
    fn an_auto_retried_job_that_finished_passes_and_reports_the_failed_attempt() {
        let mut i = input();
        i.jobs[2].attempt = 2;
        i.jobs[2].auto_retries = 1;
        i.jobs[2].log = format!(
            "{}\n{}\n{}\n{}",
            journal::separator(1, "2026-01-01 10:00:00", StartKind::First),
            journal::outcome_line(1, "сбой CONSOLE_NOT_OPEN консоль не открылась"),
            journal::separator(2, "2026-01-01 10:03:00", StartKind::Auto),
            journal::outcome_line(2, "готово"),
        );
        let r = run(&i);
        assert!(r.passed, "{:#?}", r.problems);
        assert_eq!(r.auto_retries, 1);
        assert_eq!(
            r.rows[2].failed_attempts,
            vec!["попытка 1: сбой CONSOLE_NOT_OPEN консоль не открылась".to_owned()]
        );
        assert!(r.rows[0].failed_attempts.is_empty());
    }

    #[test]
    fn series_b_must_come_from_another_process_and_start_after_series_a() {
        let a_jobs: Vec<Job> = (0..5).map(job).collect(); // updated_at до 2005
        let mut i = input();
        i.batch = "acc-1700009999-200".into();
        for j in &mut i.jobs {
            j.created_at = 5000;
        }
        i.restart_from = Some(("acc-1700000000-100".into(), a_jobs.clone()));
        assert!(run(&i).passed, "{:#?}", run(&i).problems);

        let mut same_pid = input();
        same_pid.batch = "acc-1700009999-100".into();
        same_pid.restart_from = Some(("acc-1700000000-100".into(), a_jobs.clone()));
        for j in &mut same_pid.jobs {
            j.created_at = 5000;
        }
        assert_fails(
            &same_pid,
            &|_| Ok(good_probe()),
            "в том же запуске приложения",
        );

        let mut early = input();
        early.batch = "acc-1700009999-200".into();
        early.restart_from = Some(("acc-1700000000-100".into(), a_jobs));
        for j in &mut early.jobs {
            j.created_at = 1500;
        }
        assert_fails(&early, &|_| Ok(good_probe()), "раньше конца серии A");

        let mut junk = input();
        junk.batch = "своя-партия".into();
        junk.restart_from = Some(("acc-1700000000-100".into(), vec![job(0)]));
        assert_fails(&junk, &|_| Ok(good_probe()), "PID приложения не разобран");
    }

    #[test]
    fn unknown_expected_duration_is_a_violation_not_a_pass() {
        let mut i = input();
        i.expected.insert(1, Err("record.json нет".into()));
        assert_fails(
            &i,
            &|_| Ok(good_probe()),
            "задача #1: ожидаемая длительность неизвестна: record.json нет",
        );
    }

    #[test]
    fn check_args_parse_and_reject() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let a = parse_check(&s(&[
            "--db",
            "x.db",
            "--last",
            "--expect-restart-from",
            "acc-1-2",
            "--set",
            "m.txt",
        ]))
        .unwrap();
        assert_eq!(
            a,
            CheckArgs {
                db: Some("x.db".into()),
                batch: None,
                restart_from: Some("acc-1-2".into()),
                set: Some("m.txt".into())
            }
        );
        assert!(parse_check(&s(&["--batch", "a", "--last"])).is_err());
        assert!(
            parse_check(&s(&["--db"]))
                .unwrap_err()
                .contains("требует значение")
        );
        assert!(
            parse_check(&s(&["--nope"]))
                .unwrap_err()
                .contains("неизвестный параметр")
        );
        assert!(parse_check(&s(&["лишнее"])).is_err());
    }

    #[test]
    fn data_dir_follows_the_app_rules() {
        let e = |pairs: &'static [(&str, &str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            data_dir(&e(&[("LOCALAPPDATA", "C:/L"), ("HOME", "/h")])),
            PathBuf::from("C:/L").join("cs2cinema")
        );
        assert_eq!(
            data_dir(&e(&[("XDG_DATA_HOME", "/x")])),
            PathBuf::from("/x").join("cs2cinema")
        );
        assert_eq!(
            data_dir(&e(&[("HOME", "/h")])),
            PathBuf::from("/h").join(".local/share").join("cs2cinema")
        );
    }

    #[test]
    fn report_file_name_is_sanitized_and_json_has_no_secrets() {
        assert_eq!(file_part("acc-1-2"), "acc-1-2");
        assert_eq!(file_part("../../x y"), "______x_y");
        let r = run(&input());
        let v = report_json(&r);
        assert_eq!(v["passed"], true);
        assert_eq!(v["jobs"].as_array().unwrap().len(), 5);
        let text = v.to_string();
        assert!(!text.contains("faceit_key") && !text.contains("hero_name"));
        let dir = std::env::temp_dir().join(format!("cs2cinema-acc-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = r;
        r.batch = "../evil".into();
        let p = write_report(&dir, &r).unwrap();
        assert_eq!(p.parent().unwrap(), dir, "отчёт не выходит из папки");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Файл базы во временной папке; убирается вместе с `-wal`/`-shm`.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("cs2cinema-acc-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDb(dir.join("cs2cinema.db"))
        }
        fn bytes(&self, suffix: &str) -> Vec<u8> {
            let mut s = self.0.clone().into_os_string();
            s.push(suffix);
            std::fs::read(PathBuf::from(s)).unwrap_or_default()
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn check_on_a_real_database_fails_without_changing_it() {
        let db = TempDb::new("e2e");
        let w = Store::open(&db.0).unwrap();
        let new = |i: usize| NewJob {
            kind: JobKind::Record,
            source: JobSource::Faceit(IDS[i].to_owned()),
            label: "приёмка".into(),
            hero_name: "me".into(),
            hero_steam: 1,
            map: MAPS[i].to_owned(),
        };
        let a = w.add_job(&new(0)).unwrap();
        w.set_batch(a, "acc-1700000000-100").unwrap();
        let before = (db.bytes(""), db.bytes("-wal"));
        let job_before = w.job(a).unwrap().unwrap();

        let set: Vec<String> = IDS.iter().map(|s| (*s).to_owned()).collect();
        let (r, code) = check_with(
            &db.0,
            None,
            None,
            Ok(set.clone()),
            &|_| Ok(good_probe()),
            Ok(vec![]),
        );
        assert_eq!(
            (code, r.passed, r.batch.as_str()),
            (1, false, "acc-1700000000-100")
        );
        assert!(
            r.problems.iter().any(|p| p.contains("в партии 1 задач")),
            "{:#?}",
            r.problems
        );
        assert!(
            r.problems
                .iter()
                .any(|p| p.contains("задача #1: статус «в очереди»")),
            "{:#?}",
            r.problems
        );

        assert_eq!(
            (db.bytes(""), db.bytes("-wal")),
            before,
            "проверка не меняет файлы базы"
        );
        assert_eq!(
            w.job(a).unwrap().unwrap(),
            job_before,
            "писатель видит прежнее"
        );

        // ошибки: нет партии, нет базы, плохой набор — код 2
        let (_, code) = check_with(
            &db.0,
            Some("acc-0-0"),
            None,
            Ok(set.clone()),
            &|_| Ok(good_probe()),
            Ok(vec![]),
        );
        assert_eq!(code, 2);
        let (_, code) = check_with(
            &db.0.with_file_name("нет.db"),
            None,
            None,
            Ok(set),
            &|_| Ok(good_probe()),
            Ok(vec![]),
        );
        assert_eq!(code, 2);
        let (_, code) = check_with(
            &db.0,
            None,
            None,
            Err("строка 1".into()),
            &|_| Ok(good_probe()),
            Ok(vec![]),
        );
        assert_eq!(code, 2);
    }
}
