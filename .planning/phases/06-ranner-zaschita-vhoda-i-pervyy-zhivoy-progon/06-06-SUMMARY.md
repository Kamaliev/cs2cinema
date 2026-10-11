---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 06
subsystem: runner
tags: [rust, outbox, atomic-write, resumable-upload, hello-reattach, tracer]
status: complete

requires:
  - phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
    provides: 06-05 — Link, Runner, Executor, сквозной путь claim → done на настоящем server::serve
  - phase: 05-yadro-servera-i-protokol
    provides: HEAD/PUT результата со смещением, hello с holding и указаниями keep/abandon/cancel, finish done с проверкой SHA-256
provides:
  - state.rs — runner_state.json (outbox): одна удерживаемая задача, атомарная запись, next_seq, откладывание испорченного файла
  - transfer.rs — докачка демки, загрузка результата со смещения сервера, судьба отказов (judge), sha256_file
  - перезапуск раннера: hello с holding, keep продолжает с этапа, cancel закрывает finish(Cancelled), abandon бросает
  - Config.upload_chunk_limit для учения «обрыв загрузки» (06-11)
affects: [06-08 пульс и отмена, 06-09 политика сбоев, 06-10 защищённый исполнитель и cs2-cli runner, 06-11 учения, фаза 8]

estimate:
  tokens: 80000
  raw_tokens: 80000
  tasks: 2
  confidence: low
actuals:
  tokens: 18700
  tasks: 2
  commits: 3
plan_head_before: d3defa0c0f1553dc21b53738ca5a1caf31039b73
plan_head_after: dff69195218252fd968e36fa9b758c4086b6cd9e

tech-stack:
  added: []
  patterns:
    - "Outbox: runner_state.json пишется до каждого шага (claim, демка, исполнитель, итог), очищается после ответа сервера; файл результата остаётся на месте"
    - "Атомарная запись: временный файл, sync_all, rename (на Windows rename заменяет существующий файл)"
    - "Окончательный отказ сервера (stale_epoch, job_terminal, 404, второй sha256_mismatch) = Dropped с причиной в журнале; всё остальное повторяется с паузой 1..30 с"
    - "Загрузка читает файл кусками по CHUNK_SIZE с seek, смещение берёт у сервера (HEAD, offset_mismatch), upload_busy ждёт Retry-After"

key-files:
  created:
    - crates/runner/src/state.rs
    - crates/runner/src/transfer.rs
    - crates/runner/tests/outbox.rs
  modified:
    - crates/runner/src/lib.rs
    - crates/runner/tests/common/mod.rs

key-decisions:
  - "TransferEnd стал обобщённым: TransferEnd<T = ()>, загрузка результата возвращает Done(JobState), чтобы Tick::Settled нёс состояние после finish; для демки Dropped означает демку, не сошедшуюся по SHA-256 дважды (закрывается BAD_DEMO), а Err остаётся для сбоя связи и диска"
  - "download_demo берёт &HeldJob вместо шести отдельных аргументов (clippy too_many_arguments), путь назначения = held.demo_path"
  - "Папка результата out/srv{id} записывается в состояние при первом запуске исполнителя, но сам первый запуск идёт с out_dir = None и resume = false; после перезапуска посреди записи исполнитель получает эту папку и resume = true"
  - "Tick получил варианты Stopped (флаг остановки или лимит кусков) и Dropped { job_id } (окончательный отказ сервера)"
  - "Указание cancel не бросает задачу молча: итог Cancelled сохраняется (этап Reporting) и уходит finish, файлы убираются после ответа; так сервер видит отменённую задачу отменённой, а сбой сети повторяется следующим tick"
  - "Версия формата runner_state.json, отличная от 1, откладывается в сторону так же, как нечитаемый файл"

requirements-completed: [RUN-02]

duration: 55min
completed: 2026-10-11
---

# Phase 6 Plan 06: результат не теряется, outbox раннера и докачка Summary

**Outbox раннера в `runner_state.json` с атомарной записью, докачка демки и загрузка результата со смещения сервера, сверка после перезапуска через `hello` (keep, resume, cancel, abandon) и чистка outbox при окончательном отказе сервера с причиной в `runner.log`.**

## Performance

- **Duration:** около 55 мин
- **Completed:** 2026-10-11
- **Tasks:** 2 (трассёр и TDD-задача)
- **Files modified:** 5

## Accomplishments

- Трассёр `interrupted_upload_resumes_after_restart_from_the_server_offset`: результат 20 МиБ, первый раннер останавливается после одного куска (сервер держит ровно 8 МиБ, в состоянии этап `Finished` с путём, размером и SHA-256), второй раннер в той же папке докачивает и закрывает задачу `done`, файл на сервере побайтно равен, исполнитель за оба запуска вызван один раз.
- Перезапуск после записи: настоящий `claim`, состояние `Finished` записано напрямую, новый раннер делает `hello` с `holding`, получает `keep`, загружает и закрывает задачу, исполнитель не вызывается.
- Перезапуск посреди записи (этап `Executing`): исполнитель идёт с `resume = true` и прежним `out_dir`, `attempt` растёт на 1 (D-01).
- Окончательные отказы чистят outbox (D-03): второй `422 sha256_mismatch` (после одной полной перезаливки с нуля), `stale_epoch`, `job_terminal`, `404` дают `Dropped`, файл результата и демка удалены, состояние пусто, в `runner.log` строка с причиной и `#id`. Удаляются только пути внутри папки раннера.
- Отмена на сервере (`core.cancel(id, true)`): `hello` отвечает `cancel`, раннер сообщает `finish(Cancelled)`, задача на сервере `cancelled`, файлы убраны.
- Сценарный HTTP-ответчик подтверждает порядок запросов `HEAD, PUT 0, PUT 40, PUT 40, POST finish`: после `offset_mismatch` с `offset=40` загрузка идёт с 40 (тело 60 байт), после `upload_busy` с `Retry-After: 1` повтор того же куска не раньше чем через секунду.
- Испорченный `runner_state.json` (включая невалидный UTF-8) переименовывается в `runner_state.bad-{мс}.json`, раннер стартует пустым и пишет предупреждение в журнал.
- Пока в состоянии есть задача, `claim` не вызывается: вторая задача остаётся `queued` до конца первой.
- `RunnerState::next_seq` = `max(last_seq + 1, unix-мс)`, потолок `i64::MAX`, значение хранится в файле; пульс подключит 06-08.
- Файл результата читается кусками по `CHUNK_SIZE` с `seek`, SHA-256 демки считается потоком при записи (докачка хеширует уже скачанное начало); весь файл в память не грузится.

## Task Commits

1. **Task 1: оборванная загрузка докачивается после перезапуска (трассёр)** - `2c7c72b` (feat)
2. **Task 2: resume после записи, отказы сервера, испорченное состояние (TDD)**
   - RED - `17236f6` (test): падали три теста — `resume` исполнителя, отмена на сервере через `finish`, откладывание испорченного состояния
   - GREEN - `dff6919` (feat): `resume` в `execute`, указание `cancel` через `Reporting`, `RunnerState::load` откладывает файл

**Plan metadata:** коммит `docs(06-06)` после этого файла.

Трассёр проверен после коммита: `cargo test --release -p runner --test outbox` и `--lib state` зелёные, `flow` не сломан. Тесты `second_sha256_mismatch...`, `no_claim_while_a_job_is_held` и `upload_follows_the_server_offset...` проходили уже в RED: их поведение дал код Task 1 (откат с `Dropped`, цикл по этапам, смещения в `transfer`), RED-тесты фиксируют его как контракт.

## Files Created/Modified

- `crates/runner/src/state.rs` - `RunnerState`, `HeldJob`, `Stage`, `ResultFile`, `load`/`save`/`next_seq`; три модульных теста
- `crates/runner/src/transfer.rs` - `download_demo`, `upload_result`, `TransferEnd`, `sha256_file`, `judge`/`Verdict`
- `crates/runner/src/lib.rs` - цикл по этапам (`drive`, `fetch`, `execute`, `upload`, `report`), `hello` с `holding` и указаниями, `drop_job`, `Config.upload_chunk_limit`, `Tick::{Stopped, Dropped}`
- `crates/runner/tests/outbox.rs` - восемь тестов на настоящем `server::serve` и сценарный HTTP-ответчик
- `crates/runner/tests/common/mod.rs` - `upload_chunk_limit: None` в конфиге стенда, `Stand::cancel`

## Decisions Made

См. `key-decisions` выше.

## Deviations from Plan

### Вынужденные отличия от текста плана

**1. [Rule 3 - Blocking] Форма `TransferEnd` и аргументов `download_demo`**
- **Issue:** план объявляет `TransferEnd { Done, Stopped, Dropped(String) }`, но цикл должен получить из `upload_result` состояние задачи после `finish` (для `Tick::Settled`), а у `download_demo` семь аргументов упёрлись бы в `clippy::too_many_arguments` проекта.
- **Fix:** `TransferEnd<T = ()>`; `download_demo(link, &HeldJob, stop, log)`; `upload_result(link, &mut HeldJob, save, stop, chunk_limit, log)`. Для демки второе несовпадение SHA-256 возвращается как `Dropped` (а не `Err`), чтобы отличить повреждённую демку (закрывается `BAD_DEMO`) от сбоя сети.
- **Files modified:** `crates/runner/src/transfer.rs`, `crates/runner/src/lib.rs`
- **Committed in:** `2c7c72b`

**2. [Rule 2 - Missing critical] Папка результата попадает в состояние до записи**
- **Issue:** по плану `resume` = «`out_dir` известен», но `out_dir` ранее узнавался только из итога исполнителя; без этого перезапуск посреди записи никогда не получил бы `resume`.
- **Fix:** при первом запуске исполнителя в состояние пишется `out/srv{id}`, сам первый запуск идёт с `out_dir = None`, `resume = false`; фактическая папка итога заменяет эту.
- **Committed in:** `dff6919`

**3. [Rule 2 - Missing critical] Недостающая демка при перезапуске**
- **Issue:** при `Fetched`/`Executing` демку могли удалить.
- **Fix:** файла нет — этап возвращается в `Claimed`, демка скачивается заново.
- **Committed in:** `dff6919`

---

**Total deviations:** 3 (Rule 2 x2, Rule 3 x1). **Impact on plan:** поведение, названное в плане, сохранено; изменены сигнатуры и добавлена защита.

## Issues Encountered

- Python в окружении не установлен (заглушка Microsoft Store): правки файлов сделаны через `Edit`/`sed`.
- Heredoc с литералами `\x..` ломал разбор оболочки; тест испорченного файла собирает байты через `[0xff, 0xfe]`.

## Known Stubs

None.

## Threat Flags

None. Новых сетевых поверхностей нет: все запросы уже перечислены в модели угроз 06-05; T-06-15 (эпоха и `stale_epoch`/`job_terminal` → `Dropped`), T-06-16 (испорченное состояние откладывается) и T-06-17 (каждый сброс outbox пишет причину и номер задачи в `runner.log`) закрыты тестами `job_cancelled_on_the_server_drops_the_result_from_the_outbox`, `corrupted_state_file_is_set_aside_and_the_runner_starts_empty`, `second_sha256_mismatch_drops_the_result_from_the_outbox`.

## Deferred Items

- Пульс `progress` и использование `next_seq` — 06-08; во время работы исполнителя аренда сейчас не продлевается.
- Ответ на `Cancel`/`Soft` в середине работы исполнителя — 06-08; здесь отмена обрабатывается только при сверке после перезапуска.
- Политика сбоев (release, повторы при потере связи посреди скачивания демки) — 06-09: `tick` по-прежнему возвращает `Err` при сбое сети в `fetch`, но состояние сохранено, и следующий `tick` докачивает с `.part`.
- Предупреждение `clippy::result_large_err` на `Executor::run` осталось с 06-05 (размер `pipeline::RunError`), к этому плану не относится.

## Next Phase Readiness

Outbox и сверка после перезапуска готовы к пульсу (06-08), политике сбоев (06-09) и настоящему исполнителю с `recover` и `resume` (06-10); учение «обрыв загрузки» (06-11) использует `Config.upload_chunk_limit`.

## Self-Check: PASSED

- FOUND: crates/runner/src/state.rs, crates/runner/src/transfer.rs, crates/runner/tests/outbox.rs
- FOUND commits: 2c7c72b, 17236f6, dff6919
- `cargo test --release --workspace` зелёный, `cargo build --release -p cli` собирается
