---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 08
subsystem: app
tags: [rust, app, queue, thread, catch_unwind]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::job::run_job, Host/FakeHost, recover_after_crash в app::jobs (04-04..04-07)"
provides:
  - "app::queue: Queue, QueueView, QueueConfig, Cmd, Runner, PauseReason, start — очередь в потоке cs2cinema-queue"
  - "app::jobs::runner(settings, host) — исполнитель задачи поверх run_job; JobReporter доступен очереди"
  - "Msg::JobStarted; окно читает App.queue.view() и шлёт queue.resume()"
affects: [04-09, 04-10, 04-11]

actuals:
  tokens: 7000
  tasks: 2
  commits: 2

plan_head_before: 464e08948cb57b3e08a3a94b3826e227c073602a
plan_head_after: ea2b85aef53fcbe42e7b3e0037e2cefa93495ffa
commits: 2

coverage:
  - deliverable: "Сбой одной задачи не останавливает очередь, итог типизированного сбоя доходит до строки задачи (RET-05)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#a_failed_job_does_not_stop_the_queue"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#real_runner_failure_reaches_the_row_and_the_queue_goes_on"
        status: pass
  - deliverable: "Паника внутри задачи: статус «ошибка» с «внутренняя ошибка: …», очередь идёт дальше (T-04-18)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#a_panicking_job_fails_and_the_queue_goes_on"
        status: pass
  - deliverable: "Отмена и задача, прерванная падением, ставят очередь на паузу до «Продолжить»; resume снимает любую паузу"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#cancel_pauses_the_queue_until_resume"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#interrupted_start_waits_for_resume"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#resume_clears_any_pause"
        status: pass
  - deliverable: "Очередь идёт, пока окно спрятано в трей и не перерисовывается (T-04-20)"
    human_judgment: true
    rationale: "Продвижение очереди от кадров окна не зависит (поток сам берёт задачи на каждом такте), но живая проверка со скрытым окном не делалась — это учение D6 плана 04-11; спайк A1 (04-03) не проверен"
  - deliverable: "Сборка workspace и cs2-cli"
    human_judgment: false
    verification:
      - kind: command
        ref: "cargo build --release --workspace"
        status: pass
      - kind: command
        ref: "cargo test --release -p cli"
        status: pass

key-files:
  created:
    - crates/app/src/queue.rs
  modified:
    - crates/app/src/jobs.rs
    - crates/app/src/app.rs
    - crates/app/src/main.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/settings_page.rs

key-decisions:
  - "Cmd содержит только Resume: отдельный Wake не нужен — постановка задачи и «Повторить» идут через Queue::resume, остальное поток находит на такте (500 мс)"
  - "Resume обрабатывается и потоком по порядку, и сразу в виде: пауза от задачи, которая ещё заканчивалась в момент нажатия, не переживёт команду"
  - "Ошибки базы (next_queued/start_job) пишутся в окно один раз, пока текст не изменился, чтобы не засорять журнал на каждом такте"

requirements-completed: [RET-05]

duration: 25 min
completed: 2026-10-11
---

# Phase 4 Plan 08: очередь в своём потоке Summary

**Продвижение очереди ушло из кадров окна в поток `cs2cinema-queue` (`crates/app/src/queue.rs`): задача идёт через `jobs::runner` → `pipeline::job::run_job` под `catch_unwind`, паника становится «внутренняя ошибка: …», сбой и паника очередь не останавливают, отмена и прерванная задача ставят паузу до «Продолжить».**

## Что сделано

- **Task 1 (tracer), `53f29c5`:** новый `queue.rs` (`Queue`, `QueueView`, `QueueConfig`, `Runner`, `start`); протокол одной задачи перенесён из `jobs::run_next` без изменений (разделитель попытки, `JobStarted`, `JobReporter`, `close_stage`, `finish`, открытие ролика). `jobs::runner(settings, host)` читает `Settings::load` для каждой задачи; прежние `run_next`/`pauses_queue` и тест `only_a_cancelled_job_pauses_the_queue` удалены. `App` получил `queue`, `Reporter` — `Clone`, `Msg::JobStarted`; `JobsState` без `running`/`paused`/`paused_reason`; `enqueue`, `resume_queue`, `cancel_job`, `run_precheck`, `poll`, страницы «Задачи» и «Настройки» читают `app.queue.view()`.
- **Task 2, `ea2b85a`:** политика пауз (`Cancelled` ставится потоком до `JobDone`, `resume()` снимает любую паузу), три теста пауз, строка состояния о паузе после отмены берётся из `QueueView`. Код политики был написан вместе с Task 1 (общий `run_one`), поэтому коммит Task 1 был собран без трёх тестов пауз, а коммит Task 2 их добавил.

## Проверка

Реальный вывод:

- `cargo test --release -p app --bin cs2cinema queue` — 6 passed (5 прогонов подряд без нестабильности); среди них `a_panicking_job_fails_and_the_queue_goes_on ... ok`, `cancel_pauses_the_queue_until_resume ... ok`
- `cargo test --release -p app --bin cs2cinema` — 23 passed, 0 failed
- `cargo build --release --workspace` — Finished, предупреждений нет
- `cargo test --release -p cli` — 3 + 2 + 3 passed (включая `tests/pipeline.rs`)
- `grep -c "fn run_next\|pauses_queue" crates/app/src/jobs.rs` = 0; `grep -rn run_next crates/app/src` = 0; `grep -c "pub running:\|pub paused:" crates/app/src/app.rs` = 0

## Deviations from Plan

**1. [Rule 3 - упрощение] `Cmd::Wake`/`Queue::wake` и поле `id` у `Msg::JobStarted` не введены**
- **Issue:** все места постановки задачи (`enqueue`, «Повторить», «Заново») вызывают `resume()`, который сам будит поток; `wake()` и `id` нигде не читались и давали предупреждения dead_code.
- **Fix:** `Cmd` состоит из `Resume`; `Msg::JobStarted { title }`. Поведение из плана сохранено.
- **Files modified:** crates/app/src/queue.rs, crates/app/src/app.rs

**2. [Rule 3 - Blocking] rustfmt по `main.rs` переформатировал соседние модули**
- **Found during:** Task 1 (та же ловушка, что в 04-07)
- **Fix:** `match_page.rs`, `mock.rs`, `player.rs`, `ui.rs` откачены через `git checkout -- <файл>` до коммита; в коммиты попали только мои файлы. Дальше rustfmt вызывался точечно на `queue.rs` и `app.rs`.

Остальное - план выполнен как написан. `pause_banner` оставлен принимающим `PauseReason` по значению (`Copy`), чтобы тест `pause_banner_names_the_reason` остался без изменений.

## Known Stubs

None.

## Threat Flags

None. T-04-18 (паника) закрыта `catch_unwind` и тестом; T-04-19: вид читается без `unwrap` (отравленный замок — значения по умолчанию), ошибки базы идут в окно; T-04-20 закрыта по построению (поток не зависит от кадров), живая проверка — учение D6 (04-11).

## Self-Check: PASSED

- `crates/app/src/queue.rs` существует; `53f29c5` и `ea2b85a` в истории `main`.
- Спайк A1 (очередь при скрытом в трей окне) не проверялся и от него ничего не зависит; реальный запуск приложения не делался — только тесты и сборка.
- Не затронуты и не попали в коммиты: `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `03-PATTERNS.md`, `04-PATTERNS.md`.

## Next

Ready for 04-09.
