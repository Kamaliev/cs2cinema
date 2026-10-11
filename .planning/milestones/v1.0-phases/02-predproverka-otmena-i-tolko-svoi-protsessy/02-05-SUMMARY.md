---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 05
subsystem: pipeline
tags: [rust, precheck, gather, fake-host, own-processes-guard]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: "02-01: Host::{wait_previous_game, processes, free_space, file_version, run}, FakeHost-сценарии; 02-03: Facts/evaluate/Report, space, steam, Deadlines.tool_probe/prev_game_exit/next_job_settle"
provides:
  - "pipeline::precheck::run(host, &Inputs, p) -> Result<Report>: один вызов собирает все факты окружения и отдаёт отчёт (для задачи и для кнопки в настройках)"
  - "precheck::{Demo, Inputs, check_space, hlae_ffmpeg}: второй проход оценки места по плану заменяет пункты места в том же отчёте"
  - "Страж TMO-05 по исходникам всех крейтов и по трейту Host (tests/own_processes.rs)"
affects: [02-06, 02-07, 02-08, предпроверка, настройки, очередь]

actuals:
  tokens: 7100
  tasks: 2
  commits: 2

plan_head_before: 1a5563ef692e5915b420d8e7bb20f14d39a11f3a
plan_head_after: d6138077198569231f323285d91710e9758297b3
commits: 2

tech-stack:
  added: []
  patterns:
    - "сбор фактов (gather) отделён от чистой оценки (evaluate): все ветки проверяются на FakeHost и временных папках без CS2"
    - "утилиты опрашиваются через Host::run с дедлайном tool_probe и отменой; зависшая утилита даёт провал пункта, а не зависшую задачу"
    - "страж по исходникам: запрещённые слова собираются из частей через concat(), чтобы сам тест их не содержал"

key-files:
  created:
    - crates/pipeline/src/precheck/gather.rs
    - crates/pipeline/tests/precheck.rs
    - crates/pipeline/tests/own_processes.rs
  modified:
    - crates/pipeline/src/precheck/mod.rs

key-decisions:
  - "Сначала wait_previous_game (D-14), потом снимок процессов: иначе своя зависшая игра выглядела бы чужим CS2"
  - "hlae_ffmpeg: Path= из ffmpeg.ini берётся как есть, даже если файла нет (проба даст понятный провал); относительный путь считается от ffmpeg/ в папке HLAE; без ini - ffmpeg/bin/ffmpeg.exe, иначе None"
  - "Недоступность демки (любая ошибка открытия) идёт как Missing; читается не больше 64 байт"
  - "Warn/Fail пункты пишутся в журнал сразу в run(); новые пункты места после check_space логирует вызывающий (02-06)"

patterns-established:
  - "Inputs.deadlines приходит уже с Запасом по времени; ожидание прошлой игры от запаса не зависит"

requirements-completed: [PRE-03, PRE-07, DIF-02]

duration: 25 min
completed: 2026-10-10
status: complete
---

# Phase 2 Plan 05: Сбор фактов предпроверки одним вызовом Summary

**`precheck::run` за один вызов ждёт свою прошлую игру, снимает процессы, проверяет файлы HLAE/CS2, `-version` у ffmpeg/ffprobe/ffmpeg HLAE с дедлайном, манифест Steam, версию HLAE, заголовок демки, путь результатов (пробная запись) и место на обоих дисках, ничего не завершая; `check_space` уточняет место по плану в том же отчёте.**

## Performance

- **Duration:** около 25 мин
- **Completed:** 2026-10-10
- **Tasks:** 2 (tracer + auto)
- **Files modified:** 4 (3 новых)

## Accomplishments

- Tracer: `run` на FakeHost ждёт свою прошлую игру (до 30 с и ещё 3 с после выхода), видит Steam и чужой `cs2.exe`, отдаёт `FOREIGN_GAME_RUNNING` / `PREV_GAME_STUCK` / `PRECHECK_FAILED`, отмена даёт `CANCELLED`; `terminated()` и `launches()` остаются нулевыми.
- Остальные факты: файлы PRE-03, `-version` трёх утилит (таймаут, ошибка запуска, ненулевой код, отмена), `appmanifest_730.acf` и `steam.inf` (DIF-01, DIF-02), версия `HLAE.exe`, заголовок демки (LFS-указатель ловится до скачивания), `console_safe` и пробная запись `.cs2cinema-probe-<pid>-<нс>` с удалением (T-02-17), место первым проходом (минимум) и вторым (по плану).
- Страж TMO-05: в исходниках всех крейтов нет системных утилит завершения/перечисления процессов по имени, у `Host` нет метода с `kill`.

## Task Commits

1. **Task 1: Предпроверка одним вызовом - своя прошлая игра, Steam и чужой CS2** - `6541088` (feat)
2. **Task 2: Остальные факты окружения, второй проход по плану и страж только своих процессов** - `d613807` (feat)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Test design] Сценарий места: 5 ГиБ вместо 2 ГиБ**
- **Found during:** Task 2, тест `low_space_by_plan_fails_after_the_second_pass`
- **Issue:** при 2 ГиБ свободного места первый проход (минимум 3+1 ГиБ на одном томе) уже даёт `fail`, и тест не доказывал бы, что второй проход заменяет пункты.
- **Fix:** свободно 5 ГиБ: до плана `warn` (не блокирует), после `check_space` с потребностью 5+1 ГиБ - `fail`, `blocks()`, число пунктов остаётся 15 (заменены, не дописаны).
- **Files modified:** crates/pipeline/tests/precheck.rs
- **Commit:** d613807

**2. [Rule 3 - Blocking] Фейк находит утилиту по вхождению имени**
- **Found during:** Task 2
- **Issue:** `program("ffmpeg", ..)` перекрывает `hlae-ffmpeg.exe` по порядку поиска, поэтому для сценариев провала ffmpeg нельзя брать «здоровый» набор целиком.
- **Fix:** помощник `without_ffmpeg()` (HLAE-ffmpeg + ffprobe), `healthy()` = он же + ffmpeg.
- **Files modified:** crates/pipeline/tests/precheck.rs
- **Commit:** d613807

**Сверх плана:** в `tests/precheck.rs` добавлено 6 сценариев сверх обязательных (ошибка запуска ffmpeg, отмена в пробе, отсутствие/скачивание/без задачи демки, удаление пробного файла, `hlae_ffmpeg` по ini и по bin, HLAE без ffmpeg) - итого 19 тестов при требуемых 13.

## Issues Encountered

Составные команды shell с длинными heredoc и perl падали на разборе кавычек; тесты дописаны через Write и `cat >>`. На результат не повлияло.

## Known Stubs

Нет. `run` пока не вызывается из приложения: задачу и кнопку подключают 02-06 и 02-07.

## Threat Flags

Нет новой поверхности сверх `threat_model` плана: пробная запись только в `out_root` с уникальным именем (T-02-17), чужой `cs2.exe` только обнаруживается (T-02-18, страж), утилиты с дедлайном и отменой (T-02-19).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/precheck/gather.rs, crates/pipeline/tests/precheck.rs, crates/pipeline/tests/own_processes.rs
- FOUND: 6541088, d613807 (предки HEAD)
- `cargo test --release -p pipeline --test precheck` - 19 passed; `--test own_processes` - 2 passed; `cargo build --release --workspace` - без предупреждений и ошибок; `cargo test -p pipeline -p host` зелёный.
