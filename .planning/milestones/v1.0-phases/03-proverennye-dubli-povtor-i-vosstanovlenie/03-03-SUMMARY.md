---
phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
plan: 03
subsystem: pipeline
tags: [rust, ffprobe, manifest, record-json, env-snapshot, fail-codes]

requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: "фейк ffprobe (03-02): ответ по последнему аргументу-пути, clips_dir, truncated_take, take_without_audio, video_size, finalize_lag"
  - phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
    provides: "precheck::Report, Deadlines, log::one_line"
provides:
  - "pipeline::record::verify: probe_args, parse_probe, within_tolerance, tolerance, judge, probe_take, ProbeOutcome, TakeFacts, PROBE_STDOUT_MAX"
  - "pipeline::manifest: Manifest (clips/manifest.json v1), TakeEntry, TakeStatus, AttemptEntry, EnvSnapshot, RecordJson (record.json v1), TakeSpec, valid_take_dir, reconcile"
  - "pipeline::write_demo_copy"
  - "FailCode::TakeInvalid / RetryNoData и msg_take_invalid, hint_take_invalid, msg_retry_no_data, msg_take_validated, msg_take_not_final"
  - "Deadlines.take_finalize (20 с), TAKE_TOLERANCE_REL / TAKE_TOLERANCE_MIN_SECS"
  - "precheck::Report: ffmpeg_version, ffprobe_version, hlae_ffmpeg_version, free_results, free_cs2"
affects: [03-04, 03-05, 03-06, 03-07, 03-08]

actuals:
  tokens: 17700
  tasks: 3
  commits: 3

plan_head_before: e700ca19aadc55613bf624778cbfc529194e990d
plan_head_after: 3549d5fc69e90dafc15e0e951389aa38caa92b81
commits: 3

tech-stack:
  added: []
  patterns:
    - "недоверенные JSON-файлы папки задачи: версия, имена take+4 цифры, строгая проверка полей при чтении; испорченный манифест даёт None, а не панику"
    - "атомарная запись: <имя>.tmp + rename, при отказе rename поверх файла старый удаляется и rename повторяется"
    - "вывод ffprobe: потолок 64 КиБ, типизированный serde-разбор (длительность строкой или числом), строки ошибок через log::one_line"

key-files:
  created:
    - crates/pipeline/src/record/verify.rs
    - crates/pipeline/src/manifest.rs
  modified:
    - crates/pipeline/src/record/deadlines.rs
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/precheck/mod.rs
    - crates/pipeline/src/precheck/gather.rs

key-decisions:
  - "Звук проверяется в audio.wav, видео и разрешение — в video.mp4; обе длительности сравниваются с TakePlan.game_secs (допуск ±10%, но не меньше ±0.5 с)"
  - "Длительность файла — format.duration, иначе длительность потока"
  - "ProbeOutcome::NotReady — файла нет, 'moov atom not found', 'Invalid data found', 'No such file'; любой другой код выхода ffprobe, таймаут и ошибка запуска — Invalid; отмена — Cancelled"
  - "record::take_number стал pub(crate): valid_take_dir опирается на него, чтобы правило 'take + 4 цифры' было одно"
  - "RecordJson дополнительно проверяет fps 1..=1000, путь к демке не пуст, длительности дублей конечны и неотрицательны, входы склейки ссылаются только на существующие дубли"
  - "RecordJson.shots и secs: число шотов и суммарная игровая длительность дублей (в плане поля названы без пояснений)"

patterns-established:
  - "Один источник допуска длительности — константы рядом с Deadlines (D-01)"

requirements-completed: [TAK-03, TAK-04, OBS-03]

coverage:
  - id: D1
    description: "Дубль годен, только если у video.mp4 видеопоток заданного разрешения, у audio.wav аудиопоток, длительности > 0 и в допуске; каждая причина отказа — фраза с файлом и числами"
    requirement: TAK-04
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#each_defect_has_its_own_russian_reason_with_numbers"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#tolerance_is_ten_percent_but_not_below_half_a_second"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#fake_ffprobe_responder_flags_truncated_and_silent_takes"
        status: pass
    human_judgment: false
  - id: D2
    description: "ffprobe идёт через Host::run с дедлайном и отменой; 'не дописан' отличается от 'негоден' и от отмены; вывод ограничен 64 КиБ"
    requirement: TAK-04
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#unfinished_file_is_not_ready_but_other_failures_are_invalid"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#host_errors_map_to_invalid_and_cancel_stays_cancel"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/verify.rs#oversized_stdout_is_cut_and_reported_without_panic"
        status: pass
    human_judgment: false
  - id: D3
    description: "clips/manifest.json хранит статусы, размеры, измеренное и попытки со снимком окружения; запись атомарна, чтение отвергает мусор, чужую версию и папки не вида take+4 цифры; reconcile понижает изменившийся дубль"
    requirement: TAK-03
    verification:
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#validated_take_survives_a_save_and_load_round_trip"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#garbage_and_foreign_dirs_are_rejected_on_load"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#reconcile_downgrades_a_changed_take"
        status: pass
    human_judgment: false
  - id: D4
    description: "record.json хранит всё для повтора без скачивания и плана; при чтении подменённые поля отвергаются с причиной по-русски"
    requirement: TAK-03
    verification:
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#record_json_round_trips_and_rebuilds_take_plans"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#record_json_rejects_tampered_fields"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/lib.rs#demo_copy_is_written_unpacked"
        status: pass
    human_judgment: false
  - id: D5
    description: "Снимок окружения: сборка CS2, версии HLAE/ffmpeg/ffprobe/ffmpeg HLAE, NVENC, свободное место, кадр; отчёт фазы 2 без новых полей читается"
    requirement: OBS-03
    verification:
      - kind: unit
        ref: "crates/pipeline/src/manifest.rs#env_snapshot_takes_versions_and_space_from_the_report"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/precheck/mod.rs#report_without_env_facts_still_parses"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/precheck/mod.rs#evaluate_carries_tool_versions_and_free_space_into_the_report"
        status: pass
    human_judgment: false
  - id: D6
    description: "Коды TAKE_INVALID (с номером и папкой дубля в сообщении и подсказке) и RETRY_NO_DATA; дедлайн take_finalize 20 с масштабируется 'Запасом по времени'"
    requirement: TAK-04
    verification:
      - kind: unit
        ref: "crates/pipeline/src/record/failure.rs#take_invalid_names_the_take_in_message_and_hint"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/failure.rs#retry_no_data_has_its_own_code_and_hint"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/record/deadlines.rs#take_finalize_is_twenty_seconds_and_follows_the_time_margin"
        status: pass
    human_judgment: false
---

# Phase 3 Plan 03: Проверка дубля, манифест и вход повтора Summary

**`pipeline` умеет решать «дубль годен» по ffprobe с понятной русской причиной отказа и хранит манифест дублей, вход повтора `record.json` и снимок окружения в папке задачи.**

## Что сделано

- **Task 1 (tracer, `b506700`).** `record/verify.rs`: `probe_args` (путь — последний аргумент), типизированный `parse_probe` (длительность строкой или числом, `N/A`, NaN и минус — «нет»), `within_tolerance`/`tolerance` по константам рядом с `Deadlines`, `judge` с порядком проверок: видеопоток, аудиопоток, длительности > 0, допуск, разрешение. `manifest.rs`: `Manifest` v1 (`new/load/save/entry/mark_*/begin_attempt/end_attempt/missing/all_validated`), атомарная запись `tmp + rename`, чтение отвергает мусор, версию не 1 и папки не вида `take` + 4 цифры. Трассер пройден до конца (JSON ffprobe -> решение -> файл на диске -> чтение), тесты зелёные; расширение на Task 2 и 3 не остановлено.
- **Task 2 (`586245b`).** `probe_take` через `Host::run` с дедлайном и отменой: нет файла -> `NotReady` без запуска ffprobe; `moov atom not found` / `Invalid data found` / `No such file` -> `NotReady`; другой код выхода, таймаут, ошибка запуска, слишком длинный вывод -> `Invalid`; отмена -> `Cancelled`. `FailCode::TakeInvalid` и `RetryNoData`, русские сообщения и подсказки. Подсказки `PrecheckFailed`, `ForeignGameRunning`, `PrevGameStuck` дополнены «(или «Заново», если задача не дошла до записи)».
- **Task 3 (`3549d5f`).** `RecordJson` (save/load/remove/take_plans, строгие проверки недоверенных полей), `TakeSpec`, `EnvSnapshot` (`from_report/save/one_line`), `Manifest::reconcile`, `Report` получил версии утилит и свободное место (`#[serde(default)]`, `check_space` обновляет место), `pipeline::write_demo_copy`. Подсказки состояния CS2 в Steam дополнены тем же «Заново».

## Контракт для 03-04..03-07

- `probe_take(host, ffprobe, clips, take, frame, deadline, cancel)`: `clips` — папка `clips/` задачи; `NotReady` нужно повторять до `Deadlines.take_finalize`, текст для сбоя — `failure::msg_take_not_final`; `Invalid(reason)` -> `msg_take_invalid(n, m, dir, reason)` + `hint_take_invalid(n, dir)`.
- Тесты, которым нужен ответ ffprobe фейка по файлам, обязаны дать `clips_dir` (контракт 03-02) и реально создать файлы `takeNNNN/video.mp4|audio.wav`.
- `RecordJson::load` возвращает `Err(String)` с причиной; драйвер/приложение превращает её в `msg_retry_no_data(reason)`.
- `Manifest::load` -> `None` = «дубли непроверены»; `reconcile` вызывать после загрузки перед решением «что перезаписывать».

## Deviations from Plan

### Auto-fixed Issues

Нет — план выполнен как написан.

### Прочее

- `Deadlines.take_finalize` (поле, `unit(20)`, `scaled`, тест) добавлено в коммит Task 1: файл `deadlines.rs` правился там ради констант допуска, а коммит Task 2 не требует его отдельно. Поведение не меняется.
- Дополнительные проверки в `RecordJson::load` сверх списка плана: fps 1..=1000, непустой путь к демке, `:` в имени демки, конечные неотрицательные длительности дублей, входы склейки только на существующие дубли (T-03-07).
- Поля `shots` и `secs` в `record.json` в плане названы без пояснений; приняты как число шотов и суммарная игровая длительность дублей. Потребителю (03-06) при необходимости достаточно переименовать — файл ещё нигде не пишется.
- Pre-commit проверка «HEAD не на защищённой ветке» формально срабатывает на `main`; пропущена осознанно (`branching_strategy: none`, прямое указание оркестратора, как в 03-01 и 03-02).
- Подсказка `PrevGameStuck` и др.: «Повторить доступна только при `record.json`» — сами кнопки и условия делает 03-06; здесь только тексты.

## Проверка

- `cargo test --release -p pipeline --lib` — 72 passed (`verify` 13, `manifest` 9, `precheck::` 26, `record::` 35 вместе с проверкой/failure/deadlines).
- `cargo test --release -p pipeline --test record` — 35 passed; `--test precheck` — 19 passed.
- `cargo build --release --workspace` — без ошибок; `cargo test --release --workspace` — зелёный.
- `cargo clippy -p pipeline --all-targets` — новых предупреждений в изменённых файлах нет (одно старое в `gather.rs:58`, не из этого плана).
- Реальные ответы ffprobe по дублю HLAE (длительность, разрешение, время дописывания, допуск) — план 03-08; допущения A1 (`take_finalize` 20 с) и A3 (ожидаемая длительность = `game_secs`) там и подтверждаются.

## Known Stubs

Нет.

## Threat Flags

Нет нового внешнего поверхностного риска сверх T-03-07..T-03-10 плана (mitigate выполнены: папки `take`+4 цифры, имя демки и входы склейки проверены, версия проверяется, дедлайн/отмена/потолок 64 КиБ/типизированный разбор у `probe_take`, ошибки ffprobe через `one_line`, запись манифеста через tmp + rename).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/record/verify.rs (`pub fn probe_take(`, `moov atom not found`), crates/pipeline/src/manifest.rs (`pub struct RecordJson`, `pub fn reconcile(`, `manifest.json.tmp` через `.tmp`), crates/pipeline/src/record/failure.rs (`"TAKE_INVALID"`), crates/pipeline/src/lib.rs (`pub fn write_demo_copy(`)
- FOUND commits: b506700, 586245b, 3549d5f
