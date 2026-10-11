//! Командная строка оператора: `cs2-server submit | job | runner | user | ...`.
//!
//! Разбор вручную, как в `cs2-cli`: ошибка разбора — «ошибка: …» и справка с кодом 2,
//! ошибка выполнения — «ошибка: …» с кодом 1. Все строки «ошибка:» проходят `Redactor` процесса.

use crate::Result;
use crate::auth;
use crate::clock::SystemClock;
use crate::config::{self, Flags};
use crate::core::{Core, CoreOptions, FileRow, JobRow, Submit};
use crate::log::{self, StderrSink};
use crate::secret::Redactor;
use crate::users;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const USAGE: &str = "\
cs2-server — сервер очереди монтажа

Команды:
  serve [--bind {адрес}] [--threads {N}]               запустить HTTP-сервер для раннеров (Ctrl+C — остановка)
  submit {демка} --player {ник|steamid} [--user {имя}]   поставить демку в очередь
  job list                                               список задач
  job show {id}                                          подробности задачи
  job cancel {id} [--force]                              отменить задачу (выданной раннеру — флагом; повторно или с --force — сразу)
  runner add {имя}                                       выдать раннеру токен (показывается один раз)
  runner list                                            раннеры и отпечатки их токенов
  runner revoke {имя}                                    отозвать токен раннера
  user add {имя} [--tg {id}] [--admin]                   добавить пользователя
  user list                                              список пользователей
  config show                                            итоговые настройки (токен бота скрыт)

Общие флаги:
  --config {файл}      файл настроек (JSON; образец — cs2-server.example.json)
  --data-dir {папка}   папка данных (база server.db и blobs/)
  --bind {адрес}       адрес HTTP (по умолчанию 127.0.0.1:8080)
  --threads {n}        число обработчиков HTTP (по умолчанию 16)
  -h, --help           эта справка

Порядок настроек: умолчания < файл < переменные окружения < флаги.
Переменные окружения: CS2_SERVER_CONFIG, CS2_SERVER_BIND, CS2_SERVER_DATA_DIR,
  CS2_SERVER_THREADS, CS2_SERVER_LEASE_TTL_SECS, CS2_SERVER_HEARTBEAT_SECS,
  CS2_SERVER_LONGPOLL_SECS, CS2_SERVER_MAX_GRANTS, CS2_SERVER_STARTUP_GRACE_SECS,
  CS2_SERVER_MAX_DEMO_MIB, CS2_SERVER_MAX_RESULT_MIB, TELEGRAM_BOT_TOKEN
  (или CS2_SERVER_TELEGRAM_BOT_TOKEN), STATE_DIRECTORY (папка данных по умолчанию).
";

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    config: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    player: Option<String>,
    user: Option<String>,
    tg: Option<i64>,
    admin: bool,
    force: bool,
    bind: Option<String>,
    threads: Option<usize>,
}

enum CliError {
    /// Ошибка разбора: печатается справка.
    Usage(String),
    /// Ошибка выполнения.
    Run(String),
}

impl From<String> for CliError {
    fn from(s: String) -> Self {
        CliError::Run(s)
    }
}

fn parse_args(args: Vec<String>) -> std::result::Result<Args, CliError> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .ok_or_else(|| CliError::Usage(format!("{name} требует значение")))
        };
        match a.as_str() {
            "-h" | "--help" => return Err(CliError::Usage(String::new())),
            "--config" => out.config = Some(PathBuf::from(value("--config")?)),
            "--data-dir" => out.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--player" => out.player = Some(value("--player")?),
            "--user" => out.user = Some(value("--user")?),
            "--tg" => {
                out.tg = Some(
                    value("--tg")?
                        .parse()
                        .map_err(|_| CliError::Usage("--tg: нужно целое число".to_owned()))?,
                )
            }
            "--admin" => out.admin = true,
            "--force" => out.force = true,
            "--bind" => out.bind = Some(value("--bind")?),
            "--threads" => {
                out.threads = Some(
                    value("--threads")?
                        .parse()
                        .map_err(|_| CliError::Usage("--threads: нужно целое число".to_owned()))?,
                )
            }
            s if s.starts_with("--") => {
                return Err(CliError::Usage(format!("неизвестный флаг {s}")));
            }
            _ => out.positional.push(a),
        }
    }
    Ok(out)
}

pub fn run_cli(args: Vec<String>, env: &dyn Fn(&str) -> Option<String>) -> ExitCode {
    let mut redactor = Redactor::default();
    let parsed = parse_args(args).and_then(|a| execute(a, env, &mut redactor));
    match parsed {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(msg)) => {
            if !msg.is_empty() {
                eprintln!("ошибка: {}", redactor.redact(&msg));
            }
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Run(e)) => {
            eprintln!("ошибка: {}", redactor.redact(&e));
            ExitCode::FAILURE
        }
    }
}

/// Загружает настройки, ставит `Redactor` процесса и печатает предупреждения.
fn load_config(
    a: &Args,
    env: &dyn Fn(&str) -> Option<String>,
    redactor: &mut Redactor,
) -> Result<config::Loaded> {
    let flags = Flags {
        config: a.config.clone(),
        data_dir: a.data_dir.clone(),
        bind: a.bind.clone(),
        threads: a.threads,
    };
    let loaded = config::load(&flags, env)?;
    *redactor = loaded.config.redactor();
    for w in &loaded.warnings {
        eprintln!("внимание: {}", redactor.redact(w));
    }
    Ok(loaded)
}

fn open_core(
    a: &Args,
    env: &dyn Fn(&str) -> Option<String>,
    redactor: &mut Redactor,
) -> Result<Core> {
    let loaded = load_config(a, env, redactor)?;
    Core::open(CoreOptions {
        data_dir: loaded.config.data_dir.clone(),
        limits: loaded.config.limits(),
        clock: Arc::new(SystemClock),
        redactor: redactor.clone(),
    })
}

/// `config show`: итоговые настройки; секрет печатается как `***`.
fn show_config(loaded: &config::Loaded) {
    let c = &loaded.config;
    match &loaded.file {
        Some(p) => println!("файл настроек: {}", p.display()),
        None => println!("файл настроек: нет"),
    }
    println!("bind = {}", c.bind);
    println!("data_dir = {}", c.data_dir.display());
    println!("threads = {}", c.threads);
    println!("lease_ttl_secs = {}", c.lease_ttl_secs);
    println!("heartbeat_secs = {}", c.heartbeat_secs);
    println!("longpoll_secs = {}", c.longpoll_secs);
    println!("max_grants = {}", c.max_grants);
    println!("startup_grace_secs = {}", c.startup_grace_secs);
    println!("max_demo_mib = {}", c.max_demo_mib);
    println!("max_result_mib = {}", c.max_result_mib);
    match &c.telegram_bot_token {
        Some(t) => println!("telegram_bot_token = {t}"),
        None => println!("telegram_bot_token = не задан"),
    }
}

fn execute(
    a: Args,
    env: &dyn Fn(&str) -> Option<String>,
    redactor: &mut Redactor,
) -> std::result::Result<(), CliError> {
    let words: Vec<&str> = a.positional.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["serve"] => {
            let loaded = load_config(&a, env, redactor)?;
            let running =
                crate::serve(&loaded.config, Arc::new(SystemClock), Arc::new(StderrSink))?;
            log::install_panic_hook(running.log().clone());
            // Первая строка stdout — адрес: по ней тесты и скрипты узнают настоящий порт.
            println!("cs2-server слушает http://{}", running.addr());
            let _ = std::io::stdout().flush();
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
                .map_err(|e| CliError::Run(format!("обработчик Ctrl+C: {e}")))?;
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
            running.stop();
            println!("cs2-server остановлен");
            Ok(())
        }
        ["submit", file] => {
            let player = a
                .player
                .clone()
                .ok_or_else(|| CliError::Usage("submit требует --player".to_owned()))?;
            let core = open_core(&a, env, redactor)?;
            let (user_id, requested_by) = match &a.user {
                Some(name) => {
                    let u = users::find_user(core.db(), name)?
                        .ok_or_else(|| CliError::Run(format!("пользователь «{name}» не найден")))?;
                    (Some(u.id), u.name)
                }
                None => (None, "оператор".to_owned()),
            };
            let path = PathBuf::from(file);
            let orig_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let sub = Submit {
                player,
                user_id,
                requested_by,
                orig_name: orig_name.clone(),
            };
            let (id, got) = core.submit(&path, &sub)?;
            println!(
                "задача #{id} в очереди: {orig_name}, {:.1} МиБ, sha256 {}",
                got.size as f64 / (1024.0 * 1024.0),
                got.sha256
            );
            Ok(())
        }
        ["job", "list"] => {
            let core = open_core(&a, env, redactor)?;
            let jobs = core.jobs()?;
            if jobs.is_empty() {
                println!("задач нет");
            }
            for j in jobs {
                println!(
                    "#{}  {} ({})  {}  {}  {}",
                    j.id,
                    j.state.title(),
                    j.state.as_str(),
                    j.player_name,
                    j.requested_by,
                    utc(j.created_at)
                );
            }
            Ok(())
        }
        ["job", "show", id] => {
            let id: i64 = id
                .parse()
                .map_err(|_| CliError::Usage("job show: нужен числовой id".to_owned()))?;
            let core = open_core(&a, env, redactor)?;
            let job = core
                .job(id)?
                .ok_or_else(|| CliError::Run(format!("задачи #{id} нет")))?;
            let demo = core.demo_file(id)?;
            print_job(&job, demo.as_ref());
            Ok(())
        }
        ["job", "cancel", id] => {
            let id: i64 = id
                .parse()
                .map_err(|_| CliError::Usage("job cancel: нужен числовой id".to_owned()))?;
            let core = open_core(&a, env, redactor)?;
            let state = core.cancel(id, a.force)?;
            if state == wire::JobState::Cancelled {
                println!("задача #{id}: отменена");
            } else {
                let mode = match core.job(id)?.map(|j| j.cancel_requested) {
                    Some(wire::CancelMode::Force) => "сразу",
                    _ => "мягко",
                };
                println!(
                    "задача #{id}: отмена запрошена ({mode}) — раннер узнает при следующем прогрессе"
                );
            }
            Ok(())
        }
        ["runner", "add", name] => {
            let core = open_core(&a, env, redactor)?;
            let (runner, token) = auth::add_runner(core.db(), name, core.clock().now_ms())?;
            println!("раннер «{}» добавлен", runner.name);
            println!("{token}");
            println!(
                "токен показывается один раз; на сервере хранится только его хеш (отпечаток {})",
                runner.fingerprint
            );
            Ok(())
        }
        ["runner", "list"] => {
            let core = open_core(&a, env, redactor)?;
            let list = auth::list_runners(core.db())?;
            if list.is_empty() {
                println!("раннеров нет");
            }
            for r in list {
                let seen = r
                    .last_seen_at
                    .map(utc)
                    .unwrap_or_else(|| "не входил".to_owned());
                let revoked = match r.revoked_at {
                    Some(t) => format!("  отозван {}", utc(t)),
                    None => String::new(),
                };
                println!(
                    "{}  отпечаток {}  создан {}  последний вход: {}{}",
                    r.name,
                    r.fingerprint,
                    utc(r.created_at),
                    seen,
                    revoked
                );
            }
            Ok(())
        }
        ["runner", "revoke", name] => {
            let core = open_core(&a, env, redactor)?;
            auth::revoke_runner(core.db(), name, core.clock().now_ms())?;
            println!("раннер «{name}» отозван");
            Ok(())
        }
        ["user", "add", name] => {
            let core = open_core(&a, env, redactor)?;
            let u = users::add_user(core.db(), name, a.tg, a.admin, core.clock().now_ms())?;
            println!("пользователь «{}» добавлен ({})", u.name, u.role);
            Ok(())
        }
        ["user", "list"] => {
            let core = open_core(&a, env, redactor)?;
            let list = users::list_users(core.db())?;
            if list.is_empty() {
                println!("пользователей нет");
            }
            for u in list {
                let tg = u.tg_id.map(|t| format!("  tg {t}")).unwrap_or_default();
                println!("{}  {}{}  создан {}", u.name, u.role, tg, utc(u.created_at));
            }
            Ok(())
        }
        ["config", "show"] => {
            let loaded = load_config(&a, env, redactor)?;
            show_config(&loaded);
            Ok(())
        }
        [] => Err(CliError::Usage(String::new())),
        other => Err(CliError::Usage(format!(
            "неизвестная команда «{}»",
            other.join(" ")
        ))),
    }
}

fn print_job(j: &JobRow, demo: Option<&FileRow>) {
    println!("задача #{}", j.id);
    println!("состояние: {} ({})", j.state.title(), j.state.as_str());
    println!("игрок: {}", j.player_name);
    if let Some(s) = &j.player_steam {
        println!("steamid: {s}");
    }
    println!("источник: {} {}", j.source_kind, j.orig_name);
    println!("заказал: {}", j.requested_by);
    println!("создана: {}", utc(j.created_at));
    println!("обновлена: {}", utc(j.updated_at));
    if let Some(t) = j.finished_at {
        println!("закончена: {}", utc(t));
    }
    println!("эпоха: {}, выдач: {}", j.epoch, j.grants);
    if j.cancel_requested != wire::CancelMode::NotRequested {
        println!("отмена запрошена: {}", j.cancel_requested.as_str());
    }
    if let Some(d) = demo {
        println!("демка: {:.1} МиБ", d.size as f64 / (1024.0 * 1024.0));
        println!("sha256: {}", d.sha256);
    }
    for (k, v) in [
        ("код сбоя", &j.fail_code),
        ("класс сбоя", &j.fail_class),
        ("состояние при сбое", &j.fail_state),
        ("подсказка", &j.fail_hint),
        ("сообщение", &j.fail_message),
        ("хвост журнала", &j.fail_tail),
    ] {
        if let Some(v) = v {
            println!("{k}: {v}");
        }
    }
    if let Some(ms) = j.fail_elapsed_ms {
        println!("прошло до сбоя: {} мс", ms);
    }
}

/// «2026-10-11 15:04:05 UTC» из unix-миллисекунд (без chrono: серверу хватает григорианской арифметики).
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Алгоритм civil_from_days (Говард Хиннант).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_700_000_000_000), "2023-11-14 22:13:20 UTC");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00 UTC");
    }
}
