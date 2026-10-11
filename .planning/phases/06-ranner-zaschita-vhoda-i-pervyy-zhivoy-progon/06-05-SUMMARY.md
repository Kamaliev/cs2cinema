---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 05
subsystem: runner
tags: [rust, ureq, sha256, range, chunked-upload, executor-seam, tracer]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol
    provides: HEAD/GET демки по Range, HEAD/PUT результата кусками, finish done с размером и SHA-256, server::serve
provides:
  - крейт runner: Link (клиент протокола на ureq), Token, LinkError, Executor/Task, Runner (tick/run), runner.log
  - сквозной путь «claim → демка кусками → исполнитель → результат кусками → finish» на настоящем server::serve (RUN-01)
  - отображение итога исполнителя: сбой с кодом, JOB_ERROR, отмена
  - охрана server/tests/deps.rs знает крейт runner
affects: [06-06 outbox и докачка, 06-08 пульс и отмена, 06-09 политика сбоев, 06-10 защищённый исполнитель и cs2-cli runner, фаза 8]

estimate:
  tokens: 75000
  raw_tokens: 75000
  tasks: 2
  confidence: low
actuals:
  tokens: 12300
  tasks: 2
  commits: 3
plan_head_before: da3b4bd9d328521109bfc3c6cc050d422d8d9547
plan_head_after: 662d692ef940cc16ad6495cb05dc7e16628057e6

tech-stack:
  added: []
  patterns:
    - "Раннер только исходящий: Link шлёт запросы, сервер никогда не подключается к раннеру"
    - "Срок ответа задаётся на запрос (claim: wait+15 с, PUT 180 с, GET 120 с, прочее 30 с), у агента общего таймаута нет"
    - "Путь демки строится только из номера задачи: remote/srv{job_id}.dem; orig_name в путях не участвует"
    - "Токен: Debug = ***, нет Display, только в Authorization, редиректы выключены, значение вычёркивается из runner.log и ошибок сети"
    - "Исполнитель за трейтом Executor; в тестах ScriptedExecutor с очередью сценариев"

key-files:
  created:
    - crates/runner/Cargo.toml
    - crates/runner/src/lib.rs
    - crates/runner/src/link.rs
    - crates/runner/src/exec.rs
    - crates/runner/tests/common/mod.rs
    - crates/runner/tests/flow.rs
  modified:
    - Cargo.toml
    - Cargo.lock
    - crates/server/tests/deps.rs

key-decisions:
  - "JobSpec.job_id раннера = номер задачи на сервере, attempt = 1 в этом плане (надёжный счётчик — 06-06)"
  - "Коды JOB_ERROR и BAD_DEMO пока локальные константы runner с теми же значениями, что wire::fail; 06-09 заменит их"
  - "Скачивание демки и загрузка результата живут в lib.rs; 06-06 вынесет их в transfer.rs с докачкой"
  - "Ответ 3xx не ошибка ureq (max_redirects(0) + max_redirects_will_error(false)), а LinkError::Status: токен не уходит на чужой адрес"

requirements-completed: [RUN-01]

duration: 25min
completed: 2026-10-11
---

# Phase 6 Plan 05: раннер доводит задачу от выдачи до done Summary

**Крейт `runner`: исходящий клиент протокола фазы 5 с long-poll `claim`, скачиванием демки кусками по `Range` с проверкой SHA-256, швом `Executor` и загрузкой результата кусками по 8 МиБ; сервер принимает результат как `done`, сбой и отмена исполнителя доходят своими итогами.**

## Performance

- **Duration:** около 25 мин
- **Completed:** 2026-10-11
- **Tasks:** 2 (трассёр и TDD-задача)
- **Files modified:** 9

## Accomplishments

- `Link` покрывает весь протокол раннера: `hello`, `claim`, `progress`, `release`, `finish`, `demo_head`/`demo_range`, `result_head`/`result_put`; `LinkError::is_transient()` отделяет сеть, 5xx, `overloaded` и `upload_busy`.
- `Runner::tick` на первом вызове здоровается (426 превращается в «версия протокола сервера не совпадает с раннером»), затем `claim`; пустая очередь даёт `Tick::Idle`, выданная задача доходит до `Tick::Settled` с состоянием, которое назвал сервер.
- Демка 9 МиБ приходит двумя `GET Range`, хешируется потоком при записи, сверяется по размеру и SHA-256 и переименовывается в `remote/srv{id}.dem`; несовпадение закрывает задачу `Failed` с `BAD_DEMO` и словами «демка повреждена при передаче».
- Результат 17 МиБ читается файлом по 8 МиБ, уходит тремя `PUT` с смещения из `HEAD`, `finish done` несёт размер и SHA-256; рассинхрон смещения (`offset_mismatch`) лечится переходом на смещение сервера, не больше пяти раз.
- Итог исполнителя: `RunError` с `RecordFailure` уходит как `Failed` с кодом, классом (`classify(..).title()`), состоянием автомата, подсказкой и хвостом консоли через `one_line`; ошибка без описания — `JOB_ERROR`; `pipeline::CANCELLED` или код `Cancelled` — `Cancelled`.
- Шесть сквозных тестов на настоящем `server::serve` без CS2 и три модульных теста; `cargo test --release --workspace` зелёный, `cs2-cli` собирается; `cargo tree -p server -e normal` не содержит `runner`.

## Task Commits

1. **Task 1: claim → демка → исполнитель → результат → done (трассёр)** - `957e788` (feat)
2. **Task 2: сбой, ошибка без кода, отмена, Idle, журнал без токена**
   - RED - `e019893` (test): два теста падали на `JOB_ERROR` вместо кода и на `Failed` вместо `Cancelled`
   - GREEN - `662d692` (feat): `outcome_of` в `lib.rs`

**Plan metadata:** коммит `docs(06-05)` после этого файла.

## Files Created/Modified

- `crates/runner/src/link.rs` - `Token`, `Link`, `LinkError`; модульные тесты `token_debug_hides_the_secret`, `transient_errors_are_network_5xx_overloaded_and_busy`, `server_address_must_be_http`
- `crates/runner/src/exec.rs` - `Task`, `Executor` (`run`, `recover` по умолчанию пустой)
- `crates/runner/src/lib.rs` - `Config` (ручной `Debug` без токена), `Runner`, `Tick`, `runner.log`, скачивание и загрузка, `outcome_of`
- `crates/runner/tests/common/mod.rs` - стенд `Stand`, `ScriptedExecutor` со сценариями `Result`, `Fail`, `Plain`, `Cancel`
- `crates/runner/tests/flow.rs` - шесть сквозных тестов
- `crates/server/tests/deps.rs` - `runner` в `WORKSPACE_CRATES`
- `Cargo.toml`, `Cargo.lock` - член воркспейса `crates/runner`

## Decisions Made

См. `key-decisions` выше. Дополнительно: адрес сервера без `http://` или `https://` отвергается при создании `Link`, чтобы опечатка не превращалась в запрос на непредсказуемую схему; TLS идёт через системное хранилище сертификатов, как в `faceit`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Имя раннера в тестовом стенде**
- **Found during:** Task 1
- **Issue:** `auth::add_runner` принимает только латиницу, цифры, `_` и `-`; русское имя из стенда отвергалось.
- **Fix:** имя раннера в стенде и конфиге — `tester`.
- **Files modified:** `crates/runner/tests/common/mod.rs`
- **Committed in:** `957e788`

---

**Total deviations:** 1 auto-fixed (Rule 3). **Impact on plan:** только тестовое имя, поведение не затронуто.

Замечание по порядку: план требует повторяемого `Err` исполнителя уже в трассёре, поэтому в Task 1 любая ошибка исполнителя уходила как `JOB_ERROR`, а отображение кода и отмена появились в RED/GREEN Task 2 (тесты `Idle`, `JOB_ERROR` и журнала проходили уже в RED, так как их поведение дал трассёр).

## Issues Encountered

Одна команда `bash` повисла на `cat > /tmp/dummy` без stdin; ничего в репозитории она не изменила, тесты были записаны инструментом `Write`.

## Known Stubs

None. Исполнитель в тестах - заглушка по плану; настоящий `run_job` подключит 06-10.

## Threat Flags

None. Новая сетевая поверхность - только исходящие запросы раннера, все они в модели угроз плана (T-06-12, T-06-13, T-06-14).

## Deferred Items

Намеренно не сделано здесь, это планы 06-06 - 06-09:
- при сбое связи посреди скачивания или загрузки `tick` возвращает `Err`, задача остаётся выданной до истечения аренды (release и политика повторов - 06-09);
- нет пульса `progress` во время работы исполнителя и ответа на запрошенную отмену (06-08);
- нет `runner_state.json` и докачки после перезапуска (06-06); `attempt` всегда 1; директивы `hello.directives` не разбираются, `holding` пуст.

## Next Phase Readiness

Транспорт и порядок шагов проверены на настоящем сервере; следующие планы приращивают outbox и докачку (06-06), пульс (06-08), политику сбоев (06-09) и настоящий исполнитель с `cs2-cli runner` (06-10) к готовым `Link`, `Runner` и `Executor`.

## Self-Check: PASSED

- FOUND: crates/runner/src/{lib,link,exec}.rs, crates/runner/tests/{flow.rs,common/mod.rs}
- FOUND commits: 957e788, e019893, 662d692
