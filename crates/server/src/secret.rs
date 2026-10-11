//! Защита секретов (SEC-04). Все секреты процесса живут только в `Secret`;
//! любой текст, уходящий из процесса, проходит `Redactor`.
//!
//! `Redactor` маскирует по значению и по форме:
//! - точные значения известных секретов (токен бота Telegram из настроек) — `***`;
//! - токены раннера `cs2r_` + hex от 16 символов — `cs2r_***`, даже если сервер их уже не знает (отозванный или чужой);
//! - заголовок `Bearer {значение}` (регистр не важен, значение от 8 символов) — `Bearer ***`;
//! - токен бота Telegram по форме: от 5 цифр, `:`, от 30 символов `[A-Za-z0-9_-]` — `***` (в том числе внутри
//!   адреса Bot API `bot{цифры}:{секрет}`).
//!
//! `Redactor` стоит на всех выходах сервера: журнал (`Log` маскирует каждую строку и текст паники), база (`Core` пропускает
//! через `clean` каждый текст раннера и оператора до записи: сбои, причины возврата, имя и статус раннера, имя файла
//! демки), ответы API (только фиксированные русские тексты и очищенные данные, значения запроса в ответ не попадают)
//! и строки «ошибка:» командной строки.

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
        let out = mask_tokens(&out);
        let out = mask_bearer(&out);
        mask_bot_tokens(&out)
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

/// Самое короткое значение после `Bearer`, которое считается токеном.
const MIN_BEARER_LEN: usize = 8;
/// Токен бота Telegram: `{id}:{секрет}`, не меньше 5 цифр и не меньше 30 символов секрета.
const MIN_BOT_ID_DIGITS: usize = 5;
const MIN_BOT_SECRET_LEN: usize = 30;

/// `Bearer {значение}` без учёта регистра слова: значение от `MIN_BEARER_LEN` символов заменяется на `***`.
fn mask_bearer(s: &str) -> String {
    // Приведение только ASCII не меняет длину в байтах, поэтому смещения совпадают со строкой `s`.
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(rel) = lower[search..].find("bearer ") {
        let value_at = search + rel + "bearer ".len();
        let value = s[value_at..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect::<String>();
        if value.chars().count() >= MIN_BEARER_LEN {
            out.push_str(&s[copied..value_at]);
            out.push_str("***");
            copied = value_at + value.len();
            search = copied;
        } else {
            search = value_at;
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// Токен бота Telegram по форме; покрывает и `bot{цифры}:{секрет}` в адресах Bot API.
fn mask_bot_tokens(s: &str) -> String {
    let bytes = s.as_bytes();
    let secret_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut out = String::with_capacity(s.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(rel) = s[search..].find(':') {
        let colon = search + rel;
        let mut start = colon;
        while start > copied && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let mut end = colon + 1;
        while end < bytes.len() && secret_byte(bytes[end]) {
            end += 1;
        }
        if colon - start >= MIN_BOT_ID_DIGITS && end - (colon + 1) >= MIN_BOT_SECRET_LEN {
            out.push_str(&s[copied..start]);
            out.push_str("***");
            copied = end;
            search = end;
        } else {
            search = colon + 1;
        }
    }
    out.push_str(&s[copied..]);
    out
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

    const BOT: &str = "123456789:AAEsentinelSENTINELsentinelSENTINEL01";

    #[test]
    fn bearer_and_telegram_bot_tokens_are_masked_by_shape() {
        let r = Redactor::default();
        assert_eq!(
            r.redact(&format!("токен бота {BOT} конец")),
            "токен бота *** конец"
        );
        assert_eq!(
            r.redact(&format!("https://api.telegram.org/bot{BOT}/sendMessage")),
            "https://api.telegram.org/bot***/sendMessage"
        );
        assert_eq!(
            r.redact("Authorization: Bearer abcdefgh123"),
            "Authorization: Bearer ***"
        );
        assert_eq!(r.redact("bearer abcdefgh123\nдальше"), "bearer ***\nдальше");
        assert_eq!(r.redact("BEARER Zz9-_.abcdefgh x"), "BEARER *** x");
        // Токен раннера внутри Bearer маскируется целиком.
        let token = format!("cs2r_{}", "cd34".repeat(16));
        assert_eq!(r.redact(&format!("Bearer {token}")), "Bearer ***");
        // Граница по длине: 5 цифр и 30 символов — ещё токен, 4 цифры или 29 символов — уже нет.
        let edge = format!("{}:{}", "1".repeat(5), "a".repeat(30));
        assert_eq!(r.redact(&edge), "***");
        let short_id = format!("{}:{}", "1".repeat(4), "a".repeat(30));
        assert_eq!(r.redact(&short_id), short_id);
        let short_secret = format!("{}:{}", "1".repeat(5), "a".repeat(29));
        assert_eq!(r.redact(&short_secret), short_secret);
    }

    #[test]
    fn ordinary_text_times_and_hashes_survive_redaction() {
        let r = Redactor::default();
        let sha = "ab".repeat(32);
        for text in [
            "12:00:01",
            "2026-10-11T15:04:05Z INFO обычная строка",
            "время 09:30, порт 127.0.0.1:8080, ключ: значение",
            "Bearer short",
            "носитель bearer без значения",
            &sha,
            &format!("sha256 {sha}"),
            "задача #7: failed (HLAE_ERROR), раннер main",
        ] {
            assert_eq!(r.redact(text), text, "{text}");
        }
    }

    #[test]
    fn redaction_is_idempotent() {
        let secret = Secret::new("SECRET-VALUE-123".to_owned());
        let r = Redactor::new(&[&secret]);
        let token = format!("cs2r_{}", "ab12".repeat(16));
        let text = format!("x SECRET-VALUE-123 {token} Bearer {token} bot{BOT}/y {BOT}");
        let once = r.redact(&text);
        assert!(
            !once.contains("SECRET-VALUE")
                && !once.contains(&token)
                && !once.contains("AAEsentinel")
        );
        assert_eq!(r.redact(&once), once);
        assert_eq!(r.clean(&once, 10_000), once);
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
