---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 07
subsystem: app
tags: [rust, egui, cancel, queue-pause, precheck, settings]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: "02-02: store.set_cancelling и JobStatus::Cancelling; 02-04: Progress::force_cancelled и запись, пропускающая quit; 02-05/02-06: precheck::run, jobs::precheck_inputs, jobs_page::precheck_view"
provides:
  - "Отмена выполняемой задачи сразу пишет статус «отменяется» (store.set_cancelling в потоке окна), итог «отменена» остаётся за исполнителем"
  - "JobsState.paused, App::resume_queue, jobs::pauses_queue: после отмены очередь ждёт «Продолжить»; сбой и успех паузу не включают"
  - "CancelToken::{force, is_forced}, JobReporter::force_cancelled: второе нажатие «Закрыть игру сразу»"
  - "Msg::Precheck, App::{precheck, precheck_busy, run_precheck}: раздел «Проверка окружения» в настройках на том же precheck::run"
affects: [02-08, ручная проверка отмены, очередь]

actuals:
  tokens: 5660
  tasks: 3
  commits: 3

plan_head_before: 8c32f7fe1a8f81951b4d87871e6151fe2e40fc38
plan_head_after: 992b7d26775d201764e65765cb16a0ea8b8d47b1
commits: 3

tech-stack:
  added: []
  patterns:
    - "статус «отменяется» пишет поток окна, итог пишет поток задачи: set_cancelling меняет только running, поэтому настоящий исход всегда последний"
    - "действие в настройках откладывается флагом, чтобы не конфликтовать с заимствованием app.draft"

key-files:
  created: []
  modified:
    - crates/app/src/app.rs
    - crates/app/src/jobs.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/settings_page.rs

key-decisions:
  - "Пауза очереди хранится только в памяти: после перезапуска приложения задачи из очереди продолжаются"
  - "Пауза включается только отменой выполняемой задачи; постановка новой задачи, «Повторить» и «Продолжить» её снимают"
  - "Второе нажатие — кнопка «Закрыть игру сразу» рядом с неактивной «Отменяется…»; на уровне App это повторный cancel_job по уже отменённому токену"
  - "Кнопка в настройках проверяет значения черновика (несохранённые) и неактивна во время задачи"

requirements-completed: [PRE-01, TMO-03, TMO-04, TMO-07]

duration: 25 min
completed: 2026-10-10
status: complete

coverage:
  - id: D1
    description: "Отмена выполняемой задачи переводит её в «отменяется» в том же кадре; очередь после отмены стоит до «Продолжить», после сбоя идёт дальше"
    requirement: "TMO-03"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#only_a_cancelled_job_pauses_the_queue"
        status: pass
      - kind: unit
        ref: "crates/store/src/lib.rs#job_lifecycle"
        status: pass
    human_judgment: true
    rationale: "Вид карточки и баннера egui и реакция за 3 с проверяются руками в 02-08"
  - id: D2
    description: "Второе нажатие «Закрыть игру сразу» доходит до исполнителя как Progress::force_cancelled"
    requirement: "TMO-04"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#force_implies_cancel_and_is_seen_by_the_reporter"
        status: pass
    human_judgment: true
    rationale: "Закрытие настоящей зависшей игры проверяется вручную в 02-08"
  - id: D3
    description: "Кнопка «Проверить окружение» запускает ту же precheck::run и показывает тот же отчёт; во время задачи неактивна"
    requirement: "PRE-01"
    human_judgment: true
    rationale: "Построитель входов общий с задачей (precheck_inputs) и покрыт тестами 02-06; сама кнопка и вид отчёта — ручная проверка 02-08"
---

# Phase 2 Plan 07: Отмена «отменяется» сразу, пауза очереди, «Закрыть игру сразу» и проверка окружения из настроек Summary

**Отмена выполняемой задачи сразу видна как «отменяется», очередь после неё ждёт «Продолжить», зависшую игру можно закрыть вторым нажатием, а окружение проверяется из настроек тем же `precheck::run`, что и перед записью.**

## Performance

- **Duration:** около 25 мин
- **Completed:** 2026-10-10
- **Tasks:** 3 (tracer + 2 auto)
- **Files modified:** 4

## Accomplishments

- Tracer (Task 1): `App::cancel_job` для выполняемой задачи ставит `token.cancel()` и `store.set_cancelling(id)` в том же кадре (D-11), затем перечитывает список. `Msg::JobDone` с `CANCELLED` включает `JobsState.paused` (`jobs::pauses_queue`) и пишет в строку состояния подсказку; `run_next` при `paused` ничего не запускает. `enqueue`, «Повторить» и `resume_queue` снимают паузу. На странице задач при паузе и ожидающих задачах показывается рамка «Очередь на паузе после отмены: N задач ждут» с кнопкой «Продолжить».
- Task 2: `CancelToken` держит два флага (`cancel`, `force`), `force()` ставит оба; `JobReporter::force_cancelled` отдаёт `is_forced()` драйверу записи (02-04), который пропускает `quit` и ожидание. `cancel_job` при уже отменённом токене вызывает `force()`; у карточки «отменяется» рядом с неактивной «Отменяется…» стоит `Закрыть игру сразу` с подсказкой.
- Task 3: `Msg::Precheck`, поля `precheck`/`precheck_busy`, `App::run_precheck` строит `Inputs` через `jobs::precheck_inputs(&draft, Demo::NotApplicable, last_good)` и вызывает `pipeline::precheck::run` в фоновом потоке. Раздел «Проверка окружения» в настройках: кнопка (неактивна при `precheck_busy` или выполняемой задаче, с подсказкой про чужой CS2), спиннер «проверяю…», подпись о несохранённых значениях, отчёт через `precheck_view` (раскрыт) или ошибка красным.

## Task Commits

1. **Task 1: Отмена — «отменяется» сразу, очередь ждёт «Продолжить»** - `2f59661` (feat)
2. **Task 2: Второе нажатие закрывает свой job сразу** - `7657bf7` (feat)
3. **Task 3: Кнопка «Проверить окружение» в настройках** - `992b7d2` (feat)

## Verification

- `cargo test --release -p app --bin cs2cinema`: 23 passed (в т.ч. `only_a_cancelled_job_pauses_the_queue`, `force_implies_cancel_and_is_seen_by_the_reporter`); `cargo test --release -p store`: 10 passed.
- `cargo build --release --workspace` и `cargo test --release --workspace` зелёные, `crates/cli/tests/pipeline.rs`: 2 passed.
- Ручная проверка отмены в каждом состоянии, второго нажатия, паузы и кнопки в настройках — план 02-08.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocker] Тестовый конструктор `Reporter::detached`**
- **Found during:** Task 2
- **Issue:** тесту `force_implies_cancel_and_is_seen_by_the_reporter` нужен `JobReporter`, а `Reporter` имеет приватные поля и создаётся только из окна.
- **Fix:** добавлен `#[cfg(test)] Reporter::detached()` (канал без получателя и пустой `egui::Context`).
- **Files modified:** `crates/app/src/app.rs`
- **Commit:** `7657bf7`

**2. [Rule 1 - Bug] Предупреждение `float_literal_f32_fallback`**
- **Found during:** Task 1
- **Issue:** `egui::Stroke::new(1.0, ..)` в новой рамке паузы давал будущую ошибку компилятора.
- **Fix:** `1.0_f32`, как в соседнем коде `settings_page.rs`.
- **Files modified:** `crates/app/src/jobs_page.rs`
- **Commit:** `2f59661`

Прочее: `rustfmt` по файлам app (команда плана) заодно переупорядочил часть `use` в `app.rs`; поведения это не меняет.

**Total deviations:** 2 auto-fixed (1 blocker, 1 bug). **Impact:** без изменения целей плана.

## Known Gaps (записаны, не закрыты)

- Пауза очереди живёт только в памяти (допущение плана): после перезапуска задачи из очереди стартуют сами.
- Реакция «отменяется» за 3 с и очистка за ~10 с на настоящей записи, второе нажатие на зависшей игре и вид баннера/раздела настроек проверяются вручную в 02-08.

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей сверх `<threat_model>` плана (T-02-24: `set_cancelling` меняет только `running`, итог пишет исполнитель; T-02-25: кнопка проверки неактивна, пока выполняется задача).

## Next Phase Readiness

02-08 — ручная проверка: отмена в каждом состоянии, «Закрыть игру сразу», пауза очереди и «Продолжить», кнопка «Проверить окружение».

## Self-Check: PASSED

- Файлы существуют: `crates/app/src/{app,jobs,jobs_page,settings_page}.rs`.
- Коммиты `2f59661`, `7657bf7`, `992b7d2` найдены в истории ветки.
