---
phase: 02-predproverka-otmena-i-tolko-svoi-protsessy
verified: 2026-10-10T22:30:00Z
status: passed
score: 5/5 must-haves verified
covered_files:
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-01-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-01-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-02-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-02-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-03-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-03-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-04-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-04-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-05-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-05-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-06-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-06-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-07-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-07-SUMMARY.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-08-PLAN.md"
  - ".planning/phases/02-predproverka-otmena-i-tolko-svoi-protsessy/02-08-SUMMARY.md"
  - "crates/app/src/app.rs"
  - "crates/app/src/jobs.rs"
  - "crates/app/src/launch.rs"
  - "crates/host/src/win/mod.rs"
  - "crates/pipeline/src/precheck/gather.rs"
  - "crates/pipeline/src/precheck/mod.rs"
  - "crates/pipeline/src/record/machine.rs"
  - "crates/pipeline/src/record/mod.rs"
  - "crates/store/src/lib.rs"
covered_digest: "v3:sha256:bbcf7cbc32af0375db9131b5d1e6b8b5a713c229e800171abfd3feb6f79de481"
behavior_unverified: 0
overrides_applied: 0
---

# Фаза 2: Предпроверка, отмена и только свои процессы — отчёт о проверке

**Цель фазы:** запись не начинается в неисправном окружении, не зависает на внешних процессах, отменяется за секунды в любом состоянии и завершает только те `cs2.exe` и `HLAE.exe`, которые запустило само приложение.
**Проверено:** 2026-10-10
**Статус:** passed (с замечаниями, ни одно не блокирует)
**Повторная проверка:** нет, первичная

Вывод построен по коду, а не по SUMMARY. Прогнан `cargo test --release --workspace`: 279 passed, 0 failed, 1 ignored (`host` стенд-диалог `modal`, нужен рабочий стол; по SUMMARY 02-01 он прогонялся вручную и прошёл), код выхода 0. `crates/cli/tests/pipeline.rs` проходит (2 passed), то есть совместимость с CLI сохранена.

## Достижение цели

### Наблюдаемые истины (критерии успеха ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | Предпроверка идёт первой (до скачивания, разбора, плана), единый отчёт ok/warn/fail с сообщением и подсказкой; `fail` не даёт начать запись; «только план» её пропускает; смена сборки CS2 даёт warn | VERIFIED | `app/src/jobs.rs::run` вызывает `precheck_stage` до `pipeline::analyze`; `needs_precheck(kind)` = `kind != Plan`; `Report::blocks()` и `Report::failure()` превращают `fail` в `RecordFailure` на ступени `Precheck`, `run` возвращает `Err` через `?`. Отчёт пишется в `jobs.precheck` (схема v3) и рисуется в карточке (`jobs_page::precheck_view`). `steam_items` в `precheck/mod.rs` даёт warn «сборка CS2 изменилась…» при `last_good != buildid`; `set_last_good` вызывается после успешной записи. Тесты `pipeline/tests/precheck.rs` (19) зелёные. |
| 2 | Отчёт ловит типовые поломки: нет HLAE.exe, AfxHookSource2.dll, cs2.exe; ffmpeg, ffprobe и ffmpeg из `ffmpeg.ini` не отвечают на `-version`; Steam не запущен или обновляет CS2; не хватает места (результаты, диск CS2, оценка по плану); LFS-указатель; путь с пробелами или недоступный для записи | VERIFIED | `precheck/gather.rs::run` и `evaluate`: файлы по `is_file`, три `probe()` через `Host::run` с дедлайном `tool_probe`; Steam по снимку процессов (`Host::processes` -> ToolHelp, не текст tasklist); `steam::steam_state` по `StateFlags` с маской обновления; `space_items` с общей суммой на одном томе и `check_space` вторым проходом по плану (`jobs::space_pass` до запуска игры); `demo_item` распознаёт `version https://git-lfs`; `out_item` проверяет `console_safe` и пробную запись, файл-проба удаляется. Каждая ветка покрыта тестом. |
| 3 | Чужой `cs2.exe` -> задача не стартует, чужой процесс жив; завершаются только свои `cs2.exe`/`HLAE.exe` через Job Object, `taskkill /IM` не вызывается; следующая задача только после подтверждённого выхода прошлого `cs2.exe` | VERIFIED | Пункт `foreign_game` = `fail` -> `FOREIGN_GAME_RUNNING`; в `host::Host` нет метода «завершить по PID/имени» (тест `host_trait_has_no_way_to_stop_a_process_by_pid_or_name`); тест-страж `no_process_is_ever_stopped_by_image_name` проходит: во всех `*.rs` нет `taskkill`, `tasklist`, `Stop-Process`, `pkill`, `killall`. Своё закрывается `Job::terminate` (KILL_ON_JOB_CLOSE) и `OwnedProc::kill` по собственному handle (`wait_previous_game`, игра, принятая без job). `Host::wait_previous_game` в начале `precheck::run` ждёт handle прошлой игры из реестра `OWN_GAMES`, затем пауза `next_job_settle`; `Stuck` -> `PREV_GAME_STUCK`. |
| 4 | Отмена в любом состоянии: «отменяется» за 3 с, чистое состояние за ~10 с; поэтапное закрытие `quit` -> ожидание -> завершение своего -> проверка; `.partial` для оборванных дублей; уборка `cs2c_*.cfg`, `highlights.cfg`, копии демки; ни одного `Command::output()` без дедлайна на пути записи | VERIFIED | «Отменяется»: `App::cancel_job` в том же кадре ставит `store.set_cancelling` и токен; второе нажатие -> `force()` (кнопка «Закрыть игру сразу»). Закрытие в `record::shutdown`: `quit` (до `quit_wait` 5 с) -> `session.terminate()` (Job) -> `wait_exit(kill_confirm 3 с)`, итого около 8 с плюс `mark_partial`. Опросы отмены: машина (`Input::Cancel`), `run_bounded` (шаг 100 мс), `wait_previous_game`, `precheck::probe`. `mark_partial` переименовывает `takeNNNN` в `takeNNNN.partial` только после подтверждённого выхода и ничего не удаляет. `launch::cleanup_installed` вызывается в `jobs::run` при любом исходе записи, `launch::recover` — при старте приложения для задач `running`/`cancelling` (`interrupted_jobs`). Поиск `.output()`: в `app`, `pipeline`, `host` (не тесты) нет; остались `renderer/src/assemble.rs:124` (`video_secs`, только CLI `assemble`) и `geometry/src/lib.rs:295` (`git`, не путь записи) — вне пути записи приложения. |
| 5 | Окно ошибки HLAE/AfxHookSource2 или первая строка `AFXERROR` завершают задачу сразу; текст окна и заголовки окон процессов игры попадают в ошибку | VERIFIED (механизм); A1 без живого подтверждения, см. ниже | `machine.rs`: первая `AFXERROR` (если не в списке исключений) -> `FailCode::HlaeError` немедленно; `on_windows` ищет `#32770` с `afxhook`/`hlae`/`afxerror` в заголовке или тексте; `record()` опрашивает `Session::windows()` каждые `window_poll`; на дедлайнах фазы запуска `failure::msg_windows` дописывает заголовки окон игры. `host/win/windows.rs` читает окна без отправки сообщений (`GetWindowTextW`). Стенд-тест с настоящим диалогом `wscript` проходит (SUMMARY 02-01, `--ignored modal`). |

**Счёт:** 5/5 (0 ожидают поведенческой проверки).

### Требования (запрошено по каждому)

| Требование | Статус в коде | Чекбокс в REQUIREMENTS.md | Доказательство |
|-----------|---------------|----------------------------|----------------|
| PRE-01 | SATISFIED | `[x]` верно | единый `Report`; общий `precheck::run` для задачи и кнопки «Проверить окружение» |
| PRE-02 | SATISFIED | `[x]` верно | порядок в `jobs::run`, `needs_precheck` |
| PRE-03 | SATISFIED | `[x]` верно | `evaluate` + `probe` с `tool_probe`; ffmpeg HLAE из `ffmpeg.ini` (`hlae_ffmpeg`) |
| PRE-04 | SATISFIED | `[x]` верно | `steam.exe` в снимке `Host::processes` (ToolHelp) |
| PRE-05 | SATISFIED | `[x]` верно | `foreign_game` fail, процесс не трогается; страж-тест |
| PRE-06 | SATISFIED | `[x]` верно | `space_items`, `check_space` по `space::estimate(plan.takes, …)` |
| PRE-07 | SATISFIED | `[x]` верно | `demo_item`, `out_item` |
| LCH-05 | SATISFIED | `[x]` верно | `AFXERROR`, `#32770`, `msg_windows` |
| TMO-03 | SATISFIED (по коду и тестам); тайминги на живой игре не записаны | `[x]` верно | поллинг отмены во всех ожиданиях, шаги 100 мс; `Host::run` с дедлайном везде |
| TMO-04 | SATISFIED | `[x]` верно | `record::shutdown` |
| TMO-05 | SATISFIED | `[x]` верно | Job Object `KILL_ON_JOB_CLOSE`, handle-based kill; запасной путь по PID принимает игру только с проверкой образа и времени создания лаунчера (`launcher_created`) |
| TMO-06 | SATISFIED | `[ ]` / «Pending» — отстаёт от кода | `mark_partial` (`.partial`), `cleanup_installed` (D-05, D-06), `recover` при старте (D-08), `is_ours` ограничивает удаление папками CS2; тесты `launch.rs` зелёные |
| TMO-07 | SATISFIED | `[x]` верно | `wait_previous_game` + `settle` |
| DIF-01 | SATISFIED | `[x]` верно | таблица `last_good`, warn в `steam_items`, запись после удачной задачи |
| DIF-02 | SATISFIED | `[x]` верно | `SteamState::Updating` -> `fail` «Steam обновляет CS2» |

### Расхождения в учёте (только в отчёте, файлы не правились)

1. `REQUIREMENTS.md`: **TMO-06** надо перевести в `[x]` и в таблице трассировки `Pending` -> `Complete`. Код и тесты это подтверждают; план 02-06 заявил TMO-06 как выполненное, но чекбокс не обновили.
2. План 02-01 пометил выполненными PRE-04, PRE-05, PRE-06, DIF-01, LCH-05, TMO-04, TMO-05, TMO-07 раньше времени: на тот момент 02-01 дал только хостовые факты (`processes`, `free_space`, `file_version`, `wait_exit`, реестр игр), а логики предпроверки и записи ещё не было. Автор SUMMARY 02-04 это прямо отмечает. Сейчас все эти пункты подкреплены кодом (см. таблицу), поэтому откатывать галочки не нужно. Это был ошибочный процесс, а не фактический пробел.
3. `ROADMAP.md`: в списке планов фазы 2 запись `02-08-PLAN.md` всё ещё `[ ]`, а заголовок «7/8 plans executed» устарел. Ручная проверка проведена (автор ответил «approved»), поэтому оба места надо привести к 8/8.

## Артефакты и связи

| Артефакт | Статус | Детали |
|----------|--------|--------|
| `crates/host` (`win/mod.rs`, `job.rs`, `windows.rs`, `system.rs`, `runner.rs`, `fake.rs`) | VERIFIED | реальная реализация, `UnsupportedHost` для не-Windows, фейк с виртуальными часами |
| `crates/pipeline/src/precheck/{mod,gather,space,steam}.rs` | VERIFIED | чистая оценка + сбор фактов, оба вызываются из приложения |
| `crates/pipeline/src/record/{mod,machine,failure,deadlines,markers,log}.rs` | VERIFIED | драйвер + машина, поэтапное закрытие |
| `crates/store/src/lib.rs` (схема v3, `Cancelling`, `set_precheck`, `last_good`, `interrupted_jobs`) | VERIFIED | миграции с v1/v2 покрыты тестами |
| `crates/app/src/{jobs,app,launch,jobs_page,settings_page}.rs` | VERIFIED | всё подключено (см. связи) |

| Откуда | Куда | Через | Статус |
|--------|------|-------|--------|
| `jobs::run` | `precheck::run` | `precheck_stage` до `analyze` | WIRED |
| `jobs::run` | `precheck::check_space` | `space_pass` до `install_files` | WIRED |
| `jobs::run` | `pipeline::record` | `record_stage` -> `record_request` | WIRED |
| `jobs::run` | `launch::cleanup_installed` | после `record_stage` при любом исходе | WIRED |
| `App::new` | `launch::recover` | `store.interrupted_jobs()` до `reset_running()` | WIRED |
| `App::cancel_job` | `store.set_cancelling` + `CancelToken` | `JobReporter::cancelled/force_cancelled` -> `Progress` -> `record` | WIRED |
| Настройки «Проверить окружение» | `precheck::run` | `App::run_precheck` с теми же `precheck_inputs` | WIRED |
| `Host::wait_previous_game` | `OWN_GAMES` | `Drop for WinSession` кладёт игру с handle в реестр | WIRED |

### Трассировка данных (уровень 4)

| Артефакт | Данные | Источник | Реальные данные | Статус |
|----------|--------|----------|-----------------|--------|
| карточка задачи | отчёт предпроверки | `jobs.precheck` (JSON из `Report::to_json`) | да | FLOWING |
| пункт `cs2_build` | `last_good` | таблица `last_good`, пишется после удачной записи | да | FLOWING |
| пункты места по плану | `space::estimate(plan.takes, …)` | дубли плана, размер кадра, длина демки | да | FLOWING |

### Поведенческие проверки

| Поведение | Команда | Результат | Статус |
|-----------|---------|-----------|--------|
| Весь workspace | `cargo test --release --workspace` | 279 passed, 0 failed, 1 ignored | PASS |
| Страж «только свои процессы» | `pipeline/tests/own_processes.rs` | 2 passed | PASS |
| Предпроверка на фейке | `pipeline/tests/precheck.rs` | 19 passed | PASS |
| Запись, отмена, AFXERROR, окна на фейке | `pipeline/tests/record.rs` | 35 passed | PASS |
| Job Object и окна на настоящих процессах | `host/tests/win_session.rs` | 5 passed | PASS |
| Совместимость CLI | `crates/cli/tests/pipeline.rs` | 2 passed | PASS |

### Выполнение проб

Проб вида `scripts/*/tests/probe-*.sh` фаза не объявляет; шаг не применим.

### Антипаттерны

Файлы, изменённые фазой (31 файл в `crates/` с коммита e258790): маркеров `TBD`, `FIXME`, `XXX`, `TODO`, `todo!`, `unimplemented!` нет. Заглушек нет: `UnsupportedHost` — намеренная платформенная заглушка из фазы 1.

## Замечания (не блокируют)

1. **Допущение A1 не подтверждено вживую.** Автор на ручной проверке (02-08) ответил одним словом «approved», строки `окна:` из `record.log` и времена отмены в SUMMARY не записаны. Остаточный риск: настоящее окно ошибки AfxHookSource2 может оказаться не `#32770`. Он смягчён тремя вещами: основной быстрый путь — строка `AFXERROR` в консоли, он не зависит от класса окна; детектор окна срабатывает только при `afxhook`/`hlae`/`afxerror` в заголовке или тексте, поэтому ложные срабатывания на здоровой записи маловероятны, и автор подтвердил здоровый прогон; при любом дедлайне фазы запуска заголовки всех видимых окон игры попадают в ошибку. Я не считаю риск достаточным для `human_needed`. Если в фазе 3 встретится зависание без `HLAE_ERROR` при видимом окне ошибки, надо посмотреть строки `[host] окна:` в `record.log`.
2. **Уборка в завершённых задачах.** Если `cleanup_installed` не смогла удалить файл (занят, три попытки по 500 мс), остаток остаётся в `installed.json` и подбирается только при следующем запуске той же задачи (`install_files` объединяет запись) или при падении приложения со статусом `running`/`cancelling`. Для завершённых (`failed`/`cancelled`) задач отдельного повтора при старте нет. Файлы лежат под `cs2cinema_*.dem`/`cs2c_*`/`highlights.cfg` и безвредны; кандидат в фазу 3.
3. **Тайминги отмены (3 с и ~10 с) подтверждены кодом и тестами на фейке, но не измерены на живой игре.** По дедлайнам: `quit_wait` 5 с + `kill_confirm` 3 с = 8 с, укладывается в ~10 с.
4. Отмена вне пути записи (скачивание демки, разбор) опрашивается только на границах этапов (`pipeline::check`); это за рамками требований фазы 2 (путь записи и внешние процессы).

## Резюме

Цель фазы достигнута: все пять критериев успеха ROADMAP подтверждены кодом и зелёным `cargo test --release --workspace`. Блокирующих пробелов нет. Правки в учёте для оркестратора: TMO-06 -> `[x]`/Complete в `REQUIREMENTS.md`; `02-08-PLAN.md` -> `[x]` и «8/8 plans executed» в `ROADMAP.md`.

---

_Проверено: 2026-10-10_
_Проверяющий: Claude (gsd-verifier)_
