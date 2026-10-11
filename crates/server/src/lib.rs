//! Сервер очереди монтажа: своя SQLite, автомат задач с арендой и эпохой, файлы кусками с SHA-256.
//!
//! Крейт не зависит от Windows-крейтов воркспейса (только от `wire`) и собирается на Linux.
//! Правило: ядро (`core`) работает без HTTP и с внедрёнными часами (`clock`), HTTP — тонкая обёртка над ним.
//! Секреты хранятся только в `secret::Secret`, а любой текст, уходящий из процесса, проходит `secret::Redactor`.

pub mod auth;
pub mod blobs;
pub mod cli;
pub mod clock;
pub mod config;
pub mod core;
pub mod db;
pub mod secret;
pub mod users;

pub type Result<T> = std::result::Result<T, String>;

/// Строчное шестнадцатеричное представление байтов.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
