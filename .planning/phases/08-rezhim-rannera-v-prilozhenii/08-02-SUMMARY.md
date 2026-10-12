---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 02
subsystem: runner
tags: [runner, gate, observer, admission, preflight, std-thread, server-stand]

requires:
  - phase: 08-rezhim-rannera-v-prilozhenii
    provides: "runner::Task.epoch, runner::Config::new, app::remote (08-01)"
  - phase: 06-ispolnitel-rannera
    provides: "Runner, PipelineExecutor, hello/pause_ping, runner_state.json"
provides:
  - "runner: Admission, Gate, Config.admission, Tick::Gated"
  - "runner: RunnerEvent, Observer, Config.observer, Runner::holding, реэкспорт Holding и JobState"
  - "pipeline: Artifact::Match { map, player }, Preflight.map"
  - "PipelineExecutor отправляет Artifact::Match после дочерней предпроверки"
  - "USAGE_RUNNER и подсказка о ключе FACEIT для двух хозяев раннера"
affects: [08-03, 08-04, 08-05, 08-06]

actuals:
  tokens: 26000
  tasks: 3
  commits: 3
plan_head_before: 5d20d6b22aecf2669e300c18fedd79199035a17e
plan_head_after: 27676ef5da5b26c2156f0b31a8b374aabf1bddcc

tech-stack:
  added: []
  patterns:
    - "Ворота допуска: закрытые ворота - нет claim, hello 'занят: причина' держит раннер на связи"
    - "Повторная проверка ворот после claim и release(counts: false) - гонка не сжигает выдачи"
    - "Наблюдатель - поле Config.observer; emit вычёркивает токен и зовётся после записи runner_state.json"

key-files:
  created:
    - crates/runner/tests/gate.rs
    - crates/runner/tests/observer.rs
  modified:
    - crates/runner/src/lib.rs
    - crates/runner/src/exec.rs
    - crates/runner/tests/common/mod.rs
    - crates/runner/tests/exec.rs
    - crates/cli/src/runner_cmd.rs
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/inspect.rs
    - crates/app/src/jobs.rs

key-decisions:
  - "Ворота не смотрятся для уже удерживаемой задачи (drive идёт первым), смотрятся после паузы раннера и до claim"
  - "Если release после закрытия ворот не прошёл - задача принимается как обычная и ждёт в очереди приложения"
  - "Working - одно событие на удержание (поле announced), повторный drive после ошибки связи его не дублирует"
  - "Fatal отправляется один раз в конце run() при выходе по TOKEN_REJECTED/PROTOCOL_MISMATCH, в том числе из lose()"
  - "Строка журнала предпроверки: 'карта X, ' добавляется только если карта известна, прежний текст без карты сохранён"

patterns-established:
  - "Тест ворот и наблюдателя на настоящем server::serve; статус раннера читается из runners.status через core().db()"

requirements-completed: []  # APP-01, APP-02 покрыты частично (раннер); отметка выставляется после проверки фазы (решение оркестратора)

coverage:
  - id: D1
    description: "Закрытые ворота не дают claim, раннер остаётся на связи со статусом 'занят: причина'"
    requirement: "APP-02"
    verification:
      - kind: integration
        ref: "cargo test --release -p runner --test gate (closed_gate_takes_no_job_and_keeps_the_runner_online, opened_gate_takes_the_job_as_before)"
        status: pass
    human_judgment: false
  - id: D2
    description: "Ворота закрылись после claim: release с counts: false, grants == 0, в runner_state.json задачи нет"
    requirement: "APP-02"
    verification:
      - kind: integration
        ref: "cargo test --release -p runner --test gate gate_closing_after_claim_returns_the_job_without_counting_the_grant"
        status: pass
    human_judgment: false
  - id: D3
    description: "Наблюдатель видит связь, работу, итог, ворота, обрыв сети и отказ токена; токена в событиях и runner.log нет"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p runner --test observer"
        status: pass
    human_judgment: false
  - id: D4
    description: "Исполнитель сообщает карту и игрока после предпроверки; подсказка о ключе FACEIT называет переменную и настройки"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p runner --test exec (inspected_demo_reports_its_map_and_player, resumed_task_has_no_preflight_and_reports_no_map, missing_key_is_faceit_fetch_failed)"
        status: pass
      - kind: unit
        ref: "cargo test --release -p pipeline --lib inspect (preflight_reports_the_map)"
        status: pass
    human_judgment: false

duration: 35min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 02: Ворота, наблюдатель и факты о демке в раннере Summary

**Раннер получил ворота допуска перед `claim` (с повторной проверкой и `release(counts: false)`), наблюдателя состояния без токена в событиях и карту/игрока демки из дочерней предпроверки; `cs2-cli runner` без ворот и наблюдателя работает как в фазе 6.**

## Performance

- **Tasks:** 3 (tracer + 2 auto/tdd)
- **Commits:** 3 (`fe96ffb`, `3162d4a`, `27676ef`)

## Accomplishments

- `Admission`/`Gate`/`Config.admission`/`Tick::Gated`: порядок веток `tick()` - удерживаемая задача, пауза раннера, ворота, `claim`. Закрытые ворота шлют `hello` со статусом «занят: причина» сразу и не чаще `PAUSE_HELLO_EVERY` (30 с); сервер считает раннера «на связи». После успешного `claim` ворота проверяются ещё раз: закрыты - `release` с `counts: false`, строка в `runner.log`, `runner_state.json` и `release_cooldown` не трогаются.
- `RunnerEvent`/`Observer`/`Config.observer`/`Runner::holding`: события `Connected`, `Idle`, `Gated`, `Working`, `Settled`, `Released`, `Dropped`, `Paused`, `NetError`, `Fatal` вместе с удерживаемой задачей. `emit` вычёркивает токен (`***`) во всех строковых полях и пропускает их через `one_line`.
- `Artifact::Match { map, player }` и `Preflight.map` (через `clean_text`); `PipelineExecutor::inspect_demo` сообщает пару после предпроверки, при продолжении предпроверки (и события) нет. `JobReporter` игнорирует `Match` для локальных задач.
- Тексты: `USAGE_RUNNER` предупреждает о приложении с адресом сервера; отсутствие ключа FACEIT называет `FACEIT_API_KEY` у `cs2-cli runner` и поле «Ключ Data API» в настройках приложения.

## Task Commits

1. **Task 1: ворота допуска** - `fe96ffb` (feat)
2. **Task 2: наблюдатель** - `3162d4a` (feat)
3. **Task 3: Artifact::Match и подсказка о ключе** - `27676ef` (feat)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Откат побочного форматирования `cargo fmt -p runner`**
- **Found during:** Task 1
- **Issue:** `cargo fmt -p runner` переформатировал нетронутые файлы крейта (`exec.rs`, `link.rs`, `transfer.rs`, `tests/exec.rs`, `faceit_flow.rs`, `flow.rs`), которые не были чистыми по rustfmt до плана.
- **Fix:** эти файлы возвращены к состоянию из git (`git checkout -- <файл>`), дальше форматировались только собственные новые файлы (`rustfmt` по файлу) и правки вносились вручную в существующем стиле. Хвост `tests/exec.rs` с новыми тестами отформатирован по умолчанию.
- **Commit:** изменений в коммитах нет.

**2. [Rule 2 - полнота] Журнал предпроверки без карты не меняется**
- **Issue:** план требует строку «демка проверена: карта de_mirage, игрок s1mple…»; прежний тест фазы 6 (`environment_failure_after_a_good_preflight_returns_the_job_to_the_queue`) ждёт «демка проверена: игрок s1mple, моментов 3» при ответе без карты.
- **Fix:** префикс «карта X, » добавляется только если карта непустая; прежний тест остался без правок.
- **Commit:** `27676ef`

В остальном план выполнен как написан. Дополнительная проверка по просьбе оркестратора: `cargo test --release -p cli --test pipeline` и `cargo test -p app queue` зелёные.

## Known Stubs

Нет. Ветка `Artifact::Match { .. } => {}` в `JobReporter` намеренная: карта и игрок локальной задачи записаны при постановке, запись удалённой карточки - план 08-05 (`remote::hub::CardProgress`).

## Threat Flags

Новой сетевой поверхности нет. Митигации плана: T-08-06 (`emit` вычёркивает токен, тест `run_reports_gate_and_net_errors_without_the_token` проверяет и события, и `runner.log`, в том числе токен внутри причины ворот), T-08-07 (`hello` «занят» сразу и раз в 30 с, тест проверяет `runner_online()` и `runners.status`), T-08-08 (повторная проверка ворот и `release(counts: false)`, `grants == 0`), T-08-09 (`clean_text(.., NAME_MAX)` для карты).

## Test Results

- `cargo test --release -p runner` - зелёный (27 unit + наборы flow/exec/heartbeat/outbox/policy/faceit_flow/gate/observer; `gate` 3, `observer` 5, `exec` 17)
- `cargo test --release -p pipeline --lib` - 140 passed (в т.ч. `preflight_reports_the_map`)
- `cargo test --release -p app --bin cs2cinema` - 48 passed
- `cargo test -p app queue` (debug) - 26 passed
- `cargo test --release -p cli --test pipeline` - 2 passed; `--bin cs2-cli` - 46 passed
- `cargo build --release -p cli -p app` - без ошибок; `git diff --stat -- crates/server` пуст

## Self-Check: PASSED

- Файлы `crates/runner/tests/gate.rs`, `crates/runner/tests/observer.rs` существуют.
- Коммиты `fe96ffb`, `3162d4a`, `27676ef` достижимы из HEAD.
