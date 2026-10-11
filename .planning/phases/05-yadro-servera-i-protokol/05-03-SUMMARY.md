---
phase: 05-yadro-servera-i-protokol
plan: 03
subsystem: server
tags: [rust, lease, epoch, state-machine, heartbeat, hello, sqlite]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol (план 02)
    provides: HTTP-слой, claim с эпохой и арендой, Core, Db, auth, Redactor, wire
provides:
  - rules::decide — чистый автомат задачи (Progress, Expire, Release, Reattach, Recover, ReceiveStale), D-05
  - POST /v1/jobs/{id}/progress — прогресс он же heartbeat; assigned -> running -> uploading, продление аренды (SRV-03)
  - Обратимая потеря аренды: истечение возвращает задачу в queued с lease_lost, тот же раннер с той же эпохой возвращает её себе (SRV-04)
  - Ограждение эпохой: 409 stale_epoch для старого раннера после новой выдачи (SRV-04)
  - claim_report: ленивое истечение, повторная выдача неподтверждённого claim, already_holding, потолок выдач TOO_MANY_GRANTS (SRV-07)
  - POST /v1/jobs/{id}/release (counts = false возвращает выдачу)
  - POST /v1/runner/hello: сверка holding (keep / abandon / cancel), 426 protocol_mismatch (SRV-05)
  - Core::recover до открытия порта: льготный срок аренд и RECEIVE_FAILED для зависшего приёма; поток sweep раз в 5 с
  - revoke_runner снимает аренды раннера (SRV-05)
  - wire: Phase, Take, ProgressRequest/Reply, ReleaseRequest, HelloRequest/Reply, Holding, Directive, DirectiveAction
affects: [05-04, 05-05, фаза 6, фаза 7]

estimate:
  tokens: 110000
  raw_tokens: 110000
  tasks: 3
  confidence: low
actuals:
  tokens: 36000
  tasks: 3
  commits: 3
plan_head_before: 85d14218fb9e9d3f5d1f3d701fc9c3bcae7ed11b
plan_head_after: 44b90459af49a9c04befa97fb3769e64afb2eb63

tech-stack:
  added: []
  patterns:
    - "Автомат без побочных эффектов: decide(строка, событие, now, Limits) -> новая строка; Core только грузит и пишет изменённые колонки одной транзакцией"
    - "Выдача — единственное исключение: один UPDATE ... RETURNING внутри BEGIN IMMEDIATE (claim_report)"
    - "Потеря аренды — не сбой: сервер не роняет задачу из-за молчания раннера; сбой только TOO_MANY_GRANTS по потолку выдач"
    - "Номер сообщения (last_seq), этап и дубль считаются заново в каждой эпохе; при возврате аренды эпоха и seq сохраняются"

key-files:
  created:
    - crates/server/src/rules.rs
    - crates/server/tests/core_states.rs
    - crates/server/tests/lease_http.rs
  modified:
    - crates/server/src/core.rs
    - crates/server/src/api.rs
    - crates/server/src/lib.rs
    - crates/server/src/auth.rs
    - crates/server/tests/common/mod.rs
    - crates/server/tests/protocol_http.rs
    - crates/server/tests/operator.rs
    - crates/wire/src/lib.rs

key-decisions:
  - "Срок аренды <= now считается истёкшим: revoke_runner ставит срок на «сейчас», и следующий sweep сразу возвращает задачу"
  - "last_seq, phase и take сбрасываются в UPDATE выдачи: счётчик сообщений раннера начинается заново в новой эпохе, иначе seq=1 нового раннера считался бы дублем"
  - "Потолок выдач проверяется на каждом claim любого раннера (шаг 4), а не только раннером потерянной задачи — «ядовитая» задача уходит в failed на следующей выдаче"
  - "Прогресс без take обнуляет take_n/take_m (прогресс — полный отчёт о состоянии); устаревший seq только продлевает аренду"
  - "hello не сохраняет req.name: кто это, решает токен"

patterns-established:
  - "Строка журнала: «раннер {имя} вернул себе задачу #{id} (эпоха {e})», «раннер {имя} вернул задачу #{id}: {причина}», «аренда задачи #{id} истекла — задача снова в очереди»"
  - "Причина release и app_version/status из hello проходят Redactor::clean с ограничением длины"

requirements-completed: [SRV-03, SRV-04, SRV-05, SRV-07]

duration: ~45 мин
completed: 2026-10-11
---

# Phase 5 Plan 03: Аренда и эпоха — Summary

**Прогресс раннера держит аренду, истёкшая аренда обратимо возвращает задачу в очередь (тот же раннер с той же эпохой забирает её обратно, не получая команды остановиться), новая выдача увеличивает эпоху и отсекает старого раннера, «ядовитая» демка падает с `TOO_MANY_GRANTS` после трёх выдач, а `hello`, `recover` и отзыв токена переживают обрывы и перезапуск сервера.**

## Accomplishments

- Tracer: `claim` → прогресс `fetch` (остаётся `assigned`) → прогресс `record` с дублем 1/3 (`running`, аренда сейчас + 45 с) → +46 с, `sweep` (`queued`, `lease_lost`, эпоха 1, выдача 1) → прогресс той же эпохи (`running`, `cancel = none`, строка журнала «вернул себе задачу»). Десять прогрессов через 10 с держат задачу `running` 100 с.
- `rules.rs` — чистый автомат без базы, часов и HTTP; 7 юнит-тестов (истечение только просроченной аренды, чужой раннер и эпоха = `stale_epoch`, `assigned -> running`, `upload -> uploading` без возврата, возврат потерянной аренды, `Reattach` по сохранённому этапу, `Recover` не укорачивает срок, возврат выдачи не ниже нуля).
- `claim_report` в одной транзакции: ленивое истечение, повторная выдача `assigned` без новой эпохи и выдачи, `already_holding` для `running`/`uploading`, `TOO_MANY_GRANTS` с русским объяснением и подсказкой, затем обычная выдача; очередь идёт дальше после упавшей задачи.
- `release` с `counts` (по умолчанию засчитывается); возврат чужой или повторный — `stale_epoch`.
- `hello`: сверка `holding`, продление и возврат потерянной аренды, директивы `keep`/`abandon`/`cancel`, версия и статус раннера в базе; другая версия протокола — 426 до записей.
- `serve`: `recover()` до `bind` (аренды продлены на `startup_grace`, приём старше 30 минут — `RECEIVE_FAILED`), поток `sweep` раз в 5 с, останавливаемый шагами по 200 мс; `sweep` тоже роняет зависший приём.
- `revoke_runner` ставит срок аренд раннера на «сейчас»; по HTTP отозванный токен получает 401 на `progress` и `hello`, а задача достаётся другому раннеру с эпохой 2.

## Task Commits

1. **Task 1 (tracer): аренда прогрессом, обратимая потеря, возврат тем же раннером** — `6d064f9` (feat)
2. **Task 2: повторная выдача, already_holding, потолок выдач, release** — `65ca597` (feat)
3. **Task 3: hello, восстановление, отзыв токена** — `44b9045` (feat)

## Verification

- `cargo test --release -p server --test lease_http` — 10 тестов, зелёные (включая `lease_loss_requeues_and_the_same_runner_takes_the_job_back`, `poison_demo_fails_after_max_grants_over_http`, `hello_with_another_protocol_is_refused_with_426`, `revoked_runner_is_refused_and_its_job_goes_to_another_runner`).
- `cargo test --release -p server --test core_states` — 11 тестов на виртуальных часах без HTTP.
- `cargo test --release -p server --lib rules` — 7 тестов; `cargo test --release -p wire` — 5 тестов.
- `cargo test --release -p server` целиком (`protocol_http` 10, `operator` 12, `deps`) и `cargo test --release --workspace` — зелёные; `cargo build --release -p cli` собирается, `crates/cli/tests/pipeline.rs` проходит.
- Tracer gate: после Task 1 обе проверки прогнаны и зелёные до расширения; Task 2 начат только после этого.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Тесты 05-02 противоречили новому поведению выдачи**
- **Found during:** Task 2
- **Issue:** повторный `claim` раннера с задачей в `assigned` теперь возвращает ту же задачу (шаг 2, PITFALLS P3.3), а не 204. Падали `submitted_demo_is_claimed_over_http_once` (повторный claim того же раннера), `two_processes_on_one_database_hand_out_each_job_once` (раннер в цикле из 10 claim получал одну и ту же задачу вместо следующей) и `serve_binary_answers_claims_and_survives_a_restart` (после перезапуска сервера тот же раннер получает свою выданную задачу).
- **Fix:** тесты уточнены: тот же раннер получает ту же задачу с эпохой 1 и выдачей 1, второй раннер — 204; в тесте двух процессов 8 раннеров на 6 задач с дедупликацией повторной выдачи; после перезапуска `serve` тот же токен получает 200 с эпохой 1, другой раннер — 204.
- **Files modified:** `crates/server/tests/protocol_http.rs`, `crates/server/tests/operator.rs`
- **Commit:** `65ca597`

**2. [Rule 1 - Bug] last_seq нового раннера считался дублем**
- **Found during:** Task 2, проектирование выдачи
- **Issue:** `UPDATE` выдачи из 05-02 не сбрасывал `last_seq`, `phase`, `take_*`: у раннера с новой эпохой `seq = 1` меньше сохранённого значения прежнего раннера, и его этап не записывался бы.
- **Fix:** выдача обнуляет `last_seq`, `phase`, `take_n`, `take_m`; при возврате аренды (та же эпоха) они сохраняются. Покрыто `claim_after_loss_bumps_the_epoch_and_fences_the_old_runner`.
- **Commit:** `65ca597`

**3. [Rule 3 - Blocking] Срок аренды «сейчас» не считался истёкшим**
- **Found during:** Task 3, `revoke_runner`
- **Issue:** с условием `lease_until < now` задача с `lease_until == now` не возвращалась в очередь следующим `sweep`, как требует план.
- **Fix:** истёкшей считается аренда с `lease_until <= now` (в SQL и в `decide`).
- **Commit:** `44b9045`

**4. [Уточнение плана] «last_seen_at прежний» при 426**
- `auth::authenticate` обновляет `last_seen_at` до чтения тела (раннер с верным токеном замечен), поэтому на 426 это поле меняется. Тест `hello_with_another_protocol_is_refused_with_426` проверяет то, что важно: строка задачи не тронута, `runners.version` не записана. Порядок «токен до тела» из 05-02 не менялся.

### Прочее

- `claim` сохранил сигнатуру; `Core::claim` теперь обёртка над `claim_report`.
- Рабочие файлы проекта в смеси CRLF/LF (индекс — LF, `core.autocrlf=true`); правки сделаны в LF, git нормализует при коммите.
- Два замечания clippy из 05-01 (`config.rs:262`, `tests/deps.rs:36`) не тронуты: вне области плана.

## Known Stubs

None. `HelloRequest.name` и `ClaimRequest.status` принимаются, но не используются в решениях сервера (имя определяет токен) — это не мешает цели плана. `Decision.discard_parts` вычисляется при истечении аренды с запрошенной отменой, но очистка кусков результата появится вместе с передачей файлов (05-05).

## Threat Flags

None - поверхность описана в `<threat_model>` плана. T-05-14 закрыт `claim_after_loss_bumps_the_epoch_and_fences_the_old_runner` и `stale_epoch_progress_is_refused_over_http`, T-05-15 — `job_granted_max_times_fails_with_too_many_grants` и `poison_demo_fails_after_max_grants_over_http`, T-05-16 — `runner_with_a_running_job_cannot_claim_another`, T-05-17 — `revoked_runner_loses_its_leases` и `revoked_runner_is_refused_and_its_job_goes_to_another_runner`, T-05-18 — `Redactor::clean` с ограничением длины для `reason`, `app_version`, `status`.

## Issues Encountered

- Первые попытки править файлы длинными heredoc в Bash дважды оборвались ошибкой разбора оболочки (ничего не применилось); правки выполнены скриптами из файлов и инструментом `Write`.
- Часть рабочих файлов оказалась в CRLF; замены по подстроке с `\n` не находили текст, пока файлы не были приведены к LF (повторно прогнаны без двойного применения).

## Next Phase Readiness

Состояния и аренды готовы: 05-04 добавляет отмену (`cancel_requested`, мягкая и жёсткая), сбой/успех раннера и окончательные состояния поверх `rules::decide` (`Event`, `Decision.discard_parts`); 05-05 — передачу демки и результата кусками. Контракт раннера для фаз 6 и 8: `progress`/`hello`/`release` с `epoch`, эпоха из `Assignment`, ответ `cancel` в `ProgressReply`.

## Self-Check: PASSED

- Файлы: `crates/server/src/rules.rs`, `crates/server/tests/core_states.rs`, `crates/server/tests/lease_http.rs` найдены.
- Коммиты `6d064f9`, `65ca597`, `44b9045` — предки HEAD.
