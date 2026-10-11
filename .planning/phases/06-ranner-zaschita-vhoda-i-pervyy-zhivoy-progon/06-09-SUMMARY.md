---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 09
subsystem: runner
tags: [rust, policy, release, pause, grants, job-show, tracer]
status: complete

requires:
  - phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
    provides: 06-01 — FailCode::ALL, wire::fail::KNOWN/JOB_ERROR/BAD_DEMO; 06-08 — Shared/Tee/Uplink, Control, серверная отмена
  - phase: 05-yadro-servera-i-protokol
    provides: ручка release (counts), потолок max_grants → TOO_MANY_GRANTS, hello.status
provides:
  - policy.rs — Disposition, Flags, dispose, failure_of, PausePolicy, PauseReason (чистая логика без сети и часов)
  - release с засчитанной выдачей для классов «окружение» и «запуск» (D-02 буквально), решение в runner_state.json (pending_release)
  - пауза раннера после двух одинаковых сбоев подряд: Tick::Paused, Control::resume, hello.status = «пауза: {код}»
  - cs2-server job show печатает этап, «дубль k из n» и принятый результат (размер, SHA-256)
affects: [06-10 cs2-cli runner и Control::resume из консоли, 06-11 живой прогон и учения, фаза 7 бот]

estimate:
  tokens: 80000
  raw_tokens: 80000
  tasks: 3
  confidence: low
actuals:
  tokens: 11800
  tasks: 3
  commits: 3
plan_head_before: 3539772230a1391aeb86c419fc4a55d0fc6dba01
plan_head_after: f231193109963870c459100b92110b2bc8cf913b

tech-stack:
  added: []
  patterns:
    - "Итог исполнителя разбирает чистая dispose(&Result, Flags) -> Disposition; lib.rs только применяет решение"
    - "Решение о release пишется в runner_state.json до запроса (pending_release), поэтому перезапуск повторяет release, а не гоняет исполнителя заново"
    - "Серия паузы PausePolicy — копия QueuePolicy::count приложения: (код, класс), две подряд, после паузы заново"

key-files:
  created:
    - crates/runner/src/policy.rs
    - crates/runner/tests/policy.rs
  modified:
    - crates/runner/src/lib.rs
    - crates/runner/src/state.rs
    - crates/runner/src/tee.rs
    - crates/runner/tests/common/mod.rs
    - crates/server/src/cli.rs
    - crates/server/src/core.rs
    - crates/server/tests/operator.rs

key-decisions:
  - "D-02 буквально (решение автора): release всегда ставит counts = true, и для «окружения», и для «запуска»; бесконечный круг останавливают пауза раннера после двух одинаковых сбоев и max_grants сервера (TOO_MANY_GRANTS). Протокол и сервер не менялись."
  - "Серверная отмена проверяется до release (WR-01 сервера не чинится): сбой окружения при запрошенной отмене даёт finish(Cancelled); если отмена пришла после решения о release, report() закрывает задачу итогом, а не возвращает в очередь"
  - "release_cooldown (по умолчанию 15 с, RELEASE_COOLDOWN) ждёт не в tick, а перед следующим claim, прерываясь флагом остановки"
  - "Tick получил варианты Released и Paused; вторая подряд неисправность возвращает Paused уже с того tick, где задача возвращена"
  - "Release в Disposition несёт ещё message и hint (в плане только reason, code, class): они нужны PauseReason, чтобы пауза объясняла причину по-русски"

patterns-established:
  - "Новый класс итога = новый вариант Disposition + строка в dispose + строка в PausePolicy::after; lib.rs менять только в одном match"

requirements-completed: [RUN-04, RUN-03]

duration: около 60 мин
completed: 2026-10-11
---

# Phase 6 Plan 09: класс сбоя решает судьбу задачи Summary

**Неисправность окружения или запуска возвращает задачу друга в очередь через `release` с засчитанной выдачей, контентный сбой закрывает её своим кодом, две одинаковые неисправности подряд ставят раннер на паузу, а «ядовитую» задачу сервер закрывает `TOO_MANY_GRANTS` после `max_grants` выдач; `cs2-server job show` показывает этап, дубль и принятый результат.**

## Performance

- **Duration:** около 60 мин
- **Completed:** 2026-10-11
- **Tasks:** 3 (трассёр, две TDD-задачи)
- **Files modified:** 9

## D-02 подтверждён буквально

`release` всегда засчитывает выдачу (`counts: true`, `grep -rn "counts: false" crates/runner/src` пуст). Потолок держит сервер: `max_grants` (по умолчанию 3) → `TOO_MANY_GRANTS`. Пауза раннера включается после двух одинаковых сбоев «окружения» или «запуска» подряд. Следствие, принятое автором (T-06-38): если машина автора неисправна дольше `max_grants` выдач, задача друга заканчивается `TOO_MANY_GRANTS`, и её ставят заново.

## Accomplishments

- Трассёр `environment_failure_releases_the_job_back_to_the_queue` на настоящем `server::serve`: `FOREIGN_GAME_RUNNING` → `Tick::Released`, задача `queued`, `grants == 1`, `runner_id` пуст, `fail_code` пуст, состояние раннера и демка убраны.
- `launch_failure_releases_the_job_with_a_counted_grant`: `GAME_NOT_STARTED` → `queued`, `grants == 1`.
- `content_failure_finishes_the_job_with_its_code`: `BAD_DEMO` → `failed` с `fail_code == "BAD_DEMO"`, классом «прочее»; следующая неисправность окружения паузы не даёт (серия сброшена), следующая задача берётся.
- `cancel_requested_before_release_closes_the_job_as_cancelled`: отмена запрошена на сервере, исполнитель вернул `FOREIGN_GAME_RUNNING` → задача `cancelled`, а не `queued` (обход WR-01).
- `pending_release_is_repeated_after_restart`: состояние `Reporting` с `pending_release`, записанное после настоящего `claim`, → новый `Runner` повторяет `release`, исполнитель не вызывается, задача `queued`, `grants == 1`.
- `two_identical_environment_failures_pause_the_runner_until_resume`: вторая выдача подряд даёт `Tick::Paused`, ещё два `tick` не вызывают `claim` (вторая задача `queued`, `grants == 0`), сервер видит `hello.status = «пауза: FOREIGN_GAME_RUNNING»`, `Control::resume` снимает паузу, серия после неё начинается заново.
- `environment_release_counts_every_grant_until_too_many_grants`: при вечном сбое окружения и `resume` на каждой паузе исполнитель вызван ровно 3 раза, задача `failed` с `TOO_MANY_GRANTS`, `grants == 3`.
- Страж `fail_codes_of_pipeline_are_known_to_the_protocol`: каждый `FailCode::ALL[i]` и `JOB_ERROR` входят в `wire::fail::KNOWN`.
- Модульные `dispose_*` (9) и `pause_*` (5) в `policy.rs`: классы, отмена и локальная остановка, потеря задачи, границы длины текста, серия паузы и её сброс.
- `job_show_prints_stage_take_and_result`: задача в очереди без строк `этап:`/`дубль:`/`результат:`; после прогресса `record` 2 из 3 — `этап: record` и `дубль: 2 из 3`; после принятого результата — `состояние: готово (done)` и `результат: 2.0 МиБ, sha256 {хеш загруженного файла}`.
- `cargo test --release --workspace` зелёный; `cargo tree -p server -e normal` не тянет `pipeline`, `runner`, `host`.

## Task Commits

1. **Task 1: release для окружения и запуска, finish со своим кодом для контента (трассёр)** - `765717a` (feat)
2. **Task 2: пауза после двух одинаковых сбоев, потолок TOO_MANY_GRANTS, страж кодов (TDD)** - `785ef02` (test)
3. **Task 3: job show печатает этап, дубль и результат (TDD)** - `f231193` (feat)

`PausePolicy` и `Control::resume` вошли в коммит Task 1: они в одном файле и в одном `match` с `dispose`, раздельно без сломанной сборки не собрать (как в 06-08). В коммите Task 2 остались тесты и исправление, найденное ими.

## Files Created/Modified

- `crates/runner/src/policy.rs` — новый: `Disposition`, `Flags`, `dispose`, `failure_of`, `PausePolicy`, `PauseReason`, 14 модульных тестов
- `crates/runner/src/lib.rs` — применение `Disposition`, `release`, пауза, `Tick::{Released, Paused}`, `Control::resume`, `Config.release_cooldown`, `RELEASE_COOLDOWN`; локальные `JOB_ERROR`/`BAD_DEMO` и `outcome_of` удалены
- `crates/runner/src/state.rs` — `HeldJob.pending_release` (`#[serde(default)]`)
- `crates/runner/src/tee.rs` — `Shared::cancel_mode()`
- `crates/runner/tests/common/mod.rs` — `release_cooldown: ZERO` в настройках стенда
- `crates/runner/tests/policy.rs` — новый: 8 стендовых тестов
- `crates/server/src/core.rs` — `Core::result_file`
- `crates/server/src/cli.rs` — `print_job` печатает `этап:`, `дубль:`, `результат:`
- `crates/server/tests/operator.rs` — тест вывода

## Decisions Made

См. `key-decisions` выше.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Пауза не возвращалась с ветки `claim`**
- **Found during:** Task 2, тест `two_identical_environment_failures_pause_the_runner_until_resume`
- **Issue:** подмена `Tick::Released` на `Tick::Paused` стояла только в ветке удерживаемой задачи; вторая неисправность подряд случается на задаче, взятой тем же `tick` через `claim`, и тест получил `Released` вместо `Paused`.
- **Fix:** `tick` собран в одну точку выхода: итог ветки (`drive` или `claim` + `accept`) проходит подмену на `Paused`, если пауза включена.
- **Files modified:** `crates/runner/src/lib.rs`
- **Commit:** `785ef02`

### Отличия от текста плана

- **`Control::resume` создан здесь:** план ссылается на него «из 06-08», но в 06-08 был только `Control::stop`. Добавлен вместе с флагом `resume` в `Runner`.
- **`Disposition::Release` несёт `message` и `hint`** кроме `reason`, `code`, `class`: паузе нужно объяснение для оператора. `Flags` получил поле `elapsed_ms` (план передаёт его отдельно в `failure_of`).
- **Повтор `release` на уже отпущенной задаче (409 `stale_epoch`)** отдельным стендовым тестом не проверяется: после `hello` сервер сам велит бросить такую задачу, и до `release` дело не доходит; ветка `Verdict::Drop` в `release()` использует тот же `judge`, что `finish` (покрыт тестами 06-06).
- **Тест вывода `job show` на `TestServer`** из `crates/server/tests/common` (HTTP-стенд), а не через `Core::open`: загрузка результата и `finish_done` там уже готовы.

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей: `hello` с `status = «пауза: {код}»` идёт тем же авторизованным запросом, текст проходит `Redactor` сервера (T-06-24); хвост и сообщение сбоя режутся до 2000 символов и проходят `one_line`, причина `release` — до 200.

## Self-Check: PASSED

- FOUND: `crates/runner/src/policy.rs`, `crates/runner/tests/policy.rs`
- FOUND коммиты `765717a`, `785ef02`, `f231193` в истории `main`
- `cargo test --release --workspace` зелёный
