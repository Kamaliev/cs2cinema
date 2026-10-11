---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 05
subsystem: pipeline
tags: [rust, pipeline, job, run_job, nvenc, precheck, verify, app]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::job (JobConfig, JobSpec, Outcome, resume, общие шаги записи), Progress::artifact, pipeline::gamedir (04-04)"
provides:
  - "pipeline::job::run_job(host, &JobSpec, &dyn Progress) -> Result<Outcome, RunError>: первый запуск, «только план» и «Повторить» (ветка spec.resume)"
  - "pipeline::precheck::{nvenc_check, has_nvenc}: проба NVENC через Host::run с дедлайном и отменой"
  - "record_request принимает Verify обязательно: путь pipeline::job никогда не строит запись без проверки дублей"
  - "app::jobs без собственной оркестровки: job_spec -> run_job -> store_outcome"
affects: [04-06, 04-07, 04-08, cs2-cli]

actuals:
  tokens: 10600
  tasks: 2
  commits: 2

plan_head_before: 929386b
plan_head_after: 1ef7f3014542989fd5065b014b6134f8430a43e9
commits: 2

tech-stack:
  added: []
  patterns:
    - "одна функция на всю задачу: приложение и CLI различаются только сборкой JobSpec и сохранением итога"
    - "обязательность проверки дублей закреплена типом: Verify вместо Option<Verify> в record_request"

key-files:
  created: []
  modified:
    - crates/pipeline/src/job.rs
    - crates/pipeline/src/precheck/gather.rs
    - crates/pipeline/src/precheck/mod.rs
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/tests/job.rs
    - crates/app/src/jobs.rs
    - crates/app/src/launch.rs

key-decisions:
  - "DC-1 принят как в плане: RecordRequest.verify остаётся Option на уровне машины записи (тесты на виртуальных часах без файлов), а на границе pipeline::job проверка обязательна"
  - "Обязательность усилена типом: record_request(.., verify: Verify) вместо Option; в job.rs нет ни одного вызова с пустой проверкой, даже в тестах"
  - "Проба NVENC для склейки идёт у ffmpeg из JobConfig.ffmpeg (в приложении это по-прежнему «ffmpeg» из PATH), а не у зашитого Path::new(\"ffmpeg\")"

requirements-completed: [TST-03]

duration: 35min
completed: 2026-10-11
---

# Phase 4 Plan 05: Первый запуск задачи переехал в pipeline::job::run_job Summary

**Вся задача на монтаж — предпроверка, скачивание и разбор, план, запись, склейка и «Повторить» — выполняется одной функцией `pipeline::job::run_job`; приложение только собирает `JobSpec` и кладёт `Outcome` в строку базы, проба NVENC живёт в `pipeline::precheck`, а запись через `pipeline::job` всегда идёт с проверкой дублей.**

## Что сделано

- **Task 1 (tracer, `b6925f7`).** `nvenc_check` и `has_nvenc` перенесены из `app::launch` в `pipeline::precheck::gather` (реэкспорт в `precheck/mod.rs`) вместе с тестами `nvenc_probe_that_hangs_is_cut_by_its_deadline` и `nvenc_failure_names_the_driver_line_without_the_ffmpeg_prefix`. Новая `pipeline::job::run_job`: при `spec.resume` зовёт `resume`, иначе переносит ветку первого запуска из `app::jobs::run`: предпроверка только при `spec.record` и до скачивания; источник демки (известная копия, FACEIT или файл); `Artifact::Demo` после разбора FACEIT; игрок через `find_player`; `Artifact::OutDir` до плана; проба NVENC с дедлайном `nvenc_probe` и `check(p)?` сразу после неё; `PlanOptions` из `JobConfig`; `begin_plan`, `plan_player`; «только план» возвращает `Outcome` без записи; `record_json_for`, сохранение `record.json`, `space_pass`, `record_takes`, `assemble_stage`, `last_good`. Приватная `job_demo(spec)` выбирает вид демки для предпроверки. В `app::jobs` удалены `run`, `job_demo`, `needs_precheck` и два перенесённых теста; поток задачи зовёт `job::run_job(&*host, &job_spec(..), &rep).map(store_outcome)`. Из `app::launch` удалены `hlae_ffmpeg`, `has_nvenc`, `nvenc_check`. Новые тесты в `crates/pipeline/tests/job.rs`: `record_job_with_a_failing_precheck_stops_before_the_demo` (чужой `cs2.exe` -> `FOREIGN_GAME_RUNNING`, `Artifact::Precheck { passed: false }`, ни `Artifact::Demo`, ни «качаю демку») и `plan_only_job_skips_the_precheck_and_reports_a_missing_demo` (в ошибке путь к файлу, среди `host.runs()` нет `-version`). Трейсер-ворота: `<verify>` задачи прошла, «Tracer verified end-to-end — expanding».
- **Task 2 (`1ef7f30`).** Тесты `record_json_written_after_planning_passes_its_own_check` и `stale_record_json_is_removed_before_planning` уже жили в `pipeline::job` (перенесены в 04-04), в `app` их нет. `record_request` теперь принимает `Verify`, а не `Option<Verify>`; `record_takes` собирает `Verify` и передаёт его напрямую (раньше проверка добавлялась через `..record_request(.., None)`). Добавлены тест `record_request_always_carries_the_take_verification` и интеграционный `job_recording_always_verifies_takes`: после `record_takes` с попыткой 3 оба дубля `validated` с `attempt == 3`, снимок окружения в манифесте попытки. Док-комментарий поля `RecordRequest.verify` переписан по плану; тип поля не менялся.

## Проверка

- `cargo test --release -p pipeline --test job` — 10 passed (`record_job_with_a_failing_precheck_stops_before_the_demo ... ok`, `job_recording_always_verifies_takes ... ok`).
- `cargo test --release -p pipeline --lib precheck` — 30 passed (оба теста NVENC в `precheck::gather::tests`).
- `cargo test --release -p pipeline --test record` — 54 passed.
- `cargo test --release -p app --bin cs2cinema` — 18 passed.
- `cargo test --release --workspace` — без FAILED, включая `crates/cli/tests/pipeline.rs` (2 passed) и `crates/cli/tests/assemble.rs`.
- `cargo build --release --workspace` — Finished, без предупреждений (`cs2cinema.exe` в этот раз не был занят).
- Критерии приёмки по grep: `pub fn run_job(`, `Artifact::Demo`, `Artifact::OutDir`, `nvenc_check(`, `plan_player(` есть в `job.rs`; `pub fn nvenc_check(` и `pub fn has_nvenc(` в `gather.rs`; `fn run(`/`fn job_demo`/`fn needs_precheck` в `app/jobs.rs` = 0, `job::run_job(` есть; `fn nvenc_check|has_nvenc|hlae_ffmpeg` в `launch.rs` = 0; `verify: None` в `job.rs` = 0, `verify: Some(` есть; перенесённых тестов в `app/jobs.rs` нет.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] Обязательность проверки дублей закреплена типом, а не только текстовой проверкой**
- **Found during:** Task 2
- **Issue:** `record_takes` строил запрос через `..record_request(.., None)`, то есть пустая проверка присутствовала в production-коде как база для struct-update, а модульные тесты вызывали `record_request(.., None)` — формально «вызов с пустой проверкой» в файле, который по плану (T-04-11) должен их не содержать.
- **Fix:** параметр `verify` у `record_request` стал `Verify` (внутри оборачивается в `Some`); `record_takes` передаёт его напрямую; тесты используют помощник `verify()`.
- **Files modified:** `crates/pipeline/src/job.rs`
- **Commit:** `1ef7f30`

### Прочее

- Проба NVENC склейки использует `cfg.ffmpeg` (по умолчанию и в приложении — `"ffmpeg"`), а не жёстко `Path::new("ffmpeg")`: поведение приложения прежнее, но путь теперь настраивается через `JobConfig`.
- Тесты из `<action>` Task 2 про `record_json_for` и `begin_plan` переносить не пришлось: это сделал 04-04.

## Known Stubs

None.

## Threat Flags

None. T-04-11 закрыта: `record_request` без `Verify` не вызывается ни в production, ни в тестах `job.rs`; `job_recording_always_verifies_takes` проверяет `validated` по ffprobe; склейка по-прежнему берёт только `validated` дубли. T-04-12: ключ FACEIT идёт только в `Source::Faceit`, `Debug` у `JobConfig` его скрывает (тест из 04-04). T-04-13: проба NVENC перенесена без изменений — `Host::run` с дедлайном и отменой, `check(p)?` сразу после неё.

## Заметки для следующих планов

- 04-06 (`cs2-cli`): может звать `pipeline::job::run_job` с `JobSpec`, собранным из аргументов; `Progress::artifact` по умолчанию пуст, его CLI не обязан реализовывать.
- 04-07: в `app::launch` остаются `find_cs2`, `find_hlae`, `cs2_exe(&str)`, `no_window`, `old_logs`, `recover_after_crash`.
- `job_spec` в потоке задачи строится один раз до `run_job`; `known_demo` и `last_good` читаются из базы в этот момент, а не лениво внутри оркестровки.
- В рабочем дереве остались незакоммиченными файлы, не относящиеся к плану: `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `03-PATTERNS.md`, `04-PATTERNS.md`.

## Self-Check: PASSED

- FOUND: crates/pipeline/src/job.rs, crates/pipeline/src/precheck/gather.rs, crates/pipeline/tests/job.rs, crates/app/src/jobs.rs
- FOUND commits: b6925f7, 1ef7f30
