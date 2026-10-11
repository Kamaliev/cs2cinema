---
phase: 07-telegram-bot-i-dostavka
plan: 03
subsystem: server
tags: [faceit, wire, dedup, queue-position, hard-link, resubmit]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 01
    provides: закрытые CR-01 и WR-01
  - phase: 05-yadro-servera-i-protokol
    provides: Core, Db, Blobs, TestServer, ManualClock
provides:
  - "wire::faceit: room_match_id, canonical_url, is_match_id (строгий разбор ссылки комнаты FACEIT, без зависимостей)"
  - "wire::fail::FACEIT_FETCH_FAILED в KNOWN (25 кодов)"
  - "Core::submit_faceit: задача FACEIT сразу queued без файла, в базе только канонический URL"
  - "assignment_of: ветка faceit без строки files (demo.size == 0, sha256 пустой); GET demo по ней даёт 404"
  - "Core::active_counts, find_duplicate (DedupKey), player_key, jobs_ahead, runner_online, submit_from, resubmit"
  - "Blobs::link_demo: жёсткая ссылка на демку другой задачи, копирование с проверкой SHA-256 как запасной путь"
affects: [07-04, 07-06, 07-07, 07-08, 07-09]

actuals:
  tokens: 22000
  tasks: 3
  commits: 3
plan_head_before: d93e40ef15fe40572cd944fdfb376f8113e295d3
plan_head_after: e8ef63d57a796f2678841c9b22b7a8eccb6bab00
commits: 3

tech-stack:
  added: []
  patterns:
    - "строгий разбор строк в wire, общий для сервера и раннера: сервер хранит только канонический вид, построенный из id"
    - "новая задача на чужой демке: receiving, затем link_demo вне мьютекса базы, затем Received и строка files одной транзакцией"

key-files:
  created:
    - crates/wire/src/faceit.rs
    - crates/server/tests/core_bot.rs
  modified:
    - crates/wire/src/lib.rs
    - crates/server/src/core.rs
    - crates/server/src/blobs.rs

key-decisions:
  - "Ссылка FACEIT: https, хост ровно www.faceit.com или faceit.com, только печатный ASCII без пробелов, до 300 символов, путь <язык>/<игра>/room/<id>[/<хвост>][/]; `#` и `?` отбрасываются, всё остальное отказ (D-01)"
  - "Дубль: задача в receiving/queued/assigned/running/uploading/done с тем же sha256 (или канонической ссылкой) и тем же player_key; хеш ищется и среди удалённых файлов демки, у готовой задачи это всё ещё дубль"
  - "Раннер в сети: неотозванный, last_seen_at >= now - (longpoll_secs*1000 + lease_ttl_ms); граница включительно"
  - "jobs_ahead: queued с меньшим id плюс все assigned/running/uploading (раннер один, записывает одну задачу, но считается любое число выданных)"

requirements-completed: []
---

# Phase 7 Plan 03: опоры бота в ядре сервера Summary

Ссылка FACEIT проходит строгий разбор в `wire::faceit` и становится задачей без файла, которую раннер получает по HTTP с `demo.size == 0`; ядро теперь считает активные задачи, ищет дубли, место в очереди и «раннер в сети», а повтор и задача для второго игрока берут демку жёсткой ссылкой.

## Accomplishments

- **`wire::faceit`.** `room_match_id` разбирает только `https://[www.]faceit.com/<язык>/<игра>/room/<id>[/<хвост>]` и возвращает id в нижнем регистре. Отказ для: голого id, чужого и похожего хоста (`faceit.com.evil.com`, `…@evil.com`, кириллические двойники), порта, `user@`, `http://`, `//host`, лишних и пустых сегментов, `match` вместо `room`, id с неверным префиксом или не-hex, пробелов и переводов строки внутри, строк длиннее 300. `canonical_url(id)` даёт `https://www.faceit.com/en/cs2/room/<id>` и обратим через `room_match_id`.
- **`FACEIT_FETCH_FAILED`** добавлен в `wire::fail` и `KNOWN` (теперь 25); страж раннера `FailCode::ALL ⊂ KNOWN` зелёный.
- **Задача FACEIT.** `Core::submit_faceit` перепроверяет ссылку, кладёт в `jobs.faceit_link` канонический URL, `orig_name = faceit-<id>`, `source_kind = 'faceit'`, состояние сразу `queued`, будит long-poll. `assignment_of` для такой задачи без строки `files` отдаёт `DemoInfo { size: 0, sha256: "" }`; `GET demo` по ней отвечает 404; раннер может завершить её сбоем.
- **Лимиты и дубли.** `active_counts(user)` (пользователь и вся очередь), `find_duplicate(DedupKey::File | Faceit)` (самая свежая подходящая задача; ссылка FACEIT приводится к каноническому виду, поэтому подходит и сырая), `player_key` (SteamID64 как есть, иначе trim, нижний регистр, сжатые пробелы, в том числе для кириллицы).
- **Очередь и раннер.** `jobs_ahead` и `runner_online` по правилам плана.
- **Повтор без повторной загрузки.** `Blobs::link_demo` делает `hard_link`, а при ошибке копирует через `copy_hashed` и сверяет SHA-256 с записью; несовпадение размера или хеша удаляет файл. `Core::submit_from` и `Core::resubmit` используют его; для FACEIT берётся та же ссылка без файла. Если демки исходной задачи нет, возвращается «демки задачи #N больше нет — пришлите файл заново», новой задачи нет.

## Tests

- `wire` (15): `room_link_gives_a_lowercase_match_id`, `lookalike_and_foreign_links_are_rejected` (25 негативных строк), `canonical_url_round_trips`, `match_id_shape_is_strict`, `long_but_valid_tail_is_not_cut_by_the_length_limit`, `known_codes_include_faceit_fetch_failed`.
- `server/tests/core_bot.rs` (9): `faceit_job_is_claimed_over_http_without_a_demo_file`, `faceit_submit_refuses_foreign_links_and_empty_player`, `active_counts_cover_every_unfinished_state`, `player_key_normalizes_names_and_keeps_steam_ids`, `duplicate_is_found_by_hash_and_player_key_but_not_after_failure`, `jobs_ahead_counts_queued_before_and_running`, `runner_online_follows_last_seen`, `resubmit_reuses_the_demo_by_a_hard_link`, `second_player_job_reuses_the_demo_without_a_copy`.
- `server` модульные (+2): `link_demo_makes_a_hard_link_not_a_copy` (дописанный в исходник байт виден по новому пути, то есть это одна и та же ссылка, проверка работает и на Windows), `link_demo_copies_when_the_link_cannot_be_made_and_checks_the_hash`.
- Зелёные: `cargo test --release -p wire`, `-p server` (включая `deps`), `-p runner`, `-p cli --test pipeline`. `cargo clippy -p server -p wire --all-targets` в изменённых файлах предупреждений не даёт.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - защита выдачи] `submit_from` вставляет файловую задачу как `receiving`, а не `queued`**
- **Found during:** Task 3
- **Issue:** план просит вставить `queued`, затем сделать ссылку. Между этими шагами раннер мог бы получить задачу без строки `files`, а `assignment_of` для `file` падает и ломает выдачу всей очереди (T-07-12).
- **Fix:** задача создаётся в `receiving`, после `link_demo` одной транзакцией ставится `queued` через `rules::decide(Received)` и пишется строка `files`; при сбое — `ReceiveFailed`, как в `finish_receive`. Результат для вызывающего тот же: возвращённая задача уже `queued`. FACEIT-источник по-прежнему вставляется сразу `queued` (файла там и не ждут).
- **Commit:** e8ef63d

**2. Task 2 без правок разбора:** все негативные и обратимые тесты Task 2 прошли на разборе из Task 1 (он писался сразу по списку обходов), поэтому в коммит Task 1 вошли и `lookalike_and_foreign_links_are_rejected`, и `canonical_url_round_trips`; Task 2 добавил код `FACEIT_FETCH_FAILED` и тест списка кодов.

**3. Мелкие дополнения:** модульный тест `match_id_shape_is_strict`, тест отказа `faceit_submit_refuses_foreign_links_and_empty_player`, второй модульный тест `link_demo`.

## Known Limitations

- `find_duplicate` не смотрит на `deleted_at`: у готовой задачи, чья демка уже убрана уборщиком, дубль по хешу всё равно находится (задача `done` с роликом остаётся ответом). Если бот должен считать «нет файла — не дубль», это решает 07-06 или фаза 9.
- Хеш при жёсткой ссылке не пересчитывается (только размер по метаданным): файл демки после приёма не меняется, пересчёт 2 ГБ при каждом повторе свёл бы выгоду ссылки на нет.
- Окно между `Core::submit` и записью `queued` в `tg_inbox` (07-02) этим планом не меняется.

## Known Stubs

Нет. Новые методы ядра пока не вызывает бот (планы 07-04, 07-06…07-09), это границы плана.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-09 и T-07-10 (строгий шаблон, ASCII, хост целиком; негативные тесты), T-07-11 (`link_demo` жёсткая ссылка, копирование только запасным путём), T-07-12 (ветка FACEIT в `assignment_of`, 404 на `GET demo`, `receiving` до готовности файла в `submit_from`), T-07-SC (новых пакетов нет, `deps` проходит).

## Self-Check: PASSED

- FOUND: crates/wire/src/faceit.rs (`pub fn room_match_id(`, `pub fn canonical_url(`), `pub mod faceit;` и `FACEIT_FETCH_FAILED` в crates/wire/src/lib.rs
- FOUND: `pub fn submit_faceit(`, `submit_from(`, `resubmit(`, `active_counts(`, `find_duplicate(`, `jobs_ahead(`, `runner_online(`, `pub fn player_key(` в crates/server/src/core.rs; `pub fn link_demo(` с `hard_link` в crates/server/src/blobs.rs
- FOUND: коммиты 500c17c, 082fffb, e8ef63d в истории main
