---
phase: 07-telegram-bot-i-dostavka
plan: 06
subsystem: server
tags: [telegram, bot, ingest, limits, dedup, path-safety]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 02
    provides: каркас опроса precheck/accept/ingest_once, tg_inbox, tg_files, FakeTelegram, BotStand
  - phase: 07-telegram-bot-i-dostavka
    plan: 03
    provides: Core::find_duplicate, submit_from, jobs_ahead, player_key
  - phase: 07-telegram-bot-i-dostavka
    plan: 04
    provides: whitelist, /player
provides:
  - "ingest::admit: лимиты D-03 (друг 2, админ 5, очередь 10) с учётом строк tg_inbox pending/downloading, SQL на переданной tx"
  - "precheck: размер (обязателен, не больше max_demo_bytes) и имя (.dem/.dem.zst без регистра) до getFile; слой 1 дублей по tg_files(file_unique_id)"
  - "ingest_once: восстановление после падения, submit_from для известного файла без getFile, слой 2 дублей по SHA-256, безопасный путь из getFile, удаление оригинала"
  - "ack_text/queue_row: подтверждение называет число задач впереди и правится на «Задача #N в очереди (игрок X), перед вами K»"
  - "FakeTelegram::set_file_path для тестов путей"
affects: [07-07, 07-08, 07-09, 07-10, 07-11]

actuals:
  tokens: 16000
  tasks: 3
  commits: 2
plan_head_before: 0709e5ab600f0063783251804555c7e9122fbdf1
plan_head_after: 261a59aeea17a0c49d7bcda464c354dded3e7a5c
commits: 2

tech-stack:
  added: []
  patterns:
    - "лимит и вставка строки tg_inbox в одной Db::write: пять файлов подряд дают две задачи и три отказа"
    - "повтор в два слоя: file_unique_id до getFile, SHA-256 после копирования; лишняя копия отменяется Core::cancel"
    - "путь от чужого процесса: компоненты, symlink_metadata, сверка размера, canonicalize внутри корня"

key-files:
  created:
    - crates/server/tests/bot_ingest.rs
  modified:
    - crates/server/src/bot/ingest.rs
    - crates/server/src/tg/fake.rs

key-decisions:
  - "Ключ дубля — (sha256, player_key); активная задача отвечает «уже в очереди: задача #N», готовая — «уже готово: /resend N», упавшая и отменённая дублем не считаются"
  - "Восстановление (Pitfall 3) делается ДО getFile: для строки без своей задачи ищется неприкреплённая задача того же пользователя, игрока, имени и размера (нет ни tg_jobs, ни tg_inbox.job_id); так не нужен второй getFile, оригинал после первой копии уже удалён"
  - "Повтор уже принятого сообщения (потерянное смещение) не получает ответа про дубль: precheck пропускает слой 1, accept молча возвращает Already"
  - "Для слоя 2 свой запрос вместо find_duplicate: find_duplicate возвращает самую свежую задачу, то есть только что созданную копию, а нужна другая"

requirements-completed: []
---

# Phase 7 Plan 06: защита приёма демки Summary

Приём демки файлом закрыт лимитами D-03, проверкой размера и имени до `getFile`, двухслойным узнаванием повторов, второй задачей для другого игрока без второго скачивания и проверкой пути из `getFile`; подтверждение сразу называет число задач впереди.

## Accomplishments

- **Лимиты.** `admit(bot, tx, user)` считает активные задачи пользователя и всей очереди плюс строки `tg_inbox` в `pending`/`downloading`; зовётся первой в `accept`, то есть в той же `Db::write`, что вставка строки и смещение. Повтор уже принятого сообщения проверяется раньше лимитов и молча пропускается. Тексты склоняют «активная задача / активные задачи / активных задач».
- **До `getFile`.** `precheck`: нет `file_size` — «пришлите демку файлом», больше `max_demo_bytes` — отказ с лимитом в МиБ, имя не `.dem`/`.dem.zst` — отказ; строк `tg_inbox` и вызовов `getFile` нет.
- **После скачивания.** Магию проверяет `Core::submit` (`sniff_demo`): мусор с именем `.dem` даёт «это не демка CS2…», строка `rejected`, задачи нет, оригинал в папке bot-api удаляется.
- **Повторы.** Слой 1: `tg_files(file_unique_id)` + `Core::find_duplicate` в `precheck` (ответ без строки); в `ingest_once` известный файл берётся `Core::submit_from` без `getFile` (другой игрок), с повторной защитой `find_duplicate` на случай двух строк подряд. Слой 2: после `Core::submit` ищется другая активная или готовая задача того же хеша и игрока; найдена — новая копия отменяется `Core::cancel(id, true)`, строка `rejected` («дубль задачи #N»), `file_unique_id` запоминается за найденной задачей, ответ правит сообщение статуса.
- **Путь.** `safe_local_path`: абсолютный, внутри `files_root` по компонентам, без `..`, после замены корня на `files_root_local` — `symlink_metadata` (обычный файл, не ссылка), размер равен заявленному, `canonicalize` внутри корня. Отказ — класс ошибки без пути (в журнал, чат и `reason` пути нет). После успешной постановки оригинал удаляется, ошибка удаления идёт в журнал видом ошибки.
- **Подтверждение.** До скачивания: «Принял «имя» для игрока X. Скачиваю… (перед вами K)» (K = активные задачи + более ранние строки `pending`/`downloading`); после постановки: «Задача #N в очереди (игрок X), перед вами K» (`jobs_ahead`); пустая очередь — «очередь пуста».
- **Восстановление.** `own_job` привязывает строку к уже существующей задаче, а не создаёт вторую.

## Tests

`crates/server/tests/bot_ingest.rs` (16): `same_demo_for_another_player_becomes_a_second_job_without_a_second_download`, `done_duplicate_points_to_resend`, `failed_duplicate_is_not_a_duplicate`, `same_bytes_from_another_upload_are_found_by_hash`, `oversized_or_sizeless_or_non_demo_name_is_refused_before_get_file`, `garbage_bytes_with_dem_name_are_refused_after_download_without_a_job`, `friend_limit_counts_pending_downloads`, `admin_limit_is_five_and_queue_limit_is_ten`, `ack_shows_the_place_in_queue_before_the_download`, `crash_after_submit_relinks_to_the_existing_job`, `file_outside_the_bot_api_root_is_refused`, `dot_dot_path_is_refused`, `symlinked_file_is_refused`, `size_mismatch_is_refused`, `original_is_removed_after_copy`, `ingest_errors_name_no_paths`.

Зелёные: `cargo test --release -p server` (все наборы), `cargo test --release -p cli --test pipeline`. `cargo clippy -p server --all-targets` в изменённых файлах предупреждений не даёт.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - регресс] Ответ про дубль на повтор того же сообщения**
- **Found during:** полный прогон `cargo test -p server`
- **Issue:** `repeated_update_creates_one_job` (07-02) упал: повтор уже принятого обновления теперь узнавался слоем 1 в `precheck` и получал второе сообщение «уже в очереди».
- **Fix:** `precheck` проверяет, есть ли уже строка `tg_inbox` с этой парой `chat_id/message_id`, и тогда слой 1 пропускает; `accept` молча отдаёт `Already`.
- **Files modified:** crates/server/src/bot/ingest.rs
- **Commit:** 87c1b42

**2. Восстановление до `getFile`, а не после `Core::submit`.** План описывал поиск «своей» задачи после повторного `Core::submit` по хешу с формулировкой «созданная раньше этой строки», что для задачи упавшей попытки невозможно (она создана позже строки). Реализовано до `getFile`: условия — тот же пользователь, игрок, имя и размер, `created_at >= row.created_at`, задачу не держит ни `tg_jobs`, ни `tg_inbox.job_id`. Это же избавляет от второго скачивания, когда оригинал уже удалён. Своя проверка по хешу после `submit` не нужна и не добавлялась.

**3. Тексты подтверждения.** План даёт «Перед вами в очереди: K», а тест требует «перед вами 3»; принят вид «(перед вами K)» в обоих сообщениях.

**4. Мелочи.** `FakeTelegram::set_file_path` (пути в тестах); одна фиксация кода вместо трёх: Task 1-3 правят один файл `ingest.rs`, поэтому коммиты — реализация целиком и тесты целиком, а не по заданиям; все тесты писались по `<behavior>` и прошли на готовой реализации без отдельной красной фазы.

## Known Limitations

- **`symlinked_file_is_refused` на Windows пропускается** (создание ссылки без прав недоступно: тест пишет «пропуск» и выходит). Логику ссылок покрывает `symlink_metadata` в `safe_local_path`; на Unix тест выполняется по-настоящему.
- **Окно 07-02 между `Core::submit` и `mark_queued`** остаётся, но теперь безвредно: повторный `ingest_once` привязывается к созданной задаче (`own_job`) вместо второй. Если оригинал при этом ещё не удалён, он удаляется только при новом успешном `submit`; в остальных случаях его уберёт уборщик фазы 9.
- **Лишняя копия слоя 2** остаётся на диске отменённой задачей (до 2 ГБ) до уборщика фазы 9; обычный повтор той же пересылки закрыт слоем 1 без скачивания.
- **`find_duplicate` не смотрит на `deleted_at` (из 07-03).** Для готовой задачи с удалённой демкой ответ «уже готово: /resend N» остаётся верным (ролик есть), поэтому поведение сохранено; для активных задач демка есть всегда.
- **Жёсткая ссылка без пересчёта хеша (из 07-03)** не затрагивается: `submit_from` по-прежнему сверяет размер, а не SHA-256.
- Двойной счёт строки `downloading`, чья задача уже создана, на миллисекунды при параллельных потоках опроса и приёма (07-10) даёт лишь осторожный отказ, не пропуск.

## Known Stubs

Нет. `/resend N`, упомянутая в ответе про готовый дубль, появится в плане 07-09.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-23 (`friend_limit_counts_pending_downloads`, `admin_limit_is_five_and_queue_limit_is_ten`, размер и имя до `getFile`), T-07-24 (`garbage_bytes_with_dem_name_are_refused_after_download_without_a_job`), T-07-25 (`file_outside_the_bot_api_root_is_refused`, `dot_dot_path_is_refused`, `size_mismatch_is_refused`; ссылки — на Unix), T-07-26 (`ingest_errors_name_no_paths`), T-07-27 (слой `file_unique_id`, `submit_from`, `original_is_removed_after_copy`), T-07-SC (новых пакетов нет).

## Self-Check: PASSED

- FOUND: `pub fn admit(`, `symlink_metadata`, `remove_file`, `tg_files`, `find_duplicate(`, `submit_from(` в crates/server/src/bot/ingest.rs; сравнение с `max_demo_bytes`
- FOUND: crates/server/tests/bot_ingest.rs, 16 тестов `#[test]`
- FOUND: коммиты 87c1b42 и 261a59a в истории main
