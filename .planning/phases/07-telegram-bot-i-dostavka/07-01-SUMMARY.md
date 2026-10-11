---
phase: 07-telegram-bot-i-dostavka
plan: 01
subsystem: server
tags: [tiny_http, vendor, dos, cancel, lease]
status: complete

requires:
  - phase: 05-yadro-servera-i-protokol
    provides: HTTP-слой раннеров (http.rs), правила аренды (rules.rs), Core::release/cancel
provides:
  - "CR-01 закрыт: огромный Content-Length отвергается 413 в диспетчере, дочитывание тела в tiny_http ограничено"
  - "WR-01 закрыт: release отменённой задачи заканчивает её состоянием cancelled"
affects: [07-telegram-bot-i-dostavka, 09-prokси-и-таймауты]

actuals:
  tokens: 4000
  tasks: 2
  commits: 2
plan_head_before: c3661bc9f3117e21be8217b05a4baad18aa8c2fd
plan_head_after: 800a122400901b7a240b4b7a195b08a8eabad1f7
commits: 2

tech-stack:
  added: ["vendor/tiny_http (копия tiny_http 0.12.0 с одной правкой, через [patch.crates-io])"]
  patterns: ["пропатченная копия стороннего крейта в vendor/ с VENDORED.md", "отказ по размеру тела в диспетчере до обработчика"]

key-files:
  created:
    - vendor/tiny_http/ (копия крейта)
    - vendor/tiny_http/VENDORED.md
  modified:
    - Cargo.toml
    - Cargo.lock
    - crates/server/src/http.rs
    - crates/server/src/rules.rs
    - crates/server/src/core.rs
    - crates/server/tests/protocol_http.rs
    - crates/server/tests/lease_http.rs

key-decisions:
  - "Вариант 1 ревью CR-01: копия tiny_http 0.12.0 в vendor/ с правкой EqualReader::drop (буфер 8 КиБ на стеке, не больше 1 МиБ); std::mem::forget отвергнут"
  - "MAX_BODY = wire::CHUNK_SIZE + 64 КиБ: самое большое законное тело — кусок результата 8 МиБ"
  - "Заголовок Connection: close в ответе 413 не выставляется: tiny_http 0.12 игнорирует Connection в ответах (response.rs add_header)"

requirements-completed: [BOT-08]
---

# Phase 7 Plan 01: CR-01 и WR-01 Summary

Запрос без токена с `Content-Length: 10^15` получает 413 и больше не роняет `cs2-server` (и бота в нём), а `release` отменённой задачи даёт `cancelled` без повторной выдачи.

## Accomplishments

- **CR-01.** В `vendor/tiny_http/` лежит копия `tiny_http` 0.12.0 с единственной правкой `EqualReader::drop`: остаток тела дочитывается буфером 8 КиБ на стеке и не больше `DRAIN_LIMIT` = 1 МиБ вместо `vec![0; remaining]`. Подключена через `[patch.crates-io]`, каталог исключён из рабочей области (`exclude = ["vendor"]`). `VENDORED.md` хранит версию, контрольную сумму оригинала и причину правки.
- Диспетчер `http.rs` отвергает тело больше `MAX_BODY` ответом 413 `too_large` из отдельного потока, до `busy.fetch_add`, до `tx.send`, до проверки токена и на любом пути (в том числе неизвестном).
- Проверено, что тест ловит именно баг: при временном откате правки `Drop` тест `huge_content_length_without_a_token_does_not_kill_the_server` аварийно завершает процесс с `memory allocation of 1000000000000000 bytes failed`.
- **WR-01.** `Event::Release` при запрошенной отмене (`Soft`/`Force`) переводит задачу в `Cancelled` (`finished_at`, `runner_id = None`, `lease_until = None`), число выдач не трогает, `discard_parts = true`. `Core::release` по этому признаку убирает недозагруженные куски, как `Core::cancel`.

## Tests

- `protocol_http`: `huge_content_length_without_a_token_does_not_kill_the_server`, `oversized_body_on_any_path_is_413_before_the_handler`.
- `rules` (модульный): `release_of_a_job_with_a_requested_cancel_cancels_it` (Soft/Force, counts true/false, чужая эпоха и окончательная задача по-прежнему отказ).
- `lease_http`: `cancelled_leased_job_released_by_the_runner_is_not_issued_again` (два раннера, `grants` не растёт).
- Зелёные: `cargo test --release -p server`, `-p runner`, `-p cli --test pipeline`; `cargo build --release -p cli -p server`; `cargo tree -p server -i tiny_http` показывает `vendor/tiny_http`; `server --test deps` проходит.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug в плане] Заголовок `Connection: close` в ответе 413**
- **Found during:** Task 1 (первый прогон теста)
- **Issue:** план требует `Connection: close` в ответе 413, но `tiny_http` 0.12 молча отбрасывает заголовки `Connection`, `Trailer`, `Transfer-Encoding`, `Upgrade` в `Response::add_header`; утверждение теста не могло выполниться.
- **Fix:** заголовок не выставляется, из теста убрано утверждение о нём, в докстринге `too_large` записана причина. Защита от недочитанного остатка обеспечена ограничением дочитывания (1 МиБ) и потолком `MAX_BODY`; хвост после лимита может лишь испортить разбор следующего запроса на том же соединении самого клиента.
- **Files modified:** crates/server/src/http.rs, crates/server/tests/protocol_http.rs
- **Commit:** 780c058

**2. [Rule 1 - Bug в тесте] Проверка живости в `oversized_body_on_any_path_is_413_before_the_handler`**
- **Issue:** повторный `claim` тем же раннером возвращал 200 (переприсоединение уже выданной задачи), а не 204.
- **Fix:** живость проверяется вторым раннером, у которого свободной работы нет.
- **Commit:** 780c058

**3. [Rule 3 - Блокировка] В копию крейта не вошли `Cargo.lock` и `.github/`**
- `Cargo.lock` крейта игнорируется его же `.gitignore`, `.github/` — CI апстрима; в `VENDORED.md` записано. Остальные файлы (`LICENSE-*`, `src/`, `benches/`, `examples/`, `tests/`) скопированы.

Прочее: план выполнен как написан.

## Threat Flags

Новых поверхностей нет. T-07-01, T-07-03 и T-07-SC закрыты (потолок и ограниченное дочитывание, ветка отмены, контрольная сумма оригинала в `VENDORED.md`); T-07-02 (медленные клиенты, WR-03) принят и остаётся за фазой 9.

## Known Stubs

Нет.

## Self-Check: PASSED

- FOUND: vendor/tiny_http/src/util/equal_reader.rs (содержит `DRAIN_LIMIT`), vendor/tiny_http/VENDORED.md
- FOUND: коммиты 780c058, 800a122 в истории main
