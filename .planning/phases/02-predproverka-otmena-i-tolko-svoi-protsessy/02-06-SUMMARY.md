---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 06
subsystem: app
tags: [rust, egui, precheck, cleanup, installed-json, host-run, nvenc, thumbnail]

requires:
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: "02-02: store.set_precheck/last_good/set_last_good/interrupted_jobs, fail_job; 02-04: pipeline::record с подтверждённым закрытием игры и mark_partial; 02-05: precheck::run/check_space/hlae_ffmpeg"
provides:
  - "jobs::{needs_precheck, precheck_inputs, precheck_stage}: задача с записью начинается с предпроверки до скачивания; fail останавливает её с кодом на ступени «предпроверка»"
  - "Второй проход места по плану (space::estimate + check_space) до установки файлов в игру; пара (сборка CS2, версия HLAE) сохраняется после удачной записи"
  - "jobs_page::precheck_view: отчёт ✔/⚠/✖ в карточке, у упавшей на предпроверке раскрыт"
  - "launch::{Installed, INSTALLED_FILE, cleanup_installed, recover, note_take_done, is_ours}: папка игры чистая при любом исходе и после падения приложения"
  - "NVENC-проба и превью через Host::run с дедлайном и отменой; Settings.afxerror_ignore доходит до записи"
affects: [02-07, 02-08, настройки, очередь, ручная проверка записи]

actuals:
  tokens: 13967
  tasks: 3
  commits: 3

plan_head_before: 9219c9d08adc11ba284517705e570f3aa61c99b5
plan_head_after: 7716a6558855214f39a936f4df078977a069aa60
commits: 3

tech-stack:
  added: []
  patterns:
    - "запись для уборки (installed.json) пишется ДО побочного эффекта и переживает падение приложения"
    - "удаление по недоверенному файлу только через is_ours: папка CS2 из настроек, канонизация родителя, шаблоны имён, отказ с записью в журнал"
    - "результат установки и записи держится в переменной, уборка идёт при любом исходе, ошибка пробрасывается после неё"

key-files:
  created: []
  modified:
    - crates/app/src/jobs.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/launch.rs
    - crates/app/src/app.rs
    - crates/app/src/settings.rs
    - crates/app/src/match_page.rs
    - crates/pipeline/src/lib.rs

key-decisions:
  - "Второй проход места делается в ветке «дубли ещё не записаны»: задача, которой остаётся только склейка, не блокируется нехваткой места под запись"
  - "recover помечает дубли .partial только когда installed.json есть: без него запись уже убрана после успеха, дубли целые"
  - "install_files дописывает пути прошлой недоубранной установки в новую запись, а done_takes обнуляет"
  - "Проверка путей в match_page::enqueue удалена вместе с check_paths: её пункты теперь в предпроверке задачи"

patterns-established:
  - "Один построитель Inputs (precheck_inputs) для задачи и для кнопки в настройках (02-07)"

requirements-completed: [PRE-01, PRE-02, PRE-06, PRE-07, DIF-01, LCH-05, TMO-03, TMO-06]

duration: 35 min
completed: 2026-10-10
status: complete

coverage:
  - id: D1
    description: "Задача с записью начинается с предпроверки до скачивания; fail останавливает задачу с типизированным кодом на ступени «предпроверка», задача «только план» её пропускает"
    requirement: "PRE-02"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#precheck_failure_reaches_the_job_row_with_its_report"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#plan_job_skips_the_precheck"
        status: pass
    human_judgment: false
  - id: D2
    description: "Отчёт предпроверки в карточке задачи (строки ✔/⚠/✖, подсказки, раскрыт у упавшей)"
    requirement: "PRE-01"
    human_judgment: true
    rationale: "Внешний вид карточки egui не проверяется тестами; данные отчёта проверены в jobs::tests, вид — ручная проверка 02-08"
  - id: D3
    description: "Папка игры после любого исхода и после падения приложения содержит только прежнее: удаляются лишь файлы из installed.json, подтверждённые is_ours; оборванные дубли помечаются .partial"
    requirement: "TMO-06"
    verification:
      - kind: unit
        ref: "crates/app/src/launch.rs#cleanup_removes_only_our_files_inside_the_game_folder"
        status: pass
      - kind: unit
        ref: "crates/app/src/launch.rs#recover_marks_unfinished_takes_partial"
        status: pass
    human_judgment: true
    rationale: "Настоящая запись и падение приложения посреди неё проверяются вручную в 02-08"
  - id: D4
    description: "NVENC-проба и превью идут через Host::run с дедлайном и отменой; AFXERROR-исключения из settings.json доходят до записи"
    requirement: "TMO-03"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#nvenc_probe_that_hangs_is_cut_by_its_deadline"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#afxerror_ignore_from_settings_reaches_the_record_request"
        status: pass
    human_judgment: false
---

# Phase 2 Plan 06: Предпроверка, уборка папки игры и дедлайны во всех внешних вызовах Summary

**Задача с записью начинается с предпроверки до скачивания демки (fail останавливает её кодом на ступени «предпроверка», отчёт виден в карточке), место уточняется по плану до установки файлов, папка игры чистится при любом исходе и после падения приложения по `installed.json`, а проба NVENC и превью идут через `Host::run` с дедлайном и отменой.**

## Performance

- **Duration:** около 35 мин
- **Completed:** 2026-10-10
- **Tasks:** 3 (tracer + 2 auto)
- **Files modified:** 7

## Accomplishments

- Tracer (Task 1): `precheck_stage` стоит в `run()` выше `pipeline::analyze`; `fail` отдаёт `RunError` с `RecordFailure` (`FOREIGN_GAME_RUNNING` / `PREV_GAME_STUCK` / `PRECHECK_FAILED`, состояние `precheck`), отчёт сохраняется в строку задачи, при успехе лестница прячется. Второй проход (`space::estimate` по плану и размеру распакованной демки, `Analysis::demo_len`) заменяет пункты места, `warn`/`fail` идут в журнал, нехватка места останавливает задачу до запуска игры. После удачной склейки в задаче, где шла запись, сохраняется пара (сборка CS2, версия HLAE).
- `precheck_view`: свёрнутый заголовок «Предпроверка: всё в порядке / N предупреждений / N ошибок» (с правильным склонением), строки ✔/⚠/✖ и подсказки; у задачи, упавшей на предпроверке, раскрыт.
- Уборка (Task 2): `install_files` пишет `installed.json` до копирования; `cleanup_installed` удаляет только то, что проходит `is_ours` (папка CS2 из настроек, без `..`, канонизированный родитель ровно `game/csgo` или `game/csgo/cfg`, шаблоны имён), остальное отвергает с записью в журнал; 3 попытки через 500 мс, недоубранное остаётся в записи. `run()` убирает за собой при успехе, сбое и отмене, недописанный `highlights.mp4` удаляется при сбое склейки. `JobReporter` на `TakeDone` пишет `done_takes`; `App::new` до `reset_running` вызывает `launch::recover` для задач «выполняется»/«отменяется».
- Дедлайны (Task 3): `nvenc_check`/`has_nvenc` через `Host::run` (таймаут, отмена, спавн — понятные тексты), `thumbnail` через `Host::run` с `Deadlines::thumbnail` и временным файлом; `hlae_ffmpeg` делегирует `pipeline::precheck::hlae_ffmpeg`; прежняя `check_paths` удалена. `Settings.afxerror_ignore` (по умолчанию пусто) идёт в `RecordRequest`.

## Task Commits

1. **Task 1: Задача с записью начинается с предпроверки** - `4a8a0ea` (feat)
2. **Task 2: Папка игры чистая при любом исходе** - `9eb975d` (feat)
3. **Task 3: Ни одного внешнего вызова без дедлайна** - `7716a65` (feat)

## Verification

- `cargo test --release -p app --bin cs2cinema`: 21 passed (в т.ч. `precheck_failure_reaches_the_job_row_with_its_report`, `cleanup_removes_only_our_files_inside_the_game_folder`, `nvenc_probe_that_hangs_is_cut_by_its_deadline`, `settings_without_afxerror_ignore_still_load`).
- `cargo build --release --workspace`: без ошибок и предупреждений; `cargo test --release -p cli --test pipeline`: 2 passed; `cargo test --release --workspace` зелёный.
- Критерии приёмки: `precheck_stage(&` выше `pipeline::analyze(`; `grep "\.output()"` по `crates/app/src`, `crates/pipeline/src`, `crates/host/src` и `grep check_paths crates/app/src` пусты.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocker] Проверка путей в `match_page::enqueue`**
- **Found during:** Task 3
- **Issue:** `match_page.rs` вызывал удаляемую `launch::check_paths` перед постановкой задачи с записью (перебрасывал на страницу настроек).
- **Fix:** вызов убран, автопоиск HLAE оставлен; пути проверяет предпроверка задачи и объясняет причину в карточке.
- **Files modified:** `crates/app/src/match_page.rs`
- **Commit:** `7716a65`

**2. [Rule 1 - Bug] `recover` не должен помечать целые дубли оборванными**
- **Found during:** Task 2
- **Issue:** по тексту плана `done_takes` берётся из `installed.json`; если записи уже нет (запись удалась и убрана, приложение упало на склейке), список пуст и `mark_partial` переименовал бы все дубли в `.partial`.
- **Fix:** пометка идёт только когда `installed.json` существует и разобран; тест `recover_without_a_record_keeps_finished_takes`.
- **Files modified:** `crates/app/src/launch.rs`
- **Commit:** `9eb975d`

**3. [Rule 2 - Missing critical] Недоубранные пути прошлой установки**
- **Found during:** Task 2
- **Issue:** повторная `install_files` перезаписала бы `installed.json` и потеряла пути, отложенные прошлой уборкой («уборка отложена»).
- **Fix:** пути старой записи сохраняются в новой, `done_takes` обнуляется.
- **Files modified:** `crates/app/src/launch.rs`
- **Commit:** `9eb975d`

**4. Второй проход места внутри ветки «дубли не записаны»** (проектное решение в рамках «после плана и до установки файлов»): задача, у которой остаётся только склейка, не блокируется нехваткой места под запись.

**5.** `pipeline::check(p)?` после проб NVENC стоит внутри блока, до записи «NVENC недоступен», чтобы отмена не превращалась в строку журнала; блок целиком выполняется только при `gpu_encode`.

**Total deviations:** 3 auto-fixed (1 blocker, 1 bug, 1 missing critical), 2 уточнения плана. **Impact:** без изменения целей плана.

## Known Gaps (записаны, не закрыты)

- `git ls-remote` в `geometry::latest_release_tag` (стадия плана при устаревших картах) остаётся без дедлайна (T-02-23, accept); склейка `renderer::assemble` для CLI — фаза 4.
- Разовая уборка старых копий `cs2cinema_*.dem`, оставшихся в папке игры от прошлых запусков (до этой фазы), не входит в D-05/D-06 и вынесена в отложенное.
- Отмена во время скачивания и разбора срабатывает на ближайшей границе этапа (`ureq` без отмены) — принятое ограничение; цель ~10 с обеспечена на предпроверке, пробах, записи и склейке.
- Ручная проверка настоящей записи, уборки и восстановления после падения — план 02-08.

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей сверх `<threat_model>` плана (T-02-20 закрыт `is_ours` и тестом с посторонним файлом и `..`, T-02-21 — уборка после возврата `record` с тремя попытками, T-02-22 — дедлайны `nvenc_probe`/`thumbnail`).

## Next Phase Readiness

02-07 может использовать `jobs::precheck_inputs` и `jobs_page::precheck_view` для кнопки «Проверить окружение» в настройках; 02-08 — ручная проверка записи, отмены и восстановления после падения.

## Self-Check: PASSED

- Файлы изменены и существуют: `crates/app/src/{jobs,jobs_page,launch,app,settings,match_page}.rs`, `crates/pipeline/src/lib.rs`.
- Коммиты `4a8a0ea`, `9eb975d`, `7716a65` найдены в истории ветки.
