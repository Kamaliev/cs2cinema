//! Команды бота. Здесь только `/start` и `/help`; команды игрока, админа, статуса, отмены и повтора
//! добавляют следующие планы фазы.

use super::Bot;
use crate::users::User;

/// Разбирает `/команда@имя_бота аргументы` в пару «команда без слеша в нижнем регистре, аргументы».
pub fn parse(text: &str) -> (String, String) {
    let text = text.trim();
    let body = text.strip_prefix('/').unwrap_or(text);
    let (head, args) = match body.split_once(char::is_whitespace) {
        Some((h, a)) => (h, a.trim()),
        None => (body, ""),
    };
    let cmd = head.split('@').next().unwrap_or(head).to_lowercase();
    (cmd, args.to_owned())
}

/// Меню команд для `setMyCommands`.
pub fn menu() -> Vec<(String, String)> {
    vec![
        ("start".to_owned(), "Как пользоваться ботом".to_owned()),
        ("help".to_owned(), "Справка".to_owned()),
    ]
}

fn help_text() -> String {
    "Пришлите демку CS2 файлом (.dem или .dem.zst) — бот смонтирует ролик с лучшими моментами игрока.\n\
     Ник или SteamID64 игрока напишите в подписи к файлу. Если подписи нет, берётся сохранённый игрок.\n\
     Дальше бот напишет номер задачи, а готовый ролик пришлёт отдельным сообщением."
        .to_owned()
}

/// Выполняет команду и отвечает в чат.
pub fn handle(bot: &Bot, _user: &User, chat_id: i64, text: &str) {
    let (cmd, _args) = parse(text);
    let reply = match cmd.as_str() {
        "start" | "help" => help_text(),
        _ => "Не знаю такой команды. Список команд и порядок работы: /help".to_owned(),
    };
    bot.say(chat_id, &reply);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_split_from_arguments_and_bot_name() {
        assert_eq!(
            parse("/Player@my_bot  s1mple "),
            ("player".into(), "s1mple".into())
        );
        assert_eq!(parse("/help"), ("help".into(), String::new()));
    }
}
