---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 04
subsystem: pipeline
tags: [rust, pipeline, gamedir, job, progress, artifact, resume, app]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::assemble, RunError в корне pipeline (04-01), FakeHost::sequence и Host::notify (04-03)"
provides:
  - "pipeline::gamedir: Installed, INSTALLED_FILE, install_files, is_script_name, is_ours (страж), cleanup_installed, note_take_done, kill_orphans, recover, cs2_exe"
  - "pipeline::Artifact { Demo, OutDir, Precheck, LastGood } и Progress::artifact с пустой реализацией по умолчанию"
  - "pipeline::job: JobConfig (Debug без ключа FACEIT), DemoSource, JobSpec, Outcome, deadlines, precheck_inputs, precheck_stage, space_pass, record_request, record_stage, record_takes, record_json_for, begin_plan, set_aside_clips, plan_hash, retry_no_data, all_takes_validated, last_good, resume"
  - "crates/pipeline/tests/job.rs: «Повторить» и склейка на FakeHost без базы"
  - "app::jobs::job_config и job_spec; JobReporter::artifact кладёт итоги шагов в базу"
affects: [04-05, 04-06, 04-07, 04-10, cs2-cli]

actuals:
  tokens: 26000
  tasks: 3
  commits: 3

plan_head_before: 74997f2761f991685928596d603fc30538bdd2ce
plan_head_after: 6d5d499c62b81a8cd4ca02771054792af00f2594
commits: 3

tech-stack:
  added: []
  patterns:
    - "оркестровка за &dyn Progress: итоги шагов уходят через Progress::artifact, pipeline не знает ни базы, ни настроек приложения"
    - "обёртка Progress (TakeMarker) отмечает записанные дубли в installed.json внутри record_takes, а не в приложении"
    - "расширенный вход вместо #[allow(clippy::too_many_arguments)]: номер задачи и попытки группой (i64, u32)"

key-files:
  created:
    - crates/pipeline/src/gamedir.rs
    - crates/pipeline/src/job.rs
    - crates/pipeline/tests/job.rs
  modified:
    - crates/pipeline/src/lib.rs
    - crates/pipeline/Cargo.toml
    - Cargo.lock
    - crates/app/src/jobs.rs
    - crates/app/src/launch.rs

key-decisions:
  - "Граница DC-1: всё, что jobs.rs брал из базы, приходит во JobSpec (known_demo, last_good, out_dir, attempt, job_id); всё, что писал в базу, уходит через Progress::artifact"
  - "cs2_dir в gamedir — &Path; пустой путь означает «папка CS2 неизвестна», страж is_ours отказывает (тест empty_game_folder_removes_nothing)"
  - "JobConfig печатает ключ FACEIT в Debug только как «задан»/«пусто»"
  - "record_takes принимает номер задачи и попытки кортежем ids: (i64, u32), всего 7 параметров — без allow(too_many_arguments)"
  - "Первый запуск (run) пока пишет в базу напрямую (remember_demo, set_out_dir) — это уберёт 04-05"

requirements-completed: [TST-03]

duration: 75min
completed: 2026-10-11
---

# Phase 4 Plan 04: Оркестровка записи и «Повторить» переехали в pipeline Summary

**Папка игры (`pipeline::gamedir`), общие шаги записи (`pipeline::job`) и «Повторить» целиком (`pipeline::job::resume`) живут в `pipeline` за `Progress`; итоги шагов уходят через `Progress::artifact`, `JobReporter` приложения кладёт их в базу, а `pipeline::job` не упоминает ни `Store`, ни `Settings`.**

## Что сделано

- **Task 1 (tracer, `716fe0a`).** `crates/pipeline/src/gamedir.rs`: перенесены `Installed`, `INSTALLED_FILE`, `install_files`, `is_script_name` (теперь `pub`), `is_ours`, `cleanup_installed`, `remove_with_retry`, `note_take_done`, `kill_orphans`, `finished_takes`, `recover`; новый `cs2_exe(&Path)`. Параметр `cs2_dir` стал `&Path`, пустой путь по-прежнему отказывает стражу. Тесты ушли вместе с кодом; `malformed_processes_json_kills_nothing` в `gamedir` переписан на `kill_orphans` без базы (версия в `app` через `recover_after_crash` осталась и проверяет строку журнала). Добавлены `orphan_is_killed_through_its_full_identity` и `empty_game_folder_removes_nothing`. `launch::recover_after_crash` зовёт `pipeline::gamedir`. Трейсер-ворота: обе проверки `<verify>` зелёные, расширение выполнено.
- **Task 2 (`666aac3`).** `Progress::artifact(&Artifact)`; `pipeline::job` с `JobConfig`/`JobSpec`/`Outcome` и общими шагами записи (тексты и поведение перенесены дословно, `Store` заменён на `p.artifact`). `record_takes` оборачивает `Progress` в `TakeMarker`, который на `Event::TakeDone` зовёт `gamedir::note_take_done(out_dir, n - 1)`. `JobReporter` потерял поле `out_dir: OnceLock` и получил `artifact`; в `app::jobs` появились `job_config` и `job_spec`, `precheck_inputs` остался тонкой обёрткой для кнопки в настройках. Перенесены тесты `plan_hash`, `set_aside_clips`, `time_margin`, `afxerror_ignore`, кадр из `record.json`, `record_json_for`, `begin_plan`; добавлены `failing_precheck_returns_a_typed_failure_and_reports_the_json`, `passing_precheck_reports_passed`, `job_config_debug_hides_the_faceit_key` и `job_reporter_turns_artifacts_into_store_rows`.
- **Task 3 (`6d5d499`).** `pipeline::job::resume` — перенос `resume_job`: папка из `JobSpec`, `Artifact::OutDir`, `RecordJson::load`, сверка `plan_hash`, `Manifest::reconcile`, ветка «все проверены — сразу склейка», иначе предпроверка, `space_pass`, `record_takes`, склейка, `last_good`. Шесть тестов повтора и склейки переехали в `crates/pipeline/tests/job.rs` с прежними именами (плюс `retry_without_a_known_job_folder_fails_with_retry_no_data`). В `app::jobs::run` ветка `job.resume` зовёт `job::resume(host, &job_spec(..), p).map(store_outcome)`.

## Проверка

- `cargo test --release -p pipeline --lib gamedir` — 10 passed (`cleanup_removes_only_our_files_inside_the_game_folder ... ok`).
- `cargo test --release -p pipeline --lib job` — 14 passed (`failing_precheck_returns_a_typed_failure_and_reports_the_json ... ok`).
- `cargo test --release -p pipeline --test job` — 7 passed (`retry_with_an_invalid_take_sets_clips_aside_and_records_again ... ok`).
- `cargo test --release -p app --bin cs2cinema` — 21 passed (`startup_kills_the_orphan_before_cleanup_and_marks_the_job_interrupted`, `job_reporter_turns_artifacts_into_store_rows` — ok).
- `cargo test --release --workspace` — все наборы зелёные, включая `crates/cli/tests/pipeline.rs`.
- Критерии приёмки по grep: `grep -c "store::\|&Store\|Settings" crates/pipeline/src/job.rs` = 0; `OnceLock` в `app/jobs.rs` = 0; `fn resume_job` = 0; `job::resume(` в `app/jobs.rs` есть; в `launch.rs` нет `install_files`/`cleanup_installed`/`is_ours`/`kill_orphans`/`recover(`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Зависимость `chrono` в `pipeline`**
- **Found during:** Task 2
- **Issue:** `record_request` ставит в заголовок журнала записи локальное время (`%Y-%m-%d %H:%M:%S`); в `pipeline` `chrono` не было, а сигнатура `record_request` по плану не принимает время извне.
- **Fix:** `chrono = { version = "0.4", default-features = false, features = ["clock"] }` (та же версия и фичи, что в `app`; пакет уже в `Cargo.lock`, новых пакетов из реестра нет — T-04-SC не затронута).
- **Files modified:** `crates/pipeline/Cargo.toml`, `Cargo.lock`
- **Commit:** `666aac3`

**2. [Rule 1 - перенос тестов] Дополнительные тесты вместе с кодом**
- В `job.rs` перенесены также `record_json_written_after_planning_passes_its_own_check` и `stale_record_json_is_removed_before_planning` (в плане не перечислены, но проверяют `record_json_for` и `begin_plan`, которые переехали); в `gamedir` добавлены два теста про `kill_orphans` и пустую папку CS2.
- **Commit:** `716fe0a`, `666aac3`

### Прочее

- Между Task 2 и Task 3 `resume_job` в `app` временно звал функции `pipeline::job` (чтобы коммит Task 2 оставался рабочим); `job_spec` на время Task 2 давал предупреждение `dead_code`, в Task 3 оно ушло.
- `cargo build -p app` не смог перезаписать `target/release/cs2cinema.exe` («Отказано в доступе»): файл занят запущенным приложением. Это не ошибка кода — `cargo check` и тестовые сборки чистые, без предупреждений; бинарник не пересобирался.
- Форматирование: новые файлы `pipeline` отформатированы `rustfmt` по умолчанию, `app` — с `max_width=160`; остальные файлы `pipeline` не трогались.

## Known Stubs

None.

## Threat Flags

None — новой поверхности атаки сверх `<threat_model>` нет. T-04-08 закрыта: страж `is_ours` перенесён дословно (путь без `..`, родитель после канонизации — ровно `<CS2>/game/csgo` или `.../cfg`, пустая папка CS2 ничего не удаляет), тесты переехали с кодом. T-04-09 закрыта: `kill_orphans` идёт только через `Host::kill_orphan` с полной идентичностью, испорченный `processes.json` ничего не завершает. T-04-10 закрыта: ручной `Debug` для `JobConfig` без ключа, `Artifact` несёт только пути, JSON отчёта предпроверки и версии.

## Заметки для следующих планов

- 04-05: `run` в `app/jobs.rs` ещё пишет в базу напрямую (`store.remember_demo`, `store.set_out_dir`) и сам строит план/запись из `Settings`; его переносит `pipeline::job::run_job`. `job_spec` и `JobConfig` для этого уже готовы, `Artifact::Demo`/`OutDir` реализованы в `JobReporter`.
- 04-07: `launch::nvenc_check`, `has_nvenc`, `hlae_ffmpeg`, `find_cs2`/`find_hlae`, `cs2_exe(&str)`, `no_window`, `old_logs` пока остаются в `app`.
- В рабочем дереве остались незакоммиченными файлы, не относящиеся к плану: `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `03-PATTERNS.md`, `04-PATTERNS.md` — в коммиты задач они не попали.

## Self-Check: PASSED

- FOUND: crates/pipeline/src/gamedir.rs, crates/pipeline/src/job.rs, crates/pipeline/tests/job.rs
- FOUND commits: 716fe0a, 666aac3, 6d5d499
