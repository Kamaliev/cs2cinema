---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 07
subsystem: security
tags: [rust, pipeline, cli, child-process, demo-parsing, sandbox]
status: complete

requires:
  - phase: 06-01
    provides: resolve_player, ensure_highlights, FailCode (PLAYER_*, NO_HIGHLIGHTS, BAD_DEMO, DEMO_TOO_LARGE)
  - phase: 06-03
    provides: read_demo_limited, DemoLimits, synth::demo, synth::zstd_bomb
  - phase: 06-04
    provides: host::limit_self_memory, SelfLimit::peak_bytes, quiet_crash_dialogs
provides:
  - pipeline::inspect::{InspectLimits, InspectArgs, InspectReport, inspect_demo, child_args, preflight, Preflight} — разбор чужой демки в дочернем процессе (SEC-02)
  - pipeline::analyze_with — типизированные BAD_DEMO / DEMO_TOO_LARGE; analyze осталась обёрткой
  - cs2-cli inspect — скрытая подкоманда: самоограничение памяти, сторож времени, одна JSON-строка
  - cs2-cli <демка> --player выбирает игрока тем же resolve_player, что и задача (SEC-03)
affects: [06-09 (PRECHECK_FAILED от ребёнка возвращает задачу в очередь), 06-10 (раннер зовёт preflight до run_job), 06-11 (калибровка по peak_mem_bytes)]

estimate:
  tokens: 110000
  raw_tokens: 110000
  tasks: 2
  confidence: low
actuals:
  tokens: 21000
  tasks: 2
  commits: 2
plan_head_before: 328485a6dd47b13ca8db56c10698a0f0074d6be9
plan_head_after: a04806af9534ec076f98927de52aad80ef525102

tech-stack:
  added: []
  patterns:
    - "Ребёнок отвечает одной JSON-строкой и кодом выхода 0 (и успех, и типизированный отказ); любой иной исход родитель читает как BAD_DEMO"
    - "Ответ ребёнка недоверенный: коды из разрешённого набора, тексты отказов игрока пересобираются родителем из очищенных кандидатов (не больше 10)"
    - "Сначала самоограничение (Job Object, сторож времени), потом чужой файл; паника парсера ловится catch_unwind и тоже даёт BAD_DEMO"

key-files:
  created:
    - crates/pipeline/src/inspect.rs
    - crates/cli/src/inspect.rs
    - crates/cli/tests/inspect.rs
  modified:
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/job.rs
    - crates/cli/src/main.rs

key-decisions:
  - "Тексты PLAYER_AMBIGUOUS и PLAYER_NOT_FOUND родитель собирает сам из очищенных кандидатов ребёнка (msg_player_*), а не берёт чужую строку"
  - "Не запустившийся ребёнок (HostError::Spawn и прочее, кроме таймаута и отмены) — PRECHECK_FAILED, а не BAD_DEMO: это неисправность окружения, не демки"
  - "Недоступный файл демки остаётся строкой с путём без типизации (как до фазы 6): это неисправность вызывающего, а не BAD_DEMO"
  - "В InspectReport добавлено поле more (сколько кандидатов не вошло), чтобы текст «и ещё N» не терялся при передаче"

requirements-completed: [SEC-02, SEC-03, RUN-05]

duration: 40 min
completed: 2026-10-11
---

# Phase 06 Plan 07: чужая демка разбирается там, где её можно убить Summary

**`cs2-cli inspect` разбирает демку в отдельном процессе с лимитом памяти и сторожем времени и печатает одну JSON-строку, а `pipeline::inspect::preflight` превращает любой исход ребёнка (падение, зависание, мусор, чужой код) в `BAD_DEMO` или типизированный отказ ещё до запуска CS2.**

## Выполнено

- **Задача 1 (tracer, `c5f001c`).** `pipeline::inspect`: `inspect_demo` (читает демку с потолками, `resolve_player`, `ensure_highlights`, число моментов и гранат, `duplicate_xuids`, при `flybys` — `positions::track`), `child_args`, `preflight` через `Host::run` с дедлайном `secs + 30` и отменой (берётся последняя строка stdout, начинающаяся с `{`). `analyze_with` типизирует `BAD_DEMO` и `DEMO_TOO_LARGE`; `run_job` читает демку с потолками. `cs2-cli inspect`: `quiet_crash_dialogs`, `limit_self_memory`, сторож времени (`process::exit(3)`), `catch_unwind`, `peak_mem_bytes`. Трассёр `inspect_child_answers_with_one_json_line_and_typed_refusals` зелёный.
- **Задача 2 (`a04806a`).** Тесты через собранный бинарь: zstd-бомба, файл сверх лимита, мусор и несуществующий файл, 20 усечений и 50 переворотов байта (каждый запуск короче 30 с, одна JSON-строка, допустимый код), Windows-тест потолка памяти, `--player` в обычном режиме через `resolve_player` (неоднозначный ник — код 1, «подходит нескольким игрокам»). Модульные тесты `preflight` на `FakeHost` написаны вместе с кодом в задаче 1: `preflight_sanitizes_a_hostile_child_answer`, `preflight_turns_foreign_codes_crashes_timeouts_and_garbage_into_bad_demo`, `preflight_keeps_precheck_failed_from_the_child`.

## Проверка

- `cargo test --release -p cli --test inspect` — 10 тестов; `-p pipeline --lib inspect` — 10 тестов; `cargo test --release --workspace` зелёный; `cargo build --release -p cli -p app` собирается; `cargo test --release -p cli --test pipeline` проходит.
- Живой запуск: ребёнок на несуществующем файле отвечает `BAD_DEMO` с `peak_mem_bytes` около 1,4 МиБ и кодом выхода 0.

## Deviations from Plan

**1. [Rule 1 - Bug] Windows-тест потолка памяти: ребёнок не умирает, а отвечает BAD_DEMO**
- **Found during:** задача 2
- **Issue:** план ждал смерти ребёнка при 8 МиБ бомбы и `--mem-mib 64`, но (а) 8 МиБ меньше лимита, (б) `read_to_end` при отказе выделения в Job Object возвращает `io::Error` «out of memory», и ребёнок сам отвечает `BAD_DEMO` с кодом 0.
- **Fix:** бомба 200 МиБ; тест принимает оба исхода (смерть без ответа или `BAD_DEMO` с пиком не выше 64 МиБ), а родитель в обоих случаях получает `BadDemo`. Путь «ребёнок умер» покрыт модульным тестом на `FakeHost` (код выхода `-1073740791`) и тестами `host/tests/self_limit.rs` из 06-04.
- **Files modified:** crates/cli/tests/inspect.rs
- **Commit:** a04806a

**2. [Rule 1 - Bug] Недоступный файл демки стал сбоем «демка не разбирается»**
- **Found during:** задача 2 (`cargo test --workspace`, `plan_only_job_skips_the_precheck_and_reports_a_missing_demo`)
- **Issue:** `analyze_with` типизировал любой `DemoError::Bad`, включая «файл не открывается», и терял путь в тексте и признак «это не сбой записи».
- **Fix:** перед чтением `analyze_with` проверяет `metadata` файла: недоступный файл — строка «путь: ошибка», как до фазы 6.
- **Files modified:** crates/pipeline/src/lib.rs
- **Commit:** a04806a

**3. [Rule 2 - Missing critical] Поле `more` в `InspectReport` и `PRECHECK_FAILED` при незапуске ребёнка**
- Список ответа ребёнка обрезан до 10, и без счётчика терялось «и ещё N»; не запустившийся ребёнок (`HostError::Spawn`, `Io`, `Unsupported`) иначе поставил бы очередь на паузу как «плохая демка» (Pitfall 1, наоборот): это окружение, а не демка.
- **Commit:** c5f001c

## Known Stubs

Нет.

## Threat Flags

Нет новой поверхности сверх `threat_model` плана. T-06-18, T-06-19, T-06-20 закрыты кодом и тестами (разбор в ребёнке с лимитом памяти и временем; разрешённый набор кодов, очистка и потолок кандидатов; `BAD_DEMO` класса «прочее» до `run_job`). T-06-21 закрыт `ActiveProcessLimit = 1` из 06-04, применяемым в `inspect_cmd`.

## Замечания

- Двойной разбор (ребёнок и затем `run_job`) — осознанная цена v1.1 по плану.
- Стартовые лимиты (файл 2000 МиБ, демка 3 ГиБ, память 6 ГиБ, время 900 с) ждут калибровки живым прогоном 06-11 по `peak_mem_bytes`.
- Вне Windows память не ограничивается (предупреждение в stderr), поэтому тесты потолков распаковки идут и на Linux, а тест памяти — только под `cfg(windows)`.
- Раннер (06-10) должен позвать `preflight(host, current_exe, &args, &cancel)` до `run_job`.
- Коммиты сделаны прямо в `main` (`branching_strategy: none`, подтверждено оркестратором).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/inspect.rs, crates/cli/src/inspect.rs, crates/cli/tests/inspect.rs, crates/pipeline/src/lib.rs, crates/pipeline/src/job.rs, crates/cli/src/main.rs
- FOUND commits: c5f001c, a04806a (предки HEAD)
