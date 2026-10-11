---
phase: quick-261011-hb5
plan: 01
subsystem: pipeline/record
tags: [record, verify, ffprobe, manifest, fake-host]
requires:
  - phase: 03-proverennye-dubli-povtor-i-vosstanovlenie
    provides: Verifier, clips/manifest.json, Manifest::reconcile, FakeHost
provides:
  - "Verifier::probe пересверяет размеры video.mp4/audio.wav после ffprobe: растущий дубль не получает validated со старым размером"
  - "Verifier::verify_rest сверяет validated-дубли с диском после закрытия игры и перепроверяет разошедшиеся, с лимитом FINAL_REPROBES"
  - "failure::msg_take_growing и Scenario::growing_take"
affects: [pipeline::assemble, 04-ochered-bez-prismotra-i-priemka]
requirements: [TAK-02, TAK-05]
actuals:
  tokens: 3000
  tasks: 2
  commits: 2
plan_head_before: a1986084dffa6e8ecd6927206d4c52df7d3585d5
plan_head_after: e6b2005fa4ab263f9845e92944d18ffba781b7a4
tech-stack:
  added: []
  patterns: ["пересверка размера файла после чтения внешней утилитой (stat до и после)"]
key-files:
  created: []
  modified:
    - crates/pipeline/src/record/mod.rs
    - crates/pipeline/src/record/failure.rs
    - crates/host/src/fake.rs
    - crates/pipeline/tests/record.rs
key-decisions:
  - "Дубль, чей размер сдвинулся под ffprobe, идёт по существующей ветке NotReady (повтор через REPROBE до take_finalize, потом TAKE_INVALID «не дописан»), а не получает отдельный код сбоя"
  - "check_take_for_assembly остаётся строгим: исправлена причина (устаревший размер в манифесте), а не проверка"
status: complete
completed: 2026-10-11
---

# Quick 261011-hb5: проверка стабильности дубля Summary

Дубль, который ffmpeg HLAE ещё дописывает, больше не попадает в манифест как `validated` с устаревшим размером: размеры сверяются после ffprobe, а после закрытия игры validated-дубли сверяются с диском и при расхождении перепроверяются.

## Что сделано

**Задача 1 (tracer, 9d23790).** Причина бага: `verify::probe_take` снимает размеры до двух вызовов ffprobe и кладёт их в `TakeFacts`; ожидание стабильности было только в ветке `Invalid`. Теперь `Verifier::probe` после `Valid` повторно читает `Verifier::sizes`; если размер не равен `(video_bytes, audio_bytes)` из `TakeFacts`, исход заменяется на `NotReady(msg_take_growing(..))`, а в манифест через `mark_recorded` ложатся актуальные размеры. Дальше работает существующая логика: повтор через `REPROBE` до `take_finalize`, в журнале «ещё дописывается», по истечении срока `TAKE_INVALID` с «не дописан за 20 с». В фейк добавлена ручка `Scenario::growing_take(i, lasts)`: ffprobe читает `video.mp4` успешно, а файл при каждом опросе растёт на 4096 байт.

**Задача 2 (e6b2005).** `verify_rest` перед циклом вызывает `Manifest::reconcile` (validated-дубли с разошедшимся размером или пропавшим файлом понижаются до recorded, строки идут в `record.log`) и ставит все recorded-дубли, которых нет в очереди, на проверку. В `Pending` добавлено `regrown`, константа `FINAL_REPROBES = 3`: в финальном проходе при сдвигающемся размере ровно столько повторов, затем `TAKE_INVALID` «не дописан». Цикл `verify_rest` конечен.

## Тесты

Четыре новых теста в `crates/pipeline/tests/record.rs` (все содержат `growing`):
- `take_growing_after_a_good_ffprobe_is_validated_only_with_its_final_size` (A)
- `take_growing_past_take_finalize_fails_as_not_final` (B)
- `validated_take_growing_before_game_exit_is_reprobed_with_the_new_size` (C)
- `take_growing_after_the_game_closed_fails_after_bounded_reprobes` (D)

Плюс проверка `msg_take_growing` в юнит-тесте `take_invalid_names_the_take_in_message_and_hint`. Тесты A, B и C были красными до правок (A: в манифесте 10 вместо 4106 байт, B: запись удалась вместо сбоя, C: 10 вместо 4106 байт). Тест D прошёл уже после задачи 1 (в финальном проходе обычный NotReady и так даёт сразу сбой) и остаётся защитой от зацикливания, когда в финальном проходе включаются повторы.

## Проверка (фактический вывод)

- `cargo test --release -p pipeline --test record growing`: 4 passed
- `cargo test --release -p pipeline`: lib 104 passed, record 54 passed, остальные наборы зелёные
- `cargo test --release -p host --lib`: 70 passed, 1 ignored
- `cargo test --release -p cli --test pipeline`: 2 passed
- `git diff --quiet a198608 -- crates/pipeline/src/assemble.rs crates/pipeline/src/job.rs crates/app`: код 0 (не тронуты)
- `cargo build --release --workspace`: Finished, линковка `cs2cinema.exe` прошла (файл не был занят)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `cargo fmt` переформатировал посторонние файлы**
- **Found during:** Task 1
- **Issue:** `cargo fmt -p pipeline` и `rustfmt` по `record/mod.rs` затронули `crates/pipeline/src/lib.rs` и `record/failure.rs` (не относящиеся к задаче правки форматирования).
- **Fix:** изменения отменены через `git checkout -- <файл>`; для `failure.rs` две правки задачи применены заново. В коммитах только правки по плану.
- **Commit:** 9d23790, e6b2005

Иных отклонений нет.

## Known Stubs

Нет.

## Threat Flags

Нет новой поверхности атаки (только `std::fs::metadata` и в фейке `OpenOptions`).

## Self-Check: PASSED

- FOUND: crates/pipeline/src/record/mod.rs, crates/pipeline/src/record/failure.rs, crates/host/src/fake.rs, crates/pipeline/tests/record.rs
- FOUND commits: 9d23790, e6b2005 (ancestors of HEAD)
