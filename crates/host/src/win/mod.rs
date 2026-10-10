//! Windows-часть хоста: Job Object, процессы, сессия игры (WinHost — 01-06).
//!
//! Весь `unsafe` фазы 1 живёт только здесь, у каждого блока есть `// SAFETY:`.

pub mod job;
pub mod process;

/// Последняя ошибка Win32: код и текст системы.
pub(crate) fn last_error() -> String {
    let e = std::io::Error::last_os_error();
    format!("код {}: {e}", e.raw_os_error().unwrap_or(0))
}
