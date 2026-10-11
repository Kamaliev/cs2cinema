---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 04
subsystem: pipeline
tags: [rust, record, shutdown, cancel, afxerror, hlae-dialog, partial-takes]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: "02-01: Session::windows/wait_exit, фейк с окнами, выходом после terminate, JobEmpty; 02-03: Deadlines quit_wait/kill_confirm/window_poll/prev_game_exit"
provides:
  - "Progress::force_cancelled (D-12) и RecordRequest.afxerror_ignore (D-09)"
  - "Закрытие игры в драйвере record(): shutdown (quit -> 5 с -> свой job -> подтверждение по handle 3 с), Stop/stop_for"
  - "pipeline::record::mark_partial: недописанный takeNNNN -> takeNNNN.partial (D-07)"
  - "Мгновенный HLAE_ERROR по первой AFXERROR (с исключениями-подстроками) и по диалогу #32770 HLAE/AfxHookSource2; опрос окон раз в window_poll"
  - "Заголовки окон игры в сообщении сбоя по дедлайну запуска; строки окна: в record.log"
  - "Закрытие удачной записи ждёт пустой job (WR-05), потерянный quit повторяется (WR-01)"
affects: [02-06, 02-07, 02-08, отмена, очередь, уборка]

actuals:
  tokens: 14600
  tasks: 3
  commits: 3

plan_head_before: 4d549ae17c36ac307daa1d6c87d068f5530888f8
plan_head_after: 1b79ba64d5a8c9324d7980b773d0a613df4cf0f0
commits: 3

tech-stack:
  added: []
  patterns:
    - "машина состояний только решает, процессы и ожидания закрытия ведёт драйвер record()"
    - "весь русский текст закрытия, окон и AFXERROR живёт в record/failure.rs"
    - "опрос окон — дополнительный Input::Windows тем же step(), что и события сессии"

key-files:
  created: []
  modified:
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/record/machine.rs
    - crates/pipeline/src/record/markers.rs
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/src/record/deadlines.rs
    - crates/pipeline/tests/record.rs
    - crates/app/src/jobs.rs

key-decisions:
  - "Режим закрытия по коду сбоя: успех/GAME_EXITED — Immediate без ожидания, HLAE_ERROR — Immediate с дочиткой строк 1 с, остальное (включая TAKE_TIMEOUT и отмену) — Graceful с quit"
  - "Окно ошибки HLAE — только диалог #32770 с afxhook/hlae/afxerror в заголовке или тексте; прочие диалоги — один раз факт в журнал и в сообщение сбоя по дедлайну"
  - "Окна опрашиваются в состояниях Launcher..Take, а не только пока жив HLAE.exe (окно ошибки показывает процесс игры)"
  - "Успешное закрытие ждёт JobEmpty до game_close (30 с) после выхода игры; close_settle удалён"
  - "afxerror_ignore пока всегда пустой в app (значение из settings.json подключает 02-06)"

patterns-established:
  - "shutdown() возвращает Liveness после wait_exit; все шаги закрытия пишутся строками app в record.log"

requirements-completed: []

duration: 35 min
completed: 2026-10-10
status: complete
---

# Phase 2 Plan 04: Поэтапная отмена, мгновенный HLAE_ERROR и закрытие без обрыва дубля Summary

**Запись закрывает игру в драйвере поэтапно с подтверждением выхода (отмена укладывается в ~5 с на фейке с зависшим quit, принудительная — сразу), оборванный дубль помечается `.partial`, первая `AFXERROR` и диалог ошибки HLAE роняют запись кодом `HLAE_ERROR` за секунды, а удачная запись не обрывает последний дубль.**

## Performance

- **Duration:** около 35 мин
- **Tasks:** 3 (1 tracer + 2 auto)
- **Files modified:** 8, новых нет

## Accomplishments

- `record/mod.rs`: `shutdown` — `quit` (если консоль открыта) → ожидание выхода не дольше `quit_wait` (5 с) → `terminate` своего job → `wait_exit(kill_confirm)` (3 с); принудительная отмена (`Progress::force_cancelled`) пропускает `quit` и ожидание. Каждый шаг пишется в `record.log`; «игра не закрылась… следующая задача подождёт её до 30 с» — для игры, пережившей job.
- `mark_partial` переименовывает папки `take` + 4 цифры, которых нет среди готовых, в `takeNNNN.partial` (занято — суффикс `-<unix-секунды>`), три попытки с паузой 500 мс, ничего не удаляет; вызывается после подтверждённого выхода игры.
- `Machine`: первая `AFXERROR` в любом состоянии (включая «закрытие») сразу даёт `HLAE_ERROR`; подстроки из `afxerror_ignore` строку пропускают, факт пишется один раз; `Input::Windows` — диалог `#32770` с `AfxHookSource2`/`HLAE`/`AFXERROR` роняет запись, текст окна идёт в сообщение.
- Драйвер опрашивает окна раз в секунду от лаунчера до дубля, пишет в `record.log` строку `окна: …` при смене набора и дописывает заголовки окон игры к сбою по дедлайну запуска (до пяти).
- Хвост консоли в сбое теперь собирается после закрытия игры: шторм `AFXERROR` дочитывается (`drain` 1 с для `HLAE_ERROR`).
- Закрытие удачной записи: `Action::Terminate` и `close_settle` удалены; после выхода игры машина ждёт `JobEmpty` до `game_close`; неотправленный `quit` повторяется при следующем `ConsoleUp` (WR-01, WR-05).

## Task Commits

1. **Task 1: поэтапное закрытие, force-отмена, .partial** - `7ad602a` (feat)
2. **Task 2: мгновенный HLAE_ERROR по AFXERROR и окну, окна в сбое по дедлайну** - `c045bdb` (feat)
3. **Task 3: закрытие ждёт пустой job, повтор quit** - `1b79ba6` (fix)

## Verification

- `cargo test --release -p pipeline --test record`: 35 passed (в т. ч. `cancel_during_a_take_quits_waits_terminates_and_confirms_exit_within_ten_seconds`, `force_cancel_skips_quit_and_terminates_at_once`, `unfinished_take_is_renamed_partial_and_finished_ones_stay`, `first_afxerror_fails_at_once_with_hlae_error`, `hlae_error_dialog_fails_at_once_with_its_text`, `success_waits_for_job_empty_after_the_game_exits`).
- `cargo test --release -p pipeline --lib record::`: 19 passed (`quit_that_fails_to_send_is_resent_on_reconnect`, `foreign_dialog_class_alone_is_only_a_fact`).
- `cargo test --release -p app --bin cs2cinema cancelled_recording_becomes_a_cancelled_job`: passed.
- `cargo build --release --workspace`, `cargo test --release -p cli --test pipeline` и `cargo test --release --workspace`: зелёные.
- `grep -rn close_settle crates` и `grep Action::Terminate record/machine.rs record/mod.rs` пусты.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Старый тест ожидал, что quit не отправляется после сбоя**
- **Found during:** Task 1
- **Issue:** `all_done_does_not_stand_in_for_a_lost_last_take_end` проверял отсутствие `quit` в `fake.sent()`. После переноса закрытия в драйвер `quit` законно уходит из `shutdown` после `TAKE_TIMEOUT` (код сбоя ведёт к `Graceful`).
- **Fix:** проверка заменена на «ровно один `quit`» плюс прежняя проверка, что машина в `Closing` не входила; смысл теста (ALL_DONE не засчитывает дубль) сохранён.
- **Files modified:** crates/pipeline/tests/record.rs
- **Commit:** 7ad602a

**2. [Rule 2 - Missing critical] Тексты для Task 3 добавлены в failure.rs вместе с Task 2**
- `msg_job_not_empty` объявлен в коммите Task 2 вместе с остальными `msg_*` (один файл каталога сообщений, `pub`, без предупреждений); использован в Task 3.
- **Commit:** c045bdb

Остальное выполнено как написано. Нюанс по формату: `rustfmt` на `record/mod.rs` форматирует и подмодули, но изменённых строк вне запланированных файлов нет.

## Known Stubs

Нет. `afxerror_ignore: Vec::new()` в `crates/app/src/jobs.rs` — намеренное значение по умолчанию D-09; подключение к `settings.json` — план 02-06.

## Requirements

`requirements-completed` оставлен пустым намеренно. LCH-05, TMO-04, TMO-05 уже отмечены выполненными планом 02-01 (логика записи теперь есть и здесь). TMO-03 (статус «отменяется» за 3 с, чистое состояние за ~10 с во всех этапах, включая ffmpeg, и в приложении) и TMO-06 (уборка cfg и копии демки по политике D-05/D-06) требуют приложения и уборки из 02-06/02-07; здесь сделана только запись: отмена до ~10 с на фейке и `.partial`.

## Threat Flags

Новых сетевых точек и путей аутентификации нет. Митигации плана выполнены: T-02-13 (`is_ignored` — только подстроки без регулярных выражений, активный список в `record.log`, тексты окон через `log::clean`), T-02-14 (`quit_wait` 5 с, затем завершение своего job и `wait_exit` 3 с, принудительная отмена), T-02-15 (`mark_partial` трогает только `take` + 4 цифры внутри `clips/` и только после подтверждённого выхода, ничего не удаляет).

## Next Phase Readiness

02-06 подключает `afxerror_ignore` из настроек, статус «отменяется» и вторую кнопку «Отмена» к `Progress::force_cancelled`; 02-07 — ожидание прошлой игры и очередь; 02-08 проверяет на реальной игре, как выглядит окно ошибки HLAE (допущение A1: класс `#32770` и текст не наблюдались вживую; набор окон нормального запуска виден по строкам `окна:` в `record.log`).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/record/{mod,machine,markers,failure,deadlines}.rs, crates/pipeline/tests/record.rs
- FOUND: 7ad602a, c045bdb, 1b79ba6 (предки HEAD)
