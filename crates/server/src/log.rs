//! Журнал сервера: строка уходит в приёмник (`LogSink`) только после `Redactor` (SEC-04).
//!
//! Приёмник — stderr в боевой работе и память в тестах. Заголовки запросов в журнал не пишутся вообще,
//! а всё остальное (путь, тексты ошибок, паники) проходит через маскировщик секретов.

use crate::secret::Redactor;
use std::sync::{Arc, Mutex};

/// Куда уходят готовые строки журнала.
pub trait LogSink: Send + Sync {
    fn line(&self, s: &str);
}

/// Боевой приёмник: стандартный поток ошибок (его собирает systemd/journald).
pub struct StderrSink;

impl LogSink for StderrSink {
    fn line(&self, s: &str) {
        eprintln!("{s}");
    }
}

/// Приёмник в памяти — для тестов.
#[derive(Default)]
pub struct MemorySink {
    lines: Mutex<Vec<String>>,
}

impl MemorySink {
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl LogSink for MemorySink {
    fn line(&self, s: &str) {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(s.to_owned());
    }
}

/// Самая длинная строка журнала (в символах); остальное обрезается.
const MAX_LINE_CHARS: usize = 1000;

#[derive(Clone)]
pub struct Log {
    sink: Arc<dyn LogSink>,
    redactor: Redactor,
}

impl Log {
    pub fn new(sink: Arc<dyn LogSink>, redactor: Redactor) -> Self {
        Self { sink, redactor }
    }

    pub fn info(&self, msg: &str) {
        self.write("INFO", msg);
    }

    pub fn warn(&self, msg: &str) {
        self.write("WARN", msg);
    }

    fn write(&self, level: &str, msg: &str) {
        // Управляющие символы (в том числе переводы строк) заменяются пробелом: запрос не должен
        // подделывать соседние строки журнала.
        let safe: String = self
            .redactor
            .redact(msg)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_LINE_CHARS)
            .collect();
        let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        self.sink.line(&format!("{ts} {level} {safe}"));
    }
}

/// Паника любого потока попадает в журнал строкой «паника: …» вместо прямой печати в stderr.
pub fn install_panic_hook(log: Log) {
    std::panic::set_hook(Box::new(move |info| {
        let text = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "без текста".to_owned()
        };
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "?".to_owned());
        log.warn(&format!("паника: {text} ({place})"));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::Secret;

    #[test]
    fn log_line_is_redacted_before_it_reaches_the_sink() {
        let sink = Arc::new(MemorySink::default());
        let bot = Secret::new("123456789:AAEsentinelSENTINEL".to_owned());
        let log = Log::new(sink.clone(), Redactor::new(&[&bot]));
        let token = format!("cs2r_{}", "ab".repeat(32));
        log.info(&format!("бот {} токен {token}\nподделка", bot.expose()));
        log.warn("обычная строка");
        let lines = sink.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(" INFO "), "{}", lines[0]);
        assert!(lines[1].contains(" WARN "), "{}", lines[1]);
        assert!(!lines[0].contains("AAEsentinel"), "{}", lines[0]);
        assert!(!lines[0].contains(&token), "{}", lines[0]);
        assert!(lines[0].contains("cs2r_***"), "{}", lines[0]);
        assert!(
            !lines[0].contains('\n'),
            "перевод строки не должен попасть в журнал"
        );
    }
}
