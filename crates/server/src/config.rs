//! Настройки сервера (SRV-01): умолчания < файл JSON < переменные окружения < флаги.
//!
//! Окружение передаётся замыканием, чтобы тесты не трогали переменные процесса.
//! Ошибки разбора называют место (файл, строка, столбец, переменная) и никогда не повторяют значения:
//! в настройках лежит токен бота (SEC-04).

use crate::Result;
use crate::bot::{BotSettings, SendAs};
use crate::core::Limits;
use crate::secret::{Redactor, Secret};
use crate::tg::http::{HttpTimeouts, check_base_url};
use serde::Deserialize;
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// Файл настроек больше этого размера не читается.
const MAX_FILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub threads: usize,
    pub lease_ttl_secs: u64,
    pub heartbeat_secs: u64,
    pub longpoll_secs: u64,
    pub max_grants: u32,
    pub startup_grace_secs: u64,
    pub max_demo_mib: u64,
    pub max_result_mib: u64,
    /// Токен Telegram-бота (понадобится в фазе 7); `Debug` у `Secret` печатает `***`.
    pub telegram_bot_token: Option<Secret>,
    /// Адрес локального `telegram-bot-api` (`http://127.0.0.1:порт`); нет — бот не запускается.
    pub bot_api_url: Option<String>,
    /// Корень файлов `telegram-bot-api`, как его возвращает `getFile` (обязателен при включённом боте).
    pub bot_api_files_root: Option<PathBuf>,
    /// Тот же корень на стороне сервера; нет — равен `bot_api_files_root`.
    pub bot_api_files_root_local: Option<PathBuf>,
    /// Папка результатов сервера в глазах `telegram-bot-api`; нет — `data_dir/blobs/results`.
    pub bot_api_results_prefix: Option<PathBuf>,
    pub bot_send_as: SendAs,
    pub bot_user_max_active: u32,
    pub bot_admin_max_active: u32,
    pub bot_queue_max_active: u32,
    pub bot_status_edit_secs: u64,
    pub bot_transfer_timeout_mins: u64,
    /// Сколько часов после окончания хранится демка готовой или отменённой задачи.
    pub retention_demo_hours: u64,
    /// Сколько часов после сбоя хранится демка упавшей задачи (запас на `/retry`).
    pub retention_failed_demo_hours: u64,
    /// Сколько часов после доставки (`sent` с `file_id`) хранится ролик задачи бота.
    pub retention_result_hours: u64,
    /// Сколько часов хранится ролик, который не ушёл в Telegram: задача без бота, неудавшаяся доставка.
    pub retention_unsent_result_hours: u64,
    /// Через сколько часов удаляются недозагруженные `*.part` и остатки прерванной уборки.
    pub retention_part_hours: u64,
    /// Через сколько часов удаляются файлы рабочей папки `telegram-bot-api`.
    pub retention_botapi_hours: u64,
    /// Как часто поток уборки проходит по базе и файлам, секунд.
    pub clean_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // D-03: по умолчанию только loopback.
            bind: "127.0.0.1:8080".to_owned(),
            data_dir: PathBuf::from("./data"),
            // D-02: пул обработчиков.
            threads: 16,
            lease_ttl_secs: 45,
            heartbeat_secs: 10,
            longpoll_secs: 25,
            max_grants: 3,
            startup_grace_secs: 90,
            max_demo_mib: 2000,
            max_result_mib: 2000,
            telegram_bot_token: None,
            bot_api_url: None,
            bot_api_files_root: None,
            bot_api_files_root_local: None,
            bot_api_results_prefix: None,
            // Решение спайка 07-11: по умолчанию видео, документом — если сжатие мешает.
            bot_send_as: SendAs::Video,
            // D-03: лимиты активных задач.
            bot_user_max_active: 2,
            bot_admin_max_active: 5,
            bot_queue_max_active: 10,
            bot_status_edit_secs: 10,
            bot_transfer_timeout_mins: 60,
            // Решение 09-02 (RESEARCH A1): сроки хранения; автор меняет их в файле настроек.
            retention_demo_hours: 24,
            retention_failed_demo_hours: 72,
            retention_result_hours: 24,
            retention_unsent_result_hours: 168,
            retention_part_hours: 6,
            retention_botapi_hours: 6,
            clean_interval_secs: 600,
        }
    }
}

impl Config {
    /// Настройки бота из ключей (или умолчаний). Корень файлов обязателен только при включённом боте,
    /// это проверяют `validate` и `serve_with_bot`; здесь пустой путь.
    pub fn bot_settings(&self) -> BotSettings {
        let root = self.bot_api_files_root.clone().unwrap_or_default();
        let mut s = BotSettings::new(&self.data_dir, root.clone());
        s.files_root_local = self
            .bot_api_files_root_local
            .clone()
            .unwrap_or_else(|| root.clone());
        if let Some(p) = &self.bot_api_results_prefix {
            s.results_prefix = p.clone();
        }
        s.send_as = self.bot_send_as;
        s.status_edit_ms = self.bot_status_edit_secs as i64 * 1000;
        s.user_max_active = self.bot_user_max_active as i64;
        s.admin_max_active = self.bot_admin_max_active as i64;
        s.queue_max_active = self.bot_queue_max_active as i64;
        s
    }

    /// Сроки вызовов Bot API: передача файла — `bot_transfer_timeout_mins`, остальное — умолчания клиента.
    pub fn http_timeouts(&self) -> HttpTimeouts {
        HttpTimeouts {
            transfer: Duration::from_secs(self.bot_transfer_timeout_mins * 60),
            ..HttpTimeouts::default()
        }
    }

    pub fn limits(&self) -> Limits {
        const MIB: u64 = 1024 * 1024;
        Limits {
            lease_ttl_ms: self.lease_ttl_secs as i64 * 1000,
            heartbeat_secs: self.heartbeat_secs,
            longpoll_secs: self.longpoll_secs,
            max_grants: self.max_grants as i64,
            startup_grace_ms: self.startup_grace_secs as i64 * 1000,
            max_demo_bytes: self.max_demo_mib * MIB,
            max_result_bytes: self.max_result_mib * MIB,
        }
    }

    /// Сроки хранения для уборщика: часы настроек в миллисекундах.
    pub fn retention(&self) -> crate::clean::Retention {
        const HOUR_MS: i64 = 3_600_000;
        crate::clean::Retention {
            demo_ms: self.retention_demo_hours as i64 * HOUR_MS,
            failed_demo_ms: self.retention_failed_demo_hours as i64 * HOUR_MS,
            result_ms: self.retention_result_hours as i64 * HOUR_MS,
            unsent_result_ms: self.retention_unsent_result_hours as i64 * HOUR_MS,
            part_ms: self.retention_part_hours as i64 * HOUR_MS,
            botapi_ms: self.retention_botapi_hours as i64 * HOUR_MS,
        }
    }

    /// Маскировщик со всеми секретами настроек.
    pub fn redactor(&self) -> Redactor {
        let secrets: Vec<&Secret> = self.telegram_bot_token.iter().collect();
        Redactor::new(&secrets)
    }
}

/// Значения флагов командной строки — последний, самый сильный уровень.
#[derive(Default)]
pub struct Flags {
    pub config: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub bind: Option<String>,
    pub threads: Option<usize>,
}

pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
    pub file: Option<PathBuf>,
}

/// Содержимое файла настроек: все ключи необязательные, неизвестные — ошибка.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct FileConfig {
    bind: Option<String>,
    data_dir: Option<PathBuf>,
    threads: Option<usize>,
    lease_ttl_secs: Option<u64>,
    heartbeat_secs: Option<u64>,
    longpoll_secs: Option<u64>,
    max_grants: Option<u32>,
    startup_grace_secs: Option<u64>,
    max_demo_mib: Option<u64>,
    max_result_mib: Option<u64>,
    telegram_bot_token: Option<String>,
    bot_api_url: Option<String>,
    bot_api_files_root: Option<PathBuf>,
    bot_api_files_root_local: Option<PathBuf>,
    bot_api_results_prefix: Option<PathBuf>,
    bot_send_as: Option<String>,
    bot_user_max_active: Option<u32>,
    bot_admin_max_active: Option<u32>,
    bot_queue_max_active: Option<u32>,
    bot_status_edit_secs: Option<u64>,
    bot_transfer_timeout_mins: Option<u64>,
    retention_demo_hours: Option<u64>,
    retention_failed_demo_hours: Option<u64>,
    retention_result_hours: Option<u64>,
    retention_unsent_result_hours: Option<u64>,
    retention_part_hours: Option<u64>,
    retention_botapi_hours: Option<u64>,
    clean_interval_secs: Option<u64>,
}

fn parse_send_as(v: &str) -> Result<SendAs> {
    SendAs::parse(v.trim()).ok_or_else(|| "bot_send_as: нужно video или document".to_owned())
}

fn read_file(path: &std::path::Path) -> Result<FileConfig> {
    let shown = path.display();
    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("файл настроек {shown} не найден"),
        _ => format!("файл настроек {shown}: не удалось открыть ({})", e.kind()),
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("файл настроек {shown}: не удалось прочитать ({})", e.kind()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "файл настроек {shown} больше {} КиБ",
            MAX_FILE_BYTES / 1024
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        // Текст ошибки serde_json целиком не выводим: в нём бывают значения.
        let (l, c) = (e.line(), e.column());
        match e.classify() {
            serde_json::error::Category::Data => format!(
                "{shown}: строка {l}, столбец {c}: неизвестный ключ или неверный тип значения (образец — cs2-server.example.json)"
            ),
            serde_json::error::Category::Io => format!("{shown}: ошибка чтения"),
            _ => format!("{shown}: строка {l}, столбец {c}: неверный JSON"),
        }
    })
}

fn env_num<T: FromStr>(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Result<Option<T>> {
    match env(name).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse::<T>()
            .map(Some)
            .map_err(|_| format!("переменная {name}: нужно целое число")),
    }
}

pub fn load(flags: &Flags, env: &dyn Fn(&str) -> Option<String>) -> Result<Loaded> {
    let mut c = Config::default();
    let mut warnings = Vec::new();

    // Каталог состояния от systemd — умолчание для папки данных (первый путь списка).
    if let Some(dir) = env("STATE_DIRECTORY")
        .and_then(|v| v.split(':').next().map(str::to_owned))
        .filter(|v| !v.is_empty())
    {
        c.data_dir = PathBuf::from(dir);
    }

    // Файл: указанный, но отсутствующий — ошибка; без указания не читается.
    let file_path = flags.config.clone().or_else(|| {
        env("CS2_SERVER_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    let mut file_token = None;
    if let Some(path) = &file_path {
        let f = read_file(path)?;
        if let Some(v) = f.bind {
            c.bind = v;
        }
        if let Some(v) = f.data_dir {
            c.data_dir = v;
        }
        if let Some(v) = f.threads {
            c.threads = v;
        }
        if let Some(v) = f.lease_ttl_secs {
            c.lease_ttl_secs = v;
        }
        if let Some(v) = f.heartbeat_secs {
            c.heartbeat_secs = v;
        }
        if let Some(v) = f.longpoll_secs {
            c.longpoll_secs = v;
        }
        if let Some(v) = f.max_grants {
            c.max_grants = v;
        }
        if let Some(v) = f.startup_grace_secs {
            c.startup_grace_secs = v;
        }
        if let Some(v) = f.max_demo_mib {
            c.max_demo_mib = v;
        }
        if let Some(v) = f.max_result_mib {
            c.max_result_mib = v;
        }
        file_token = f.telegram_bot_token.filter(|v| !v.is_empty());
        if let Some(v) = f.bot_api_url.filter(|v| !v.is_empty()) {
            c.bot_api_url = Some(v);
        }
        if let Some(v) = f.bot_api_files_root.filter(|v| !v.as_os_str().is_empty()) {
            c.bot_api_files_root = Some(v);
        }
        if let Some(v) = f
            .bot_api_files_root_local
            .filter(|v| !v.as_os_str().is_empty())
        {
            c.bot_api_files_root_local = Some(v);
        }
        if let Some(v) = f
            .bot_api_results_prefix
            .filter(|v| !v.as_os_str().is_empty())
        {
            c.bot_api_results_prefix = Some(v);
        }
        if let Some(v) = f.bot_send_as {
            c.bot_send_as = parse_send_as(&v)?;
        }
        if let Some(v) = f.bot_user_max_active {
            c.bot_user_max_active = v;
        }
        if let Some(v) = f.bot_admin_max_active {
            c.bot_admin_max_active = v;
        }
        if let Some(v) = f.bot_queue_max_active {
            c.bot_queue_max_active = v;
        }
        if let Some(v) = f.bot_status_edit_secs {
            c.bot_status_edit_secs = v;
        }
        if let Some(v) = f.bot_transfer_timeout_mins {
            c.bot_transfer_timeout_mins = v;
        }
        if let Some(v) = f.retention_demo_hours {
            c.retention_demo_hours = v;
        }
        if let Some(v) = f.retention_failed_demo_hours {
            c.retention_failed_demo_hours = v;
        }
        if let Some(v) = f.retention_result_hours {
            c.retention_result_hours = v;
        }
        if let Some(v) = f.retention_unsent_result_hours {
            c.retention_unsent_result_hours = v;
        }
        if let Some(v) = f.retention_part_hours {
            c.retention_part_hours = v;
        }
        if let Some(v) = f.retention_botapi_hours {
            c.retention_botapi_hours = v;
        }
        if let Some(v) = f.clean_interval_secs {
            c.clean_interval_secs = v;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.permissions().mode() & 0o077 != 0 {
                    warnings.push(format!(
                        "файл настроек {} доступен группе или всем: выполните chmod 600",
                        path.display()
                    ));
                }
            }
        }
    }

    // Окружение.
    if let Some(v) = env("CS2_SERVER_BIND").filter(|v| !v.is_empty()) {
        c.bind = v;
    }
    if let Some(v) = env("CS2_SERVER_DATA_DIR").filter(|v| !v.is_empty()) {
        c.data_dir = PathBuf::from(v);
    }
    if let Some(v) = env_num(env, "CS2_SERVER_THREADS")? {
        c.threads = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_LEASE_TTL_SECS")? {
        c.lease_ttl_secs = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_HEARTBEAT_SECS")? {
        c.heartbeat_secs = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_LONGPOLL_SECS")? {
        c.longpoll_secs = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_MAX_GRANTS")? {
        c.max_grants = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_STARTUP_GRACE_SECS")? {
        c.startup_grace_secs = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_MAX_DEMO_MIB")? {
        c.max_demo_mib = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_MAX_RESULT_MIB")? {
        c.max_result_mib = v;
    }
    if let Some(v) = env("CS2_SERVER_BOT_API_URL").filter(|v| !v.is_empty()) {
        c.bot_api_url = Some(v);
    }
    if let Some(v) = env("CS2_SERVER_BOT_API_FILES_ROOT").filter(|v| !v.is_empty()) {
        c.bot_api_files_root = Some(PathBuf::from(v));
    }
    if let Some(v) = env("CS2_SERVER_BOT_API_FILES_ROOT_LOCAL").filter(|v| !v.is_empty()) {
        c.bot_api_files_root_local = Some(PathBuf::from(v));
    }
    if let Some(v) = env("CS2_SERVER_BOT_API_RESULTS_PREFIX").filter(|v| !v.is_empty()) {
        c.bot_api_results_prefix = Some(PathBuf::from(v));
    }
    if let Some(v) = env("CS2_SERVER_BOT_SEND_AS").filter(|v| !v.is_empty()) {
        c.bot_send_as = parse_send_as(&v).map_err(|_| {
            "переменная CS2_SERVER_BOT_SEND_AS: нужно video или document".to_owned()
        })?;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_BOT_USER_MAX_ACTIVE")? {
        c.bot_user_max_active = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_BOT_ADMIN_MAX_ACTIVE")? {
        c.bot_admin_max_active = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_BOT_QUEUE_MAX_ACTIVE")? {
        c.bot_queue_max_active = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_BOT_STATUS_EDIT_SECS")? {
        c.bot_status_edit_secs = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_BOT_TRANSFER_TIMEOUT_MINS")? {
        c.bot_transfer_timeout_mins = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_DEMO_HOURS")? {
        c.retention_demo_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_FAILED_DEMO_HOURS")? {
        c.retention_failed_demo_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_RESULT_HOURS")? {
        c.retention_result_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_UNSENT_RESULT_HOURS")? {
        c.retention_unsent_result_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_PART_HOURS")? {
        c.retention_part_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_RETENTION_BOTAPI_HOURS")? {
        c.retention_botapi_hours = v;
    }
    if let Some(v) = env_num(env, "CS2_SERVER_CLEAN_INTERVAL_SECS")? {
        c.clean_interval_secs = v;
    }
    let env_token = env("CS2_SERVER_TELEGRAM_BOT_TOKEN")
        .filter(|v| !v.is_empty())
        .or_else(|| env("TELEGRAM_BOT_TOKEN").filter(|v| !v.is_empty()));
    c.telegram_bot_token = env_token.or(file_token).map(Secret::new);

    // Флаги.
    if let Some(v) = &flags.bind {
        c.bind = v.clone();
    }
    if let Some(v) = &flags.data_dir {
        c.data_dir = v.clone();
    }
    if let Some(v) = flags.threads {
        c.threads = v;
    }

    validate(&c)?;
    if let Ok(addr) = c.bind.parse::<SocketAddr>() {
        if !addr.ip().is_loopback() {
            warnings.push(format!(
                "{} — не loopback: HTTP без TLS, ставьте впереди Tailscale Serve или Caddy (D-03)",
                c.bind
            ));
        }
    }
    if c.telegram_bot_token.is_some() && c.bot_api_url.is_none() {
        warnings.push("бот не запущен: не задан bot_api_url".to_owned());
    }
    Ok(Loaded {
        config: c,
        warnings,
        file: file_path,
    })
}

/// Проверка согласованности. Сообщения называют ключ и правило, но не повторяют значение.
pub fn validate(c: &Config) -> Result<()> {
    if c.bind.parse::<SocketAddr>().is_err() {
        return Err("bind: нужен адрес вида 127.0.0.1:8080".to_owned());
    }
    if !(1..=256).contains(&c.threads) {
        return Err("threads: нужно число от 1 до 256".to_owned());
    }
    if c.heartbeat_secs < 1 {
        return Err("heartbeat_secs: нужно не меньше 1".to_owned());
    }
    if c.lease_ttl_secs < 2 * c.heartbeat_secs {
        return Err("lease_ttl_secs: нужно не меньше удвоенного heartbeat_secs".to_owned());
    }
    if c.longpoll_secs < 1 || c.longpoll_secs >= c.lease_ttl_secs {
        return Err("longpoll_secs: нужно от 1 и меньше lease_ttl_secs".to_owned());
    }
    if c.max_grants < 1 {
        return Err("max_grants: нужно не меньше 1".to_owned());
    }
    if !(1..=4096).contains(&c.max_demo_mib) {
        return Err("max_demo_mib: нужно число от 1 до 4096".to_owned());
    }
    if !(1..=4096).contains(&c.max_result_mib) {
        return Err("max_result_mib: нужно число от 1 до 4096".to_owned());
    }
    if let Some(url) = &c.bot_api_url {
        check_base_url(url).map_err(|why| {
            format!(
                "bot_api_url: адрес telegram-bot-api должен быть локальным http://127.0.0.1:ПОРТ ({why})"
            )
        })?;
        if c.telegram_bot_token.is_none() {
            return Err(
                "bot_api_url задан, а токена бота нет: задайте TELEGRAM_BOT_TOKEN или telegram_bot_token"
                    .to_owned(),
            );
        }
        match &c.bot_api_files_root {
            None => {
                return Err("bot_api_files_root: при включённом боте нужен абсолютный путь к файлам telegram-bot-api".to_owned());
            }
            Some(p) if !is_abs(p) => {
                return Err("bot_api_files_root: нужен абсолютный путь".to_owned());
            }
            Some(_) => {}
        }
        for (name, p) in [
            ("bot_api_files_root_local", &c.bot_api_files_root_local),
            ("bot_api_results_prefix", &c.bot_api_results_prefix),
        ] {
            if p.as_ref().is_some_and(|p| !is_abs(p)) {
                return Err(format!("{name}: нужен абсолютный путь"));
            }
        }
    }
    for (name, v) in [
        ("bot_user_max_active", c.bot_user_max_active),
        ("bot_admin_max_active", c.bot_admin_max_active),
        ("bot_queue_max_active", c.bot_queue_max_active),
    ] {
        if !(1..=1000).contains(&v) {
            return Err(format!("{name}: нужно число от 1 до 1000"));
        }
    }
    if !(3..=600).contains(&c.bot_status_edit_secs) {
        return Err("bot_status_edit_secs: нужно число от 3 до 600".to_owned());
    }
    if !(1..=240).contains(&c.bot_transfer_timeout_mins) {
        return Err("bot_transfer_timeout_mins: нужно число от 1 до 240".to_owned());
    }
    for (name, v) in [
        ("retention_demo_hours", c.retention_demo_hours),
        ("retention_failed_demo_hours", c.retention_failed_demo_hours),
        ("retention_result_hours", c.retention_result_hours),
        (
            "retention_unsent_result_hours",
            c.retention_unsent_result_hours,
        ),
        ("retention_part_hours", c.retention_part_hours),
        ("retention_botapi_hours", c.retention_botapi_hours),
    ] {
        if !(1..=8760).contains(&v) {
            return Err(format!("{name}: нужно число от 1 до 8760"));
        }
    }
    if !(1..=86400).contains(&c.clean_interval_secs) {
        return Err("clean_interval_secs: нужно число от 1 до 86400".to_owned());
    }
    // Приём большой демки идёт до bot_transfer_timeout_mins: файл bot-api не должен исчезнуть раньше.
    if c.retention_botapi_hours * 60 < 2 * c.bot_transfer_timeout_mins {
        return Err(
            "retention_botapi_hours: нужно не меньше удвоенного bot_transfer_timeout_mins (в часах)"
                .to_owned(),
        );
    }
    Ok(())
}

/// Абсолютный путь: на Windows принимается и вид «/var/lib/…» (корень bot-api может жить в другой системе).
pub(crate) fn is_abs(p: &std::path::Path) -> bool {
    p.is_absolute() || p.has_root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SENTINEL_TOKEN: &str = "123456789:AAEsentinelSENTINELsentinelSENTINEL01";

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn temp_file(tag: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2srv-cfg-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cfg.json");
        std::fs::write(&p, body).unwrap();
        // загрузчик на Unix предупреждает о файле, открытом группе или всем: тесты, которые
        // ждут пустых предупреждений, получают файл с правами владельца
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        p
    }

    fn load_with(
        file_body: Option<&str>,
        tag: &str,
        env: &[(&str, &str)],
        flags: Flags,
    ) -> Result<Loaded> {
        let mut flags = flags;
        if let Some(body) = file_body {
            flags.config = Some(temp_file(tag, body));
        }
        load(&flags, &env_of(env))
    }

    #[test]
    fn file_env_and_flags_override_in_order() {
        let flags = Flags {
            data_dir: Some(PathBuf::from("/flag/dir")),
            ..Flags::default()
        };
        let got = load_with(
            Some(r#"{"threads": 4, "max_grants": 5, "data_dir": "/file/dir"}"#),
            "order",
            &[
                ("CS2_SERVER_THREADS", "6"),
                ("CS2_SERVER_DATA_DIR", "/env/dir"),
            ],
            flags,
        )
        .unwrap();
        assert_eq!(got.config.threads, 6, "окружение сильнее файла");
        assert_eq!(got.config.max_grants, 5, "файл сильнее умолчания");
        assert_eq!(
            got.config.data_dir,
            PathBuf::from("/flag/dir"),
            "флаг сильнее окружения"
        );
        assert!(got.file.is_some());
    }

    #[test]
    fn unknown_file_key_is_an_error_without_values() {
        let e = load_with(Some(r#"{"thread": 4}"#), "unknown", &[], Flags::default())
            .err()
            .unwrap();
        assert!(e.contains("строка 1"), "{e}");
        let e = load_with(
            Some(r#"{"threads": "SENTINEL-VALUE-12345"}"#),
            "badtype",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(
            e.contains("строка 1") && !e.contains("SENTINEL-VALUE-12345"),
            "{e}"
        );
        let e = load_with(
            Some(r#"{"threads": SENTINEL-VALUE-12345}"#),
            "syntax",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(!e.contains("SENTINEL-VALUE-12345"), "{e}");
    }

    #[test]
    fn missing_config_file_is_an_error() {
        let flags = Flags {
            config: Some(PathBuf::from("/нет/такого/файла.json")),
            ..Flags::default()
        };
        assert!(load(&flags, &env_of(&[])).is_err());
    }

    #[test]
    fn bad_number_in_env_names_the_variable_only() {
        let e = load(&Flags::default(), &env_of(&[("CS2_SERVER_THREADS", "abc")]))
            .err()
            .unwrap();
        assert_eq!(e, "переменная CS2_SERVER_THREADS: нужно целое число");
    }

    #[test]
    fn bot_token_from_env_is_a_secret() {
        let got = load(
            &Flags::default(),
            &env_of(&[("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN)]),
        )
        .unwrap();
        let c = got.config;
        assert!(c.telegram_bot_token.is_some());
        assert!(!format!("{c:?}").contains(SENTINEL_TOKEN));
        assert!(
            !c.redactor()
                .redact(&format!("ключ {SENTINEL_TOKEN} конец"))
                .contains(SENTINEL_TOKEN)
        );
    }

    #[test]
    fn invalid_limits_are_refused() {
        let e = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_LEASE_TTL_SECS", "15")]),
        )
        .err()
        .unwrap();
        assert!(e.contains("lease_ttl_secs"), "{e}");
        let e = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_LONGPOLL_SECS", "45")]),
        )
        .err()
        .unwrap();
        assert!(e.contains("longpoll_secs"), "{e}");
        let e = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_BIND", "не адрес")]),
        )
        .err()
        .unwrap();
        assert!(e.contains("bind") && !e.contains("не адрес"), "{e}");
        let e = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_MAX_GRANTS", "0")]),
        )
        .err()
        .unwrap();
        assert!(e.contains("max_grants"), "{e}");
    }

    #[test]
    fn state_directory_is_the_default_data_dir() {
        let got = load(
            &Flags::default(),
            &env_of(&[("STATE_DIRECTORY", "/var/lib/cs2-server:/other")]),
        )
        .unwrap();
        assert_eq!(got.config.data_dir, PathBuf::from("/var/lib/cs2-server"));
        assert_eq!(
            load(&Flags::default(), &env_of(&[]))
                .unwrap()
                .config
                .data_dir,
            PathBuf::from("./data")
        );
    }

    #[test]
    fn non_loopback_bind_warns() {
        let got = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_BIND", "0.0.0.0:8080")]),
        )
        .unwrap();
        assert!(
            got.warnings
                .iter()
                .any(|w| w.contains("не loopback: HTTP без TLS")),
            "{:?}",
            got.warnings
        );
        let quiet = load(&Flags::default(), &env_of(&[])).unwrap();
        assert!(quiet.warnings.is_empty());
    }

    /// Абсолютный путь на любой платформе.
    fn abs_root() -> String {
        std::env::temp_dir()
            .join("bot-api-files")
            .to_string_lossy()
            .into_owned()
    }

    /// JSON настроек бота; пути экранируются самим serde_json.
    fn bot_json(extra: serde_json::Value) -> String {
        let mut v = serde_json::json!({
            "bot_api_url": "http://127.0.0.1:8081",
            "bot_api_files_root": abs_root(),
        });
        for (k, val) in extra.as_object().cloned().unwrap_or_default() {
            v[k] = val;
        }
        v.to_string()
    }

    #[test]
    fn bot_keys_load_from_file_and_env() {
        let body = bot_json(serde_json::json!({
            "bot_user_max_active": 3,
            "bot_send_as": "document",
            "bot_status_edit_secs": 20,
            "bot_transfer_timeout_mins": 30,
        }));
        let got = load_with(
            Some(&body),
            "bot-keys",
            &[("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN)],
            Flags::default(),
        )
        .unwrap();
        let c = &got.config;
        assert_eq!(c.bot_api_url.as_deref(), Some("http://127.0.0.1:8081"));
        assert_eq!(c.bot_user_max_active, 3);
        assert_eq!(c.bot_admin_max_active, 5, "умолчание D-03");
        assert_eq!(c.bot_queue_max_active, 10, "умолчание D-03");
        let s = c.bot_settings();
        assert_eq!(s.send_as, SendAs::Document);
        assert_eq!(s.user_max_active, 3);
        assert_eq!(s.admin_max_active, 5);
        assert_eq!(s.queue_max_active, 10);
        assert_eq!(s.status_edit_ms, 20_000);
        assert_eq!(s.files_root, PathBuf::from(abs_root()));
        assert_eq!(
            s.files_root_local,
            PathBuf::from(abs_root()),
            "по умолчанию вид с сервера совпадает с видом bot-api"
        );
        assert!(
            s.results_prefix
                .ends_with(PathBuf::from("blobs").join("results")),
            "{:?}",
            s.results_prefix
        );
        assert!(s.results_prefix.is_absolute() || s.results_prefix.has_root());
        assert_eq!(
            c.http_timeouts().transfer,
            std::time::Duration::from_secs(30 * 60)
        );

        // Окружение сильнее файла.
        let got = load_with(
            Some(&body),
            "bot-keys-env",
            &[
                ("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN),
                ("CS2_SERVER_BOT_USER_MAX_ACTIVE", "4"),
                ("CS2_SERVER_BOT_SEND_AS", "video"),
                ("CS2_SERVER_BOT_API_URL", "http://localhost:9000"),
            ],
            Flags::default(),
        )
        .unwrap();
        assert_eq!(got.config.bot_user_max_active, 4);
        assert_eq!(got.config.bot_send_as, SendAs::Video);
        assert_eq!(
            got.config.bot_api_url.as_deref(),
            Some("http://localhost:9000")
        );

        // Неизвестный ключ — ошибка с номером строки и без значения.
        let e = load_with(
            Some("{\n\"bot_foo\": \"SENTINEL-VALUE-12345\"\n}"),
            "bot-unknown",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(
            e.contains("строка") && !e.contains("SENTINEL-VALUE-12345"),
            "{e}"
        );
    }

    #[test]
    fn bot_url_must_be_local_http() {
        let token = [("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN)];
        for bad in [
            "https://api.telegram.org",
            "http://secret-host.example:80",
            "http://10.0.0.5:8081",
        ] {
            let body = bot_json(serde_json::json!({ "bot_api_url": bad }));
            let e = load_with(Some(&body), "bot-url", &token, Flags::default())
                .err()
                .unwrap_or_else(|| panic!("{bad} должен быть отвергнут"));
            assert!(
                e.contains("bot_api_url")
                    && e.contains(
                        "адрес telegram-bot-api должен быть локальным http://127.0.0.1:ПОРТ"
                    )
                    && !e.contains(bad),
                "{e}"
            );
        }
        let body = bot_json(serde_json::json!({}));
        let got = load_with(Some(&body), "bot-url-ok", &token, Flags::default()).unwrap();
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    #[test]
    fn bot_url_without_token_or_root_is_an_error() {
        let token = [("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN)];
        // Адрес без токена.
        let body = bot_json(serde_json::json!({}));
        let e = load_with(Some(&body), "bot-no-token", &[], Flags::default())
            .err()
            .unwrap();
        assert!(e.contains("токен"), "{e}");
        // Без корня файлов.
        let e = load_with(
            Some(r#"{"bot_api_url": "http://127.0.0.1:8081"}"#),
            "bot-no-root",
            &token,
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(e.contains("bot_api_files_root"), "{e}");
        // Относительный корень.
        let body = bot_json(serde_json::json!({ "bot_api_files_root": "relative/dir" }));
        let e = load_with(Some(&body), "bot-rel-root", &token, Flags::default())
            .err()
            .unwrap();
        assert!(e.contains("bot_api_files_root"), "{e}");
        // Прочие ключи называются по имени.
        for (key, value, name) in [
            ("bot_send_as", serde_json::json!("photo"), "bot_send_as"),
            (
                "bot_status_edit_secs",
                serde_json::json!(1),
                "bot_status_edit_secs",
            ),
            (
                "bot_transfer_timeout_mins",
                serde_json::json!(0),
                "bot_transfer_timeout_mins",
            ),
            (
                "bot_user_max_active",
                serde_json::json!(0),
                "bot_user_max_active",
            ),
            (
                "bot_admin_max_active",
                serde_json::json!(0),
                "bot_admin_max_active",
            ),
            (
                "bot_queue_max_active",
                serde_json::json!(0),
                "bot_queue_max_active",
            ),
        ] {
            let body = bot_json(serde_json::json!({ key: value }));
            let e = load_with(Some(&body), "bot-bad-key", &token, Flags::default())
                .err()
                .unwrap_or_else(|| panic!("{key} должен быть отвергнут"));
            assert!(e.contains(name), "{e}");
        }
    }

    #[test]
    fn retention_keys_have_defaults_and_are_checked() {
        let c = load(&Flags::default(), &env_of(&[])).unwrap().config;
        assert_eq!(
            (
                c.retention_demo_hours,
                c.retention_failed_demo_hours,
                c.retention_result_hours,
                c.retention_unsent_result_hours,
                c.retention_part_hours,
                c.retention_botapi_hours,
                c.clean_interval_secs,
            ),
            (24, 72, 24, 168, 6, 6, 600)
        );
        let r = c.retention();
        assert_eq!(r.demo_ms, 24 * 3_600_000);
        assert_eq!(r.unsent_result_ms, 168 * 3_600_000);
        assert_eq!(r, crate::clean::Retention::default());

        let e = load_with(
            Some(r#"{"retention_demo_hours": 0}"#),
            "ret-zero",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(
            e.contains("retention_demo_hours: нужно число от 1 до 8760"),
            "{e}"
        );
        let e = load_with(
            Some(r#"{"clean_interval_secs": 0}"#),
            "ret-interval",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(e.contains("clean_interval_secs"), "{e}");
        let e = load_with(
            Some(r#"{"retention_botapi_hours": 1, "bot_transfer_timeout_mins": 60}"#),
            "ret-botapi",
            &[],
            Flags::default(),
        )
        .err()
        .unwrap();
        assert!(
            e.contains(
                "retention_botapi_hours: нужно не меньше удвоенного bot_transfer_timeout_mins"
            ),
            "{e}"
        );

        // Окружение сильнее файла.
        let got = load_with(
            Some(r#"{"retention_demo_hours": 12}"#),
            "ret-env",
            &[
                ("CS2_SERVER_RETENTION_DEMO_HOURS", "48"),
                ("CS2_SERVER_CLEAN_INTERVAL_SECS", "30"),
            ],
            Flags::default(),
        )
        .unwrap();
        assert_eq!(got.config.retention_demo_hours, 48);
        assert_eq!(got.config.clean_interval_secs, 30);
        let e = load(
            &Flags::default(),
            &env_of(&[("CS2_SERVER_RETENTION_PART_HOURS", "abc")]),
        )
        .err()
        .unwrap();
        assert_eq!(
            e,
            "переменная CS2_SERVER_RETENTION_PART_HOURS: нужно целое число"
        );
    }

    #[test]
    fn token_without_bot_url_warns() {
        let got = load(
            &Flags::default(),
            &env_of(&[("TELEGRAM_BOT_TOKEN", SENTINEL_TOKEN)]),
        )
        .unwrap();
        assert!(
            got.warnings
                .iter()
                .any(|w| w.contains("бот не запущен: не задан bot_api_url")),
            "{:?}",
            got.warnings
        );
        assert!(
            got.warnings.iter().all(|w| !w.contains(SENTINEL_TOKEN)),
            "{:?}",
            got.warnings
        );
    }
}
