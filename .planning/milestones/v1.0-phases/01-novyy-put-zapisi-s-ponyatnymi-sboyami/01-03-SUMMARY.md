---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 03
subsystem: store
tags: [rust, sqlite, rusqlite, migration, user_version, job-failure]

requires:
  - phase: 01-01
    provides: "типизированные коды сбоев записи (строки code/state/hint/tail передаются в store без зависимости от pipeline)"
provides:
  - "MIGRATIONS: &[&str] и Store::migrate: шаги схемы в транзакции вместе с PRAGMA user_version (версия 2)"
  - "store::JobFailure { message, code, state, hint, tail, elapsed_ms } и Store::fail_job(id, &JobFailure)"
  - "Job.fail_code / fail_state / fail_hint / fail_tail / fail_elapsed_ms (Option) и Job.phase (String)"
  - "Store::set_out_dir(id, &Path) и Store::set_phase(id, &str)"
affects: [01-04, 01-07]

actuals:
  tokens: 8000
  tasks: 2
  commits: 2
plan_head_before: 44645affe60e7eaf6dc947bf7fc6fb8983b20342
plan_head_after: 65a27be29fddb5d761c12229a663986be80276dd

tech-stack:
  added: []
  patterns:
    - "миграция списком шагов: элемент k поднимает схему до версии k+1, шаг и user_version в одной транзакции"
    - "SCHEMA не меняется: колонки сбоя добавляет только миграция, поэтому у свежей и старой базы один путь"
    - "store получает сбой строками (JobFailure), типы pipeline в него не протекают"

key-files:
  created: []
  modified:
    - crates/store/src/lib.rs

key-decisions:
  - "Колонки: six-columns (выбор пользователя на checkpoint Task 2): четыре колонки D-05 плюс fail_elapsed_ms и phase вторым шагом миграции"
  - "fail_job и reset_running тоже сбрасывают phase: после сбоя выполняемого состояния нет, а состояние сбоя лежит в fail_state"
  - "cargo fmt не запускался: store отформатирован широкой строкой, fmt по умолчанию переписал бы около 400 строк чужого кода"

patterns-established:
  - "Новая версия схемы: только новым шагом в конец MIGRATIONS и только ADD COLUMN"

requirements-completed: [LCH-02]

coverage:
  - id: D1
    description: "Упавшая задача хранит типизированный сбой (error, fail_code, fail_state, fail_hint, fail_tail, fail_elapsed_ms); повтор (requeue, start_job) очищает его"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#typed_failure_round_trips_and_retry_clears_it"
        status: pass
    human_judgment: false
  - id: D2
    description: "База старой схемы (user_version 0) и база версии 1 мигрируют до версии 2 без потери задач, повторное открытие ничего не меняет"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#old_database_gains_failure_columns"
        status: pass
      - kind: unit
        ref: "crates/store/src/lib.rs#version_one_database_gains_elapsed_and_phase"
        status: pass
    human_judgment: true
    rationale: "Проверено на синтетических базах во временных файлах; живая база пользователя в %LOCALAPPDATA% не открывалась. Миграция необратима без миграции вниз."
  - id: D3
    description: "Папка задачи (set_out_dir) и состояние записи (set_phase) сохраняются, пока задача выполняется; stage не меняется; завершение, отмена, ошибка и повтор сбрасывают phase"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#out_dir_and_phase_are_kept_while_running"
        status: pass
    human_judgment: false
---

# Phase 1 Plan 03: Типизированный сбой записи в базе задач Summary

**SQLite хранит типизированный сбой записи (`fail_code/state/hint/tail/elapsed_ms`) и состояние записи (`phase`) двумя транзакционными шагами миграции по `PRAGMA user_version`; старые базы поднимаются до версии 2 без потери задач.**

Решение по колонкам: six-columns

## Performance

- **Duration:** около 12 min
- **Tasks:** 3 (1 tracer, 1 checkpoint:decision, 1 auto/tdd), исполнено 2, решение Task 2 дано пользователем заранее
- **Files modified:** 1

## Accomplishments

- `MIGRATIONS` и `Store::migrate`: каждый шаг — `unchecked_transaction()` с `execute_batch` шага и `PRAGMA user_version = k+1`; ошибка шага откатывается `Drop`-ом транзакции, `open` возвращает `Err`. Шаг 1 — четыре колонки D-05, шаг 2 — `fail_elapsed_ms INTEGER` и `phase TEXT NOT NULL DEFAULT ''`. `SCHEMA` не тронут.
- `JobFailure` и `Store::fail_job`: пишет `status = 'failed'`, `error`, четыре `fail_*` и `fail_elapsed_ms` через `params!`; `stage` остаётся, `phase` обнуляется.
- `Store::set_out_dir` и `Store::set_phase`; `start_job` и `requeue` очищают `fail_*`, `fail_elapsed_ms` и `phase`; `finish_job` (обе ветки) и `cancel_job` сбрасывают `phase`.
- Тест о повторе доказывает, что прошлый сбой не показывается как текущий, и что кавычки и `DROP TABLE` в хвосте консоли хранятся как данные (T-01-07).

## Task Commits

1. **Task 1: Типизированный сбой записывается и читается на базе со старой схемой (tracer)** - `a537925` (feat; RED: тесты не компилировались, затем GREEN)
2. **Task 2: Решение по колонкам** - без коммита, ответ пользователя: six-columns
3. **Task 3: set_out_dir, set_phase и второй шаг миграции** - `65a27be` (feat; RED: тесты не компилировались, затем GREEN)

**Tracer gate:** `<verify>` Task 1 (`cargo test --release -p store`, `cargo build --release --workspace`) перепроверен перед расширением — Tracer verified end-to-end — expanding.

## Files Created/Modified

- `crates/store/src/lib.rs` — миграции, `JobFailure`, `fail_job`, `set_out_dir`, `set_phase`, новые поля `Job`, чтение в `row`, четыре новых теста

## Decisions Made

См. `key-decisions` во frontmatter. Колонки сверх D-05 добавлены по явному выбору пользователя; 01-07 исполняется как написан (под six-columns).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] `fail_job` и `reset_running` сбрасывают `phase`**
- **Found during:** Task 3
- **Issue:** план перечисляет сброс `phase` только в `finish_job`, `cancel_job`, `start_job`, `requeue`. Упавшая через `fail_job` или оборванная `reset_running` задача сохранила бы устаревшее состояние записи, и лестница в карточке показывала бы «выполняется» у мёртвой задачи.
- **Fix:** `phase = ''` добавлен в `UPDATE` обоих методов; тест `typed_failure_round_trips_and_retry_clears_it` проверяет `phase == ""` после `fail_job`.
- **Files modified:** `crates/store/src/lib.rs`
- **Committed in:** `65a27be`

### Уточнения сверх плана

- `cargo fmt` не запускался: `cargo fmt -p store --check` даёт около 400 строк различий в существующем коде. Новый код написан в стиле соседнего (широкая строка), как и в 01-02.
- Ветка `main` — защищённая по правилам исполнителя, но запуск по прямому указанию оркестратора (последовательный исполнитель на основном дереве), коммиты сделаны в `main`.

---

**Total deviations:** 1 auto-fixed (Rule 2)
**Impact on plan:** контракт не изменился, только `phase` ведёт себя последовательнее.

## Issues Encountered

- В окружении нет Python; правки сделаны инструментом Edit.
- Живая база пользователя не открывалась: миграция проверена на синтетических файлах версий 0 и 1.

## Known Stubs

Нет.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. T-01-07: все записи через `params!`, текст сбоя и хвост консоли в SQL не форматируются (в тесте хвост содержит `'; DROP TABLE jobs;--`). T-01-08: каждый шаг в транзакции вместе с `user_version`, только `ADD COLUMN`, тесты на версиях 0 и 1 и на повторном открытии.

## Next Phase Readiness

Машина записи (01-04) и приложение (01-07) могут вызывать `fail_job`, `set_phase`, `set_out_dir` и читать `Job.fail_*`, `Job.phase`. Внимание: `Job` получил новые обязательные поля — любой код, строящий `Job` литералом, надо дополнять (в workspace таких мест нет: `Job` строится только в `Store::row`).

## Self-Check: PASSED

- Файл `crates/store/src/lib.rs` на месте; коммиты `a537925` и `65a27be` в истории ветки, измерено `commits: 2` от `plan_head_before`.
- Критерии приёмки Task 1 и Task 3: все подстроки (`MIGRATIONS`, `PRAGMA user_version`, `ALTER TABLE jobs ADD COLUMN fail_code TEXT`, `...fail_tail TEXT`, `...fail_elapsed_ms INTEGER`, `...phase TEXT NOT NULL DEFAULT ''`, `pub struct JobFailure`, `pub fn fail_job`, `pub fn set_out_dir`, `pub fn set_phase`, `r.get("fail_code")`, `fail_elapsed_ms = NULL`) найдены.
- `cargo test --release -p store`: 6 passed, 0 failed (в том числе `old_database_gains_failure_columns`, `typed_failure_round_trips_and_retry_clears_it`, `out_dir_and_phase_are_kept_while_running`, `version_one_database_gains_elapsed_and_phase`, а также `job_lifecycle` и `crash_recovery_and_caches`).
- `cargo build --release --workspace`: код 0.
