---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 08
subsystem: recording
tags: [rust, pipeline, record-log, console-tail, failure-facts, fake-host]

requires:
  - phase: 01-04
    provides: "pipeline::record (Machine, RecordFailure, LauncherFact, FailCode с 9 кодами D-08, markers) и тесты на FakeHost"
  - phase: 01-05
    provides: "01-SPIKES.md: HLAE.exe выходит через ~3 с, игра живёт; pipe закрывается только при выходе игры; HLAE почти ничего не пишет в stdout/stderr"
  - phase: 01-06
    provides: "WinHost: метки с nonce только после ворот демки, Note о сброшенном раннем READY, netcon на 0.0.0.0 — недоверенный ввод"
provides:
  - "pipeline::record::log::RecordLog — record.log на дозапись в папке задачи (разделитель попытки с nonce, строки `+сек [источник] текст`, блок итога сбоя)"
  - "pipeline::record::log::Tail — хвост консоли: свёртка повторов «×N», кольцо 500, закреплены первая AFXERROR, самая частая повторяющаяся AFXERROR и последняя метка; render(30) не длиннее 30 строк"
  - "pipeline::record::facts_line(&LauncherFact, &Liveness) и failure::{code_text, msg_hlae_error, msg_game_appeared, msg_game_ended, msg_take_timing}"
  - "RecordFailure.console_tail заполняется, к message (кроме CANCELLED) дописывается « (HLAE.exe: … · cs2.exe: …)»"
  - "FailCode::HlaeError: дедлайн при уже увиденной AFXERROR"
affects: [01-07]

actuals:
  tokens: 11500
  tasks: 3
  commits: 6
plan_head_before: 9f2666c86232d659fee9d46f1a83c3c0dea9770b
plan_head_after: 06e50b608523d6a57997bca230685625fb4924ba

tech-stack:
  added: []
  patterns:
    - "Journal в драйвере record(): один объект держит record.log, Tail, счётчики строк HLAE и флаг netcon; каждое событие сессии, Emit машины и отправленная команда проходят через него"
    - "второй канал консоли (console.log при живом netcon) идёт в Tail::shadow: в кольцо не попадает, но первая AFXERROR и метка запоминаются"
    - "весь русский текст новых фактов — функции msg_* в failure.rs, машина передаёт им числа"
    - "в record.log и хвост пишется только очищенный текст: управляющие символы -> пробел, профиль C:/Users/<имя> -> USER, значения token=/password=/api_key=/Bearer скрыты"

key-files:
  created:
    - crates/pipeline/src/record/log.rs
  modified:
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/record/machine.rs
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/src/record/markers.rs
    - crates/pipeline/tests/record.rs

key-decisions:
  - "record.log открывается до host.launch, поэтому LAUNCHER_FAILED тоже оставляет журнал; не открылся — запись идёт без файла, Progress::log сообщает «record.log не записан: …»"
  - "stdout/stderr HLAE.exe считаются по SessionEvent::Line: «stdout — 0 строк, stderr — 0 строк» пишется при любом исходе как явный факт (спайки: HLAE почти молчит)"
  - "HLAE_ERROR срабатывает на любом истёкшем дедлайне (кроме закрытия), если машина видела AFXERROR из любого источника; сама AFXERROR сбой не вызывает (LCH-05 остаётся фазой 2)"
  - "Строка фактов добавляется и к сбою запуска лаунчера («HLAE.exe: не запускался · cs2.exe: не запускалась»); у отмены в message её нет, но в журнале она есть"
  - "Live-проверка игры берётся ДО первого Terminate (как в 01-04), поэтому «не жива» в сообщении — не следствие нашего terminate"

patterns-established:
  - "RED/GREEN в отдельных коммитах: test(01-08) с падающими тестами, затем feat(01-08)"

requirements-completed: [LCH-02, LCH-04, LCH-06, TMO-02]

coverage:
  - id: D1
    description: "record.log есть в папке задачи при успехе, сбое и отмене, дописывается с разделителем попытки (nonce) и содержит источники netcon/app, команды `→`, счётчики HLAE и итог с кодом, сообщением, подсказкой, прошедшим временем"
    requirement: "LCH-04"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#record_log_is_written_on_success_and_on_failure_with_source_tags"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#cancelled_recording_still_leaves_a_complete_record_log"
        status: pass
    human_judgment: false
  - id: D2
    description: "Хвост консоли: повторы свёрнуты, первая AFXERROR и шторм сохранены, строки двух каналов не дублируются, не больше 30 строк"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/log.rs#tail_tests::tail_folds_repeats_and_keeps_the_first_afxerror"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#failure_tail_contains_recent_console_lines"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#console_log_lines_are_not_doubled_in_the_tail_while_netcon_is_up"
        status: pass
    human_judgment: false
  - id: D3
    description: "Код HLAE.exe и живость cs2.exe — раздельные факты в журнале и в сообщении сбоя; Unknown выводится «неизвестно», не «жива»"
    requirement: "TMO-02"
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/failure.rs#tests::facts_line_prints_windows_codes_in_hex_and_unknown_as_unknown"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#unknown_liveness_is_reported_as_unknown_never_alive"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#game_exit_mid_take_reports_the_dead_game_with_its_code"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#journal_reports_launcher_code_and_game_liveness_separately"
        status: pass
    human_judgment: false
  - id: D4
    description: "Дедлайн при AFXERROR в консоли даёт HLAE_ERROR с первой строкой AFXERROR; каждый дубль пишет в журнал время записи против игрового"
    requirement: "LCH-06"
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#deadline_with_afxerror_in_tail_is_reported_as_hlae_error"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#journal_reports_launcher_code_and_game_liveness_separately"
        status: pass
    human_judgment: false

duration: 25min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 08: record.log, хвост консоли, факты лаунчера и игры Summary

**Сбой записи теперь объясняет себя сам: `record.log` в папке задачи при любом исходе (на дозапись, с разделителем попытки), хвост консоли без шума в `RecordFailure.console_tail`, строка «HLAE.exe: код 0 · cs2.exe: не жива (код 0xc0000005)» в сообщении и журнале, «неизвестно» вместо «жива» и код `HLAE_ERROR` при дедлайне на фоне `AFXERROR`.**

## Performance

- **Duration:** около 25 мин
- **Tasks:** 3 (1 tracer, 2 TDD)
- **Files changed:** 6 (1 новый, 5 изменённых), +918 / −17 строк
- **Тесты:** `pipeline --test record` — 24 passed (было 15), `record::log` — 2 unit, `record::failure` — +1 unit, `cargo test --release --workspace` зелёный, `cargo build --release --workspace` без ошибок

## Accomplishments

- **record.log (Task 1, tracer).** `RecordLog::open` (`create + append`, без обрезки, `create_dir_all`) пишет разделитель `==== запись · {header} · nonce {nonce} · дублей {M} ====`; `line` — `+0012.345 [источник] текст` с `flush` на каждую строку; `failure` — блок итога. Драйвер `record()` получил `Journal`: журнал открывается до `host.launch`, пишет события сессии с `Source::label()`, `Note` и состояние порта как `host`, `Emit` и команды (`→ exec highlights`) как `app`, считает строки `HlaeStdout`/`HlaeStderr`.
- **Хвост (Task 2).** `Tail` сворачивает подряд идущие повторы уже при добавлении, поэтому шторм из 800 `AFXERROR` занимает одну запись «… ×800» и не вытесняет остальное. Закреплены (если не попали в окно): первая `AFXERROR` записи, самая частая повторяющаяся `AFXERROR` и последняя метка `CS2C …`; окно сжимается ровно на число закреплённых, итог ≤ `max`. `console.log` при подключённом netcon идёт в `shadow` (в кольцо не кладётся).
- **Факты (Task 3).** `facts_line` строит «HLAE.exe: … · cs2.exe: …»; коды ≥ `0x8000_0000` печатаются как `0xc0000005`, `Unknown` — «неизвестно (…)». Машина эмитит факты о выходе лаунчера, появлении (PID) и выходе игры, строку «дубль 1/2: 5 с записи на 5.0 с игры (×1.0)» после каждого `TAKE_END` и `HLAE_ERROR` вместо кода дедлайна, если видела `AFXERROR`.

## Task Commits

1. **Task 1: record.log при любом исходе (tracer)** — RED `5b161f8` (test), GREEN `14114ee` (feat)
2. **Task 2: хвост консоли** — RED `6d9360e` (test; заглушка `Tail`), GREEN `f945767` (feat)
3. **Task 3: факты и HLAE_ERROR** — RED `b0326f2` (test; 5 падающих сценариев), GREEN `06e50b6` (feat)

**Tracer gate:** `<verify>` задачи 1 автоматический (`human_verify_mode=end-of-phase`, auto выключен), перепрогнан сквозным на FakeHost и после расширения: «Tracer verified end-to-end — expanding». `gate="blocking-human"` не задан.

**Plan metadata:** отдельный коммит `docs(01-08)` (SUMMARY, STATE, ROADMAP, REQUIREMENTS).

## Files Created/Modified

- `crates/pipeline/src/record/log.rs` — `RecordLog`, `Tail`, `clean`, `one_line`, `redact` (профиль и секреты), 2 теста
- `crates/pipeline/src/record/mod.rs` — `Journal`, использование `RecordRequest.out_dir`/`header`, заполнение `console_tail`, строка фактов, реэкспорт `facts_line`
- `crates/pipeline/src/record/machine.rs` — факты лаунчера/игры/времени дубля, память первой `AFXERROR`, `HLAE_ERROR`
- `crates/pipeline/src/record/failure.rs` — `facts_line`, `code_text`, `msg_hlae_error`, `msg_game_appeared`, `msg_game_ended`, `msg_take_timing`, тест
- `crates/pipeline/src/record/markers.rs` — `strip_timestamp` стал `pub`
- `crates/pipeline/tests/record.rs` — 9 новых сценариев

## Decisions Made

См. `key-decisions` во frontmatter.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] Скрытие профиля и секретов в record.log и хвосте**
- **Found during:** Task 1 (указание оркестратора: netcon на 0.0.0.0 недоверенный, `record.log` — redact профиль и секреты)
- **Issue:** в плане нет редакции; консоль может содержать `C:/Users/<имя>/…` и значения вида `token=…`, а хвост уходит в базу.
- **Fix:** `log::one_line`/`redact` применяются ко всем строкам журнала и к хвосту: управляющие символы -> пробел, `C:\Users\имя` -> `C:\Users\USER` (формат фикстур спайков), значения после `apikey/api_key/api-key/token/secret/password/passwd` и `Bearer` -> `***`. Консоль по-прежнему пишется дословно в остальном и без свёртки.
- **Files modified:** `crates/pipeline/src/record/log.rs`
- **Commit:** `14114ee`

**2. [Rule 3 - Blocking] `markers::strip_timestamp` сделана `pub`**
- **Found during:** Task 2
- **Issue:** хвост должен снимать тот же префикс `MM/DD HH:MM:SS `, что и `parse_marker`; дублировать разбор — расхождение форматов. План говорил, что 01-08 не трогает `markers.rs` (из-за параллельной 01-06), но 01-06 уже выполнена.
- **Fix:** одно слово `pub` в `markers.rs`, логика не менялась.
- **Files modified:** `crates/pipeline/src/record/markers.rs`
- **Commit:** `f945767`

**3. [Rule 1 - Bug в плане] Тест `tail_folds_repeats…` не выполним с «последние 30 строк»**
- **Found during:** Task 2
- **Issue:** по плану после 800 `Failed writing` идут 40 разных строк, а `render(30)` обязан содержать «… ×800»; обычное окно из 30 последних строк его вытесняет.
- **Fix:** помимо первой `AFXERROR` закрепляется самая частая повторяющаяся `AFXERROR` (≥3 подряд), окно сжимается на число закреплённых, итог ≤ 30. Это ровно то, что нужно при диагностике шторма «Failed writing image».
- **Files modified:** `crates/pipeline/src/record/log.rs`
- **Commit:** `f945767`

### Уточнения сверх плана (не отклонения от контракта)

- Пустая строка перед разделителем пишется только если файл уже не пуст (чтобы `record.log` не начинался с пустой строки); дозапись по-прежнему отделена пустой строкой.
- Строка фактов добавляется и к `LAUNCHER_FAILED` из `launch_failed` (раньше `launcher`/`game` там уже были `NotLaunched`/`NotStarted`).
- `Journal::failure` пишет факты в `record.log` и для `CANCELLED` (сообщение отмены без фактов — как в плане).
- Порядок «лаунчер вышел раньше игры» в тесте журнала не проверяется: на фейке игра появляется на 1-й секунде, лаунчер выходит на 2-й; в реальности (спайки) наоборот — машина не зависит от порядка.
- `cargo fmt` выполнен через `rustfmt --edition 2024` только для файлов плана (запуск `cargo fmt -p pipeline` переформатировал бы постороннюю `crates/pipeline/src/lib.rs`, как было в 01-06).

## Known Stubs

Нет.

## Deferred Issues

- Код выхода лаунчера в сообщении «HLAE.exe завершился с кодом 0xC0000005» (01-04, `exit_code_text`) пишется заглавными буквами, а в `facts_line` и для игры — строчными (`0xc0000005`, как требует план/Pitfall 6). Единый регистр можно выбрать при доработке карточки (01-07).
- Временные папки `cs2cinema-record-<pid>-*` в `%TEMP%`, оставленные сценариями, которые не читают журнал, не чистятся (читающие тесты чистят за собой).
- Ротация `record.log` (`record.log.N`) — фаза 3 (T-01-13 accept).

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: untrusted-console-to-disk | crates/pipeline/src/record/log.rs | строки netcon (порт слушает 0.0.0.0), `console.log` и HLAE попадают в `record.log`, `console_tail` и далее в базу: управляющие символы заменяются, профиль и секреты скрываются, в исполняемое ничего не уходит (T-01-29 — смягчено сверх плана) |

## Self-Check: PASSED

- `crates/pipeline/src/record/log.rs` — найден; `grep truncate(true)|thread::sleep|Instant::now crates/pipeline/src/record` — пусто
- коммиты `5b161f8`, `14114ee`, `6d9360e`, `f945767`, `b0326f2`, `06e50b6` — в истории `main`
- `cargo test --release -p pipeline --test record` — 24 passed; `cargo test --release --workspace` и `cargo build --release --workspace` — зелёные
