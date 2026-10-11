//! Настройки сервера: умолчания, переменные окружения и флаги.
//!
//! Окружение передаётся замыканием, чтобы тесты не трогали переменные процесса.

use crate::Result;
use crate::core::Limits;
use std::path::PathBuf;

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
        }
    }
}

impl Config {
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

pub fn load(flags: &Flags, env: &dyn Fn(&str) -> Option<String>) -> Result<Loaded> {
    let mut config = Config::default();
    if let Some(dir) = env("STATE_DIRECTORY")
        .and_then(|v| v.split(':').next().map(str::to_owned))
        .filter(|v| !v.is_empty())
    {
        config.data_dir = PathBuf::from(dir);
    }
    if let Some(dir) = env("CS2_SERVER_DATA_DIR").filter(|v| !v.is_empty()) {
        config.data_dir = PathBuf::from(dir);
    }
    if let Some(dir) = &flags.data_dir {
        config.data_dir = dir.clone();
    }
    Ok(Loaded {
        config,
        warnings: Vec::new(),
        file: None,
    })
}
