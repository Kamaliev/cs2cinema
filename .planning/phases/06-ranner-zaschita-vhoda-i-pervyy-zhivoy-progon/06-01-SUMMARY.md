---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 01
subsystem: pipeline
tags: [rust, player-selection, fail-codes, wire, precheck]
status: complete

requires:
  - phase: 04 (запись и склейка)
    provides: FailCode/RecordFailure/classify, job::run_job, retry_no_data как образец типизированного RunError
  - phase: 05 (ядро сервера и протокол)
    provides: wire::fail, wire::Phase
provides:
  - pipeline::resolve_player (SteamID64 → точное имя → без регистра и с одинарными пробелами → подстрока), PlayerError, CANDIDATES_MAX = 10 (SEC-03)
  - pipeline::ensure_highlights и job::hero_for — единственная точка выбора героя; run_job зовёт её сразу после разбора демки
  - FailCode::{PlayerNotFound, PlayerAmbiguous, NoHighlights, BadDemo, DemoTooLarge, ResultInvalid, UnsupportedSource}, FailCode::ALL (21), msg_* и clean_text (RUN-05)
  - wire::fail — строки всех кодов раннера, JOB_ERROR и fail::KNOWN (24 строки); wire::Phase::Precheck (ALL = 10)
affects: [06-07 (cs2-cli inspect зовёт hero_for), 06-10 (BAD_DEMO, DEMO_TOO_LARGE, RESULT_INVALID, UNSUPPORTED_SOURCE выдаёт раннер), страж FailCode::ALL против wire::fail::KNOWN]

estimate:
  tokens: 75000
  raw_tokens: 75000
  tasks: 2
  confidence: low
actuals:
  tokens: 7500
  tasks: 2
  commits: 2
plan_head_before: d0fd274e810d26e9b090fbc358161064239e564f
plan_head_after: bfbf8c6bf2db330619e17b68c1b5bc791e04ab32

tech-stack:
  added: []
  patterns:
    - "На каждом ярусе выбора: один — игрок, несколько — PLAYER_AMBIGUOUS, ноль — следующий ярус; угадывания нет"
    - "Записи одного человека (тот же SteamID64 и то же имя после нормализации) сливаются в запись с наибольшим числом убийств и гранат"
    - "Недоверенный текст (ник, имя из демки) в сообщениях сбоя проходит failure::clean_text: без управляющих символов, до 64 символов по символам"
    - "Отказ до записи — RunError с RecordFailure в состоянии Precheck и классом «прочее»"

key-files:
  created: []
  modified:
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/job.rs
    - crates/wire/src/lib.rs

key-decisions:
  - "Слияние двойников применяется на каждом ярусе, а не только на ярусе SteamID: переподключившийся игрок с тем же SteamID и именем не даёт ложной неоднозначности по нику"
  - "find_player остался с прежней сигнатурой как resolve_player(..).ok(): crates/app собирается без правок, но неоднозначный ник теперь даёт None"
  - "JOB_ERROR есть только строкой в wire (FailCode его не содержит): pipeline такого кода не выдаёт"
  - "Если SteamID задан, но не найден, а ник пуст, в сообщение идёт сам SteamID как запрос"

requirements-completed: [SEC-03, RUN-05]

duration: 25 min
completed: 2026-10-11
---

# Phase 06 Plan 01: строгий выбор игрока и коды отказа до запуска CS2 Summary

**Неоднозначный ник («mple» → `s1mple` и `s1mple_fan`) теперь даёт `PLAYER_AMBIGUOUS` со списком «имя (SteamID64)» до записи; отсутствующий игрок — `PLAYER_NOT_FOUND`, игрок без моментов — `NO_HIGHLIGHTS`; строки всех кодов раннера лежат в `wire::fail::KNOWN`.**

## Выполнено

- **Задача 1 (tracer, `9342ca3`).** `resolve_player` заменил угадывание по подстроке, `job::hero_for` стал единственной точкой выбора героя (в `run_job` — сразу после `analyze`, до `out_dir`, проб NVENC, плана и запуска игры). В `failure.rs` — семь новых `FailCode` (класс «прочее»), `FailCode::ALL`, русские тексты и подсказки. В `wire` — константы кодов, `fail::KNOWN`, `Phase::Precheck`. Трассёр `ambiguous_nick_stops_the_job_before_recording_with_candidates` зелёный.
- **Задача 2 (`bfbf8c6`).** Шесть тестов `resolve_player` (ярусы, SteamID сильнее ника, двойники SteamID, потолок в 10 кандидатов, пустой ник, очистка и обрезка), расширенный `codes_have_stable_names_and_hints`, `new_codes_are_class_other_in_any_state`, тест текстов сбоя.

## Проверка

- `cargo test --release -p pipeline --lib` — 125 тестов, `-p wire` — 9, `-p server` — 43, `-p cli --test pipeline` — 2: всё зелёное.
- `cargo build --release -p cli -p app` собирается; `crates/app` не менялся.
- Сервер читает `precheck` как «идёт работа» (ветка `_` в `rules.rs`), правок не потребовалось.

## Deviations from Plan

None - plan executed exactly as written. (Мелочи: слияние двойников сделано общим `settle` для всех ярусов; рабочая копия `failure.rs` приведена с CRLF к LF — индекс git всегда был LF, diff не изменился.)

## Известные заглушки

Нет. `BAD_DEMO`, `DEMO_TOO_LARGE`, `RESULT_INVALID`, `UNSUPPORTED_SOURCE` объявлены намеренно без выдающего кода: их выдают планы 06-07 и 06-10 (так и записано в плане).

## Threat Flags

Нет новой поверхности сверх `threat_model` плана. T-06-01, T-06-02 закрыты тестами; T-06-03 принят (запрос из одного символа даёт `PLAYER_AMBIGUOUS` с 10 кандидатами).

## Замечание о ветке

Коммиты сделаны прямо в `main`: в проекте `branching_strategy: none`, все прошлые фазы шли так же, а оркестратор запустил исполнителя на основной рабочей копии. Проверка «защищённая ветка» (`git.base-branch --is-protected`) для `main` возвращает `true`; конфиг я не менял.

## Self-Check: PASSED

- FOUND: crates/pipeline/src/record/failure.rs, crates/pipeline/src/lib.rs, crates/pipeline/src/job.rs, crates/wire/src/lib.rs
- FOUND commits: 9342ca3, bfbf8c6 (предки HEAD)
