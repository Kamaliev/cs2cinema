---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 01
subsystem: infra
tags: [rust, host-trait, job-object, windows-sys, fake-clock, spikes]

requires: []
provides:
  - "крейт crates/host: трейты Host/Session, SessionEvent, Source, Liveness, HostError, LaunchSpec, CmdSpec, CmdOutput, console_line_ok, default_host"
  - "host::fake::{FakeHost, FakeSession, Scenario, FakeMarker} — фейк с виртуальным временем и сценариями сбоев"
  - "host::UnsupportedHost — заглушка для не-Windows (launch/run -> HostError::Unsupported)"
  - "host::win::job::Job — безопасная обёртка Job Object (KILL_ON_JOB_CLOSE)"
  - "host::win::process::{OwnedProc, ProcEntry, snapshot_processes, liveness_of_pid} — живость по handle, неизвестность никогда не Alive"
  - "пример crates/host/examples/spikes.rs — стенд спайков S1-S4 на настоящем Job"
affects: [01-02, 01-04, 01-05, 01-06, 01-07, 01-08]

actuals:
  tokens: 29900
  tasks: 3
  commits: 3
plan_head_before: b194212a8bd2786c701c6ed88c3b5e5cbd737484
plan_head_after: a5e25718fd9a36377cef4ac1c0aea2987091ccad

tech-stack:
  added: ["windows-sys 0.61.2 (только под cfg(windows), 5 фич)"]
  patterns:
    - "единственное блокирующее ожидание сессии — Session::poll(max_wait); политика дедлайнов снаружи"
    - "виртуальные часы фейка двигает только poll; потолок 10 ч защищает от зависшего сценария"
    - "весь unsafe только в crates/host/src/win/, у каждого блока // SAFETY:"
    - "живость: WAIT_TIMEOUT=Alive, WAIT_OBJECT_0=Exited(код), любая ошибка=Unknown(текст ОС)"

key-files:
  created:
    - crates/host/Cargo.toml
    - crates/host/src/lib.rs
    - crates/host/src/fake.rs
    - crates/host/src/unsupported.rs
    - crates/host/src/win/mod.rs
    - crates/host/src/win/job.rs
    - crates/host/src/win/process.rs
    - crates/host/examples/spikes.rs
  modified:
    - Cargo.toml
    - Cargo.lock

key-decisions:
  - "Fake: отклонённые (BadCommand) и отправленные до ConsoleUp (NotConnected) команды не попадают в sent() — как в must_haves «ничего не отправляя»"
  - "Fake: задержка метки сдвигает и все последующие метки той же цепочки (как медленная игра), убранная метка не приходит, но файлы дубля на диске остаются"
  - "Fake: terminate() считается один раз на сессию и в Drop не считается, чтобы terminated() отвечал только на явное завершение"
  - "Fake не шлёт ConsoleDown при выходе игры (по таблице плана: GameExited + JobEmpty); после GameExited команды получают NotConnected"
  - "Стенд S2: без --fix запуски чередуются off/on; для схемы 5+5 запускать дважды с --fix off и --fix on, чтобы уложиться в 10 минут"

patterns-established:
  - "FakeHost клонируется: копия в тесте видит sent()/terminated()/launches() и now()"
  - "Сценарий — pub-структура с методами-строителями (Scenario::happy(n).drop_marker(..).delay_marker(..))"

requirements-completed: [TST-01, TMO-02]

coverage:
  - id: D1
    description: "Листовой крейт host без зависимостей на крейты workspace; windows-sys только под cfg(windows); workspace собирается, cargo test --workspace зелёный"
    requirement: "TST-01"
    verification:
      - kind: integration
        ref: "cargo build --release --workspace; cargo test --release --workspace"
        status: pass
    human_judgment: false
  - id: D2
    description: "FakeHost проигрывает сценарий записи в виртуальном времени (READY, SCRIPT, TAKE_START/END, ALL_DONE, quit), метки можно убрать или задержать, зациклившийся сценарий паникует, а не висит"
    requirement: "TST-01"
    verification:
      - kind: unit
        ref: "crates/host/src/fake.rs#fake_session_plays_launch_console_and_markers_in_virtual_time"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#idle_poll_advances_virtual_time_by_max_wait"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#runaway_scenario_panics_instead_of_hanging"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#dropped_marker_never_arrives_and_send_rejects_newlines"
        status: pass
    human_judgment: false
  - id: D3
    description: "UnsupportedHost на не-Windows отдаёт HostError::Unsupported вместо паники"
    requirement: "TST-01"
    verification:
      - kind: unit
        ref: "crates/host/src/unsupported.rs#unsupported_host_refuses_to_launch"
        status: pass
    human_judgment: true
    rationale: "Тест выполняется на Windows (там default_host() тоже UnsupportedHost); сборка под Linux проверяется только CI ubuntu"
  - id: D4
    description: "Job Object владеет деревом процессов: потомки попадают в job, terminate и закрытие handle завершают всё; живость по handle трёхзначна, ошибка = Unknown"
    requirement: "TMO-02"
    verification:
      - kind: unit
        ref: "crates/host/src/win/job.rs#child_lands_in_job_and_terminate_kills_the_tree"
        status: pass
      - kind: unit
        ref: "crates/host/src/win/job.rs#dropping_the_job_kills_its_processes"
        status: pass
      - kind: unit
        ref: "crates/host/src/win/process.rs#liveness_of_a_missing_process_is_unknown_not_alive"
        status: pass
      - kind: unit
        ref: "crates/host/src/win/process.rs#snapshot_finds_the_current_process"
        status: pass
    human_judgment: false
  - id: D5
    description: "Стенд спайков S1-S4: собирается, печатает справку и предпроверку, отказывается работать при запущенном cs2.exe; реальные прогоны S1-S4 с игрой не выполнялись"
    requirement: "TMO-02"
    verification:
      - kind: other
        ref: "cargo run --release -q -p host --example spikes -- --help; ... -- job --dry-run; ... -- job --hlae C:/nonexistent-hlae (exit 2)"
        status: pass
    human_judgment: true
    rationale: "Поведение HLAE/CS2 (попадает ли cs2.exe в job, -afxFixNetCon, тексты консоли, абсолютный +playdemo) можно проверить только запуском игры на машине автора — это план 01-05"

duration: 11min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 01: Крейт host — шов между политикой записи и ОС Summary

**Листовой крейт `host` с трейтами `Host`/`Session`, фейком на виртуальных часах, безопасными обёртками Job Object и живости по handle и стендом спайков S1-S4 на настоящем `Job`.**

## Performance

- **Duration:** 11 min
- **Started:** 2026-10-10T10:41:26Z
- **Completed:** 2026-10-10T10:53Z
- **Tasks:** 3 (1 tracer, 2 auto)
- **Files modified:** 10 (8 новых, `Cargo.toml` и `Cargo.lock`)

## Accomplishments

- Крейт `crates/host` в workspace: контракт `Host`/`Session`/`SessionEvent`/`Liveness`/`HostError` заморожен в виде из плана, на него опираются 01-04, 01-06, 01-07. `default_host()` пока везде возвращает `UnsupportedHost`; Windows-ветку подключит 01-06.
- `FakeHost` с виртуальным временем: `poll(max_wait)` без событий сдвигает часы ровно на `max_wait`, ожидание 150 с в тесте занимает микросекунды, зациклившийся сценарий падает с паникой «фейк: виртуальное время перевалило за 10 ч». Фейк отвечает на READY / `exec highlights` / `quit` как игра, умеет убирать и задерживать метки, поздний netcon (`ready_only_direct`), дубли из `console.log`, смерть игры, пишет файлы дублей во временную папку.
- `Job` (KILL_ON_JOB_CLOSE, `assign_child`/`assign_pid`, `pids`, `active_count`, `terminate`, `Drop`) и `OwnedProc` (живость ожиданием на handle; ошибка = `Unknown`, не `Alive`) + `snapshot_processes`. Тесты на Windows стабильны (5 прогонов подряд).
- Стенд `spikes` (`job`, `netcon`, `failures`, `abspath`, `--dry-run`): работает на настоящем `Job`, при запущенном `cs2.exe` отказывается с кодом 2 (проверено на машине автора: его CS2 с PID 48660 не тронут), пути профиля в транскриптах заменяются на `C:/Users/USER`.

## Task Commits

1. **Task 1: Сессия записи на фейковом хосте (tracer)** - `754cb82` (feat)
2. **Task 2: Job Object и живость по handle (Windows)** - `5467223` (feat; RED подтверждён ошибкой компиляции тестов до реализации, затем GREEN)
3. **Task 3: Стенд спайков S1-S4 на настоящем Job** - `a5e2571` (feat)

**Tracer gate:** `<verify>` задачи 1 (авто) перепроверен после форматирования — `Tracer verified end-to-end — expanding`.

## Files Created/Modified

- `Cargo.toml` — `crates/host` в members; `Cargo.lock` — запись `host` (windows-sys остался 0.61.2)
- `crates/host/Cargo.toml` — манифест, windows-sys только под `cfg(windows)`
- `crates/host/src/lib.rs` — трейты и типы, `console_line_ok`, `default_host`
- `crates/host/src/fake.rs` — `FakeHost`, `Scenario`, `FakeMarker`, 10 тестов
- `crates/host/src/unsupported.rs` — `UnsupportedHost`
- `crates/host/src/win/{mod,job,process}.rs` — Job Object, процессы, снимок
- `crates/host/examples/spikes.rs` — стенд спайков

## Decisions Made

См. `key-decisions` во frontmatter. Главное: фейк трактует «ничего не отправляя» буквально (отклонённые команды не попадают в `sent()`), задержка метки сдвигает цепочку, `terminate` считается один раз на сессию.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] `impl std::error::Error for HostError`**
- **Found during:** Task 1
- **Issue:** тип ошибки без `Error` нельзя класть в `Box<dyn Error>` в крейтах `faceit`/`cli`/`geometry` без обёрток.
- **Fix:** добавлена пустая реализация `Error`.
- **Files modified:** `crates/host/src/lib.rs`
- **Committed in:** `754cb82`

**2. [Rule 1 - Bug] Конфликт тестового сценария «смерть игры»**
- **Found during:** Task 1
- **Issue:** первый вариант теста `game_death_silences_pending_markers_and_reports_the_exit` ставил смерть на 20 с, когда `TAKE_END 0` приходил раньше (13 с) — тест ждал не то событие.
- **Fix:** смерть игры перенесена на 10 с (между `TAKE_START 0` и `TAKE_END 0`).
- **Files modified:** `crates/host/src/fake.rs`
- **Committed in:** `754cb82`

**3. [Rule 1 - Lint] Вложенные `if` в стенде**
- **Found during:** Task 3
- **Issue:** `cargo clippy` предупреждал о четырёх сворачиваемых `if` (edition 2024, let-цепочки).
- **Fix:** применён `cargo clippy --fix` к примеру; предупреждений нет.
- **Files modified:** `crates/host/examples/spikes.rs`
- **Committed in:** `a5e2571`

### Уточнения сверх плана (не отклонения от контракта)

- `cargo fmt -p host` с настройками по умолчанию (ширина 100) раздул файлы: `spikes.rs` ~1350 строк, `fake.rs` ~915. Содержимое то же; остальные крейты не форматировались.
- Добавлены тесты сверх обязательных шести: `late_netcon_loses_the_timed_ready_but_answers_the_direct_echo`, `delayed_marker_shifts_the_following_ones`, `game_death_silences_pending_markers_and_reports_the_exit`, `duplicate_markers_come_from_console_log_too`, `launch_error_run_result_and_liveness_override_come_from_the_scenario`, `runaway_scenario_panics_instead_of_hanging`, три теста `lib.rs`.
- `FakeHost::run` проверяет `cancel()` до возврата `run_result` (отмена -> `HostError::Cancelled`).
- Стенд S2 без `--fix` чередует запуски off/on (в плане режим не уточнён); добавлен бюджет 8 минут на подкоманду: новые запуски игры не стартуют, если он исчерпан.

---

**Total deviations:** 3 auto-fixed (1 Rule 2, 2 Rule 1)
**Impact on plan:** на контракт Host/Session не влияет; всё нужно для корректности или чистоты сборки.

## Issues Encountered

- На машине автора во время выполнения был запущен собственный `cs2.exe` (PID 48660). Реальные прогоны S1-S4 поэтому не делались: это и есть работа плана 01-05, а стенд сам отказывается стартовать при чужом `cs2.exe` (проверено, код 2). Проверены сборка, справка, `--dry-run` и путь отказа.
- В Git Bash нет `node` в PATH; для `gsd_run` использовался `PATH="/c/Program Files/nodejs:$PATH"`.

## Known Stubs

Нет. `default_host()` возвращает `UnsupportedHost` на всех платформах по плану; `WinHost` — задача 01-06 (Task 3), `UnsupportedHost::run` тоже станет рабочим там.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Митигации: весь `unsafe` только в `crates/host/src/win/` с `// SAFETY:` (T-01-01); завершение только через свой job/handle, в коде и стенде нет `taskkill`/`tasklist`, стенд отказывается при чужом `cs2.exe` (T-01-02); `windows-sys` 0.61.2 из lock, пять фич (T-01-03); `scrub()` заменяет путь профиля в транскриптах (T-01-04).

## Next Phase Readiness

Контракт `host` готов для 01-02..01-08. 01-05 запускает стенд спайков на машине автора с закрытым CS2 и переносит результаты в `01-SPIKES.md`; 01-06 подключает `WinHost` и переключает `default_host()`.

## Self-Check: PASSED

- Файлы: все 9 созданных/изменённых на месте.
- Коммиты `754cb82`, `5467223`, `a5e2571` найдены в истории ветки; `evaluation-scope --commits-only` вернул 3 коммита плана.
- Критерии приёмки всех задач и `<verification>` плана перепроверены: `cargo test --release -p host --lib` — 19 passed, 4 из них `win::`; `cargo build --release --workspace` и `cargo test --release --workspace` — зелёные; `grep unsafe` вне `src/win/` и `grep taskkill|tasklist` — пусто.
