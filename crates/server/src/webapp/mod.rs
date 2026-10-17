//! Telegram Mini App: экран настроек монтажа.
//!
//! Страница (`settings.html`) открывается из чата кнопкой `/settings` и говорит с сервером тремя адресами:
//! `GET /app/settings` (сама страница, без секретов), `GET`/`PUT /app/api/settings` (значения). Вход — строка
//! `initData`, которую Telegram подписывает токеном бота; проверка по правилам Telegram
//! (<https://core.telegram.org/bots/webapps#validating-data-received-via-the-mini-app>) живёт здесь, а разбор
//! и проверка самих настроек — в `wire::settings`, чтобы экран, сервер и раннер не расходились.

use crate::secret::Secret;
use sha2::{Digest, Sha256};

/// Страница настроек: один файл без сборки.
pub const SETTINGS_PAGE: &str = include_str!("settings.html");

/// Сколько живёт подпись `initData`: старее — просим открыть экран заново.
const MAX_AGE_SECS: i64 = 24 * 3600;

/// Что нужно для проверки входа: токен бота.
#[derive(Clone)]
pub struct Auth {
    token: Secret,
}

impl Auth {
    pub fn new(token: Secret) -> Self {
        Self { token }
    }

    /// Проверяет `initData` и возвращает Telegram-ID пользователя.
    pub fn verify(&self, init_data: &str, now_secs: i64) -> Result<i64, &'static str> {
        verify_init_data(init_data, self.token.expose(), now_secs)
    }
}

/// HMAC-SHA256 (RFC 2104) поверх `sha2`: отдельная зависимость ради одной функции не нужна.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Сравнение без раннего выхода по первому отличию.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `%XX` и `+` из строки запроса; неверная последовательность — `None`.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Проверка подписи `initData` по правилам Telegram: из всех полей, кроме `hash`, собирается строка
/// `ключ=значение`, отсортированная по ключу и склеенная переводами строк; подпись — HMAC-SHA256 этой строки
/// ключом `HMAC_SHA256("WebAppData", токен)`. Дополнительно требуется свежая `auth_date`.
pub fn verify_init_data(init_data: &str, token: &str, now_secs: i64) -> Result<i64, &'static str> {
    const BAD: &str = "подпись Telegram не подошла: откройте экран из чата заново";
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in init_data.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').ok_or(BAD)?;
        pairs.push((percent_decode(k).ok_or(BAD)?, percent_decode(v).ok_or(BAD)?));
    }
    let hash = pairs.iter().find(|(k, _)| k == "hash").map(|(_, v)| v.clone()).ok_or(BAD)?;
    pairs.retain(|(k, _)| k != "hash");
    pairs.sort();
    let check = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
    let secret = hmac_sha256(b"WebAppData", token.as_bytes());
    let expected = crate::hex(&hmac_sha256(&secret, check.as_bytes()));
    if !same(expected.as_bytes(), hash.to_ascii_lowercase().as_bytes()) {
        return Err(BAD);
    }
    let get = |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let auth_date: i64 = get("auth_date").and_then(|v| v.parse().ok()).ok_or(BAD)?;
    if now_secs - auth_date > MAX_AGE_SECS || auth_date - now_secs > 300 {
        return Err("экран открыт слишком давно: откройте его из чата заново");
    }
    let user: serde_json::Value = serde_json::from_str(get("user").ok_or(BAD)?).map_err(|_| BAD)?;
    user.get("id").and_then(serde_json::Value::as_i64).ok_or(BAD)
}

/// Собирает `initData`, как это делает Telegram (для тестов).
#[doc(hidden)]
pub fn sign_init_data(token: &str, user_id: i64, auth_date: i64) -> String {
    let user = format!(r#"{{"id":{user_id},"first_name":"Тест"}}"#);
    let mut pairs = vec![("auth_date".to_owned(), auth_date.to_string()), ("query_id".to_owned(), "AAH".to_owned()), ("user".to_owned(), user)];
    pairs.sort();
    let check = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n");
    let secret = hmac_sha256(b"WebAppData", token.as_bytes());
    let hash = crate::hex(&hmac_sha256(&secret, check.as_bytes()));
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    let mut out: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect();
    out.push(format!("hash={hash}"));
    out.join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456:TEST-token";

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(crate::hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn signed_init_data_gives_the_telegram_id() {
        let data = sign_init_data(TOKEN, 777, 1_000_000);
        assert_eq!(verify_init_data(&data, TOKEN, 1_000_100), Ok(777));
    }

    #[test]
    fn forged_old_or_foreign_init_data_is_refused() {
        let data = sign_init_data(TOKEN, 777, 1_000_000);
        assert!(verify_init_data(&data, "999:other-token", 1_000_100).is_err(), "чужой токен");
        let tampered = data.replace("777", "778");
        assert!(verify_init_data(&tampered, TOKEN, 1_000_100).is_err(), "подмена id");
        assert!(verify_init_data(&data, TOKEN, 1_000_000 + MAX_AGE_SECS + 1).is_err(), "подпись устарела");
        assert!(verify_init_data(&data, TOKEN, 1_000_000 - 3600).is_err(), "дата из будущего");
        for bad in ["", "hash=00", "мусор", "a=b&hash=zz"] {
            assert!(verify_init_data(bad, TOKEN, 1_000_000).is_err(), "{bad:?}");
        }
    }
}
