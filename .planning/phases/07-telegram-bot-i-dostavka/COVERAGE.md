# API Coverage — Telegram Bot API (локальный `telegram-bot-api --local`) и FACEIT Data API

> Полное покрытие по умолчанию. Каждый отказ (OPT-OUT) — явное решение с причиной.
> Фаза 7 (Telegram-бот и доставка). Планы: 07-02 (каркас, `FakeTelegram`), 07-05 (`HttpTg`), 07-06 (приём файлов), 07-07 (FACEIT), 07-08 (статус), 07-09 (доставка и отчёты), 07-10 (`serve`), 07-11 (спайк и живой стенд).

## Telegram Bot API

| capability | decision | reason |
|---|---|---|
| getUpdates (long polling, allowed_updates=message) | INTEGRATE | 07-02, 07-05, 07-10: смещение в одной транзакции с tg_inbox |
| update.message в личных чатах из белого списка | INTEGRATE | 07-02, 07-04 |
| getMe (самопроверка при старте, cs2-server bot check) | INTEGRATE | 07-05, 07-10 |
| sendMessage (простой текст, подтверждения, ответы, отчёты) | INTEGRATE | 07-02 |
| editMessageText (одно сообщение статуса не чаще ~10 с) | INTEGRATE | 07-08 |
| getFile (локальный режим, абсолютный путь на диске bot-api) | INTEGRATE | 07-02, 07-06 |
| sendVideo по file:/// (ролик новым сообщением, supports_streaming) | INTEGRATE | 07-02, 07-05, 07-09 |
| sendVideo по file_id (/resend доставленного ролика) | INTEGRATE | 07-09 |
| sendDocument по file:/// (способ отправки по ключу bot_send_as) | INTEGRATE | 07-05, 07-09; выбор по умолчанию — спайк 07-11 |
| setMyCommands (меню команд) | INTEGRATE | 07-02, 07-04, 07-08, 07-09 |
| ResponseParameters.retry_after (429, flood control) | INTEGRATE | 07-05, 07-08, 07-09 |
| ошибки 400 not modified, 400 message to edit not found, 403 blocked | INTEGRATE | 07-08, 07-09 |
| ошибка 409 (второй экземпляр getUpdates) | INTEGRATE | 07-10: предупреждение и пауза |
| Document.file_unique_id (дубль до getFile) | INTEGRATE | 07-06 |
| update.edited_message | OPT-OUT | отсечено allowed_updates: правка не должна давать второй путь к getFile (BOT-01) |
| update.channel_post и edited_channel_post | OPT-OUT | бот обслуживает только личные чаты из белого списка |
| update.callback_query, inline-клавиатуры, answerCallbackQuery | OPT-OUT | кнопки выбора игрока — BOT-F2, отложено автором |
| update.my_chat_member и chat_member | OPT-OUT | группы не обслуживаются, добавление бота в группу игнорируется |
| update.inline_query и inline-режим | OPT-OUT | не нужен для приёма демок |
| групповые чаты и супергруппы | OPT-OUT | только chat.type=private: группа — путь обхода белого списка |
| setWebhook, deleteWebhook, getWebhookInfo | OPT-OUT | выбран long polling: не нужен входящий порт и TLS; 409 пишется в журнал |
| logOut | OPT-OUT | разовая ручная операция автора при переходе с облака (спайк 07-11, развёртывание фазы 9); бот не вызывает её сам |
| close | OPT-OUT | перенос между локальными серверами не планируется |
| multipart-загрузка файлов | OPT-OUT | в локальном режиме file:/// читает сам telegram-bot-api, тело ролика не идёт через клиент |
| облачный Bot API api.telegram.org | OPT-OUT | лимиты 20/50 МБ не подходят демкам и роликам; облачный адрес отвергается проверкой настроек |
| маршрут скачивания /file/bot<token>/ | OPT-OUT | в локальном режиме файл читается с диска по абсолютному пути |
| parse_mode MarkdownV2 и HTML | OPT-OUT | ники из демок ломают разметку; весь текст простой |
| sendVideo width, height, duration | OPT-OUT | в протоколе раннера нет метаданных ролика; спайк 07-11 проверяет показ без них, при проблеме — план закрытия разрыва |
| sendVideo thumbnail | OPT-OUT | превью строит Telegram; ролик собирается с faststart |
| sendChatAction | OPT-OUT | ход задачи показывает сообщение статуса |
| deleteMessage | OPT-OUT | сообщения бота остаются историей задачи |
| forwardMessage и copyMessage | OPT-OUT | повторная отправка идёт по file_id |
| sendMediaGroup | OPT-OUT | один ролик на задачу |
| getChat и getChatMember | OPT-OUT | личность — числовой from.id из белого списка в базе сервера |
| reply_parameters (ответ на исходное сообщение) | OPT-OUT | статус и ролик называют номер задачи |
| платежи, стикеры, игры, бизнес-аккаунты, истории | OPT-OUT | вне задачи бота |

## FACEIT Data API

| capability | decision | reason |
|---|---|---|
| GET /matches/{match_id} — demo_url | INTEGRATE | 07-07: раннер, ключ автора, только match_id из строгого разбора |
| Downloads API — подписанный download_url | INTEGRATE | 07-07: раннер скачивает демку в свою папку |
| обращение сервера по ссылке пользователя | OPT-OUT | D-01: сервер хранит только канонический URL из match_id и никуда не ходит (нет SSRF) |
| выбор карты bo3 (demo_index больше 0) | OPT-OUT | берётся первая демка, как в pipeline (A9); выбор карты — вне фазы |
| статистика игроков и история матчей | OPT-OUT | боту не нужна |
