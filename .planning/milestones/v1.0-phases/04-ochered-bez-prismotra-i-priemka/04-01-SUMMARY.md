---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 01
subsystem: pipeline
tags: [rust, pipeline, assemble, ffmpeg, cli, host, progress]
status: complete

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "Manifest/TakeStatus/RecordJson (03-03), Host::run с дедлайном и отменой, assemble_stage в app (03-05/03-06)"
provides:
  - "pipeline::assemble: assemble_stage, run_ffmpeg, part_args, take_invalid_failure, ffmpeg_error, PART_NAME"
  - "pipeline::RunError (текст + необязательный RecordFailure) в корне крейта"
  - "cs2-cli assemble <папка задачи>: склейка из record.json и clips/manifest.json тем же кодом, что в приложении"
  - "cli::ConsoleProgress — реализация pipeline::Progress для консоли (этапы и журнал в stderr)"
affects: [04-02, 04-03, 04-04, pipeline::job, cs2-cli]

actuals:
  tokens: 11500
  tasks: 2
  commits: 2

plan_head_before: 830353243520413e17f06662cdad46b3b46129fd
plan_head_after: 34649a0f87212d7f008e3e4042fd49605ffaf764
commits: 2

tech-stack:
  added: []
  patterns:
    - "общий код за &dyn Progress: приложение и CLI вызывают одну функцию этапа"
    - "ffmpeg запускается только через Host::run (дедлайн + отмена); запуска без дедлайна в workspace нет"

key-files:
  created:
    - crates/pipeline/src/assemble.rs
    - crates/cli/tests/assemble.rs
  modified:
    - crates/pipeline/src/lib.rs
    - crates/app/src/jobs.rs
    - crates/cli/Cargo.toml
    - crates/cli/src/main.rs
    - crates/renderer/src/lib.rs
    - Cargo.lock
  deleted:
    - crates/renderer/src/assemble.rs

key-decisions:
  - "DC-1 по рекомендации исследования: cs2-cli assemble принимает папку задачи, а не --plan/--clips; старые флаги дают объяснение и код выхода 2"
  - "RunError переехала в корень pipeline без изменений (используют склейка, будущий pipeline::job и CLI)"
  - "Ошибка assemble с типизированным сбоем печатается как «ошибка: КОД: сообщение» + «подсказка: …», код выхода 1; ошибки аргументов — код 2"

requirements-completed: [TST-03]

duration: 25min
completed: 2026-10-11
---

# Phase 4 Plan 01: Склейка одним кодом для приложения и CLI Summary

**Склейка ролика из проверенных дублей переехала из `app` в `pipeline::assemble`; `cs2-cli assemble <папка задачи>` и приложение вызывают одну `assemble_stage` через `Host::run` с дедлайном, а старая склейка `renderer::assemble` (без звука, без дедлайна) удалена вместе с флагами `--plan/--clips`.**

## Что сделано

- **Task 1 (tracer, `f8d77b2`).** Создан `crates/pipeline/src/assemble.rs`: `PART_NAME`, `part_args`, `check_take_for_assembly` (приватная), `assemble_stage`, `take_invalid_failure`, `ffmpeg_error`, `run_ffmpeg` перенесены из `jobs.rs` без изменения поведения и текстов. `RunError` с `From<String>`/`From<&str>` вынесена в корень `pipeline`. В `app` перенесённый код и локальная `RunError` удалены, импорт `pipeline::{RunError, assemble::assemble_stage}`; вызовы в `run` и `resume_job` не менялись. В `cli` добавлена зависимость `pipeline`, `ConsoleProgress` и новый `assemble_cmd`: `RecordJson::load(папка)` -> `assemble_stage(default_host(), …, Deadlines::default().ffmpeg_for(video_secs), &ConsoleProgress)`. Юнит-тесты `ffmpeg_failures_are_explained_in_russian` (перенесён) и `part_args_swaps_only_the_final_highlights_name` (новый); интеграционные тесты бинарника `assemble_without_manifest_fails_with_take_invalid_before_ffmpeg` и `assemble_without_record_json_names_the_file`.
- **Task 2 (`34649a0`).** Удалён `crates/renderer/src/assemble.rs` и `pub mod assemble;`; генерация `assemble.ffgraph/.sh/.bat` и `ffmpeg_args` в `renderer` остались. `cs2-cli assemble --plan …` печатает «старая склейка по --plan/--clips удалена … cs2-cli assemble <папка задачи>» и `USAGE`, код выхода 2. Тест `old_plan_and_clips_flags_explain_the_replacement`. Раздел `USAGE` про склейку заменён описанием нового вызова (сделан в Task 1, чтобы справка не врала между коммитами).

## Проверка

- `cargo test --release -p pipeline --lib assemble` — 2 passed.
- `cargo test --release -p cli --test assemble` — 3 passed (включая `assemble_without_manifest_fails_with_take_invalid_before_ffmpeg`).
- `cargo test --release -p app --bin cs2cinema` — 42 passed.
- `cargo test --release -p renderer` — 8 passed.
- `cargo test --release -p cli --test pipeline` — 2 passed.
- `cargo build --release --workspace` — без ошибок и предупреждений.
- Критерии приёмки по grep: `renderer/src/assemble.rs` отсутствует; `mod assemble` в `renderer/src/lib.rs` — 0; `renderer::assemble` в коде — 0; `Command::new("ffmpeg")` в `renderer/src/lib.rs` — 0; `fn assemble_stage|fn run_ffmpeg|pub struct RunError` в `jobs.rs` — 0.
- Трейсер-ворота: проверка `<verify>` Task 1 перепрогнана целиком после правок; зелёная, расширение (Task 2) выполнено.

## Deviations from Plan

None - plan executed exactly as written.

Мелкие уточнения без отклонения от замысла: `HostError` в одном из тестов `jobs.rs` (`run_result(Err(HostError::Timeout))`) теперь пишется полным путём `pipeline::host::HostError`, потому что импорт `HostError` и `CmdSpec` в `jobs.rs` стал неиспользуемым и удалён. Форматирование применено только к новым файлам (`rustfmt` по ним), остальные файлы крейтов `cli` и `pipeline` уже не соответствовали `cargo fmt` до этого плана и не трогались, чтобы не раздувать дифф.

## Known Stubs

None.

## Threat Flags

None — новой поверхности атаки сверх `<threat_model>` нет. T-04-01 закрыта: `RecordJson::load` проверяет поля, склейка идёт только из `validated` дублей с совпавшими размерами, ffmpeg получает вектор аргументов через `Host::run`. T-04-02 закрыта: дедлайн `Deadlines::ffmpeg_for`, запуска ffmpeg без дедлайна в `renderer` больше нет.

## Заметки

- Оценка плана (70000 токенов, 2 задачи) оказалась завышенной: фактически по тем же шкале chars/4 над изменённым кодом — около 11500 токенов.
- В рабочем дереве остались незакоммиченными файлы, не относящиеся к плану: `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `03-PATTERNS.md`, `04-PATTERNS.md` — в коммиты задач они не попали.

## Self-Check: PASSED

- FOUND: crates/pipeline/src/assemble.rs, crates/cli/tests/assemble.rs
- MISSING (как и должно быть): crates/renderer/src/assemble.rs
- FOUND commits: f8d77b2, 34649a0
