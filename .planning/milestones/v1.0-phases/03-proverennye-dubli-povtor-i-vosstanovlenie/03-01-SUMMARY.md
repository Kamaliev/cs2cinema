---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 01
subsystem: store
tags: [rust, sqlite, rusqlite, egui, journal, migration]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: статус «отменяется», пауза очереди после отмены (D-13), reset_running, interrupted_jobs
provides:
  - "JobStatus::Interrupted («прервана») и Store::reset_running -> Reset { interrupted, cancelled }"
  - "Миграция базы 4: jobs.attempt, jobs.resume; Job.attempt/Job.resume; start_job -> номер попытки без стирания журнала"
  - "Store::requeue_resume (флаг «повтор с записи»); requeue сбрасывает resume"
  - "store::journal: StartKind, separator, prefix, stage_line, outcome_line, stage_summary, fmt_dur"
  - "app::PauseReason и pause_banner(reason, waiting): очередь стоит после прерванной задачи до «Продолжить»"
  - "JobReporter::new(.., attempt), метка [этап #N] на строках базы, строки «⏱ этап X: N», close_stage()"
affects: [03-06, 03-07, 03-08, карточка задачи, кнопка «Повторить»]

actuals:
  tokens: 12100
  tasks: 3
  commits: 3

plan_head_before: 7caeafed0a63313f49afaba84e76bdb0b19cfe19
plan_head_after: 740f30101080feba2451238d972ee8bb8dd652a7
commits: 3

tech-stack:
  added: []
  patterns:
    - "формат журнала задачи живёт в одном модуле store::journal — писатель (app::jobs) и читатель (карточка) не расходятся"
    - "журнал не стирается: попытки копятся, каждая открывается разделителем и закрывается строкой итога"
    - "в окно (Msg::Log) уходит сырой текст, метка этапа и времени — только в строку базы"

key-files:
  created:
    - crates/store/src/journal.rs
  modified:
    - crates/store/src/lib.rs
    - crates/app/src/app.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/jobs.rs

key-decisions:
  - "Прерванная задача: stage остаётся, достигнутое phase уходит в fail_state через NULLIF(phase, ''), код INTERRUPTED, подсказка про «Повторить»/«Заново»"
  - "Метка до первого этапа — «Старт», во время предпроверки — «Предпроверка» (на усмотрение Claude по CONTEXT)"
  - "Строка итога пишется в finish до смены статуса; этап в итоге берётся из строки задачи, пустой — «Старт»"
  - "До 03-07 «Повторить» у прерванной задачи вызывает requeue (как у упавшей); requeue_resume и колонка resume готовы"

patterns-established:
  - "Миграция базы только добавлением колонок в конец MIGRATIONS, тест версии N-1 -> N"
  - "Причина паузы очереди — enum PauseReason, текст баннера — чистая функция pause_banner"

requirements-completed: [RET-01, RET-04, OBS-01]

coverage:
  - id: D1
    description: "Задача, прерванная падением приложения, получает статус «прервана» с этапом и достигнутым состоянием, а не «ошибка»"
    requirement: RET-04
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#crash_recovery_turns_cancelling_into_cancelled_and_running_into_interrupted"
        status: pass
    human_judgment: false
  - id: D2
    description: "Очередь стоит на паузе после прерванной задачи, баннер называет причину"
    requirement: RET-04
    verification:
      - kind: unit
        ref: "crates/app/src/jobs_page.rs#pause_banner_names_the_reason"
        status: pass
    human_judgment: false
  - id: D3
    description: "Журнал переживает повтор: номер попытки, разделитель, строка итога"
    requirement: RET-01
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#log_survives_restart_and_attempts_count_up"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#finished_attempt_leaves_an_outcome_line_and_the_next_one_a_separator"
        status: pass
    human_judgment: false
  - id: D4
    description: "Каждая строка журнала несёт метку [этап #попытка], смена этапа пишет длительность"
    requirement: OBS-01
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#journal_lines_carry_stage_and_attempt_and_stage_durations"
        status: pass
    human_judgment: false
  - id: D5
    description: "База версии 3 открывается и получает колонки версии 4"
    requirement: RET-01
    verification:
      - kind: unit
        ref: "crates/store/src/lib.rs#version_three_database_gains_attempt_and_resume"
        status: pass
    human_judgment: false
  - id: D6
    description: "Живая проверка: убить приложение посреди записи и увидеть «прервана» и баннер"
    verification: []
    human_judgment: true
    rationale: "Нужны реальное падение приложения и глаза на карточке; запланировано в 03-08"

duration: 25min
completed: 2026-10-11
status: complete
---

# Phase 3 Plan 01: Прерванные задачи и журнал попыток Summary

**Статус `interrupted` («прервана») для задач, оборванных падением приложения, пауза очереди с причиной, и журнал задачи, переживающий повторы: попытки нумеруются (`jobs.attempt`), разделяются, закрываются итогом, а каждая строка помечена `[этап #N]` с длительностью этапов.**

## Performance

- **Duration:** около 25 мин
- **Tasks:** 3 (1 tracer, 2 auto/tdd)
- **Files modified:** 5 (1 создан)

## Accomplishments

- `Store::reset_running` теперь переводит `running` в `interrupted` (этап остаётся, `phase` -> `fail_state`, код `INTERRUPTED`, подсказка), `cancelling` -> `cancelled` как раньше; возвращает `Reset { interrupted, cancelled }`.
- `App::new` при `reset.interrupted > 0` ставит очередь на паузу с `PauseReason::Interrupted`; баннер на странице задач называет причину; бейдж «прервана», лестница до достигнутого состояния и блок ошибки; кнопка «Повторить» есть.
- Миграция 4 добавила `attempt` и `resume`; `start_job` возвращает номер попытки и больше не стирает журнал; `requeue_resume` и сброс `resume` в `requeue` готовы для 03-06/03-07.
- Модуль `store::journal` — единый формат журнала (разделитель, метка, строка этапа, итог, разбор `stage_summary`).
- `JobReporter` пишет в базу строки `время  [этап #N] текст`, при смене этапа и в конце попытки — «⏱ этап X: N мин M с»; в окно уходит сырой текст (разбор `status.rs` не затронут).

## Task Commits

1. **Task 1: прерванная задача и пауза очереди (tracer)** - `d75cfda`
2. **Task 2: журнал переживает повтор** - `250f248`
3. **Task 3: метка этапа и длительности в журнале** - `740f301`

## Deviations from Plan

None - план выполнен как написан. Мелкие уточнения в рамках усмотрения: в строке итога пустой этап показывается как «Старт»; для состояния сбоя в итоге используется русский заголовок `RecordState::title()`.

Pre-commit проверка «HEAD не на защищённой ветке» не выполнялась намеренно: оркестратор запустил план последовательно на основной рабочей копии (ветка `main`, worktree отключены), как и предыдущие фазы.

## Verification

- `cargo test --release -p store` — 17 passed
- `cargo test --release -p app --bin cs2cinema` — 26 passed (в т.ч. `pause_banner_names_the_reason`, `finished_attempt_leaves_an_outcome_line_...`, `journal_lines_carry_stage_and_attempt_and_stage_durations`)
- `cargo test --release -p cli` — 2 passed (`pipeline.rs`)
- `cargo build --release --workspace` — без ошибок

## Known Stubs

None.

## Threat Flags

None - новых сетевых путей или границ доверия нет; все значения пишутся параметрами `params![..]`, сообщение в строке итога обрезается до 200 символов (T-03-01), `interrupted` ставится только строкам `running` (T-03-03).

## Self-Check: PASSED

- FOUND: crates/store/src/journal.rs
- FOUND commits: d75cfda, 250f248, 740f301
