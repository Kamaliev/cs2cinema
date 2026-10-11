---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 05
subsystem: pipeline
tags: [rust, record, tests, fake-host, d17, cancel]

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "Scenario::truncated_take, launch_error, windows_at, game_dies_at, game_appears, ignore_quit (03-02); TAKE_INVALID и проверка дублей в машине (03-04)"
provides:
  - "Тест every_d17_scenario_ends_with_its_own_code: таблица из десяти сценариев D-17, код каждого различен и записан в record.log"
  - "Тест launcher_spawn_error_fails_with_launcher_failed_and_still_writes_record_log"
  - "Тест cancel_in_every_state_returns_cancelled_closes_the_session_within_ten_seconds: отмена в Launcher, Game, Console, Demo, Script, Take, Closing"
affects: [03-06, 03-07, 03-08]

actuals:
  tokens: 2750
  tasks: 2
  commits: 2

plan_head_before: 70927d2
plan_head_after: f93ff96ff64c78ff6f224712b20ca22bca09619b
commits: 2

tech-stack:
  added: []
  patterns:
    - "таблица сценариев сбоя: структура D17 { название, fn(&Path) -> Scenario, verify, cancel_after, код } вместо десяти копий теста"

key-files:
  created: []
  modified:
    - crates/pipeline/tests/record.rs

key-decisions:
  - "Сценарий в таблице D-17 — указатель fn(&Path) -> Scenario: папка clips известна только после fresh(), а замыкания без захвата не нужны Box"
  - "Для Game сценарий держит машину в состоянии через game_appears(10 с), для Launcher — launcher_never_exits, для Closing — ignore_quit"

patterns-established:
  - "Отмена по состоянию: Capture::cancelling_after(state).watching(&fake) и проверка fake.now() - cap.cancelled_at() <= 10 с"

requirements-completed: [TST-02]

coverage:
  - id: D1
    description: "Десять сценариев D-17 (лаунчер, игра, окно ошибки HLAE, консоль, демка, скрипт, маркер дубля, обрезанный дубль, смерть игры, отмена) заканчиваются попарно различными кодами, и в record.log каждого есть строка «код: ...»"
    requirement: TST-02
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#every_d17_scenario_ends_with_its_own_code"
        status: pass
    human_judgment: false
  - id: D2
    description: "Ошибка запуска лаунчера самим хостом даёт LAUNCHER_FAILED в состоянии Launcher, сообщение называет ошибку хоста, запусков 0, record.log с итогом"
    requirement: TST-02
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#launcher_spawn_error_fails_with_launcher_failed_and_still_writes_record_log"
        status: pass
    human_judgment: false
  - id: D3
    description: "Отмена в любом состоянии лестницы возвращает CANCELLED с этим состоянием, закрывает сессию, укладывается в 10 с виртуального времени и оставляет итог в record.log"
    requirement: TST-02
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#cancel_in_every_state_returns_cancelled_closes_the_session_within_ten_seconds"
        status: pass
    human_judgment: false
---

# Phase 3 Plan 05: Таблица сценариев D-17 и отмена в каждом состоянии Summary

**Десять сценариев сбоя записи (D-17) прогоняются на `FakeHost` одной таблицей без CS2: у каждого свой код ошибки и `record.log` с итогом, а отмена проверена в каждом состоянии лестницы.**

## Что сделано

- **Task 1 (tracer, `a6982f6`).** `every_d17_scenario_ends_with_its_own_code`: структура `D17` и таблица из десяти строк с параметрами сценариев из существующих тестов. Для каждой строки своя чистая папка, ожидаемый код (сообщение `assert_eq!` называет сценарий по-русски), строка «код: ...» в `record.log`, а в конце множество кодов имеет размер 10. `launcher_spawn_error_fails_with_launcher_failed_and_still_writes_record_log`: `HostError::Spawn` даёт `LAUNCHER_FAILED`, состояние `Launcher`, `fake.launches() == 0`, итог в `record.log`.
- **Task 2 (`f93ff96`).** `cancel_in_every_state_returns_cancelled_closes_the_session_within_ten_seconds`: таблица `(RecordState, Scenario)` для `Launcher`, `Game`, `Console`, `Demo`, `Script`, `Take`, `Closing`. Проверяется код `CANCELLED`, состояние сбоя, `fake.terminated() >= 1`, не более 10 с виртуального времени от отмены и «код: CANCELLED» в `record.log`. Отмена во время ffprobe уже покрыта `cancel_during_ffprobe_cancels_the_recording` (03-04), не дублируется.

Код записи не менялся: оба теста прошли на существующей реализации с первого запуска, настоящих дефектов таблицы не выявили.

## Deviations from Plan

### Auto-fixed Issues

Нет — Rules 1-3 не срабатывали.

### Прочее

- Сценарий «игра умерла посреди дубля» в таблице использует `game_dies_at(20 с)` из существующего теста: это внутри второго дубля, а не первого, как написано в `<behavior>` плана. Код тот же (`GAME_EXITED`); правило плана «параметры брать из существующих тестов» соблюдено.
- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как в 03-01..03-04).
- Леджер `gsd-plan-head-before-03-05` не создавался перед первым коммитом; `plan_head_before` взят вручную как HEAD на старте плана (`70927d2`), число коммитов (2) сверено с `git rev-list`.

## Проверка

- `cargo test --release -p pipeline --test record d17` — 1 passed.
- `cargo test --release -p pipeline --test record launcher_spawn_error` — 1 passed.
- `cargo test --release -p pipeline --test record cancel_in_every_state` — 1 passed.
- `cargo test --release -p pipeline --test record` — 50 passed.
- `cargo test --release --workspace` — 343 passed, 0 failed, включая `crates/cli/tests/pipeline.rs` (2 passed).
- `rustfmt --edition 2024 --check crates/pipeline/tests/record.rs` — чисто.

## Known Stubs

Нет.

## Threat Flags

Нет. Тесты пишут только во временные папки процесса (T-03-14, accept).

## Self-Check: PASSED

- FOUND: crates/pipeline/tests/record.rs (`every_d17_scenario_ends_with_its_own_code`, `launcher_spawn_error_fails_with_launcher_failed_and_still_writes_record_log`, `cancel_in_every_state_returns_cancelled_closes_the_session_within_ten_seconds`, `RecordState::Closing`, `launch_error(`, `truncated_take(`)
- FOUND commits: a6982f6, f93ff96
