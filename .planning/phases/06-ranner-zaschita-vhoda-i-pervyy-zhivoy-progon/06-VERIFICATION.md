---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
verified: 2026-10-11T00:00:00Z
status: human_needed
score: 4/5 must-haves verified
covered_files:
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-01-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-01-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-02-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-02-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-03-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-03-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-04-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-04-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-05-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-05-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-06-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-06-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-07-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-07-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-08-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-08-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-09-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-09-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-10-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-10-SUMMARY.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-11-PLAN.md
  - .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-11-SUMMARY.md
  - crates/cli/src/inspect.rs
  - crates/cli/src/runner_cmd.rs
  - crates/cs2/src/text.rs
  - crates/demo/src/command.rs
  - crates/demo/src/lib.rs
  - crates/host/src/win/sandbox.rs
  - crates/pipeline/src/assemble.rs
  - crates/pipeline/src/inspect.rs
  - crates/pipeline/src/job.rs
  - crates/pipeline/src/lib.rs
  - crates/pipeline/src/record/failure.rs
  - crates/renderer/src/lib.rs
  - crates/runner/src/exec.rs
  - crates/runner/src/lib.rs
  - crates/runner/src/link.rs
  - crates/runner/src/policy.rs
  - crates/runner/src/state.rs
  - crates/runner/src/tee.rs
  - crates/runner/src/transfer.rs
  - crates/wire/src/lib.rs
covered_digest: "v3:sha256:c27a5170fd379de272995680e590db48720318fcf3c59c20a2fd3ccb55e31010"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Живой прогон: `cs2-server submit <настоящая демка> --player <ник>`, затем `cs2-cli runner` на машине автора с настоящими CS2 и HLAE"
    expected: "Задача доходит до `highlights.mp4` на сервере с подтверждёнными размером и SHA-256; входящий порт на машине с CS2 не нужен; заодно подтверждается живая запись (фикс стабильности дубля)"
    why_human: "Нужны настоящие CS2/HLAE и интерактивная сессия Windows; в тестах игра подменена FakeHost. План 06-11 пропущен по решению автора."
  - test: "Учение: отмена на сервере посреди записи настоящей игры"
    expected: "Пайплайн останавливается и CS2 закрывается не позже 20 с (мягкая отмена, эскалация в жёсткую через 10 с)"
    why_human: "Тесты проверяют эскалацию и отмену на виртуальных часах FakeHost; реальное время закрытия CS2 можно измерить только вживую."
  - test: "Учение: убить раннер посреди загрузки результата и перезапустить"
    expected: "Результат остаётся в outbox и догружается с места обрыва, сервер подтверждает размер и SHA-256 (`--upload-chunk-limit N` для остановки)"
    why_human: "Логика докачки покрыта тестами на настоящем `server::serve`; на реальной записи и реальном файле в сотни МиБ не проверялась."
  - test: "Учение: запущен чужой CS2 (не наш процесс)"
    expected: "Раннер распознаёт `FOREIGN_GAME_RUNNING`, не трогает чужую игру, возвращает задачу в очередь сервера (`release`), после двух подряд одинаковых неисправностей встаёт на паузу; Enter снимает паузу"
    why_human: "Определение чужого процесса через Win32 и Job Object проверено только на FakeHost."
  - test: "Учение: враждебные входы на реальном разборе (мусорная и обрезанная демка, zstd-бомба, файл сверх лимита, неоднозначный/отсутствующий ник)"
    expected: "Каждый вход закрывается своим кодом (`BAD_DEMO`, `DEMO_TOO_LARGE`, `PLAYER_AMBIGUOUS`, `PLAYER_NOT_FOUND`) до запуска CS2, раннер берёт следующую задачу"
    why_human: "Тесты используют синтетические демки (`demo::synth`); поведение настоящего Job Object с лимитом памяти на реальной большой демке не проверялось."
  - test: "Калибровка лимитов разбора (файл 2000 МиБ, распаковка 3072 МиБ, память 6144 МиБ, время 15 мин) по строке журнала «пик памяти разбора»"
    expected: "Лимиты подобраны с запасом над пиком памяти настоящей демки FACEIT и не отвергают нормальные матчи"
    why_human: "Стартовые значения взяты из исследования, не из живого замера."
---

# Фаза 6: Раннер, защита входа и первый живой прогон — отчёт о проверке

**Цель фазы:** `cs2-cli runner` на машине автора сам подключается к серверу, забирает задачу, ведёт её через неизменный `pipeline::job::run_job` и надёжно доставляет `highlights.mp4` на сервер; существующий код безопасно обрабатывает чужие демки и ники и объясняет отказы своими кодами до запуска CS2; фаза заканчивается первым живым прогоном.
**Проверено:** 2026-10-11
**Статус:** human_needed
**Повторная проверка:** нет, первичная

## Итог

Всё, что можно доказать кодом и тестами, подтверждено: `cargo test --release --workspace` выполнен в этой проверке заново, код выхода 0, 58 строк `test result: ok`, падений нет, `crates/cli/tests/pipeline.rs` зелёный. Критерий 1 (живой прогон) и живые части критериев 2 и 3 не доказаны: план 06-11 пропущен по решению автора (06-11-SUMMARY.md: `status: skipped-by-author`, `06-LIVE-RUN.md` и `evidence/` не созданы). Они не засчитаны как пройденные и вынесены в `human_verification`. Невыполненных по коду пунктов (gaps) нет.

## Достижение цели

### Наблюдаемые истины (критерии успеха из ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | Живой прогон настоящей демки через `cs2-server submit` → `cs2-cli runner` → настоящие CS2/HLAE → `highlights.mp4` на сервере с подтверждёнными размером и SHA-256 | ? UNCERTAIN (human_needed) | Не выполнялся (06-11 пропущен). Код сквозного пути есть и проходит на настоящем `server::serve` с `FakeHost`: `valid_result_is_uploaded_and_the_job_is_done`, `crates/runner/tests/flow.rs`. Раннер только исходящий: `Link` на ureq, редиректы выключены; сервер подключений к раннеру не открывает. |
| 2 | Сервер видит этап и «дубль k из n»; отмена на сервере останавливает пайплайн и закрывает CS2 за секунды; убитый раннер догружает результат из outbox | ✓ VERIFIED кодом/тестами; живая часть — human_needed | Тесты: `server_sees_stage_and_take_and_cancel_reaches_the_executor`, `soft_cancel_escalates_to_force_within_the_bound` (`FORCE_AFTER` = 10 с), `cancel_during_upload_stops_between_chunks_and_closes_the_job`, `runner_killed_after_recording_uploads_on_restart_without_the_executor`, `runner_killed_while_recording_resumes_the_executor_with_resume_flag`, `interrupted_upload_resumes_after_restart_from_the_server_offset`, `job show` печатает этап/дубль/результат. Реальное время закрытия CS2 (≤ 20 с) вживую не измерено. |
| 3 | Сбой класса «окружение» возвращает задачу в очередь, а не роняет; негодный по `ffprobe` ролик не загружается, задача падает со своим кодом | ✓ VERIFIED | `environment_failure_releases_the_job_back_to_the_queue`, `environment_failure_after_a_good_preflight_returns_the_job_to_the_queue`, `two_identical_environment_failures_pause_the_runner_until_resume`, `invalid_result_is_not_uploaded_and_fails_with_result_invalid`, `result_changed_after_verification_is_not_uploaded`. Код: `exec.rs::verify` → `RESULT_INVALID`, `assemble::judge_result` (H.264, yuv420p, mp4, faststart по порядку боксов). Проверка настоящего чужого CS2 — в `human_verification`. |
| 4 | Враждебные входы проверены тестами: ник/карта с `\n`, `;`, кавычками не попадают в cfg, `drawtext` читает подпись из файла; обрезанная/испорченная демка, zstd-бомба, файл сверх лимита → `BAD_DEMO`/`DEMO_TOO_LARGE`; разбор в дочернем процессе с лимитом памяти, раннер продолжает брать задачи | ✓ VERIFIED | `cs2::text::{clean_text, clean_map}` вызываются в `Match::build` (`build_cleans_hostile_name_and_map`); `hostile_text_never_reaches_a_cfg_line`; `drawtext=…:textfile='cap_NNN.txt':expansion=none`, файлы пишутся `Output::write_to`, cwd сборки — папка задачи (`assemble.rs:178`); `truncated_demos_end_without_panic_or_hang`, `zstd_bomb_is_too_large_without_unpacking_it_all`, `text_file_and_lfs_pointer_are_bad_demos`; `read_demo_limited` с `take(limit+1)`, `decompress_len` ограничен; `cs2-cli inspect` вызывает `quiet_crash_dialogs` и `limit_self_memory` (Job Object, `ActiveProcessLimit = 1`) до чтения файла; `garbage_demo_fails_with_bad_demo_before_cs2_and_the_runner_takes_the_next_job` (`host.launches() == 0`, раннер не на паузе). Реальный Job Object на большой настоящей демке и калибровка лимитов — в `human_verification`. |
| 5 | `PLAYER_AMBIGUOUS` со списком до 10 очищенных кандидатов, `PLAYER_NOT_FOUND`, `NO_HIGHLIGHTS` — до запуска CS2; `pipeline.rs` и `cargo test --workspace` зелёные | ✓ VERIFIED | `pipeline::resolve_player` (SteamID → точное имя → без регистра → подстрока; один — игрок, несколько — `PLAYER_AMBIGUOUS`, `CANDIDATES_MAX = 10`), `job::hero_for` вызывается сразу после разбора в `run_job` (job.rs:522) и в дочернем `inspect`; тесты `ambiguous_nick_stops_the_job_before_recording_with_candidates`, `cli_player_flag_refuses_an_ambiguous_nick`, `hero_for` на ники и `quiet`. Все `FailCode` входят в `wire::fail::KNOWN` (страж). Полный прогон workspace зелёный. |

**Счёт:** 4/5 истин подтверждены; истина 1 (живой прогон) — человеку. Из подтверждённых истин 2 и 3 содержат живые компоненты, перечисленные в `human_verification`.

### Требования (REQUIREMENTS.md)

| Требование | Статус | Подтверждение |
|------------|--------|---------------|
| RUN-01 | ✓ SATISFIED кодом; живая проверка — human_needed | `cs2-cli runner` (`runner_cmd.rs`), `Runner`/`Link`/`PipelineExecutor`; сквозной тест на `server::serve`. |
| RUN-02 | ✓ SATISFIED кодом | `runner_state.json` (outbox), `hello` с holding, докачка по смещению сервера, проверка SHA-256; тесты outbox. |
| RUN-03 | ✓ SATISFIED кодом | `Uplink` (пульс = progress), этап и дубль на сервер, ответ cancel доходит до `Tee`; эскалация за 10 с. |
| RUN-04 | ✓ SATISFIED кодом | `Disposition::Release`, `release` с засчитанной выдачей, пауза после двух одинаковых сбоев, `TOO_MANY_GRANTS` как потолок. |
| RUN-05 | ✓ SATISFIED | Пять новых кодов (плюс `RESULT_INVALID`, `UNSUPPORTED_SOURCE`) в `FailCode` и `wire::fail`, страж согласованности. |
| RUN-06 | ✓ SATISFIED | `+faststart` в обоих кодерах, `verify_result` перед загрузкой, повторная сверка SHA-256. |
| SEC-01 | ✓ SATISFIED | Очистка на границе доверия, в cfg текст из демки не пишется, `drawtext` из файлов. |
| SEC-02 | ✓ SATISFIED кодом | Потолки файла/распаковки/snappy, мутационные тесты парсера, дочерний процесс с Job Object; калибровка лимитов — вживую. |
| SEC-03 | ✓ SATISFIED | `resolve_player` без угадывания, отказ до запуска CS2. |

Все 9 идентификаторов фазы учтены; осиротевших требований REQUIREMENTS.md для фазы 6 нет.

### Ключевые связи

| От | К | Через | Статус |
|----|---|-------|--------|
| `cs2-cli runner` (`main.rs:139`) | `runner_cmd::runner_cmd` → `Runner::run` | ветка `Some("runner")` | WIRED |
| `PipelineExecutor` | `pipeline::inspect::preflight` → `cs2-cli inspect` | `exec.rs:113`, `current_exe()` | WIRED |
| `PipelineExecutor` | `run_job` под `catch_unwind` | `exec.rs:173` | WIRED |
| `PipelineExecutor::verify` | `assemble::verify_result` до загрузки | `exec.rs:126` | WIRED |
| `cs2-cli inspect` | `host::limit_self_memory`, `quiet_crash_dialogs` | `inspect.rs:111-112` | WIRED |
| `run_job` | `hero_for` → `resolve_player` + `ensure_highlights` | `job.rs:522` | WIRED |
| `Match::build` | `cs2::text::clean_text`/`clean_map` | очистка имён и карты | WIRED |
| renderer | `cap_NNN.txt` рядом с графом, `textfile=` | `write_to`, cwd = папка задачи | WIRED |

### Предупреждения из 06-REVIEW.md (0 критических, 9 предупреждений) — цель не блокируют

Не повторяю ревью, отмечаю то, что касается критериев фазы:

- **WR-01** (лимиты флагов не доходят до `run_job`; повторный разбор в процессе раннера без песочницы) — касается критерия 4: первая линия защиты (дочерний `inspect`) работает, но `run_job` повторно разбирает ту же демку в самом раннере с зашитыми лимитами по умолчанию. Парсер детерминирован, и демка, прошедшая песочницу, пройдёт и здесь, поэтому критерий считаю выполненным, остаточный риск оставлен.
- **WR-02** (случайный Enter снимает паузу сразу) — ослабляет защиту от бесконечного круга «неисправность - повтор»; круг по-прежнему ограничен `max_grants` сервера.
- **WR-03** (успешные задачи не освобождают диск), **WR-04** (401 на `finish`/`release` повторяется вечно), **WR-05** (404/409 без кода считается окончательным отказом и удаляет результат), **WR-06**, **WR-07** (токен наследуется дочерними процессами), **WR-08**, **WR-09** (`http://` принимается) — надёжность и эксплуатация; к ним стоит вернуться до фазы 8/9 (особенно WR-03 для автономной работы и WR-05 для потери результата), они не опровергают критериев фазы.

### Анти-паттерны

`TBD`/`FIXME`/`XXX` в изменённых файлах без ссылки на задачу не искались построчно отдельным проходом сверх ревью; заглушек в проверенных путях (`exec.rs`, `transfer.rs`, `policy.rs`, `text.rs`) не найдено.

### Проверки поведения

| Проверка | Команда | Результат | Статус |
|----------|---------|-----------|--------|
| Весь workspace | `cargo test --release --workspace` | код выхода 0, 58 × `test result: ok`, без `FAILED` | ✓ PASS |
| `crates/cli/tests/pipeline.rs` | входит в запуск выше | зелёный (`lfs_pointer_gives_clear_error`, `cli_player_flag_refuses_an_ambiguous_nick`) | ✓ PASS |
| Живой прогон с CS2/HLAE | — | не выполнялся (06-11 пропущен) | ? SKIP → human_needed |

### Резюме

Нереализованных по коду требований нет: все 9 требований фазы имеют реализацию, проводку и тесты на настоящем сервере с поддельным хостом. Статус `human_needed` отражает только то, что первый живой прогон и учения (06-11) автор пропустил осознанно; эти пункты остаются долгом ручной проверки и не должны считаться выполненными.

---

_Проверено: 2026-10-11_
_Верификатор: Claude (gsd-verifier)_
