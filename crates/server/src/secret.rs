//! Защита секретов (SEC-04). Все секреты процесса живут только в `Secret`;
//! любой текст, уходящий из процесса — журнал, колонки базы, ответы API, ошибки CLI, — проходит `Redactor`.

use std::fmt;
use std::sync::Arc;

/// Секретное значение. `Debug` и `Display` печатают `***`; в JSON не сериализуется (нет derive).
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Само значение: вызывать только там, где оно действительно нужно (например, при обращении к Telegram).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Короткие значения не маскируем: они совпадали бы с обычным текстом.
const MIN_SECRET_LEN: usize = 8;
const TOKEN_PREFIX: &str = "cs2r_";
const MIN_TOKEN_HEX: usize = 16;

/// Маскирует известные секреты и всё, что похоже на токен раннера.
#[derive(Clone, Default)]
pub struct Redactor {
    exact: Arc<Vec<String>>,
}

impl Redactor {
    pub fn new(secrets: &[&Secret]) -> Self {
        let exact = secrets
            .iter()
            .map(|s| s.expose())
            .filter(|v| v.chars().count() >= MIN_SECRET_LEN)
            .map(str::to_owned)
            .collect();
        Self {
            exact: Arc::new(exact),
        }
    }

    /// Точные значения секретов заменяются на `***`, токены вида `cs2r_` + hex — на `cs2r_***`.
    pub fn redact(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for secret in self.exact.iter() {
            out = out.replace(secret.as_str(), "***");
        }
        mask_tokens(&out)
    }

    /// `redact`, затем удаление управляющих символов (кроме `\n` и `\t`) и обрезка до `max_chars`.
    pub fn clean(&self, s: &str, max_chars: usize) -> String {
        let redacted = self.redact(s);
        let cleaned: String = redacted
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        if cleaned.chars().count() <= max_chars {
            return cleaned;
        }
        let mut out: String = cleaned.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn mask_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(TOKEN_PREFIX) {
        let after = &rest[at + TOKEN_PREFIX.len()..];
        let hex_len = after
            .bytes()
            .take_while(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            .count();
        if hex_len >= MIN_TOKEN_HEX {
            out.push_str(&rest[..at]);
            out.push_str("cs2r_***");
            rest = &after[hex_len..];
        } else {
            out.push_str(&rest[..at + TOKEN_PREFIX.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_secret_and_runner_token_shape_are_masked() {
        let secret = Secret::new("SECRET-VALUE-123".to_owned());
        let r = Redactor::new(&[&secret]);
        let token = format!("cs2r_{}", "ab12".repeat(16));
        assert_eq!(
            r.redact(&format!("x SECRET-VALUE-123 y {token}")),
            "x *** y cs2r_***"
        );
        assert_eq!(r.redact("cs2r_abc stays"), "cs2r_abc stays");
        // Короткие секреты не маскируются.
        let short = Secret::new("abc".to_owned());
        assert_eq!(Redactor::new(&[&short]).redact("abc"), "abc");
    }

    #[test]
    fn clean_strips_control_chars_and_caps_length() {
        let r = Redactor::default();
        assert_eq!(r.clean("a\u{7}b\nc\td\u{1b}", 100), "ab\nc\td");
        assert_eq!(r.clean("ёёёёёёёёёё", 4), "ёёё…");
        assert_eq!(r.clean("short", 10), "short");
    }

    #[test]
    fn debug_and_display_of_a_secret_hide_the_value() {
        let s = Secret::new("hunter2hunter2".to_owned());
        assert_eq!(format!("{s:?} {s}"), "*** ***");
        assert_eq!(s.expose(), "hunter2hunter2");
    }
}
