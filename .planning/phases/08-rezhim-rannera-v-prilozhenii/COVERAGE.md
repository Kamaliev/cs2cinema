# Покрытие фазы 8 — Режим раннера в приложении

> Полное покрытие по умолчанию. Каждый отказ (OPT-OUT) — явное решение с причиной.
> Планы: 08-01 (трассирующий срез: карточка сервера через очередь), 08-02 (ворота, наблюдатель, карта и игрок из предпроверки), 08-03 (жизненный цикл карточки: перезапуск, отмены, «Повторить»), 08-04 (раннер в приложении: настройки, токен, состояние), 08-05 (окно: настройки и карточка), 08-06 (сквозные проверки с подделками).

## Волны

| Волна | Планы | Зависит от |
|-------|-------|------------|
| 1 | 08-01 | — |
| 2 | 08-02, 08-03 | 08-01 |
| 3 | 08-04 | 08-02, 08-03 |
| 4 | 08-05, 08-06 | 08-04 |

## Требования

| ID | Требование | Планы | Как проверяется |
|----|------------|-------|-----------------|
| APP-01 | В настройках приложения задаются адрес сервера и токен; удалённая задача — обычная карточка очереди | 08-01, 08-02, 08-03, 08-04, 08-05, 08-06 | `server_job_becomes_a_card_and_its_video_reaches_the_server`, `address_and_token_make_the_app_take_a_server_job`, тесты отмен и «Повторить» (08-03), раздел «Задачи с сервера» и `<human-check>` (08-05) |
| APP-02 | Удалённая задача берётся только когда локальная очередь свободна и не на паузе | 08-02, 08-04, 08-06 | `crates/runner/tests/gate.rs`, `admission_closes_for_every_busy_queue_state`, `local_job_closes_the_gate_and_the_server_job_waits`, `one_recording_at_a_time_and_local_jobs_wait_their_turn`, `paused_queue_keeps_server_jobs_out` |
| APP-03 | После перезапуска прерванная удалённая задача возобновляется сама | 08-03, 08-04, 08-06 | `interrupted_server_job_resumes_after_restart_without_a_pause`, `killed_during_recording_the_app_finishes_the_server_job_by_itself`, `killed_during_upload_the_app_uploads_without_recording_again`, `closed_app_with_a_waiting_server_card_resumes_it` |
| APP-04 | Приложение без сервера работает как раньше, `crates/cli/tests/pipeline.rs` остаётся зелёным | 08-01, 08-04, 08-06 | прежние тесты `app::queue` без правок, `empty_address_starts_no_runner_and_the_queue_works_as_before`, `without_an_address_the_app_has_no_runner_and_behaves_like_v1`, `cargo test -p app queue`, `cargo test --release -p cli --test pipeline`, `cargo test --release --workspace` |

## Критерии успеха (ROADMAP, Phase 8)

| # | Критерий | Планы |
|---|----------|-------|
| 1 | Адрес и токен в настройках, состояние подключения; карточка с журналом, этапами, отменой и «Повторить»; результат уходит на сервер | 08-01, 08-03, 08-04, 08-05 |
| 2 | Удалённая берётся только при пустой, свободной и не стоящей на паузе очереди; локальная ждёт; одна запись | 08-02, 08-04, 08-06 |
| 3 | Закрыли или убили посреди удалённой задачи — продолжается сама, результат доходит до сервера | 08-03, 08-04, 08-06 |
| 4 | Без адреса — v1.0; `crates/cli/tests/pipeline.rs` и тесты `app::queue` зелёные | 08-01, 08-04, 08-06 |

## Решения автора по открытым вопросам исследования (2026-10-12)

| ID | Решение | Планы |
|----|---------|-------|
| D-01 | «Повторить» у закрытой выдачи — локальное продолжение по `record.json` без загрузки на сервер; «Заново» скрыта | 08-01 (ветка `queue_runner`), 08-03 (тесты), 08-05 (кнопки) |
| D-02 | Токен — отдельный файл в каталоге раннера с отключённым наследованием ACL (`icacls /inheritance:r`), не в `Settings`/`settings.json`; значение держит `runner::Token` | 08-04 (`remote::token`, `RemoteRunner`), 08-05 (поле и кнопки), 08-06 (поиск токена) |
| D-03 | `epoch` в `runner::Task` | 08-01 |
| D-04 | Потолка ожидания удержанной задачи нет, но статус виден | 08-03 (поведение), 08-04 (`Working`), 08-05 (`waiting_reason`, `server_note`), 08-06 (сценарий ожидания) |
| D-05 | Подпись карточки — игрок и карта, если известны, иначе «Задача с сервера #N» | 08-01 (`card_label`), 08-02 (`Artifact::Match`), 08-05 (`server_heading`, `set_card_info`) |

## Ограничения фазы

| Ограничение | Где соблюдается |
|-------------|-----------------|
| `queue::start` не меняется; хук через `start_with_hook`; `QueueConfig` литералом из трёх полей | 08-01 (acceptance: неизменная строка сигнатуры), все планы гоняют прежние тесты очереди |
| `cargo test -p app queue` и `cargo test --release -p cli --test pipeline` зелёные | verify в 08-01…08-06 |
| `crates/server` без зависимостей от `pipeline`/`runner`/`host`/`faceit` | `server` только в `[dev-dependencies]` приложения (08-01); `--test deps` и пустой diff `crates/server` (08-01, 08-02, 08-04, 08-06) |
| Одна запись за раз | запись только в потоке очереди (08-01), `max_concurrent == 1` (08-06) |
| Без адреса — поведение v1.0 | 08-04, 08-06 |
| Подставной токен не в журналах, базе, состоянии, `settings.json` | 08-02 (`emit`), 08-04 (`token`, `RemoteStatus`), 08-06 (`issued_token_is_found_only_in_its_file`) |
| Пользовательские строки по-русски | тексты во всех планах; `status_texts_are_russian_and_short` (08-04) |

## Исследование (08-RESEARCH.md)

| Пункт | Решение | Планы |
|-------|---------|-------|
| Pattern 1: карточка = строка `jobs` с `external_ref`, миграция 6, усыновление | INTEGRATE | 08-01 |
| Pattern 2: `RemoteExecutor` пересылает события каналом, `Artifact::OutDir` обязателен | INTEGRATE | 08-01 |
| Pattern 3: ветка удалённой карточки в исполнителе очереди, `on_settled` после политики | INTEGRATE | 08-01 (как обёртка `queue_runner` над `jobs::runner`) |
| Pattern 4: ворота допуска, `hello`-пинг, `release(counts: false)` | INTEGRATE | 08-02, 08-04 |
| Pattern 5: наблюдатель состояния | INTEGRATE | 08-02 (поле `Config.observer` вместо `run_observed`), 08-04 |
| Anti-patterns: `Queue::resume` как «разбудить», второй путь записи, `unsafe`, `hello` как проверка, смена сигнатуры `start` | соблюдены | 08-01, 08-05 («Переподключить» перезапускает цикл) |
| Don't hand-roll: `RunnerState`, `Uplink`/`Tee`, `PipelineExecutor`, `runner::Token`, `runner.lock` | INTEGRATE | 08-01, 08-03, 08-04 |
| Ответ §1: литералы `5` в тестах store, совместимость `cs2-cli acceptance` | INTEGRATE | 08-01 |
| Ответ §3: `recover_after_crash`, `Reset.interrupted_remote`, усыновление в любом статусе | INTEGRATE | 08-01, 08-03 |
| Ответ §4: адрес, токен, права файла, состояния, кнопки | INTEGRATE | 08-04, 08-05 |
| Ответ §5: без адреса — ноль потоков и запросов | INTEGRATE | 08-04, 08-06 |
| Pitfall 1: отмена сервера ставит очередь на паузу | mitigate | 08-01 (`Advice`), 08-03 |
| Pitfall 2: два автомата пауз | mitigate | 08-05 («Продолжить» снимает обе) |
| Pitfall 3: `exit(0)` трея и самообновления | accept | выход равен убийству; задача продолжается после запуска (08-03, 08-06) |
| Pitfall 4: остановка раннера = отмена карточки | mitigate | 08-04 (`pending`) |
| Pitfall 5: ролик удалён после подтверждения сервером | mitigate | 08-04 (строка журнала), 08-05 (`server_note`) |
| Pitfall 6: тихий пропуск предпроверки | mitigate | 08-01 (`PipelineExecutor` всегда), 08-04 (`NoCli`) |
| Pitfall 7: текст о ключе FACEIT | INTEGRATE | 08-02 |
| Pitfall 8: аренда держится, карточка не стартует | D-04: без потолка, со статусом | 08-03, 08-05 |
| `USAGE_RUNNER` после фазы 8 | INTEGRATE | 08-02 |
| A1 (`icacls` без повышения прав) | проверка кода выхода + `<human-check>` | 08-04, 08-05 |
| A2 (`hello` обновляет `last_seen_at`), A3 (`claim?wait=5`) | подтверждены кодом сервера | 08-02 |
| A4 (`cs2-cli.exe` рядом с `cs2cinema.exe`) | подтверждено `release.yml`; иначе состояние `NoCli` | 08-04 |
| A5 (потолок ожидания) | снят решением D-04 | — |
| Живой стенд бот → сервер → приложение | OPT-OUT | отложен в фазу 9 решением автора (07-UAT, коммит 01e2ffd) |
| Уборка файлов раннера, развёртывание, учения | OPT-OUT | фаза 9 (OPS-01…04) |
| DPAPI для токена | OPT-OUT | автор выбрал файл с ACL (D-02) |

## Протокол раннера (`cs2-server`, через крейт `runner`)

| capability | decision | reason |
|---|---|---|
| `POST hello` (версия, статус, `holding`, директивы `Keep`/`Cancel`/`Abandon`) | INTEGRATE | 08-02 (статус «занят: …»), 08-03/08-06 (возобновление с `holding`) |
| `POST claim?wait=N` (long-poll) | INTEGRATE | 08-01; `poll_secs = 5` в приложении (08-04) |
| `POST progress` (пульс, этап, дубль, отмена сервера) | INTEGRATE | 08-01 (события карточки в `Tee`), 08-03 (отмена) |
| `POST release` с `counts: false` | INTEGRATE | 08-02 (гонка ворот) |
| `POST release` с `counts: true` (сбой окружения или запуска) | INTEGRATE | без изменений раннера; строка журнала карточки (08-04) |
| `POST finish` (`failed`, `cancelled`) | INTEGRATE | 08-03 |
| скачивание демки кусками (`HEAD`/`GET Range`) | INTEGRATE | без изменений раннера (08-01, 08-03) |
| загрузка результата кусками с докачкой (`HEAD`/`PUT`, SHA-256) | INTEGRATE | 08-01, 08-06 (загрузка после убийства) |
| несколько раннеров на одной машине | OPT-OUT | RUN-F1 (v2); `runner.lock` не даёт запустить два (08-04) |
