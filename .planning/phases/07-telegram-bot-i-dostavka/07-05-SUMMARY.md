---
phase: 07-telegram-bot-i-dostavka
plan: 05
subsystem: server
tags: [telegram, bot-api, ureq, http-client, secrets, redaction, loopback]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 02
    provides: трейт Tg, TgError, InputFile, Sent, FakeTelegram, BotStand
provides:
  - "tg::http::HttpTg: боевая реализация Tg на ureq 3 (три агента, разбор Reply<T>, классификация sent: true/false)"
  - "tg::http::check_base_url / HttpTimeouts: только http:// на loopback с портом, сроки 30 с / опрос + 15 с / передача 60 мин"
  - "secret::Redactor: форма токена бота с '~' (точный секрет и по форме)"
  - "tests/common::FakeBotApi: поддельный Bot API на tiny_http (журнал, сценарные ответы, молчание)"
affects: [07-06, 07-09, 07-10, 07-11]

actuals:
  tokens: 26000
  tasks: 2
  commits: 2
plan_head_before: a668b553c858d7f0d644316a47ca4df9af202374
plan_head_after: 0759cf9a97262b05fdc0cf36bb8727b45f3a3c33
commits: 2

tech-stack:
  added: []
  patterns:
    - "токен бота только в URL внутри tg/http.rs; любой текст ошибки идёт через scrub (точный токен, форма с ~, Redactor)"
    - "sent: false только если соединения не было (Timeout Resolve/Connect, HostNotFound, ConnectionRefused и подобные), всё остальное после начала отправки — sent: true"
    - "поддельный сервер обрабатывает каждый запрос в своём потоке, молчание держится до остановки"

key-files:
  created:
    - crates/server/src/tg/http.rs
    - crates/server/tests/tg_http.rs
  modified:
    - crates/server/Cargo.toml
    - crates/server/src/tg/mod.rs
    - crates/server/src/secret.rs
    - crates/server/tests/common/mod.rs
    - crates/server/tests/secrets.rs

key-decisions:
  - "ureq перенесён из dev- в обычные зависимости без смены версии и признаков; Cargo.lock не изменился, deps.rs зелёный"
  - "Прокси из окружения отключён (proxy(None)), перенаправления не выполняются: токен не уходит никуда, кроме локального bot-api"
  - "Срок длинного опроса ставится на каждый запрос (config().timeout_global = timeout_secs + poll_extra), потому что он зависит от вызова"
  - "check_base_url возвращает Result<(), String>, ошибки по-русски без повтора адреса; порт 0 и нецифровой порт отвергаются"

requirements-completed: []
---

# Phase 7 Plan 05: клиент Bot API и токен в форме с тильдой Summary

`HttpTg` говорит с локальным `telegram-bot-api` настоящими запросами (`getMe`, `getUpdates` с `allowed_updates=["message"]`, `sendMessage` без `parse_mode`, ролик `sendVideo`/`sendDocument` полем `file:///…`), отличает «запрос мог дойти» от «не ушёл» и принимает только loopback-адрес; `Redactor` теперь маскирует токен бота и в форме `цифры~секрет`, под которой bot-api называет рабочую папку.

## Accomplishments

- **`tg/http.rs`.** `HttpTimeouts` (по умолчанию 30 с, 15 с, 60 мин), `check_base_url` (схема ровно `http://`, хост `127.0.0.1`, `localhost` или `[::1]`, обязательный порт 1–65535, без пути, логина, query), `HttpTg::new` с тремя агентами (управление: соединение 5 с и общий срок 30 с; опрос: срок на вызов; передача: ожидание ответа до `transfer`). Все с `http_status_as_error(false)`, без прокси, без перенаправлений. `call<T>` разбирает `Reply<T>`: `ok:false` превращается в `TgError::Api` с кодом, описанием и `retry_after`, `ok:true` без `result` и неразборный ответ дают `Io { sent: true }`.
- **Классификация ошибок.** Соответствие вариантов `ureq::Error` описано в `///`-доке `classify`: `Timeout(Resolve|Connect)`, `HostNotFound`, `ConnectionFailed`, `BadUri`, `Io(ConnectionRefused|AddrNotAvailable|NetworkUnreachable|…)` дают `sent: false`; любой другой тайм-аут и обрыв дают `sent: true` и, значит, `maybe_delivered()`.
- **`secret.rs`.** К точному секрету с `:` добавляется вариант с `~`, `mask_bot_tokens` ищет оба разделителя (те же пороги: 5 цифр, 30 символов секрета). Тильда в обычных путях (`~/x`, `x~1`, `~1.2`) не трогается.
- **`FakeBotApi`** в общем стенде тестов: `start`, `url`, `port`, `requests`, `requests_to`, `reply`, `silence`; по умолчанию `getMe`, `setMyCommands` и пустой `getUpdates` с ожиданием `min(timeout, 1 с)`; неверный токен в пути даёт 401, метод без сценария — 404.

## Tests

`tests/tg_http.rs` (7): `http_client_speaks_bot_api_against_a_fake_server`, `api_error_with_retry_after_is_parsed` (429, not modified, message to edit not found, 403), `silent_server_is_a_timeout_after_the_request_was_sent` (за ~1 с, `sent: true`), `silent_poll_times_out_after_the_poll_timeout_plus_extra`, `closed_port_error_has_no_token_in_any_form` (`sent: false`, ни токена, ни тильды, ни секретной части в `Display` и `Debug`), `non_loopback_or_cloud_url_is_refused` (11 плохих адресов, 3 хороших, адрес в тексте ошибки не повторяется), `unexpected_replies_are_classified_not_panicked_on`.
`secret.rs`: `tilde_form_of_the_bot_token_is_masked`. `tests/secrets.rs`: `bot_token_in_tilde_form_is_masked_everywhere` (путь файла фейка содержит токен с `~`, `Redactor` сервера его маскирует, в журнале, байтах базы и чате токена нет ни в одной форме).
Зелёные: `cargo test --release -p server` целиком, `cargo test --release -p cli --test pipeline`, `deps.rs`. `git diff Cargo.lock` пуст. Новых предупреждений clippy в изменённых файлах нет.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - защита токена] Прокси и перенаправления отключены**
- **Found during:** Task 1
- **Issue:** `ureq` 3 по умолчанию берёт прокси из окружения (`HTTP_PROXY`, `ALL_PROXY`): токен из URL мог бы уйти через чужой прокси, несмотря на проверку loopback.
- **Fix:** `proxy(None)`, `max_redirects(0)`, `max_redirects_will_error(false)` на всех трёх агентах.
- **Files modified:** `crates/server/src/tg/http.rs`
- **Commit:** a7ef9da

**2. [Rule 2 - защита токена] `scrub` режет точный токен и его форму с `~` независимо от переданного `Redactor`**
- **Issue:** клиент не должен зависеть от того, что вызывающий положил токен в `Redactor`.
- **Fix:** перед `Redactor::redact` текст ошибки проходит замену самого токена и его варианта с `~`.
- **Commit:** a7ef9da

**3. [тест слабее нужного] Интеграционный тест тильды дополнен прямой проверкой**
- **Issue:** путь с токеном из `getFile` в боте в текст не попадает вовсе (причина приёма — фиксированная строка), поэтому `bot_token_in_tilde_form_is_masked_everywhere` один не поймал бы возврат старого `Redactor`.
- **Fix:** тест дополнительно проверяет, что `core.redactor().redact(<путь файла>)` не содержит ни тильда-формы, ни секретной части; без правки в `secret.rs` он падает.
- **Commit:** 0759cf9

**4. Task 2 (`tdd="true"`) закоммичен одним коммитом, а не парой RED/GREEN.** Тесты и правка `secret.rs` написаны вместе; отдельного RED-коммита с падающим тестом нет. Эффект правки проверен утверждением в п. 3.

**5. `check_base_url` вместо `Result<()>` из `<interfaces>` возвращает `Result<(), String>`** — серверный алиас `Result` здесь не используется, строка ошибки совпадает с остальным кодом сервера.

## Known Stubs

Нет.

## Threat Flags

Нет новой поверхности сверх `<threat_model>`: клиент ходит только на loopback-адрес из настроек; проверка адреса при создании (T-07-20), `scrub` на всех текстах ошибок (T-07-18), форма с `~` (T-07-19), `sent: true` на таймауте после отправки (T-07-21), раздельные сроки агентов (T-07-22).

## Self-Check: PASSED

- FOUND: crates/server/src/tg/http.rs, crates/server/tests/tg_http.rs, crates/server/tests/common/mod.rs (`FakeBotApi`)
- FOUND commits: a7ef9da, 0759cf9
