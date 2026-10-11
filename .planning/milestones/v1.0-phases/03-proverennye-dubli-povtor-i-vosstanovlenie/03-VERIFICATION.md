---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
verified: 2026-10-11T00:00:00Z
status: passed
score: 5/5 must-haves verified
covered_files:
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-01-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-01-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-02-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-02-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-03-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-03-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-04-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-04-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-05-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-05-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-06-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-06-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-07-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-07-SUMMARY.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-08-PLAN.md
  - .planning/phases/03-proverennye-dubli-povtor-i-vosstanovlenie/03-08-SUMMARY.md
  - crates/app/src/jobs.rs
  - crates/app/src/jobs_page.rs
  - crates/app/src/launch.rs
  - crates/pipeline/src/manifest.rs
  - crates/pipeline/src/record/mod.rs
  - crates/pipeline/src/record/verify.rs
  - crates/pipeline/tests/record.rs
  - crates/store/src/lib.rs
covered_digest: "v3:sha256:35a3bb258f773ca3a05b89bc4da4f0ca8fc3fa1aee873d01f5aed97461ed945b"
behavior_unverified: 0
overrides_applied: 1
overrides:
  - must_have: "D-07: «Повторить» дописывает только недостающие и невалидные дубли (подмножество шотов с сохранением нумерации takeNNNN)"
    reason: "CONTEXT D-07 сам допускает запасной вариант «писать всё заново, кроме случая все валидны»; HLAE нумерует новые дубли «наибольший + 1» и не заполняет пропуски. Частичное возобновление вынесено в DIF-05 (v2)."
    accepted_by: "автор (решение зафиксировано в 03-CONTEXT.md D-07 и задании на проверку)"
    accepted_at: "2026-10-11T00:00:00Z"
---

# Phase 3: Проверенные дубли, повтор и восстановление — отчёт проверки

**Цель фазы:** Сборка идёт только из проверенных дублей, упавшую задачу можно повторить кнопкой без повторного скачивания и планирования, задача, прерванная падением приложения, честно помечается, а журнал и карточка сохраняют улики всех попыток.
**Проверено:** 2026-10-11
**Статус:** passed
**Повторная проверка:** нет, первичная

## Тесты

`cargo test --release --workspace` (cargo из `~/.cargo/bin`): код выхода 0, ни одного FAILED. Сюиты: `pipeline` (unit 76), `pipeline/tests/record.rs`, `precheck.rs` (19), `own_processes.rs` (2), `transcripts.rs` (6), `host` (63 + 1 ignored, `win_session` 6), `store` (17), `app` (43), `cli/tests/pipeline.rs` (2), `renderer` (13), `director` (31) и остальные крейты. Workspace собирается (`cargo check --release --workspace` чисто). Маркеров TBD/FIXME/XXX/TODO/todo!/unimplemented! в изменённых фазой файлах нет.

## Критерии успеха из ROADMAP

| # | Критерий | Статус | Доказательства |
|---|----------|--------|----------------|
| 1 | Дубль записан только после `TAKE_END n` с nonce; перед сборкой `ffprobe` (видео+аудио, длительность в допуске, разрешение); `clips/manifest.json` со снимком окружения; обрезанный/пустой дубль даёт ошибку с номером | VERIFIED | см. ниже |
| 2 | Карточка упавшей задачи: код, состояние, сообщение, подсказка, хвост консоли, «Повторить» и «Открыть папку»; «Повторить» без скачивания и плана, при всех валидных сразу сборка; `highlights.mp4` только после успешного ffmpeg | VERIFIED | см. ниже |
| 3 | Журнал с метками времени/этапа/попытки, не стирается, длительности этапов; на задаче этап сбоя, код, попытка; `clips_old_<ts>`, `record.log.N` | VERIFIED | см. ниже |
| 4 | Убить приложение посреди записи и запустить снова: осиротевший `cs2.exe` завершён, задача «прервана» с достигнутым этапом и причиной | VERIFIED (код и тесты) + подтверждено автором вживую (03-08) | см. ниже |
| 5 | `cargo test --workspace` без CS2: десять сценариев D-17, каждый со своим кодом, отмена в каждом состоянии | VERIFIED | см. ниже |

### Критерий 1: проверенные дубли и манифест

- Метка принимается только если вся строка `CS2C <nonce> TAKE_END n`: `crates/pipeline/src/record/markers.rs:34` (`parse_marker`, сверка nonce, строгий разбор числа). `ALL_DONE` не заменяет `TAKE_END` (тест `all_done_does_not_stand_in_for_a_lost_last_take_end`, `tests/record.rs:344`).
- Файл не читается до `TAKE_END`: `Verifier::take_ended` вызывается только по принятому `TAKE_END` (`record/mod.rs:513`); тест `ffprobe_never_reads_a_take_before_its_take_end` (`tests/record.rs:1035`). Поиска дублей по mtime в пути приложения нет: `find_clip` (по номеру/fps, `renderer/src/assemble.rs:130`) используется только в CLI-ветке `assemble`; `modified()` в `crates/app`, `pipeline`, `renderer` не встречается.
- Вторичная защита от недописанного ffmpeg HLAE: `NotReady` (нет файла, `moov atom not found`) переопрашивается до `take_finalize` (`record/mod.rs:590-603`), последний дубль проверяется после закрытия игры и пустого job (`verify_rest`, `record/mod.rs:622`). Тесты `finalizing_take_is_retried_until_ffmpeg_finishes_it`, `take_that_never_finalizes_fails_after_take_finalize`, `last_take_is_verified_after_the_game_closes`.
- `ffprobe` через `Host::run` с дедлайном и отменой: `record/verify.rs:219` (`probe_take`) и `probe_file`; решение `judge` (`verify.rs:158`): нет видео, нет аудио, длительность 0, длительность вне допуска (±10%, не меньше 0.5 с, `deadlines.rs:16`), разрешение не равно заданному. Причины по-русски с числами (юнит-тест `each_defect_has_its_own_russian_reason_with_numbers`).
- Обрезанный/пустой дубль → `TAKE_INVALID` с номером дубля и папкой в сообщении и подсказке (`failure::msg_take_invalid`, `hint_take_invalid`); тесты `truncated_take_fails_with_take_invalid_within_seconds_naming_the_take` (`tests/record.rs:1064`), `take_without_audio_is_invalid`, `wrong_resolution_is_invalid_and_names_both_sizes`.
- Манифест: `crates/pipeline/src/manifest.rs` (`TakeEntry`: ожидаемая длительность, статус `pending/recorded/validated/invalid`, размеры, измеренные длительности, разрешение, попытка; `EnvSnapshot` строка 89: сборка CS2, версии HLAE/ffmpeg/ffprobe/ffmpeg HLAE, NVENC, свободное место, кадр/fps; `from_report` строка 332). Сохраняется по ходу записи (`Verifier::save`, `record/mod.rs:496`), снимок окружения также в `env.json` и в `AttemptEntry` (`jobs.rs:496-510`).
- Сборка только из проверенных: `assemble_stage` (`jobs.rs:637`) отказывает без манифеста, а `check_take_for_assembly` (`jobs.rs:606`) требует `validated` и совпадения размеров файлов; тест `assembly_refuses_a_take_that_is_not_validated`. Запись приложением всегда идёт с `verify: Some(..)` (`jobs.rs:512`).

### Критерий 2: карточка, «Повторить», атомарный `highlights.mp4`

- Карточка: `error_block` (`jobs_page.rs:421`) показывает строку «Попытка N · этап · КОД · состояние» (`attempt_line`, строка 399), сообщение, подсказку, прошедшее время, путь `record.log`, сворачиваемый хвост консоли. Кнопки «Повторить» / «Заново» / «Открыть папку» для `Failed | Cancelled | Interrupted` (`jobs_page.rs:262-290`); «Повторить» активна только при наличии `record.json` (`app.rs:350-362`).
- «Повторить» = `Store::requeue_resume` (`store/src/lib.rs:513`) → `run` ветка `job.resume` → `resume_job` (`jobs.rs:541`): берёт `record.json`, не качает и не планирует; нет данных или скрипты изменились → `RETRY_NO_DATA`; все дубли валидированы → сразу `assemble_stage` без предпроверки и игры (`jobs.rs:558-562`). Тесты `retry_with_every_take_validated_goes_straight_to_assembly`, `retry_without_record_json_fails_with_retry_no_data`, `retry_with_changed_scripts_fails_with_retry_no_data`, `retry_with_an_invalid_take_sets_clips_aside_and_records_again`.
- `highlights.mp4` только после успеха ffmpeg: склейка пишет `highlights.part.mp4`, `rename` после кода 0, при сбое удаляется только `.part`, прежний ролик остаётся (`jobs.rs:587-678`); тест `failed_assembly_keeps_the_previous_highlights_and_drops_the_part`.
- Оговорка D-07 (см. override): «Повторить» с невалидным/недостающим дублем перезаписывает все дубли заново (прежние в `clips_old_<ts>`), а не подмножество. Это документированный запасной вариант самого D-07 (`jobs.rs:565-567`, журнал называет невалидные номера); частичное возобновление отнесено к DIF-05 (v2, `REQUIREMENTS.md:89`). Критерий 2 ROADMAP этого не требует.

### Критерий 3: журнал и улики

- Миграция базы 4 (`jobs.attempt`, `jobs.resume`, `store/src/lib.rs:259-277`); `start_job` увеличивает попытку и НЕ стирает `log` (`lib.rs:388`); `requeue_with` журнал тоже не трогает (`lib.rs:517`).
- Формат одним модулем `crates/store/src/journal.rs`: разделитель «──── попытка N · дата · способ ────», метка `[этап #N]`, «⏱ этап X: 4 мин 12 с», строка итога попытки. Писатель: `JobReporter::write` с временем `%H:%M:%S` (`jobs.rs:100`), `close_stage` перед `finish` (`jobs.rs:327`). Сводка по этапам и журналы прошлых попыток в карточке (`jobs_page.rs:198-216`, `launch::old_logs`).
- `record.log` → `record.log.N` при старте записи (`record/log.rs:21` `rotate`, без ограничения числа, не перезаписывает; тест `record_log_rotation_never_overwrites_old_logs`). Дубли прошлой попытки → `clips_old_<ts>` (`jobs.rs:466` `set_aside_clips`, суффикс `-2`, `-3`; тест `set_aside_clips_never_overwrites_an_earlier_set_aside_folder`); прежний манифест сохраняется внутри `clips_old_*`.
- Этап сбоя, код, номер попытки видны на задаче: `fail_state`/`fail_code` + `jobs.attempt` → `attempt_line`.

### Критерий 4: восстановление после падения приложения

- `App::new` вызывает `launch::recover_after_crash` до создания состояния (`app.rs:199`): для каждой оставшейся «выполняется»/«отменяется» задачи (1) `kill_orphans` по `processes.json`, (2) `recover` (уборка папки игры, `.partial`, манифест-осведомлённо), (3) только потом `store.reset_running()` (`launch.rs:293-302`).
- Завершение по полной идентичности: `host::win::process::kill_if_same` (`process.rs:188`): образ из списка своих, `OpenProcess`, сверка времени создания и образа на одном handle, и только затем `TerminateProcess`; чужой процесс с тем же PID → `Mismatch`, не тронут. `processes.json` пишется драйвером, пока игра жива (`record/mod.rs:677`, оставляется, если живость неизвестна), читается как недоверенный.
- Статус: `JobStatus::Interrupted` («прервана»), `reset_running` кладёт достигнутое состояние в `fail_state`, код `INTERRUPTED`, причина «приложение закрылось во время выполнения», подсказка про «Повторить»/«Заново» (`store/src/lib.rs:536`). Очередь стоит до «Продолжить» (`PauseReason::Interrupted`, `app.rs:236`, `jobs_page.rs:311`, D-16). Тесты: `launch.rs` (кейсы `recover_after_crash`, строки 519-567), `store` (`Interrupted`, строки 626-713), `pipeline/tests/own_processes.rs`, `host/tests/win_session.rs`.
- Живой прогон «убить приложение посреди записи» выполнен автором в плане 03-08 и одобрен (замеров нет); принято как подтверждение человеком, не как пробел.

### Критерий 5: сценарии TST-02 без CS2

- `every_d17_scenario_ends_with_its_own_code` (`tests/record.rs:1306`): десять строк, десять РАЗНЫХ кодов (`LauncherFailed`, `GameNotStarted`, `HlaeError`, `ConsoleNotOpen`, `DemoNotLoaded`, `ScriptNotRun`, `TakeTimeout`, `TakeInvalid`, `GameExited`, `Cancelled`), проверяются и код, и строка «код: …» в `record.log`, и уникальность (`assert_eq!(codes.len(), 10)`).
- `cancel_in_every_state_returns_cancelled_closes_the_session_within_ten_seconds` (`tests/record.rs:1467`): отмена в Launcher, Game, Console, Demo, Script, Take, Closing; сессия закрыта, возврат не позже 10 с виртуального времени; отмена во время ffprobe — отдельный тест `cancel_during_ffprobe_cancels_the_recording`. Все зелёные в прогоне.
- Не-Windows: `host` имеет `cfg(not(windows))` заглушку (`host/src/lib.rs:357`, `unsupported.rs`); `Host::kill_orphan` на ней возвращает `Unsupported`.

## Покрытие требований

| Требование | Статус | Основные доказательства |
|------------|--------|--------------------------|
| TAK-02 | SATISFIED | `markers.rs:34`; `record/mod.rs:513`; тест `ffprobe_never_reads_a_take_before_its_take_end` |
| TAK-03 | SATISFIED | `manifest.rs` (`Manifest`, `TakeEntry`, `EnvSnapshot`); `Verifier::save` |
| TAK-04 | SATISFIED | `verify.rs:158/219`; `jobs.rs:606/637`; тесты обрезан/без звука/разрешение |
| TAK-05 | SATISFIED | `NotReady`-повторы, `verify_rest`, `check_take_for_assembly` вместо «файл существует» |
| TAK-06 | SATISFIED | `highlights.part.mp4` → rename (`jobs.rs:587-678`) |
| RET-01 | SATISFIED | `start_job`, `journal.rs`, `attempt_line` |
| RET-02 | SATISFIED | `resume_job`, `record.json` (`manifest.rs:422`), `begin_plan` убирает устаревший |
| RET-03 | SATISFIED | `jobs.rs:558`; тест `retry_with_every_take_validated_goes_straight_to_assembly` |
| RET-04 | SATISFIED (+ вживую автором) | `launch.rs:293`, `process.rs:188`, `store/src/lib.rs:536` |
| RET-06 | SATISFIED | `record/log.rs:21`, `jobs.rs:466` |
| OBS-01 | SATISFIED | `jobs.rs:96-131`, `journal.rs` |
| OBS-02 | SATISFIED | `jobs_page.rs:262-290, 421-447` |
| OBS-03 | SATISFIED | `manifest.rs:89/332`, `env.json`, `jobs.rs:496-510` |
| TST-02 | SATISFIED | `tests/record.rs:1306, 1467` |

Осиротевших требований фазы 3 в REQUIREMENTS.md нет (все 14 ID заявлены планами).

## Anti-patterns

Блокирующих нет. Отмечено (не влияет на статус):

- Информационно: `Option<Verify>` в `RecordRequest` (`record/mod.rs:47`) допускает запись без проверки; приложение всегда передаёт `Some` (`jobs.rs:512`), а `None` остался для CLI/старых тестов до фазы 4 (RET-05/TST-03). Это ожидаемая граница фазы, но в фазе 4 путь без проверки должен исчезнуть.
- Информационно: завершение «дочернего ffmpeg HLAE» для промежуточных дублей определяется косвенно (успешный `ffprobe` читает готовый mp4, повторы до `take_finalize`, размеры стабилизированы), а для последнего дубля — по закрытию игры и пустому job. Прямого отслеживания процесса ffmpeg HLAE по дублю нет; поведение на живом HLAE подтверждено автором (03-08), допуски подстраиваются в фазе 4 по приёмке.

## Ручная проверка

Манипуляции на живых HLAE/CS2 (план 03-08: «Повторить» после сбоя, запись с проверкой, убийство приложения и перезапуск) одобрены автором; замеров не записано. По условию задания считается подтверждением человеком; требований к дополнительной проверке не остаётся.

## Итог

Цель фазы достигнута в коде: сборка идёт только из дублей со статусом `validated` в манифесте и совпадающими размерами; «Повторить» работает по `record.json` без скачивания и планирования и при полностью проверенных дублях идёт сразу на склейку; осиротевшие процессы завершаются по (PID, время создания, образ) до перевода задачи в «прервана»; журнал, `record.log.N` и `clips_old_<ts>` сохраняют улики всех попыток; сценарии TST-02 на `FakeHost` зелёные. Пробелов нет.

---

_Проверено: 2026-10-11_
_Верификатор: Claude (gsd-verifier)_
