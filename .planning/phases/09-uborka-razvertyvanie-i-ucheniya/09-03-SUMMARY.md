---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 03
subsystem: runner
tags: [runner, cleanup, sweep, retention, log-rotation, outbox, rust]

requires:
  - phase: 06-raskhozhdenie-runner
    provides: "Runner::remove_finished, outbox runner_state.json с этапом Finished и записью результата, идемпотентный повтор finish на сервере"
provides:
  - "runner::sweep: проход уборки на простое (подтверждённые дубли по метке сразу, неподтверждённые папки дублей по 72 ч, демки и части по 24 ч), ротация runner.log"
  - "Метка out/srv{N}.accepted, пишется до удаления и снимается последней; Runner::settle_accepted, server_accepted, settle_or_drop"
  - "Тесты crates/runner/tests/sweep.rs (13), закрепляющие инвариант «Finished с результатом на диске до первой загрузки»"
affects: [09-10 учения (учение 04, отмена при офлайн-раннере), приложение в режиме раннера (та же уборка без правок crates/app)]

actuals:
  tokens: 14500
  tasks: 3
  commits: 5

tech-stack:
  added: []
  patterns:
    - "Подтверждённое и неподтверждённое разделены состоянием (метка), а не возрастом"
    - "Возраст папки — самое позднее mtime обычных файлов внутри, ссылки не разыменовываются"
    - "Строгий разбор имён srv{N}, srv{N}_{хвост}, srv{N}.accepted, srv{N}.dem[.part], srv{N}.faceit; число без ведущих нулей"

key-files:
  created:
    - crates/runner/src/sweep.rs
    - crates/runner/tests/sweep.rs
  modified:
    - crates/runner/src/lib.rs

key-decisions:
  - "Сроки уборки — константы модуля sweep, а не поля runner::Config: литералы Config в cs2-cli и тестах не меняются"
  - "Метка подтверждения — отдельный файл рядом с папкой дублей, а не поле runner_state.json: состояние перезаписывают снимком три места, а метка переживает ещё и порчу состояния"
  - "Повтор finish уходит один раз и только для задачи на Finished с проверенным результатом; любая ошибка означает «не подтверждено» и ведёт к прежнему drop_job"
  - "Возраст папки считается по файлам внутри: mtime каталога на Windows непредсказуем и не ставится через std в тестах"

patterns-established:
  - "Уборка идёт только на простое (state.job пусто) и не чаще SWEEP_EVERY; итог — одна строка «уборка: …» в runner.log без путей"
  - "Порядок «метка до удаления, удаление до persist» живёт в одном месте — Runner::settle_accepted"

requirements-completed: [OPS-01]

coverage:
  - id: D1
    description: "Простаивающий раннер сам удаляет сиротские папки дублей старше 72 ч (имя srv{N} и srv{N}_{герой}), свежие не трогает, пишет строку «уборка:» в runner.log"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#idle_runner_sweeps_old_orphan_take_folders_and_keeps_fresh_ones"
        status: pass
    human_judgment: false
  - id: D2
    description: "Скачанные демки, .dem.part и папки FACEIT старше 24 ч убираются; удерживаемая задача, чужие имена, ссылки и всё вне out/ и remote/ неприкосновенны; проход не чаще раза в SWEEP_EVERY"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#old_remote_demos_parts_and_faceit_folders_are_swept"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#held_job_is_never_swept"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#foreign_names_and_links_are_left_alone"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#sweep_runs_at_most_once_per_interval"
        status: pass
    human_judgment: false
  - id: D3
    description: "runner.log больше 10 МиБ переименовывается в runner.log.1 при старте и на проходе уборки, токен не попадает ни в один из файлов"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#runner_log_is_rotated_when_it_grows_too_large"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#runner_log_is_rotated_on_the_sweep_pass"
        status: pass
      - kind: unit
        ref: "crates/runner/src/sweep.rs#rotate_log_leaves_small_files_and_replaces_the_previous_rotation"
        status: pass
    human_judgment: false
  - id: D4
    description: "Дубли задач с подтверждённым сервером результатом уходят при любом обрыве не позже ближайшего прохода (метка out/srv{N}.accepted, повтор finish перед drop_job), без подтверждения остаются до срока"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#confirmed_takes_are_swept_at_once_even_when_fresh"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#restart_after_the_server_accepted_the_result_removes_the_takes"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#result_accepted_behind_the_runners_back_is_settled_not_dropped"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#unconfirmed_dropped_job_keeps_its_takes_until_the_age_limit"
        status: pass
    human_judgment: false
  - id: D5
    description: "Раннер не начинает загрузку, пока Finished с результатом не записан в runner_state.json; упавший посреди загрузки раннер после перезапуска закрывает принятую сервером задачу повтором finish"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#upload_never_starts_before_the_finished_state_is_on_disk"
        status: pass
      - kind: integration
        ref: "crates/runner/tests/sweep.rs#runner_stopped_mid_upload_restarts_from_finished_and_settles_the_accepted_result"
        status: pass
    human_judgment: false
  - id: D6
    description: "Приложение в режиме раннера получает ту же уборку без правок crates/app"
    requirement: OPS-01
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema remote"
        status: pass
    human_judgment: false

duration: 20min
completed: 2026-10-12
status: complete
plan_head_before: f177b87
plan_head_after: 7ee1c860c7ebb65de7acee52da8730168ade14e3
commits: 5
---

# Phase 9 Plan 03: уборка раннера Summary

**Раннер на простое сам убирает дубли подтверждённых задач по метке `out/srv{N}.accepted` и повтору `finish`, сироты и демки уходят по возрасту, а `runner.log` ротируется; всё в `runner::sweep` без правок `crates/app`.**

## Performance

- **Duration:** около 20 мин
- **Completed:** 2026-10-12
- **Tasks:** 3 (трассер, два TDD-задания)
- **Files modified:** 3 (`sweep.rs` и `tests/sweep.rs` созданы, `lib.rs` изменён)
- **Коммитов:** 5 (измерено от `f177b87`)

## Accomplishments

- Модуль `runner::sweep`: `sweep`, `marker_path`, `mark_accepted`, `remove_confirmed`, `rotate_log`, `Swept` с итоговой строкой, константы `ORPHAN_OUT_AGE` (72 ч), `ORPHAN_REMOTE_AGE` (24 ч), `LOG_MAX_BYTES` (10 МиБ), `SWEEP_EVERY` (10 мин). Строгий разбор имён, без ссылок, только внутри `out/` и `remote/` рабочей папки, удерживаемая задача пропускается.
- `Runner::tick` на простое зовёт `sweep_now`; `Log` хранит путь и под мьютексом подменяет хэндл при ротации; `Runner::new` ротирует журнал до открытия.
- Подтверждение приёма, переживающее обрыв: `settle_accepted` пишет метку до удаления и снимает её последней; `server_accepted` один раз повторяет `finish` с тем же размером и SHA-256 только для этапа `Finished` с результатом; `settle_or_drop` стоит на `hello` (abandon и неизвестная задача), на `lose` и на `TransferEnd::Dropped` в `upload`.
- Инвариант фазы 6 «`Finished` с результатом на диске до первого запроса загрузки» не менялся и закреплён: тест краснеет, если убрать `self.persist()?` из `execute` (проверено мутацией), плюс проверка порядка `awk`.

## Task Commits

1. **Task 1: простой раннера убирает старые сиротские папки дублей** — `c2bdc0b` (feat, трассер)
2. **Task 2: демки, удерживаемая задача, чужие имена, журнал** — RED `7e56a94` (test), GREEN `814a37b` (feat)
3. **Task 3: подтверждённые дубли по метке и повтору finish** — RED `64fb70b` (test), GREEN `7ee1c86` (feat)

**Plan metadata:** отдельные docs-коммиты (SUMMARY, затем STATE и ROADMAP).

## Files Created/Modified

- `crates/runner/src/sweep.rs` — уборка, метки подтверждения, ротация журнала, модульные тесты разбора имён и `rotate_log`
- `crates/runner/src/lib.rs` — `pub mod sweep;`, `Runner.last_sweep`, `sweep_now`, `Log::rotate`, `settle_accepted`, `server_accepted`, `settle_or_drop`, снятие старой метки в `accept`
- `crates/runner/tests/sweep.rs` — 13 интеграционных тестов на настоящем сервере и временных папках

## Decisions Made

- Сроки — константы модуля, не поля `Config` (литералы в `cs2-cli` и тестах не меняются).
- Метка подтверждения — отдельный файл рядом с папкой дублей: частичный `remove_dir_all` не уносит её раньше дублей, а порча `runner_state.json` её не затрагивает.
- Повтор `finish` — один вопрос без повторов; ответ, отличный от `Done`, и любая ошибка ведут к прежнему `drop_job` без удаления дублей.
- Имя папки проверяется в каноническом виде (`srv7`, не `srv007`): чужие имена не попадают под уборку.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Дополнительный тест ротации на проходе уборки**
- **Found during:** Task 2
- **Issue:** план проверял ротацию только при старте раннера, а путь `sweep_now` → `Log::rotate` (смена хэндла под открытым файлом) оставался без теста.
- **Fix:** тест `runner_log_is_rotated_on_the_sweep_pass`; всего 13 тестов вместо 12.
- **Files modified:** `crates/runner/tests/sweep.rs`
- **Committed in:** `7e56a94`

**2. [Rule 2 - Missing Critical] Запрет ведущих нулей и проверка сиблингов имён**
- **Found during:** Task 1
- **Issue:** `srv007` разбирался бы как задача 7 и мог попасть под уборку чужих данных.
- **Fix:** номер принимается только в каноническом виде; модульный тест `names_are_parsed_strictly`, в тест чужих имён добавлены `out/srv012`, `remote/srv3x.dem`, `remote/srv3.dem.bak`.
- **Files modified:** `crates/runner/src/sweep.rs`, `crates/runner/tests/sweep.rs`
- **Committed in:** `c2bdc0b`, `7e56a94`

---

**Total deviations:** 2 auto-fixed (оба Rule 2)
**Impact on plan:** только усиление проверок, поведение по плану.

## TDD Gate Compliance

Task 2 и Task 3 шли как RED → GREEN (`test(09-03)` → `feat(09-03)`). Часть тестов в RED уже была зелёной по построению: `held_job_is_never_swept`, `foreign_names_and_links_are_left_alone`, `sweep_runs_at_most_once_per_interval` закрепляют правила Task 1, а `upload_never_starts_before_the_finished_state_is_on_disk` и `unconfirmed_dropped_job_keeps_its_takes_until_the_age_limit` закрепляют код фазы 6 (по плану). Красными в RED были: уборка демок, ротация журнала (два теста) и четыре теста подтверждения. Проверка мутацией (удаление `self.persist()?` из `execute`) подтвердила, что тест порядка записи краснеет.

## Issues Encountered

None. Предупреждения сборки только из `vendor/tiny_http` (были до плана).

## Known Stubs

None.

## Threat Flags

None. Новая поверхность — удаление файлов в рабочей папке раннера — покрыта T-09-10 (строгие имена, без ссылок, `starts_with(dir)`, пропуск удерживаемой), T-09-38 (повтор `finish` только на `Finished` с результатом, решает сервер), T-09-39 (метка снимается при выдаче задачи с тем же номером в `accept`).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Раннерная половина критерия 1 (OPS-01) закрыта; серверная половина — в плане 09-02.
- Учение 04 (отмена при офлайн-раннере, план 09-10) может опираться на уборку по возрасту: неподтверждённые папки дублей уходят через 72 ч.
- Допущение из плана: «Повторить» закрытой выдачи на неподтверждённой папке старше 72 ч может совпасть с проходом уборки (окно — один проход раз в 10 минут); принято как редкий случай.

## Self-Check: PASSED

- `crates/runner/src/sweep.rs`, `crates/runner/tests/sweep.rs` существуют; коммиты `c2bdc0b`, `7e56a94`, `814a37b`, `64fb70b`, `7ee1c86` входят в историю `main`.
- `cargo test --release -p runner` (в том числе 13 тестов `sweep` и `outbox` без правок), `-p app --bin cs2cinema remote`, `-p cli --bin cs2-cli runner_cmd`, `-p cli --test pipeline` зелёные; `git diff --stat -- crates/app crates/cli crates/server crates/wire` пуст.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-12*
