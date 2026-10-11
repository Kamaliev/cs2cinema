---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 03
subsystem: security
tags: [rust, demo, zstd, gzip, snappy, limits, mutation-tests]
status: complete

requires:
  - phase: 06-01/06-02
    provides: окружение фазы (независимый план волны 1)
provides:
  - demo::{DemoLimits, DemoError, read_demo_limited} — чтение демки с потолками файла и распаковки (SEC-02)
  - demo::MAX_COMMAND_UNPACKED (128 МиБ) и проверка snap::raw::decompress_len в Command::decode_payload (SEC-02)
  - demo::synth::{SynthPlayer, SynthKill, demo, zstd_bomb} — синтетическая демка с игроками и zstd-бомба для тестов других крейтов
affects: [06-07 (дочерний разбор зовёт read_demo_limited), 06-10 (стенды используют synth)]

estimate:
  tokens: 65000
  raw_tokens: 65000
  tasks: 2
  confidence: low
actuals:
  tokens: 9500
  tasks: 2
  commits: 2
plan_head_before: fe227bc26b34b8b2aafd3fe4ca6071d1438f9666
plan_head_after: 81c4053b53f222644f92421806df64367c55a980

tech-stack:
  added: []
  patterns:
    - "Недоверенный файл читается потоком с Read::take(limit + 1): бомба не накапливается в памяти, превышение — типизированный TooLarge"
    - "Заявленный размер snappy-команды проверяется до decompress_vec"
    - "Мутационный тест собственного парсера: отдельный поток + catch_unwind + recv_timeout, детерминированный LCG"

key-files:
  created:
    - crates/demo/src/synth.rs
    - crates/demo/tests/limits.rs
    - crates/demo/tests/hostile.rs
  modified:
    - crates/demo/src/lib.rs
    - crates/demo/src/command.rs

key-decisions:
  - "Стартовые лимиты: файл 2000 МиБ, распакованная демка 3 ГиБ (калибруются живым прогоном 06-11)"
  - "read_demo и decompress не менялись: их зовут приложение и cs2-cli; ограниченное чтение — отдельная функция"

requirements-completed: [SEC-02]

duration: 15min
completed: 2026-10-11
---

# Phase 6 Plan 03: Потолки чтения демки Summary

Демка читается с потолками размера файла и распаковки (zstd/gzip потоком, `take(limit + 1)`), snappy-команда с заявленными гигабайтами отвергается до выделения памяти, а собственный парсер не падает и не виснет на 50 усечённых и 200 побитово испорченных синтетических демках.

## Что сделано

- **Task 1 (tracer, fdfa7ca):** `read_demo_limited` с `DemoLimits` (2000 МиБ / 3 ГиБ) и `DemoError { TooLarge, Bad }`; zstd и gzip — потоковая распаковка с остановкой на `max_unpacked + 1`; нет магии `PBDEMS2\0` — `Bad`, для LFS-указателя с подсказкой `git lfs pull`. Модуль `demo::synth`: `demo()` собирает контейнер с таблицей `userinfo`, списком событий, `round_start`, убийствами и `round_end`; `zstd_bomb()` для крейтов без зависимости `zstd`. Тесты `tests/limits.rs` (7 шт., включая `zstd_bomb_is_too_large_without_unpacking_it_all`, `gzip_bomb_is_too_large`).
- **Task 2 (81c4053):** `MAX_COMMAND_UNPACKED = 128 МиБ` и проверка `decompress_len` в `decode_payload`; тесты `tests/hostile.rs` (4 шт.): усечения, перевёрнутые байты, команда с заявленными 4 ГиБ, обычная сжатая команда.

## Deviations from Plan

None - plan executed exactly as written.

`parse.rs`, `events.rs`, `bits.rs` не менялись: мутационные тесты не нашли ни паники, ни зависания (остальной разбор уже использует `Option`/`io::Result` и проверки границ). Мутации — только на синтетической демке; остаточный риск `source2-demo` закрывает дочерний процесс 06-07 с самоограничением памяти 06-04.

## Verification

- `cargo test --release -p demo` — 15 тестов зелёные (4 юнит, 4 hostile, 7 limits).
- `cargo test --release --workspace` зелёный, `cargo build --release -p cli` собирается, `cargo test --release -p cli --test pipeline` проходит.

## Known Stubs

None.

## Threat Flags

None - новых сетевых, файловых и авторизационных поверхностей сверх плана нет.

## Self-Check: PASSED

Файлы `synth.rs`, `limits.rs`, `hostile.rs`, `lib.rs`, `command.rs` существуют; коммиты fdfa7ca и 81c4053 в истории.
