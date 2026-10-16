//! Токены раннеров (SRV-05). Токен — 32 байта из `getrandom`, показывается один раз;
//! в базе только `sha256` токена. Поиск идёт по индексированному хешу и не нуждается в сравнении
//! за постоянное время: сравнивается хеш, прообраз которого знает только владелец токена.

use crate::db::{Db, db_err};
use crate::{Result, hex};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

pub const TOKEN_PREFIX: &str = wire::TOKEN_PREFIX;

/// Сколько живёт код заявки на подключение через Telegram.
pub const PAIR_TTL_MS: i64 = 10 * 60 * 1000;
/// Сколько заявок ждут одобрения одновременно: адрес заявки открыт без токена, и потолок не даёт забить таблицу.
const PAIR_MAX: usize = 20;
/// Длина кода заявки в цифрах.
const PAIR_CODE_LEN: usize = 6;

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

const NAME_RULE: &str =
    "имя раннера: от 1 до 32 символов, только латинские буквы, цифры, «_» и «-»";

/// Вставляет раннера с готовым хешем токена; занятое имя — ошибка.
fn insert_runner(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    hash: &str,
    now_ms: i64,
) -> Result<Runner> {
    let taken: Option<(i64, Option<i64>)> = tx
        .query_row(
            "SELECT id, revoked_at FROM runners WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(db_err)?;
    match taken {
        None => {}
        // Имя отозванного раннера свободно: старая запись остаётся в журнале под служебным именем.
        Some((id, Some(_))) => {
            tx.execute(
                "UPDATE runners SET name = ?2 WHERE id = ?1",
                params![id, format!("{name}#отозван-{id}")],
            )
            .map_err(db_err)?;
        }
        Some((_, None)) => return Err(format!("раннер «{name}» уже есть")),
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
        fingerprint: fingerprint(hash),
    })
}

/// Добавляет раннера и возвращает его вместе с токеном: токен больше нигде не сохраняется.
pub fn add_runner(db: &Db, name: &str, now_ms: i64) -> Result<(Runner, String)> {
    if !valid_runner_name(name) {
        return Err(NAME_RULE.to_owned());
    }
    let token = issue_token()?;
    let hash = hash_token(&token);
    let runner = db.write(|tx| insert_runner(tx, name, &hash, now_ms))?;
    Ok((runner, token))
}

/// Заявка раннера на подключение через Telegram: код ждёт `/runner код` от админа.
#[derive(Debug, Clone, PartialEq)]
pub struct Pairing {
    pub code: String,
    pub name: String,
    pub expires_at: i64,
}

/// Почему заявка не принята.
#[derive(Debug, Clone, PartialEq)]
pub enum PairError {
    /// Имя или хеш не по правилам, либо токен уже зарегистрирован.
    Invalid(String),
    /// Заявок ждёт одобрения больше `PAIR_MAX`.
    TooMany,
    Db(String),
}

impl From<String> for PairError {
    fn from(s: String) -> Self {
        PairError::Db(s)
    }
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairError::Invalid(why) | PairError::Db(why) => f.write_str(why),
            PairError::TooMany => f.write_str(
                "слишком много заявок на подключение ждут одобрения — попробуйте через несколько минут",
            ),
        }
    }
}

fn valid_token_hash(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Случайный код заявки: `PAIR_CODE_LEN` десятичных цифр с ведущими нулями.
fn issue_code() -> Result<String> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|e| format!("нет случайных байтов от системы: {e}"))?;
    let n = u32::from_le_bytes(bytes) % 10u32.pow(PAIR_CODE_LEN as u32);
    Ok(format!("{n:0width$}", width = PAIR_CODE_LEN))
}

/// Принимает заявку: просроченные заявки стираются, повторная заявка с тем же хешем заменяет прежнюю,
/// хеш уже зарегистрированного раннера отклоняется. Код уникален среди ждущих заявок.
pub fn request_pairing(
    db: &Db,
    name: &str,
    token_hash: &str,
    now_ms: i64,
) -> std::result::Result<Pairing, PairError> {
    if !valid_runner_name(name) {
        return Err(PairError::Invalid(NAME_RULE.to_owned()));
    }
    if !valid_token_hash(token_hash) {
        return Err(PairError::Invalid(
            "хеш токена: 64 строчных шестнадцатеричных знака".to_owned(),
        ));
    }
    let expires_at = now_ms + PAIR_TTL_MS;
    db.write(|tx| {
        tx.execute(
            "DELETE FROM runner_pairings WHERE expires_at <= ?1 OR token_hash = ?2",
            params![now_ms, token_hash],
        )
        .map_err(db_err)?;
        let known: Option<i64> = tx
            .query_row(
                "SELECT id FROM runners WHERE token_hash = ?1",
                [token_hash],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if known.is_some() {
            return Err(PairError::Invalid(
                "этот токен уже зарегистрирован — заявка не нужна".to_owned(),
            ));
        }
        let waiting: i64 = tx
            .query_row("SELECT COUNT(*) FROM runner_pairings", [], |r| r.get(0))
            .map_err(db_err)?;
        if waiting as usize >= PAIR_MAX {
            return Err(PairError::TooMany);
        }
        // код не должен совпасть с кодом другой ждущей заявки; при 20 заявках и миллионе кодов это почти невозможно
        let mut code = issue_code()?;
        for _ in 0..16 {
            let taken: Option<String> = tx
                .query_row(
                    "SELECT code FROM runner_pairings WHERE code = ?1",
                    [&code],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            if taken.is_none() {
                break;
            }
            code = issue_code()?;
        }
        tx.execute(
            "INSERT INTO runner_pairings(code, name, token_hash, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![code, name, token_hash, now_ms, expires_at],
        )
        .map_err(db_err)?;
        Ok(Pairing {
            code,
            name: name.to_owned(),
            expires_at,
        })
    })
}

/// Одобряет заявку по коду: раннер создаётся с хешем из заявки под её именем или именем `name`.
/// Занятое имя оставляет заявку ждать — админ повторит команду с другим именем.
pub fn approve_pairing(db: &Db, code: &str, name: Option<&str>, now_ms: i64) -> Result<Runner> {
    const NO_SUCH: &str = "заявки с таким кодом нет или код устарел — нажмите в приложении «Подключить через Telegram» ещё раз";
    if code.len() != PAIR_CODE_LEN || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("код заявки — {PAIR_CODE_LEN} цифр"));
    }
    if let Some(n) = name {
        if !valid_runner_name(n) {
            return Err(NAME_RULE.to_owned());
        }
    }
    db.write(|tx| {
        let found: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT name, token_hash, expires_at FROM runner_pairings WHERE code = ?1",
                [code],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((proposed, hash, expires_at)) = found else {
            return Err(NO_SUCH.to_owned());
        };
        if expires_at <= now_ms {
            tx.execute("DELETE FROM runner_pairings WHERE code = ?1", [code])
                .map_err(db_err)?;
            return Err(NO_SUCH.to_owned());
        }
        let runner = insert_runner(tx, name.unwrap_or(&proposed), &hash, now_ms)?;
        tx.execute("DELETE FROM runner_pairings WHERE code = ?1", [code])
            .map_err(db_err)?;
        Ok(runner)
    })
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
    fn revoked_runner_name_can_be_registered_again() {
        let db = Db::in_memory().unwrap();
        let (_, old) = add_runner(&db, "PC", 100).unwrap();
        assert!(add_runner(&db, "PC", 101).unwrap_err().contains("уже есть"));
        revoke_runner(&db, "PC", 102).unwrap();
        let (_, new) = add_runner(&db, "PC", 103).unwrap();
        assert!(authenticate(&db, &old, 104).is_err());
        assert_eq!(authenticate(&db, &new, 104).unwrap().name, "PC");
        assert_eq!(list_runners(&db).unwrap().len(), 2);
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

    #[test]
    fn pairing_code_turns_a_self_made_token_into_a_runner() {
        let db = Db::in_memory().unwrap();
        let token = issue_token().unwrap();
        let hash = hash_token(&token);
        let p = request_pairing(&db, "my-pc", &hash, 1_000).unwrap();
        assert_eq!(p.code.len(), 6);
        assert!(p.code.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(
            (p.name.as_str(), p.expires_at),
            ("my-pc", 1_000 + PAIR_TTL_MS)
        );
        assert!(
            matches!(
                authenticate(&db, &token, 1_500),
                Err(AuthError::Unknown { .. })
            ),
            "до одобрения токен не принимается"
        );

        // повторная заявка с тем же хешем заменяет прежнюю: старый код больше не действует
        let again = request_pairing(&db, "my-pc", &hash, 2_000).unwrap();
        assert!(approve_pairing(&db, &p.code, None, 2_100).is_err() || again.code == p.code);

        let bad_code = approve_pairing(&db, "12", None, 2_100).unwrap_err();
        assert!(bad_code.contains("6 цифр"), "{bad_code}");
        let runner = approve_pairing(&db, &again.code, None, 2_200).unwrap();
        assert_eq!(runner.name, "my-pc");
        assert_eq!(runner.fingerprint, fingerprint(&hash));
        assert_eq!(authenticate(&db, &token, 2_300).unwrap().name, "my-pc");
        assert!(
            approve_pairing(&db, &again.code, None, 2_400)
                .unwrap_err()
                .contains("нет или код устарел"),
            "одобренная заявка стёрта"
        );
        assert!(
            matches!(request_pairing(&db, "my-pc", &hash, 2_500), Err(PairError::Invalid(why)) if why.contains("уже зарегистрирован")),
            "хеш зарегистрированного раннера в заявку не принимается"
        );
    }

    #[test]
    fn pairing_respects_names_expiry_and_the_cap() {
        let db = Db::in_memory().unwrap();
        let hash_of = |i: u32| hash_token(&format!("t{i}"));
        assert!(matches!(
            request_pairing(&db, "плохое имя", &hash_of(0), 0),
            Err(PairError::Invalid(_))
        ));
        assert!(matches!(
            request_pairing(&db, "ok", "abc", 0),
            Err(PairError::Invalid(_))
        ));

        // занятое имя: заявка остаётся, админ называет другое имя
        add_runner(&db, "main", 0).unwrap();
        let p = request_pairing(&db, "main", &hash_of(1), 0).unwrap();
        let err = approve_pairing(&db, &p.code, None, 10).unwrap_err();
        assert!(err.contains("уже есть"), "{err}");
        let r = approve_pairing(&db, &p.code, Some("second"), 20).unwrap();
        assert_eq!(r.name, "second");
        assert!(approve_pairing(&db, &p.code, Some("bad name!"), 30).is_err());

        // срок: код живёт PAIR_TTL_MS
        let p = request_pairing(&db, "late", &hash_of(2), 100).unwrap();
        let err = approve_pairing(&db, &p.code, None, 100 + PAIR_TTL_MS).unwrap_err();
        assert!(err.contains("устарел"), "{err}");

        // потолок ждущих заявок (одна — «late» — уже ждёт); просроченные ему не мешают
        for i in 10..10 + PAIR_MAX as u32 - 1 {
            request_pairing(&db, "many", &hash_of(i), 1_000).unwrap();
        }
        assert_eq!(
            request_pairing(&db, "many", &hash_of(99), 1_000),
            Err(PairError::TooMany)
        );
        assert!(request_pairing(&db, "many", &hash_of(99), 1_000 + PAIR_TTL_MS).is_ok());
    }
}
