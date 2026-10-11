---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 04
subsystem: security
tags: [rust, host, windows, job-object, memory-limit, sandbox]
status: complete

requires:
  - phase: 06-01/06-03
    provides: независимый план волны 1 (зависимостей нет)
provides:
  - host::limit_self_memory(bytes) -> SelfLimit — процесс сам ограничивает память Job Object'ом (ProcessMemoryLimit) и запрещает порождать процессы (ActiveProcessLimit = 1) (SEC-02)
  - host::SelfLimit::peak_bytes — пиковая память для калибровки лимитов на живом прогоне
  - host::quiet_crash_dialogs — SetErrorMode против аварийного диалога Windows
affects: [06-07 (дочерний cs2-cli inspect ограничивает себя в начале main), 06-11 (калибровка по peak_bytes)]

estimate:
  tokens: 45000
  raw_tokens: 45000
  tasks: 2
  confidence: low
actuals:
  tokens: 5500
  tasks: 2
  commits: 3
plan_head_before: 3a0612636794b307006dc5a129b4027d84b02a25
plan_head_after: 074295b40d2ca424e5b2259a2448c57c2bdf624d

tech-stack:
  added: []
  patterns:
    - "Самоприсвоение Job Object: ребёнок помещает сам себя в job, нового метода Host и окна гонки spawn -> assign нет"
    - "Handle job самоограничения не закрывается до конца процесса, KILL_ON_JOB_CLOSE не ставится"
    - "Тест перезапускает собственный бинарь с переменной окружения CS2C_SELF_LIMIT_CHILD"

key-files:
  created:
    - crates/host/src/win/sandbox.rs
    - crates/host/tests/self_limit.rs
  modified:
    - crates/host/Cargo.toml
    - crates/host/src/lib.rs
    - crates/host/src/win/mod.rs

key-decisions:
  - "Самоприсвоение Job Object вместо нового метода Host: проверено спайком, нет гонки"
  - "KILL_ON_JOB_CLOSE не ставится: закрытие последнего handle убило бы сам процесс"
  - "Размер лимита (6 ГиБ) задаёт вызывающий (06-07), здесь только механизм"

requirements-completed: [SEC-02]

duration: 15min
completed: 2026-10-11
---

# Phase 6 Plan 04: самоограничение памяти процесса Summary

Один вызов `host::limit_self_memory` помещает процесс в Job Object с лимитом памяти и запретом порождать процессы: в лимите процесс работает, за лимитом умирает за доли секунды, пик памяти доступен через `SelfLimit::peak_bytes`.

## Что сделано

- **Task 1 (tracer):** `win/sandbox.rs` (`Guard`, `limit_self_memory`: `CreateJobObjectW`, `JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, `AssignProcessToJobObject(GetCurrentProcess())`), кросс-платформенная оболочка `SelfLimit` / `limit_self_memory` в `lib.rs` (вне Windows — `HostError::Unsupported`), тест `allocation_within_the_limit_succeeds` (100 МиБ, выход 0) и `allocation_over_the_limit_kills_the_child_quickly` (600 МиБ, ненулевой код, быстрее 10 с).
- **Task 2 (TDD):** RED-коммит с тестами `peak_memory_is_reported_after_allocation` и `limited_process_cannot_spawn_children`; затем `Guard::peak_bytes` (`QueryInformationJobObject`, `PeakProcessMemoryUsed`), `quiet_crash_dialogs` (`SetErrorMode`), возможность `Win32_System_Diagnostics_Debug` у `windows-sys`, `SelfLimit::peak_bytes`, `host::quiet_crash_dialogs`, модульный тест `limit_self_memory_is_unsupported_off_windows` (`cfg(not(windows))`).

## Коммиты

- b74d238 feat(06-04): процесс сам ограничивает свою память Job Object'ом
- bdcdc1a test(06-04): пик памяти, запрет порождения, тихий диалог (RED)
- 074295b feat(06-04): пик памяти и тихий аварийный диалог

## Проверка

`cargo test --release -p host` зелёный (70 модульных + 5 в `self_limit`), `cargo test --release --workspace` зелёный. В `sandbox.rs` 11 комментариев `// SAFETY:` на 9 блоков `unsafe {`.

## Deviations from Plan

Task 1 писался без `quiet_crash_dialogs` и `peak_bytes` в тесте — эти части по плану относятся к Task 2, поэтому ребёнок в Task 1 вызывает только `limit_self_memory`.

**1. [Rule 1 - Bug] Парсинг `peak=` в тесте**
- **Found during:** Task 2
- **Issue:** при `--nocapture` libtest печатает `test child_entry ... ` без перевода строки, поэтому `peak=` оказывается в середине строки и `strip_prefix` его не находил.
- **Fix:** поиск через `split_once("peak=")`.
- **Files modified:** crates/host/tests/self_limit.rs
- **Commit:** 074295b

Больше отклонений нет.

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей: T-06-11 закрыт механизмом (лимит памяти, запрет дочерних процессов, подавление диалога); применять его в `cs2-cli inspect` будет план 06-07.

## Self-Check: PASSED

Файлы `crates/host/src/win/sandbox.rs` и `crates/host/tests/self_limit.rs` существуют, коммиты b74d238, bdcdc1a, 074295b присутствуют в истории.
