---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 02
subsystem: store
tags: [rust, sqlite, rusqlite, migration, job-status]

requires:
  - phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
    provides: миграции MIGRATIONS 1-2, колонки fail_* и phase на задаче
provides:
  - "Схема версии 3: jobs.precheck (JSON отчёта предпроверки) и таблица last_good (одна строка: сборка CS2 / версия HLAE)"
  - "Job.precheck, Store::set_precheck, LastGood, Store::last_good, Store::set_last_good"
  - "JobStatus::Cancelling («отменяется»), Store::set_cancelling (только running -> cancelling)"
  - "Store::interrupted_jobs и reset_running: running -> failed, cancelling -> cancelled"
  - "Ветки JobStatus::Cancelling в карточке задачи"
affects: [02-03, 02-05, 02-06, 02-07, предпроверка, отмена, уборка после падения]

actuals:
  tokens: 5000
  tasks: 2
  commits: 2

plan_head_before: a64d6eb319bf106ea9bb51c3fdbf2d0ac1cdcd76
plan_head_after: 5e3bea72b4c2b78e2888c0091a88d962d08461dd
commits: 2

tech-stack:
  added: []
  patterns:
    - "миграция только добавлением колонок или таблиц, шаг в одной транзакции с PRAGMA user_version"
    - "store хранит отчёт предпроверки строкой JSON и не разбирает его"
    - "смена статуса с условием в WHERE (status = 'running'), результат — bool изменения"

key-files:
  created: []
  modified:
    - crates/store/src/lib.rs
    - crates/app/src/jobs_page.rs

key-decisions:
  - "set_cancelling возвращает bool и меняет только running: гонка с завершением задачи не превращает итог обратно в «отменяется» (T-02-07)"
  - "reset_running делает оба перехода в одной транзакции и возвращает общее число строк"
  - "interrupted_jobs читает статус через JobStatus::parse и отдаёт out_dir, чтобы приложение убрало файлы игры до reset_running"

patterns-established:
  - "Новые статусы задачи добавляются и в JobStatus::title/parse, и в Job::is_active, и во все match карточки"

requirements-completed: []

duration: 8 min
completed: 2026-10-10
status: complete
---

# Phase 2 Plan 02: База версии 3 и статус «отменяется» Summary

**База поднимается до версии 3 одной транзакцией (отчёт предпроверки на задаче, пара «сборка CS2 / версия HLAE» последней удачной записи), появился статус «отменяется», переживающий перезапуск, и запрос прерванных падением задач для уборки.**

## Performance

- **Duration:** около 8 мин
- **Tasks:** 2 (1 tracer + 1 auto)
- **Files modified:** 2

## Accomplishments

- `MIGRATIONS[2]`: `CREATE TABLE IF NOT EXISTS last_good (id INTEGER PRIMARY KEY CHECK (id = 1), …)` и `ALTER TABLE jobs ADD COLUMN precheck TEXT`; прежние шаги не тронуты, старые задачи читаются без потерь, повторное открытие ничего не меняет.
- `Job.precheck`, `set_precheck`; `start_job` и `requeue` сбрасывают отчёт предыдущего запуска. `LastGood`, `last_good()`, `set_last_good()` (`INSERT OR REPLACE` в строку с `id = 1`).
- `JobStatus::Cancelling`: «отменяется», `parse("cancelling")`, входит в `Job::is_active`; `next_queued` и `running` его не берут.
- `set_cancelling` (только из `running`), `interrupted_jobs` (running и cancelling вместе с `out_dir`), `reset_running`: «выполняется» -> «ошибка» с прежним текстом, «отменяется» -> «отменена» с очисткой `stage` и `phase`.
- Карточка задачи: спиннер и значок «отменяется», неактивная кнопка «Отменяется…», удаление запрещено у `Running` и `Cancelling`, лестница и значок превью как у `Running`.

## Task Commits

1. **Task 1: схема версии 3** - `909aea4` (feat)
2. **Task 2: статус «отменяется» и прерванные задачи** - `5e3bea7` (feat)

## Verification

- `cargo test --release -p store`: 10 passed, среди них `version_two_database_gains_precheck_and_last_good`, `precheck_report_is_cleared_on_start_and_requeue`, `cancelling_is_set_only_on_a_running_job_and_survives_reopen`, `crash_recovery_turns_cancelling_into_cancelled_and_running_into_failed`; `crash_recovery_and_caches` зелёный.
- `cargo build --release -p app` без ошибок; `cargo test --release -p app`: 8 passed; `cargo test --release -p cli --test pipeline`: 2 passed.
- `grep -c "ALTER TABLE jobs ADD COLUMN" crates/store/src/lib.rs` = 7.

## Deviations from Plan

None - plan executed exactly as written.

`requirements-completed` оставлено пустым намеренно: план затрагивает PRE-01, DIF-01, TMO-03, TMO-06, но поведение (предпроверка, сравнение сборок, отмена, уборка при старте) появится в 02-05, 02-06, 02-07; здесь только хранилище. Отметка требований не ставилась.

## Known Stubs

Нет. Новые методы store (`set_precheck`, `last_good`, `set_last_good`, `set_cancelling`, `interrupted_jobs`) пока не вызываются из app — их подключают 02-05, 02-06 и 02-07; это ожидаемо по плану.

## Threat Flags

Нет новых поверхностей сверх `threat_model` плана: миграция в одной транзакции (T-02-06), `set_cancelling` с условием `status = 'running'` (T-02-07), все значения через `params!` (T-02-08).

## Next Phase Readiness

Предпроверка (02-05/02-06) может писать отчёт в `jobs.precheck` и читать `last_good`; отмена (02-07) ставит `set_cancelling` синхронно; старт приложения находит прерванные задачи через `interrupted_jobs` и затем зовёт `reset_running`. Исполнитель в `jobs.rs` пока не ставит `Cancelling` и не знает про него: `cancel_job` по-прежнему ведёт задачу прямо в `cancelled`.

## Self-Check: PASSED

- FOUND: crates/store/src/lib.rs, crates/app/src/jobs_page.rs
- FOUND: 909aea4, 5e3bea7 (предки HEAD)
