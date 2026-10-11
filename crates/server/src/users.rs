//! Пользователи сервера (SRV-02). Бот фазы 7 ведёт те же строки (`/adduser`).

use crate::Result;
use crate::db::{Db, db_err};
use rusqlite::{OptionalExtension, params};

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub tg_id: Option<i64>,
    pub role: String,
    pub created_at: i64,
}

fn user_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        tg_id: r.get(2)?,
        role: r.get(3)?,
        created_at: r.get(4)?,
    })
}

const USER_COLUMNS: &str = "id, name, tg_id, role, created_at";

pub fn add_user(db: &Db, name: &str, tg_id: Option<i64>, admin: bool, now_ms: i64) -> Result<User> {
    if !(1..=64).contains(&name.chars().count()) || name.chars().any(char::is_control) {
        return Err("имя пользователя: от 1 до 64 символов без управляющих".to_owned());
    }
    let role = if admin { "admin" } else { "member" };
    db.write(|tx| {
        let taken: Option<i64> = tx
            .query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        if taken.is_some() {
            return Err(format!("пользователь «{name}» уже есть"));
        }
        if let Some(tg) = tg_id {
            let by_tg: Option<i64> = tx
                .query_row("SELECT id FROM users WHERE tg_id = ?1", [tg], |r| r.get(0))
                .optional()
                .map_err(db_err)?;
            if by_tg.is_some() {
                return Err(format!("Telegram-id {tg} уже у другого пользователя"));
            }
        }
        tx.execute(
            "INSERT INTO users(name, tg_id, role, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, tg_id, role, now_ms],
        )
        .map_err(db_err)?;
        Ok(User {
            id: tx.last_insert_rowid(),
            name: name.to_owned(),
            tg_id,
            role: role.to_owned(),
            created_at: now_ms,
        })
    })
}

pub fn list_users(db: &Db) -> Result<Vec<User>> {
    db.read(|c| {
        let mut st = c
            .prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY id"))
            .map_err(db_err)?;
        let rows = st.query_map([], user_from_row).map_err(db_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(db_err)
    })
}

pub fn find_user(db: &Db, name: &str) -> Result<Option<User>> {
    db.read(|c| {
        c.query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            [name],
            user_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_are_added_listed_and_found() {
        let db = Db::in_memory().unwrap();
        let alice = add_user(&db, "alice", Some(42), true, 10).unwrap();
        assert_eq!(alice.role, "admin");
        add_user(&db, "bob", None, false, 11).unwrap();
        assert!(
            add_user(&db, "alice", None, false, 12)
                .unwrap_err()
                .contains("уже есть")
        );
        assert!(add_user(&db, "carol", Some(42), false, 12).is_err());
        assert_eq!(list_users(&db).unwrap().len(), 2);
        assert_eq!(find_user(&db, "bob").unwrap().unwrap().role, "member");
        assert!(find_user(&db, "nobody").unwrap().is_none());
    }
}
