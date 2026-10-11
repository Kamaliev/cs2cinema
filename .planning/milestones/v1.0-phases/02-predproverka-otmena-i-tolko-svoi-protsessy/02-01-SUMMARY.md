---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 01
subsystem: host
tags: [rust, windows-sys, job-object, enumwindows, toolhelp, fake-host]

requires:
  - phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
    provides: крейт host (Job Object, OwnedProc, WinSession, FakeHost на виртуальных часах)
provides:
  - "Session::windows: видимые окна процессов своего job (класс, заголовок, текст Static у #32770, до 300 символов)"
  - "Session::wait_exit: подтверждение выхода своей игры по handle срезами 100 мс"
  - "OWN_GAMES + Host::wait_previous_game: своя игра, пережившая сессию, остаётся с открытым handle (Clear / Stuck / Cancelled)"
  - "Host::processes / free_space / file_version (WinHost, UnsupportedHost по умолчанию, FakeHost из сценария)"
  - "Строители сценария фейка: windows_at, exit_after_terminate, previous_game, take_dirs_on_start, job_empty_after_exit, processes, free_space, file_version, program"
affects: [02-03, 02-04, 02-05, pipeline::record, предпроверка, отмена, детектор окна ошибки HLAE]

actuals:
  tokens: 15700
  tasks: 3
  commits: 3

plan_head_before: e258790ef500b6ae2ea7b0fcd25c0894e4e150e3
plan_head_after: 52c3649621b75461953dce287238917bbd99411c
commits: 3

tech-stack:
  added: []
  patterns:
    - "хост только сообщает факты; политика ожиданий и сбоев остаётся в pipeline"
    - "ожидание на handle срезами по 100 мс, mutex Shared.game держится один срез"
    - "реестр своих процессов, общий на процесс (static Mutex), хранит OwnedProc, а не PID"

key-files:
  created:
    - crates/host/src/win/windows.rs
    - crates/host/src/win/system.rs
  modified:
    - crates/host/Cargo.toml
    - crates/host/src/lib.rs
    - crates/host/src/fake.rs
    - crates/host/src/unsupported.rs
    - crates/host/src/win/mod.rs
    - crates/host/src/win/watch.rs
    - crates/host/tests/win_session.rs

key-decisions:
  - "Игра, не вышедшая к моменту Drop сессии, переезжает в static OWN_GAMES вместе с открытым OwnedProc: PID заново не открывается (T-02-02)"
  - "В трейте Host нет метода завершения по PID или имени; kill вызывается только своим handle внутри wait_previous_game"
  - "wait_exit подтверждает выход только при Exited по handle и пустом job; если процессы job живы к дедлайну, ответ Alive, ошибка опроса Unknown"
  - "Для игры, принятой без job, watch.rs после GameExited шлёт и JobEmpty, чтобы закрытие не ждало пустого job"

patterns-established:
  - "Фейк: Drop for FakeSession ведёт себя как WinSession (регистрирует оставленную игру в состоянии хоста)"
  - "Фейк: отложенный выход после terminate вычисляется по виртуальным часам, без sleep"

requirements-completed: [LCH-05, TMO-04, TMO-05, TMO-07, PRE-04, PRE-05, PRE-06, DIF-01]

duration: 10 min
completed: 2026-10-10
status: complete
---

# Phase 2 Plan 01: Факты ОС для предпроверки, отмены и детектора окна ошибки Summary

**Крейт host научился сообщать окна своего job, подтверждать выход своей игры по handle, помнить игру, пережившую сессию, и отдавать снимок процессов, свободное место и версию файла; фейк воспроизводит каждый факт на виртуальных часах.**

## Performance

- **Duration:** около 10 мин
- **Tasks:** 3 (1 tracer + 2 auto)
- **Files modified:** 9 (2 новых)

## Accomplishments

- `Session::windows`: `EnumWindows` + `GetWindowTextW` по PID из `Job::pids()` (плюс PID игры, принятой без job); невидимые служебные окна отбрасываются; для `#32770` собирается текст дочерних `Static`; строки обрезаются по `chars` до 300. Окнам не шлётся ни одного сообщения.
- `Session::wait_exit` и реестр `OWN_GAMES`: выход подтверждается по handle срезами не длиннее 100 мс; живая после `terminate` игра остаётся в реестре, `WinHost::wait_previous_game` ждёт её выхода, выдерживает паузу `settle` и сообщает `Stuck`, если игра жива через `limit` после завершения job.
- `Host::processes` (ToolHelp), `Host::free_space` (`GetDiskFreeSpaceExW`, откат к существующему предку), `Host::file_version` (`VS_FIXEDFILEINFO`); у `UnsupportedHost` работают тела по умолчанию, поэтому workspace собирается не на Windows.
- Фейк: окна по расписанию, выход после `terminate` (или никогда), игра прошлой сессии, папки дублей на `TAKE_START`, поздний или отсутствующий `JobEmpty`, снимок процессов, место, версия, ответы утилит по имени программы.

## Task Commits

1. **Task 1: Session::windows от Win32 до фейка** - `6fbcd4a` (feat)
2. **Task 2: wait_exit и реестр игр, переживших сессию** - `3b529c6` (feat)
3. **Task 3: процессы, свободное место, версия файла, ответы утилит** - `52c3649` (feat)

## Verification

- `cargo test --release -p host --lib`: 55 passed, 1 ignored (стенд-диалог).
- `cargo test --release -p host --lib -- --ignored modal`: 1 passed (диалог `wscript` с кириллицей найден по классу, заголовку и тексту).
- `cargo test --release -p host --test win_session`: 5 passed, включая `terminated_stand_in_game_is_confirmed_gone_by_wait_exit`.
- `cargo build --release --workspace` и `cargo test --release -p cli --test pipeline`: зелёные; `cargo test --release --workspace` без падений; `cargo clippy -p host --all-targets` без замечаний.
- `grep -rn SendMessage crates/host/src` пусто; в трейте `Host` нет методов `kill`/`terminate`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Объявления трейтов собраны в первом коммите**
- **Found during:** Task 1
- **Issue:** `WindowInfo`, `PrevGame`, `ProcInfo`, `Session::wait_exit` и `Host::wait_previous_game` лежат в одном `lib.rs`; разделить правки по задачам без интерактивного `git add -p` нельзя.
- **Fix:** все типы и объявления с телами по умолчанию вошли в коммит Task 1; реализации и тесты Task 2 и Task 3 идут отдельными коммитами. `Host::processes/free_space/file_version` добавлены в коммите Task 3.
- **Commit:** 6fbcd4a

**2. [Rule 1 - Bug] Комментарий в windows.rs содержал литерал `SendMessage`**
- **Found during:** Task 1
- **Issue:** критерий приёмки требует, чтобы `grep -rn SendMessage crates/host/src` ничего не выводил, а doc-комментарий модуля содержал это слово.
- **Fix:** комментарий переформулирован без литерала.
- **Commit:** 6fbcd4a

Других отклонений нет. Остальные детали плана выполнены как написано; `Drop for FakeSession` теперь вызывает `terminate`, поэтому `FakeHost::terminated()` учитывает и сессии, закрытые без явного `terminate` (тесты workspace это не затронуло).

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей сверх `threat_model` плана: окна читаются без сообщений (T-02-04), строки обрезаются до 300 символов (T-02-03), реестр держит только `OwnedProc` своих сессий (T-02-01, T-02-02).

## Next Phase Readiness

Планы 02-03, 02-04 и 02-05 получают все факты ОС. Открытое: реальное поведение `Stuck` для игры, найденной через job (handle без права завершения), проверено только на стенд-процессах; `kill` внутри `wait_previous_game` — одна попытка своим handle, у игр, открытых через `OwnedProc::open`, она вернёт ошибку ОС и не повлияет на ответ.

## Self-Check: PASSED

- FOUND: crates/host/src/win/windows.rs, crates/host/src/win/system.rs
- FOUND: 6fbcd4a, 3b529c6, 52c3649 (предки HEAD)
