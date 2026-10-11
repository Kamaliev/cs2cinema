---
phase: 05-yadro-servera-i-protokol
verified: 2026-10-11T12:30:00Z
status: human_needed
score: 5/5 must-haves verified
covered_files:
  - .github/workflows/ci.yml
  - .planning/phases/05-yadro-servera-i-protokol/05-01-PLAN.md
  - .planning/phases/05-yadro-servera-i-protokol/05-01-SUMMARY.md
  - .planning/phases/05-yadro-servera-i-protokol/05-02-PLAN.md
  - .planning/phases/05-yadro-servera-i-protokol/05-02-SUMMARY.md
  - .planning/phases/05-yadro-servera-i-protokol/05-03-PLAN.md
  - .planning/phases/05-yadro-servera-i-protokol/05-03-SUMMARY.md
  - .planning/phases/05-yadro-servera-i-protokol/05-04-PLAN.md
  - .planning/phases/05-yadro-servera-i-protokol/05-04-SUMMARY.md
  - .planning/phases/05-yadro-servera-i-protokol/05-05-PLAN.md
  - .planning/phases/05-yadro-servera-i-protokol/05-05-SUMMARY.md
  - Cargo.toml
  - crates/server/Cargo.toml
  - crates/server/cs2-server.example.json
  - crates/server/src/api.rs
  - crates/server/src/auth.rs
  - crates/server/src/blobs.rs
  - crates/server/src/cli.rs
  - crates/server/src/clock.rs
  - crates/server/src/config.rs
  - crates/server/src/core.rs
  - crates/server/src/db.rs
  - crates/server/src/http.rs
  - crates/server/src/lib.rs
  - crates/server/src/log.rs
  - crates/server/src/main.rs
  - crates/server/src/rules.rs
  - crates/server/src/secret.rs
  - crates/server/src/users.rs
  - crates/server/tests/common/mod.rs
  - crates/server/tests/core_states.rs
  - crates/server/tests/deps.rs
  - crates/server/tests/lease_http.rs
  - crates/server/tests/operator.rs
  - crates/server/tests/protocol_http.rs
  - crates/server/tests/secrets.rs
  - crates/server/tests/transfer_http.rs
  - crates/wire/Cargo.toml
  - crates/wire/src/lib.rs
covered_digest: "v3:sha256:a1b4aab358d9fd405ba6cf07bb75343a13ff3098122601c0499562d600812f27"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Открыть последний запуск CI на ubuntu-latest (job `test (ubuntu-latest)`) для коммита с фазой 05"
    expected: "`cargo test --release --workspace` и `cargo build --release -p server` зелёные, артефакт `cs2-server-linux-x64` приложен"
    why_human: "Верификатор работал на Windows, `gh` недоступен; запуск на Linux (SRV-01) и ветка `#[cfg(unix)]` проверки прав файла настроек в config.rs:200 здесь не исполнялись"
---

# Phase 5: Ядро сервера и протокол — отчёт о проверке

**Цель фазы:** отдельная программа `cs2-server` на Linux хранит пользователей, задачи, раннеров и файлы в своей SQLite, выдаёт задачу раннеру атомарно с арендой и эпохой, принимает и отдаёт файлы кусками с докачкой и проверкой SHA-256 и не пишет токены в журналы; проверяется без Telegram и CS2.
**Проверено:** 2026-10-11
**Статус:** human_needed (все автоматические проверки пройдены; осталась проверка запуска на Linux через CI)
**Повторная верификация:** нет, первичная

## Достижение цели

### Наблюдаемые истины (критерии успеха из ROADMAP)

| # | Истина | Статус | Доказательства |
|---|--------|--------|----------------|
| 1 | `cs2-server` — отдельная программа, настройки из файла и окружения, сама создаёт и мигрирует SQLite, данные переживают перезапуск; сборка не тянет `host`/`pipeline`/`demo` | VERIFIED | `crates/server/Cargo.toml`: единственная path-зависимость — `wire`; `cargo tree -p server` не содержит `host/pipeline/demo/windows`; тест `deps.rs::server_depends_only_on_wire_inside_the_workspace` и `wire_has_no_workspace_dependencies` зелёные. `config show`, порядок «умолчания < файл < окружение < флаги» описан в справке бинаря; тесты `db::tests::fresh_database_migrates_to_the_current_version`, `operator::submitted_demo_is_queued_and_seen_by_a_new_process`, `core_states::startup_grace_keeps_leases_after_a_server_restart`. Бинарь `cs2-server serve` я поднял сам: восстановление при старте и приём соединений работают. CI-шаг `cargo build --release -p server` на ubuntu есть (`.github/workflows/ci.yml:29-36`), результат запуска не проверен (см. human_verification). |
| 2 | `submit <демка> --player <ник>` ставит задачу без Telegram; токен раннера выдаётся, на сервере хранится только хеш, отозванный получает отказ | VERIFIED | `operator.rs`: `submitted_demo_is_queued_and_seen_by_a_new_process`, `runner_token_is_shown_once_and_only_its_hash_is_stored`, `revoked_token_no_longer_authenticates`, `submit_for_an_unknown_user_is_refused`; `protocol_http::missing_malformed_unknown_or_revoked_token_is_refused`; `lease_http::revoked_runner_is_refused_and_its_job_goes_to_another_runner`. |
| 3 | Виртуальные часы: все переходы receiving→queued→assigned→running→uploading→done/failed/cancelled; ровно один раннер получает задачу; истёкшая аренда возвращает в очередь; устаревшая эпоха не загружает и не завершает; тот же раннер возвращает потерянную задачу без остановки записи | VERIFIED | Чистая функция `rules::decide`; `core_states::every_state_and_event_pair_matches_the_transition_table`, `the_happy_path_walks_receiving_to_done_on_virtual_time`, `random_sequences_keep_the_invariants`; `protocol_http::concurrent_claims_hand_the_job_to_exactly_one_runner` (Barrier, N потоков) и `two_processes_on_one_database_hand_out_each_job_once`; `expired_lease_requeues_without_a_new_epoch_or_grant`, `claim_after_loss_bumps_the_epoch_and_fences_the_old_runner`, `same_runner_reattaches_a_lost_job_by_its_epoch`, `lease_loss_requeues_and_the_same_runner_takes_the_job_back`; `transfer_http::stale_epoch_cannot_upload_or_finish`, `lease_http::stale_runner_cannot_finish`, `stale_epoch_progress_is_refused_over_http`. Поведенческие истины подкреплены прогоном тестов, а не только наличием символов. |
| 4 | Куски по 8 МиБ, докачка с места обрыва, неверные размер/SHA-256 отвергаются; «ядовитая» задача уходит в `failed` после лимита выдач | VERIFIED | `transfer_http`: `chunk_size_constant_matches_the_documented_eight_mebibytes`, `result_uploads_in_chunks_and_closes_the_job`, `aborted_chunk_keeps_its_bytes_and_upload_resumes`, `wrong_offset_is_refused_with_the_real_one_and_the_connection_survives`, `size_or_sha_mismatch_is_not_a_result`, `demo_downloads_in_ranges_and_matches_its_sha256`, `range_past_the_end_is_416`, `expect_continue_upload_works_...`, `eight_mebibyte_put_and_a_response_over_32_kib_keep_their_content_length`; `blobs.rs:246-259` сверка SHA-256; `core_states::job_granted_max_times_fails_with_too_many_grants`, `lease_http::poison_demo_fails_after_max_grants_over_http`. |
| 5 | Подставные токены раннера и бота не находятся в журнале, колонках ошибок БД и ответах API после принудительных сбоев | VERIFIED | `secrets.rs`: `runner_token_in_a_failure_report_stays_out_of_logs_db_and_responses`, `forced_failures_leave_no_token_anywhere`, `panic_message_with_a_token_is_redacted`, `bot_token_never_leaves_config_errors_or_config_show`, `chunk_rejections_do_not_leak_the_token`, а также `the_check_itself_catches_a_leak` (проверка самой проверки). `secret::tests::*` на `Secret`/`Redactor`. |

**Счёт:** 5/5 истин подтверждены (0 присутствуют без поведенческой проверки).

### Покрытие требований

| Требование | План | Статус | Доказательство |
|------------|------|--------|----------------|
| SRV-01 | 05-01, 05-02 | SATISFIED (Linux-запуск — через CI, см. человеческую проверку) | отдельный бинарь `cs2-server`, конфиг файл+окружение, CI-сборка на ubuntu |
| SRV-02 | 05-01 | SATISFIED | `db.rs` миграции, пользователи/задачи/раннеры/файлы, тесты перезапуска |
| SRV-03 | 05-03, 05-04, 05-05 | SATISFIED | таблица переходов и случайные последовательности на виртуальных часах |
| SRV-04 | 05-02, 05-03 | SATISFIED | атомарный `claim` с эпохой, аренда, восстановление |
| SRV-05 | 05-01, 05-02, 05-03 | SATISFIED | хеш токена, отзыв |
| SRV-06 | 05-05 | SATISFIED | куски 8 МиБ, докачка, SHA-256 |
| SRV-07 | 05-03 | SATISFIED | `TOO_MANY_GRANTS` |
| SRV-08 | 05-01 | SATISFIED | `cs2-server submit` |
| SEC-04 | 05-01, 05-04, 05-05 | SATISFIED | `tests/secrets.rs` |

Все 9 ID из фазы (SRV-01..08, SEC-04) заявлены в PLAN-файлах и отмечены в REQUIREMENTS.md. Ни одна не потеряна, лишних (orphaned) требований для фазы 5 в REQUIREMENTS.md нет.

### Прогон тестов

`cargo test --release --workspace` на Windows: exit 0, FAILED нет. Для сервера: `server` lib 43, `core_states` 20, `deps` 2, `lease_http` 14, `operator` 13, `protocol_http` 10, `secrets` 6, `transfer_http` 15; `wire` 7. Регрессии остальных крейтов (`cli` pipeline, `host`, `store`, ...) отсутствуют.

### Антипаттерны

Маркеры `TBD/FIXME/XXX/TODO`, `todo!`, `unimplemented!` в `crates/server` и `crates/wire` не найдены.

| Файл | Строка | Паттерн | Серьёзность | Влияние |
|------|--------|---------|-------------|---------|
| `crates/server/src/http.rs` | 263-292 | CR-01 (подтверждён мной, см. ниже) | WARNING (не блокирует цель фазы; исправить до открытия сервера наружу) | один неаутентифицированный запрос убивает процесс |
| `crates/server/src/rules.rs` | 190-214 | WR-01 `release` игнорирует запрошенную отмену | WARNING | отменённая задача выдаётся повторно, тратит выдачу |
| `crates/server/src/core.rs` | 393-420 | WR-02 чистка «ядовитых» трогает `lease_lost = 1` | WARNING | задачу с потерянной арендой на третьей выдаче можно ошибочно провалить |
| прочее | | WR-03..06, IN-01..05 из 05-REVIEW.md | WARNING/INFO | укрепление, не цель фазы |

## Оценка CR-01 (независимая)

**Воспроизведено.** Исходник `tiny_http` 0.12 (`src/util/equal_reader.rs:66-86`) в `Drop` делает `vec![0; remaining_to_read]` на всё объявленное `Content-Length`. Я запустил собранный `target/release/cs2-server.exe serve` и отправил `POST /v1/runner/claim` без токена с `Content-Length: 1000000000000000`. Клиент получил `401`, после чего процесс умер: `memory allocation of 1000000000000000 bytes failed`. Тесты это не ловят (413-тесты используют `CHUNK_SIZE + 1`).

**Это не провал критериев успеха фазы.** Ни один из пяти критериев и ни одно из девяти требований (SRV-01..08, SEC-04) не формулирует устойчивость к враждебному транспорту. SEC-01..03 (фаза 6) касаются содержимого демок и ников, а не HTTP-слоя. Модуль `http.rs` прямо документирует, что таймаутами занимается прокси, а дефолтный `bind` — `127.0.0.1`. Поэтому статус я не понижаю до `gaps_found`.

**Но это реальный дефект высокой серьёзности, и в ROADMAP его никто не ведёт.** Прокси в фазе 9 (Caddy/Tailscale) не спасает автоматически: выделение происходит при `Drop` независимо от того, пришло ли тело, и сработает, если прокси не ограничивает `Content-Length` (у Caddy нужен явный `request_body max_size`). Пока сервер слушает только loopback, а раннер не реализован (фаза 6), риск теоретический; он становится практическим, когда сервер откроют раннеру по сети.

**Рекомендация:** не переносить молча. Либо (а) закрыть сейчас малым gap-closure планом (ограничить `body_length()` в диспетчере до `CHUNK_SIZE + запас` до отдачи запроса в `Drop`, плюс регрессионный тест `Content-Length: 10^15` на `/v1/runner/claim` без токена; запасной вариант — `[patch.crates-io]` с ограниченным буфером дренажа), либо (б) явно добавить пункт во вход фазы 6 как условие до сетевого подключения раннера. Заодно закрыть WR-01 и WR-02: это небольшие правки `rules.rs` и SQL-условие `lease_lost = 0`.

## Нужна проверка человеком

### 1. Запуск на Linux в CI

**Тест:** открыть последний запуск CI на `ubuntu-latest` для коммита с фазой 05.
**Ожидается:** `cargo test --release --workspace` и `cargo build --release -p server` зелёные, артефакт `cs2-server-linux-x64` загружен.
**Почему человек:** проверка выполнялась на Windows, `gh` недоступен; Linux-специфичная ветка `#[cfg(unix)]` (права файла настроек, `config.rs:200`) здесь не исполнялась.

## Итог

Цель фазы достигнута: все пять критериев успеха подтверждены кодом и зелёными тестами (включая конкурентную выдачу, ограждение эпохой, докачку, проверку SHA-256, лимит выдач и отсутствие токенов в журналах). Единственное открытое — подтверждение сборки и тестов на Linux в CI. Отдельно зафиксирован воспроизведённый CR-01: он не нарушает критерии фазы, но должен быть закрыт до сетевого подключения раннера.

---

_Проверено: 2026-10-11_
_Верификатор: Claude (gsd-verifier)_
