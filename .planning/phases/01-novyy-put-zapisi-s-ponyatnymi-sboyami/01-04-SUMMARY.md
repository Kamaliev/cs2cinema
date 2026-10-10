---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 04
subsystem: recording
tags: [rust, pipeline, state-machine, deadlines, markers, fake-host]

requires:
  - phase: 01-01
    provides: "трейты Host/Session, SessionEvent, FakeHost с виртуальными часами и сценариями"
  - phase: 01-02
    provides: "renderer::MARKER_TAG/marker, TakePlan {index, game_secs, capture_fps}, nonce плана"
provides:
  - "pipeline::record::record(&dyn Host, &RecordRequest, &dyn Progress) -> Result<RecordReport, RecordFailure>"
  - "Machine::step(now, Input) -> Vec<Action>: чистая машина без ОС и часов; RecordState::{LADDER, code, from_code, title, journal}"
  - "Deadlines (default, fast, scaled 0.5-5.0, take_run по D-12, ffmpeg_for) — именованный дедлайн на каждое ожидание"
  - "markers::{Marker, parse_marker, ready_request, ready_direct, EXEC, is_demo_running_evidence, is_afxerror}"
  - "FailCode (9 кодов D-08, as_str, hint), RecordFailure {code, state, message, hint, elapsed, console_tail, launcher, game}, LauncherFact"
  - "Event {State, TakeDone, Fact} с Display; Progress::event с телом по умолчанию"
  - "pipeline зависит от host: pub use host; реэкспорт record-типов из корня"
affects: [01-06, 01-07, 01-08]

actuals:
  tokens: 18900
  tasks: 2
  commits: 3
plan_head_before: e1003ae3a1c3bdacadc885a53d0c3568b615f23b
plan_head_after: 968c3e1285730c9611ba3031344a6676e08948f2

tech-stack:
  added: []
  patterns:
    - "машина получает время аргументом (Duration от Host::now), драйвер ждёт только Session::poll(100 мс) и проверяет Progress::cancelled на каждом шаге"
    - "метка принимается только как целая строка CS2C <nonce> <вид> [n]; принятые метки запоминаются, повтор из второго канала молчит, метка не по порядку не двигает ни состояние, ни дедлайн"
    - "весь русский текст сбоев — функции msg_* в failure.rs; машина передаёт им секунды и номера"
    - "сессия обёрнута в Guard с Drop -> terminate(); liveness игры снимается до первого Terminate"

key-files:
  created:
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/record/machine.rs
    - crates/pipeline/src/record/markers.rs
    - crates/pipeline/src/record/deadlines.rs
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/tests/record.rs
  modified:
    - Cargo.lock
    - crates/pipeline/Cargo.toml
    - crates/pipeline/src/lib.rs

key-decisions:
  - "Машина отдаёт тайминги принятых дублей через Machine::timings(), а Event::TakeDone остаётся {n, m} (формат «записан дубль N/M» читает status.rs)"
  - "Метка не по порядку (включая ALL_DONE вместо TAKE_END) молча меняет только текст будущего сообщения «пришла метка X вместо Y» и одну строку журнала; повтор принятой метки игнорируется"
  - "Прямой запрос READY отправляется один раз через ready_grace после max(признак идущей демки, вход в Demo); exec highlights — один раз, потерянный exec ловит дедлайн SCRIPT"
  - "Closing без пустого job: GameExited + close_settle или game_close -> Terminate и успех; сбой SendFailed(quit) тоже заканчивается успехом"
  - "Лаунчер с кодом выхода None (неизвестен) считается сбоем LAUNCHER_FAILED, как любой код кроме Some(0)"

patterns-established:
  - "RED/GREEN в двух коммитах: test(01-04) с 7 падающими сценариями, затем feat(01-04) с каталогом сообщений"
  - "Каждый сценарий сбоя на FakeHost проверяет код, состояние, текст сообщения и elapsed против поля Deadlines"

requirements-completed: [LCH-01, LCH-02, LCH-03, TMO-01, TAK-01, TST-01]

coverage:
  - id: D1
    description: "Успешная запись M=3 на FakeHost проходит лаунчер -> игра -> консоль -> демка -> скрипт -> дубль 1/3..3/3 -> закрытие -> готово; журнал сначала пишет ожидаемый набор take0000..take0002 с video.mp4 и audio.wav; exec highlights ровно один раз"
    requirement: "LCH-01"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#fake_recording_walks_the_whole_ladder_and_returns_every_take"
        status: pass
    human_judgment: false
  - id: D2
    description: "Каждое ожидание ограничено своим полем Deadlines и падает с кодом D-08, состоянием и прошедшим временем: LAUNCHER_FAILED (код и таймаут), GAME_NOT_STARTED, CONSOLE_NOT_OPEN, DEMO_NOT_LOADED, SCRIPT_NOT_RUN, TAKE_TIMEOUT (TAKE_START и TAKE_END с номером дубля), GAME_EXITED"
    requirement: "TMO-01"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#missing_ready_marker_fails_with_demo_not_loaded_after_the_deadline"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#missing_script_marker_fails_with_script_not_run_and_exec_is_sent_once"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#missing_take_end_fails_with_take_timeout_naming_the_take"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#console_that_never_opens_fails_with_console_not_open"
        status: pass
    human_judgment: false
  - id: D3
    description: "Пропавший маркер — сбой, а не предупреждение; ALL_DONE не подменяет TAKE_END ни одного дубля, включая последний: дедлайн take_run идёт дальше, quit не отправляется, в Closing не переходим"
    requirement: "LCH-03"
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/machine.rs#all_done_never_completes_a_take"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#all_done_does_not_stand_in_for_a_lost_last_take_end"
        status: pass
    human_judgment: false
  - id: D4
    description: "Парсер принимает только целую строку с nonce задачи; эхо команды, трассировка mirv_cmd, чужой nonce, лишние токены и повтор из второго канала игнорируются"
    requirement: "LCH-03"
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/markers.rs#marker_parser_accepts_only_whole_lines_with_our_nonce"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#stale_nonce_echoes_and_duplicate_markers_are_ignored"
        status: pass
    human_judgment: false
  - id: D5
    description: "Отмена в любом состоянии даёт CANCELLED и вызывает Session::terminate(); множитель Deadlines::scaled(k) умножает всю таблицу, включая дедлайн дубля, и зажат в 0.5-5.0"
    requirement: "TMO-01"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#cancel_during_demo_load_returns_cancelled_and_terminates"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/deadlines.rs#scaled_multiplies_everything_including_the_take_deadline_within_the_bounds"
        status: pass
    human_judgment: false
  - id: D6
    description: "Поздний netcon: если демка уже идёт, а READY нет ready_grace, машина один раз просит метку напрямую; формат «записан дубль N/M» сохранён для status.rs"
    requirement: "LCH-01"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#late_console_gets_ready_through_the_direct_request"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#take_done_lines_keep_the_legacy_format"
        status: pass
    human_judgment: true
    rationale: "Что настоящая игра в таком случае пишет в консоль и сработает ли прямой echo, подтверждает спайк S3(e) плана 01-05; фейк кодирует представление автора о поведении"
  - id: D7
    description: "Карточка в окне показывает код, состояние, сообщение и подсказку сбоя (LCH-02): здесь только данные RecordFailure и Display; проводка в базу и UI — планы 01-07 и 01-08"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/pipeline/tests/record.rs (проверки message и hint в сценариях сбоев)"
        status: pass
    human_judgment: true
    rationale: "Видна ли причина сбоя в карточке задачи, оценивает человек в плане 01-07"

duration: 35min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 04: Машина записи поверх Host Summary

**`pipeline::record` — явная машина состояний записи (чистая `Machine::step`, драйвер с шагом ≤100 мс, таблица `Deadlines`, парсер меток с nonce, девять кодов сбоев D-08 с русскими сообщениями и подсказками), которая на `FakeHost` проходит успешную запись и падает по именованному дедлайну на каждой пропавшей метке.**

## Performance

- **Duration:** ~35 min
- **Completed:** 2026-10-10
- **Tasks:** 2 (1 tracer, 1 auto/tdd)
- **Files modified:** 9 (6 новых, `Cargo.toml`, `Cargo.lock`, `lib.rs` крейта pipeline)

## Accomplishments

- Машина `Machine::step(now, Input) -> Vec<Action>` без `Instant`, потоков и ввода-вывода. Таблица переходов плана реализована целиком: лаунчер (`Some(0)` — переход, иной код — сбой сразу, «ещё работает» — сбой по `launcher`), игра, консоль, демка, скрипт, дубли (ожидание начала и запись отдельно), закрытие. Отмена важнее дедлайнов.
- Драйвер `record()`: `Session::poll(100 мс)` — единственное ожидание, отмена проверяется на каждом шаге, сессия обёрнута в `Guard` с `terminate()` в `Drop`. До лаунчера пишется «ожидаю дублей: M — take0000..take{M-1}, в каждом video.mp4 и audio.wav».
- `Deadlines` с дедлайном дубля по D-12 (`game_secs * max(1, fps/60) * 2 + 30 с`), `scaled(k)` (зажат в 0.5-5.0, умножает и множители), `fast()` для тестов машины.
- `ALL_DONE` никогда не засчитывается вместо `TAKE_END`: в ожидании `TAKE_END i` это метка не по порядку; в сообщении сбоя «Дубль 3 из 3: метка TAKE_END не пришла за 40 с; пришла метка ALL_DONE вместо TAKE_END 2».
- 15 интеграционных сценариев на `FakeHost` и 14 юнит-тестов проходят за миллисекунды реального времени (150 секунд ожидания — на виртуальных часах).

## Task Commits

1. **Task 1: Запись на фейке — лестница и пропавший READY (tracer)** - `3de4fef` (feat)
2. **Task 2 RED: сценарии сбоев (7 из 15 падают)** - `8fb7cc6` (test)
3. **Task 2 GREEN: каталог сообщений, метки не по порядку, прямой READY** - `968c3e1` (feat)

**Tracer gate:** автоматический `<verify>` Task 1 перепроверен после форматирования — `Tracer verified end-to-end — expanding`.

## Files Created/Modified

- `crates/pipeline/src/record/mod.rs` — `record()`, `RecordRequest`, `RecordReport`, `TakeTiming`, `Event`, `Guard`
- `crates/pipeline/src/record/machine.rs` — `RecordState`, `Input`, `Action`, `Machine`
- `crates/pipeline/src/record/markers.rs` — `Marker`, `parse_marker`, запросы READY, признаки демки и `AFXERROR`
- `crates/pipeline/src/record/deadlines.rs` — `Deadlines`
- `crates/pipeline/src/record/failure.rs` — `FailCode`, `RecordFailure`, `LauncherFact`, каталог сообщений
- `crates/pipeline/tests/record.rs` — 15 сценариев (01-08 дописывает свои сюда же)
- `crates/pipeline/src/lib.rs` — `pub use host;`, `pub mod record;`, реэкспорт, `Progress::event`
- `crates/pipeline/Cargo.toml`, `Cargo.lock` — зависимость `host`

## Decisions Made

См. `key-decisions` во frontmatter. Главное: тайминги дублей отдаёт `Machine::timings()` (форма `Event::TakeDone` не меняется), метка не по порядку попадает только в сообщение и одну строку журнала, `exec highlights` один раз.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Тестирование workspace с запущенным spikes.exe**
- **Found during:** финальная проверка `cargo test --release --workspace`
- **Issue:** процесс `spikes.exe` (PID 42256, стенд спайков плана 01-01, запущен не мной) держал `target\release\examples\spikes.exe`, линковка примера падала с LNK1104. Процесс не трогал.
- **Fix:** workspace проверен как `cargo test --release --workspace --lib --bins --tests` (все тесты, кроме сборки примеров); `cargo build --release --workspace` проходит.
- **Files modified:** нет
- **Verification:** все тестовые бинарники workspace зелёные, включая `crates/cli/tests/pipeline.rs`

### Уточнения сверх плана (не отклонения от контракта)

- Добавлены юнит-тесты сверх обязательных: `closing_waits_for_job_empty_and_a_late_game_is_terminated_not_failed`, три теста `deadlines.rs`, два теста `failure.rs`, два дополнительных теста `markers.rs`.
- В сообщение `GAME_EXITED` для дубля добавлено «(дубль N из M)».
- Сообщения ожиданий меток (`DEMO_NOT_LOADED`, `SCRIPT_NOT_RUN`, `TAKE_TIMEOUT`) получают суффикс «; пришла метка X вместо Y», если машина видела метку не по порядку.
- `rustfmt` запускался только на новых файлах (`record/*`, `tests/record.rs`) с настройками по умолчанию; `lib.rs` пайплайна не переформатировался, чтобы не раздувать дифф.

**Total deviations:** 1 (Rule 3, окружение). **Impact:** нет влияния на код.

## Issues Encountered

- `cargo clippy` предупреждает `result_large_err` для `record()` (`RecordFailure` ≥128 байт). Сигнатура зафиксирована планом, `allow` не добавлялся (CI не запускает clippy); при желании `RecordFailure` можно упаковать в `Box` в 01-07/01-08.
- Предупреждения clippy в `renderer` уже были до плана.

## Known Stubs

Нет. `RecordRequest.out_dir` и `header` пока никем не читаются: их использует журнал `record.log` из плана 01-08; `RecordFailure.console_tail` пуст по границе планов, а `HLAE_ERROR` и строку фактов HLAE.exe/cs2.exe добавляет 01-08. Это зафиксировано в плане, не заглушка цели 01-04.

## Next Phase Readiness

- 01-06 (`WinHost`) и 01-07 (приложение) подключаются к `pipeline::record` и `Progress::event`; 01-08 заполняет `console_tail`, `launcher`/`game` в сообщении, `HLAE_ERROR` и `record.log`, дописывая сценарии в `crates/pipeline/tests/record.rs`.
- Блокер для плана 01-05/01-06: на машине автора запущен `spikes.exe` (PID 42256) и держит `target\release\examples\spikes.exe`.

## Self-Check: PASSED

Файлы `record/{mod,machine,markers,deadlines,failure}.rs` и `tests/record.rs` на месте; коммиты `3de4fef`, `8fb7cc6`, `968c3e1` входят в HEAD. Критерии приёмки Task 1 и Task 2 выполнены: `cargo test --release -p pipeline --test record` — 15 passed; `--lib record::` — зелёный; `grep` по `thread::sleep|Instant::now|EXEC_RETRY|retries` в `crates/pipeline/src/record` пуст; `cargo build --release --workspace` — код 0.
