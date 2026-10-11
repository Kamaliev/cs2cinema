---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 07
subsystem: app
tags: [rust, app, refactor, launch, recover_after_crash]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::gamedir (cs2_exe, kill_orphans, recover), Host, run_job (04-04, 04-05)"
provides:
  - "app::jobs::recover_after_crash поверх pipeline::gamedir с перенесёнными тестами"
  - "app::settings::find_cs2 / find_hlae"
  - "app::ui::no_window (приватная), app::app::old_logs (приватная) с тестом"
  - "crates/app/src/launch.rs удалён; в crates/ нет упоминаний утилит списка и завершения процессов по имени"
affects: [04-11]

actuals:
  tokens: 5200
  tasks: 2
  commits: 2

plan_head_before: 73f30122105d8e651acc4d1ab6f2d9e4dc188549
plan_head_after: 50bf151c9f06dbe4d1ba7825e96db68f23bb4e28
commits: 2

coverage:
  - deliverable: "recover_after_crash в app::jobs: завершить осиротевшую игру, убрать папку игры, пометить «прервана»; чужой процесс не трогается"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/jobs.rs#startup_kills_the_orphan_before_cleanup_and_marks_the_job_interrupted"
        status: pass
      - kind: test
        ref: "crates/app/src/jobs.rs#orphan_with_another_creation_time_is_left_alone"
        status: pass
      - kind: test
        ref: "crates/app/src/jobs.rs#malformed_processes_json_kills_nothing"
        status: pass
  - deliverable: "old_logs в app::app: record.log.N по возрастанию номера, чужие имена не входят"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/app.rs#old_logs_are_listed_in_attempt_order"
        status: pass
  - deliverable: "launch.rs удалён, ссылок на модуль и упоминаний tasklist/taskkill в crates/ нет, workspace собирается"
    human_judgment: false
    verification:
      - kind: command
        ref: "test ! -f crates/app/src/launch.rs && grep -rn 'launch::' crates/app/src | wc -l == 0 && grep -rniE 'tasklist|taskkill' crates | wc -l == 0"
        status: pass
      - kind: command
        ref: "cargo build --release --workspace"
        status: pass
  - deliverable: "cs2-cli продолжает проходить crates/cli/tests/pipeline.rs"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/cli/tests/pipeline.rs"
        status: pass
  - deliverable: "поиск CS2/HLAE в настройках и открытие папок без окна консоли в реальном приложении на ПК автора"
    human_judgment: true
    rationale: "код перенесён без изменения поведения, но кнопки «Найти CS2/HLAE» и запуск cmd start без окна автоматическими проверками не покрыты"

key-files:
  created: []
  modified:
    - crates/app/src/jobs.rs
    - crates/app/src/app.rs
    - crates/app/src/settings.rs
    - crates/app/src/settings_page.rs
    - crates/app/src/match_page.rs
    - crates/app/src/ui.rs
    - crates/app/src/main.rs
  deleted:
    - crates/app/src/launch.rs

key-decisions:
  - "Помощник тестов running_job(store, job) из launch.rs переименован в running_job_in: в jobs.rs уже есть running_job(tag)"
  - "Проверка cs2.exe в settings_page — pipeline::gamedir::cs2_exe(Path), прежняя launch::cs2_exe(&str) удалена вместе с файлом"

requirements-completed: [TST-03]

duration: 10 min
completed: 2026-10-11
---

# Phase 4 Plan 07: удаление launch.rs в приложении Summary

**Остатки `crates/app/src/launch.rs` разъехались по местам использования (`recover_after_crash` — в `app::jobs` поверх `pipeline::gamedir`, `find_cs2`/`find_hlae` — в `app::settings`, `no_window` — в `app::ui`, `old_logs` — в `app::app`), файл удалён, а в `crates/` нет ни одного упоминания утилит списка и завершения процессов по имени.**

## Что сделано

- **Task 1 (tracer), `932df8c`:** `recover_after_crash` перенесена в `crates/app/src/jobs.rs` дословно (порядок «завершить осиротевший процесс → убрать папку игры → `reset_running`» и строки журнала «уборка после перезапуска: …» сохранены), вместе с тремя тестами и помощниками; `old_logs` — приватная функция `crates/app/src/app.rs` с тестом `old_logs_are_listed_in_attempt_order` (в `app.rs` появился блок `mod tests`). `App::new` зовёт `crate::jobs::recover_after_crash`, `refresh_jobs` — локальную `old_logs`.
- **Task 2, `50bf151`:** `find_cs2`/`find_hlae` — в `crates/app/src/settings.rs` (`Settings::default` зовёт локальную `find_cs2`); `no_window` — приватная `#[cfg(windows)]` в `crates/app/src/ui.rs`; `settings_page.rs` и `match_page.rs` используют `pipeline::gamedir::cs2_exe`, `crate::settings::find_cs2`/`find_hlae`; файл `launch.rs` и объявление `mod launch;` удалены.

## Проверка

Реальный вывод:

- `cargo test --release -p app --bin cs2cinema` — 18 passed, 0 failed; среди них `startup_kills_the_orphan_before_cleanup_and_marks_the_job_interrupted ... ok`, `orphan_with_another_creation_time_is_left_alone ... ok`, `malformed_processes_json_kills_nothing ... ok`, `app::tests::old_logs_are_listed_in_attempt_order ... ok`
- `cargo build --release --workspace` — Finished, предупреждений и `error[` нет
- `cargo test --release -p cli --test pipeline` — 2 passed
- Проверка порядка шагов `kill_orphans -> recover -> reset_running` (awk из плана) — код 0
- `test ! -f crates/app/src/launch.rs`, `grep -rn 'launch::' crates/app/src` — 0, `grep -rniE 'tasklist|taskkill' crates` — 0, `grep -c "mod launch" main.rs` — 0

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] rustfmt по `main.rs` форматирует и соседние модули**
- **Found during:** Task 2
- **Issue:** `rustfmt` на `main.rs` обходит объявленные модули и переформатировал нетронутые `mock.rs`, `player.rs`, а в `ui.rs` и `match_page.rs` — посторонний код.
- **Fix:** посторонние изменения откачены (`git checkout -- ` для файлов, которые я не правил по существу; для `ui.rs` и `match_page.rs` правки повторены точечно). В коммит попали только осмысленные строки.
- **Files modified:** crates/app/src/ui.rs, crates/app/src/match_page.rs
- **Commit:** 50bf151

Остальное - план выполнен как написан (переименование `running_job` → `running_job_in` предусмотрено планом).

## Known Stubs

None.

## Threat Flags

None. Поведение завершения процессов не менялось (T-04-17: только `gamedir::kill_orphans` → `Host::kill_orphan` с полной идентичностью, порядок закреплён тестами и проверкой по тексту).

## Self-Check: PASSED

- `crates/app/src/launch.rs` отсутствует (ожидаемо); `jobs.rs`, `app.rs`, `settings.rs`, `ui.rs` содержат перенесённые функции.
- `932df8c` и `50bf151` в истории `main`.
- Реальный запуск приложения (кнопки «Найти CS2/HLAE», открытие папок без окна консоли) не проверялся — только сборка и тесты.

## Next

Ready for 04-08.
