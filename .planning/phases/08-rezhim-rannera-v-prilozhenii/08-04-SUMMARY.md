---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 04
subsystem: app-runner
tags: [runner, app, settings, token, acl, gate, observer, std-thread, server-stand]

requires:
  - phase: 08-rezhim-rannera-v-prilozhenii
    provides: "08-01: RemoteHub, RemoteExecutor, queue_runner, стенд Stand/Rig; 08-02: ворота Gate, наблюдатель Observer, RunnerEvent, Runner::holding, Control; 08-03: set_held_ref, жизненный цикл карточки"
provides:
  - "Settings.runner_server (по умолчанию пусто)"
  - "remote::token: TOKEN_FILE, validate, load, exists, save (пустой файл -> icacls -> значение), clear"
  - "remote::control: RemoteRunner (apply/tick/status/is_running/busy/pending/resume_paused/restart/save_token/clear_token/has_token), RemoteDeps, RemoteTuning, RemoteStatus (text/short/level), StatusLevel, admission, app_factory, inspect_exe, runner_dir"
  - "App::new: хаб и queue_runner через start_with_hook, RemoteRunner::apply(&s.runner_server); remote.tick() каждый кадр"
affects: [08-05, 08-06]

actuals:
  tokens: 30000
  tasks: 3
  commits: 5
plan_head_before: 5df165167ae23d4c6d9fe6f3b5d460605fe4b5ce
plan_head_after: db4a255895bec2932f1089592ad637b2c3e7fa43

tech-stack:
  added: []
  patterns:
    - "Поток раннера приложения (cs2cinema-runner) владеет замком runner.lock (File::try_lock, общий с cs2-cli runner); поколение состояния отсекает запоздавшие события остановленного потока"
    - "Смена адреса или токена при открытой карточке сервера откладывается (pending) и применяется в tick после закрытия карточки: остановка раннера отменила бы идущую карточку"
    - "Токен: пустой файл -> сужение доступа (icacls /inheritance:r /grant:r пользователь:F, не-Windows 0600) -> значение; при отказе файла нет"

key-files:
  created:
    - crates/app/src/remote/token.rs
    - crates/app/src/remote/control.rs
  modified:
    - crates/app/src/settings.rs
    - crates/app/src/remote/mod.rs
    - crates/app/src/remote/testkit.rs
    - crates/app/src/app.rs
    - crates/app/src/queue.rs

key-decisions:
  - "Хаб и queue_runner подключаются к очереди всегда: адрес можно задать без перезапуска, а без адреса поток раннера не создаётся и сетевых запросов нет"
  - "Отказы до запуска потока (нет cs2-cli, нет токена, токен не читается) определяются в apply без потока; замок и адрес (Runner::new) - внутри потока; порядок: NoCli, затем токен"
  - "busy() = hub.has_slots(): открытая карточка - единственное, что нельзя прерывать остановкой раннера; остановка при загрузке результата безопасна (задача остаётся в состоянии и продолжится)"
  - "Ошибки замка (создание папки, открытие файла) показываются как BadAddress: отдельного состояния для них в контракте нет"

requirements-completed: []  # APP-01, APP-02, APP-03, APP-04 покрыты кодом и тестами; отметка выставляется после проверки фазы (решение оркестратора)

coverage:
  - id: D1
    description: "С адресом сервера и токеном приложение само поднимает поток раннера, забирает задачу сервера и доводит её до done через свою очередь"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema address_and_token_make_the_app_take_a_server_job"
        status: pass
    human_judgment: false
  - id: D2
    description: "Ворота раннера закрыты при идущей, поставленной на паузу или ждущей авто-повтора очереди, при локальных задачах, открытой карточке сервера и пустых путях HLAE/CS2"
    requirement: "APP-02"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema admission_closes_for_every_busy_queue_state"
        status: pass
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema local_job_closes_the_gate_and_the_server_job_waits"
        status: pass
    human_judgment: false
  - id: D3
    description: "Без адреса сервера поток раннера не создаётся, очередь ведёт локальные задачи как в v1.0"
    requirement: "APP-04"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema empty_address_starts_no_runner_and_the_queue_works_as_before"
        status: pass
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema settings_without_runner_server_still_load"
        status: pass
    human_judgment: false
  - id: D4
    description: "Токен хранится только в файле с суженным доступом; не удалось сузить - токена нет (D-02)"
    requirement: "APP-01"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema remote::token (saved_token_loads_back_and_the_acl_is_narrowed_first, failed_acl_leaves_no_token_file, invalid_token_is_not_written, clear_removes_the_token)"
        status: pass
    human_judgment: true
    rationale: "Права файла на живой машине проверены вручную одним вызовом icacls на временном файле (остался только пользователь с (F)); целиком путь через окно настроек - <human-check> плана 08-05"
  - id: D5
    description: "Состояние раннера по-русски называет каждый отказ; смена адреса не рвёт карточку сервера; итог выдачи пишется в журнал карточки"
    requirement: "APP-02"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::control (refusals_before_start_name_their_reason, saving_a_token_restarts_the_runner, second_runner_in_the_same_folder_is_refused, revoked_token_shows_token_rejected, address_change_waits_for_the_server_card, server_outcome_is_written_into_the_card_journal, status_texts_are_russian_and_short)"
        status: pass
    human_judgment: false
  - id: D6
    description: "Прерванная падением задача сервера не ставит очередь на паузу, строка состояния обещает продолжение (APP-03)"
    requirement: "APP-03"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema interrupted_server_job_resumes_after_restart_without_a_pause (08-03) + строка состояния в App::new при reset.interrupted_remote > 0"
        status: pass
    human_judgment: true
    rationale: "Строка состояния в App::new показывается только в окне; тестами покрыт сам механизм продолжения (08-03), текст проверяется глазами в плане 08-05"

duration: 12min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 04: Приложение как хозяин раннера Summary

**Приложение при заданных адресе и токене само поднимает поток `cs2cinema-runner` с воротами по виду очереди, наблюдателем состояния и журналом итогов выдачи; токен лежит только в файле с доступом автора; без адреса потока и запросов нет, очередь работает как в v1.0.**

## Performance

- **Duration:** ~12 мин
- **Started:** 2026-10-12T13:16:45Z
- **Completed:** 2026-10-12T13:29:11Z
- **Tasks:** 3 (tracer + 2 auto/tdd)
- **Files:** 2 созданы, 5 изменены
- **Commits:** 5 (`a0ca56e`, `c4b8598`, `1ff387f`, `647a476`, `db4a255`)

## Accomplishments

- `Settings.runner_server` (пусто - режим выключен); старые `settings.json` грузятся. Про токен в `Settings` ни слова (grep-проверка плана: 0 вхождений).
- `remote::control::RemoteRunner`: поток `cs2cinema-runner` ждёт прежний поток, занимает `runner.lock` (общий с `cs2-cli runner`), читает токен, собирает `runner::Config` с воротами и наблюдателем и крутит `Runner::run`. Ворота (`admission`) закрыты по шести причинам по порядку: идёт задача приложения, пауза, ожидание авто-повтора, ждущие локальные задачи, открытая карточка сервера, не заданы папки HLAE и CS2. Удалённая карточка, уже лежащая в хабе, не считается «локальной ждущей» - причину называет `remote_open`.
- `App::new` подключает `RemoteHub` и `queue_runner` через `queue::start_with_hook` всегда; `remote.apply(&s.runner_server)` при запуске; `remote.tick()` каждый кадр; при `reset.interrupted_remote > 0` строка состояния обещает продолжение. `queue::start` теперь только для тестов очереди.
- `remote::token`: `save` пишет пустой файл, сужает доступ (`icacls <файл> /inheritance:r /grant:r <USERNAME>:F` через `Host::run` с дедлайном 15 с, на не-Windows `0600`), и только потом значение; при любой ошибке файл удаляется. Ошибки не содержат значения токена. Живая проверка: тот же `icacls` на временном файле оставил единственную запись пользователя с `(F)`, код выхода 0.
- Отказы названы состояниями `RemoteStatus`: `NoToken`, `BadToken`, `NoCli`, `LockBusy`, `BadAddress`, `TokenRejected`, `ProtocolMismatch`; `text`/`short`/`level` по-русски.
- Отложенная смена: при живом потоке и открытой карточке `apply` запоминает адрес в `pending`, `tick` применяет его после закрытия карточки; `save_token`/`clear_token`/`restart` пользуются тем же правилом. Строка состояния один раз сообщает «адрес применится, когда закончится задача с сервера».
- Журнал карточки получает строки итога выдачи: «результат принят сервером; локальная копия...», «итог отправлен на сервер: сбой/отменена», «задача возвращена в очередь сервера...», «сервер снял задачу: ...».

## Task Commits

1. **Task 1 (tracer): приложение само работает раннером сервера** - `a0ca56e` (feat)
2. **Task 2 RED: тесты токена и заглушки** - `c4b8598` (test)
3. **Task 2 GREEN: токен с суженным доступом** - `1ff387f` (feat)
4. **Task 3 RED: тесты отказов, отложенной смены, журнала и заглушки** - `647a476` (test)
5. **Task 3 GREEN: состояния, отложенная смена, журнал карточки** - `db4a255` (feat)

Тракер проверен до расширения: четыре теста `remote::control` (в том числе настоящий сервер + очередь + поток раннера) прошли, затем выполнены Task 2 и Task 3.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `queue::start` стал неиспользуемым в бинарнике**
- **Found during:** Task 1 (сборка)
- **Issue:** приложение теперь зовёт `start_with_hook`, `start` остался только для тестов очереди, и сборка выдала `dead_code`-предупреждение.
- **Fix:** `#[allow(dead_code)]` с комментарием на `queue::start`; сама функция и её тесты не менялись.
- **Files modified:** crates/app/src/queue.rs
- **Commit:** a0ca56e

**2. [Rule 3 - Blocking] Стенду нужна очередь без потока раннера**
- **Found during:** Task 1
- **Issue:** `Rig::start_with` сразу запускает свой поток раннера, а `RemoteRunner` строит собственный.
- **Fix:** выделен `testkit::start_queue(factory, opts)`, `Rig::start_with` пользуется им; `probing_host` получил ответ `icacls` с кодом 0 (для `save_token` в тестах).
- **Files modified:** crates/app/src/remote/testkit.rs
- **Commit:** a0ca56e

**3. [Rule 1 - Bug в тесте] `Working` приходит раньше карточки**
- **Found during:** Task 1
- **Issue:** событие `Working` идёт при взятии задачи, а карточка и запись хаба появляются после скачивания и предпроверки; проверка `busy()` сразу после `Working` падала.
- **Fix:** тест ждёт `busy()` отдельно (до 15 с).
- **Files modified:** crates/app/src/remote/control.rs
- **Commit:** a0ca56e

**Total deviations:** 3 auto-fixed (1 bug в тесте, 2 blocking). **Impact:** контракт `<interfaces>` сохранён: все имена и сигнатуры плана есть в коде. Добавлен лишний тест `local_job_that_finishes_reaches_done_with_the_hub_connected` (локальная задача в очереди с подключённым хабом).

## Notes

- Коммиты сделаны на `main` по прямому указанию оркестратора (последовательный исполнитель без worktree), как и в планах 08-01...08-03.
- Решение сверх плана: в `start` сначала проверяется `cs2-cli`, потом токен (порядок плана не задавал); ошибки замка (создание папки, открытие файла) показываются как `BadAddress`, так как отдельного состояния в контракте нет.
- Дедлайн остановки: раннер, стоящий в long-poll (`poll_secs: Some(5)` у приложения), выходит не позднее пяти секунд; новый поток ждёт его через `join`, окно не блокируется.
- Остановка раннера при загрузке результата (карточка уже закрыта, `busy() == false`) разрешена: задача остаётся в `runner_state.json` и продолжится после перезапуска потока (Keep).
- `crates/store/src/journal.rs`, `.planning/config.json`, `.planning/state.json` и `.planning/milestone.lock` не затрагивались (чужие правки рабочего дерева).

## Known Stubs

None. Заглушки RED-коммитов (`c4b8598`, `647a476`) заменены реализацией в следующих коммитах.

## Threat Flags

None - новых сетевых точек и границ доверия сверх `<threat_model>` плана нет (запуск `icacls` учтён как T-08-SC).

## Verification

- `cargo test --release -p app --bin cs2cinema remote::control`: ok, 12 passed.
- `cargo test --release -p app --bin cs2cinema`: ok, 75 passed (в том числе `remote::token` 5, `settings_without_runner_server_still_load`, все прежние `queue`).
- `cargo test --release -p cli --test pipeline`: ok, 2 passed.
- `cargo build --release -p app -p cli`: без ошибок и предупреждений в коде проекта.
- `git diff --stat -- crates/server`: пусто.
- Критерии: `grep -c "queue::start(" crates/app/src/app.rs` = 0; `grep -vE '^\s*//' crates/app/src/settings.rs | grep -ci token` = 0; в `token.rs` есть `/inheritance:r`, `/grant:r`, `0o600`; в `control.rs` есть `pub fn admission(`, `"cs2cinema-runner"`, `runner.lock`, `LockBusy`, `NoCli`, `TokenRejected`, `pub fn pending(`, `pub fn save_token(`, `job_by_external_ref(`.
- `cargo test --workspace --release`: `test result` - ok во всех группах, всего 925 passed, 0 failed, 1 ignored (повторный прогон - то же).

## Self-Check: PASSED

- Файлы: crates/app/src/remote/token.rs, crates/app/src/remote/control.rs, settings.rs, mod.rs, testkit.rs, app.rs, queue.rs - найдены.
- Коммиты a0ca56e, c4b8598, 1ff387f, 647a476, db4a255 - в истории `main`.
