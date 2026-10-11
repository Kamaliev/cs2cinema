---
phase: 05-yadro-servera-i-protokol
plan: 02
subsystem: server
tags: [rust, tiny_http, http, long-poll, sqlite, redaction, ctrlc]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol (план 01)
    provides: Core, Db, auth (токены раннеров), Redactor, Config, wire
provides:
  - HTTP-слой на tiny_http 0.12: диспетчер, пул обработчиков, 503 Retry-After, catch_unwind (D-01, D-02)
  - POST /v1/runner/claim?wait=N: вход по токену до тела, long-poll, 200 Assignment или 204 (SRV-04, SRV-05)
  - Core::claim — атомарная выдача с эпохой и арендой одним UPDATE RETURNING в BEGIN IMMEDIATE (D-04)
  - Core::wake / work_generation / wait_for_work(_since) — пробуждение long-poll без потери сигнала
  - wire: ClaimRequest, Assignment, DemoInfo, Player, ApiError, codes, paths, headers
  - журнал через Redactor (LogSink, StderrSink, MemorySink, Log, install_panic_hook)
  - cs2-server serve (адрес из настроек, Ctrl+C/SIGTERM), сервер переживает перезапуск
  - тестовый стенд tests/common и tests/protocol_http.rs через настоящий HTTP на localhost
affects: [05-03, 05-04, 05-05, фаза 6, фаза 7]

estimate:
  tokens: 90000
  raw_tokens: 90000
  tasks: 2
  confidence: low
actuals:
  tokens: 17000
  tasks: 2
  commits: 2
plan_head_before: 2754bfa7f9130cf22f2c422073bdc622d3628e9f
plan_head_after: 5d97669a07cf74c4afa1b819e185282884aec7a4

tech-stack:
  added: [tiny_http 0.12 (без TLS-фич), ctrlc 3.5 (termination), chrono 0.4 (clock), ureq 3 (dev)]
  patterns:
    - "HTTP — тонкая обёртка: handle → route → Result<Reply, Reject>, единая точка ответа и строки журнала"
    - "Ожидание long-poll по Instant, а не по Clock; поколение wake читается до проверки очереди"
    - "503 из отдельного потока, а не из диспетчера; общий флаг остановки у HttpServer и Ctx"

key-files:
  created:
    - crates/server/src/http.rs
    - crates/server/src/api.rs
    - crates/server/src/log.rs
    - crates/server/tests/common/mod.rs
    - crates/server/tests/protocol_http.rs
  modified:
    - Cargo.lock
    - crates/server/Cargo.toml
    - crates/server/src/core.rs
    - crates/server/src/lib.rs
    - crates/server/src/cli.rs
    - crates/server/tests/operator.rs
    - crates/wire/src/lib.rs

key-decisions:
  - "tiny_http 0.12 подтверждён как HTTP-стек (D-01): все девять тестов протокола проходят на Windows; запасной axum не нужен"
  - "http::start сохраняет сигнатуру из плана; над ним start_with_flag принимает готовый флаг остановки, чтобы Ctx и HttpServer делили один флаг"
  - "Core::claim не фильтрует по max_grants: потолок выдач обрабатывает следующий план (аренды)"
  - "Пустое тело JSON читается как {}: ClaimRequest без тела допустим"

patterns-established:
  - "Строка журнала запроса: метод, путь без query, статус, мс, имя раннера; заголовки не пишутся; управляющие символы заменяются пробелом"
  - "Отказ 401 для чужого и отозванного токена пишет в журнал отпечаток (8 hex), но не токен"

requirements-completed: [SRV-01, SRV-04, SRV-05]

duration: ~40 мин
completed: 2026-10-11
---

# Phase 5 Plan 02: HTTP-сервер, вход по токену и атомарная выдача — Summary

**`cs2-server serve` на `tiny_http` 0.12: пул обработчиков с 503, вход раннера по токену, `POST /v1/runner/claim` с long-poll и выдачей одним `UPDATE … RETURNING` под `BEGIN IMMEDIATE`; задача, поставленная `submit`, уходит ровно одному раннеру с эпохой 1 и арендой 45 с.**

## Accomplishments

- Tracer: `submit` → `claim` по HTTP с токеном: 200 с `job_id`, `epoch = 1`, размером и SHA-256 демки, игроком; задача `assigned`, `grants = 1`, аренда сейчас + 45 с; повторный запрос — 204.
- Выдача атомарна: гонка двух и четырёх раннеров за общим `Barrier` и два `Core` на одном `server.db` (четыре потока по 10 вызовов, шесть задач) выдают каждую задачу ровно один раз.
- Long-poll: `claim?wait=N` ждёт `min(N, longpoll_secs)`, просыпается сразу при постановке в очередь (`Condvar` в `Core`), а для `submit` из другого процесса опрашивает базу раз в секунду.
- Вход: нет заголовка, `Basic`, короткий токен, чужой и отозванный токены дают 401 JSON `unauthorized` до чтения тела; в журнале отпечаток токена, самого токена нет.
- Пул: при `threads = 1` и висящем `claim?wait=3` второй запрос получает 503 `overloaded` с `Retry-After: 2`; после освобождения — 204.
- Паника обработчика даёт 500 и строку «обработчик упал», следующий запрос обслуживается; неизвестный путь — 404 JSON; `Running::stop()` во время `claim?wait=30` возвращается за секунды.
- `cs2-server serve`: первая строка stdout — адрес, остановка по Ctrl+C/SIGTERM (`ctrlc`), после перезапуска процесса тот же токен проходит и выданная задача на месте.
- `wire` получил типы протокола, коды ошибок, пути и имена заголовков.

## Task Commits

1. **Task 1 (tracer): serve, пул, вход по токену, атомарная выдача с эпохой** — `ead9115` (feat)
2. **Task 2: ровно один раннер, long-poll, 401, 503, паника, остановка, перезапуск** — `5d97669` (feat)

## Verification

- `cargo test --release -p server --test protocol_http` — 10 тестов, зелёные (включая `submitted_demo_is_claimed_over_http_once`, `concurrent_claims_hand_the_job_to_exactly_one_runner`, `two_processes_on_one_database_hand_out_each_job_once`, `overflow_answers_503_with_retry_after`, `missing_malformed_unknown_or_revoked_token_is_refused`).
- `cargo test --release -p server --test operator` — 12 тестов, включая `serve_binary_answers_claims_and_survives_a_restart`.
- `cargo test --release -p server --lib` — 28 тестов, включая `log_line_is_redacted_before_it_reaches_the_sink`.
- `cargo test --release -p wire` — 3 теста (`assignment_without_optional_fields_still_parses`).
- `cargo test --release --workspace` и `cargo build --release -p cli` — зелёные, `crates/cli/tests/pipeline.rs` проходит.
- Tracer gate: после Task 1 проверка прогнана до расширения, Task 2 начат только после зелёного результата.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Работа на защищённой ветке main**
- **Found during:** подготовка коммита Task 1
- **Issue:** `git.base-branch --is-protected main` вернул `true` (в `.planning/config.json` нет `git.allow_default_branch_commits`), а правило протокола коммита запрещает коммитить в защищённую ветку.
- **Fix:** создана ветка `gsd/phase-05-yadro-servera-i-protokol` от `main` (`2754bfa`); оба коммита задач и коммит метаданных лежат на ней. Конфигурация не менялась.
- **Следствие:** оркестратору нужно влить ветку в `main` (fast-forward) или разрешить коммиты в `main` настройкой. План 05-01 был закоммичен прямо в `main`.

**2. [Rule 2 - Missing critical] Потерянное пробуждение long-poll**
- **Found during:** Task 1, проектирование `claim`
- **Issue:** `wait_for_work(timeout)` без поколения теряет сигнал, пришедший между проверкой очереди и началом ожидания.
- **Fix:** добавлены `Core::work_generation()` и `wait_for_work_since(seen, timeout)`; `wait_for_work` оставлен как обёртка. Поколение читается до `claim`.
- **Files modified:** `crates/server/src/core.rs`, `crates/server/src/api.rs`
- **Commit:** `ead9115`

**3. [Rule 3 - Blocking] Общий флаг остановки до запуска HTTP**
- **Found during:** Task 1, `serve`
- **Issue:** `Ctx` должен содержать флаг остановки до построения обработчика, а `http::start` из плана создаёт флаг сам.
- **Fix:** `http::start` сохранён как в плане, добавлен `http::start_with_flag`, который принимает готовый флаг; `HttpServer::shutdown_flag()` возвращает тот же `Arc`.
- **Files modified:** `crates/server/src/http.rs`, `crates/server/src/lib.rs`
- **Commit:** `ead9115`

**4. [Rule 1 - Bug] clippy: вложенный `if` в `respond_error`**
- **Fix:** свёрнут в let-цепочку. **Commit:** `5d97669`

### Прочее

- Остановка по Ctrl+C/SIGTERM проверена только построением (`ctrlc` с фичей `termination`); тест останавливает процесс через `Child::kill`, как заложено в плане.
- Два замечания clippy в файлах плана 05-01 (`config.rs:262`, `tests/deps.rs:36`) не тронуты: вне области этого плана.

## Known Stubs

None. `ClaimRequest.status` принимается, но пока не сохраняется — состояние раннера появится в следующих планах протокола; это не мешает цели плана.

## Threat Flags

None - вся поверхность описана в `<threat_model>` плана. T-05-08 закрыт тестом `missing_malformed_unknown_or_revoked_token_is_refused`, T-05-09 — `concurrent_claims_hand_the_job_to_exactly_one_runner` и `two_processes_on_one_database_hand_out_each_job_once`, T-05-10 — `overflow_answers_503_with_retry_after`, T-05-11/T-05-12 — редактирование в `Log::write`, отпечатки вместо токенов и сообщение 400 без текста `serde_json`.

## Issues Encountered

- Первая попытка дописать тесты длинным `cat <<'XEOF'` оборвалась ошибкой разбора оболочки, ничего не применилось; файл записан инструментом `Write`. На результат не повлияло.

## Next Phase Readiness

Раннер входит по токену и получает задачу с эпохой и арендой; HTTP-слой принимает новые маршруты через `api::route`. Следующим планам нужны heartbeat и продление аренды, истечение аренды с потолком `max_grants`, отмена и передача файлов кусками поверх `Core`, `Reject`, `read_json` и `respond_*`.

## Self-Check: PASSED

- Файлы: `crates/server/src/{http,api,log}.rs`, `crates/server/tests/common/mod.rs`, `crates/server/tests/protocol_http.rs` найдены.
- Коммиты `ead9115` и `5d97669` — предки HEAD.
