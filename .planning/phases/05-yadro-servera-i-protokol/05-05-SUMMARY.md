---
phase: 05-yadro-servera-i-protokol
plan: 05
subsystem: server
tags: [rust, tiny_http, resumable-upload, range, sha256, epoch-fencing, sqlite]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol (план 04)
    provides: rules::decide с Finish/Cancel, Blobs::discard_parts, Redactor, tests/secrets.rs
provides:
  - HEAD/PUT /v1/jobs/{id}/result — загрузка результата кусками до 8 МиБ с докачкой (SRV-06)
  - finish `done` { size, sha256 } — результат становится highlights.mp4 только после совпадения размера и SHA-256 (SRV-03, SRV-06)
  - HEAD/GET /v1/jobs/{id}/demo — демка по Range: 206, Content-Range, настоящий Content-Length, ETag = SHA-256 (SRV-06)
  - rules: события Chunk и Touch (аренда от загрузки и скачивания)
  - Blobs: result_part/result_path/part_len/lock_upload/append_chunk/finalize_result/open_demo_range
  - tests/transfer_http.rs (15 тестов), стенд с сырыми соединениями
  - Key Decisions в PROJECT.md: tiny_http 0.12 и модель базы сервера
affects: [фаза 6 (раннер), фаза 7 (бот), фаза 9 (прокси и таймауты)]

estimate:
  tokens: 110000
  raw_tokens: 110000
  tasks: 3
  confidence: low
actuals:
  tokens: 24700
  tasks: 3
  commits: 3
plan_head_before: d7ccd2b5ee628e099fcc0050a3fd7eea2cdfa30d
plan_head_after: 985235afaa900a5042dc4dcda36972a41b80b184

tech-stack:
  added: []
  patterns:
    - "Все отказы PUT идут до чтения тела (нет 100 Continue отвергнутому запросу, тело не принимается зря)"
    - "Длина .part на диске — единственная правда о смещении; своя .part у каждой эпохи"
    - "Проверка файла (длина, SHA-256 потоком, rename) вне мьютекса базы, затем вторая транзакция с повторным decide и строкой files"
    - "Ответы с файлом и на HEAD — Response::new с настоящей длиной и with_chunked_threshold(usize::MAX)"

key-files:
  created:
    - crates/server/tests/transfer_http.rs
  modified:
    - crates/wire/src/lib.rs
    - crates/server/src/rules.rs
    - crates/server/src/blobs.rs
    - crates/server/src/core.rs
    - crates/server/src/api.rs
    - crates/server/tests/common/mod.rs
    - crates/server/tests/core_states.rs
    - crates/server/tests/secrets.rs
    - .planning/PROJECT.md

key-decisions:
  - "Event::Touch реализован как делегирование в Event::Reattach (то же ограждение, продление и возврат потерянной аренды по этапу), отдельно в таблице переходов"
  - "Chunk всегда ставит phase = upload: иначе возврат потерянной аренды по сохранённому этапу вернул бы running при уже идущей загрузке"
  - "Reject получил поле headers (Retry-After, Content-Range): код ошибки не несёт заголовков сам"
  - "Повтор finish done идемпотентен только если принятый файл тот же (размер и sha256 из files); иной описанный результат у done-задачи — job_terminal"
  - "Если вторая транзакция finish done отказала, готовый highlights.mp4 удаляется, пока задача не done: такой файл не должен остаться результатом"

patterns-established:
  - "Тест с сырым соединением: заголовки, часть тела, shutdown(Write), чтение статуса — обрыв проверяется условием 0 < off <= отправлено, без cfg по ОС"
  - "RawReply/read_reply в tests/common: чтение ответа по Content-Length на том же соединении (проверка, что отказ не ломает соединение)"

requirements-completed: [SRV-03, SRV-06, SEC-04]

duration: ~1 ч 30 мин
completed: 2026-10-11
---

# Phase 5 Plan 05: Передача файлов кусками — Summary

**Раннер загружает результат кусками по 8 МиБ (`HEAD` даёт смещение, `PUT` с `X-Epoch` и `Upload-Offset` дописывает `.part` своей эпохи), `finish done` сверяет размер и SHA-256 и только тогда делает файл `highlights.mp4`; демка отдаётся по `Range` с настоящим `Content-Length`; обрыв, чужое смещение, устаревшая эпоха и поздний кусок после отмены отвергаются, а поведение `tiny_http` (PUT по 8 МиБ, `Expect: 100-continue`, обрыв тела) закреплено тестами и записано в Key Decisions.**

## Accomplishments

- Tracer (Task 1): `HEAD` -> три `PUT` (8, 8, 1 МиБ) -> `finish done` -> файл побайтно равен, `.part` нет, строка `files kind = 'result'`, повтор `finish` — 200. Первый кусок переводит задачу в `uploading`, каждый кусок сдвигает аренду (часы стенда двигаются вручную). Гейт «трассёр проверен до расширения» пройден: `result_uploads_in_chunks_and_closes_the_job` зелёный до начала Task 2.
- `wire`: `Outcome::Done { size, sha256 }`, тест `done_outcome_round_trips` (итог без sha256 не разбирается, а не молча становится `Unknown`).
- `rules`: `Event::Chunk` (`assigned|running` -> `uploading`, потерянная аренда возвращается, аренда продлевается) и `Event::Touch` (продление без смены состояния); таблица переходов выросла до 25 x 9 пар.
- `blobs`: `UploadGuard` (общий `HashSet` пар `(задача, эпоха)` под мьютексом с восстановлением, снятие в `Drop`), `append_chunk` (буфер 64 КиБ, повторная сверка длины под охраной, принятые байты не теряются при EOF или ошибке чтения, `sync_data`), `finalize_result` (длина, SHA-256 потоком по 1 МиБ, неверный хеш удаляет `.part`, `rename` после закрытия файла).
- `core.finish_done`: пробный `decide` без записи -> проверка файла вне мьютекса базы под охраной -> вторая транзакция (повторный `decide`, `apply`, `INSERT files`). `SizeMismatch` -> 409 `offset_mismatch` с настоящим размером; `ShaMismatch` -> 422 `sha256_mismatch`; занятая загрузка -> 409 `upload_busy`.
- `api.rs`: `HEAD`/`PUT` результата, `HEAD`/`GET` демки. Порядок `PUT`: токен -> заголовки -> `Content-Length` (411/400/413) -> ограждение и аренда -> охрана (409 `upload_busy` + `Retry-After: 1`) -> смещение (409 с `Upload-Offset`) -> лимит результата (`checked_add`) -> только потом чтение тела. Обрыв: одна строка журнала «кусок задачи #N оборван: принято X из Y байт», ответ 400.
- `GET` демки: только `bytes=N-` и `bytes=N-M`, не больше 8 МиБ за ответ, 206 с `Content-Range`, `ETag` = SHA-256, 416 с `Content-Range: bytes */size`, `with_chunked_threshold(usize::MAX)`.
- `tests/transfer_http.rs` (15 тестов): передача кусками, обрыв и докачка, чужое смещение и живое соединение (на сыром сокете: после 409 тот же сокет отвечает на `HEAD`), 411/413/400, неверные размер и SHA-256, устаревшая эпоха (`1.part` не тронут, `2.part` свой), занятая загрузка, поздний кусок после отмены (в том числе после истечения аренды отменённой), Range (демка 20 МиБ: 8+8+4 МиБ, склейка совпала по SHA-256), 416, `Expect: 100-continue` (принятому — `100 Continue`, отвергнутому — сразу 409/413), ответ больше 32 КиБ с настоящим `Content-Length`.
- SEC-04: `chunk_rejections_do_not_leak_the_token` — токены раннера и бота в `Authorization` и в query отказанных кусков (слишком большой, оборванный, чужое смещение) не находятся в журнале, байтах `server.db`/`-wal` и ответах.

## Task Commits

1. **Task 1 (tracer): результат грузится кусками, итог done проверяется по размеру и SHA-256** — `cd1d40c` (feat)
2. **Task 2: оборванная загрузка продолжается, неверный файл не становится результатом** — `1d87bac` (feat)
3. **Task 3: демка по Range, tiny_http закреплён тестами, Key Decisions** — `985235a` (feat)

## Verification

- `cargo test --release -p server --test transfer_http` — 15 тестов (запущен шесть раз подряд вместе с `secrets`: без нестабильности); `--test secrets` — 6; `--test core_states` — 20; `--lib` — 43 (в том числе три новых теста `blobs`); `cargo test --release -p wire` — 7.
- `cargo test --release --workspace` зелёный (включая `crates/cli/tests/pipeline.rs`); `cargo build --release -p cli` собирается.
- Критерии: `as_reader` последним в `put_result` (после `lock_upload` и `part_len`); `grep -cE 'cfg\((unix|windows)\)' transfer_http.rs` = 0; `PROJECT.md` содержит `tiny_http` 0.12 и `BEGIN IMMEDIATE`.
- `cargo clippy -p server -p wire --all-targets`: только два замечания из 05-01 (`config.rs:262`, `tests/deps.rs:36`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `sync_all` на Windows падал с «Отказано в доступе»**
- **Found during:** Task 1 (первый прогон трассёра, `finish` -> 500)
- **Issue:** `finalize_result` открывал `.part` только на чтение и вызывал `sync_all`; `FlushFileBuffers` на Windows требует права записи.
- **Fix:** файл открывается через `OpenOptions::new().write(true)` (без усечения) перед `sync_all`.
- **Files modified:** `crates/server/src/blobs.rs`
- **Commit:** `cd1d40c`

**2. [Rule 2 - Missing critical functionality] Заголовки отказов (`Retry-After`, `Content-Range`)**
- **Found during:** Task 1, проектирование `upload_busy` и 416
- **Issue:** `Reject` не мог нести дополнительные заголовки, а план требует `Retry-After: 1` и `Content-Range: bytes */size`.
- **Fix:** поле `Reject.headers`, `respond_error` выставляет их в ответ.
- **Files modified:** `crates/server/src/core.rs`, `crates/server/src/api.rs`
- **Commit:** `cd1d40c`

### Уточнения плана

- `touch_upload` возвращает `Result<(), Reject>`, как в плане; признак «раннер вернул себе задачу» в журнал при загрузке не пишется (он пишется при `progress`).
- `Event::Touch` исполняется тем же кодом, что и `Event::Reattach`, но остаётся отдельным событием со своими строками в таблице.
- Для `Chunk` из состояния `assigned|running|queued+lost` ставится `phase = upload`, чтобы последующее `hello` или `Touch` при потерянной аренде возвращали `uploading`, а не `running`.
- В Task 3 добавлен тест `eight_mebibyte_put_and_a_response_over_32_kib_keep_their_content_length` (PUT ровно 8 МиБ и ответ 64 КиБ с настоящим `Content-Length`), а в `blobs` — три модульных теста; в плане их нет.
- Интеграция с `*.part` из 05-04: отдельная правка `sweep`/`recover` не потребовалась — они удаляют любые `*.part` (теперь `{epoch}.part`) окончательных задач; `touch_upload` дополнительно зовёт `discard_parts` при `job_terminal`.

## Known Stubs

None.

## Threat Flags

None — поверхность описана в `<threat_model>` плана. T-05-24 закрыт `size_or_sha_mismatch_is_not_a_result` и повторным `decide` во второй транзакции; T-05-25 — `stale_epoch_cannot_upload_or_finish` и `demo_needs_the_current_epoch`; T-05-26 — `concurrent_upload_of_the_same_epoch_is_busy`; T-05-27 — `chunk_without_length_or_too_big_is_refused`; T-05-28 (застрявший клиент) принят и записан в Key Decisions; T-05-29 — пути строятся из `job_id` и `epoch`.

## Issues Encountered

- Длинные heredoc с кириллицей и кавычками в Git Bash падали на разборе («unexpected EOF»): крупные фрагменты кода записывались инструментом Write во временный файл и добавлялись `cat`. Одна правка `perl -0pi` из-за преобразования аргументов обрезала `use std::fs::...` в `blobs.rs`; исправлено вручную до сборки.
- Принятое ограничение `tiny_http` (нет таймаутов сокета): застрявший клиент удерживает охрану загрузки до закрытия соединения; смягчается пулом + 503 и прокси впереди (фаза 9).

## Next Phase Readiness

Раннер фазы 6 может: скачать демку по `GET .../demo` с `Range`, сверить SHA-256 с `Assignment.demo.sha256`, загрузить `highlights.mp4` кусками (`HEAD` -> `PUT` с места обрыва) и закрыть задачу `finish` `{"kind":"done","size":N,"sha256":"..."}`. Контракт: `X-Epoch` обязателен на всех четырёх ручках; смещение берётся только из `Upload-Offset` ответа (`HEAD`, `409 offset_mismatch`, `204`); после 422 `sha256_mismatch` загрузка начинается с нуля.

## Self-Check: PASSED

- Файлы найдены: `crates/server/tests/transfer_http.rs`, `crates/server/src/blobs.rs`, `crates/server/src/api.rs`, `crates/server/src/core.rs`, `crates/server/src/rules.rs`, `crates/wire/src/lib.rs`, `.planning/PROJECT.md`.
- Коммиты `cd1d40c`, `1d87bac`, `985235a` — предки HEAD.
