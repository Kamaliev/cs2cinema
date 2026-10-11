---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 06
subsystem: cli
tags: [rust, cli, record, retry, run_job]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::job::run_job, JobSpec, JobConfig, DemoSource, RETRY_NO_DATA (04-05)"
provides:
  - "cs2-cli retry <папка задачи>: «Повторить» тем же run_job, что и кнопка в приложении"
  - "cs2-cli record <ссылка FACEIT | .dem> --player <ник>: полная задача тем же run_job, что и очередь приложения"
  - "crates/cli/src/record.rs: ConsoleProgress, print_failure, parse_record, parse_retry, record_cmd, retry_cmd, USAGE_RECORD"
affects: [04-07, 04-08]

actuals:
  tokens: 5660
  tasks: 2
  commits: 3

plan_head_before: 3f98beff6cc40214cdf50932fd966850f2b5d82c
plan_head_after: 63b7848985d690c6a82435cc443425ebe4e64b0a
commits: 3

coverage:
  - deliverable: "cs2-cli retry повторяет задачу через run_job; без record.json — RETRY_NO_DATA и код выхода 1"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/cli/tests/record.rs#retry_without_record_json_exits_with_retry_no_data"
        status: pass
  - deliverable: "retry без папки и record без --player печатают использование, код выхода 2"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/cli/tests/record.rs#retry_without_a_folder_prints_usage"
        status: pass
      - kind: test
        ref: "crates/cli/tests/record.rs#record_without_player_prints_usage"
        status: pass
  - deliverable: "разбор аргументов record/retry в JobSpec (ссылка FACEIT, .dem, --size/--fps/--margin, HLAE_DIR/CS2_DIR)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/cli/src/record.rs#tests"
        status: pass
  - deliverable: "cs2-cli без подкоманды ведёт себя как прежде"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/cli/tests/pipeline.rs"
        status: pass
  - deliverable: "реальный прогон record/retry с HLAE и CS2 на ПК автора"
    human_judgment: true
    rationale: "в проверках нет игры и HLAE; сквозной прогон входит в приёмку фазы (5 матчей без присмотра)"

key-files:
  created:
    - crates/cli/src/record.rs
    - crates/cli/tests/record.rs
  modified:
    - crates/cli/src/main.rs

key-decisions:
  - "Папки HLAE и CS2 в CLI: --hlae/--cs2, иначе HLAE_DIR/CS2_DIR; поиска по дискам нет (решение плана, DC-1)"
  - "Флаги плана (--player, --out, --size, --fps) у retry отвергаются как неизвестные: план берётся из record.json и они ничего бы не изменили"
  - "Пути результатов, папки задачи и файла демки делаются абсолютными в record_cmd/retry_cmd, а не в parse_*: разбор остаётся чистым и проверяется без файловой системы"
  - "Номер попытки при retry — наибольший n в clips/manifest.json плюс один; без манифеста — 1"

requirements-completed: [TST-03]

duration: 12 min
completed: 2026-10-11
---

# Phase 4 Plan 06: cs2-cli record и retry поверх run_job Summary

**`cs2-cli` получил подкоманды `record` и `retry`, которые только собирают `JobSpec` из аргументов и переменных окружения и вызывают `pipeline::job::run_job` — ту же функцию, что и очередь приложения; сбои печатаются кодом, сообщением и подсказкой по-русски.**

## Что сделано

- **Task 1 (tracer), `7917500`:** `crates/cli/src/record.rs` — `ConsoleProgress` перенесена из `main.rs`, `print_failure` (код, сообщение, подсказка; отмена — «отменено»), общий `parse_cfg`, `parse_retry`, `retry_cmd`, `USAGE_RECORD`. В `main` ветка `retry` до `parse_args`, `assemble_cmd` печатает сбой через `print_failure`. Тесты бинарника: `retry` без `record.json` (код 1, `RETRY_NO_DATA`, «запустите задачу заново») и без папки (код 2).
- **Task 2, RED `42e3e00` и GREEN `63b7848`:** `parse_record` (`faceit::parse_match_id` для ссылки, иначе путь; `--player` обязателен), `record_cmd` со строкой «запись: … · игрок … · кадр ШxВ · результаты в …» (без ключа FACEIT), ветка `record` в `main`, отсылка в общем `USAGE`, `--help`/`-h` подкоманд печатает `USAGE_RECORD` с кодом 0.
- Поведение `cs2-cli` без подкоманды не менялось.

## Проверка

Реальный вывод:

- `cargo test --release -p cli --bin cs2-cli` — 6 passed
- `cargo test --release -p cli --test record` — 3 passed (`retry_without_record_json_exits_with_retry_no_data`, `record_without_player_prints_usage` ok)
- `cargo test --release -p cli --test pipeline` — 2 passed
- `cargo build --release --workspace` — Finished, без `error[`
- Дополнительно: `cs2-cli record match.dem` — код 2 и использование; `cs2-cli record --help` — код 0.

## Deviations from Plan

None - plan executed exactly as written.

(Не отклонение, а уточнение: у `retry` разрешены только `--hlae`, `--cs2`, `--margin`, `--no-gpu`; остальные флаги плана дают «неизвестный параметр … (у retry план берётся из record.json)».)

## Known Stubs

None.

## Threat Flags

None. Новых сетевых точек, путей авторизации или схем нет; ссылка FACEIT проходит только через строгий `faceit::parse_match_id`, ключ FACEIT не печатается (проверено тестом на `Debug`).

## Self-Check: PASSED

- `crates/cli/src/record.rs`, `crates/cli/tests/record.rs` на месте; `7917500`, `42e3e00`, `63b7848` в истории `main`.
- Критерии приёмки обеих задач выполнены (`run_job(`, `pub struct ConsoleProgress`, `pub fn parse_retry(`, `pub fn print_failure(`, `pub fn parse_record(`, `pub fn record_cmd(`, `DemoSource::Faceit`, `parse_match_id`, `HLAE_DIR`, `CS2_DIR`, `mod record;`, `"retry"`, `"record"`, `record::record_cmd()`).
- Сквозной прогон с реальными HLAE и CS2 здесь не делался (нет игры в проверках) — входит в приёмку фазы.

## Next

Ready for 04-07.
