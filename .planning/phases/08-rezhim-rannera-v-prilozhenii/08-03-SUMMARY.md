---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 03
subsystem: app-runner
tags: [runner, queue, store, resume, cancel, auto-retry, server-stand]

requires:
  - phase: 08-rezhim-rannera-v-prilozhenii
    provides: "08-01: RemoteHub, RemoteExecutor, queue_runner, SettleHook, adopt_remote, стенд Stand/Rig"
  - phase: 06-ispolnitel-rannera
    provides: "runner_state.json, hello/Keep, Tee, Disposition"
provides:
  - "store: Reset.interrupted_remote, раздельный reset_running, cancel_if_queued"
  - "remote::hub: отмены сервера и автора в RemoteExecutor::run, RemoteHub::set_held_ref / is_held"
  - "стенд: RigOpts + Rig::start_with, after_kill, JobFnCalls, blocking_job_fn"
  - "тесты жизненного цикла карточки на настоящем сервере: перезапуск, четыре отмены, авто-повтор внутри выдачи, «Повторить» закрытой выдачи"
affects: [08-04, 08-05, 08-06]

actuals:
  tokens: 12000
  tasks: 3
  commits: 3
plan_head_before: 3b58d7ed7212f2b83989300496607da38431c4e2
plan_head_after: e3d25b33074fca2e0f5935274d3b643f3130c07b

tech-stack:
  added: []
  patterns:
    - "Удалённые строки jobs (external_ref IS NOT NULL) после падения не ставят паузу очереди: карточка прервана с обещанием «продолжится сама», обратно в очередь её возвращает только усыновление под замком хаба"
    - "Отмена не автором (сервер, потеря выдачи, остановка раннера) поднимает server_cancel в записи хаба; хук очереди снимает только паузу Cancelled"
    - "Признак started записи хаба: карточку исполняет поток очереди; ставится под замком хаба при взятии и снимается после сбоя без отмены"

key-files:
  created: []
  modified:
    - crates/store/src/lib.rs
    - crates/app/src/remote/hub.rs
    - crates/app/src/remote/testkit.rs
    - crates/app/src/remote/tests.rs

key-decisions:
  - "reset_running переводит в interrupted и удалённые queued: поток очереди после падения стартует раньше раннера и иначе взял бы карточку без записи в хабе (T-08-11)"
  - "Любое p.cancelled() в потоке раннера считается отменой не автором: пауза Cancelled снимается; остановка раннера остаётся в runner_state.json (Keep) и карточка усыновляется снова"
  - "Ждущая карточка закрывается потоком раннера: cancel_if_queued под замком хаба при отмене сервера, чтение строки раз в секунду при отмене или удалении автором"
  - "Повторное ожидание в авто-повторе: после сбоя без отмены started снимается, чтобы отмену автора в паузе перед повтором тоже поймал поток раннера"

requirements-completed: []  # APP-03, APP-01 покрыты; отметка выставляется после проверки фазы (решение оркестратора)

coverage:
  - id: D1
    description: "Падение приложения посреди удалённой задачи не требует действий автора: продолжение по папке без повторной предпроверки, ролик принят сервером, очередь не на паузе"
    requirement: "APP-03"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema interrupted_server_job_resumes_after_restart_without_a_pause"
        status: pass
      - kind: unit
        ref: "cargo test --release -p store remote_cards_are_reset_apart_and_promise_to_continue"
        status: pass
    human_judgment: false
  - id: D2
    description: "Отмена с сервера останавливает идущую карточку без паузы очереди и закрывает ждущую без запуска"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema server_cancel_stops_the_running_card_without_pausing_the_queue server_cancel_of_a_waiting_card_never_starts_it"
        status: pass
    human_judgment: false
  - id: D3
    description: "Отмена автором идущей карточки ставит паузу как в v1.0; отмена и удаление ждущей закрывают её и доходят до сервера"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema author_cancel_of_a_running_card_pauses_the_queue_and_cancels_on_the_server author_cancel_of_a_waiting_card_reaches_the_server"
        status: pass
    human_judgment: false
  - id: D4
    description: "Сбой запуска повторяется внутри одной выдачи: один итог и одна выдача на сервере"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema launch_failure_is_retried_inside_the_same_grant"
        status: pass
    human_judgment: false
  - id: D5
    description: "D-01: «Повторить» закрытой выдачи идёт локально без загрузки, начать заново нельзя; удержанная раннером карточка распознаётся"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema retry_of_a_closed_server_card_runs_locally_and_uploads_nothing closed_server_card_cannot_start_over held_reference_marks_the_card"
        status: pass
    human_judgment: false

duration: 45min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 03: Жизненный цикл карточки сервера Summary

**Карточка задачи сервера переживает падение приложения без действий автора, корректно отзывается отменами сервера и автора и повторяет сбой запуска внутри одной выдачи; всё доказано на настоящем `server::serve`.**

## Performance

- **Duration:** ~45 мин (включая продолжение после прерванной сессии)
- **Tasks:** 3 (tracer + 2 auto/tdd)
- **Commits:** 3 (`3a77f0d`, `824753f`, `e3d25b3`)

## Accomplishments

- `store`: `Reset.interrupted_remote`; `reset_running` в одной транзакции делит строки на локальные (`running` -> `interrupted` с прежним текстом и подсказкой «Повторить») и карточки сервера (`running` и `queued` с `external_ref` -> `interrupted` с текстом «продолжится сама» и пустой подсказкой); `cancelling` -> `cancelled` как раньше. `cancel_if_queued(id)` атомарно закрывает только ждущую карточку.
- Критерий 3 фазы: тест `interrupted_server_job_resumes_after_restart_without_a_pause` воспроизводит убитое приложение (`after_kill`: задача выдана, `runner_state.json` на этапе `Executing`, карточка запущена в файловой базе), новый запуск (`recover_after_crash` даёт `interrupted == 0`, `interrupted_remote == 1`) и показывает: раннер получает `Keep`, карточка усыновляется с `resume`, `job_fn` вызван один раз с `resume == true` и прежней папкой, предпроверки нет, ролик принят сервером, очередь ни разу не на паузе, следующая локальная задача идёт.
- `RemoteExecutor::run`: на каждом пробуждении смотрит `p.cancelled()` / `p.force_cancelled()` и раз в секунду строку карточки. Отмена сервера: `server_cancel` в записи, ждущая карточка закрывается `cancel_if_queued` под замком хаба без запуска, идущая получает `set_cancelling` и отмену через `CardProgress`; хук очереди снимает паузу `Cancelled`. Отмена или удаление ждущей карточки автором закрывает запись и отдаёт серверу `cancelled`. Отмена автором идущей карточки не менялась (пауза v1.0).
- `queue_runner` берёт запись через `RemoteHub::take` (поиск и пометка `started` под одним замком), после сбоя без отмены снимает `started`: авто-повтор остаётся внутри той же выдачи, сервер видит один итог и одну выдачу (`grants == 1`).
- `RemoteHub::set_held_ref` / `is_held`; D-01 закреплён тестами: «Повторить» закрытой выдачи идёт в локальный исполнитель, ничего не грузит на сервер и не меняет его итог; без `resume` карточка падает с текстом «начать её заново нельзя».
- Стенд: `RigOpts` + `Rig::start_with`, `after_kill`, `JobFnCalls`, `blocking_job_fn`.

## Task Commits

1. **Task 1 (tracer): удалённая задача, прерванная падением, продолжается сама** - `3a77f0d` (feat)
2. **Task 2: отмены сервера и автора, авто-повтор внутри выдачи** - `824753f` (feat)
3. **Task 3: «Повторить» закрытой выдачи, set_held_ref / is_held** - `e3d25b3` (feat)

Тесты каждой задачи вошли в её коммит вместе с реализацией (код `store` для Task 1 был написан прерванной сессией и доведён здесь: добавлены тесты `remote_cards_are_reset_apart_and_promise_to_continue` и `cancel_if_queued_touches_only_waiting_cards`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `started` оставался поднятым после сбоя перед авто-повтором**
- **Found during:** Task 2 (проектирование чтения строки карточки)
- **Issue:** при планировании «ждущая = не `started`» карточка в паузе перед авто-повтором считалась бы идущей, и отмена автором в эти 20 с не дошла бы до сервера (T-08-13).
- **Fix:** `queue_runner` снимает `started` после сбоя, не являющегося отменой; итог `Done` и `CANCELLED` его не снимают, чтобы хук и `Settled` дошли до записи.
- **Files modified:** crates/app/src/remote/hub.rs
- **Commit:** 824753f

**2. [Rule 3 - Blocking] Поиск записи и пометка `started` были двумя шагами**
- **Found during:** Task 2
- **Issue:** гонка «очередь взяла карточку в момент отмены» из плана требует общего замка.
- **Fix:** `RemoteHub::take` ищет запись и ставит `started` под замком `slots`; проверки в `RemoteExecutor` идут под тем же замком.
- **Files modified:** crates/app/src/remote/hub.rs
- **Commit:** 824753f

**Total deviations:** 2 auto-fixed (1 bug, 1 blocking). **Impact:** без изменений контракта плана; имена и сигнатуры из `<interfaces>` сохранены.

## Notes

- Коммиты сделаны на `main` по прямому указанию оркестратора (последовательный исполнитель без worktree); защита «ветка по умолчанию» из протокола коммитов по конфигурации проекта срабатывает на `main`, как и у планов 08-01 и 08-02.
- Тест Task 2 о гонке «очередь взяла карточку в момент отмены» не воспроизводится детерминированно: она решается замком хаба и `cancel_if_queued` (атомарно с `start_job` на уровне SQL); остаточное окно (очередь выбрала карточку через `next_queued` до отмены, но `start_job` ещё не вызван) закончится сбоем «задача уже закрыта», а серверу итог `cancelled` уже ушёл.
- `crates/store/src/journal.rs`, `.planning/config.json`, `.planning/state.json` и `.planning/milestone.lock` не затрагивались (чужие правки рабочего дерева).

## Known Stubs

None.

## Threat Flags

None - новых сетевых точек, путей доступа к файлам и границ доверия план не добавил.

## Verification

- `cargo test --release -p store`: ok, 31 passed.
- `cargo test --release -p app --bin cs2cinema`: ok, 57 passed (11 из них `remote::tests`, `queue` и `jobs::tests` без правок).
- `cargo test --release -p cli --test pipeline`: ok, 2 passed.
- `cargo test --workspace --release`: все строки `test result` - ok, FAILED нет.

## Self-Check: PASSED

- Файлы: crates/store/src/lib.rs, crates/app/src/remote/hub.rs, testkit.rs, tests.rs - найдены.
- Коммиты 3a77f0d, 824753f, e3d25b3 - в истории `main`.
- Критерии: `interrupted_remote`, `external_ref IS NOT NULL`, `pub fn cancel_if_queued(`, `cancel_if_queued(` и `set_cancelling(` в hub.rs, `pub fn set_held_ref(`, `pub fn is_held(` - найдены.
