---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 06
subsystem: app-runner-e2e
tags: [runner, app, e2e, server-stand, fake-host, token, queue, recovery]

requires:
  - phase: 08-rezhim-rannera-v-prilozhenii
    provides: "08-01: RemoteHub, queue_runner, Stand; 08-02: Gate/Observer; 08-03: reset_running, after_kill; 08-04: RemoteRunner, token, start_queue; 08-05: карточка сервера в окне"
provides:
  - "testkit: AppStand::boot (порядок App::new на файловой базе и папке раннера), BootOpts, Recordings (счётчик одновременных записей), gated_local, gated_job_fn, after_upload_cut, after_kill_without_dir, put_token"
  - "remote/e2e.rs: семь сквозных проверок четырёх критериев фазы 8 на server::serve и FakeHost"
affects: [09]

actuals:
  tokens: 11650
  tasks: 3
  commits: 3
plan_head_before: 56c3202b6245e5a0dffc7afc229e05caf04e9eba
plan_head_after: 4728fbb6fbe47bcb51cfd9881690df0f35e63d19

tech-stack:
  added: []
  patterns:
    - "Сквозной стенд приложения без окна: Store::open(файл) -> recover_after_crash -> RemoteHub -> start_with_hook -> RemoteRunner::apply, повторяет App::new построчно"
    - "Одна запись за раз проверяется общим счётчиком Recordings, который считает и локальную заглушку очереди, и job_fn удалённой карточки"
    - "Поиск секрета по сырым байтам всех файлов и по Debug-строкам; перед поиском доказывается, что токен был во входе (файл токена) и что сам поиск не холостой"

key-files:
  created:
    - crates/app/src/remote/e2e.rs
  modified:
    - crates/app/src/remote/testkit.rs
    - crates/app/src/remote/mod.rs

key-decisions:
  - "Без папки задачи (убит до того, как пайплайн её назвал) раннер начинает запись заново с предпроверкой демки, один раз: продолжать нечем (так задумано в runner/src/lib.rs, тест фиксирует поведение)"
  - "cargo fmt для крейта app не запускался: репозиторий отформатирован с широкой строкой, а rustfmt.toml нет, прогон переписал бы чужие файлы; новый код оформлен в стиле соседнего"

requirements-completed: [APP-02, APP-03, APP-04, APP-01]

coverage:
  - id: D1
    description: "Удалённая задача не берётся, пока идёт, ждёт или стоит на паузе локальная работа; локальная задача, поставленная во время удалённой, ждёт её конца; одновременно идёт не больше одной записи"
    requirement: "APP-02"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::e2e::one_recording_at_a_time_and_local_jobs_wait_their_turn"
        status: pass
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::e2e::paused_queue_keeps_server_jobs_out"
        status: pass
    human_judgment: false
  - id: D2
    description: "Приложение, убитое посреди записи или загрузки, само доводит задачу сервера до конца без повторной записи и без паузы очереди"
    requirement: "APP-03"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::e2e (killed_during_recording..., killed_during_upload..., closed_app_with_a_waiting_server_card_resumes_it)"
        status: pass
    human_judgment: false
  - id: D3
    description: "Токен раннера после отказа задачи, недоступного адреса и отозванного токена находится только в файле token"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::e2e::issued_token_is_found_only_in_its_file"
        status: pass
    human_judgment: false
  - id: D4
    description: "Без адреса сервера нет потока раннера и файлов в его папке; локальная очередь ведёт себя как в v1.0"
    requirement: "APP-04"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote::e2e::without_an_address_the_app_has_no_runner_and_behaves_like_v1"
        status: pass
      - kind: integration
        ref: "cargo test -p app queue; cargo test --release -p cli --test pipeline"
        status: pass
    human_judgment: false
  - id: D5
    description: "Живая проверка с настоящими CS2, HLAE и Telegram"
    requirement: "APP-01"
    verification: []
    human_judgment: true
    rationale: "Живой стенд автор отложил в фазу 9; в этой фазе всё идёт на server::serve в процессе теста и FakeHost"

duration: 13min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 06: Сквозные проверки режима раннера Summary

**Семь сквозных тестов на настоящем `server::serve` и `FakeHost` через сборку `AppStand`, повторяющую `App::new`: одна запись за раз, ожидание очереди, продолжение после убийства приложения, токен только в своём файле и приложение v1.0 без адреса сервера.**

## Performance

- **Duration:** ~13 мин
- **Started:** 2026-10-12T13:41Z
- **Completed:** 2026-10-12T13:55Z
- **Tasks:** 3 (tracer + 2 auto/tdd)
- **Files:** 1 создан, 2 изменены (только тесты и помощники `testkit`; код приложения и `crates/server` не менялись)
- **Commits:** 3 (`ea821fa`, `b0d5b0b`, `4728fbb`)

## Accomplishments

- `AppStand::boot` поднимает приложение в порядке `App::new`: файловая база, `recover_after_crash` (внутри `reset_running`), `RemoteHub`, `queue::start_with_hook` с паузой `Interrupted` при прерванной локальной задаче, `RemoteRunner::apply(адрес)`. Закрытие: `apply("")` и ожидание потока раннера до 10 с (также в `Drop`).
- `Recordings` с `enter`/`leave` считает одновременные записи для локальной заглушки (`gated_local`) и `job_fn` карточки (`gated_job_fn`); обе держат «запись» до сигнала теста, отмену автора или сервера принимают.
- **Критерий 2.** `one_recording_at_a_time_and_local_jobs_wait_their_turn`: пока идёт своя A, задача сервера 3 с остаётся `queued`, `grants == 0`, состояние `Gated «идёт задача приложения»`, раннер остаётся на связи; после A идёт карточка (`Working`), своя B, поставленная во время карточки, 1 с остаётся `queued`; порядок A, карточка, B, `max_concurrent == 1`. `paused_queue_keeps_server_jobs_out`: после отмены автором ворота называют «очередь приложения на паузе», после `resume()` задача доходит до `done`.
- **Критерий 3.** Убито посреди записи: `reset.interrupted == 0`, `interrupted_remote == 1`, пауза не ставится, карточка удержана (`hub.is_held`), запись продолжена один раз с `resume == true` в сохранённой папке без повторной предпроверки, в журнале «продолжение после перезапуска» и «результат принят сервером». Убито посреди загрузки: результат на сервере равен файлу, `job_fn`, предпроверка и игра не вызывались, карточка остаётся `Done`. Карточка, ждавшая за локальной задачей: своя `Interrupted`, пауза `Interrupted` как в v1.0, карточка удержана и ждёт, после `resume()` доходит до `done`, своя сама не стартует.
- **SEC-04 приложения.** `issued_token_is_found_only_in_its_file`: после отказа задачи, успешной задачи, недоступного адреса и отзыва токена значения токена нет ни в одном файле папки раннера (`runner.log`, `runner_state.json`, демки и ролики) кроме `token`, ни в байтах файлов базы, ни в `jobs()`, `text()`/`short()`/`Debug` каждого увиденного состояния, `settings.json`, `Debug` настроек и `runner::Config`, запусках программ `FakeHost` (`icacls`). Поиск проверен мутацией: подложенный `leak.txt` с токеном роняет тест.
- **Критерий 4.** `without_an_address_the_app_has_no_runner_and_behaves_like_v1`: нет потока и записей хаба, состояние `Off`, в папке раннера нет `runner.lock`, `runner.log`, `runner_state.json`; сбой запуска даёт ровно один авто-повтор с «авто-повтор» в журнале; отмена ставит паузу `Cancelled`; прерванная задача при следующем запуске даёт паузу `Interrupted`, «Повторить» доходит до исполнителя с `resume == true`.

## Task Commits

1. **Task 1 (tracer): стенд и сквозные проверки очереди** - `ea821fa` (test)
2. **Task 2: убитое приложение доводит задачу сервера до конца** - `b0d5b0b` (test)
3. **Task 3: токен только в файле, приложение v1.0 без адреса** - `4728fbb` (test)

Тракер проверен до расширения (оба теста Task 1 прошли), затем выполнены Task 2 и Task 3. Помощники Task 2 (`after_upload_cut`, `after_kill_without_dir`) добавлены вместе с остальными в коммите Task 1, потому что `<interfaces>` плана описывает их одним блоком.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug в тесте] Ожидание `resume == true` при убийстве до названия папки**
- **Found during:** Task 2
- **Issue:** в `closed_app_with_a_waiting_server_card_resumes_it` я ожидал, что `job_fn` вызовется с `resume == true`. Раннер при `Executing` без `out_dir` называет задачу не продолжением, а новым запуском (`runner/src/lib.rs`, `resumed_dir = held.out_dir.clone()` пусто), и предпроверка демки идёт заново. Это задуманное поведение: продолжать нечем.
- **Fix:** тест фиксирует его: `resumes == [false]`, ровно один вызов, ровно одна предпроверка, карточка доходит до `done`.
- **Files modified:** crates/app/src/remote/e2e.rs
- **Commit:** b0d5b0b

**Total deviations:** 1 (ошибка ожидания в новом тесте). **Impact:** дефектов в коде планов 08-01…08-05 сценарии не нашли, исправлений кода приложения и сервера нет.

## Notes

- TDD: Task 2 и Task 3 помечены `tdd="true"`, но проверяют поведение, которое построили планы 08-01…08-05, поэтому КРАСНОЙ фазы нет: тесты прошли сразу (кроме одного неверного ожидания выше), заглушек RED-коммитов нет. Подтверждение, что тесты не холостые: мутация поиска токена (подложенный файл) роняет `issued_token_is_found_only_in_its_file`; `closed_app...` упал на неверном ожидании и показал реальное поведение.
- `cargo fmt` не запускался: в репозитории нет `rustfmt.toml`, код оформлен с широкой строкой, и прогон переформатировал бы тысячи чужих строк.
- Коммиты сделаны на `main` по прямому указанию оркестратора (последовательный исполнитель без worktree), как и в планах 08-01…08-05.
- `crates/store/src/journal.rs`, `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock` не затрагивались (чужие правки рабочего дерева).
- Живой стенд (настоящие CS2, HLAE, Telegram) не входил в план: отложен в фазу 9.

## Known Stubs

None. Заглушки (`gated_local`, `gated_job_fn`, `FakeHost`) существуют только в `#[cfg(test)]`.

## Threat Flags

None - новых сетевых точек и границ доверия нет.

## Verification

- `cargo test --release -p app --bin cs2cinema remote::e2e`: 7 passed; прогнан 3 раза подряд после последнего изменения теста, каждый раз зелёный (~6.4 с).
- `cargo test -p app queue`: ok, 31 passed.
- `cargo test --release -p cli --test pipeline`: ok, 2 passed.
- `cargo test --release -p server --test deps`: ok, 2 passed; `git log --oneline --grep "(08-" -- crates/server` пуст, `git diff 56c3202 -- crates/server` пуст.
- `cargo test --release --workspace`: все группы `ok`, всего 938 passed, 0 failed, 1 ignored (повторный прогон тот же). `cargo test --release -p host --test win_session`: 6 passed (известный флак здесь не проявился).
- Критерии: `testkit.rs` содержит `pub(crate) struct AppStand` и `max_concurrent`; `remote/mod.rs` содержит `mod e2e;`; `e2e.rs` содержит все семь тестов.

## Self-Check: PASSED

- Файлы: crates/app/src/remote/e2e.rs, testkit.rs, mod.rs - найдены.
- Коммиты ea821fa, b0d5b0b, 4728fbb - в истории `main`.
