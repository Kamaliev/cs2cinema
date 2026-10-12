---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 01
subsystem: app-runner
tags: [runner, queue, store, sqlite, migration, std-thread, server-stand]

requires:
  - phase: 06-ispolnitel-rannera
    provides: "runner::Executor, PipelineExecutor, Tee, outbox runner_state.json"
  - phase: 05-server
    provides: "server::serve (настоящий сервер в тестах приложения)"
provides:
  - "runner::Task.epoch и runner::Config::new"
  - "store: миграция 6 (external_ref + уникальный частичный индекс), adopt_remote, job_by_external_ref"
  - "app::queue: SettleHook, Advice, start_with_hook; Queue: Clone; opens_result"
  - "app::remote: RemoteHub, RemoteExecutor, ExecFactory, queue_runner, стенд тестов (Stand, Rig, pipeline_factory)"
  - "трассирующий тест: задача сервера -> карточка -> PipelineExecutor -> ролик на сервере"
affects: [08-02, 08-03, 08-04, 08-05, 08-06]

actuals:
  tokens: 12600
  tasks: 2
  commits: 2
plan_head_before: b852683e8175de8bf8d731302b3bf0f94af14e51
plan_head_after: 7655ebbefd10702154ae9d8da03900ff640797f5

tech-stack:
  added: []
  patterns:
    - "Удалённая задача - обычная карточка jobs с external_ref = srv:{job_id}:{epoch}"
    - "Поток раннера только создаёт карточку и ждёт итога по каналу; запись идёт в единственном потоке очереди"
    - "Итог раннеру - через SettleHook после QueuePolicy и только если решение не AutoRetry"

key-files:
  created:
    - crates/app/src/remote/mod.rs
    - crates/app/src/remote/hub.rs
    - crates/app/src/remote/testkit.rs
    - crates/app/src/remote/tests.rs
  modified:
    - crates/runner/src/exec.rs
    - crates/runner/src/lib.rs
    - crates/runner/tests/exec.rs
    - crates/store/src/lib.rs
    - crates/app/Cargo.toml
    - crates/app/src/main.rs
    - crates/app/src/queue.rs
    - crates/cli/src/acceptance.rs
    - Cargo.lock

key-decisions:
  - "D-03: epoch в runner::Task; external_ref = srv:{job_id}:{epoch}"
  - "D-05 (часть): подпись карточки - Задача с сервера #N, карта пуста до плана 08-02"
  - "Ветка удалённой карточки - обёртка queue_runner над прежним jobs::runner; jobs::runner и queue::start не менялись"
  - "Хук очереди применяется только к окончательному решению; совет снимает лишь паузу отмены"

patterns-established:
  - "Тестовый стенд приложения: Stand (server::serve) + Rig (база, хаб, очередь, поток раннера) + pipeline_factory на FakeHost"

requirements-completed: []  # APP-01, APP-04 покрыты частично; отметка выставляется после проверки фазы (решение оркестратора)

coverage:
  - id: D1
    description: "Задача настоящего сервера становится карточкой очереди и её ролик принимается сервером"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema server_job_becomes_a_card_and_its_video_reaches_the_server"
        status: pass
    human_judgment: false
  - id: D2
    description: "queue::start сохранил сигнатуру и поведение; хук слышит окончательное решение один раз"
    requirement: "APP-04"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema queue (hook_hears_the_final_verdict_once_after_an_auto_retry, hook_lifts_only_the_cancel_pause, queue_without_hook_keeps_the_cancel_pause)"
        status: pass
    human_judgment: false
  - id: D3
    description: "Миграция 6 и усыновление карточки по external_ref"
    requirement: "APP-01"
    verification:
      - kind: unit
        ref: "cargo test --release -p store (version_five_database_gains_external_ref, adopt_remote_creates_one_card_per_grant_and_requeues_it_in_any_status, job_by_external_ref_matches_the_whole_reference)"
        status: pass
    human_judgment: false

duration: 40min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 01: Трассирующий срез - задача сервера карточкой очереди Summary

**Задача настоящего `server::serve` проходит путь «раннер -> RemoteExecutor -> карточка с external_ref -> поток очереди -> PipelineExecutor на FakeHost -> ролик на сервере» без изменения `queue::start`.**

## Performance

- **Duration:** ~40 мин
- **Tasks:** 2 (tracer + auto)
- **Commits:** 2 (`e83f9bb`, `7655ebb`)

## Accomplishments

- `runner::Task` получил `epoch` (D-03), `runner::Config::new(server, token, dir)` собирает настройки со значениями по умолчанию: приложение и его тесты не зависят от появления новых полей `Config`.
- `store`: миграция 6 только добавляет колонку `external_ref` и уникальный частичный индекс `jobs_external_ref`; `Job.external_ref`, `Store::adopt_remote` (одна транзакция: создать карточку или вернуть найденную в `queued` с очисткой сбоя, `auto_retries` и прежняя папка сохраняются), `Store::job_by_external_ref` (точное совпадение).
- `queue.rs`: `SettleHook` и `Advice`, `start_with_hook`; `start` - прежняя сигнатура, зовёт `start_with_hook(.., None)`; `Queue: Clone`; `opens_result` не открывает в плеере ролик чужой задачи. Хук зовётся один раз после `QueuePolicy::after` и только если решение не `AutoRetry`; совет `suppress_pause` снимает исключительно паузу отмены.
- `app::remote`: `RemoteHub` (записи карточек, канал событий, `impl SettleHook`), `RemoteExecutor` (поток раннера: карточка + запись хаба под одним замком, пересылка этапов, строк, событий и `Artifact::OutDir`, ожидание итога), `queue_runner` (ветка удалённой карточки над прежним исполнителем очереди; закрытая выдача + «Повторить» идёт локально без отправки, без `resume` - понятная ошибка), `CardProgress` (журнал карточки + копия в канал; отмена автора или сервера).
- Стенд тестов приложения: `Stand` на настоящем `server::serve`, `Rig` (база, хаб, очередь, поток раннера), `pipeline_factory` с настоящим `PipelineExecutor` на `FakeHost` (дочерняя предпроверка и ffprobe-проверка ролика проходят).

## Task Commits

1. **Task 1: задача сервера становится карточкой очереди, ролик принимается сервером** - `e83f9bb` (feat)
2. **Task 2: миграция, усыновление и хук очереди закреплены тестами** - `7655ebb` (test)

Тесты `store` (миграция v5->v6, усыновление, точная ссылка) вошли в коммит Task 1 вместе с реализацией; тесты очереди и `remote` - в коммит Task 2.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Литерал `store::Job` в тесте `cs2-cli acceptance`**
- **Found during:** Task 1 (новое поле `Job.external_ref`)
- **Issue:** `crates/cli/src/acceptance.rs` (тест `fn job`) собирает `store::Job` литералом - без нового поля крейт `cli` не собирался бы.
- **Fix:** добавлено `external_ref: None`.
- **Files modified:** `crates/cli/src/acceptance.rs`
- **Commit:** `e83f9bb`

**2. [Rule 3 - Blocking] dev-зависимость `wire`**
- **Found during:** Task 1 (тест сравнивает `JobRow.state` с `wire::JobState::Done`)
- **Issue:** тип состояния сервера доступен приложению только через `wire`; план называл одну dev-зависимость `server`.
- **Fix:** `wire = { path = "../wire" }` добавлена в `[dev-dependencies]` приложения рядом с `server`. Путевой крейт воркспейса, внешних пакетов нет; манифест `crates/server` не менялся (`--test deps` зелёный).
- **Files modified:** `crates/app/Cargo.toml`, `Cargo.lock`
- **Commit:** `e83f9bb`

**3. [Rule 3 - Blocking] `#![allow(dead_code)]` в `remote/mod.rs`**
- **Issue:** хаб подключается к окну и настройкам планами 08-04/08-05; до тех пор вне тестов он не используется, что даёт предупреждения.
- **Fix:** модульный `#![allow(dead_code)]` с комментарием, снимается планом 08-04.

Других отклонений нет. `jobs::runner`, `queue::start`, `QueueConfig` и `crates/server` не менялись.

## Known Stubs

Нет. Карта карточки пуста намеренно (заполнение - план 08-02), отмена сервера пересылается флагами `server_cancel`/`server_force` без источника (поднимает план 08-03): цикл `RemoteExecutor::run` пока только пересылает события и ждёт итога, как требует план.

## Threat Flags

Нет новой сетевой поверхности: приложение остаётся исходящим клиентом, входящих запросов не принимает. Митигации плана: T-08-01 (удалённая карточка идёт только через `PipelineExecutor`, `inspect_runs == 1` в тесте), T-08-02 (поток раннера записи не запускает), T-08-03 (ник через `clean_text`, подпись из номера), T-08-04 (`external_ref` + уникальный индекс + одна транзакция), T-08-05 (`on_settled` после политики, не при `AutoRetry`).

## Test Results

- `cargo test --release -p app --bin cs2cinema` - 48 passed (включая 26 прежних тестов очереди без правок и `server_job_becomes_a_card_and_its_video_reaches_the_server`)
- `cargo test -p app queue` (debug) - 26 passed
- `cargo test --release -p store` - 29 passed
- `cargo test --release -p runner` - зелёный (все наборы)
- `cargo test --release -p cli` (включая `--test pipeline`) - зелёный
- `cargo test --release -p server --test deps` - 2 passed; `git diff -- crates/server` пуст
- `cargo build --release -p cli -p app` - без ошибок

## Self-Check: PASSED

- Файлы `crates/app/src/remote/{mod,hub,testkit,tests}.rs` существуют.
- Коммиты `e83f9bb` и `7655ebb` достижимы из HEAD.
