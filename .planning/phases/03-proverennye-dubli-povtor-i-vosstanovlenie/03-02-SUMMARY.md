---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 02
subsystem: host
tags: [rust, windows-sys, process, fake-host, ffprobe, orphan]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: Job Object с KILL_ON_JOB_CLOSE, OwnedProc, adopted_without_job, страж «только свои процессы»
provides:
  - "host::ProcIdentity { pid, created (FILETIME), image }, OrphanOutcome, ORPHAN_IMAGES"
  - "Session::processes() — лаунчер с запуска, игра после GameAppeared; Host::kill_orphan(&ProcIdentity)"
  - "win::process::kill_if_same — сравнение PID, времени создания и образа и завершение на одном handle"
  - "Фейк: ответчик ffprobe по дублям, program_for, truncated_take, take_without_audio, video_size, finalize_lag, no_ffprobe_auto, orphan, runs/take_ends/orphan_kills, FakeSession::processes"
affects: [03-03, 03-04, 03-05, 03-07, 03-08]

actuals:
  tokens: 9159
  tasks: 2
  commits: 2

plan_head_before: 8fc28a81af577e30fd8221a2631c2b10ea42ad13
plan_head_after: 7c5470261324a473f845aa9dad599f47feae6c57
commits: 2

tech-stack:
  added: []
  patterns:
    - "завершение чужого по PID процесса возможно только через Host::kill_orphan(&ProcIdentity); страж own_processes ограничивает трейт одним таким методом"
    - "проверка идентичности и TerminateProcess идут на одном OwnedProc (handle держит объект процесса, PID не меняется под ним)"
    - "фейк отвечает на ffprobe только на вызовы с -show_entries, путь к файлу — последний аргумент; ответ строками JSON через on_line"

key-files:
  created: []
  modified:
    - crates/host/src/lib.rs
    - crates/host/src/win/process.rs
    - crates/host/src/win/mod.rs
    - crates/host/src/fake.rs
    - crates/host/tests/win_session.rs
    - crates/pipeline/tests/own_processes.rs

key-decisions:
  - "kill_if_same: вышедший процесс определяется до чтения образа — у вышедшего процесса QueryFullProcessImageName не работает, а время создания читается; порядок проверок: образ из списка, время создания, выход, имя образа, завершение"
  - "Фейк записывает TAKE_END в take_ends при доставке TakeFx::End независимо от clips; файл video.mp4 с содержимым partial (заготовка на старте дубля) читается как недописанный"
  - "runs() хранит полный путь программы как его передали в CmdSpec, а не только имя файла"
  - "Для сложного типа programs_for введён публичный псевдоним ProgramFor (clippy::type_complexity)"
  - "Pre-commit проверка защищённой ветки пропущена осознанно: branching_strategy=none, оркестратор прямо велел коммитить в main на основном дереве (так же, как в 03-01)"

patterns-established:
  - "Страж «нет завершения по PID или имени» уточнён, а не снят: один метод по &ProcIdentity без pid:/name в сигнатуре"

requirements-completed: [RET-04, TST-02]

coverage:
  - id: D1
    description: "Хост завершает осиротевший процесс только при совпадении PID, времени создания и образа из ORPHAN_IMAGES; чужой процесс и занятый другим PID остаются живы"
    requirement: RET-04
    verification:
      - kind: unit
        ref: "crates/host/src/win/process.rs#orphan_is_killed_only_when_pid_creation_time_and_image_match"
        status: pass
      - kind: integration
        ref: "crates/host/tests/win_session.rs#session_names_its_own_processes_and_refuses_to_kill_a_foreign_image"
        status: pass
      - kind: unit
        ref: "crates/pipeline/tests/own_processes.rs#host_trait_stops_a_process_only_by_its_full_identity"
        status: pass
    human_judgment: false
  - id: D2
    description: "Фейк отвечает как ffprobe по дублям и разыгрывает обрезанный, беззвучный, другого размера и дописываемый дубли без CS2"
    requirement: TST-02
    verification:
      - kind: unit
        ref: "crates/host/src/fake.rs#ffprobe_answers_for_a_finished_take_with_planned_length_and_window_size"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#truncated_take_and_missing_audio_are_reported_as_such"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#take_file_is_unreadable_until_the_finalize_lag_passes"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#args_matcher_wins_over_the_program_name"
        status: pass
    human_judgment: false
  - id: D3
    description: "Фейк записывает запуски утилит с виртуальным временем, моменты TAKE_END, идентичности процессов сессии и вызовы kill_orphan с исходами из сценария"
    requirement: TST-02
    verification:
      - kind: unit
        ref: "crates/host/src/fake.rs#runs_and_take_ends_are_recorded_with_virtual_time"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#session_names_launcher_and_then_the_game"
        status: pass
      - kind: unit
        ref: "crates/host/src/fake.rs#orphan_kills_are_recorded_with_scripted_outcomes"
        status: pass
    human_judgment: false
---

# Phase 3 Plan 02: Идентичность процессов и фейк ffprobe Summary

**Хост называет свои процессы полной идентичностью (PID + время создания + образ) и завершает осиротевший cs2.exe/HLAE.exe только при полном совпадении на одном handle; фейк отвечает как настоящий ffprobe по дублям и разыгрывает обрезанные, беззвучные и недописанные файлы.**

## Что сделано

- **Task 1 (tracer, `7f04a0d`).** `ProcIdentity`, `OrphanOutcome` (с русским `Display`), `ORPHAN_IMAGES = [cs2.exe, hlae.exe]`, `Session::processes`, `Host::kill_orphan` (по умолчанию `Unsupported`). `kill_if_same` открывает процесс с правом завершения, сверяет время создания и имя образа, завершает и ждёт до 3 с — всё на одном `OwnedProc`. `WinSession` хранит идентичность лаунчера (снимается сразу после запуска) и отдаёт игру из `Shared.game` после появления. Страж `own_processes` заменён на `host_trait_stops_a_process_only_by_its_full_identity`; первый страж (системные утилиты завершения по имени) не менялся.
- **Task 2 (`7c54702`).** Ответчик ffprobe в `FakeHost::run`: срабатывает на `-show_entries`, путь — последний аргумент, номер дубля из компонента `takeNNNN`. Сценарии: `truncated_take` (30% длительности), `take_without_audio` (пустые потоки), `video_size`, `finalize_lag` (код 1, `moov atom not found` / `Invalid data found when processing input` до TAKE_END + lag), нет файла (`No such file or directory`). Порядок выбора ответа: `programs_for` по подстроке аргумента, `programs` по имени, ответчик, `run` по умолчанию. `runs()` пишет и отменённые вызовы. `FAKE_PID` и `FAKE_LAUNCHER_PID` публичны.

## Контракт для 03-03/03-04/03-05

- Ответ ffprobe фейка: многострочный JSON `{"streams":[…],"format":{"duration":"5.000000"}}`, длительности — строки `{:.6}`; видео: `codec_type/codec_name(h264)/width/height/duration`, аудио: `codec_type(audio)/codec_name(pcm_s16le)/duration`. Дубль без звука: `"streams": []`, `format.duration` = `0.000000`.
- Размер кадра: `video_size`, иначе размер окна последнего `launch` (если не нулевой), иначе 1280x720.
- Код выхода ошибки ffprobe в фейке — 1; stderr_tail содержит `"{путь}: {сообщение}"`.
- Тест, которому нужны файлы, обязан дать `clips_dir`, иначе ответ — «No such file or directory».

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Порядок проверок в kill_if_same**
- **Found during:** Task 1 (тест повторного вызова)
- **Issue:** по плану образ сравнивался до проверки выхода; у вышедшего процесса `QueryFullProcessImageNameW` не работает, повтор давал `Mismatch` вместо `Gone`.
- **Fix:** проверка `liveness() == Exited` перенесена между сравнением времени создания и сравнением образа. Безопасность не страдает: до завершения всё равно сверяются время создания и образ.
- **Files modified:** crates/host/src/win/process.rs
- **Commit:** 7f04a0d

**2. [Rule 3 - Blocking] clippy::type_complexity у `Scenario::programs_for`**
- **Fix:** публичный псевдоним `ProgramFor`.
- **Files modified:** crates/host/src/fake.rs
- **Commit:** 7c54702

### Прочее

- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как и в 03-01).
- В плане не указано, как ответчик ведёт себя на заготовке `partial` (старт дубля при `take_dirs_on_start`): добавлено — такой файл читается как недописанный (`moov atom not found`).

## Проверка

- `cargo test --release -p host --lib` — 63 passed (1 ignored, как раньше); `--test win_session` — 6 passed.
- `cargo test --release -p pipeline --test own_processes` — 2 passed; `--test record` — 35 passed; `--test precheck` — 19 passed.
- `cargo test --release --workspace` — зелёный; `cargo build --release --workspace` — без ошибок; `cargo clippy -p host --all-targets` — без предупреждений.
- Живая проверка завершения осиротевшей игры после падения приложения — план 03-08 (по плану).

## Known Stubs

Нет.

## Threat Flags

Нет нового внешнего поверхностного риска сверх T-03-04/T-03-05 плана (mitigate выполнены: образ вне списка отвергается до открытия процесса, проверка и завершение на одном handle, тесты с чужим образом и другим временем создания, страж трейта).

## Self-Check: PASSED

- FOUND: crates/host/src/lib.rs (`pub struct ProcIdentity`, `fn kill_orphan`), crates/host/src/win/process.rs (`pub fn kill_if_same(`), crates/host/src/fake.rs (`pub fn program_for(`, `moov atom not found`), crates/pipeline/tests/own_processes.rs
- FOUND commits: 7f04a0d, 7c54702
