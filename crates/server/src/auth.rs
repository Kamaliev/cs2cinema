//! Токены раннеров (SRV-05). Токен — 32 байта из `getrandom`, показывается один раз;
//! в базе только `sha256` токена. Поиск идёт по индексированному хешу и не нуждается в сравнении
//! за постоянное время: сравнивается хеш, прообраз которого знает только владелец токена.

use crate::db::{Db, db_err};
use crate::{Result, hex};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

pub const TOKEN_PREFIX: &str = "cs2r_";

/// Новый случайный токен: `cs2r_` и 64 строчных hex.
pub fn issue_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("нет случайных байтов от системы: {e}"))?;
    Ok(format!("{TOKEN_PREFIX}{}", hex(&bytes)))
}

/// Хеш токена, который хранится в базе.
pub fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Короткий отпечаток для списков: первые 8 hex хеша.
pub fn fingerprint(hash: &str) -> String {
    hash.chars().take(8).collect()
}

pub fn looks_like_token(s: &str) -> bool {
    match s.strip_prefix(TOKEN_PREFIX) {
        Some(h) => h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        None => false,
    }
}

#[derive(Debug, Clone)]
pub struct Runner {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub fingerprint: String,
}

#[derive(Debug)]
pub enum AuthError {
    /// Строка не похожа на токен; база не трогалась.
    Malformed,
    Unknown {
        fingerprint: String,
    },
    Revoked {
        fingerprint: String,
    },
    Db(String),
}

impl From<String> for AuthError {
    fn from(s: String) -> Self {
        AuthError::Db(s)
    }
}

fn valid_runner_name(name: &str) -> bool {
    (1..=32).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn runner_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Runner> {
    let hash: String = r.get(5)?;
    Ok(Runner {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        revoked_at: r.get(3)?,
        last_seen_at: r.get(4)?,
        fingerprint: fingerprint(&hash),
    })
}

const RUNNER_COLUMNS: &str = "id, name, created_at, revoked_at, last_seen_at, token_hash";

/// Добавляет раннера и возвращает его вместе с токеном: токен больше нигде не сохраняется.
pub fn add_runner(db: &Db, name: &str, now_ms: i64) -> Result<(Runner, String)> {
    if !valid_runner_name(name) {
        return Err(
            "имя раннера: от 1 до 32 символов, только латинские буквы, цифры, «_» и «-»".to_owned(),
        );
    }
    let token = issue_token()?;
    let hash = hash_token(&token);
    let runner = db.write(|tx| {
        let taken: Option<i64> = tx
            .query_row("SELECT id FROM runners WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_err)?;
        if taken.is_some() {
            return Err(format!("раннер «{name}» уже есть"));
        }
        tx.execute(
            "INSERT INTO runners(name, token_hash, created_at) VALUES (?1, ?2, ?3)",
            params![name, hash, now_ms],
        )
        .map_err(db_err)?;
        Ok(Runner {
            id: tx.last_insert_rowid(),
            name: name.to_owned(),
            created_at: now_ms,
            revoked_at: None,
            last_seen_at: None,
            fingerprint: fingerprint(&hash),
        })
    })?;
    Ok((runner, token))
}

pub fn list_runners(db: &Db) -> Result<Vec<Runner>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!("SELECT {RUNNER_COLUMNS} FROM runners ORDER BY id"))
            .map_err(db_err)?;
        let rows = st.query_map([], runner_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

/// Отзывает токен раннера: дальше `authenticate` отвечает `Revoked`.
pub fn revoke_runner(db: &Db, name: &str, now_ms: i64) -> Result<()> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE runners SET revoked_at = COALESCE(revoked_at, ?2) WHERE name = ?1",
                params![name, now_ms],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err(format!("раннера «{name}» нет"));
        }
        // Аренды раннера заканчиваются «сейчас»: следующий `sweep` вернёт задачи в очередь. Вернуть их себе
        // под отозванным токеном нельзя — вход закрыт до ядра.
        tx.execute(
            "UPDATE jobs SET lease_until = ?2, updated_at = ?2 WHERE state IN ('assigned','running','uploading') \
             AND runner_id = (SELECT id FROM runners WHERE name = ?1)",
            params![name, now_ms],
        )
        .map_err(db_err)?;
        Ok(())
    })
}

/// Узнаёт раннера по токену и отмечает время последнего входа.
pub fn authenticate(db: &Db, token: &str, now_ms: i64) -> std::result::Result<Runner, AuthError> {
    if !looks_like_token(token) {
        return Err(AuthError::Malformed);
    }
    let hash = hash_token(token);
    db.write(|tx| {
        let found = tx
            .query_row(
                &format!("SELECT {RUNNER_COLUMNS} FROM runners WHERE token_hash = ?1"),
                [&hash],
                runner_from_row,
            )
            .optional()
            .map_err(db_err)?;
        let Some(mut runner) = found else {
            return Err(AuthError::Unknown {
                fingerprint: fingerprint(&hash),
            });
        };
        if runner.revoked_at.is_some() {
            return Err(AuthError::Revoked {
                fingerprint: runner.fingerprint,
            });
        }
        tx.execute(
            "UPDATE runners SET last_seen_at = ?2 WHERE id = ?1",
            params![runner.id, now_ms],
        )
        .map_err(db_err)?;
        runner.last_seen_at = Some(now_ms);
        Ok(runner)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_tokens_have_the_expected_shape_and_differ() {
        let a = issue_token().unwrap();
        let b = issue_token().unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("cs2r_") && a.len() == 5 + 64);
        assert!(looks_like_token(&a));
        for bad in ["cs2r_abc", "Bearer x", ""] {
            assert!(!looks_like_token(bad), "{bad}");
        }
        assert!(!looks_like_token(&a.to_uppercase()));
    }

    #[test]
    fn add_list_authenticate_and_revoke() {
        let db = Db::in_memory().unwrap();
        let (runner, token) = add_runner(&db, "main", 100).unwrap();
        assert_eq!(runner.fingerprint, fingerprint(&hash_token(&token)));
        let list = list_runners(&db).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "main");
        assert!(
            add_runner(&db, "main", 100)
                .unwrap_err()
                .contains("уже есть")
        );
        assert!(add_runner(&db, "bad name!", 100).is_err());

        let ok = authenticate(&db, &token, 500).unwrap();
        assert_eq!(ok.last_seen_at, Some(500));
        assert_eq!(list_runners(&db).unwrap()[0].last_seen_at, Some(500));

        let stranger = issue_token().unwrap();
        assert!(matches!(
            authenticate(&db, &stranger, 600),
            Err(AuthError::Unknown { .. })
        ));
        assert!(matches!(
            authenticate(&db, "cs2r_abc", 600),
            Err(AuthError::Malformed)
        ));

        revoke_runner(&db, "main", 700).unwrap();
        assert!(matches!(
            authenticate(&db, &token, 800),
            Err(AuthError::Revoked { .. })
        ));
        assert!(revoke_runner(&db, "nobody", 900).is_err());
    }
}
