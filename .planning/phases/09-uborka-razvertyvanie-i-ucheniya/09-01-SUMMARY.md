---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 01
subsystem: server-ci
tags: [server, lease, claim, ci, github-actions, secrets-scan, rust]

requires:
  - phase: 05-yadro-servera-i-protokol
    provides: "Core::claim_report с «ядовитыми» задачами (TOO_MANY_GRANTS), аренда lease_lost, Limits.lease_ttl_ms и startup_grace_ms; CI-воркфлоу с job test на ubuntu-latest и windows-latest"
provides:
  - "WR-02 закрыт: потерянная на последней выдаче задача ждёт свой раннер окно lease_ttl + startup_grace, после окна падает с прежним TOO_MANY_GRANTS"
  - "ci.yml: concurrency с cancel-in-progress, timeout-minutes: 60, отдельный шаг Dependency guard"
  - "Улики evidence/00-ci/: prepush-scan.txt (семь классов секретов), run-0-red.txt, run-1.txt (зелёный прогон на обеих ОС), RESULT.md"
affects: [09-10 учения (00-ci), публикация версии 1.1]

actuals:
  tokens: 3500
  tasks: 3
  commits: 5
plan_head_before: 980ff757634e9289e2036e6a2d1f895bcd2632b5
plan_head_after: 70bf09777bca38009c95e840afdfcaa241ff587d

tech-stack:
  added: []
  patterns:
    - "Окно возврата для потерянной аренды: чужой claim не добивает задачу, пока её раннер может вернуться через hello"
    - "Улика прогона CI без токена: публичный API actions/runs, jobs, artifacts, аннотации check-runs"

key-files:
  created:
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/prepush-scan.txt
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/run-0-red.txt
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/run-1.txt
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/RESULT.md
  modified:
    - crates/server/src/core.rs
    - crates/server/src/config.rs
    - crates/server/tests/core_states.rs
    - crates/server/tests/lease_http.rs
    - .github/workflows/ci.yml

key-decisions:
  - "Задача с lease_lost на последней выдаче падает с TOO_MANY_GRANTS только если claim делает её прежний владелец или окно возврата вышло (условие lease_lost = 0 OR runner_id IS NULL OR runner_id = ?2 OR updated_at <= ?3)"
  - "Класс секретов exact-values автор опубликовал без проверки точных значений (файла образцов нет); решение автора записано в prepush-scan.txt"
  - "Падение ubuntu-latest лечится в тестовой подготовке (temp_file с правами 0600), загрузчик настроек не менялся"

patterns-established:
  - "Красная попытка CI хранится отдельным файлом run-0-red.txt, главная улика run-1.txt — первый зелёный прогон"

requirements-completed: [OPS-03]

coverage:
  - id: D1
    description: "WR-02: потерянная на последней выдаче задача ждёт свой раннер, после окна возврата падает с TOO_MANY_GRANTS; старые тесты ядовитой демки без правок"
    requirement: OPS-03
    verification:
      - kind: unit
        ref: "crates/server/tests/core_states.rs#lost_lease_on_the_last_grant_waits_for_its_runner"
        status: pass
      - kind: unit
        ref: "crates/server/tests/core_states.rs#lost_lease_on_the_last_grant_fails_after_the_return_window"
        status: pass
      - kind: integration
        ref: "crates/server/tests/lease_http.rs#lost_lease_on_the_last_grant_survives_another_runners_claim_over_http"
        status: pass
    human_judgment: false
  - id: D2
    description: "CI обрывает повисшие и устаревшие прогоны, шаг охраны зависимостей виден отдельно, первый прогон версии 1.1 зелёный на ubuntu-latest и windows-latest, артефакт cs2-server-linux-x64 выгружен"
    requirement: OPS-03
    verification:
      - kind: other
        ref: "GitHub Actions run 38291001638 (head_sha a8420b5), evidence/00-ci/run-1.txt"
        status: pass
    human_judgment: false
  - id: D3
    description: "Поиск секретов по истории origin/main..HEAD перед публикацией, семь классов; решение автора по exact-values"
    requirement: OPS-03
    verification: []
    human_judgment: true
    rationale: "Пункты «НУЖНО ПРОВЕРИТЬ АВТОРУ» и класс exact-values снимает только автор; автор опубликовал без проверки точных значений"

duration: 36min+
completed: 2026-10-12
status: complete
---

# Phase 9 Plan 01: WR-02, подготовка CI и первый зелёный прогон версии 1.1 Summary

**Потерянная на последней выдаче задача ждёт свой раннер окно lease_ttl + startup_grace; CI с concurrency, timeout и охраной зависимостей; первый прогон версии 1.1 на ubuntu-latest и windows-latest зелёный, артефакт cs2-server-linux-x64 выгружен.**

## Performance

- **Duration:** от 36 минут (коммиты 23:01–23:37 +05:00; работа над Task 1 началась раньше первого коммита)
- **Completed:** 2026-10-12
- **Tasks:** 3 (Task 3 — точка решения автора с публикацией)
- **Files modified:** 9 (без учёта SUMMARY, STATE и ROADMAP)

## Accomplishments

- WR-02 закрыт в `Core::claim_report`: чужой `claim` не роняет задачу с `lease_lost = 1` на последней выдаче, пока не вышло окно возврата; её раннер возвращает задачу через `hello` и доводит до конца. Если `claim` делает сам владелец потерянной аренды или окно вышло, задача падает сразу с прежним `TOO_MANY_GRANTS`. Тесты `job_granted_max_times_fails_with_too_many_grants` и `poison_demo_fails_after_max_grants_over_http` не менялись.
- `ci.yml`: `concurrency` (`group: ci-${{ github.ref }}`, `cancel-in-progress: true`), `timeout-minutes: 60`, отдельный шаг `Dependency guard` (`cargo test --release -p server --test deps`).
- Перед публикацией история `origin/main..HEAD` (380 коммитов) проверена по семи классам секретов, улика без значений лежит в `prepush-scan.txt`.
- Автор опубликовал `main`; прогон 38291001638 на `a8420b5` зелёный на обеих ОС, артефакты `cs2-server-linux-x64` и `cs2-cli-windows-x64` созданы.

## Task Commits

1. **Task 1 (tracer): WR-02, потерянная на последней выдаче задача ждёт свой раннер** — `2d8d64e` (fix)
2. **Task 2: CI и поиск секретов перед публикацией** — `b14970a` (ci)
3. **Task 3: решение автора, улики прогонов, исправление красного CI**
   - `3d18967` (docs) — улики первого прогона: красный, решение автора по exact-values
   - `a8420b5` (fix) — тестовый файл настроек сервера создаётся с правами 0600
   - `70bf097` (docs) — улики зелёного прогона и `RESULT.md`

**Plan metadata:** коммит `docs(09-01): complete …` (SUMMARY, STATE, ROADMAP).

## Files Created/Modified

- `crates/server/src/core.rs` — запрос «ядовитых» задач с окном возврата и условием на владельца аренды (WR-02)
- `crates/server/tests/core_states.rs` — два регрессионных теста на виртуальных часах
- `crates/server/tests/lease_http.rs` — тот же случай через HTTP
- `crates/server/src/config.rs` — `temp_file` в тестах выставляет права 0600 под `cfg(unix)`
- `.github/workflows/ci.yml` — concurrency, timeout, Dependency guard
- `evidence/00-ci/` — `prepush-scan.txt`, `run-0-red.txt`, `run-1.txt`, `RESULT.md`

## Decisions Made

- Окно возврата берётся из существующих `Limits` (`lease_ttl_ms + startup_grace_ms`), новых настроек нет.
- Класс секретов `exact-values` остался непроверенным по решению автора (файла образцов `drills\patterns.txt` нет): автор опубликовал историю без этой проверки. Остальные шесть классов проверены, пункты «НУЖНО ПРОВЕРИТЬ АВТОРУ» в `prepush-scan.txt` остаются за автором.
- Красная первая попытка CI хранится в `run-0-red.txt`, а `run-1.txt` по критериям плана содержит первый зелёный прогон.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Тест bot_url_must_be_local_http падал на ubuntu-latest**
- **Found during:** Task 3 (первый прогон CI, прогон 38289485363)
- **Issue:** `temp_file` в `crates/server/src/config.rs` писал `cfg.json` с правами по umask; на Unix загрузчик добавляет предупреждение «доступен группе или всем: выполните chmod 600», а тест ждёт пустых предупреждений. Дефект тестовой подготовки, не продукта.
- **Fix:** после записи файл получает права 0o600 под `#[cfg(unix)]`. Загрузчик не менялся, другие тесты на предупреждение не опираются.
- **Files modified:** crates/server/src/config.rs
- **Verification:** `cargo test --release -p server` на Windows зелёный (ветка `cfg(unix)` локально не исполняется); на Linux подтверждено прогоном 38291001638.
- **Committed in:** `a8420b5`

**Итого:** 1 автоисправление (Rule 1).
**Влияние на план:** без расширения объёма; красный CI не закрывает план, поэтому исправление вошло в Task 3.

## Issues Encountered

- Первый прогон CI красный на обеих ОС. Linux объяснён и исправлен (см. выше). Падение `test (windows-latest)` (шаг Tests, код 1) причины не получило: лог недоступен без токена, локально на чистом чекауте `b14970a` 956 тестов зелёные, а прогон 38291001638 на Windows прошёл без изменений кода под Windows. Вероятнее всего это нестабильность раннера, но причина не установлена; при повторении нужен хвост лога шага Tests.
- `git push` делал только автор; исполнитель его не выполнял.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Код версии 1.1 собран и протестирован на Linux в CI, артефакт сервера доступен.
- Для публикации остаются за автором пункты «НУЖНО ПРОВЕРИТЬ АВТОРУ» из `prepush-scan.txt` (подставные токены в планах фаз 5 и 7 и имена полей конфигурации).
- Windows-нестабильность из первого прогона стоит держать на виду при следующих прогонах CI.

## Self-Check: PASSED

- Файлы на месте: core.rs, config.rs, core_states.rs, lease_http.rs, ci.yml, prepush-scan.txt, run-0-red.txt, run-1.txt, RESULT.md.
- Коммиты 2d8d64e, b14970a, 3d18967, a8420b5, 70bf097 присутствуют в истории main; измеренное число коммитов плана — 5 (`git rev-list --count 980ff75..70bf097`).
- Прогон 38291001638: `completed`, `success`, `head_sha` = a8420b5, оба job `test` success, артефакт `cs2-server-linux-x64` есть.
- `cargo test --release -p server` и `cargo test --release -p cli --test pipeline` зелёные.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-12*
