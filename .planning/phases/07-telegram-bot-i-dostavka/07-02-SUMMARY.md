---
phase: 07-telegram-bot-i-dostavka
plan: 02
subsystem: server
tags: [telegram, bot, tracer, fake-telegram, sqlite-migration, exactly-once]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol
    provides: Core::submit, Db::write, ManualClock, TestServer
  - phase: 07-telegram-bot-i-dostavka
    plan: 01
    provides: закрытые CR-01 и WR-01 (сервер не падает от огромного Content-Length)
provides:
  - "шов server::tg::Tg (get_me, get_updates, send_message, edit_message_text, get_file, send_video, send_document, set_my_commands), TgError, InputFile, Sent и типы обновлений"
  - "server::tg::fake::FakeTelegram: сценарий обновлений, документы на диске, журнал вызовов, fail_next"
  - "миграция 2: tg_kv, tg_inbox (UNIQUE chat_id+message_id), tg_jobs, tg_files, users.removed_at, индекс files(sha256)"
  - "server::bot: Bot::step / poll_once / ingest_once / notify_once / deliver_once / startup / recover, BotSettings, file_uri"
  - "tests/common BotStand для планов 07-03..07-11"
affects: [07-04, 07-05, 07-06, 07-07, 07-08, 07-09, 07-10, 07-11]

actuals:
  tokens: 18000
  tasks: 2
  commits: 2
plan_head_before: 63f2ed47c15763a8610deb9172f41c0ad666a9c9
plan_head_after: c932b21a546d355946a46476c9aee6348ee5515c
commits: 2

tech-stack:
  added: []
  patterns:
    - "бот — шаговый автомат поверх Core: потоки только крутят step, время из core.clock()"
    - "принять-потом-обработать: tg_inbox и смещение getUpdates в одной Db::write, UNIQUE(chat_id, message_id)"
    - "намерение отправки sending пишется до вызова Telegram, слепого повтора нет"

key-files:
  created:
    - crates/server/src/tg/mod.rs
    - crates/server/src/tg/fake.rs
    - crates/server/src/bot/mod.rs
    - crates/server/src/bot/store.rs
    - crates/server/src/bot/dispatch.rs
    - crates/server/src/bot/commands.rs
    - crates/server/src/bot/ingest.rs
    - crates/server/src/bot/notify.rs
    - crates/server/src/bot/deliver.rs
    - crates/server/tests/bot_flow.rs
  modified:
    - crates/server/src/lib.rs
    - crates/server/src/db.rs
    - crates/server/src/users.rs
    - crates/server/tests/common/mod.rs

key-decisions:
  - "Игрок задачи (D-02): непустая подпись (после Redactor::clean, до 64 символов) переопределяет users.player_steam/player_name; нет ни того, ни другого — Precheck::Reply с просьбой /player, строка tg_inbox не создаётся"
  - "Команды выполняются не больше одного раза (смещение пишется до выполнения), документы ровно один раз через tg_inbox"
  - "Accepted получил третий вариант Already: повтор уже принятого сообщения молча пропускается, второго подтверждения нет"
  - "После успешного Core::submit оригинал в папке bot-api удаляется (двойная копия на 2 ГБ, Pitfall 7); ошибка удаления только в журнал без пути"
  - "Текст ошибки приёма для пользователя — класс ошибки, а не строка Core::submit (в ней путь, а в пути из getFile токен)"

requirements-completed: []
---

# Phase 7 Plan 02: срез «друг -> демка -> задача -> ролик» Summary

Друг из белого списка присылает `.dem` с ником в подписи, бот подтверждает приём, ставит задачу через `Core::submit`, правит подтверждение на «Задача #N в очереди», а когда фейковый раннер доводит задачу до `done`, присылает `highlights.mp4` новым сообщением; повтор обновления и перезапуск не дают второй задачи или второй отправки.

## Accomplishments

- **Шов `Tg`.** Трейт, `TgError` с `maybe_delivered`, `is_not_modified`, `is_message_missing`, `is_forbidden`, `retry_after_ms` и русским `Display` без адресов и путей. `HttpTg` на `ureq` в этом плане нет (план 07-05).
- **`FakeTelegram`** (публичный модуль): очередь обновлений с отбрасыванием `update_id < offset`, документы на настоящем диске, `getFile` с реальным абсолютным путём, правка и повторная правка (`not modified`, `message to edit not found`), разбор `file:///` с процентным декодированием, `fail_next` по имени метода. Для `Timeout{sent:true}` и `Io{sent:true}` на `send_video` и `send_document` ролик сначала записывается как доставленный, потом приходит ошибка.
- **Миграция 2** добавлением: `tg_kv`, `tg_inbox`, `tg_jobs`, `tg_files`, `users.removed_at`, индекс `files_sha256`. `users::User` получил `player_steam`, `player_name`, `removed_at`, `is_active()`; добавлены `find_user_by_tg` и `find_user_by_id`.
- **`Bot`.** `poll_once` решает без записи (белый список, `precheck`), одной `Db::write` пишет строку `tg_inbox` и смещение, затем отвечает. `getFile` только в `ingest_once`. Путь из `getFile` обязан лежать внутри `files_root` по компонентам и не содержать `..`; в журнал, чат и `reason` он не попадает. `deliver_once` пишет `sending` до вызова, итог `sent` / `uncertain` / `failed`. `recover` (из `startup`): `downloading` в `pending`, `sending` в `uncertain`.
- **`BotStand`**: настоящий `TestServer` на `ManualClock`, `FakeTelegram`, `Bot`; помощники друга, демки и фейкового раннера (`finish_done`, `finish_failed`).

## Tests

- `bot_flow` (11): сквозной срез `friend_demo_becomes_a_job_and_the_video_comes_back_as_a_new_message`, `repeated_update_creates_one_job`, `restart_after_poll_ingests_the_pending_demo_once`, `interrupted_download_is_retried_after_restart`, `sending_delivery_becomes_uncertain_after_restart_without_a_resend`, `poll_does_not_call_get_file`, `document_without_player_asks_for_player_and_queues_nothing`, `stranger_document_is_never_fetched`, `offset_is_last_update_id_plus_one_and_reaches_the_next_poll`, плюс `missing_confirmation_is_sent_before_the_download` и `saved_player_is_used_when_the_caption_is_empty`.
- Модульные: `file_uri_percent_encodes_unsafe_bytes`, `fresh_database_migrates_to_the_current_version` (таблицы бота и `users.removed_at`), разбор обновлений и ошибок `tg`, разбор URI и правок в фейке, разбор команд.
- Зелёные: `cargo test --release -p server` (включая `deps`, тесты фаз 5-6 и 07-01), `cargo test --release -p cli --test pipeline`. `cargo clippy -p server --all-targets` в новых файлах предупреждений не даёт (остались старые в `config.rs` и `tests/deps.rs`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - недостающее поведение] Третий исход `Accepted::Already`**
- **Issue:** контракт плана знает только `Row` и `Refused`, а `INSERT OR IGNORE` повторного сообщения не создаёт строки, и без третьего исхода бот либо падал, либо слал бы второе подтверждение.
- **Fix:** вариант `Accepted::Already` (молча пропустить). Перечисления плана и так предполагают расширение; на имена из контракта это не влияет.
- **Commit:** faf6de5

**2. [Rule 2 - недостающее поведение] Путь файла без `..` и оригинал после копирования**
- **Fix:** помимо префикса `files_root` по компонентам отвергаются компоненты, кроме обычных (`..`, корень), хотя полные проверки путей и ссылок остаются за 07-06; после успешной постановки оригинал в папке bot-api удаляется (исследование §2, Pitfall 7), ошибка удаления идёт в журнал без пути.
- **Commit:** faf6de5

**3. Мелкие дополнения к контракту:** `users::find_user_by_id`, `FakeTelegram::message_ids_to`, `fake::path_from_uri`, `BotStand::finish_done` возвращает размер результата, `document_update` принимает подпись как `Option<&str>`, два дополнительных теста.

**4. Task 2 без правок реализации:** все восемь поведений Task 2 прошли на коде Task 1, поэтому второй коммит содержит только тесты.

## Known Limitations

- Между успешным `Core::submit` и записью `queued` в `tg_inbox` остаётся окно из двух коротких вызовов (после копирования файла). Падение именно в нём вернёт строку в `pending` при перезапуске и поставит вторую задачу из той же демки. Закрыть можно связыванием `job_id` со строкой до `finish_receive`; решение отложено, окно узкое, а дубль безвреден (две задачи вместо потери).
- Подтверждение (`status_message_id`) в `tg_jobs` остаётся пустым, если отправка подтверждения не удалась; статусы ведёт план 07-08.

## Known Stubs

Нет. `commands::handle` знает пока `/start` и `/help` (остальные команды планируют 07-04, 07-08, 07-09), `notify::tick` пока только отдаёт `done` доставке; это границы плана, а не заглушки данных.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-04 (`stranger_document_is_never_fetched`: `get_file_calls() == 0`), T-07-05 (путь `getFile` не пишется нигде), T-07-06 (префикс и компоненты; остальное в 07-06), T-07-07 (`file_uri` из числового id, модульный тест), T-07-08 (`sending` до вызова, транзакция `tg_inbox` и смещения, тесты Task 2), T-07-SC (новых пакетов нет, `deps` проходит).

## Self-Check: PASSED

- FOUND: crates/server/src/tg/mod.rs (`pub trait Tg`), crates/server/src/tg/fake.rs, crates/server/src/bot/{mod,store,dispatch,commands,ingest,notify,deliver}.rs, crates/server/tests/bot_flow.rs
- FOUND: `pub fn file_uri(` в deliver.rs, `INSERT OR IGNORE INTO tg_inbox` в `pub fn accept(`, `tg_inbox` / `tg_jobs` / `tg_files` / `removed_at` в db.rs
- FOUND: коммиты faf6de5 и c932b21 в истории main
