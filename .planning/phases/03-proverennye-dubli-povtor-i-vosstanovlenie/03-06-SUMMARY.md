---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 06
subsystem: app
tags: [rust, jobs, retry, record-json, manifest, assemble, part-file, env-snapshot]

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "Job.resume, requeue_resume, JobReporter.attempt (03-01); FakeHost runs/launches/clips_dir/program_for (03-02); RecordJson, Manifest, EnvSnapshot, TAKE_INVALID, RETRY_NO_DATA, write_demo_copy (03-03); RecordRequest.verify (03-04)"
provides:
  - "jobs::run(host, job, s, store, p): хост снаружи; ui::open_path перенесён в run_next"
  - "jobs::resume_job: «Повторить» по record.json без скачивания, разбора и плана; все дубли валидированы -> сразу склейка, иначе запись всех дублей заново"
  - "jobs::assemble_stage: склейка только из валидированных дублей, через highlights.part.mp4 с переименованием после кода 0"
  - "jobs::record_takes: одна дорога записи (дубли в сторону, снимок окружения, установка, запись с проверкой, уборка игры)"
  - "jobs::record_request(s, out_dir, rj, job_id, verify), record_json_for, set_aside_clips, part_args, retry_no_data, begin_plan"
affects: [03-07, 03-08]

actuals:
  tokens: 17000
  tasks: 3
  commits: 4

plan_head_before: f5c10f743e3b53318adb0a8b1778fc88149339c5
plan_head_after: 1c338a054c28aac847c93901432aab20319235e1
commits: 4

tech-stack:
  added: []
  patterns:
    - "одна дорога записи для первого запуска, «Заново» и «Повторить»: record_takes, а вход берётся из RecordJson, не из плана"
    - "результат пишется во временное имя и переименовывается после успеха; при сбое удаляется только временный файл"
    - "недоверенное решение «склеить без записи» принимается по манифесту, сверенному с диском (reconcile + проверка размеров перед ffmpeg)"

key-files:
  created: []
  modified:
    - crates/app/src/jobs.rs

key-decisions:
  - "Повтор с хоть одним невалидированным дублем записывает ВСЕ дубли заново, прежнюю папку clips целиком откладывает в clips_old_<ts> (D-07, запасной вариант): HLAE нумерует дубли «наибольший + 1»"
  - "Манифест из папки задачи принимается повтором, только если его plan_hash равен plan_hash из record.json; иначе дубли считаются непроверенными"
  - "RecordJson.secs хранит длину ролика по плану (timeline.total_secs), т.е. то, что показывает карточка; в 03-03 поле называлось «суммарная игровая длительность дублей»"
  - "Демка для записи копируется из пути в record.json (write_demo_copy), а не из памяти Analysis: первый запуск и повтор идут одним кодом"
  - "Первый запуск без отчёта предпроверки (теоретически) пишет снимок окружения без версий — запись не блокируется"

patterns-established:
  - "Тестовая папка повтора retry_job(tag, statuses): скрипты, record.json, дубли на диске и манифест с заданными статусами, задача requeue_resume + start_job"

requirements-completed: [RET-02, RET-03, RET-06, TAK-04, TAK-05, TAK-06, OBS-03]

coverage:
  - id: D1
    description: "«Повторить» при всех валидированных дублях сразу склеивает ролик без игры и без предпроверки, через highlights.part.mp4"
    requirement: RET-03
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#retry_with_every_take_validated_goes_straight_to_assembly"
        status: pass
    human_judgment: false
  - id: D2
    description: "Повтор без record.json или с изменёнными скриптами падает кодом RETRY_NO_DATA с «запустите задачу заново»"
    requirement: RET-02
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#retry_without_record_json_fails_with_retry_no_data"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#retry_with_changed_scripts_fails_with_retry_no_data"
        status: pass
    human_judgment: false
  - id: D3
    description: "Склейка отказывается от дубля, не валидированного по манифесту или с изменившимся размером, кодом TAKE_INVALID до запуска ffmpeg"
    requirement: TAK-05
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#assembly_refuses_a_take_that_is_not_validated"
        status: pass
    human_judgment: false
  - id: D4
    description: "Сбой ffmpeg склейки не трогает прежний highlights.mp4 и удаляет highlights.part.mp4"
    requirement: TAK-06
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#failed_assembly_keeps_the_previous_highlights_and_drops_the_part"
        status: pass
    human_judgment: false
  - id: D5
    description: "Повтор с негодным дублем: предпроверка с демкой из record.json, прежние дубли в clips_old_<ts>, все дубли записаны заново в кадре из record.json, снимок окружения в env.json, манифесте и журнале"
    requirement: RET-06
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#retry_with_an_invalid_take_sets_clips_aside_and_records_again"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#resume_launches_at_the_recorded_frame_not_the_current_setting"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#set_aside_clips_never_overwrites_an_earlier_set_aside_folder"
        status: pass
    human_judgment: false
  - id: D6
    description: "Первый запуск и «Заново» удаляют устаревший record.json до плана и пишут новый сразу после плана, проходящий собственную проверку"
    requirement: RET-02
    verification:
      - kind: unit
        ref: "crates/app/src/jobs.rs#record_json_written_after_planning_passes_its_own_check"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#stale_record_json_is_removed_before_planning"
        status: pass
    human_judgment: false
  - id: D7
    description: "Живой повтор на настоящих HLAE и CS2: удалённый дубль -> «Повторить»; падение на склейке -> «Повторить» сразу склеивает"
    verification: []
    human_judgment: true
    rationale: "Нужны настоящие игра и HLAE; запланировано в 03-08"

duration: 40min
completed: 2026-10-11
status: complete
---

# Phase 3 Plan 06: Повтор с записи и проверенная склейка Summary

**«Повторить» в приложении берёт всё из `record.json`: при всех валидированных дублях сразу склеивает ролик (без игры и предпроверки), иначе откладывает прежние дубли в `clips_old_<ts>` и записывает все заново в кадре из `record.json`; склейка идёт только из проверенных по манифесту дублей и через `highlights.part.mp4`.**

## Что сделано

- **Task 1 (tracer, `2e5e868`).** `run` получил `host: &dyn Host` (хост создаёт `run_next`), открытие ролика в плеере перенесено в `run_next`. `resume_job`: папка из `job.out_dir`, `RecordJson::load`, сверка `plan_hash` скриптов, `Manifest::load` + `reconcile`, при всех валидированных дублях — сразу `assemble_stage`. `assemble_stage`: проверка манифеста и размеров файлов (`TAKE_INVALID` с номером дубля до запуска ffmpeg), ffmpeg пишет `highlights.part.mp4`, `rename` после кода 0, при сбое удаляется только `.part`, при отказе `rename` — русское объяснение, `.part` остаётся. `part_args`, `retry_no_data`. Трассер пройден: `run` на `FakeHost` без запуска игры и без `-version`.
- **Task 2 (`ff094d0`).** `record_request(s, out_dir, rj, job_id, verify)` берёт кадр, метку, имя демки и дубли из `record.json`. `set_aside_clips` (`clips_old_<сек>`, суффиксы `-2`, `-3`). `record_takes` — единая дорога: отложить `clips`, снимок окружения (`env.json`, манифест попытки, строка «окружение: …» в журнале), `install_files` с `write_demo_copy`, запись с `verify: Some(..)`, уборка папки игры при любом исходе. Ветка «не все валидированы» в `resume_job`: предпроверка с демкой из `record.json` (нет демки — `PRECHECK_FAILED`), проход по месту, запись, склейка, `set_last_good`. Первый запуск тоже идёт через `record_json_for` и `record_takes`; файл-отпечаток `clips/plan.hash` и ветка «уже записано» удалены.
- **Task 3 (`41ea9ff`).** `begin_plan` удаляет устаревший `record.json` до `plan_player`; новый `record.json` сохраняется сразу после плана и проверки пути (ошибка сохранения — ошибка задачи). Тесты: `record.json` проходит собственную проверку, устаревший файл убран.
- **`1c338a0`.** Мелкий рефакторинг: `verify: Some(..)` явно в запросе записи (под key_link плана).

## Deviations from Plan

### Auto-fixed Issues

Нет — Rules 1-3 не срабатывали.

### Прочее

- Граница Task 2/Task 3 сдвинута: чтобы `record_request` мог принимать `RecordJson`, `record_json_for` и перевод первого запуска на `record_takes` вошли в коммит Task 2; Task 3 добавил удаление устаревшего `record.json`, сохранение нового и тесты. Содержимое плана выполнено целиком.
- В коммит Task 1 первый запуск уже включает `verify: Some(..)` (иначе `assemble_stage` не нашёл бы манифест); старая проверка «дубли этого плана уже записаны» осталась до Task 2.
- `RecordJson.secs` = длина ролика по плану, а не сумма игровых секунд дублей (см. key-decisions): так `JobOutcome` повтора совпадает с первым запуском.
- Манифест, чей `plan_hash` не совпадает с `record.json`, повтор не принимает (дополнительная защита от чужого манифеста, T-03-16).
- Тестовый `request()` не менялся: он собирает `RecordRequest` напрямую, а не через `record_request`.
- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как в 03-01..03-05).
- Предупреждения clippy `result_large_err` для `RunError` существовали до плана; не трогались.

## Проверка

- `cargo test --release -p app --bin cs2cinema retry` — зелёный (в т.ч. `retry_with_every_take_validated_goes_straight_to_assembly`); `... assembl` — 4 passed; `... retry_with_an_invalid_take` — 1 passed; `... record_json` — зелёный.
- `cargo test --release -p app --bin cs2cinema` — 36 passed.
- `cargo test --release --workspace` — зелёный, включая `crates/cli/tests/pipeline.rs` (2 passed); `cargo build --release --workspace` — без ошибок.
- `awk '/^fn run\(/,/^}/' crates/app/src/jobs.rs | grep -c open_path` — 0; `grep -c 'clips.join("plan.hash")'` — 0.
- `rustfmt --edition 2024 --config max_width=160,use_small_heuristics=Max crates/app/src/jobs.rs` применён.
- Живой повтор на настоящих HLAE и CS2 — план 03-08.

## Для 03-07

- `run_next` теперь передаёт `&*host` в `run`; поток открывает ролик после `finish`.
- `Job.resume` читается в `run`; кнопка «Повторить» должна вызывать `requeue_resume`, «Заново» — `requeue` (сбрасывает `resume`). Условие «Повторить возможна» — наличие `record.json`; иначе задача упадёт `RETRY_NO_DATA`.
- `processes.json` и `kill_orphan` в этом плане не трогались.

## Known Stubs

Нет.

## Threat Flags

Нет нового внешнего риска сверх T-03-15..T-03-17 плана: `RecordJson::load` проверяет поля, хеш скриптов и демка (предпроверка) сверяются до записи, ffmpeg получает вектор аргументов через `Host::run` (T-03-15); перед склейкой размеры файлов сверяются с манифестом (T-03-16); ролик пишется в `.part` и переименовывается только после кода 0 (T-03-17).

## Self-Check: PASSED

- FOUND: crates/app/src/jobs.rs (`fn resume_job(`, `pub fn assemble_stage(`, `fn part_args(`, `fn retry_no_data(`, `fn set_aside_clips(`, `fn record_takes(`, `fn record_json_for(`, `fn begin_plan(`, `highlights.part.mp4`, `verify: Some(`)
- FOUND commits: 2e5e868, ff094d0, 41ea9ff, 1c338a0
