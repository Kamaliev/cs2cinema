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
    /// Сохранённый игрок по умолчанию: SteamID64.
    pub player_steam: Option<String>,
    /// Сохранённый игрок по умолчанию: ник.
    pub player_name: Option<String>,
    /// Когда друга убрали из белого списка; строка остаётся ради истории задач.
    pub removed_at: Option<i64>,
}

impl User {
    /// Друг в белом списке: его ещё не убирали.
    pub fn is_active(&self) -> bool {
        self.removed_at.is_none()
    }
}

fn user_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        name: r.get(1)?,
        tg_id: r.get(2)?,
        role: r.get(3)?,
        created_at: r.get(4)?,
        player_steam: r.get(5)?,
        player_name: r.get(6)?,
        removed_at: r.get(7)?,
    })
}

const USER_COLUMNS: &str =
    "id, name, tg_id, role, created_at, player_steam, player_name, removed_at";

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
            player_steam: None,
            player_name: None,
            removed_at: None,
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

/// Пользователь по Telegram-id (в том числе убранный: `is_active()` решает, пускать ли его).
pub fn find_user_by_tg(db: &Db, tg_id: i64) -> Result<Option<User>> {
    db.read(|c| {
        c.query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE tg_id = ?1"),
            [tg_id],
            user_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

/// Полные настройки монтажа пользователя: сохранённое поверх умолчаний реестра (`wire::settings`).
pub fn user_settings(db: &Db, user_id: i64) -> Result<serde_json::Map<String, serde_json::Value>> {
    let raw: Option<String> = db.read(|c| {
        c.query_row("SELECT settings FROM users WHERE id = ?1", [user_id], |r| r.get(0))
            .optional()
            .map_err(db_err)
    })?;
    Ok(wire::settings::resolve(&raw.unwrap_or_default()))
}

/// Меняет настройки пользователя (`{"ключ": значение}`, `null` — сбросить) и возвращает полный набор после правки.
/// Проверка и слияние целиком в `wire::settings`; чтение и запись — одной транзакцией, чтобы две правки подряд не потерялись.
pub fn patch_user_settings(db: &Db, user_id: i64, patch: &serde_json::Value) -> Result<serde_json::Map<String, serde_json::Value>> {
    let raw = db.write(|tx| {
        let old: String = tx
            .query_row("SELECT settings FROM users WHERE id = ?1", [user_id], |r| r.get(0))
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| "нет такого пользователя".to_owned())?;
        let new = wire::settings::apply_patch(&old, patch)?;
        tx.execute("UPDATE users SET settings = ?2 WHERE id = ?1", params![user_id, new]).map_err(db_err)?;
        Ok::<String, String>(new)
    })?;
    Ok(wire::settings::resolve(&raw))
}

pub fn find_user_by_id(db: &Db, id: i64) -> Result<Option<User>> {
    db.read(|c| {
        c.query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
            [id],
            user_from_row,
        )
        .optional()
        .map_err(db_err)
    })
}

fn check_name(name: &str) -> Result<()> {
    if !(1..=64).contains(&name.chars().count()) || name.chars().any(char::is_control) {
        return Err("имя пользователя: от 1 до 64 символов без управляющих".to_owned());
    }
    Ok(())
}

/// Добавляет друга по Telegram-ID с ролью `member`. Убранного с этим ID возвращает в список (имя прежнее) —
/// тогда второй элемент ответа `true`. Активный с этим ID или занятое имя — ошибка. Админов отсюда не создают.
pub fn add_member_by_tg(db: &Db, tg_id: i64, name: &str, now_ms: i64) -> Result<(User, bool)> {
    check_name(name)?;
    db.write(|tx| {
        let existing: Option<User> = tx
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE tg_id = ?1"),
                [tg_id],
                user_from_row,
            )
            .optional()
            .map_err(db_err)?;
        if let Some(mut u) = existing {
            if u.is_active() {
                return Err(format!("Telegram-ID {tg_id} уже в списке"));
            }
            tx.execute("UPDATE users SET removed_at = NULL WHERE id = ?1", [u.id])
                .map_err(db_err)?;
            u.removed_at = None;
            return Ok((u, true));
        }
        let taken: Option<i64> = tx
            .query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        if taken.is_some() {
            return Err(format!("имя «{name}» уже занято"));
        }
        tx.execute(
            "INSERT INTO users(name, tg_id, role, created_at) VALUES (?1, ?2, 'member', ?3)",
            params![name, tg_id, now_ms],
        )
        .map_err(db_err)?;
        Ok((
            User {
                id: tx.last_insert_rowid(),
                name: name.to_owned(),
                tg_id: Some(tg_id),
                role: "member".to_owned(),
                created_at: now_ms,
                player_steam: None,
                player_name: None,
                removed_at: None,
            },
            false,
        ))
    })
}

/// Мягко убирает друга из белого списка (`removed_at`): строка остаётся ради истории задач. Админа убрать
/// нельзя, неизвестного и уже убранного — ошибка.
pub fn remove_user(db: &Db, tg_id: i64, now_ms: i64) -> Result<User> {
    db.write(|tx| {
        let found: Option<User> = tx
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE tg_id = ?1"),
                [tg_id],
                user_from_row,
            )
            .optional()
            .map_err(db_err)?;
        let Some(mut u) = found else {
            return Err(format!("Telegram-ID {tg_id} нет в списке"));
        };
        if u.role == "admin" {
            return Err("админа убирает оператор на сервере".to_owned());
        }
        if !u.is_active() {
            return Err(format!("{} ({tg_id}) уже убран", u.name));
        }
        tx.execute(
            "UPDATE users SET removed_at = ?1 WHERE id = ?2",
            params![now_ms, u.id],
        )
        .map_err(db_err)?;
        u.removed_at = Some(now_ms);
        Ok(u)
    })
}

/// Запоминает игрока пользователя по умолчанию: SteamID64 (если это он) и ник. Прежнее значение заменяется целиком.
pub fn set_player(db: &Db, user_id: i64, steam: Option<&str>, name: &str) -> Result<()> {
    db.write(|tx| {
        let n = tx
            .execute(
                "UPDATE users SET player_steam = ?1, player_name = ?2 WHERE id = ?3",
                params![steam, name, user_id],
            )
            .map_err(db_err)?;
        if n == 0 {
            return Err("пользователь не найден".to_owned());
        }
        Ok(())
    })
}

/// Сколько задач пользователя ещё не закончено (приём, очередь, выдана, запись, выгрузка).
pub fn active_jobs_of(db: &Db, user_id: i64) -> Result<i64> {
    db.read(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM jobs WHERE user_id = ?1
               AND state IN ('receiving','queued','assigned','running','uploading')",
            [user_id],
            |r| r.get(0),
        )
        .map_err(db_err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_user_is_found_but_inactive_and_can_be_restored() {
        let db = Db::in_memory().unwrap();
        let (u, restored) = add_member_by_tg(&db, 222, "петя", 10).unwrap();
        assert!(!restored);
        assert_eq!(u.role, "member");
        assert!(
            add_member_by_tg(&db, 222, "другой", 11)
                .unwrap_err()
                .contains("уже в списке")
        );
        let gone = remove_user(&db, 222, 20).unwrap();
        assert_eq!(gone.removed_at, Some(20));
        let found = find_user_by_tg(&db, 222).unwrap().unwrap();
        assert!(!found.is_active(), "убранный найден, но не активен");
        assert!(list_users(&db).unwrap().iter().any(|x| x.id == u.id));
        assert!(remove_user(&db, 222, 21).unwrap_err().contains("уже убран"));
        let (back, restored) = add_member_by_tg(&db, 222, "иначе", 30).unwrap();
        assert!(restored);
        assert_eq!(back.name, "петя", "имя прежнее");
        assert!(find_user_by_tg(&db, 222).unwrap().unwrap().is_active());
        assert!(
            remove_user(&db, 999, 40).is_err(),
            "неизвестного убрать нельзя"
        );
        add_user(&db, "ada", Some(1), true, 1).unwrap();
        assert!(
            remove_user(&db, 1, 50).unwrap_err().contains("оператор"),
            "админа через чат не убрать"
        );
        assert!(
            add_member_by_tg(&db, 7, "петя", 60)
                .unwrap_err()
                .contains("занято")
        );
    }

    #[test]
    fn player_is_replaced_as_a_whole_and_active_jobs_are_zero_without_jobs() {
        let db = Db::in_memory().unwrap();
        let (u, _) = add_member_by_tg(&db, 5, "вася", 1).unwrap();
        set_player(&db, u.id, Some("76561198034202275"), "76561198034202275").unwrap();
        set_player(&db, u.id, None, "ZywOo").unwrap();
        let u = find_user_by_id(&db, u.id).unwrap().unwrap();
        assert_eq!(u.player_steam, None);
        assert_eq!(u.player_name.as_deref(), Some("ZywOo"));
        assert!(set_player(&db, 999, None, "x").is_err());
        assert_eq!(active_jobs_of(&db, u.id).unwrap(), 0);
    }

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
