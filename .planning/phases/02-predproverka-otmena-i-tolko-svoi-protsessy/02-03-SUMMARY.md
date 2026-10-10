---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
plan: 03
subsystem: pipeline
tags: [rust, precheck, steam, disk-space, deadlines, failure-codes]

requires:
  - phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
    provides: RecordFailure, FailCode, RecordState::LADDER, таблица Deadlines
provides:
  - "pipeline::precheck: Level/Item/Report, Facts и факты-перечисления, evaluate, Report::{worst, blocks, replace, to_json, from_json, failure}, space_items"
  - "pipeline::precheck::steam: Manifest, parse_manifest, manifest_path, SteamState, steam_state, patch_version"
  - "pipeline::precheck::space: константы размера, Need, floors, estimate, same_volume, verdict"
  - "FailCode::{PrecheckFailed, ForeignGameRunning, PrevGameStuck} (PRECHECK_FAILED, FOREIGN_GAME_RUNNING, PREV_GAME_STUCK)"
  - "RecordState::Precheck — первая ступень лестницы (код precheck)"
  - "Deadlines: tool_probe, nvenc_probe, thumbnail (растут с запасом); quit_wait, kill_confirm, window_poll, prev_game_exit, next_job_settle (не растут)"
affects: [02-05, 02-06, 02-07, 02-08, предпроверка, отмена, настройки]

actuals:
  tokens: 16000
  tasks: 2
  commits: 2

plan_head_before: d071e60519cf339d8336d3603bf725d5d8eb6e2c
plan_head_after: 2414a08aaa3d38ee20cc71992275ba3f4d3e6955
commits: 2

tech-stack:
  added: []
  patterns:
    - "предпроверка = сбор фактов (хост, 02-05) + чистая evaluate(&Facts) -> Report; весь русский текст пунктов в одном модуле"
    - "факт по умолчанию «не проверялось» — пункта в отчёте нет; нераспознанное из чужих файлов — warn, не fail"
    - "оценка места двухпроходная: минимум до плана, оценка по плану после; общий том — сумма потребностей"

key-files:
  created:
    - crates/pipeline/src/precheck/mod.rs
    - crates/pipeline/src/precheck/steam.rs
    - crates/pipeline/src/precheck/space.rs
  modified:
    - crates/pipeline/Cargo.toml
    - crates/pipeline/src/lib.rs
    - crates/pipeline/src/record/failure.rs
    - crates/pipeline/src/record/machine.rs
    - crates/pipeline/src/record/deadlines.rs
    - Cargo.lock

key-decisions:
  - "Свою застрявшую игру исключаем из проверки чужого CS2 по PID из PrevGameFact::Stuck: иначе она выдавалась бы за чужую (Pitfall 2)"
  - "Report::failure выбирает код по приоритету PREV_GAME_STUCK > FOREIGN_GAME_RUNNING > PRECHECK_FAILED; подсказка — первого проваленного пункта, пустая заменяется подсказкой кода"
  - "StateFlags: идёт обновление (маска 256…8388608) и 32/128, а также отсутствие бита 4 без 2/512 — fail; 2/512, недокачанная целевая сборка или недокачанные байты — warn; 64 игнорируется; нет файла или флагов — warn"
  - "Сборка CS2 для сравнения — только buildid манифеста; PatchVersion из steam.inf идёт лишь в текст сообщения"
  - "Версия ffmpeg разбирается как старший номер из первой строки (допустим префикс n); git-сборки N-… не разбираются и дают warn, а не fail"

patterns-established:
  - "RecordState::Precheck в limit() возвращает None: машина записи в это состояние не входит, его сообщает приложение событием Event::State"

requirements-completed: []

duration: 25 min
completed: 2026-10-10
status: complete
---

# Phase 2 Plan 03: Чистая часть предпроверки Summary

**Чистая оценка `precheck::evaluate(&Facts) -> Report` превращает факты окружения (процессы, Steam и сборка CS2, утилиты, демка, путь, место на дисках) в пункты ok/warn/fail по-русски, а провал даёт типизированный сбой `PRECHECK_FAILED` / `FOREIGN_GAME_RUNNING` / `PREV_GAME_STUCK` на новой первой ступени лестницы.**

## Performance

- **Duration:** около 25 мин
- **Tasks:** 2 (1 tracer + 1 auto)
- **Files modified:** 9 (3 новых модуля)

## Accomplishments

- `precheck/mod.rs`: `Level`/`Item`/`Report` (сериализуются в JSON для базы и карточки), `Facts` со всеми фактами «не проверялось» по умолчанию, `evaluate`, `space_items`. Пункты: своя прошлая игра, чужой `cs2.exe`, Steam, обновление CS2, сборка CS2 против последней удачной, `HLAE.exe`, `AfxHookSource2.dll`, `cs2.exe`, `ffmpeg` (7+), `ffprobe`, ffmpeg HLAE, демка (нет файла, LFS-указатель, не демка, сжатая), путь результатов, место на дисках результатов и CS2.
- `Report::failure(elapsed)` даёт `RecordFailure` на `RecordState::Precheck` с сообщением «предпроверка: …» и подсказкой; `warn` старт не блокирует.
- `precheck/steam.rs`: построчный разбор `appmanifest_730.acf` и `steam.inf` без внешних крейтов, `steam_state` по битам `StateFlags` (DIF-02).
- `precheck/space.rs`: ≈10 МБ видео на игровую секунду при 1080p60 по площади кадра и fps записи + 0.192 МБ/с звука, запас ×1.5, минимумы 3 ГиБ / 1 ГиБ до плана, `same_volume` по строке пути (работает на ubuntu).
- `FailCode` получил три кода с подсказками; `RecordState::Precheck` стала первой ступенью `LADDER` (теперь 8 ступеней, карточка задачи читает её без правок).
- `Deadlines`: пробы утилит/NVENC/превью растут с «Запасом по времени», бюджеты отмены и выхода игры (`quit_wait` 5 с, `kill_confirm` 3 с, `window_poll` 1 с, `prev_game_exit` 30 с, `next_job_settle` 3 с) фиксированы.

## Task Commits

1. **Task 1: отчёт предпроверки, коды сбоя, ступень Precheck** - `5ad983e` (feat)
2. **Task 2: пункты окружения, steam, space, дедлайны** - `2414a08` (feat)

## Verification

- `cargo test --release -p pipeline --lib precheck::`: 24 passed (6 в Task 1, 18 после Task 2; цель плана — не меньше 20).
- `cargo test --release -p pipeline --lib record::`: зелёный (14 passed после Task 1); `record::deadlines`: 4 passed, включая `cancel_and_exit_budgets_do_not_grow_with_the_time_margin`.
- `cargo test --release -p pipeline -p cli`: зелёный (в т. ч. `crates/cli/tests/pipeline.rs`, `crates/pipeline/tests/record.rs`).
- `cargo build --release --workspace`: зелёный.
- `Cargo.lock`: только две строки зависимостей крейта `pipeline` (`serde`, `serde_json`), новых пакетов нет.

## Deviations from Plan

None - plan executed exactly as written.

## Requirements

`requirements-completed` оставлен пустым намеренно. Здесь реализована только чистая оценка фактов; сбор фактов на хосте (02-05) и встраивание в задачу и кнопку «Проверить окружение» (02-06 и далее) ещё не сделаны, поэтому PRE-01, PRE-03…PRE-07, DIF-01, DIF-02, TMO-07 пользователь пока не получает. `requirements.ready-ids` подтверждает: 0 из 9 готовы, их же заявляют планы 02-05…02-08.

## Known Stubs

None.

## Threat Flags

None. Новых сетевых точек, путей аутентификации и обращений к файлам нет: разбор манифестов и вывода утилит работает над строками, переданными снаружи; в отчёт попадают пути, версии и PID.

## Next Phase Readiness

Готово к 02-05 (сбор `Facts` на хосте: процессы, свободное место, версии утилит, чтение манифеста) и 02-06 (вызов `evaluate`, запись `Report::to_json` в задачу, `Report::failure` как сбой записи).

## Self-Check: PASSED

- Файлы `precheck/mod.rs`, `precheck/steam.rs`, `precheck/space.rs` существуют; коммиты `5ad983e` и `2414a08` — предки `HEAD`.
- Критерии приёмки обеих задач проверены командами `grep` и `cargo test`, все выполнены.
