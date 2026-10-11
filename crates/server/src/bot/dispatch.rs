//! Разбор одного обновления: кто пишет и что это (до любых записей), затем запись смещения и побочные действия.
//!
//! Порядок проверок обязателен (исследование §9): нет `message` — игнор; не личный чат или нет `from` — игнор;
//! `from.id` ищется среди активных пользователей, незнакомцу — один ответ с его ID, и дальше НИЧЕГО не
//! разбирается (ни `document`, ни подпись, ни текст). Новые виды входа добавляются только в `ingest.rs`.

use super::ingest::{self, Accepted, Intake, Precheck};
use super::{Bot, commands, store};
use crate::Result;
use crate::tg::{Message, Update};
use crate::users::{self, User};

/// Решение «что это», принятое без записей в базу.
pub enum Decision {
    Ignore,
    /// Ответ без строки во входящих: незнакомцу, подсказка, просьба задать игрока.
    Reply {
        chat_id: i64,
        text: String,
    },
    Command {
        user: User,
        chat_id: i64,
        text: String,
    },
    Intake {
        user: User,
        message: Box<Message>,
        intake: Intake,
    },
}

/// Ответ незнакомцу; ID нужен, чтобы друг передал его автору.
fn stranger_text(from_id: i64) -> String {
    format!(
        "Бот работает только для друзей автора. Ваш Telegram-ID: {from_id} — передайте его автору."
    )
}

/// Определяет, что делать с обновлением. Только чтение базы.
pub fn decide(bot: &Bot, upd: &Update) -> Result<Decision> {
    let Some(msg) = &upd.message else {
        return Ok(Decision::Ignore);
    };
    let Some(from) = &msg.from else {
        return Ok(Decision::Ignore);
    };
    if msg.chat.kind != "private" || from.is_bot {
        return Ok(Decision::Ignore);
    }
    let chat_id = msg.chat.id;
    let user = match users::find_user_by_tg(bot.core().db(), from.id)? {
        Some(u) if u.is_active() => u,
        _ => {
            return Ok(if bot.stranger_may_get_reply(from.id) {
                Decision::Reply {
                    chat_id,
                    text: stranger_text(from.id),
                }
            } else {
                Decision::Ignore
            });
        }
    };
    let text = msg.text.as_deref().map(str::trim).unwrap_or("");
    if msg.document.is_none() && text.starts_with('/') {
        return Ok(Decision::Command {
            user,
            chat_id,
            text: text.to_owned(),
        });
    }
    if msg.document.is_none() && text.is_empty() {
        return Ok(Decision::Ignore);
    }
    Ok(match ingest::precheck(bot, &user, msg) {
        Precheck::Reply(text) => Decision::Reply { chat_id, text },
        Precheck::Accept(intake) => Decision::Intake {
            user,
            message: Box::new(msg.clone()),
            intake,
        },
    })
}

/// Обрабатывает обновление целиком: решение, запись смещения (и строки входящих) одной транзакцией, затем
/// побочные действия. Команды выполняются «не больше одного раза»: смещение сохраняется до выполнения.
pub fn handle_update(bot: &Bot, upd: Update) -> Result<()> {
    let decision = decide(bot, &upd)?;
    let accepted = bot.core().db().write(|tx| {
        let accepted = match &decision {
            Decision::Intake {
                user,
                message,
                intake,
            } => Some(ingest::accept(bot, tx, user, message, intake)?),
            _ => None,
        };
        store::set_offset(tx, upd.update_id + 1)?;
        Ok::<_, String>(accepted)
    })?;
    match decision {
        Decision::Ignore => {}
        Decision::Reply { chat_id, text } => {
            bot.say(chat_id, &text);
        }
        Decision::Command {
            user,
            chat_id,
            text,
        } => commands::handle(bot, &user, chat_id, &text),
        Decision::Intake { message, .. } => match accepted {
            Some(Accepted::Row(row_id)) => {
                let text = ingest::ack_text(bot, row_id);
                if let Some(mid) = bot.say(message.chat.id, &text) {
                    store::set_inbox_status_message(bot.core().db(), row_id, mid)?;
                }
            }
            Some(Accepted::Refused(text)) => {
                bot.say(message.chat.id, &text);
            }
            Some(Accepted::Already) | None => {}
        },
    }
    Ok(())
}
