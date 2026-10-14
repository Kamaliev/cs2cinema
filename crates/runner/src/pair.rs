//! Подключение раннера через Telegram без консоли сервера: токен придумывает сам раннер и хранит у себя,
//! серверу уходит только его `sha256` (`POST /v1/runner/pair`), а админ называет боту код заявки
//! (`/runner код`). Токен по сети не передаётся ни в заявке, ни в чате; после одобрения обычный `hello`
//! с этим токеном проходит.

use std::time::Duration;

use sha2::{Digest, Sha256};
use wire::{PairRequest, TOKEN_PREFIX};

use crate::link::{Link, Token};

/// Новый случайный токен по образцу сервера: `cs2r_` и 64 строчных hex. Значение отдаётся строкой, потому что
/// вызывающий должен положить его в файл токена; дальше оно живёт только там.
pub fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("нет случайных байтов от системы: {e}"))?;
    Ok(format!("{TOKEN_PREFIX}{}", hex(&bytes)))
}

/// Хеш токена, который уходит в заявку и по которому сервер узнаёт раннера.
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Принятая заявка: код для админа и срок его действия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub expires_in: Duration,
}

/// Отправляет заявку с хешем `token` под именем `name`. Ошибка — одной строкой по-русски, без токена.
pub fn request(server: &str, name: &str, token: &str) -> Result<Pairing, String> {
    let link = Link::new(server, Token::new(token))?;
    let req = PairRequest {
        name: name.to_owned(),
        token_hash: token_hash(token),
    };
    let reply = link.pair(&req).map_err(|e| e.to_string())?;
    Ok(Pairing {
        code: reply.code,
        expires_in: Duration::from_secs(reply.expires_in_secs),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_made_tokens_follow_the_server_shape_and_hash_with_sha256() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("cs2r_") && a.len() == 5 + 64);
        assert!(a[5..].bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "sha256 в строчном hex, как считает сервер"
        );
    }
}
