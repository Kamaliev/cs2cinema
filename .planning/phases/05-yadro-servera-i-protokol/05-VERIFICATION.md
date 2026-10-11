---
phase: 05-yadro-servera-i-protokol
verified: 2026-10-11T18:40:00Z
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
covered_digest: "v3:sha256:a80719f2002505f8affb8f91c7f7ff95950f6edad9730f8cfcd2d8f13197b6dd"
behavior_unverified: 0
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 5/5
  gaps_closed: []
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "Открыть последний запуск CI на ubuntu-latest (job `test (ubuntu-latest)`) для текущего коммита main"
    expected: "`cargo test --release --workspace` и `cargo build --release -p server` зелёные, артефакт `cs2-server-linux-x64` приложен"
    why_human: "Верификатор работал на Windows. Запуск на Linux (SRV-01) и ветка `#[cfg(unix)]` проверки прав файла настроек в config.rs не исполнялись. С фазы 6 воркспейс включает ещё крейт `runner`, так что прогон на Linux теперь шире, чем при первой проверке"
  - test: "Решить судьбу открытых дефектов ревью фазы 5: CR-01, WR-01, WR-02 (отдельный gap-план фазы 5 или явный перенос)"
    expected: "Либо запланирован gap-closure план до открытия сервера в сеть, либо дефекты записаны во вход фазы 9 как условие"
    why_human: "Это решение автора о приоритетах. Дефекты не нарушают пять критериев успеха, но CR-01 воспроизведён заново на текущем HEAD, а фаза 6 оставила их нетронутыми"
---

# Phase 5: Ядро сервера и протокол — отчёт о повторной проверке

**Цель фазы:** отдельная программа `cs2-server` на Linux хранит пользователей, задачи, раннеров и файлы в своей SQLite, выдаёт задачу раннеру атомарно с арендой и эпохой, принимает и отдаёт файлы кусками с докачкой и проверкой SHA-256 и не пишет токены в журналы; проверяется без Telegram и CS2.
**Проверено:** 2026-10-11
**Статус:** human_needed (автоматические проверки зелёные; Linux-запуск доказуем только в CI)
**Повторная верификация:** да. Прежний отчёт стал `stale`, потому что после него фаза 6 изменила `crates/wire`, `crates/server` (тесты, `cli.rs`, `core.rs`) и `Cargo.toml`. Все истины перепроверены по коду, а не по прежнему отчёту.

## Что изменила фаза 6 и влияет ли это на фазу 5

Разница между последним коммитом фазы 5 (`985235a`) и HEAD для `crates/server`, `crates/wire`, `Cargo.toml`, `.github`:

| Изменение | Файл | Влияние на поведение фазы 5 |
|-----------|------|-----------------------------|
| крейт `runner` добавлен в воркспейс | `Cargo.toml` | сервер от него не зависит: в `crates/server/Cargo.toml` единственная path-зависимость — `wire`; `cargo tree -p server` не показывает `runner`, `pipeline`, `host`, `demo`; в `tests/deps.rs` `runner` добавлен в запрещённый список, оба теста зелёные. Зависимость идёт в обратную сторону: `runner` берёт `server` только как `dev-dependency` для тестов |
| новые коды сбоев и `fail::KNOWN` | `crates/wire/src/lib.rs` | только добавление строковых констант (24 кода); `TOO_MANY_GRANTS` и `RECEIVE_FAILED` на месте, тест `known_codes_are_ascii_and_unique` зелёный |
| `Phase::Precheck` | `crates/wire/src/lib.rs` | вариант добавлен, `Phase::ALL` стал 10. В `rules.rs` он обрабатывается через `_ => Running` (`state_for_phase`, ветка прогресса, ветка возврата аренды), порядок переходов не менялся. Серверного HTTP-теста именно на `precheck` нет, есть тест разбора на стороне `wire` (`precheck_phase_is_additive_and_parses`); корректность вывожу из чтения кода |
| `cs2-server job show` печатает этап, дубль и результат | `cli.rs`, `core.rs` (`result_file`) | только чтение; тест `operator::job_show_prints_stage_take_and_result` зелёный |

Вывод: переходы, аренда, эпоха, файлы и редакция токенов не менялись, регрессий нет.

## Достижение цели

### Наблюдаемые истины (критерии успеха из ROADMAP)

| # | Истина | Статус | Доказательства |
|---|--------|--------|----------------|
| 1 | `cs2-server` — отдельная программа, настройки из файла и окружения, сама создаёт и мигрирует SQLite, данные переживают перезапуск; сборка не тянет `host`/`pipeline`/`demo` | VERIFIED (запуск на Linux — см. человеческую проверку) | Бинарь `cs2-server serve` запущен мной на Windows: восстановление при старте и приём соединений работают. `deps.rs::server_depends_only_on_wire_inside_the_workspace` и `wire_has_no_workspace_dependencies` зелёные; `db::tests::fresh_database_migrates_to_the_current_version`, `operator::submitted_demo_is_queued_and_seen_by_a_new_process`, `core_states::startup_grace_keeps_leases_after_a_server_restart`. CI-шаг `cargo build --release -p server` на ubuntu есть, `.github/workflows/ci.yml` с фазы 5 не менялся |
| 2 | `submit <демка> --player <ник>` ставит задачу без Telegram; токен раннера выдаётся, в базе только хеш, отозванный получает отказ | VERIFIED | `operator.rs` (14 тестов, включая `runner_token_is_shown_once_and_only_its_hash_is_stored`, `revoked_token_no_longer_authenticates`), `protocol_http::missing_malformed_unknown_or_revoked_token_is_refused`, `lease_http::revoked_runner_is_refused_and_its_job_goes_to_another_runner` |
| 3 | Виртуальные часы: все переходы receiving→queued→assigned→running→uploading→done/failed/cancelled; ровно один раннер получает задачу; истёкшая аренда возвращает в очередь; устаревшая эпоха не загружает и не завершает; тот же раннер возвращает потерянную задачу без остановки записи | VERIFIED (с оговоркой: граничный случай WR-02, см. ниже) | Чистая `rules::decide`; `core_states` (20 тестов: таблица пар «состояние-событие», счастливый путь, случайные последовательности с инвариантами), `protocol_http::concurrent_claims_hand_the_job_to_exactly_one_runner` и `two_processes_on_one_database_hand_out_each_job_once`, `lease_http` (14), `transfer_http::stale_epoch_cannot_upload_or_finish`. Прочитан `core.rs::claim_report`: выдача одним `UPDATE … RETURNING` внутри `BEGIN IMMEDIATE` |
| 4 | Куски по 8 МиБ, докачка с места обрыва, неверные размер/SHA-256 отвергаются; «ядовитая» задача уходит в `failed` после лимита выдач | VERIFIED | `transfer_http` (15 тестов): размер куска 8 МиБ, докачка после обрыва, `offset_mismatch` с настоящим смещением, отказ при несовпадении размера или SHA-256, скачивание демки по Range, 416; `core_states::job_granted_max_times_fails_with_too_many_grants`, `lease_http::poison_demo_fails_after_max_grants_over_http` |
| 5 | Подставные токены раннера и бота не находятся в журнале, колонках ошибок БД и ответах API после принудительных сбоев | VERIFIED | `secrets.rs` (6 тестов, в том числе проверка самой проверки `the_check_itself_catches_a_leak`), `secret::tests::*`. Фаза 6 в серверный путь редакции не вмешивалась |

**Счёт:** 5/5 истин подтверждены (0 присутствуют без поведенческой проверки). Все поведенческие истины подкреплены прогоном тестов, а не только наличием символов.

### Прогон тестов

`cargo test --release -p server -p wire` на Windows: exit 0, FAILED нет. `server` lib 43, `core_states` 20, `deps` 2, `lease_http` 14, `operator` 14 (+1 к прежнему: `job_show_prints_stage_take_and_result`), `protocol_http` 10, `secrets` 6, `transfer_http` 15; `wire` 9 (+2: `known_codes_are_ascii_and_unique`, `precheck_phase_is_additive_and_parses`).

### Покрытие требований

| Требование | План | Статус | Доказательство |
|------------|------|--------|----------------|
| SRV-01 | 05-01, 05-02 | SATISFIED на Windows; Linux — человеческая проверка | отдельный бинарь `cs2-server`, конфиг файл+окружение+флаги, CI-сборка на ubuntu |
| SRV-02 | 05-01 | SATISFIED | миграции `db.rs`, тесты перезапуска |
| SRV-03 | 05-03, 05-04, 05-05 | SATISFIED | таблица переходов и случайные последовательности на виртуальных часах |
| SRV-04 | 05-02, 05-03 | SATISFIED (граничный случай WR-02 открыт) | атомарная выдача с эпохой, аренда, восстановление |
| SRV-05 | 05-01, 05-02, 05-03 | SATISFIED | хеш токена, отзыв |
| SRV-06 | 05-05 | SATISFIED | куски 8 МиБ, докачка, SHA-256 |
| SRV-07 | 05-03 | SATISFIED | `TOO_MANY_GRANTS` |
| SRV-08 | 05-01 | SATISFIED | `cs2-server submit` |
| SEC-04 | 05-01, 05-04, 05-05 | SATISFIED | `tests/secrets.rs` |

Все 9 ID фазы заявлены в PLAN-файлах и отмечены `[x]` в REQUIREMENTS.md (таблица трассировки: Phase 5 / Complete). Потерянных и лишних (orphaned) требований для фазы 5 нет.

### Антипаттерны и открытые дефекты ревью

Маркеры `TBD/FIXME/XXX/TODO`, `todo!`, `unimplemented!` в `crates/server` и `crates/wire` не найдены. Файлы `http.rs`, `rules.rs`, `auth.rs`, `config.rs` с фазы 5 не менялись, поэтому дефекты из `05-REVIEW.md` остаются в силе. Три главных перепроверены по коду, CR-01 ещё и запуском.

| Файл | Дефект | Серьёзность | Проверка сейчас |
|------|--------|-------------|-----------------|
| `crates/server/src/http.rs`, `api.rs` | CR-01: запрос с огромным `Content-Length` без токена роняет процесс | WARNING высокой серьёзности, критерии фазы не нарушает | Воспроизведено на текущем HEAD: запущен `target/release/cs2-server.exe serve`, отправлен `POST /v1/runner/claim` без токена с `Content-Length: 1000000000000000`; клиент получил `401`, затем процесс умер (`memory allocation of 1000000000000000 bytes failed`). `body_length()` ограничивается только в `api.rs` (строки 259 и 534), на путях 401/404/503 ограничения нет |
| `crates/server/src/rules.rs:190-214` | WR-01: `release` игнорирует запрошенную отмену, задача снова уходит в очередь | WARNING | Подтверждено чтением: ветка `Event::Release` не смотрит `cancel_requested`, в отличие от `Expire` |
| `crates/server/src/core.rs:399-414` | WR-02: чистка «ядовитых» (`state='queued' AND grants >= max`) не исключает `lease_lost = 1`; любой `claim` проваливает задачу, чья аренда лишь истекла, и раннер уже не вернёт её | WARNING | Подтверждено чтением SQL. Затрагивает только случай «аренда потеряна на последней разрешённой выдаче». Обычный возврат потерянной задачи тем же раннером (`same_runner_reattaches_a_lost_job_by_its_epoch`, `lease_loss_requeues_and_the_same_runner_takes_the_job_back`) работает |
| прочее | WR-03..06, IN-01..05 из `05-REVIEW.md` | WARNING/INFO | укрепление, не цель фазы |

**Почему не `gaps_found`.** Ни один из пяти критериев успеха и ни одно из девяти требований не формулирует устойчивость к враждебному транспорту или граничный случай WR-02. Дефекты не прячутся под «готово»: они записаны здесь и в `human_verification` как решение автора. Фаза 6 по `06-CONTEXT.md` назвала их «отдельным gap-планом фазы 5 до подключения раннера по сети» и не закрыла, а в ROADMAP такого плана нет. Живой прогон 06-11 велся на loopback, где CR-01 не опасен; раннер из фазы 6 уже существует, поэтому при открытии сервера в сеть (фаза 9) риск станет практическим.

## Нужна проверка человеком

### 1. Запуск на Linux в CI

**Тест:** открыть последний запуск CI на `ubuntu-latest` для текущего коммита `main`.
**Ожидается:** `cargo test --release --workspace` и `cargo build --release -p server` зелёные, артефакт `cs2-server-linux-x64` приложен.
**Почему человек:** проверка выполнялась на Windows; ветка `#[cfg(unix)]` (права файла настроек) и сам запуск `cs2-server` на Linux здесь не исполнялись. Это единственное, что нельзя доказать без Linux-хоста.

### 2. Решение по CR-01, WR-01, WR-02

**Тест:** выбрать: (а) закрыть малым gap-closure планом фазы 5 (`/gsd-plan-phase 5 --gaps`) с регрессионными тестами `Content-Length: 10^15` на `/v1/runner/claim` без токена, `release` при запрошенной отмене и `lease_lost` при `grants == max`; (б) явно записать перенос во вход фазы 9 как условие до сетевого подключения.
**Ожидается:** решение зафиксировано в ROADMAP или STATE.
**Почему человек:** вопрос приоритета, а не наблюдаемого факта; сам факт дефекта подтверждён.

## Итог

Цель фазы достигнута: пять критериев успеха подтверждены кодом и зелёными тестами (`cargo test --release -p server -p wire`, ноль падений), изменения фазы 6 в `wire` и `server` чисто аддитивны и поведение фазы 5 не нарушают, граница зависимостей «`server` -> только `wire`» держится и охраняется тестом. Статус `human_needed` — из-за Linux-запуска, доказуемого только в CI, и открытых дефектов CR-01/WR-01/WR-02, по которым нужно решение автора.

---

_Проверено: 2026-10-11_
_Верификатор: Claude (gsd-verifier)_
