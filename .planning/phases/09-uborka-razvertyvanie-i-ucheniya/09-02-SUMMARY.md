---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 02
subsystem: server-cleanup
tags: [server, cleanup, retention, sqlite, telegram-bot-api, rust]

requires:
  - phase: 05-yadro-servera-i-protokol
    provides: "Core с внедрёнными часами, Db.write/read, Blobs::lock_upload, строки files с колонкой deleted_at"
  - phase: 07-telegram-bot
    provides: "tg_jobs.delivery и result_file_id, /retry, /resend, find_duplicate_for"
  - phase: 09-uborka-razvertyvanie-i-ucheniya
    provides: "План 09-01: claim_report без WR-02, CI с охраной зависимостей"
provides:
  - "Core::clean и модуль clean.rs: Retention, CleanMode, Cleaned (is_empty, summary) — сроки хранения по состояниям задач и доставки"
  - "spawn_cleaner и Running.cleaner: поток уборки в процессе serve, останавливается вместе с сервером"
  - "Ключи retention_*_hours и clean_interval_secs (файл, CS2_SERVER_RETENTION_*, validate), Config::retention()"
  - "Условие живого ролика в find_duplicate_impl и ответ /resend про срок хранения"
  - "TestOptions.clean_interval_secs и tests/cleanup.rs (13 тестов на виртуальных часах)"
affects: [09-05 подкоманда clean и config show, 09-08 стенд и сроки хранения, 09-10 учение 08-day-soak]

actuals:
  tokens: 17400
  tasks: 3
  commits: 5
plan_head_before: afda745688414152eccef995e8de90389c358aec
plan_head_after: dd195e89a1bc5e46100fe46f49631211fe191811

tech-stack:
  added: []
  patterns:
    - "Двухшаговое удаление: UPDATE files SET deleted_at ... RETURNING в одной транзакции, файлы стираются после коммита"
    - "Уборщик идёт по диску и базе только с внедрёнными часами; mtime файлов в тестах задаётся File::set_modified относительно ManualClock"
    - "Журнал уборки состоит из Cleaned::summary(): числа без путей и имён папок bot-api"

key-files:
  created:
    - crates/server/src/clean.rs
    - crates/server/tests/cleanup.rs
  modified:
    - crates/server/src/lib.rs
    - crates/server/src/config.rs
    - crates/server/src/core.rs
    - crates/server/src/bot/deliver.rs
    - crates/server/cs2-server.example.json
    - crates/server/tests/common/mod.rs

key-decisions:
  - "Сроки по умолчанию 24/72/24/168/6/6 ч и 600 с взяты из RESEARCH A1 как есть; автор подтверждает или меняет их в Task 1 плана 09-08 до первого живого запуска"
  - "Кусок .part окончательной задачи или чужой эпохи удаляется сразу (под lock_upload), кусок текущей эпохи живой задачи — только старше retention_part_hours"
  - "Остаток (папка incoming/<id> или results/<id>) определяется возрастом самого свежего файла внутри; пустая папка — своим mtime"
  - "Файл bot-api отбирается по symlink_metadata().is_file(), родителю documents/videos и глубине не больше трёх; canonicalize + starts_with корня — второй рубеж"
  - "Готовая задача без живого ролика и без result_file_id дублем не считается (иначе тупик «уже готов» против «не найден»)"

patterns-established:
  - "Правила отбора файлов — один SQL-фрагмент EXPIRED_FILES для UPDATE ... RETURNING и для DryRun SELECT"
  - "Тест уборки двигает ManualClock и ставит mtime файлов явно: настоящие mtime старше виртуальных часов стенда"

requirements-completed: [OPS-01]

coverage:
  - id: D1
    description: "Сервер сам раз в clean_interval_secs удаляет демки done/cancelled через 24 ч и failed через 72 ч; поток останавливается с сервером"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#done_job_demo_is_removed_by_the_cleaner_thread_after_its_retention"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#failed_demo_waits_longer_and_then_retry_says_it_is_gone"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#queued_job_keeps_its_demo_however_old_it_is"
        status: pass
      - kind: integration
        ref: "crates/server/tests/bot_e2e.rs (остановка сервера с потоком уборки)"
        status: pass
    human_judgment: false
  - id: D2
    description: "Ролик удаляется только когда доставка закончена или не будет: sent+file_id 24 ч, без Telegram/failed/sent без file_id 168 ч, при none/pending/sending/uncertain никогда"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#result_is_kept_while_delivery_is_not_finished"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#delivered_result_with_file_id_goes_after_a_day_and_is_still_a_duplicate"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#result_without_telegram_or_with_failed_delivery_goes_after_a_week"
        status: pass
    human_judgment: false
  - id: D3
    description: "Недозагруженные .part, остатки прерванной уборки и осиротевшие папки уходят по возрасту, идущая загрузка и активные задачи остаются; два прохода не конфликтуют"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#stale_and_foreign_part_files_go_but_an_active_upload_stays"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#interrupted_clean_is_finished_by_the_next_pass"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#orphan_folders_go_only_when_old_and_never_for_active_jobs"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#dry_run_reports_what_would_go_and_deletes_nothing"
        status: pass
    human_judgment: false
  - id: D4
    description: "Рабочая папка telegram-bot-api чистится в границах корня (documents/videos, глубина до трёх, не ссылки, не служебные файлы); журнал без путей и токена"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#bot_api_files_go_only_inside_documents_and_only_when_old"
        status: pass
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#cleaner_log_has_counts_but_no_paths_or_token"
        status: pass
    human_judgment: false
  - id: D5
    description: "После уборки друг не упирается в тупик: /retry и /resend отвечают понятно, повторно присланная демка ставит новую задачу"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/server/tests/cleanup.rs#cleaned_result_is_not_a_duplicate_and_resend_explains_it"
        status: pass
    human_judgment: false
  - id: D6
    description: "Сроки — ключи настроек с умолчаниями, окружением и проверкой; образец настроек знает все ключи"
    requirement: OPS-01
    verification:
      - kind: unit
        ref: "crates/server/src/config.rs#retention_keys_have_defaults_and_are_checked"
        status: pass
      - kind: integration
        ref: "crates/server/tests/operator.rs#example_config_is_accepted"
        status: pass
    human_judgment: false
  - id: D7
    description: "Значения сроков 24/72/24/168/6/6 ч подходят автору по существу: они удаляют демки, ролики и файлы друзей"
    requirement: OPS-01
    verification: []
    human_judgment: true
    rationale: "Уборщик проверен только на виртуальных часах; подтверждение сроков автором — Task 1 плана 09-08 (checkpoint:decision стенда) до первого живого запуска"

duration: 35min
completed: 2026-10-12
status: complete
---

# Phase 9 Plan 02: Уборщик сервера Summary

**Поток уборки в процессе `serve` с двухшаговым удалением через `files.deleted_at`: сроки хранения демок, роликов, `.part`, остатков и файлов `telegram-bot-api` по состояниям задач и доставки, а бот после уборки отвечает другу понятно вместо тупика**

## Performance

- **Duration:** около 35 мин
- **Started:** 2026-10-12T18:20Z (оценка)
- **Completed:** 2026-10-12T18:57Z
- **Tasks:** 3 (трассер и два TDD-задания)
- **Files modified:** 8 (2 созданы)

## Accomplishments

- Модуль `clean.rs`: `Retention` (умолчания 24/72/24/168/6/6 ч), `CleanMode` (`Apply`/`DryRun`), `Cleaned` с `summary()` для журнала, `Core::clean` с проходами «пометка → удаление файлов → куски → остатки → bot-api». Пометка одной транзакцией `UPDATE files SET deleted_at … RETURNING`, файлы стираются после коммита, `NotFound` не ошибка, поэтому два уборщика (поток `serve` и будущий `cs2-server clean`) не мешают друг другу.
- Ролик не удаляется, пока доставка в `none`, `pending`, `sending`, `uncertain`; в остальных случаях срок считается от `tg_jobs.updated_at` (доставлен, есть `file_id`) или от `finished_at` (без Telegram, `failed`, `sent` без `file_id`).
- `spawn_cleaner` и `Running.cleaner`: поток ждёт шагами по 200 мс, останавливается с сервером; корень bot-api передаётся только когда бот запущен.
- Рабочая папка bot-api чистится только обычными файлами `documents`/`videos` на глубине до трёх уровней, после `canonicalize` и `starts_with` корня; журнал уборки состоит из чисел.
- Тупик после уборки закрыт: готовая задача без живого ролика и без `result_file_id` не считается дублем; `/resend` отвечает «Ролик задачи #N уже удалён с сервера по сроку хранения. Пришлите демку ещё раз — сделаю заново.»; `/retry` без демки отвечает уже существовавшим «демки задачи #N больше нет — пришлите файл заново».
- Семь ключей настроек во всех местах маршрута (структура, умолчания, файл, слияние, окружение, `validate`), образец `cs2-server.example.json` обновлён.

## Task Commits

1. **Task 1: трассер — демка готовой задачи исчезает по сроку потоком уборки** — `acaade5` (feat)
2. **Task 2 RED: правила по состояниям, куски, остатки, ответы бота** — `175886a` (test)
3. **Task 2 GREEN: ролики, куски, остатки, дубль, /resend** — `d3e4740` (feat)
4. **Task 3 RED: bot-api и журнал без путей и токена** — `a80ef9b` (test)
5. **Task 3 GREEN: папка bot-api, образец настроек** — `dd195e8` (feat)

**Plan metadata:** коммит `docs(09-02)` с этим SUMMARY, STATE.md и ROADMAP.md.

## TDD Gate Compliance

- **RED Task 2** (`175886a`): из девяти тестов семь упали на утверждениях о новом поведении (`results` пуст вместо `[id]`, `parts`/`leftovers` равны 0, `find_duplicate` находит дубль, нет ответа про срок хранения). Два теста прошли сразу и охраняют уже реализованное: `failed_demo_waits_longer_and_then_retry_says_it_is_gone` (правило демки упавшей задачи заложено в SQL трассера, сообщение `больше нет` существовало в `submit_from`) и `result_is_kept_while_delivery_is_not_finished` (ролики до Task 2 не отбирались вовсе). Оценка: красные тесты упали по заданной причине (assertion на планируемое значение), не из-за ошибки компиляции или фикстуры.
- **RED Task 3** (`a80ef9b`): оба теста упали: `botapi_files == 0` вместо 2, файл bot-api остался, итога в журнале нет.
- **GREEN** (`d3e4740`, `dd195e8`): все 13 тестов `cleanup.rs` проходят. REFACTOR-коммитов нет. Классификатор `check tdd-red-evidence` не запускался: отчёт `cargo test` не входит в поддерживаемые форматы, а `tdd_mode=false`.

## Files Created/Modified

- `crates/server/src/clean.rs` — правила уборки, двухшаговое удаление, куски, остатки, папка bot-api
- `crates/server/src/lib.rs` — `pub mod clean;`, `spawn_cleaner`, `Running.cleaner`, остановка после `sweeper`
- `crates/server/src/config.rs` — семь ключей и `Config::retention()`, `validate`, тест `retention_keys_have_defaults_and_are_checked`
- `crates/server/src/core.rs` — условие «живого ролика» в `find_duplicate_impl`
- `crates/server/src/bot/deliver.rs` — ответ `/resend` о сроке хранения
- `crates/server/cs2-server.example.json` — ключи сроков
- `crates/server/tests/common/mod.rs` — `TestOptions.clean_interval_secs`
- `crates/server/tests/cleanup.rs` — 13 тестов уборщика

## Decisions Made

- Сроки по умолчанию взяты из RESEARCH A1 без изменений; их подтверждение автором остаётся за Task 1 плана 09-08 (точка решения до развёртывания), значения меняются в файле настроек без пересборки.
- `.part` окончательной задачи или чужой эпохи удаляется сразу, под охраной `lock_upload`; кусок текущей эпохи живой задачи — только по возрасту. Так идущая загрузка не теряет файл, а брошенное не копится.
- Возраст остатка — возраст самого свежего файла внутри папки (пустая папка — собственный mtime): на Windows mtime каталога через `std` не поставить, и он меняется от любых операций внутри.
- Журнал уборки пишет только `Cleaned::summary()`, ошибки файлов — счётчик `errors` без текста: имя рабочей папки bot-api содержит токен бота.
- В тесте `cleaned_result_is_not_a_duplicate_and_resend_explains_it` вместо бота в потоках `serve_with_bot` взят `BotStand` с ручными шагами `step()`: он детерминированнее, а поток уборки покрыт отдельными тестами с `serve_with_bot` (`done_job_demo_is_removed_by_the_cleaner_thread…`, `cleaner_log_has_counts_but_no_paths_or_token`).

## Deviations from Plan

None - plan executed exactly as written.

Единственное отличие от формулировки плана вынесено в «Decisions Made»: `/resend`-тест идёт на `BotStand`, а не на потоках бота (поведение то же, проверка детерминированнее).

## Issues Encountered

- Первый запуск трассера упал на «Wrong number of parameters passed to query. Got 4, needed 3»: SQL трассера использовал три параметра из пяти. Исправлено передачей трёх параметров в Task 1 и всех пяти после добавления правил роликов в Task 2.
- Блок `#[cfg(unix)]` теста с символической ссылкой (`bot_api_files_go_only_inside_documents_and_only_when_old`) на Windows не компилируется и не запускается; его проверит CI на ubuntu-latest. Остальные утверждения теста выполнены на Windows.

## Known Stubs

None. Заглушек, TODO и пустых значений, текущих в интерфейс, в созданных файлах нет.

## Threat Flags

Нет нового сетевого или авторизационного периметра. Удаление файлов процессом сервера описано моделью угроз плана (T-09-04…T-09-09), все шесть закрыты тестами:

| Угроза | Тест |
|--------|------|
| T-09-04 ролик с незаконченной доставкой | `result_is_kept_while_delivery_is_not_finished` |
| T-09-05 удаление вне корня bot-api | `bot_api_files_go_only_inside_documents_and_only_when_old` (ссылка — под `cfg(unix)`) |
| T-09-06 токен в журнале | `cleaner_log_has_counts_but_no_paths_or_token` |
| T-09-07 `td.binlog` и базы bot-api | `bot_api_files_go_only_inside_documents_and_only_when_old` |
| T-09-08 `.part` идущей загрузки | `stale_and_foreign_part_files_go_but_an_active_upload_stays` |
| T-09-09 тупик друга после уборки | `cleaned_result_is_not_a_duplicate_and_resend_explains_it` |

## User Setup Required

None - no external service configuration required. Подтверждение сроков хранения автором запланировано в плане 09-08 (раздел «Сроки хранения» документа `09-STAND.md`).

## Next Phase Readiness

- Контракт `Core::clean(&Retention, Option<&Path>, CleanMode)` готов для подкоманды `cs2-server clean [--dry-run]` и вывода сроков в `config show` (план 09-05).
- Флаг-допущение не проверено: задача `done` с `delivery = 'pending'` навсегда держит ролик бессрочно; покажет учение 08-day-soak.
- Не сделано по плану (вне OPS-01): проверка свободного места до `getFile`; замена — наблюдение `df -h` в документе развёртывания.

## Self-Check: PASSED

- Файлы на диске: `crates/server/src/clean.rs`, `crates/server/tests/cleanup.rs`, `crates/server/cs2-server.example.json`.
- Коммиты в ветке: `acaade5`, `175886a`, `d3e4740`, `a80ef9b`, `dd195e8`; `commits: 5` измерено `git rev-list --count` от `plan_head_before`.
- `cargo test --release -p server` зелёный целиком (65 модульных и все интеграционные, 13 в `cleanup`), `--test deps` и `cargo test --release -p cli --test pipeline` зелёные; `Cargo.toml` сервера не менялся.
- `crates/runner`, `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `crates/store/src/journal.rs` не затронуты.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-12*
