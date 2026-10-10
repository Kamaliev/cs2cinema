---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 07
subsystem: app
tags: [rust, egui, app, pipeline-record, job-card, settings, ffmpeg-deadline]

requires:
  - phase: 01-02
    provides: "Ladder/Event/RecordState и контракт записи"
  - phase: 01-03
    provides: "store: fail_job, JobFailure, set_phase, set_out_dir, колонки fail_* и phase (six-columns)"
  - phase: 01-04
    provides: "pipeline::record, RecordFailure, Deadlines::{scaled, ffmpeg_for}"
  - phase: 01-06
    provides: "WinHost, host::default_host(), Host::run с дедлайном"
  - phase: 01-08
    provides: "console_tail, раздельные факты HLAE.exe и cs2.exe, подсказка в RecordFailure"
provides:
  - "app::jobs::{RunError, record_request, record_stage, finish, job_failure}: мост pipeline::record -> строка задачи"
  - "JobReporter::event: состояние записи в jobs.phase, событие в журнал задачи"
  - "Карточка задачи: лестница состояний и блок типизированной ошибки с хвостом консоли"
  - "Settings.time_margin («Запас по времени» ×0.5–×5.0) вместо load_wait_secs"
  - "Склейка ffmpeg через Host::run с Deadlines::ffmpeg_for × запас"
  - "launch.rs без запуска игры, консоли и завершения по имени образа"
affects: [01-verify, phase-3-takes, phase-4-cli-cutover]

actuals:
  tokens: 12400
  tasks: 3
  commits: 3
plan_head_before: d944739c743a8d0beb22dd766de9846a9a19e926
plan_head_after: ed91733f96dfc372ad44f665c079e52c4dfacaaf

tech-stack:
  added: []
  patterns:
    - "RunError { message, fail } + From<String>/From<&str>: все прежние `?` на Result<_, String> компилируются без правок; отмена остаётся сентинелом pipeline::CANCELLED"
    - "мост без зависимости app -> host: host приходит через pipeline::host (D-01)"
    - "тип сбоя -> строки для store в одном месте (job_failure), store не знает типов pipeline"

key-files:
  created: []
  modified:
    - crates/app/src/jobs.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/status.rs
    - crates/app/src/launch.rs
    - crates/app/src/settings.rs
    - crates/app/src/settings_page.rs

key-decisions:
  - "Добавлено Status.take_now (разбор строки «запись: дубль N из M»): лестница показывает «дубль N/M» с самого начала дубля, а не только после первого записанного"
  - "Макет файлов app форматировался не rustfmt по умолчанию, а с `max_width=160,use_small_heuristics=Max`: так прежний jobs.rs даёт 8 строк различий вместо сотен; полный `cargo fmt -p app` перепишет все файлы крейта — не запускать"
  - "RunError оставлен с полем fail: Option<RecordFailure> как в контракте плана, хотя clippy::result_large_err ругается на размер — не блокирует"

patterns-established:
  - "RED/GREEN не применялись (задачи tracer/auto): тесты на FakeHost идут вместе с кодом в одном коммите"

requirements-completed: [LCH-01, LCH-02, LCH-04, LCH-06, TMO-01, TAK-01]

coverage:
  - id: D1
    description: "Задача с записью идёт через pipeline::record на default_host(); сбой с фейка (CONSOLE_NOT_OPEN) доходит до строки задачи: fail_code/fail_state/fail_hint/fail_tail/fail_elapsed_ms, error со строкой фактов cs2.exe, record.log в папке задачи"
    requirement: "LCH-01"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#typed_failure_reaches_the_job_row"
        status: pass
    human_judgment: false
  - id: D2
    description: "Отмена записи даёт статус «отменена» через сентинел CANCELLED, без fail_code; игра закрыта"
    requirement: "LCH-02"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#cancelled_recording_becomes_a_cancelled_job"
        status: pass
    human_judgment: false
  - id: D3
    description: "Папка задачи записывается в строку до записи; record.log остаётся в ней после сбоя"
    requirement: "LCH-04"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#typed_failure_reaches_the_job_row"
        status: pass
    human_judgment: false
  - id: D4
    description: "Карточка упавшей задачи: код и состояние, сообщение, подсказка, «прошло N с», путь к record.log, сворачиваемый хвост консоли; лестница отмечает пройденные шаги, упавший и текущий"
    requirement: "LCH-06"
    verification:
      - kind: command
        ref: "cargo build --release -p app"
        status: pass
    human_judgment: true
    rationale: "Внешний вид карточки и лестницы в egui проверяет человек на реальной задаче; автотеста на отрисовку нет"
  - id: D5
    description: "«Запас по времени» вместо «Ожидание загрузки демки»: масштабирует дедлайны записи и склейки, старый settings.json читается; значение переживает перезапуск"
    requirement: "TMO-01"
    verification:
      - kind: unit
        ref: "crates/app/src/settings.rs#old_settings_file_with_load_wait_still_loads"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#time_margin_scales_the_assemble_deadline"
        status: pass
    human_judgment: true
    rationale: "Сохранение значения через интерфейс настроек и строка «запас по времени ×K» в живом журнале проверяются руками"
  - id: D6
    description: "Склейка через Host::run с дедлайном ffmpeg_for × запас; тексты ошибок по-русски (не уложился, отмена, не запустился, код выхода с хвостом stderr)"
    requirement: "TMO-01"
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#ffmpeg_failures_are_explained_in_russian"
        status: pass
    human_judgment: false
  - id: D7
    description: "Живая запись из приложения до highlights.mp4 на реальных HLAE и CS2, журнал с пройденными состояниями"
    requirement: "TAK-01"
    verification: []
    human_judgment: true
    rationale: "Реальные cs2.exe и HLAE.exe в плане не запускались (запрет оркестратора); проверка — в блоке «Human verification pending»"

duration: 25min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 07: Приложение на pipeline::record, карточка с лестницей и ошибкой Summary

**Запись в приложении идёт через `pipeline::record` на `host::default_host()`; типизированный сбой (`fail_code`, состояние, подсказка, хвост консоли, время) доходит до строки задачи и показывается в карточке рядом с лестницей «лаунчер → игра → консоль → демка → скрипт → дубль N/M → закрытие»; «Запас по времени» вместо ожидания загрузки масштабирует все дедлайны, склейка ffmpeg ограничена дедлайном через `Host::run`, старый запуск с завершением по имени образа удалён.**

## Performance

- **Duration:** около 25 мин
- **Completed:** 2026-10-10
- **Tasks:** 3 (1 tracer, 2 auto)
- **Files changed:** 6 (все изменённые, 381 добавлена / 375 удалено строк)

## Accomplishments

- **Мост записи (Task 1, tracer).** `RunError { message, fail }` с `From<String>`/`From<&str>`; `record_request` собирает `LaunchSpec` (hlae_dir, cs2_exe, ASCII-имя демки, размер, nonce, `console_log = game/csgo/console.log`) и `Deadlines`; `record_stage` превращает `FailCode::Cancelled` в сентинел `CANCELLED`, остальное в `RunError` с типизированным сбоем; `finish` пишет в базу `cancel_job` / `fail_job(job_failure)` / прежний `finish_job`. `JobReporter::event` пишет `Event::State` в `jobs.phase` (не в `stage`) и любое событие в журнал, поэтому строка «записан дубль N/M» по-прежнему доходит до `status.rs`. `run()` сразу после вычисления `out_dir` делает `set_out_dir`. Старая функция записи (netcon, `wait_for_clips`, повторы `exec highlights`) и константы `LOAD_CAP`, `EXEC_RETRY`, `EXEC_RETRIES`, `RECONNECT_EVERY` удалены. Tracer gate: `<verify>` автоматический, перепрогнан после форматирования — `typed_failure_reaches_the_job_row` проводит `CONSOLE_NOT_OPEN` от `FakeHost` через `pipeline::record` и мост до строки `Store` и `record.log`.
- **Карточка (Task 2).** `ladder` (пройденные шаги `✓`, текущий жирным у выполняемой, упавший `✕`, «дубль a/b» из `Status.take_now`) и `error_block` (код · состояние мелким шрифтом, сообщение `BAD` с переносом, подсказка, «прошло N с», `record.log: <папка>/record.log`, сворачиваемый «Последние строки консоли» с прокруткой и моноширинным текстом). Для сбоев без `fail_code` прежняя обрезанная строка.
- **Запас по времени и склейка (Task 3).** `Settings.time_margin: f32` (по умолчанию 1.0, `DragValue` ×0.5–×5.0, два знака), поле `load_wait_secs` удалено — старый `settings.json` загружается (структура с `#[serde(default)]`, лишние поля игнорируются; тест). `deadlines(s) = Deadlines::default().scaled(time_margin)` питает `RecordRequest` и дедлайн склейки `ffmpeg_for(video_secs)`. `run_ffmpeg` — через `Host::run` с `CmdSpec`, разбором `out_time_us=` и логом «склейка: N%» раз в 10 с; ошибки: «ffmpeg не уложился в N с — … увеличьте «Запас по времени»», отмена, «не удалось запустить ffmpeg (он должен быть в PATH)», «ffmpeg завершился с ошибкой: последние 6 строк».
- **Уборка `launch.rs`.** Удалены `NETCON_PORT`, `free_port`, `launch`, `Console`, `newest_mtime`, `game_alive`, `kill_game`, `wait_for_clips`, `dir_size`; остались `no_window`, `find_cs2`, `find_hlae`, `cs2_exe`, `check_paths`, `install_files`, `hlae_ffmpeg`, `has_nvenc`, `nvenc_check`. Сборка без предупреждений о мёртвом коде.

## Task Commits

1. **Task 1: запись в приложении через pipeline::record (tracer)** — `4b67b12` (feat)
2. **Task 2: карточка — лестница состояний и блок ошибки** — `98b8dab` (feat)
3. **Task 3: «Запас по времени», склейка через Host::run, уборка launch.rs** — `ed91733` (feat)

**Plan metadata:** отдельный коммит `docs(01-07)` (SUMMARY, STATE, ROADMAP, REQUIREMENTS).

## Verification

- `cargo test --release -p app --bin cs2cinema` — 8 passed: `typed_failure_reaches_the_job_row`, `cancelled_recording_becomes_a_cancelled_job`, `plain_error_keeps_the_old_failed_row`, `ffmpeg_failures_are_explained_in_russian`, `time_margin_scales_the_assemble_deadline`, `old_settings_file_with_load_wait_still_loads`, `current_take_comes_from_the_record_line`, `plan_hash_ignores_the_nonce`.
- `cargo test --release --workspace` — все тест-бинарники зелёные (в т.ч. `cli --test pipeline`, `host --lib` 41, `host --test win_session` 4); `cargo build --release --workspace` и `cargo build --release -p cli` без предупреждений.
- Критерии приёмки: `grep` по `LOAD_CAP|EXEC_RETRY|RECONNECT_EVERY|fn record(` в `jobs.rs`, `^host` в `app/Cargo.toml`, `load_wait_secs` в `app/src` (кроме теста совместимости), `taskkill|tasklist` в `app/src`, `fn (launch|game_alive|kill_game|wait_for_clips|free_port)\(|struct Console` в `launch.rs`, `Command::new("ffmpeg")` в `jobs.rs` — пусто; `ffmpeg_for` в `jobs.rs` встречается 4 раза.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] `Status.take_now` и разбор «запись: дубль N из M» (status.rs не в списке файлов плана)**
- **Found during:** Task 2
- **Issue:** `app.status.takes` заполняется только строкой «записан дубль a/b», то есть после первого готового дубля; до этого лестница не могла показать «дубль N/M» (критерий D-06).
- **Fix:** новое поле `Status.take_now`, заполняется из события `Event::State` в журнале («запись: дубль N из M»), сбрасывается при старте, смене этапа и окончании; тест `current_take_comes_from_the_record_line`.
- **Files modified:** `crates/app/src/status.rs`
- **Commit:** `98b8dab`

**2. [Rule 3 - Blocking] Форматирование крейта app**
- **Found during:** Task 1 (`cargo fmt -p app`)
- **Issue:** крейт `app` не форматирован rustfmt по умолчанию (длинные строки); `cargo fmt -p app` переписал 13 файлов.
- **Fix:** чужие файлы возвращены через `git checkout -- <файл>` (до коммита, измененными оставались только плановые); свои файлы форматировались `rustfmt --edition 2024 --config max_width=160,use_small_heuristics=Max`, что почти совпадает с прежним стилем. Две побочные правки rustfmt в `settings.rs`/`settings_page.rs` откатил вручную.
- **Commit:** `4b67b12`, `ed91733`

**Total deviations:** 2 (1 Rule 2, 1 Rule 3). **Impact:** контракт плана и наборы файлов не менялись, кроме добавления `status.rs`.

## Issues Encountered

- `cargo clippy -p app` ругается на `result_large_err` для `RunError` (две функции: `record_stage`, `finish`-вызов). Контракт плана фиксирует `fail: Option<RecordFailure>`, поэтому оставлено; при желании в фазе 4 — `Box<RecordFailure>`. Остальные предупреждения clippy в `app` (status.rs, jobs_page.rs:159/164, launch.rs:51, settings_page.rs:177) старые.
- В среде нет `python`; правки делались `Edit`, `sed`, `perl`. Perl с кириллицей в heredoc оказался ненадёжным (не находил подстроки) — для строк с русским текстом использовал `Edit`.

## Behavior Notes for the Next Plans

- **Что изменилось в дожидании клипов.** Прежний `wait_for_clips` ждал, пока папка последнего дубля перестанет меняться (3 с тишины после метки конца, 8 с после смерти игры, 20 с иначе). Новый путь считает дубль готовым по метке `TAKE_END`, затем `quit` и ожидание закрытия игры; существование `video.mp4` и `audio.wav` каждого дубля после записи приложение не проверяет (проверка дублей — TAK-02, фаза 3). Если на реальной записи склейка найдёт недописанный клип, это место для первой проверки.
- **Копия демки.** `install_files` по-прежнему кладёт демку в `game/csgo` под именем `cs2cinema_<console_name(label)>` (ASCII, без пробелов, `"` и `;` — требование `WinHost`, TMO-06 note в 01-06-SUMMARY.md). Удаление копии после записи в плане не требовалось и не сделано.
- **Журнал «запас по времени ×K»** пишется только когда дубли реально записываются (не при пропуске записи «дубли этого плана уже записаны»).

## Known Stubs

Нет.

## Threat Flags

Новой поверхности нет. T-01-26 (завершение игры по имени образа) закрыто: в `crates/app/src` нет `taskkill`/`tasklist`, игру закрывает только `Session::terminate` своей сессии. T-01-24: проверка `renderer::console_safe(absolute_slash(out_dir))` в `run()` осталась до записи; T-01-25: диапазон 0.5–5.0 в UI и повторное ограничение в `Deadlines::scaled` (тест с NaN в настройках); T-01-23 принят (egui выводит текст как есть).

## Human verification pending

Эти проверки НЕ выполнялись (запрет на запуск настоящих `cs2.exe`/`HLAE.exe` в этом плане) и НЕ отмечены пройденными. Собираются в UAT в конце фазы (`workflow.human_verify_mode: end-of-phase`).

**Подготовка:** машина автора; Steam запущен; свой CS2 закрыт; HLAE и CS2 указаны в настройках; ffmpeg в PATH. Запуск: `cargo run --release -p app`.

1. **Успешная запись (Task 1).** Поставить задачу «ролик» по ссылке FACEIT или по `.dem`. Ожидается:
   - появляется `highlights.mp4`;
   - в журнале задачи по порядку: «запись: запуск лаунчера (HLAE)», «HLAE.exe завершился с кодом 0», «запись: ожидание игры (cs2.exe)», «cs2.exe запущена (PID …)», «запись: игра запущена, жду консоль», «запись: консоль открыта, жду загрузку демки», «запись: демка готова, скрипт запущен», «запись: дубль 1 из M» … «записан дубль M/M», «запись: все дубли записаны, закрываю игру» и строка фактов «HLAE.exe: код 0 · cs2.exe: …»;
   - во время записи в карточке лестница подсвечивает текущий шаг, на шаге записи — «дубль a/b»;
   - в папке задачи есть `record.log` со строками `[netcon]`, `[console.log]`, `[app]` и строкой о числе строк stdout/stderr HLAE;
   - после задачи `cs2.exe` не запущен (диспетчер задач).
2. **Вызванный сбой (Task 2).** Поставить ещё одну задачу «ролик» и, когда на лестнице подсвечен шаг «дубль 1/M», закрыть окно CS2 вручную. Ожидается: после сбоя в карточке `GAME_EXITED · дубль` мелким шрифтом, сообщение «Игра закрылась на этапе «дубль»» со строкой фактов «… · cs2.exe: не жива …», подсказка, «прошло N с», путь `record.log`, раскрывающийся блок «Последние строки консоли» с моноширинным хвостом (повторы свёрнуты в «×N»); на лестнице «лаунчер → игра → консоль → демка → скрипт» отмечены ✓, «дубль» — ✕; `record.log` в папке задачи содержит итог сбоя; кнопка «Повторить» работает.
3. **Отмена.** Во время записи нажать «Отменить»: статус «отменена», блока ошибки нет, `cs2.exe` закрыт.
4. **«Запас по времени» (Task 3).** В «Настройках» вместо «Ожидание загрузки демки, с» есть «Запас по времени» со значением ×1.00; поставить ×2.00, сохранить, перезапустить приложение — значение сохранилось; в журнале следующей задачи с записью есть строка «запас по времени ×2.00». Открыть приложение со старым `settings.json` (с `load_wait_secs`) — запускается без ошибки.
5. **Склейка.** В журнале успешной задачи есть «склейка: N%» и «склейка заняла N с». (Необязательно: поставить ×0.5 и очень длинный ролик, чтобы увидеть «ffmpeg не уложился в N с».)

## Self-Check: PASSED

- Файлы: `crates/app/src/{jobs,jobs_page,status,launch,settings,settings_page}.rs` на месте.
- Коммиты `4b67b12`, `98b8dab`, `ed91733` найдены в истории; `git rev-list --count d944739..HEAD` до SUMMARY = 3.
- `cargo test --release --workspace`, `cargo build --release --workspace`, `cargo build --release -p cli` зелёные; все `grep`-критерии приёмки пусты/положительны, как указано выше.
