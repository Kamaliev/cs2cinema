---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 02
subsystem: store
tags: [rust, store, sqlite, journal, pipeline, failure-class, acceptance]
status: complete

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "Store: attempt/resume (миграция 4), journal (разделители попыток), FailCode и RecordState"
provides:
  - "store: миграция 5 (jobs.auto_retries, jobs.batch), Job.auto_retries/Job.batch, requeue_auto, set_batch, jobs_in_batch, last_batch, open_read_only, schema_version"
  - "store::journal: StartKind::Auto, StartKind::ALL, attempts(log), outcomes(log)"
  - "pipeline::record::failure: FailClass { Launch, Environment, Other }, classify(code, state)"
affects: [04-09, 04-10, acceptance, queue-policy]

actuals:
  tokens: 9500
  tasks: 2
  commits: 2

plan_head_before: 532c465f1d979d13379bc99e653bcbd29f745023
plan_head_after: b6fc09a702fe2bd1a4d253fdd98ad0002ce0d006
commits: 2

tech-stack:
  added: []
  patterns:
    - "одна таблица StartKind::ALL для писателя (separator) и читателя (attempts)"
    - "исчерпывающий match без ветки `_` для классификации кодов сбоя"
    - "проверка приёмки открывает живую базу только на чтение, без миграций и без смены режима журнала"

key-files:
  created: []
  modified:
    - crates/store/src/lib.rs
    - crates/store/src/journal.rs
    - crates/pipeline/src/record/failure.rs

key-decisions:
  - "DC-4: бюджет авто-повтора — колонка auto_retries; requeue_with (Повторить/Заново) обнуляет её, requeue_auto увеличивает"
  - "DC-7: партия приёмки — колонка batch и отдельный set_batch, NewJob и его литералы не менялись"
  - "DC-3: GAME_EXITED до состояния Take — «запуск», посреди дубля и позже — «прочее»"
  - "open_read_only: SQLITE_OPEN_READ_ONLY по обычному пути хватило, запасной URI mode=ro не понадобился"

requirements-completed: [DIF-03, DIF-04, ACC-02]

duration: 15min
completed: 2026-10-11
---

# Phase 4 Plan 02: Факты для политики очереди и приёмки Summary

**База версии 5 хранит счётчик авто-повторов и партию приёмки, журнал различает «авто-повтор» и читается обратно через `journal::attempts`, `Store::open_read_only` читает живую WAL-базу без записи, а `classify(code, state)` раскладывает каждый `FailCode` по классам «запуск» / «окружение» / «прочее».**

## Что сделано

- **Task 1 (tracer, `db30f81`).** Миграция 5 (`auto_retries INTEGER NOT NULL DEFAULT 0`, `batch TEXT`), поля `Job.auto_retries`/`Job.batch`, `Store::requeue_auto` (resume = 1, `auto_retries + 1`), `requeue_with` обнуляет счётчик, `schema_version()`. В `journal.rs`: `StartKind::Auto` («авто-повтор»), `StartKind::ALL`, `attempts` и `outcomes` разбирают журнал по тем же меткам (`SEPARATOR_MARK`, `SEPARATOR_END`, `OUTCOME_MARK`), что пишут `separator` и `outcome_line`. Старые проверки версии `== 4` заменены на `schema_version()`.
- **Task 2 (`b6fc09a`).** `Store::open_read_only`: `OpenFlags::SQLITE_OPEN_READ_ONLY`, отказ на отсутствующем файле (файл не создаётся) и на базе старше сборки («запустите приложение этой сборки»), без смены режима журнала и без миграций. `set_batch`, `jobs_in_batch` (по возрастанию id), `last_batch`. В `failure.rs`: `FailClass` с `title()` и `classify` — исчерпывающий `match` без `_`.

## Проверка

- `cargo test --release -p store` — 26 passed (среди них `version_four_database_gains_auto_retries_and_batch`, `requeue_auto_counts_and_a_manual_requeue_resets`, `auto_retry_attempt_is_visible_in_the_journal`, `attempts_reads_back_every_separator_kind`, `outcomes_read_back_what_outcome_line_wrote`, `read_only_store_reads_a_batch_while_the_writer_is_open`, `read_only_store_refuses_an_old_database`, `read_only_store_does_not_create_a_missing_file`).
- `cargo test --release -p pipeline --lib failure` — 13 passed (`launch_class_covers_start_codes_and_an_early_game_exit`, `game_exit_mid_take_is_not_retried`, `environment_class_holds_deterministic_codes`, `everything_else_is_other`).
- `cargo build --release --workspace` — без ошибок; `cargo test --release -p cli --test pipeline` — 2 passed; `cargo test --release -p app --bin cs2cinema` — 42 passed.
- Критерии по grep: в теле `open_read_only` слов `migrate`/`journal_mode` нет (0); `user_version(&s), 4)` в `store/src/lib.rs` — 0.
- Трейсер-ворота: `<verify>` Task 1 зелёная до расширения (Task 2).

## Deviations from Plan

None - plan executed exactly as written.

Уточнение без отклонения от замысла: помимо тестов из плана добавлен `last_batch_is_none_without_batches`.

## Known Stubs

None.

## Threat Flags

None — новой поверхности сверх `<threat_model>`. T-04-03 закрыта: только `SQLITE_OPEN_READ_ONLY`, версия проверяется до запросов, запись через читателя возвращает ошибку (тест). T-04-04: `auto_retries` лежит в базе и переживает перезапуск; политика бюджета — в 04-09. T-04-05: `attempts` принимает способ запуска только из `StartKind::ALL`.

## Заметки

- Незакоммиченными остались файлы, не относящиеся к плану: `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `03-PATTERNS.md`, `04-PATTERNS.md`.
- Ветка `main` (последовательный режим по указанию оркестратора), защита default-ветки при коммитах задач не срабатывала.

## Self-Check: PASSED

- FOUND: crates/store/src/lib.rs, crates/store/src/journal.rs, crates/pipeline/src/record/failure.rs
- FOUND commits: db30f81, b6fc09a
