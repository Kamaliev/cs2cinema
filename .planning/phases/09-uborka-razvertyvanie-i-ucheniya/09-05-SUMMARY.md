---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 05
subsystem: server-deploy
tags: [server, deploy, systemd, docker-compose, backup, sqlite, tailscale, caddy, rust]

requires:
  - phase: 09-uborka-razvertyvanie-i-ucheniya
    provides: "План 09-02: Core::clean, Retention, CleanMode, Cleaned::summary, Config::retention и ключи retention_* / clean_interval_secs"
  - phase: 05-yadro-servera-i-protokol
    provides: "Db с общим соединением в режиме WAL, Core::open, загрузчик настроек"
provides:
  - "Db::vacuum_into: консистентная копия живой базы вне транзакции"
  - "Подкоманды cs2-server backup {папка} [--keep N] и clean [--dry-run], флаги --keep и --dry-run, ключи сроков в config show и справке"
  - "docs/deploy: README из шестнадцати разделов, docker-compose.yml, cs2-server.service, cs2-server-backup.service и .timer, cs2-server.json, check-host.sh"
  - "tests/deploy_docs.rs: семь тестов охраны заготовок развёртывания"
affects: [09-08 стенд и живое развёртывание по документу, 09-06 и 09-07 документы учений, 09-10 учение 08-day-soak]

actuals:
  tokens: 16300
  tasks: 3
  commits: 3
plan_head_before: affa1d1e997e5f7790588786adf15064a9dc30e0
plan_head_after: f84307576da800b1322eb605877f45ca910bbbdb

tech-stack:
  added: []
  patterns:
    - "Копия базы подкомандой VACUUM INTO, а не копированием файла: безопасна при работающем сервере и проверяется в CI"
    - "Секреты и адрес bot-api только в EnvironmentFile службы; файл настроек без них проходит validate, служебные команды работают без токена"
    - "Заготовки развёртывания включены в тесты через include_str!, опасные советы ловятся подстроками"

key-files:
  created:
    - crates/server/tests/deploy_docs.rs
    - docs/deploy/README.md
    - docs/deploy/docker-compose.yml
    - docs/deploy/cs2-server.service
    - docs/deploy/cs2-server-backup.service
    - docs/deploy/cs2-server-backup.timer
    - docs/deploy/cs2-server.json
    - docs/deploy/check-host.sh
  modified:
    - crates/server/src/db.rs
    - crates/server/src/cli.rs
    - crates/server/tests/operator.rs
    - .gitattributes

key-decisions:
  - "Основной доступ раннера по Tailscale Serve (https://<узел>.<tailnet>.ts.net), запасной Caddy с доменом; проверку plain_http_allowed в раннере не ослабляем. Выбор варианта автор делает в 09-08"
  - "Адрес bot-api и токен только в /etc/cs2-server/env; файл настроек 0600 cs2-server без секретов"
  - "Контейнер botapi-janitor (uid 101) включён по умолчанию: права сервера на файлы bot-api не проверены (A3); спайк 09-08 решит, нужен ли он"
  - "Копии встроенной подкомандой через VACUUM INTO, а не sqlite3 .backup: не нужен пакет sqlite3 на хосте, поведение тестируется"
  - "WR-03 фазы 5 закрыт документом (раздел 14), правки http.rs нет: loopback, Tailscale и таймауты Caddy с пределом тела 10 МБ"
  - "Служебная команда clean чистит папку bot-api только при заданном bot_api_url; в окружении службы он есть, у оператора на хосте нет, поэтому папку bot-api на хосте убирают поток сервера и janitor"

patterns-established:
  - "Имя копии server-ГГГГММДД-ЧЧММСС-ммм.db (UTC): ротация трогает только имена строго этого вида, чужие файлы в папке остаются"
  - "docs/deploy/* хранится с LF (.gitattributes), тест следит, что в файлах нет CR"

requirements-completed: [OPS-02]

coverage:
  - id: D1
    description: "cs2-server backup снимает целую копию через VACUUM INTO в server-ГГГГММДД-ЧЧММСС-ммм.db, оставляет N последних (по умолчанию 14), не трогает чужие файлы; на Linux папка 0700 и файл 0600"
    requirement: OPS-02
    verification:
      - kind: integration
        ref: "crates/server/tests/operator.rs#backup_makes_a_consistent_copy_and_keeps_only_the_newest"
        status: pass
      - kind: integration
        ref: "crates/server/tests/operator.rs#backup_leaves_foreign_files_alone"
        status: pass
      - kind: integration
        ref: "crates/server/tests/operator.rs#backup_refuses_a_bad_keep"
        status: pass
      - kind: unit
        ref: "crates/server/src/cli.rs#only_our_backup_names_are_pruned"
        status: pass
    human_judgment: false
  - id: D2
    description: "Копия, снятая при работающем serve на той же папке данных, проходит integrity_check и содержит все задачи"
    requirement: OPS-02
    verification:
      - kind: integration
        ref: "crates/server/tests/operator.rs#backup_while_the_server_runs_is_consistent"
        status: pass
    human_judgment: false
  - id: D3
    description: "cs2-server clean [--dry-run] делает проход уборщика со сроками из настроек и печатает итог числами; config show печатает ключи retention_* и clean_interval_secs"
    requirement: OPS-02
    verification:
      - kind: integration
        ref: "crates/server/tests/operator.rs#clean_dry_run_prints_counts_and_deletes_nothing"
        status: pass
      - kind: integration
        ref: "crates/server/tests/operator.rs#config_show_prints_retention_keys"
        status: pass
    human_judgment: false
  - id: D4
    description: "Заготовки службы, compose, таймера, образца настроек и скрипта проверки не расходятся с кодом сервера и не содержат опасных советов (порт наружу, динамический пользователь, секреты в файле настроек)"
    requirement: OPS-02
    verification:
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#deploy_config_is_accepted_and_keeps_secrets_out"
        status: pass
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#service_runs_as_a_plain_user_and_restarts_after_failures"
        status: pass
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#compose_publishes_bot_api_only_on_loopback"
        status: pass
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#readme_never_exposes_the_server_port"
        status: pass
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#backup_timer_runs_the_backup_subcommand_daily"
        status: pass
      - kind: integration
        ref: "crates/server/tests/deploy_docs.rs#check_host_script_covers_services_ports_and_backups"
        status: pass
    human_judgment: false
  - id: D5
    description: "По README автор поднимает цепочку на чистой Linux-машине, снаружи открыт только задуманный порт, развёртывание переживает перезагрузку"
    requirement: OPS-02
    verification: []
    human_judgment: true
    rationale: "Команды документа не проверялись на живом хосте (нет Linux-машины и Docker на этой): systemd, ufw, Docker, Tailscale и Caddy проверит живое развёртывание плана 09-08"

duration: 40min
completed: 2026-10-12
status: complete
---

# Phase 9 Plan 05: Развёртывание сервера Summary

**Подкоманды `backup` (целая копия работающей базы через `VACUUM INTO`, ротация, права 0700/0600) и `clean [--dry-run]`, плюс `docs/deploy`: compose для bot-api только на loopback, служба systemd, ежесуточный таймер копий, Tailscale Serve или Caddy для раннера, `check-host.sh` и шестнадцатиразделный документ, охраняемые тестом**

## Performance

- **Duration:** около 40 мин
- **Completed:** 2026-10-12T19:05Z
- **Tasks:** 3 (трассер и два обычных)
- **Files modified:** 12 (8 созданы)

## Accomplishments

- `Db::vacuum_into` снимает консистентную копию живой базы под мьютексом соединения вне транзакции; существующий файл — ошибка «копия уже существует: …».
- `cs2-server backup {папка} [--keep N]`: имя `server-ГГГГММДД-ЧЧММСС-ммм.db` по UTC, оставляются N последних (по умолчанию 14, допустимо 1–1000), удаляются только файлы строго такого имени, на Linux папка копий 0700 и файл 0600. Копия при работающем `serve` проходит `integrity_check`.
- `cs2-server clean [--dry-run]` вызывает `Core::clean` из плана 09-02 со сроками из настроек и печатает «уборка: …» или «уборка (проверка): …» числами, для проверки добавляет номера задач; `config show` и справка знают семь ключей сроков.
- `docs/deploy`: `telegram-bot-api` только на `127.0.0.1:8081` и контейнер `botapi-janitor` (uid 101); служба под обычным пользователем `cs2-server` с `Restart=on-failure`, `StartLimitIntervalSec=0`, секретами из `EnvironmentFile`; таймер копий раз в сутки; образец настроек без токена и адреса bot-api.
- `check-host.sh`: службы, контейнеры, слушающие порты (снаружи только 22, при Caddy ещё 80 и 443), ответ сервера 401, свежесть копии, права `600 root:root` файлов секретов, токен не в файле настроек, `tailscale serve status`, место на диске.
- README: шестнадцать разделов от пользователя и бинарника до перезагрузки, отката и секретов; WR-03 фазы 5 закрыт разделом 14; проверка «снаружи открыт только задуманный порт» (`ss -tlnp`, `nmap`, `Test-NetConnection`) и «переживает перезагрузку» записаны командами.

## Task Commits

1. **Task 1 (трассер): backup и clean для оператора** — `235fbe7` (feat)
2. **Task 2: bot-api в compose, служба systemd, образец настроек и тест** — `0b4156b` (feat)
3. **Task 3: документ развёртывания, таймер копий, проверка хоста** — `f843075` (docs)

**Plan metadata:** коммит `docs(09-05)` с этим SUMMARY, STATE.md и ROADMAP.md.

## Files Created/Modified

- `crates/server/src/db.rs` — `Db::vacuum_into`
- `crates/server/src/cli.rs` — подкоманды, флаги `--keep` и `--dry-run`, ротация копий, ключи сроков в `config show` и справке
- `crates/server/tests/operator.rs` — семь новых имён в `ENV_KEYS`, шесть тестов оператора
- `crates/server/tests/deploy_docs.rs` — семь тестов заготовок
- `docs/deploy/*` — README, compose, юниты, таймер, образец настроек, скрипт проверки
- `.gitattributes` — `docs/deploy/* text eol=lf`

## Decisions Made

См. `key-decisions` во frontmatter. Главное: доступ раннера по HTTPS (Tailscale Serve или Caddy), проверка `plain_http_allowed` не ослаблена; секреты и адрес bot-api только в окружении службы; WR-03 закрыт документом, а не правкой `http.rs`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Правило LF для заготовок развёртывания**
- **Found during:** Task 2 (проверка `.gitattributes`, как требовал контекст исполнения)
- **Issue:** правил для `*.sh` и юнитов не было; на Windows с `core.autocrlf` рабочая копия получила бы CRLF, а `bash` и systemd на хосте CRLF не принимают
- **Fix:** строка `docs/deploy/* text eol=lf` в `.gitattributes`; тест `deploy_files_have_unix_line_endings` следит, что в файлах нет CR; скрипту выставлен исполняемый бит (`git update-index --chmod=+x`)
- **Files modified:** `.gitattributes`, `crates/server/tests/deploy_docs.rs`
- **Committed in:** `0b4156b`, `f843075`

**2. [Rule 2 - Missing critical] Три дополнительных теста**
- **Found during:** Tasks 1 и 3
- **Issue:** пять тестов оператора и шесть тестов заготовок из плана не охраняли три места: ротация не должна удалять чужие файлы в папке копий, шестнадцать разделов README должны идти по порядку, в скрипте и юнитах не должно быть CR
- **Fix:** `backup_leaves_foreign_files_alone`, модульный `only_our_backup_names_are_pruned`, проверка шестнадцати заголовков в `readme_never_exposes_the_server_port`, `deploy_files_have_unix_line_endings`
- **Committed in:** `235fbe7`, `f843075`

---

**Total deviations:** 2 auto-fixed (1 blocking, 1 missing critical)
**Impact on plan:** обе правки только добавляют охрану; поведение подкоманд и содержимое заготовок соответствуют плану.

## Issues Encountered

- Heredoc с русским текстом и `«»` в Bash дважды обрывался на разборе кавычек (скрипт правки `cli.rs` через Python): правки сделаны инструментом Edit, а Rust-тесты дописаны через heredoc с ASCII-ограничителем без проблем.
- `cargo fmt -p server --check` показывает расхождения в посторонних файлах (`bot_faceit.rs`, `core_states.rs` и др.): форматированы только файлы этого плана через `rustfmt`.

## Known Stubs

None. Заглушек и TODO в созданных файлах нет.

## Threat Flags

Нового сетевого периметра код не добавил (подкоманды локальны). Из реестра плана закрыты тестами: T-09-15 (`compose_publishes_bot_api_only_on_loopback`), T-09-16 (`readme_never_exposes_the_server_port`), T-09-17 (раздел 14), T-09-18 (права копий, `backup_makes_a_consistent_copy_and_keeps_only_the_newest` под `cfg(unix)`), T-09-19 (`deploy_config_is_accepted_and_keeps_secrets_out`, `stat -c %a` в `check-host.sh`), T-09-20 и T-09-21 (разделы 4 и 2), T-09-22 (`service_runs_as_a_plain_user_and_restarts_after_failures`).

## User Setup Required

None для этого плана. Живое развёртывание по документу и выбор варианта доступа (Tailscale Serve или Caddy) — Task плана 09-08.

## Next Phase Readiness

- Для 09-08 готовы заготовки и порядок развёртывания; ни одна команда документа ещё не выполнялась на живом хосте (systemd, ufw, Docker, Tailscale, Caddy, права на файлы bot-api, имена переменных образа bot-api, формат вывода `tailscale serve status`).
- Блок `#[cfg(unix)]` теста `backup_makes_a_consistent_copy_and_keeps_only_the_newest` (права 0700/0600) на Windows не компилируется и не запускается; его проверит CI на ubuntu-latest.
- Флаг для 09-08: `check-host.sh` не запускался ни разу (только `bash -n`); формат столбцов `ss -tlnp` и ответ `/v1/runner/hello` без токена (ожидается 401) проверить на живом хосте.
- Опция `clean` на хосте не видит адрес bot-api (он в окружении службы), поэтому не считает папку bot-api; это сделано сознательно и записано в README, раздел 15.

## Self-Check: PASSED

- Файлы на диске: `docs/deploy/README.md`, `docker-compose.yml`, `cs2-server.service`, `cs2-server-backup.service`, `cs2-server-backup.timer`, `cs2-server.json`, `check-host.sh`, `crates/server/tests/deploy_docs.rs`.
- Коммиты в ветке: `235fbe7`, `0b4156b`, `f843075`; `commits: 3` измерено `git rev-list --count` от `plan_head_before`.
- `cargo test --release -p server` зелёный целиком (66 модульных, `operator` 23, `deploy_docs` 7, `cleanup` 13, `deps` 2), `cargo test --release -p cli --test pipeline` зелёный, `bash -n docs/deploy/check-host.sh` без ошибок.
- `crates/runner`, `.github/workflows/ci.yml`, `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `crates/store/src/journal.rs` не затронуты.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-12*
