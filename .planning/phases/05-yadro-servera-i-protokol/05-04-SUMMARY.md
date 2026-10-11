---
phase: 05-yadro-servera-i-protokol
plan: 04
subsystem: server
tags: [rust, finish, cancel, state-machine, redactor, sec-04, sqlite]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol (план 03)
    provides: rules::decide, аренда и эпоха, hello, recover, Redactor, wire
provides:
  - POST /v1/jobs/{id}/finish — итог раннера failed / cancelled; поля сбоя очищены Redactor и ограничены по длине (SRV-03)
  - wire: Failure, Outcome (failed / cancelled / unknown), FinishRequest, FinishReply
  - rules::decide — полная таблица: Finish, Cancel, Received, ReceiveFailed; окончательные состояния неизменны, повтор того же итога идемпотентен
  - Core::finish / finish_report / cancel / sanitize_failure; finish_receive идёт через rules
  - cs2-server job cancel {id} [--force]; флаг cancel_requested доходит до раннера в ответе на прогресс и в hello
  - Blobs::discard_parts / results_dir; recover и sweep убирают куски *.part окончательных и отменённых задач
  - Redactor узнаёт Bearer и токен бота Telegram по форме (SEC-04)
  - tests/secrets.rs — подставные токены не находятся в журнале, байтах server.db/-wal и ответах API
affects: [05-05, фаза 6, фаза 7]

estimate:
  tokens: 105000
  raw_tokens: 105000
  tasks: 3
  confidence: low
actuals:
  tokens: 22000
  tasks: 3
  commits: 3
plan_head_before: c2bf400a3ce14200327454a724a6980b9969ae59
plan_head_after: af012d1ee76b18d54fce678259b69cf37d6a41ef

tech-stack:
  added: []
  patterns:
    - "Итог раннера и отмена оператора — события чистого автомата: Core грузит строку, зовёт decide, пишет одной транзакцией"
    - "Тест SEC-04 сначала доказывает, что подставной токен был во входе (Tap.sent), и только потом ищет его на выходе"
    - "assert_absent называет место утечки, но не печатает соседний текст; сама проверка проверена на буфере с токеном"

key-files:
  created:
    - crates/server/tests/secrets.rs
  modified:
    - crates/wire/src/lib.rs
    - crates/server/src/rules.rs
    - crates/server/src/core.rs
    - crates/server/src/api.rs
    - crates/server/src/blobs.rs
    - crates/server/src/cli.rs
    - crates/server/src/secret.rs
    - crates/server/tests/common/mod.rs
    - crates/server/tests/core_states.rs
    - crates/server/tests/lease_http.rs
    - crates/server/tests/operator.rs

key-decisions:
  - "Итог `done` пока не принимается по HTTP: Outcome читает его как Unknown (400 «неизвестный итог задачи»); rules умеет FinishKind::Done, а загрузка и проверка файла — план 05-05"
  - "finish на окончательной задаче: тот же итог тем же раннером с той же эпохой — 200 без изменений, иначе job_terminal (в том числе для чужого раннера); stale_epoch — только для не окончательных"
  - "Core::finish_report рядом с finish (как claim_report): ошибка удаления кусков не отменяет записанный итог и уходит в журнал предупреждением"
  - "claim освобождает прежнюю задачу этого же раннера с потерянной арендой (runner_id = NULL, lease_lost = 0): раннер, просящий новую задачу, свободен, иначе поздний прогресс вернул бы ему вторую"
  - "В журнал строки finish идёт код сбоя из базы (уже очищенный), а не текст раннера"

patterns-established:
  - "Строки журнала: «задача #{id}: {состояние} ({код сбоя или —}), раннер {имя}»"
  - "Ожидание в таблице переходов: `=`, `S:{состояние}`, `C:{флаг}`, `!{код отказа}`; 21 событие × 9 состояний"

requirements-completed: [SRV-03, SEC-04]

duration: ~1 ч 20 мин
completed: 2026-10-11
---

# Phase 5 Plan 04: Итог задачи и чистые журналы — Summary

**Раннер сообщает сбой или отмену по `POST /v1/jobs/{id}/finish`, оператор отменяет задачу командой `cs2-server job cancel` (флаг доходит до раннера в ответе на прогресс и в `hello`), все переходы автомата проверены таблицей 21 × 9 и случайными последовательностями на виртуальных часах, а подставные токены раннера и бота после принудительных сбоев не находятся ни в журнале, ни в байтах `server.db`, ни в ответах API.**

## Accomplishments

- Tracer (Task 1): раннер присылает сбой с собственным токеном в `message` и `tail` — задача `failed`, `fail_message` содержит `boom` и `cs2r_***`, `fail_elapsed_ms == 1234`, аренда снята; повтор того же итога — 200, `cancelled` после `failed` — 409 `job_terminal`. Токен был в отправленном запросе и не найден в журнале, байтах `server.db`/`-wal` (до и после остановки) и ответах. Гейт «трассёр проверен до расширения» пройден: обе проверки Task 1 зелёные до начала Task 2.
- `sanitize_failure`: код сбоя `[A-Z0-9_]{1,64}` или `UNKNOWN`; класс 32, состояние 64, подсказка 1000, сообщение 2000, хвост 16 384 символа; управляющие символы кроме `\n`, `\t` удаляются, секреты маскируются.
- `rules::decide` — полная таблица: `Finish` (Failed / Cancelled / Done), `Cancel`, `Received`, `ReceiveFailed`. `Done` разрешён только из `uploading` или из `queued` с потерянной арендой, из `assigned`/`running` — `offset_mismatch` «результат ещё не загружен».
- Отмена: `receiving`/`queued` — сразу; выданной ставится флаг `soft`, повторная отмена или `--force` — `force`; раннер видит флаг в `ProgressReply.cancel` и в директиве `cancel` ответа на `hello`; выданная отменённая задача с истёкшей арендой после `sweep` становится `cancelled`, а не `queued`.
- Тесты автомата на виртуальных часах без HTTP: `every_state_and_event_pair_matches_the_transition_table` (189 пар), `the_happy_path_walks_receiving_to_done_on_virtual_time` (встречаются все 8 состояний `JobState::ALL`), `random_sequences_keep_the_invariants` (пять зёрен по 600 шагов, три задачи, два раннера, `max_grants = 3`, финал — всё окончательное), липкость и идемпотентность итогов, отмена, `recover` и куски результата.
- SEC-04: `Redactor` узнаёт `Bearer {значение}` (регистр не важен) и токен бота Telegram по форме (5+ цифр, `:`, 30+ символов), плюс точные значения и `cs2r_`; не трогает время, адрес с портом, sha256, обычный текст; повторное маскирование ничего не меняет.
- `tests/secrets.rs` (5 тестов): принудительные сбои (чужой, отозванный и кривой токен, `Basic`, неверный JSON с токеном, устаревшая эпоха, токены в `status`, `hello`, `release`, сообщении, подсказке, состоянии и хвосте сбоя, имени файла демки, игроке и заказчике), паника обработчика с токенами, ошибки настроек и `config show` через бинарь, проверка самой проверки.

## Task Commits

1. **Task 1 (tracer): сбой раннера с токеном в тексте — задача `failed`, токена нигде нет** — `346fb7d` (feat)
2. **Task 2: отмена доходит до раннера, таблица переходов и случайные последовательности** — `5ba0416` (feat)
3. **Task 3: подставные токены раннера и бота не находятся нигде** — `af012d1` (feat)

## Verification

- `cargo test --release -p server --test secrets` — 5 тестов, зелёные (`runner_token_in_a_failure_report_stays_out_of_logs_db_and_responses`, `forced_failures_leave_no_token_anywhere`, `panic_message_with_a_token_is_redacted`, `bot_token_never_leaves_config_errors_or_config_show`, `the_check_itself_catches_a_leak`).
- `cargo test --release -p server --test core_states` — 20 тестов; `--test lease_http` — 14; `--test operator` — 13; `--lib` — 40 (в том числе `failure_fields_are_cleaned_and_capped`, `bearer_and_telegram_bot_tokens_are_masked_by_shape`, `ordinary_text_times_and_hashes_survive_redaction`, `redaction_is_idempotent`); `cargo test --release -p wire` — 6.
- `cargo test --release --workspace` зелёный (включая `crates/cli/tests/pipeline.rs`); `cargo build --release -p cli` собирается.
- Проверка, что тест SEC-04 не холостой: временно отключил `Redactor::redact` — три теста утечек падают с сообщением «подставной секрет найден: токен раннера: журнал сервера» без печати соседнего текста; без отключения проходят.
- `cargo clippy -p server -p wire --all-targets`: только два замечания из 05-01 (`config.rs:262`, `tests/deps.rs:36`), вне области плана.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Раннер мог вести две задачи сразу**
- **Found during:** Task 2, проектирование случайных последовательностей (инвариант «у раннера не больше одной выданной задачи»)
- **Issue:** `claim` не освобождал прежнюю задачу этого же раннера с потерянной арендой (`queued` + `lease_lost`). Если раннер брал новую задачу, а потом присылал прогресс или `hello` по старой, она возвращалась ему, и у раннера оказывались две выданные задачи.
- **Fix:** при выдаче новой задачи `claim_report` обнуляет `runner_id` и `lease_lost` у остальных его потерянных задач: раннер, просящий работу, свободен. Прогресс по старой — `stale_epoch`, её берёт любой раннер.
- **Test:** `runner_that_claims_again_drops_its_own_lost_lease` (падает без исправления, проверено). Сами случайные последовательности этот случай не достигают (claim всегда берёт самую старую задачу в очереди, в том числе собственную потерянную), поэтому нужен детерминированный тест.
- **Files modified:** `crates/server/src/core.rs`, `crates/server/tests/core_states.rs`
- **Commit:** `5ba0416`

**2. [Rule 2 - Missing critical functionality] Куски результата отменённых задач не убирались**
- **Found during:** Task 2
- **Issue:** `Decision.discard_parts` при истечении аренды отменённой задачи вычислялся в 05-03, но `sweep` его не применял.
- **Fix:** `sweep` вызывает `discard_parts` для отменённых по сроку задач; `recover` убирает `*.part` всех окончательных задач (поле `Recovered.parts_discarded`).
- **Commit:** `5ba0416`

### Уточнения плана

- `Core::finish` оставлен с сигнатурой из плана; ошибка удаления кусков (план: «в `Reject.detail`») возвращается из `finish_report` полем `Finished.discard_error` (как `claim` / `claim_report`), `api.rs` пишет её в журнал предупреждением, ответ 200.
- Для `finish` на окончательной задаче чужой раннер получает `job_terminal`, а не `stale_epoch` (так в тексте плана: «иначе `job_terminal`; иначе `stale_epoch`» относится к не окончательным).
- `cli`: кроме `job cancel`, `job show` печатает «отмена запрошена: soft|force»; добавлен тест бинарника `job_cancel_is_immediate_for_a_queued_job_and_a_flag_for_a_leased_one` в `operator.rs`.
- В таблицу вынесены два дополнительных события (`receive_stale` зависшего и свежего приёма, `expire` просроченной и непросроченной аренды), поэтому строк 21, а не 18: пар 21 × 9 = 189.

### Найденные утечки

Принудительные сбои не нашли ни одного пути, где подставной токен доходит до журнала, базы или ответов: исправлений в `api.rs` и `core.rs` для SEC-04 не потребовалось, кроме сканеров `Bearer` и токена бота в `Redactor`.

## Known Stubs

None. Итог `done` по HTTP намеренно не принимается: он описан в `FinishKind::Done` и таблице переходов, а загрузка результата и проверка файла — план 05-05.

## Threat Flags

None - поверхность описана в `<threat_model>` плана. T-05-19 закрыт `runner_token_in_a_failure_report_stays_out_of_logs_db_and_responses` и `forced_failures_leave_no_token_anywhere` (байты `server.db`, `-wal`, `-shm`), T-05-20 — `panic_message_with_a_token_is_redacted`, T-05-21 — сбор тел и заголовков всех ответов в `forced_failures_leave_no_token_anywhere`, T-05-22 — проверка присутствия во входе и `the_check_itself_catches_a_leak`, T-05-23 — `failed_and_cancelled_outcomes_are_terminal_and_sticky` и `stale_runner_cannot_finish`.

## Issues Encountered

- Python в окружении нет (запускается заглушка Microsoft Store); пакетные правки сделаны `sed`/`perl` и инструментами редактирования.
- Предупреждения git «LF will be replaced by CRLF» при коммитах (`core.autocrlf=true`) ожидаемы и ничего не ломают.

## Next Phase Readiness

05-05 добавляет передачу демки и результата кусками с SHA-256: итог `done` появится в `Outcome` вместе с проверкой файла (`FinishKind::Done` и таблица переходов готовы), а `Blobs::results_dir` и `discard_parts` уже определяют папку `results/{id}` и уборку недозагруженных кусков. Контракт раннера для фаз 6 и 7: `Failure { code, class, state, hint, message, tail, elapsed_ms }` и `Outcome` с тегом `kind`; незнакомый итог читается как `Unknown`.

## Self-Check: PASSED

- Файлы найдены: `crates/server/tests/secrets.rs`, `crates/server/src/rules.rs`, `crates/server/src/core.rs`, `crates/server/src/secret.rs`, `crates/wire/src/lib.rs`.
- Коммиты `346fb7d`, `5ba0416`, `af012d1` — предки HEAD.
