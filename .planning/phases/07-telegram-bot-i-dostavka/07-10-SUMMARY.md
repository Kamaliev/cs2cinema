---
phase: 07-telegram-bot-i-dostavka
plan: 10
subsystem: server
tags: [telegram, bot, threads, config, operator-cli, e2e]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 02
    provides: Bot (step-функции), BotSettings, FakeTelegram, BotStand
  - phase: 07-telegram-bot-i-dostavka
    plan: 05
    provides: HttpTg, HttpTimeouts, check_base_url, FakeBotApi
provides:
  - "server::serve_with_bot(cfg, clock, sink, Option<Arc<dyn Tg>>); serve сохраняет сигнатуру и сам строит HttpTg по bot_api_url и токену"
  - "bot::spawn / BotHandle: потоки bot-poll, bot-ingest, bot-notify, bot-deliver, остановка не дольше 3 с, catch_unwind вокруг шагов"
  - "Bot::poll_tick -> PollOutcome { Updates, Conflict (409), Failed }; poll_once остался обёрткой"
  - "ключи bot_* в файле и CS2_SERVER_BOT_* в окружении; Config::bot_settings(), Config::http_timeouts()"
  - "cs2-server bot check; строка «доставка в Telegram» в job show; ключи бота в config show"
  - "TestOptions.tg / bot_api_url / bot_api_files_root, TestServer::tg(), try_start_with, wait_until"
affects: [07-11]

actuals:
  tokens: 16250
  tasks: 3
  commits: 3
plan_head_before: 9efd08acf96fa62ed96d29c3dcb2361b7cc7ee78
plan_head_after: cf2523ce81e3c648b43eea3ef4f3cfb0fc4f7c82
commits: 3

tech-stack:
  added: []
  patterns:
    - "потоки бота крутят те же step-функции (poll_inner, ingest_once, notify_once, deliver_once), что и тесты; логика не дублируется"
    - "счётчик живых потоков с AliveGuard (как busy у HttpServer::stop): остановка ждёт не больше 3 с и не join-ит застрявшие потоки"
    - "бот проверяется (getMe, меню, recover) между recover() ядра и bind порта: отказ bot-api не оставляет сервер наполовину запущенным"
    - "Throttle журнала: первый сбой и каждый тридцатый повтор"

key-files:
  created:
    - crates/server/tests/bot_e2e.rs
  modified:
    - crates/server/src/lib.rs
    - crates/server/src/bot/mod.rs
    - crates/server/src/config.rs
    - crates/server/src/cli.rs
    - crates/server/src/http.rs
    - crates/server/cs2-server.example.json
    - crates/server/tests/common/mod.rs
    - crates/server/tests/operator.rs

key-decisions:
  - "Бот включает только bot_api_url (или внедрённый Tg в serve_with_bot): тесты фаз 5-6 с одним токеном бота не запускают"
  - "Остановка: BotHandle::stop первым в Running::stop ставит общий флаг HTTP и ждёт потоки до 3 с; опрос после пробуждения проверяет флаг до разбора обновлений, смещение не двигается"
  - "Абсолютный путь в validate: is_absolute() или has_root(), чтобы «/var/lib/telegram-bot-api» принимался и на Windows (корень bot-api может жить в другой системе)"
  - "Ошибка настроек для адреса называет ключ и правило, но не значение; текст check_base_url добавляется в скобках"
  - "panic_text в http.rs стал pub(crate) и используется потоками бота вместо копии"

requirements-completed: []
---

# Phase 7 Plan 10: бот внутри cs2-server serve Summary

`cs2-server serve` с `bot_api_url` и токеном сам запускает бота (самопроверка `getMe`, меню, восстановление, четыре потока), демка друга доходит до доставленного ролика на настоящих потоках, а остановка укладывается в секунды.

## Accomplishments

- **Запуск.** `serve_with_bot` после `core.recover()` и до `bind` вызывает `Bot::startup()`; отказ даёт ошибку «бот не запустился: локальный telegram-bot-api не отвечает (getMe) — проверьте bot_api_url и что telegram-bot-api запущен с --local (…)» без токена и адреса, порт остаётся свободным (тест `bot_failing_getme_stops_serve_with_a_clear_error`). Потоки стартуют после HTTP; в журнал пишется «бот @имя запущен через локальный Bot API».
- **Потоки.** `bot-poll` (409 — одно предупреждение «второй экземпляр бота забирает обновления» и пауза 60 с; прочие сбои — пауза 2 с), `bot-ingest` (500 мс при пустой очереди), `bot-notify` (раз в 2 с), `bot-deliver` (1 с при пустой очереди). Паузы шагами по 200 мс с проверкой флага; паника шага ловится и пишется в журнал.
- **Остановка.** `Running::stop` сначала `BotHandle::stop` (до 3 с, застрявшие потоки не join-ятся), затем как раньше. Сквозной тест: `stop()` быстрее 5 с, в том числе с висящим долгим опросом.
- **Настройки.** Десять ключей `bot_*` в `Config`, `FileConfig` (с `deny_unknown_fields`), окружении (`CS2_SERVER_BOT_*`) и `validate`: только `http://` loopback с портом, адрес без токена и без абсолютного `bot_api_files_root` — ошибка, `bot_send_as` video/document, статус 3…600 с, передача 1…240 мин, лимиты 1…1000; токен без адреса — предупреждение «бот не запущен: не задан bot_api_url».
- **Оператор.** `bot check` (имя бота без токена), «доставка в Telegram: <состояние>» и ошибка доставки в `job show`, ключи бота и `telegram_bot_token = ***` в `config show`, образец настроек и справка обновлены.

## Verification

- `cargo test --release -p server` зелёный целиком (lib 61, bot_e2e 4, operator 17, deps 2 и все прежние); `cargo test --release -p cli --test pipeline` зелёный; `cargo build --release -p server -p cli` собирается.
- Тесты: `running_server_with_the_bot_turns_a_friend_demo_into_a_delivered_video`, `server_without_bot_url_does_not_start_the_bot`, `serve_starts_the_bot_against_a_local_bot_api_and_stops_quickly`, `bot_failing_getme_stops_serve_with_a_clear_error`, `bot_keys_load_from_file_and_env`, `bot_url_must_be_local_http`, `bot_url_without_token_or_root_is_an_error`, `token_without_bot_url_warns`, `config_show_prints_bot_keys_without_the_token`, `bot_check_reports_the_bot_name_without_the_token`, `job_show_prints_the_telegram_delivery_state`.
- Токен и адрес bot-api не попадают в журнал сервера, ошибку запуска, вывод `bot check`, `config show` и ошибки настроек (T-07-39, T-07-40).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `TestServer::try_start_with`**
- **Found during:** Task 3
- **Issue:** `start_with` паникует при ошибке запуска; тесту отказа `getMe` нужна сама ошибка.
- **Fix:** добавлен `try_start_with` (возвращает `Result`), `start_with` — тонкая обёртка. Сам тест отказа собирает `Config` вручную, чтобы проверить, что порт остался свободным.
- **Files modified:** crates/server/tests/common/mod.rs
- **Commit:** e0aa160

**2. [Rule 1 - Bug] Проверка «в ошибке нет значения» совпала с текстом подсказки**
- **Found during:** Task 2
- **Issue:** тест `bot_url_must_be_local_http` с «плохим» `http://127.0.0.1` (без порта) падал: значение — подстрока подсказки `http://127.0.0.1:ПОРТ`.
- **Fix:** значение без порта убрано из перебора «значение не повторяется», вместо него `http://secret-host.example:80`.
- **Commit:** 7487e13

Других отклонений нет. `http.rs` в `files_modified` плана не значился: `panic_text` сделан `pub(crate)`, чтобы не копировать функцию.

## Known Stubs

Нет. `notify::tick` по-прежнему только переводит `done` в `pending` доставки: статусы и отчёты о сбоях — планы 07-08 и 07-09, они войдут в потоки без изменений этого плана.

## Threat Flags

Нет новых поверхностей сверх `<threat_model>` плана (все T-07-39…T-07-42 закрыты тестами выше; 409 распознаётся в `poll_tick`).

## Notes for 07-11

- Живой стенд: `cs2-server --config <файл> serve`, проверка связи — `cs2-server --config <файл> bot check`. Обязательные ключи при включённом боте: `bot_api_url`, `bot_api_files_root` (+ токен в `TELEGRAM_BOT_TOKEN`).
- `bot_send_as` по умолчанию `video`; решение спайка 07-11 меняет одно умолчание в `Config::default`.
- Свободное место до `getFile` (Open Question 4) по-прежнему фаза 9.

## Self-Check: PASSED

- Файлы: crates/server/tests/bot_e2e.rs, crates/server/src/lib.rs (`pub fn serve_with_bot(`), crates/server/src/bot/mod.rs (`pub struct BotHandle`, `pub fn spawn(`, `bot-poll`, `bot-ingest`), crates/server/src/cli.rs (`bot check`, «доставка в Telegram»), crates/server/cs2-server.example.json (`"bot_api_url": null`) на месте.
- Коммиты e0aa160, 7487e13, cf2523c в истории main.
