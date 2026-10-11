---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 10
subsystem: acceptance
tags: [rust, cli, app, acceptance, ffprobe, sqlite, read-only]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "Store::open_read_only/jobs_in_batch/last_batch/set_batch и auto_retries (04-02), journal::attempts/outcomes (04-02), verify::probe_args/parse_probe/within_tolerance (фаза 3), политика очереди и StartKind::Auto (04-09)"
provides:
  - "pipeline::acceptance: SET_SIZE, SET_ENV, EMBEDDED, parse_set, load_set, require_full, batch_name, batch_pid; acceptance/matches.txt (шаблон без ссылок)"
  - "cs2-cli acceptance check: машинная проверка партии (evaluate — чистая, check_with — по файлу базы только на чтение), отчёт acceptance-<партия>.json"
  - "cs2-cli acceptance state / job <id> / set: улики учений и набора"
  - "app: acceptance_jobs (чистая), enqueue_acceptance, Msg::AcceptanceQueued, кнопка «Поставить приёмочный набор» на странице «Задачи»"
affects: [04-11, 04-12]

actuals:
  tokens: 24000
  tasks: 3
  commits: 3

plan_head_before: 8e75445084fe7cb48e735c5a82831dac93d2ad45
plan_head_after: 481d034e3d5154cc55282a501919d01ae16d5151
commits: 3

tech-stack:
  added: ["chrono (уже в workspace) и store как зависимости крейта cli"]
  patterns:
    - "Проверка = чистая функция над снятым состоянием (CheckInput/StateInput) + тонкие обёртки, читающие базу, файлы и процессы; ffprobe и снимок процессов внедряются замыканиями"
    - "База читается только Store::open_read_only; отчёт пишется отдельным файлом рядом с базой с безопасным именем"

key-files:
  created:
    - acceptance/matches.txt
    - crates/pipeline/src/acceptance.rs
    - crates/cli/src/acceptance.rs
  modified:
    - crates/pipeline/src/lib.rs
    - crates/cli/Cargo.toml
    - crates/cli/src/main.rs
    - crates/app/src/app.rs
    - crates/app/src/jobs_page.rs
    - Cargo.lock

key-decisions:
  - "Набор хранится в репозитории и встраивается include_str!; CS2CINEMA_ACCEPTANCE подменяет его файлом без пересборки; ссылки автор вписывает в 04-12"
  - "«Ручных действий не было» выводится только из сохранённых данных: разделители попыток в журнале (First/Auto) и равенство attempt == 1 + auto_retries"
  - "Две серии (DC-9): серия B доказана другим PID в имени партии и тем, что её задачи поставлены позже конца серии A (updated_at задач A)"
  - "Допуск длительности ролика — ±10% не меньше 0.5 с, как у дублей; ожидаемая длительность — video_secs из record.json задачи; подстраивается по итогам серии A в 04-12"
  - "Диспетчер подкоманд acceptance лежит в main.rs (check/state/job/set), функции модуля читают env::args сами"

patterns-established:
  - "Список наших файлов в папке игры в выводе state режется до 10 имён с общим счётом: на машине автора лежит 127 остатков старых версий"

requirements-completed: [ACC-01]
requirements-tooling-delivered: [ACC-02, ACC-03]

coverage:
  - id: D1
    description: "Приёмочный набор — файл в репозитории, встроенный в обе программы; строгий разбор строк, пределы и повторы отвергаются, источник виден"
    requirement: ACC-01
    verification:
      - kind: unit
        ref: "crates/pipeline/src/acceptance.rs#parse_set_rejects_garbage_and_repeats_with_the_line_number"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/acceptance.rs#embedded_template_is_an_empty_set"
        status: pass
      - kind: unit
        ref: "crates/pipeline/src/acceptance.rs#load_set_reads_the_file_named_by_the_variable"
        status: pass
  - id: D2
    description: "Кнопка «Поставить приёмочный набор» ставит 5 задач записи на 5 разных картах про игрока из настроек одной партией; при любой неполадке набора не ставится ни одна"
    requirement: ACC-01
    verification:
      - kind: unit
        ref: "crates/app/src/app.rs#acceptance_jobs_need_five_distinct_maps_and_the_player"
        status: pass
      - kind: manual_procedural
        ref: "живая постановка кнопкой с настоящим ключом FACEIT и пятью ссылками — серия A в 04-12"
        status: unknown
  - id: D3
    description: "cs2-cli acceptance check подтверждает партию машинно по всем критериям и не меняет базу"
    requirement: ACC-02
    verification:
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#five_good_jobs_pass_without_auto_retries"
        status: pass
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#manual_retry_or_redo_in_the_journal_fails"
        status: pass
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#an_auto_retried_job_that_finished_passes_and_reports_the_failed_attempt"
        status: pass
      - kind: integration
        ref: "crates/cli/src/acceptance.rs#check_on_a_real_database_fails_without_changing_it"
        status: pass
  - id: D4
    description: "--expect-restart-from подтверждает, что серия B шла в другом процессе приложения после конца серии A"
    requirement: ACC-02
    verification:
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#series_b_must_come_from_another_process_and_start_after_series_a"
        status: pass
  - id: D5
    description: "acceptance state / job / set машинно подтверждают чистое состояние после учения и показывают коды сбоев и журнал"
    requirement: ACC-03
    verification:
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#every_dirty_thing_is_a_violation_with_its_own_line"
        status: pass
      - kind: unit
        ref: "crates/cli/src/acceptance.rs#job_text_shows_the_row_and_the_whole_journal"
        status: pass
      - kind: other
        ref: "cs2-cli acceptance state / job 11 / set на настоящей базе автора (только чтение): печатает задачи, 127 остатков в папке игры, код 1; job 999 — код 2"
        status: pass
  - id: D6
    description: "Workspace тестируется и собирается, cs2-cli проходит crates/cli/tests/pipeline.rs"
    verification:
      - kind: other
        ref: "cargo test --release --workspace"
        status: pass
      - kind: other
        ref: "cargo check --release --workspace; cargo build --release -p cli"
        status: pass
---

# Phase 4 Plan 10: инструменты приёмки Summary

**Одна команда `cs2-cli acceptance check` по базе приложения (только чтение) отвечает «приёмка пройдена или нет и почему»; `acceptance state/job/set` дают улики для учений; кнопка на странице «Задачи» ставит пять задач приёмки одной партией.**

## Выполнено

1. **Task 1 (tracer), `09a4a22`.** `acceptance/matches.txt` (шаблон только из комментариев), `pipeline::acceptance` (разбор набора строгим `faceit::parse_match_id`, пределы 64 КиБ и 100 строк, повторы, `load_set` с `CS2CINEMA_ACCEPTANCE`, `batch_name`/`batch_pid`). `cli::acceptance::evaluate` — чистая проверка: состав партии против набора, 5 разных карт (из `plan.json`, иначе `job.map`), статус «готово», ролик `highlights.mp4` через ffprobe (видео и звук, длительность в допуске от `record.json.video_secs`), снимок процессов без `cs2.exe`/`HLAE.exe`, ручные «Повторить»/«Заново» по журналу, `attempt == 1 + auto_retries`, итоги упавших попыток авто-повторов отдельным списком, две серии. `check_with` открывает базу только `Store::open_read_only`; отчёт `acceptance-<партия>.json` пишется рядом с базой под безопасным именем. Коды выхода 0/1/2.
2. **Task 2, `6364a08`.** `state_report` (чистая) и `state`: задачи «выполняется»/«отменяется», процессы, наши `cs2cinema_*.dem` и скрипты в `game/csgo[/cfg]`, `installed.json` в папках задач; папка CS2 из `--cs2`, `CS2_DIR` или поля `cs2_dir` в `settings.json`; последние 10 задач с кодами сбоев. `job <id>` печатает строку задачи и журнал целиком, `set` — источник и id набора (код 0 только при 5 матчах). В рабочем коде нет удаления файлов и завершения процессов.
3. **Task 3, `481d034`.** `acceptance_jobs` (чистая), `enqueue_acceptance` (фоновый поток: набор, игрок, статистика из кэша `stats:{id}` или FACEIT, проверка, постановка пяти задач с `set_batch`, откат поставленных при сбое базы), `Msg::AcceptanceQueued`, кнопка в шапке страницы «Задачи» (неактивна, пока постановка не вернулась).

## Проверки

- `cargo test --release -p pipeline --lib acceptance`: 7 тестов, 0 упало.
- `cargo test --release -p cli --bin cs2-cli acceptance`: 27 тестов, 0 упало (включая сквозной на файле базы: байты базы и `-wal` до и после проверки совпадают, писатель видит прежнюю задачу).
- `cargo test --release -p app --bin cs2cinema acceptance`: 1 тест, 0 упало.
- `cargo test --release --workspace`: все наборы зелёные (в том числе `crates/cli/tests/pipeline.rs`).
- Критерии по тексту: шаблон без ссылок (`grep` выводит 0); `Store::open(` в рабочем коде `cli/acceptance.rs` — 0; `remove_file|remove_dir|kill_orphan` в рабочем коде — 0.
- Живой прогон на настоящей базе автора (только чтение): `acceptance state` напечатал последние задачи и вердикт «НЕ ЧИСТО», `acceptance job 11` — строку и журнал, `job 999` — код 2, `check` без партий — код 2 с понятной причиной, `set` — «0 матчей из 5», код 1.
- Полная `cargo build --release --workspace` не прошла по причине окружения, не кода: `target\release\cs2cinema.exe` занят запущенным приложением автора (os error 5). Вместо неё `cargo check --release --workspace` и `cargo build --release -p cli` без ошибок.

## Deviations from Plan

**1. [Rule 3 - зависимость] `chrono` в `crates/cli/Cargo.toml`**
- План называл одну новую зависимость (`store`). Для вывода «обновлена ЧЧ:ММ:СС» по местному времени в `state` и `job` добавлен `chrono` с теми же версией и признаками, что уже в workspace (`pipeline`, `app`). Нового пакета в `Cargo.lock` нет.

**2. [Rule 3 - сигнатуры] Подкоманды читают `env::args` сами, диспетчер в `main.rs`**
- План перечислял `check_cmd()`, `state_cmd()`, `job_cmd()`, `set_cmd()` без аргументов и требовал `"state"`, `"job"`, `"set"` в `main.rs`; диспетчер `match` лежит в `main.rs`, неизвестная подкоманда и `--help` идут в `acceptance::usage_cmd`.

**3. [Rule 2 - полнота] Отчёт без JSON-структуры `serde`**
- У `cli` нет прямой зависимости на `serde`, поэтому отчёт собирается `serde_json::json!`, а `Report`/`Row` — обычные структуры. Состав полей отчёта соответствует плану.

**4. [Rule 2 - полнота] Предел списка файлов игры в `state`**
- На машине автора в папке игры лежат 127 остатков старых версий, и перечисление по именам завалило бы вывод. Печатается общий счёт и первые 10 имён (тест `a_long_list_of_game_files_is_counted_and_cut`).

**5. [Rule 2 - безопасность] Имя отчёта санитизируется**
- Имя партии приходит из базы или аргумента `--batch`; в имени файла оставлены только `A-Za-z0-9-_` (до 80 символов), чтобы отчёт не вышел из папки базы.

**6. Карта из `plan.json` читается как JSON-значение**
- Вместо полной десериализации `director::Timeline` берётся только поле `map` (файл больше 32 МиБ не читается): проверка не должна падать из-за незнакомых полей плана.

Иных отклонений нет.

## Находки для следующих планов

- Состояние машины автора перед учениями не чистое: в `game/csgo` лежат `cs2cinema_*.dem` (8 шт.) и в `game/csgo/cfg` — `cs2c_*` (около 119 шт.) от старых версий приложения; `acceptance state` вернёт 1 и покажет их. Перед первым учением 04-11 их нужно один раз убрать вручную (команда `state` ничего не удаляет), иначе вердикт «ЧИСТО» недостижим.
- В базе автора у старых задач `attempt = 0` и нет партии: приёмка смотрит только на партии `acc-…`.
- Для 04-12: `acceptance/matches.txt` пока без ссылок — кнопка и `acceptance set` честно отвечают «0 матчей из 5»; после правки нужна пересборка `app` и `cli` либо `CS2CINEMA_ACCEPTANCE=<файл>`.
- Для 04-12: допуск длительности ±10% и источник ожидания (`video_secs`) подстраиваются по итогам серии A; проверка вывода реального ffprobe на настоящем `highlights.mp4` ещё не выполнялась (в тестах ffprobe подставной).

## Known Stubs

Нет. `acceptance/matches.txt` без ссылок — намеренный шаблон, ссылки вписываются в плане 04-12 (записано в плане и в самом файле).

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. T-04-24 (строгий разбор и пределы набора, повторы, постановка только целиком), T-04-25 (только `open_read_only`, сквозной тест неизменности базы), T-04-26 (в отчёте только id, карты, статусы, длительности, коды и итоги попыток; из `settings.json` читается лишь поле `cs2_dir`), T-04-27 (только чтение; ffprobe через `Host::run` с вектором аргументов и дедлайном) выполнены.

## Self-Check: PASSED

- Файлы найдены: `acceptance/matches.txt`, `crates/pipeline/src/acceptance.rs`, `crates/cli/src/acceptance.rs`.
- Коммиты `09a4a22`, `6364a08`, `481d034` лежат в истории `main`.

## Заметка по требованиям

`requirements.mark-complete` отметил ACC-01 и ACC-03; ACC-03 («учения проводятся вручную») возвращён в «Pending» вручную: инструмент проверки `acceptance state` готов, но сами учения проводятся в 04-11. ACC-02 в `REQUIREMENTS.md` уже стоял «Complete» до этого плана; его живое подтверждение (две серии по 5 матчей, `acceptance check` с кодом 0) выполняется в 04-12 — стоит вернуть в «Pending» до тех пор, если нужна строгость.
