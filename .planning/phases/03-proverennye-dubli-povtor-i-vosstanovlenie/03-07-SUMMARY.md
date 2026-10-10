---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 07
subsystem: app
tags: [rust, egui, recovery, orphan-process, processes-json, manifest, jobs-page, retry]
status: complete

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "Store::reset_running/interrupted_jobs/requeue_resume, journal::stage_summary, Job.attempt (03-01); ProcIdentity, OrphanOutcome, Host::kill_orphan, FakeHost::orphan_kills (03-02); Manifest, TakeStatus, RECORD_FILE (03-03); processes::load/remove (03-04); jobs::resume_job (03-06)"
provides:
  - "launch::kill_orphans(host, out_dir): завершает cs2.exe/HLAE.exe прошлого экземпляра по processes.json только через Host::kill_orphan (полная идентичность)"
  - "launch::recover: «готовые» дубли берутся из clips/manifest.json (recorded и validated), запасной вариант installed.json.done_takes"
  - "launch::recover_after_crash(store, host, cs2_dir): kill_orphans -> recover -> reset_running, строки «уборка после перезапуска: …» в журнале задачи"
  - "launch::old_logs(out_dir): record.log.N по возрастанию номера"
  - "JobsState.resumable / old_logs / fs_seen; refresh_jobs смотрит папку задачи только при изменении updated_at"
  - "jobs_page: Act::Retry (requeue_resume), Act::Redo (requeue), CardButtons/card_buttons, stage_summary_text, «Попытка N · этап · код · состояние», ссылки record.log.N"
affects: [03-08, карточка задачи, запуск приложения]

actuals:
  tokens: 6300
  tasks: 2
  commits: 2

plan_head_before: d87e2b9a3cdf1cef94b3441a3658e30b4b9d888a
plan_head_after: 298e259ab6c8eae26b54005e9a95ff76c605e687
commits: 2

tech-stack:
  added: []
  patterns:
    - "восстановление при запуске в строгом порядке: завершить процесс, убрать папку игры, пометить задачу"
    - "состояние файловой системы для карточки кэшируется по updated_at задачи, в кадре диск не трогается"

key-files:
  created: []
  modified:
    - crates/app/src/launch.rs
    - crates/app/src/app.rs
    - crates/app/src/jobs_page.rs

key-decisions:
  - "«Повторить» ставит resume через requeue_resume, «Заново» — requeue; обе снимают паузу очереди (общая функция requeue в jobs_page)"
  - "card_buttons.retry = Some(resumable && !busy); «Заново» отключается по app.status.busy прямо в UI, а не в card_buttons"
  - "Файл processes.json остаётся, если хоть одно завершение провалилось (Failed) или файл не разобран; в остальных случаях удаляется"

requirements-completed: [RET-04, OBS-02, RET-01, RET-06, OBS-01]

duration: 35min
completed: 2026-10-11
---

# Phase 3 Plan 07: Восстановление после падения и повтор из карточки Summary

**При запуске приложения осиротевшая игра прошлого экземпляра завершается по сохранённой идентичности, папка игры убирается, а задача честно становится «прервана»; карточка даёт «Повторить» (с записи), «Заново», «Открыть папку», показывает попытку, этап сбоя, сводку по этапам и журналы прошлых попыток.**

## Что сделано

- **Task 1 (tracer, `5fcf11a`).** `kill_orphans` читает `processes.json` через `processes::load` и зовёт `Host::kill_orphan` для каждой идентичности; исходы `Killed/Gone/Mismatch/Failed/Unsupported` превращаются в русские строки журнала, испорченный файл ничего не завершает («processes.json не разобран — процессы не трогаю»). `recover` теперь считает готовыми дубли, которые манифест знает как `recorded` или `validated` (Pitfall 2), и только без манифеста падает на `installed.json.done_takes`. `recover_after_crash` выполняет по каждой «выполняется»/«отменяется» задаче завершение процесса, затем уборку папки игры, и в конце `reset_running`; `App::new` вызывает её с `default_host()` вместо прежнего цикла. В карточке `Act::Retry` теперь `requeue_resume`, новый `Act::Redo` — `requeue`; `card_buttons(status, resumable, has_dir, busy)` решает, какие кнопки показывать; «Папка» переименована в «Открыть папку». `refresh_jobs` заполняет `resumable` (есть ли `record.json`) и `old_logs` и пересчитывает их только при смене `updated_at` задачи.
- **Task 2 (`298e259`).** `attempt_line` («Попытка N · этап X · КОД · состояние», пустые части пропускаются), `stage_summary_text` («по этапам: Скачивание 12 с · Разбор 4 с · …» из `store::journal::stage_summary`, только последняя попытка, нет строк этапов — `None`), строка «прошлые попытки:» с маленькой кнопкой на каждый `record.log.N` (открывается через `ui::open_path`) у задач, которые не выполняются. Подсказки при наведении на «Повторить» (активная и неактивная) и «Заново» — по тексту плана.

## Deviations from Plan

### Auto-fixed Issues

Нет — Rules 1-3 не срабатывали.

### Прочее

- `launch::old_logs` и его тест `old_logs_are_listed_in_attempt_order` попали в коммит Task 1 (одним файлом `launch.rs`), хотя по плану относятся к Task 2; поведение и тесты те же. Сводка, `attempt_line` и ссылки на журналы — в коммите Task 2.
- Подсказки кнопок («Повторить», «Заново») добавлены уже в Task 1, т.к. кнопки строились там же; Task 2 их не менял.
- `old_logs` и `resumable` считаются и для готовых задач (`Done`), а не только для упавших: у готовой задачи тоже могут лежать журналы прошлых попыток.
- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как в 03-01..03-06).
- Python на машине нет: файлы правились через Edit и склейку `head`/`tail`; на содержимое это не влияет.

## Проверка

- `cargo test --release -p app --bin cs2cinema` — 43 passed (в том числе `startup_kills_the_orphan_before_cleanup_and_marks_the_job_interrupted`, `orphan_with_another_creation_time_is_left_alone`, `malformed_processes_json_kills_nothing`, `recover_keeps_takes_the_manifest_knows`, `recover_marks_unfinished_takes_partial`, `recover_without_a_record_keeps_finished_takes`, `card_offers_retry_only_with_record_json_and_redo_for_finished_jobs`, `old_logs_are_listed_in_attempt_order`, `stage_summary_text_lists_the_last_attempt`).
- `cargo test --release --workspace` — зелёный, включая `crates/cli/tests/pipeline.rs` (2 passed); `cargo build --release --workspace` — без ошибок.
- Проверка порядка в `recover_after_crash` (awk из критериев приёмки): `kill_orphans(` < `recover(` < `reset_running(` — код выхода 0.
- `cargo clippy -p app --all-targets` — предупреждения только прежние (к плану не относятся).
- Внешний вид карточки и живое восстановление после убийства приложения посреди записи — план 03-08.

## Known Stubs

Нет.

## Threat Flags

Нет нового внешнего поверхностного риска сверх T-03-18..T-03-20: завершение только через `Host::kill_orphan` по файлу, прочитанному строго (`processes::load`), с проверкой времени создания и образа на одном handle; список `record.log.N` строится только из имён `record.log.<цифры>` внутри папки задачи; открытие через `ui::open_path`.

## Self-Check: PASSED

- FOUND: crates/app/src/launch.rs (`pub fn kill_orphans(`, `pub fn recover_after_crash(`, `pub fn old_logs(`, «не трогаю»), crates/app/src/app.rs (`recover_after_crash(`, `resumable`, `old_logs`), crates/app/src/jobs_page.rs (`pub fn card_buttons(`, `pub fn stage_summary_text(`, `requeue_resume(`, `Act::Redo`, «Попытка», «по этапам», «прошлые попытки», «Нет данных записи (record.json)»)
- FOUND commits: 5fcf11a, 298e259
