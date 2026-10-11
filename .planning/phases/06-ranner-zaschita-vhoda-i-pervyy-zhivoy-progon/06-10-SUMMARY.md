---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 10
subsystem: runner
tags: [rust, runner, executor, cli, preflight, verify-result, lock, tracer]
status: complete

requires:
  - phase: 06-02
    provides: pipeline::assemble::verify_result, ResultFacts, msg_result_invalid
  - phase: 06-05
    provides: шов Executor/Task, Runner, Config
  - phase: 06-07
    provides: pipeline::inspect::preflight (дочерний cs2-cli inspect)
  - phase: 06-08
    provides: Control::stop, Tee, Uplink, серверная отмена
  - phase: 06-09
    provides: Disposition/dispose, release с засчитанной выдачей, пауза, Control::resume
provides:
  - runner::PipelineExecutor и InspectSettings — защищённый исполнитель (предпроверка, run_job под catch_unwind, resume с откатом, verify_result, recover)
  - Config.max_demo_bytes и MAX_DEMO_BYTES — DEMO_TOO_LARGE до скачивания; ссылка FACEIT без файла не скачивается
  - повторная сверка SHA-256 результата перед первой загрузкой (RESULT_INVALID при расхождении)
  - Runner::run возвращает Result<(), String>: Err при отказе токена и несовпадении протокола
  - cs2-cli runner — токен только из CS2_RUNNER_TOKEN или --token-file, замок runner.lock, Ctrl+C, Enter
affects: [06-11 (живой прогон и учения: --upload-chunk-limit, калибровка по peak_mem_bytes), фаза 8 (интеграция очереди приложения и раннера)]

estimate:
  tokens: 120000
  raw_tokens: 120000
  tasks: 3
  confidence: low
actuals:
  tokens: 19000
  tasks: 3
  commits: 3
plan_head_before: 54612ddab981995c445bcc64bec3e3bf4338dd70
plan_head_after: 6beb5fe21d13665bda6484bea23ba2d2d6a94d1e

tech-stack:
  added: []
  patterns:
    - "Недоверенная демка проходит дочерний inspect до run_job; run_job вызывается под catch_unwind, паника становится обычной ошибкой (JOB_ERROR у раннера)"
    - "Выбор источника — две чистые функции needs_download и task_of: ссылка FACEIT без файла не качается, Task.demo пустой, отказ даёт исполнитель"
    - "Одна копия раннера держит runner.lock через File::try_lock: ОС снимает замок и при падении"

key-files:
  created:
    - crates/cli/src/runner_cmd.rs
    - crates/cli/tests/runner.rs
    - crates/runner/tests/exec.rs
  modified:
    - crates/runner/src/exec.rs
    - crates/runner/src/lib.rs
    - crates/runner/tests/common/mod.rs
    - crates/runner/tests/outbox.rs
    - crates/cli/Cargo.toml
    - crates/cli/src/main.rs
    - Cargo.lock

key-decisions:
  - "Сверка SHA-256 результата перед загрузкой идёт только при sha_retries == 0 (первая загрузка): после отказа сервера по SHA-256 решает сам сервер, иначе тест «второй отказ сервера убирает результат» нельзя воспроизвести через состояние"
  - "Runner::run теперь возвращает Result<(), String>: Err при отклонённом токене (в том числе пойманном в lose()) и при несовпадении версии протокола (426), остальные ошибки связи по-прежнему повторяются"
  - "Токен проверяется на печатные ASCII-знаки: не-ASCII знак в заголовке Authorization давал вечные повторы запроса (найдено тестом)"
  - "Пик памяти разбора пишется в журнал строкой «демка проверена: … пик памяти разбора N МиБ» для калибровки лимитов в 06-11"
  - "Потолок скачивания демки (--max-demo-mib) задаёт и потолок размера файла дочернего разбора: что скачали, то и разберут"

patterns-established:
  - "Новый отказ входа = типизированный Failure через typed_failure(FailCode, RecordState, msg) + fail_job: итог идёт этапом Reporting, серия неисправностей обрывается, файл остаётся уликой"

requirements-completed: [RUN-01, RUN-05, RUN-06, SEC-02]

duration: около 75 мин
completed: 2026-10-11
---

# Phase 6 Plan 10: настоящий раннер в руках автора Summary

**`cs2-cli runner` забирает задачи исходящими запросами, мусорную или враждебную демку закрывает своим кодом через дочерний `inspect` ещё до запуска CS2, негодный по ffprobe или изменённый после проверки ролик не загружает (`RESULT_INVALID`), а паника `run_job` не роняет раннер.**

## Выполнено

- **Задача 1 (tracer, `0c90336`).** `PipelineExecutor` в `exec.rs`: `UNSUPPORTED_SOURCE` для ссылки FACEIT без файла, дочерняя предпроверка `preflight` (не при продолжении), `run_job` через подменяемый `job_fn` под `catch_unwind` (паника → `RunError` «исполнитель задачи упал: …» без `fail`), при `resume` и `RETRY_NO_DATA` — строка журнала и повтор с нуля вместе с предпроверкой, `recover` = `kill_orphans`, затем `gamedir::recover`. Трассёр `garbage_demo_fails_with_bad_demo_before_cs2_and_the_runner_takes_the_next_job` на настоящем `server::serve` и `FakeHost`: две задачи закрыты `BAD_DEMO` и `PLAYER_NOT_FOUND`, `host.launches() == 0`, два запуска `cs2-cli inspect`, раннер не на паузе. Гейт трассёра (автоматический): `<verify>` перезапущен целиком, зелёный.
- **Задача 2 (`8c39157`).** `verify_result` после `run_job` (годный ролик — строка «ролик проверен: H.264, ШxВ, N с, faststart», негодный — `RESULT_INVALID` на ступени `Done`, файл остаётся); `Config.max_demo_bytes` (2000 МиБ) и отказ `DEMO_TOO_LARGE` до скачивания; `needs_download`/`task_of`; сверка SHA-256 результата перед первой загрузкой. Тесты: `invalid_result_is_not_uploaded_and_fails_with_result_invalid`, `valid_result_is_uploaded_and_the_job_is_done`, `demo_over_the_limit_is_refused_before_download`, `result_changed_after_verification_is_not_uploaded`, `recover_kills_own_orphans_before_cleaning_the_game_dir`, модульный `faceit_link_without_a_demo_file_is_not_downloaded`.
- **Задача 3 (`6beb5fe`).** `cs2-cli runner`: `parse_runner` по образцу `record::parse_cfg` (токен только из `CS2_RUNNER_TOKEN` или `--token-file`, `--token` и `--token=` отвергаются без повтора значения, ручной `Debug` без токена, все лимиты и скрытый `--upload-chunk-limit`), замок `runner.lock`, Ctrl+C (первое нажатие — `stop`, второе — `exit(130)`), поток stdin снимает паузу по Enter, `PipelineExecutor` с `current_exe()` как дочерним разбором. 8 модульных тестов и 4 теста бинаря.

## Проверка

- `cargo test --release -p runner --test exec` — 12 тестов; `-p runner --lib faceit_link_without_a_demo_file_is_not_downloaded` — 1; `-p runner` целиком зелёный.
- `cargo test --release -p cli --test runner` — 4 теста, `-p cli --bin cs2-cli runner_cmd` — 8; `cargo test --release -p cli --test pipeline` проходит.
- `cargo test --release --workspace` зелёный (58 строк `test result: ok`, падений нет); `cargo build --release -p cli -p server -p app` собирается.

## Deviations from Plan

**1. [Rule 1 - Bug] Существующий тест `second_sha256_mismatch_drops_the_result_from_the_outbox` перестал воспроизводиться**
- **Found during:** задача 2 (`cargo test -p runner`)
- **Issue:** тест подсовывал в состояние неверный SHA-256 при целом файле, чтобы сервер дважды отверг его; новая сверка перед загрузкой по требованию плана закрывает такую задачу `RESULT_INVALID` раньше.
- **Fix:** сверка идёт только перед первой загрузкой (`sha_retries == 0`), а тест ставит `sha_retries = 1` — состояние «первый отказ сервера уже был, раннер перезапустили». Сценарий «сервер отверг SHA-256 повторно — результат убирается из outbox» сохранён.
- **Files modified:** crates/runner/src/lib.rs, crates/runner/tests/outbox.rs
- **Commit:** 8c39157

**2. [Rule 2 - Missing critical] `Runner::run` не умел сообщить о 401 и 426**
- **Found during:** задача 3 (план требует «`401`, `426` → «ошибка: …» и код 1»)
- **Issue:** `run()` возвращал `()`: отклонённый токен только останавливал цикл, а несовпадение протокола (426) повторялось бесконечно с паузой.
- **Fix:** `run()` возвращает `Result<(), String>`; константа `PROTOCOL_MISMATCH`; поле `fatal`, которое ставят ветка `run`, отклонённый токен в `lose()` и несовпадение протокола. Вызовов `run()` в тестах до этого не было.
- **Files modified:** crates/runner/src/lib.rs
- **Commit:** 6beb5fe

**3. [Rule 1 - Bug] Не-ASCII токен вешал раннер на вечных повторах**
- **Found during:** задача 3 (тест `run_returns_an_error_when_the_server_rejects_the_token` с кириллическим токеном завис)
- **Issue:** недопустимый символ в заголовке `Authorization` даёт ошибку запроса, которую раннер считает временной и повторяет без конца.
- **Fix:** `parse_runner` принимает только печатные ASCII-знаки без пробелов; тест использует ASCII-токен.
- **Files modified:** crates/cli/src/runner_cmd.rs, crates/runner/tests/exec.rs
- **Commit:** 6beb5fe

### Отличия от текста плана

- `ctrlc` и `runner` добавлены в `crates/cli/Cargo.toml`; `Cargo.lock` получил только две связи (`ctrlc` уже был в замке через `server`), новых пакетов нет (T-06-SC).
- `cargo fmt` не применялся ко всему `cli`: он перепишет посторонние строки `main.rs`, `inspect.rs` и тестов; правки в `main.rs` минимальные (модуль, ветка `Some("runner")`, раздел справки).
- Ожидание старта первой копии в тесте замка идёт по появлению `runner.log`, а не `runner.lock`: `runner.log` создаёт `Runner::new` уже после захвата замка, и гонка «второй копии удалось взять замок» исключена.

## Known Stubs

Нет.

## Threat Flags

Нет новой поверхности сверх `threat_model` плана. T-06-26 (паника) — `catch_unwind` и дочерняя предпроверка; T-06-27 (токен) — только окружение или файл, `--token` отвергается, `Debug` без токена; T-06-28 (подмена ролика) — `verify_result` и повторная сверка SHA-256; T-06-29 (огромная демка) — `max_demo_bytes` до скачивания; T-06-30 (две копии) — `runner.lock`.

## Замечания для 06-11 (живой прогон)

- Раннер и очередь приложения не знают друг о друге до фазы 8: справка предупреждает «не запускайте очередь приложения одновременно с раннером».
- Скрытый `--upload-chunk-limit N` останавливает раннер после N кусков результата с кодом выхода 0 и строкой «загрузка остановлена после N кусков (учение)»; результат остаётся в outbox.
- В журнале раннера строка «демка проверена: … пик памяти разбора N МиБ» — вход для калибровки стартовых лимитов (файл 2000 МиБ, распаковка 3072 МиБ, память 6144 МиБ, время 15 мин).
- В живом прогоне `cs2-cli runner` нужно запускать из консоли с интерактивной сессией Windows (CS2 не стартует из службы).

## Self-Check: PASSED

- FOUND: crates/runner/src/exec.rs, crates/runner/tests/exec.rs, crates/cli/src/runner_cmd.rs, crates/cli/tests/runner.rs
- FOUND коммиты `0c90336`, `8c39157`, `6beb5fe` в истории `main`
- `cargo test --release --workspace` зелёный
