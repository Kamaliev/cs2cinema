---
phase: 05-yadro-servera-i-protokol
plan: 01
subsystem: server
tags: [rust, sqlite, rusqlite, sha256, getrandom, serde, ci]
status: complete

requires:
  - phase: 04-host-i-pipeline
    provides: образцы миграций (`store`), поведенческий контракт CLI (`cs2-cli`), фейковые часы (`host::fake`)
provides:
  - крейт `wire` (PROTOCOL_VERSION, CHUNK_SIZE, JobState, CancelMode, fail::*)
  - крейт `server` и бинарник `cs2-server` с командами submit, job list/show, runner add/list/revoke, user add/list, config show
  - база server.db с миграциями (users, runners, jobs, files)
  - токен раннера cs2r_ + 64 hex, в базе только sha256
  - Secret и Redactor (основа SEC-04)
  - настройки: умолчания < файл JSON < CS2_SERVER_* < флаги
  - охрана зависимостей (tests/deps.rs) и сборка cs2-server в CI на Linux
affects: [05-02, 05-03, 05-04, 05-05, фаза 6, фаза 7]

actuals:
  tokens: 25750
  tasks: 3
  commits: 3
plan_head_before: a4c72da7bebdaf0041416ac2ea9c94f77e91f7b1
plan_head_after: 087094e05a509d06e09cd7eb72eb8780e9eab978

tech-stack:
  added: [getrandom 0.4, sha2 0.10 (в server), rusqlite 0.32 (в server)]
  patterns:
    - "ядро без HTTP: Core + внедрённые Clock/Redactor"
    - "копирование и хеширование вне мьютекса базы, затем короткая BEGIN IMMEDIATE транзакция"
    - "ошибки настроек называют место (файл/строка/столбец/переменная), но не значение"

key-files:
  created:
    - crates/wire/Cargo.toml
    - crates/wire/src/lib.rs
    - crates/server/Cargo.toml
    - crates/server/cs2-server.example.json
    - crates/server/src/lib.rs
    - crates/server/src/main.rs
    - crates/server/src/cli.rs
    - crates/server/src/config.rs
    - crates/server/src/secret.rs
    - crates/server/src/db.rs
    - crates/server/src/clock.rs
    - crates/server/src/core.rs
    - crates/server/src/blobs.rs
    - crates/server/src/auth.rs
    - crates/server/src/users.rs
    - crates/server/tests/operator.rs
    - crates/server/tests/deps.rs
  modified:
    - Cargo.toml
    - Cargo.lock
    - .github/workflows/ci.yml

key-decisions:
  - "Настройки сервера — JSON-файл без нового крейта toml; пример cs2-server.example.json"
  - "Время в базе — unix-мс по часам сервера (Clock); id задач AUTOINCREMENT, папка задачи не переиспользуется"
  - "Пути хранилища строятся только из целых id; имя файла пользователя хранится как очищенное orig_name"
  - "submit проверяет магию PBDEMS2 / zstd и размер до создания строки задачи"
  - "Таблицу job_events не создаём: журнал раннера остаётся на раннере; миграции только дописываются"

patterns-established:
  - "Сообщения об ошибках CLI проходят Redactor процесса, который строится из загруженных настроек"
  - "tests/deps.rs читает манифесты через include_str! и охраняет границы воркспейса"

requirements-completed: [SRV-01, SRV-02, SRV-05, SRV-08, SEC-04]

duration: ~35 мин
completed: 2026-10-11
---

# Phase 5 Plan 01: Ядро сервера, wire и команды оператора — Summary

**Крейты `wire` и `server` (бинарник `cs2-server`): своя SQLite с миграциями, постановка демки в очередь командой `submit` с SHA-256, токены раннеров `cs2r_…` с хешем в базе, настройки из файла/окружения/флагов без утечки секретов, сборка на Linux в CI.**

## Accomplishments

- `cs2-server submit {демка} --player {ник}` копирует демку в `blobs/incoming/{id}/demo` через `.part` + `rename`, считает SHA-256, ставит задачу в `queued`; `job list` и `job show` в другом процессе видят её с размером и хешем.
- Файл без магии `PBDEMS2\0`/zstd и файл больше `max_demo_mib` отвергаются до создания задачи; сбой копирования оставляет задачу `failed` с `RECEIVE_FAILED`.
- `runner add` печатает токен один раз; тест ищет токен в байтах `server.db` и `server.db-wal` и находит только хеш; после `runner revoke` `authenticate` отвечает `Revoked`.
- `Secret` (Debug/Display = `***`, без Serialize) и `Redactor` (точные секреты, форма `cs2r_` + hex, очистка управляющих символов и длины).
- Настройки: умолчания < файл (`--config`/`CS2_SERVER_CONFIG`, `deny_unknown_fields`, до 64 КиБ) < `CS2_SERVER_*` < флаги; ошибки файла дают строку и столбец без значения; `validate`; предупреждения о не-loopback `bind` и правах файла (unix).
- `tests/deps.rs`: сервер зависит от воркспейса только через `wire`, `wire` — ни от чего; `cargo tree -p server` не содержит `host`, `pipeline`, `demo`, `store`, `cs2`, `positions`, `geometry`.
- CI: шаги `Build cs2-server` (обе ОС) и `Upload Linux server build`.

## Task Commits

1. **Task 1: сквозной путь оператора (tracer)** — `78324c0` (feat)
2. **Task 2: токены раннеров, пользователи, Secret/Redactor** — `2baf4ee` (feat)
3. **Task 3: настройки, охрана зависимостей, CI** — `087094e` (feat)

## Verification

- `cargo test --release -p wire` — 2 теста, зелёные (включая `job_state_round_trips_through_json_and_db_strings`).
- `cargo test --release -p server` — 25 модульных, 2 в `deps.rs`, 11 в `operator.rs`, все зелёные.
- `cargo test --release --workspace` и `cargo build --release -p cli` — зелёные (`crates/cli/tests/pipeline.rs` проходит).
- `cargo tree -p server -e normal --prefix none | grep -cE '^(host|pipeline|demo|store|cs2|positions|geometry) v'` — 0.
- Tracer gate: сквозной путь `submit` → `job list` в новом процессе проверен тестом `submitted_demo_is_queued_and_seen_by_a_new_process` до расширения (Task 2, 3).

## Deviations from Plan

None - plan executed exactly as written.

Мелкие замечания, не меняющие поведения: `getrandom` добавлен в `Cargo.toml` сервера в Task 2 (как в плане), а не в Task 1; `cargo fmt -p server -p wire` применён с настройками по умолчанию (в репозитории нет `rustfmt.toml`), существующие крейты не форматировались.

## Known Stubs

None. Поле `Config::telegram_bot_token` читается и маскируется, но не используется до фазы 7 — это заявлено в плане, не заглушка.

## Threat Flags

None - новая поверхность целиком описана в `<threat_model>` плана (T-05-01..T-05-06 закрыты тестами `runner_token_is_shown_once_and_only_its_hash_is_stored`, `revoked_token_no_longer_authenticates`, `config_show_hides_the_bot_token`, `invalid_config_names_the_place_but_not_the_value`).

## Issues Encountered

- Вызов `Bash` с длинным heredoc в первой попытке записи файлов завершился ошибкой разбора оболочки; файлы записаны инструментом `Write`. На результат не повлияло.
- Леджер `gsd-plan-head-before-05-01` (протокол 0c) не создавался перед первым коммитом; база `plan_head_before` = `a4c72da` (HEAD до плана), `commits` посчитано по `git log` вручную: 3.

## Next Phase Readiness

База, ядро (`Core`, `Limits`, `Clock`), токены и настройки готовы; планы 05-02…05-05 могут строить HTTP, аренды с эпохой и файлы кусками поверх `Core`/`Db` и `wire`.

## Self-Check: PASSED

- Файлы: `crates/wire/src/lib.rs`, `crates/server/src/{lib,main,cli,config,secret,db,clock,core,blobs,auth,users}.rs`, `crates/server/tests/{operator,deps}.rs`, `crates/server/cs2-server.example.json` — найдены.
- Коммиты `78324c0`, `2baf4ee`, `087094e` — предки HEAD.
