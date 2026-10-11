---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 04
subsystem: pipeline
tags: [rust, record, ffprobe, manifest, processes-json, record-log, take-invalid]

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "probe_take, Manifest, EnvSnapshot, TAKE_INVALID и тексты (03-03); фейк ffprobe, ProcIdentity, Session::processes (03-02)"
provides:
  - "record::Verify { ffprobe, plan_hash, attempt, env } и RecordRequest.verify: Option<Verify>"
  - "machine::Action::Verify { take }, machine::Input::TakeInvalid { message }"
  - "Драйвер record(): очередь проверок дублей, ведение clips/manifest.json (pending -> recorded -> validated/invalid), проверка оставшихся дублей после закрытия игры"
  - "record::processes: PROCESSES_FILE, save, load (строгое чтение), remove; settle_processes по живости игры"
  - "RecordLog::open переносит прежний record.log в record.log.N"
affects: [03-05, 03-06, 03-07, 03-08]

actuals:
  tokens: 15137
  tasks: 3
  commits: 3

plan_head_before: b23108c73e0b02613094a23ae029de9f59d97f65
plan_head_after: e63802a0b74b47a6d18d0047b20ec0ea4ebc5462
commits: 3

tech-stack:
  added: []
  patterns:
    - "очередь проверок без сна: не больше одной за шаг цикла, каждая через Host::run с дедлайном и отменой; повтор раз в секунду до take_finalize"
    - "обработка очереди действий машины в цикле `loop { while pop {..}; один проба-шаг }`: действия от пробы идут тем же кодом, что и действия шага"
    - "processes.json как недоверенный файл: версия, <=8 записей, ненулевой PID, образ из ORPHAN_IMAGES без учёта регистра"
    - "ротация журнала по папке: наибольший номер + 1, ничего не удаляется"

key-files:
  created:
    - crates/pipeline/src/record/processes.rs
  modified:
    - crates/pipeline/src/record/machine.rs
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/record/log.rs
    - crates/pipeline/tests/record.rs
    - crates/app/src/jobs.rs

key-decisions:
  - "Менеджер проверок (Verifier) живёт в record/mod.rs: общий код для проверки в цикле (probe_due) и после закрытия игры (verify_rest, last=true без повторов)"
  - "Размеры файлов дубля читаются std::fs::metadata (не modified()): они нужны только для mark_recorded и признака «файл ещё растёт»"
  - "Для TakeInvalid при Closing состояние сбоя — Take, останов игры остаётся Graceful (как у прочих сбоев)"
  - "Манифест создаётся до запуска лаунчера; при сбое запуска итог попытки тоже пишется"
  - "Негодный ответ Invalid повторяется, пока (размеры изменились или прошло < 2 с) и не вышел take_finalize; отсутствующий ffprobe даёт TAKE_INVALID через ~2 с виртуального времени"

patterns-established:
  - "Verdict {Settled, Cancelled, Rejected(msg)}: единое решение по пробе, из которого драйвер строит Input::Cancel или Input::TakeInvalid"

requirements-completed: [TAK-02, TAK-03, TAK-04, TAK-05, RET-04, RET-06, TST-02]

coverage:
  - id: D1
    description: "Дубль проверяется ffprobe сразу после принятого TAKE_END своей задачи; обрезанный, беззвучный и другого размера дубль роняет запись за секунды кодом TAKE_INVALID с номером и папкой дубля в сообщении и подсказке"
    requirement: TAK-04
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#truncated_take_fails_with_take_invalid_within_seconds_naming_the_take"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#take_without_audio_is_invalid"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#wrong_resolution_is_invalid_and_names_both_sizes"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#ffprobe_never_reads_a_take_before_its_take_end"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/machine.rs#take_end_asks_the_driver_to_verify_that_take"
        status: pass
    human_judgment: false
  - id: D2
    description: "clips/manifest.json ведётся по ходу записи: pending, recorded, validated с измеренным или invalid с причиной; у попытки номер, снимок окружения и итог"
    requirement: TAK-03
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#verified_recording_validates_every_take_in_the_manifest"
        status: pass
    human_judgment: false
  - id: D3
    description: "Недописанный файл ждёт ffmpeg HLAE (повтор раз в секунду до take_finalize); последний дубль проверяется после закрытия игры; отмена во время ffprobe и недоступный ffprobe обработаны"
    requirement: TAK-02
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#finalizing_take_is_retried_until_ffmpeg_finishes_it"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#take_that_never_finalizes_fails_after_take_finalize"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#last_take_is_verified_after_the_game_closes"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#cancel_during_ffprobe_cancels_the_recording"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#missing_ffprobe_fails_the_take_with_the_host_error"
        status: pass
    human_judgment: false
  - id: D4
    description: "processes.json со своими HLAE.exe и cs2.exe лежит, пока игра может быть жива, и удаляется после подтверждённого выхода; чтение отвергает чужой образ, нулевой PID, больше 8 записей и мусор"
    requirement: RET-04
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#own_processes_file_lives_only_while_the_game_may_be_alive"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/processes.rs#foreign_image_garbage_and_oversized_lists_are_rejected"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/processes.rs#saved_identities_round_trip_and_remove_deletes_the_file"
        status: pass
    human_judgment: false
  - id: D5
    description: "Новая запись в ту же папку не затирает журнал: прежний record.log становится record.log.N с N на единицу больше наибольшего; пустой не переносится"
    requirement: RET-06
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#record_log_rotation_never_overwrites_old_logs"
        status: pass
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#record_log_is_written_on_success_and_on_failure_with_source_tags"
        status: pass
    human_judgment: false
  - id: D6
    description: "Запрос без проверки (verify: None) ведёт себя как раньше: прежние 35 сценариев записи зелёные; приложение передаёт verify: None"
    requirement: TST-02
    verification:
      - kind: integration
        ref: "crates/pipeline/tests/record.rs#fake_recording_walks_the_whole_ladder_and_returns_every_take"
        status: pass
      - kind: unit
        ref: "crates/app/src/jobs.rs#afxerror_ignore_from_settings_reaches_the_record_request"
        status: pass
    human_judgment: false
---

# Phase 3 Plan 04: Проверка дублей в машине записи, processes.json и ротация журнала Summary

**Каждый дубль проверяется ffprobe сразу после своего `TAKE_END`: обрезанный, беззвучный или другого размера дубль останавливает запись за секунды кодом `TAKE_INVALID`, манифест `clips/manifest.json` ведётся по ходу записи, а папка задачи знает свои процессы и прошлые журналы.**

## Что сделано

- **Task 1 (tracer, `9197cd7`).** Машина: `Action::Verify { take }` после `TakeDone` и строки времени (формат `TakeDone` не менялся), `Input::TakeInvalid` даёт `TAKE_INVALID` с состоянием `Take` даже в `Closing`. Драйвер: `Verify` и `RecordRequest.verify`, `Verifier` с очередью `Pending`, манифест (`new`, `begin_attempt`, `mark_recorded/validated/invalid`, `end_attempt`), одна проверка за шаг через `probe_take` с дедлайном `ffprobe` и отменой, подсказка `hint_take_invalid` вместо общей. В приложении `verify: None`.
- **Task 2 (`928fe6f`).** `NotReady` и `Invalid` повторяются раз в секунду до `take_finalize` (негодный ответ при неизменных размерах окончателен через 2 с); после `Finish(Ok)` и `shutdown` оставшиеся дубли проверяются по разу (`verify_rest`, без повторов) и превращают успех в `TAKE_INVALID`/`CANCELLED`. Новый `record/processes.rs`: атомарная запись, строгое чтение недоверенного файла, `remove`; драйвер пишет файл после запуска и на `GameAppeared`, удаляет при `Exited`/`NotStarted` и оставляет строку в журнале, если игра может быть жива.
- **Task 3 (`e63802a`).** `RecordLog::open` через `rotate`: непустой `record.log` -> `record.log.{max+1}`, новый файл начинается строкой «прошлая попытка: record.log.N»; неудачный перенос -> дозапись в старый файл с отметкой. Тест `record_log_is_written_on_success_and_on_failure_with_source_tags` обновлён под ротацию.

## Контракт для 03-05..03-07

- `RecordRequest.verify = Some(Verify { ffprobe, plan_hash, attempt, env })` включает проверку и ведение `clips/manifest.json`; манифест при каждой попытке создаётся заново (`Manifest::new`), поэтому план 03-06 (повтор) обязан сам сохранить/перенести статусы `validated` прежнего манифеста до вызова `record`, если нужно записывать только недостающие дубли — это в самом `record` не предусмотрено планом 03-04.
- Негодный дубль остаётся в `clips/takeNNNN` как улика (`mark_partial` его не трогает: его `TAKE_END` принят).
- `processes::load(out_dir)` -> `Ok(Some(list))` готов для 03-07: каждый элемент передавать `Host::kill_orphan`.
- Файлы `record.log.N` создаёт сам `RecordLog::open`; `jobs.attempt` с ними не связан.

## Deviations from Plan

### Auto-fixed Issues

Нет — Rules 1-3 не срабатывали.

### Прочее

- **Тест `finalizing_take_is_retried_until_ffmpeg_finishes_it`:** кроме `finalize_lag(5 с)` в нём задано `job_empty_after_exit(6 с)`: фейк по умолчанию опустошает job сразу после выхода игры, а в реальности job пустеет, когда ffmpeg HLAE закончил. Без этого последний дубль проверялся бы после закрытия (по проверенному поведению Task 2) и честно падал.
- **Тест `last_take_is_verified_after_the_game_closes`:** вместо `finalize_lag(10 с)` из плана взято 6 с — при 10 с не успевал бы и дубль 2 (дубли идут каждые 7 с), и сбой назвал бы «дубль 2 из 3», а не «дубль 3 из 3».
- **Тест `take_that_never_finalizes...`:** 6 дублей вместо 3, чтобы игра ещё писала, когда истёк `take_finalize`; решение приходит в пределах `take_finalize + 10 с` после `TAKE_END` 0.
- Файл `crates/pipeline/src/lib.rs` `cargo fmt` переформатирует (длинная строка `pub use record::…`); возвращён `git checkout` — к плану не относится, правок в нём нет.
- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как в 03-01..03-03).
- Лишний вызов `state.advance-plan` до написания SUMMARY (порядок шагов): на результат не влияет.

## Проверка

- `cargo test --release -p pipeline --test record` — 47 passed (прежние 35 зелёные, 12 новых: 5 по Task 1, 6 по Task 2, 1 по Task 3, плюс переписанный тест журнала).
- `cargo test --release -p pipeline --lib record::machine` — 8 passed; `record::processes` — 2 passed; `-p pipeline --lib` — 76 passed.
- `cargo test --release -p app --bin cs2cinema` — 26 passed.
- `cargo test --release --workspace` — зелёный, включая `crates/cli/tests/pipeline.rs` (2 passed); `cargo build --release --workspace` — без ошибок.
- `grep -rn "modified()" crates/pipeline/src crates/app/src` — пусто.
- `cargo clippy -p pipeline --all-targets` — новых предупреждений в изменённых файлах нет.
- Настоящее время дописывания файла ffmpeg HLAE и разрешение на живом HLAE — план 03-08.

## Known Stubs

Нет.

## Threat Flags

Нет нового внешнего поверхностного риска сверх T-03-11..T-03-13 плана: проверки без сна и с дедлайном/отменой (T-03-11), строгое чтение `processes.json` (T-03-12), ротация журнала без удаления (T-03-13).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/record/processes.rs (`pub const PROCESSES_FILE`), crates/pipeline/src/record/mod.rs (`pub struct Verify`, `probe_take(`, `mark_validated(`, `hint_take_invalid(`, `processes::save(`, `processes::remove(`), crates/pipeline/src/record/machine.rs (`Verify { take: usize }`, `TakeInvalid { message: String }`), crates/pipeline/src/record/log.rs (`record.log.`), crates/app/src/jobs.rs (`verify: None`)
- FOUND commits: 9197cd7, 928fe6f, e63802a
