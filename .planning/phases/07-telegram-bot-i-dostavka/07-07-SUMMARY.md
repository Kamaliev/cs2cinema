---
phase: 07-telegram-bot-i-dostavka
plan: 07
subsystem: server+runner
tags: [telegram, bot, faceit, runner, ingest, failure-codes]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 03
    provides: wire::faceit (room_match_id, canonical_url), Core::submit_faceit, DedupKey::Faceit, wire::fail::FACEIT_FETCH_FAILED
  - phase: 07-telegram-bot-i-dostavka
    plan: 06
    provides: ingest::{precheck, accept, ingest_once, admit}, лимиты D-03, узнавание повторов
provides:
  - "bot::ingest: IntakeKind::Faceit; текст со ссылкой разбирается строго (первое слово - ссылка, остаток - ник), задача ставится через Core::submit_faceit без getFile"
  - "runner::PipelineExecutor: скачивание демки FACEIT по match_id ключом автора перед inspect_demo, шов with_fetch_fn"
  - "pipeline::FailCode::FaceitFetchFailed (FACEIT_FETCH_FAILED, класс Other), msg_faceit_fetch_failed, ALL: [FailCode; 22]"
  - "cs2-cli runner: ключ FACEIT только из переменной FACEIT_API_KEY"
affects: [07-08, 07-09, 07-11]

actuals:
  tokens: 15500
  tasks: 3
  commits: 3
plan_head_before: 531ecd7f6630826b7f621e0e0980b2c116e005d5
plan_head_after: 2af79e03db343e30dccc648416f4704d6305da67
commits: 3

tech-stack:
  added: []
  patterns:
    - "сервер хранит только канонический URL и не ходит по ссылкам; раннер перепроверяет ту же wire::faceit и передаёт в faceit::Client только match_id"
    - "наружу из раннера идут фиксированные фразы по классу ошибки; исходный текст ошибки клиента (подписанный адрес CDN) только в локальный журнал, без ключа и адресов"
    - "скачивание во временную папку srv{id}.faceit, затем переименование в srv{id}.dem; папка убирается при любом исходе и при уборке задачи"

key-files:
  created:
    - crates/runner/tests/faceit_flow.rs
    - crates/server/tests/bot_faceit.rs
  modified:
    - crates/server/src/bot/ingest.rs
    - crates/pipeline/src/record/failure.rs
    - crates/runner/Cargo.toml
    - crates/runner/src/exec.rs
    - crates/runner/src/lib.rs
    - crates/runner/tests/exec.rs
    - crates/cli/src/runner_cmd.rs
    - Cargo.lock

key-decisions:
  - "Голый id матча (без faceit и http) тоже считается попыткой прислать ссылку и получает объяснение с примером, а не общую подсказку: так требует тест Task 2, а <objective> говорил только про faceit/http"
  - "Task.demo теперь всегда remote/srv{id}.dem; ссылка FACEIT без файла на сервере по-прежнему не качается с сервера (needs_download), а демку кладёт в этот путь исполнитель"
  - "Пустой путь демки у задачи FACEIT остаётся UNSUPPORTED_SOURCE (скачивать некуда); проверка id идёт после неё, строгим разбором wire::faceit"
  - "ensure_faceit_demo вызывается и перед повторным inspect_demo после RETRY_NO_DATA при продолжении: иначе при удалённой демке откат к записи с нуля падал бы на отсутствующем файле"
  - "Журнал раннера получает исходный текст ошибки скачивания только после one_line, замены ключа на *** и замены адресов на <адрес>: tee не пересылает p.log на сервер, но локальный журнал тоже не должен нести подписанные ссылки"
  - "BOT-05 отмечен выполненным: ссылка принимается ботом, демку качает раннер ключом автора, ролик приходит в чат (вертикальный тест), сбои объяснены"

requirements-completed: [BOT-05]
---

# Phase 7 Plan 07: ссылки FACEIT от друга Summary

Друг присылает боту ссылку на комнату матча FACEIT (с ником после неё или с сохранённым игроком), сервер ставит задачу с канонической ссылкой без `getFile`, раннер скачивает демку своим ключом по `match_id`, записывает ролик, и бот присылает его в чат; ключ остаётся у автора.

## Accomplishments

- **Бот (`ingest.rs`).** Текст без документа разбирается так: нет `faceit`, `http` и голого id матча - прежняя подсказка (теперь она упоминает ссылку FACEIT); иначе первое слово проверяется `wire::faceit::room_match_id`. Не прошла - ответ «Это не ссылка на комнату матча FACEIT. Пример: ...» без повтора присланной строки. Прошла - игрок из остатка текста или сохранённый (D-02), иначе просьба `/player`; повтор `(ссылка, игрок)` через `Core::find_duplicate(DedupKey::Faceit)` отвечает «уже в очереди: задача #N» или «уже готово: /resend N». `accept` пишет строку `tg_inbox` kind `faceit` под теми же лимитами D-03 (`admit`), `ingest_once` ставит задачу `Core::submit_faceit` без `getFile`, дальше `queue_row` как у файла. Восстановление после падения между `submit_faceit` и `queued` - `own_faceit_job`; две строки подряд для той же пары дают одну задачу (`ingest_faceit` проверяет повтор ещё раз).
- **`FailCode::FaceitFetchFailed`.** Код `FACEIT_FETCH_FAILED`, класс `Other` (без автоповтора: друг нажимает `/retry`, когда матч доиграют), подсказка про доигранный матч и файл демки, `ALL: [FailCode; 22]` (страж `FailCode::ALL` в `wire::fail::KNOWN` зелёный). `UNSUPPORTED_SOURCE` теперь значит «ссылка не похожа на комнату матча FACEIT».
- **Раннер (`exec.rs`, `lib.rs`).** `runner` получил path-зависимость `faceit` (`server` по-прежнему без неё, `deps.rs` зелёный). `ensure_faceit_demo`: если у задачи есть ссылка и файла демки нет - этап `Download`, строгая перепроверка ссылки (`UNSUPPORTED_SOURCE`), пустой ключ (`FACEIT_FETCH_FAILED` «у раннера не задан ключ FACEIT (переменная FACEIT_API_KEY)»), скачивание первой демки матча (`demo_index 0`, A9) в `srv{id}.faceit/`, переименование в `srv{id}.dem`, папка убирается при любом исходе; дальше обычный путь `inspect_demo` (потолки фазы 6) - `run_job` - `verify`. Шов `with_fetch_fn` для тестов. Уборка задачи удаляет и `srv{id}.faceit`.
- **Фразы сбоев.** По подстроке ошибки `faceit`: «у матча FACEIT нет демки: матч ещё не доигран или демка удалена», «FACEIT отказал в доступе к демке: ключу автора нужен доступ к Downloads API», «матч FACEIT не найден», иначе «не удалось скачать демку FACEIT: ошибка сети или FACEIT API». В `Failure` уходит только фраза.
- **Ключ из окружения.** `parse_runner` читает `FACEIT_API_KEY` (как `record::parse_cfg`); флага нет (`--faceit-key` - «неизвестный параметр»); `Debug` аргументов и `JobConfig` ключ не показывают; `USAGE_RUNNER` об этом говорит. Пустой ключ не ошибка запуска.

## Tests

- `crates/runner/tests/faceit_flow.rs`: `faceit_link_from_a_friend_is_downloaded_by_the_runner_and_delivered` - друг - бот - сервер - раннер (скачивание подменено, `with_job_fn` и `probing_host` дают ролик) - ролик в чате друга; `getFile` не вызывался, в базе только канонический URL, скачивание вызвано один раз с `match_id` и ключом `ключ-автора`, ключа нет ни в журнале бота, ни в файлах данных сервера, ни в чате.
- `crates/server/tests/bot_faceit.rs` (7): `strict_link_is_queued_without_get_file`, `non_room_links_are_refused_with_an_explanation`, `plain_text_gets_a_hint`, `link_without_player_asks_for_player`, `same_match_same_player_is_a_duplicate`, `two_pending_rows_for_the_same_pair_make_one_job`, `link_respects_limits`.
- `crates/runner/tests/exec.rs`: `faceit_link_is_downloaded_by_match_id_with_the_runner_key`, `link_that_fails_the_strict_check_is_unsupported_source`, `missing_key_is_faceit_fetch_failed`, `fetch_failure_is_faceit_fetch_failed_without_urls` (заменили `faceit_link_without_a_demo_file_fails_with_unsupported_source`); в `exec.rs` два модульных теста на очистку текста журнала и фразы; в `lib.rs` тест переименован в `faceit_link_is_not_downloaded_from_the_server_and_keeps_the_demo_path`.
- `crates/cli/src/runner_cmd.rs`: `faceit_key_comes_from_the_environment_only`.

Зелёные: `cargo test --release -p runner -p pipeline`, `-p server`, `-p wire`, `-p cli --test pipeline`, `-p cli --bin cs2-cli runner_cmd`, `cargo test --release -p server --test deps`; `cargo build --release -p app` собирается.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - согласование тестов и objective] Голый id матча как попытка ссылки**
- **Found during:** разбор `<behavior>` Task 2.
- **Issue:** `<objective>` отправляет текст без `faceit` и `http` на подсказку, а тест `non_room_links_are_refused_with_an_explanation` требует для голого `<id>` объяснение «Это не ссылка...».
- **Fix:** `looks_like_link_attempt` считает попыткой ссылки и первое слово, совпадающее с `wire::faceit::is_match_id`.
- **Files modified:** crates/server/src/bot/ingest.rs
- **Commit:** 4e87744

**2. Подтверждение приёма ссылки проверяется до `step`.** Сообщение статуса сразу правится в «Задача #N в очереди», поэтому «FACEIT» в подтверждении видно только после `poll_once`, а `#N` - после `step`; тест `strict_link_is_queued_without_get_file` проверяет обе стадии.

**3. Защита от задачи FACEIT без пути.** Добавлена проверка «пустой путь демки - `UNSUPPORTED_SOURCE`» (план её не требовал): скачивать некуда, а раньше тот же случай обозначал «без файла».

**4. TDD без отдельной красной фазы.** Тесты Task 2 и Task 3 писались по `<behavior>` на реализации, уже готовой в трассирующем Task 1 (бот и раннер собраны вертикальным срезом), и прошли сразу; оговорено в плане как tracer + последующие задачи.

Остальное выполнено как написано. `type="tracer"` Task 1 прошёл сквозную проверку до расширения (интерактивный режим, `<verify>` автоматический).

## Known Limitations

- **Скачивание FACEIT не отменяется посреди загрузки:** `faceit::Client::download` не принимает признак отмены; отмена учитывается до и после скачивания (`pipeline::check`).
- **В bo3 берётся первая демка** (A9); выбор карты не предусмотрен.
- **Скачанный файл переименовывается в `srv{id}.dem` независимо от формата:** `.zst`/`.dem` распознаются по сигнатуре (как у файлов из чата).
- **Реальный запрос к FACEIT в тестах не выполняется:** сеть подменена `with_fetch_fn`; настоящий `faceit::Client` (`demo_urls` + `download`) проверен только компиляцией и ранее существующими тестами крейта. Проверка на живом матче - вручную, при приёмке фазы.

## Known Stubs

Нет.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-28 (строгий шаблон на боте и у раннера, в клиент уходит только `match_id`; `non_room_links_are_refused_with_an_explanation`, `link_that_fails_the_strict_check_is_unsupported_source`), T-07-29 (ключ только в окружении раннера, без флага; проверка отсутствия ключа в журнале бота, данных сервера и чате), T-07-30 (фразы вместо исходного текста; `fetch_failure_is_faceit_fetch_failed_without_urls`), T-07-31 (скачанная демка идёт через `inspect_demo` с потолками фазы 6), T-07-SC (новых внешних пакетов нет, `Cargo.lock` меняется одной строкой `faceit` у `runner`).

## Self-Check: PASSED

- FOUND: crates/runner/tests/faceit_flow.rs, crates/server/tests/bot_faceit.rs (7 тестов `#[test]`)
- FOUND: `fn with_fetch_fn(`, `room_match_id(` в crates/runner/src/exec.rs; `IntakeKind::Faceit`, `submit_faceit(` в crates/server/src/bot/ingest.rs; `"FACEIT_FETCH_FAILED"`, `pub const ALL: [FailCode; 22]` в crates/pipeline/src/record/failure.rs; `FACEIT_API_KEY` в crates/cli/src/runner_cmd.rs
- FOUND: коммиты 4e87744, 55e37ce, 2af79e0 в истории main
