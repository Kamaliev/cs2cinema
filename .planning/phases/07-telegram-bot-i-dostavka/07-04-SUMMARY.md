---
phase: 07-telegram-bot-i-dostavka
plan: 04
subsystem: server
tags: [telegram, bot, whitelist, admin-commands, player, access-control]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 02
    provides: Bot, dispatch с белым списком, FakeTelegram, BotStand
provides:
  - "users::add_member_by_tg, remove_user (мягко, removed_at), set_player, active_jobs_of"
  - "bot::commands: /adduser, /removeuser, /users (только role = admin), /player, /start, /help, valid_player, menu() на шесть команд"
  - "tests/bot_access.rs: 12 тестов на белый список, обходы, команды админа, игрока и судьбу задач убранного друга"
affects: [07-06, 07-08, 07-09, 07-10]

actuals:
  tokens: 21000
  tasks: 3
  commits: 3
plan_head_before: f77deda1b17f735222fb56fc827a3dc72d8cb9e7
plan_head_after: 3af7bc9ebd07de221dfe96adfa5ef8796564579f
commits: 3

tech-stack:
  added: []
  patterns:
    - "роль только из базы: команды админа проверяют user.role == admin до любого изменения, /adduser создаёт только member"
    - "мягкое удаление: users.removed_at, строка остаётся ради jobs.user_id, повторный /adduser снимает отметку"

key-files:
  created:
    - crates/server/tests/bot_access.rs
  modified:
    - crates/server/src/bot/commands.rs
    - crates/server/src/users.rs
    - crates/server/src/core.rs

key-decisions:
  - "valid_player (1-64 символа, без управляющих, U+200E/F, U+202A-E, U+2066-9, U+2028/9, U+FEFF) общий для /player и имени в /adduser"
  - "Флаги в /adduser (любой токен, начинающийся с «-») отвергаются с подсказкой формата: админа назначает только оператор"
  - "SteamID64 в /player сохраняется парой player_steam + player_name, ник — только player_name (прежний player_steam сбрасывается)"
  - "Политика убранного друга: принятые задачи доходят и доставляются через tg_jobs.chat_id, deliver.rs белый список не сверяет и не менялся"

requirements-completed: []
---

# Phase 7 Plan 04: доступ к боту Summary

Админ управляет белым списком из чата (`/adduser`, `/removeuser`, `/users`), друг задаёт игрока командой `/player` (подпись к файлу переопределяет его на одну задачу), а незнакомец, убранный друг и любые необычные виды сообщений (правка, пост канала, группа, пересланный документ) не доводят бота до `getFile`.

## Accomplishments

- **`users.rs`.** `add_member_by_tg` (убранного с тем же ID возвращает с прежним именем, активного и занятое имя отвергает, роль всегда `member`), `remove_user` (мягко, админа и неизвестного отвергает, повторное убирание тоже ошибка), `set_player` (заменяет пару целиком), `active_jobs_of` (receiving/queued/assigned/running/uploading).
- **`commands.rs`.** Ветки `/adduser`, `/removeuser`, `/users` с проверкой роли до любых действий («Эта команда только для администратора»), `/player` (без аргумента показывает, с аргументом сохраняет), `/start`, `/help` (админу дополнительно команды управления). Telegram-ID — только цифры, положительный `i64`. `/removeuser` сообщает число активных задач друга и что они закончатся, ролики будут доставлены. `menu()` содержит `start`, `help`, `player`, `adduser`, `removeuser`, `users`.
- **Обходы.** `dispatch` менять не пришлось: решение принимается только по `message` из личного чата, `from.id` проверяется до чтения документа, незнакомцу отвечают не чаще раза за `stranger_reply_ms`, остальное в `Update` игнорируется serde, а смещение всё равно сдвигается.

## Tests

`tests/bot_access.rs` (12): `admin_adds_and_removes_a_friend_from_the_chat`, `stranger_gets_only_the_id_and_nothing_is_fetched` (текст, документ, пересланный документ, `/start`, `/adduser`: одно сообщение, `getFile` 0, пустые `tg_inbox` и `jobs`, смещение за все обновления), `edited_channel_and_group_updates_are_ignored` (`edited_message`, `channel_post`, `my_chat_member`, `callback_query`, документ в группе: ни одной отправки, `getFile` 0), `stranger_reply_is_rate_limited` (9 минут молчание, 10 минут ответ), `member_cannot_run_admin_commands`, `adduser_never_grants_admin`, `forwarded_document_from_a_friend_is_his_own_demo`, `player_is_saved_and_fixed_in_the_job`, `caption_overrides_the_saved_player_for_one_job`, `player_with_control_or_bidi_characters_is_refused`, `start_and_help_list_the_commands`, `removed_friend_job_still_finishes_and_is_delivered`.
Модульные: `removed_user_is_found_but_inactive_and_can_be_restored`, `player_is_replaced_as_a_whole_and_active_jobs_are_zero_without_jobs`, `player_name_is_checked_for_length_and_hidden_characters`, `telegram_id_is_digits_only_and_positive`.
Зелёные: `cargo test --release -p server` целиком, `cargo test --release -p cli --test pipeline`. Новых предупреждений clippy в изменённых файлах нет.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - блокер] `core::is_steam64` стала `pub`**
- **Issue:** `/player` должен отличать SteamID64 по той же форме, что `Core::submit`; дублировать проверку значило бы разъехаться при правке.
- **Fix:** одно слово `pub` в `crates/server/src/core.rs` (файла нет в `files_modified`).
- **Commit:** 8f91e49

**2. Task 2 и Task 3 без правок реализации.** Все поведения прошли на коде Task 1 (`dispatch` из 07-02 уже закрывал обходы, а `deliver` белый список не сверяет), поэтому коммиты 2 и 3 содержат только тесты. Тест с убранным другом подтвердил политику «дать задаче закончиться»: правка 07-02 не понадобилась.

## Known Limitations

- Незнакомцу, убранному только что, бот не ответит, если ему уже отвечали как незнакомцу менее `stranger_reply_ms` назад (общий счётчик по `from.id`).
- `/start` называет ссылку на комнату FACEIT как способ прислать матч; приём текстовой ссылки в чате появляется в плане 07-06, пока такое сообщение получает подсказку «пришлите файл».
- Меню команд общее для всех (Telegram не прячет команды по ролям), права проверяются при выполнении.

## Known Stubs

Нет.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-13 (тесты Task 2), T-07-14 (`member_cannot_run_admin_commands`, `adduser_never_grants_admin`), T-07-15 (`stranger_reply_is_rate_limited`), T-07-16 (`valid_player` до сохранения, тест Task 3), T-07-17 (`removed_friend_job_still_finishes_and_is_delivered`), T-07-SC (новых пакетов нет).

## Self-Check: PASSED

- FOUND: `pub fn remove_user(`, `pub fn add_member_by_tg(`, `pub fn set_player(`, `pub fn active_jobs_of(` в crates/server/src/users.rs; `fn valid_player(`, `/adduser`, `/removeuser`, `/users`, проверка роли admin в crates/server/src/bot/commands.rs
- FOUND: 12 тестов `#[test]` и 7 проверок `get_file_calls()` в crates/server/tests/bot_access.rs
- FOUND: коммиты 8f91e49, d48b34e, 3af7bc9 в истории main
