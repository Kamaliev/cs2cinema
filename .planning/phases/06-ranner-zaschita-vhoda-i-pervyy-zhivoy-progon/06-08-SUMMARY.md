---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 08
subsystem: runner
tags: [rust, heartbeat, cancel, escalation, progress, tracer]
status: complete

requires:
  - phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
    provides: 06-06 — outbox runner_state.json, transfer.rs (докачка и загрузка), hello с holding, last_seq
  - phase: 05-yadro-servera-i-protokol
    provides: ручка progress (он же heartbeat), cancel_requested soft/force, stale_epoch/job_terminal
provides:
  - tee.rs — Shared, Tee (impl pipeline::Progress), Uplink (поток cs2c-uplink), SeqSource, FORCE_AFTER, Timing
  - пульс на всё время удержания задачи: этап и «дубль k из n» уходят на сервер, ответ cancel приходит в пайплайн
  - эскалация мягкой отмены сервера в жёсткую через FORCE_AFTER = 10 с
  - Control::stop (остановка раннера без закрытия задачи), Config.heartbeat/debounce/force_after
  - StopFn/StopWhy в transfer.rs: загрузка и скачивание прерываются между кусками при отмене и потере задачи
affects: [06-09 политика сбоев, 06-10 защищённый исполнитель и cs2-cli runner, 06-11 учения (D1: не позже 20 с), фаза 7 отмена из чата]

estimate:
  tokens: 80000
  raw_tokens: 80000
  tasks: 2
  confidence: low
actuals:
  tokens: 16900
  tasks: 2
  commits: 2
plan_head_before: 50c1a3c6dae0c94a1867c52f7d32bde9eb8ba610
plan_head_after: 3a6820d999c767d26130ab625198cd5b38102229

tech-stack:
  added: []
  patterns:
    - "Shared — общая доска задачи (Mutex<Board> + Condvar + атомики): пульс пишет на неё ответы сервера, Tee читает их как cancelled()/force_cancelled(), пайплайн пишет этап и дубль"
    - "Первый пульс несёт этап на момент запуска Uplink::start, а не на момент старта потока: сервер всегда видит fetch"
    - "Эскалация лениво в Tee::force_cancelled(): момент первой мягкой отмены + force_after, одна строка в журнал через swap флага"
    - "StopFn = &dyn Fn() -> Option<StopWhy> с приоритетом Lost > Cancelled > Local вместо одного AtomicBool"

key-files:
  created:
    - crates/runner/src/tee.rs
    - crates/runner/tests/heartbeat.rs
  modified:
    - crates/runner/src/lib.rs
    - crates/runner/src/link.rs
    - crates/runner/src/transfer.rs
    - crates/runner/tests/common/mod.rs
    - crates/runner/tests/outbox.rs

key-decisions:
  - "Uplink::start принимает Timing { interval, debounce } вместо двух отдельных Duration и ещё Arc-замыкание журнала: семь аргументов укладываются в лимит clippy::too_many_arguments"
  - "Строка об эскалации пишется один раз через inner.log: в раннере inner (RunnerProgress) и есть runner.log, второй вызов дал бы дубль строки"
  - "Токен отозван (401 в пульсе, hello или claim): раннер ставит флаг остановки с сообщением «токен раннера не принят сервером», задача остаётся в состоянии (не сбрасывается), в отличие от stale_epoch/job_terminal"
  - "Отмена сервера при уже готовом результате (исполнитель вернул Ok) прерывает загрузку на первой проверке: итог finish(Cancelled), файл результата и демка удаляются из outbox"
  - "Остановка раннера не отмена: исполнитель, вернувший отмену при поднятом флаге остановки без серверной отмены, оставляет задачу в этапе Executing; серверная отмена имеет приоритет над локальной остановкой"
  - "Пульс не запускается на этапе Reporting: итог уходит одним finish, пульс остановился до него; last_seq из Uplink::stop сохраняется в runner_state.json"

patterns-established:
  - "Любая новая передача файла получает StopFn, а не флаг: Local оставляет состояние, Cancelled закрывает finish(Cancelled), Lost сбрасывает без finish"

requirements-completed: [RUN-03, RUN-02]

duration: около 70 мин
completed: 2026-10-11
---

# Phase 6 Plan 08: пульс, отмена с сервера и эскалация Summary

**Поток пульса несёт этап и «дубль k из n» на сервер, ответ `cancel` за секунды доходит до пайплайна через `Tee`, а мягкая отмена, не доведённая до конца за 10 с, сама становится жёсткой; остановка раннера оставляет задачу, потерянная задача бросается без `finish`.**

## Performance

- **Duration:** около 70 мин
- **Completed:** 2026-10-11
- **Tasks:** 2 (трассёр и TDD-задача)
- **Files modified:** 7

## Accomplishments

- Трассёр `server_sees_stage_and_take_and_cancel_reaches_the_executor` на настоящем `server::serve`: сервер по порядку показывает `fetch`, `precheck`, `record` и «дубль 2 из 3»; мягкая `core.cancel(id, false)` доходит до исполнителя быстрее секунды, задача `cancelled`, в состоянии раннера задачи нет, `force_cancelled()` за всё время ложно.
- `force_cancel_on_the_server_sets_force_cancelled`: жёсткая отмена сервера делает `force_cancelled()` истинным быстрее секунды.
- `soft_cancel_escalates_to_force_within_the_bound`: заглушка, игнорирующая мягкую отмену, получает `force_cancelled()` не позже 1,5 с после `cancel` (пульс 200 мс, `force_after` 300 мс); в `runner.log` ровно одна строка «мягкая отмена не завершила задачу за … — закрываю игру принудительно». Обратный случай `soft_cancel_that_the_executor_honours_does_not_escalate` строки не оставляет.
- Модульный `escalation_needs_a_soft_cancel_and_the_bound` в `tee.rs`: без серверной отмены и при одной локальной остановке жёсткой отмены нет сколько ни жди, после мягкой — ложно до `force_after` и истинно после, `FORCE_AFTER == 10 с`.
- `local_stop_keeps_the_job_for_resume`: флаг остановки посреди работы даёт `Tick::Stopped`, задача на сервере не закрыта, в состоянии этап `Executing`; новый `Runner` в той же папке получает `keep`, исполнитель идёт с `resume = true` и `out_dir`.
- `lost_lease_stops_the_executor_and_drops_the_job_without_finish` на ручных часах сервера (`ManualClock`): аренда истекла, задачу забрал второй раннер (эпоха 2), следующий пульс первого получает `stale_epoch`, исполнитель видит жёсткую остановку, `Tick::Dropped`, чужая задача осталась `assigned`, в `runner.log` причина.
- `seq_keeps_growing_after_restart_in_the_same_epoch`: seq прошлого раннера на час впереди часов, после перезапуска сервер всё равно принимает новый этап (мутация `SeqSource::new(0)` ронит тест), `last_seq` сохраняется при остановке пульса.
- `cancel_during_upload_stops_between_chunks_and_closes_the_job` и `upload_stops_between_chunks_when_asked`: отмена сервера в начале загрузки закрывает задачу `finish(Cancelled)` и удаляет результат из outbox; прямой вызов `upload_result` с функцией остановки, отменяющей перед вторым куском, оставляет на сервере ровно один кусок (`CHUNK_SIZE`).
- Все тесты `runner` зелёные (9 модульных, `flow` 6, `heartbeat` 9, `outbox` 8), `cargo test --release --workspace` зелёный; `heartbeat` 25 раз подряд под нагрузкой на восьми занятых потоках без сбоев.

## Task Commits

1. **Task 1: сервер видит этап и дубль, отмена доходит до исполнителя (трассёр)** - `8540420` (feat)
2. **Task 2: эскалация, остановка раннера, потеря задачи, seq, отмена при загрузке (TDD)** - `3a6820d` (test)

Production-код Task 2 (`StopFn`/`StopWhy`, `FORCE_AFTER` и эскалация, `Control`, обработка остановки и потери) вошёл в коммит Task 1: эти части сплетены в `lib.rs` и `transfer.rs`, раздельно их не собрать без сломанной сборки. В коммите Task 2 остались тесты. Красной фазы TDD у Task 2 не было; поведение проверено мутацией `SeqSource::new(0)`.

## Files Created/Modified

- `crates/runner/src/tee.rs` — `Shared`, `Tee`, `Uplink`/`Pulse`, `SeqSource`, `Timing`, `FORCE_AFTER`, `TOKEN_REJECTED`, `phase_of`, три модульных теста
- `crates/runner/src/lib.rs` — пульс на всё время удержания задачи (`drive`, `start_uplink`, `stop_uplink`), `Tee` для исполнителя, `Control`, `Config.heartbeat/debounce/force_after`, обработка остановки, потери и отмены, 401 останавливает `run`
- `crates/runner/src/link.rs` — `Link: Clone`
- `crates/runner/src/transfer.rs` — `StopWhy`, `StopFn`, варианты `TransferEnd::{Cancelled, Lost}`, пауза и проверки между кусками через `StopFn`
- `crates/runner/tests/common/mod.rs` — `Stand::start_manual`, `advance`, `sweep`, `second_token`, `core`, `wait_until`, заглушки `WaitForCancel`, `IgnoreSoftCancel`, `StopMidway`, `FinishThenCancel`
- `crates/runner/tests/heartbeat.rs` — девять тестов
- `crates/runner/tests/outbox.rs` — вызов `upload_result` с функцией остановки вместо флага

## Decisions Made

См. `key-decisions` выше.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Первый пульс мог не нести `fetch`**
- **Found during:** Task 1, прогон под нагрузкой
- **Issue:** поток пульса читал доску при старте потока; под нагрузкой этап уже успевал смениться на `precheck`, и сервер никогда не видел `fetch`.
- **Fix:** `Uplink::start` снимает состояние синхронно, первый пульс уходит с ним, а смену этапа помечает как «изменилось» (уйдёт через `debounce`).
- **Files modified:** `crates/runner/src/tee.rs`
- **Committed in:** `8540420`

**2. [Rule 3 - Blocking] Сигнатура `Uplink::start` и тест `outbox`**
- **Issue:** вместе с журналом аргументов выходило восемь (лимит `clippy::too_many_arguments` — семь); `outbox.rs` передавал флаг в `upload_result`.
- **Fix:** `Timing { interval, debounce }` одним аргументом; в `outbox.rs` передаётся `&|| None`.
- **Committed in:** `8540420`

### Отличия от текста плана

- Тест `cancel_during_upload_stops_between_chunks_and_closes_the_job` вынужден отменять задачу до загрузки (заглушка `FinishThenCancel`): на localhost 20 МиБ уходят быстрее, чем тест успеет отменить между кусками. Прерывание именно между кусками проверяет отдельный `upload_stops_between_chunks_when_asked` (функция остановки отменяет перед вторым куском, сервер принял ровно один кусок).
- Остановка раннера и потеря задачи в `execute` проверяются после возврата исполнителя; потеря стоит впереди остальных итогов, даже если исполнитель вернул `Ok`.

**Total deviations:** 2 авто-исправления (Rule 1, Rule 3), 2 отличия формы. **Impact on plan:** поведение плана сохранено.

## Issues Encountered

- Python в окружении нет (заглушка Microsoft Store); правки через `Edit`, `sed` и `perl`. Первый скрипт `perl` испортил кодировку кириллицы (нет `use utf8`), строки исправлены вручную до сборки.
- Оболочка не разбирает heredoc с апострофами в теле; крупные вставки сделаны через `Write`.
- Тест этапов на сервере сначала мигал под нагрузкой (см. Deviation 1); оставшийся запас во времени: `debounce` 300 мс и пауза исполнителя 600 мс.

## Known Stubs

None.

## Threat Flags

None. Новых сетевых поверхностей нет: пульс идёт в ту же ручку `progress`. Угрозы плана закрыты: T-06-35 (эскалация `FORCE_AFTER`, тесты `soft_cancel_escalates_to_force_within_the_bound`, `soft_cancel_that_the_executor_honours_does_not_escalate`, `escalation_needs_a_soft_cancel_and_the_bound`), T-06-36 (текст состояния через `one_line`/`redact`, не длиннее 120 символов), T-06-37 (`stale_epoch`/`job_terminal` → жёсткая остановка и сброс без `finish`, тест `lost_lease_stops_the_executor_and_drops_the_job_without_finish`).

## Deferred Items

- Живое подтверждение границы «CS2 закрывается не позже 20 с после `job cancel`» (учение D1) — 06-11; здесь граница проверена на заглушке (пульс + `force_after`).
- Полная политика итогов (`release`/`finish`/пауза) — 06-09; сейчас неотменный отказ исполнителя обрабатывается как в 06-05. Метод `Control::resume()` добавит 06-09.
- Вторая часть эскалации (сам `quit` и `kill_confirm` в `pipeline::record::shutdown`) не менялась: она вызывается пайплайном по `force_cancelled()`.
- Предупреждение `clippy::result_large_err` на `Executor::run` осталось с 06-05.

## Next Phase Readiness

`Tee` и `Uplink` готовы для настоящего исполнителя 06-10 (дочерний разбор, предпроверка, `pipeline::job::run_job` получает `&dyn Progress` и сам видит серверную отмену и эскалацию); `Control::stop` готов для потока консоли и Ctrl+C. Политика итогов 06-09 может опираться на `Tick::{Stopped, Dropped}` и признак отозванного токена.

## Self-Check: PASSED

- FOUND: crates/runner/src/tee.rs, crates/runner/tests/heartbeat.rs
- FOUND commits: 8540420, 3a6820d
- `cargo test --release -p runner` и `cargo test --release --workspace` зелёные
