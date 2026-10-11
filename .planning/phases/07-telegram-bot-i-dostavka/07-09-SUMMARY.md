---
phase: 07-telegram-bot-i-dostavka
plan: 09
subsystem: server
tags: [telegram, bot, delivery, resend, failure-report, uncertain]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 02
    provides: каркас доставки (sending до вызова, uncertain), TgError, InputFile::Id, FakeTelegram, BotStand
  - phase: 07-telegram-bot-i-dostavka
    plan: 08
    provides: тик notify (статусы, «отменена»), commands со /status /queue /cancel /retry
  - phase: 07-telegram-bot-i-dostavka
    plan: 10
    provides: потоки бота на запущенном сервере, tests/common::TestServer с tg
provides:
  - "deliver::fail_delivery: классы ошибок доставки (uncertain / повтор / failed / muted), MAX_ATTEMPTS = 5, повтор через 30 с или retry_after"
  - "deliver::SEND_LIMIT_BYTES = 1950 МиБ: больший ролик не отправляется, друг получает объяснение"
  - "deliver::resend + /resend [N]: по file_id для доставленного, с файла для неясного и не ушедшего"
  - "notify::phrase_for / friend_phrase: своя фраза для каждого кода wire::fail::KNOWN"
  - "notify: отчёт о сбое другу (новое сообщение с /retry N, правка статуса на «сбой») и каждому админу (код, этап, подсказка, выдач раннеру)"
  - "store: requeue_delivery, retry_delivery, failure_candidates, claim_admin_report"
affects: [07-11]

actuals:
  tokens: 18000
  tasks: 3
  commits: 3
plan_head_before: 945513fce2fa89f3dca4681b9c57d909805bd102
plan_head_after: cc0fd50f5c38d85916421de411713a9c9e7e260e
commits: 3

tech-stack:
  added: []
  patterns:
    - "одно сообщение на исход: право его отправить занимает UPDATE ... WHERE final_reported_ms IS NULL (доставка) или admin_reported_ms IS NULL (отчёт админам); точно не ушедшее снимает отметку"
    - "повторяется только то, что точно не ушло: maybe_delivered() -> uncertain без автоповтора, прочее по классам"
    - "то, что пришло от раннера, идёт только админу и только через Redactor::clean; другу — строка из таблицы по коду"

key-files:
  created:
    - crates/server/tests/bot_delivery.rs
  modified:
    - crates/server/src/bot/deliver.rs
    - crates/server/src/bot/notify.rs
    - crates/server/src/bot/commands.rs
    - crates/server/src/bot/store.rs
    - crates/server/tests/bot_e2e.rs
    - crates/server/tests/bot_status.rs

key-decisions:
  - "Исход доставки другу и отчёт о сбое делят отметку final_reported_ms (у готовой задачи она значит «про доставку сказано», у упавшей — «про сбой сказано»); /resend её снимает, чтобы новая попытка получила своё сообщение"
  - "/resend от пользователя разглушает весь его чат: человек только что написал боту, значит чат открыт (muted = 0 у всех строк чата)"
  - "Если Telegram не принял file_id (400), /resend не отвечает ошибкой, а возвращает доставку в pending и отправляет с файла"
  - "retry_after = 0 при 429 поднимается до 1 с: нулевая пауза крутила бы цикл доставки"
  - "Админ получает отчёт даже при заглушённом чате владельца; друг при заглушённом чате не получает ничего"
  - "/resend чужой или несуществующей задачи отвечает одинаково «Это не ваша задача.» (как /cancel и /retry); админ чужой ролик тоже не получает"

requirements-completed: []
---

# Phase 7 Plan 09: доставка без слепых повторов и отчёты о сбоях Summary

Неясная отправка ролика даёт `uncertain` и не повторяется сама, друг повторяет её командой `/resend N`, а при сбое задачи друг получает короткую фразу без путей и кода с `/retry N`, каждый админ — подробный отчёт.

## Accomplishments

- **Классы ошибок доставки** (`deliver::fail_delivery`). Тайм-аут или обрыв после начала отправки: `uncertain`, друг получает «Не уверен, что ролик задачи #N дошёл. Если его нет в чате — отправьте /resend N», автоповтора нет. `429`: `pending`, `next_try_ms = now + retry_after` (не меньше секунды). Ошибка до отправки запроса: `pending` через 30 с. После пятой попытки: `failed` и сообщение другу с `/resend N`. `403`: `failed`, `muted = 1` для чата, в чат больше ничего не пишется, остальные чаты обрабатываются тем же `step()`. Прочие ответы API: `failed` и сообщение другу.
- **Предел размера.** `SEND_LIMIT_BYTES = 1950 МиБ` (запас до 2000 МБ, допущение A5), размер берётся из `files`; больший ролик помечается `failed` без вызова Telegram, друг читает «Ролик задачи #N больше лимита Telegram (≈2 ГБ); автор передаст его иначе.». Недостающий файл результата тоже сообщается другу.
- **`/resend [N]`** (`deliver::resend`, ветка в `commands.rs`, пункт в меню и справке). Без номера берётся последняя готовая. Доставленный (`sent` с `result_file_id`) уходит сразу `send_video`/`send_document` с `InputFile::Id`, файл не читается, `result_message_id` обновляется. `uncertain`/`failed`: «Отправляю ещё раз; если ролик уже пришёл, дубль — нормально», затем `pending` с `delivery_attempts = 0`, `muted = 0`, отправку делает `deliver_once` с файла. Идущая доставка: «уже отправляется», неготовая задача: «ещё не готов», чужая: отказ.
- **Фразы другу** (`notify::phrase_for` / `friend_phrase`). Явная строка для каждого из 25 кодов `wire::fail::KNOWN`, общая фраза для остальных (`UNKNOWN` и неизвестные). Модульный страж проверяет длину до 300 символов, отсутствие `\`, `:\`, `/home`, `/var`, `/tmp`, `/mnt`, `/srv`, `Users`, `AppData`, `.dem` и самого кода.
- **Отчёты о сбое** (тик `notify`). Другу новое сообщение «Задача #N не получилась: <фраза>. Повторить: /retry N», сообщение статуса правится на «Задача #N: сбой». Каждому админу (`role = 'admin'`, есть `tg_id`, не убран) отдельное сообщение: номер, пользователь, игрок, код, этап (и фаза), подсказка, «выдач раннеру: K», сообщение раннера до 500 символов, всё через `Redactor::clean`; хвост консоли в чат не идёт. Отметки `final_reported_ms` и `admin_reported_ms` дают ровно одно сообщение на получателя. Владелец-админ получает оба.

## Tests

`crates/server/tests/bot_delivery.rs` (11): `timeout_after_upload_marks_uncertain_and_resend_sends_once_more`, `sent_video_is_resent_by_file_id`, `rate_limit_and_unsent_errors_retry_without_duplicates`, `forbidden_marks_undeliverable`, `oversized_result_is_refused_before_sending`, `document_mode_sends_a_document`, `other_users_cannot_resend`, `resend_says_when_the_video_is_not_ready_or_already_going`, `failure_sends_a_friend_phrase_and_an_admin_report`, `server_side_failures_are_reported_too`, `admin_owner_gets_both_messages_and_a_muted_friend_only_the_admin_report`. Модульный `notify::tests::every_known_code_has_a_friend_phrase_without_paths`. Сквозной `failure_report_reaches_friend_and_admin_through_the_running_server` в `bot_e2e.rs` (потоки бота, HTTP-раннер, сбой `GAME_NOT_STARTED`).

Зелёные: `cargo test --release -p server` целиком (все наборы), `cargo test --release -p cli --test pipeline`. `cargo clippy` по изменённым файлам замечаний не даёт (оставшиеся в `config.rs` и `deps.rs` старые). Форматирование — `rustfmt` по затронутым файлам.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Тест 07-08 предполагал, что после сбоя в чате нет других сообщений**
- **Found during:** Task 3 (полный прогон `-p server`)
- **Issue:** `retry_requeues_from_the_stored_demo_without_get_file` в `bot_status.rs` берёт «последнее сообщение чата» как ответ на `/retry`; теперь тот же `step()` приносит отчёт о сбое, и последним оказывается он.
- **Fix:** в двух местах после `finish_failed` добавлен `st.step()`, отчёт приходит до команды. Поведение кода не менялось, поправлено допущение теста.
- **Files modified:** crates/server/tests/bot_status.rs (в `files_modified` плана не значился)
- **Commit:** cc0fd50

**2. Коммиты.** Задание 1 содержит и каркас хранилища `requeue_delivery`; повторные ошибки, 403 и предел размера (`retry_delivery`) вошли в задание 2, как в плане. Предел размера и недостающий файл тоже сообщаются другу через `tell` (в плане сообщения только для `uncertain` и `failed`; это и есть `failed`).

## Known Limitations

- **Перезапуск во время отправки.** `Bot::recover` переводит `sending` в `uncertain`, но друг сообщения об этом не получает (только запись в журнале): сообщение посылает `deliver_once` в момент ошибки. Узнать об этом друг может по отсутствию ролика, `/resend` работает. Кандидат на 07-11 или отложенный пункт.
- **Повтор отчёта при упорной ошибке отправки.** Если `send_message` с отчётом о сбое стабильно падает не-403 ошибкой без признака «могло дойти», отметка снимается, и тик повторяет попытку каждые 2 с (как у «отменена» в 07-08), с записью в журнал. Предела числа попыток у отчётов нет.
- **Отчёт админам не повторяется.** Отметка `admin_reported_ms` занимается до рассылки: падение процесса между занятием и отправкой теряет отчёт (в `job show` данные остаются). Недоступный админ не блокирует остальных.
- **Ранее упавшие задачи.** На базе, где до 07-09 уже были упавшие задачи с `tg_jobs`, первый тик пришлёт по ним отчёты один раз.

## Known Stubs

Нет.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-35 (`uncertain` без автоповтора, `/resend` с предупреждением, повтор только после «точно не ушло»; тесты Task 1-2), T-07-36 (другу только фраза из таблицы, страж по всем `KNOWN`, `fail_message` только админу через `Redactor::clean`; тесты `every_known_code_has_a_friend_phrase_without_paths`, `failure_sends_a_friend_phrase_and_an_admin_report`), T-07-37 (владелец проверяется в `resend`; `other_users_cannot_resend`), T-07-38 (ошибки изолированы по строке, 403 глушит чат, 429 откладывает, предел 1950 МиБ до вызова), T-07-SC (новых пакетов нет).

## Notes for 07-11

- Живой стенд: проверить фактический предел `sendVideo` локального Bot API около 1950 МиБ и поведение `file_id` при `/resend` на настоящем Telegram (в фейке `file_id` не истекает).
- BOT-07 и BOT-09 остаются Pending в REQUIREMENTS.md до живой приёмки (07-11).

## Self-Check: PASSED

- FOUND: `pub fn resend(`, `maybe_delivered()`, `SEND_LIMIT_BYTES`, `is_forbidden()` в crates/server/src/bot/deliver.rs
- FOUND: ветка `"resend"` в crates/server/src/bot/commands.rs
- FOUND: `pub fn phrase_for(`, `pub fn friend_phrase(`, `admin_reported_ms` в crates/server/src/bot/notify.rs, цикл по `wire::fail::KNOWN` в тесте
- FOUND: crates/server/tests/bot_delivery.rs (11 тестов), сквозной тест в crates/server/tests/bot_e2e.rs
- FOUND: коммиты dbd4f57, 3b3f22f, cc0fd50 в истории main
