---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 06
subsystem: recording
tags: [rust, host, windows, job-object, netcon, hlae, win32, console-log]

requires:
  - phase: 01-01
    provides: "трейты Host/Session, SessionEvent, Job, OwnedProc, snapshot_processes"
  - phase: 01-04
    provides: "pipeline::record::markers (parse_marker, is_demo_running_evidence, is_afxerror) и машина записи поверх Host"
  - phase: 01-05
    provides: "01-SPIKES.md: таблица «Решения для 01-06» и фикстуры консоли s3a/s3b/s3c/s3e"
provides:
  - "host::win::WinHost (impl Host) и WinSession (impl Session + Drop): сессия HLAE+CS2 в своём Job Object"
  - "host::win::{watch, netcon, console_log}: поток наблюдения, консоль netcon, хвост console.log от смещения запуска"
  - "host::gate::{DemoGate, Verdict, is_demo_playing}: метки задачи до подтверждения демки отбрасываются"
  - "host::runner::run_bounded: утилита с дедлайном, отменой и дочитыванием pipe не дольше 2 с; Host::run у WinHost и UnsupportedHost"
  - "host::default_host() = WinHost под cfg(windows), UnsupportedHost иначе"
  - "OwnedProc::open_owned / kill для режима «игра принята без job»"
affects: [01-07, 01-08]

actuals:
  tokens: 26900
  tasks: 3
  commits: 5
plan_head_before: 1c9123f3101ac31b92bb4da488f5140d88f45891
plan_head_after: 1273f169b43a27b1a9bbede9313df0b9d7650de9

tech-stack:
  added: []
  patterns:
    - "один mpsc-канал SessionEvent; потоки netcon/console.log/pipe/watch не ждутся через join, заканчиваются по EOF, ошибке или флагу stop"
    - "ворота DemoGate стоят в Sink (слой сессии): замок держится до отправки, поэтому подтверждение демки всегда раньше следующих меток"
    - "принятие по PID — чистая функция pick_adoptee, единственный кандидат с ожидаемым путём, созданный после лаунчера"
    - "каждое событие потока наблюдения сообщается один раз (struct Told), повторяющиеся Note глушатся note_once"

key-files:
  created:
    - crates/host/src/gate.rs
    - crates/host/src/runner.rs
    - crates/host/src/win/netcon.rs
    - crates/host/src/win/watch.rs
    - crates/host/src/win/console_log.rs
    - crates/host/tests/win_session.rs
    - crates/pipeline/tests/transcripts.rs
  modified:
    - crates/host/src/lib.rs
    - crates/host/src/unsupported.rs
    - crates/host/src/win/mod.rs
    - crates/host/src/win/process.rs

key-decisions:
  - "READY (и любая метка с nonce задачи) до подтверждения демки ОТБРАСЫВАЕТСЯ с Note, а не удерживается: удержанный ложный READY ушёл бы в машину сразу после «playing demo from», за ~2 с до настоящей загрузки. Потерянный настоящий READY возвращает машина 01-04 сама (ready_direct после evidence + ready_grace)"
  - "Подтверждение демки для ворот — «Host activate: Playing Demo» или «playing demo from» (по таблице спайков); машина 01-04 не менялась"
  - "abs_playdemo=fails: WinHost::launch до запуска отвергает имя демки, если оно не-ASCII, пустое или содержит пробел/управляющий символ/кавычку/«;» (HostError::Spawn по-русски). Копия в game/csgo остаётся задачей 01-07 (TMO-06)"
  - "fix_netcon=on: константа FIX_NETCON = true со ссылкой на 01-SPIKES.md, -afxFixNetCon всегда в строке запуска"
  - "Закрытие обязано заканчиваться Job::terminate(): WinSession::terminate завершает job безусловно, режим «без job» ещё и своим handle игры"
  - "Поток наблюдения живёт ещё 3 с после terminate (STOP_GRACE), чтобы выход игры и пустой job дошли до журнала; netcon после GameExited не переподключается (порт мог достаться чужому процессу)"
  - "Хвост console.log читается от длины файла на момент запуска; пересоздание файла (длина меньше смещения) — чтение с нуля"

patterns-established:
  - "SessionParams + launch_session — публичный вход для тестов со стенд-процессами (cmd/ping и своя копия ping.exe вместо cs2.exe); launch_with(cmd, image, port, console_log) — упрощённый вход из плана"
  - "RED/GREEN в двух коммитах на каждую TDD-задачу: test(01-06) с падающими тестами и заглушкой, затем feat(01-06)"

requirements-completed: [TST-01, TMO-02, LCH-04, LCH-06, TMO-01]

coverage:
  - id: D1
    description: "WinHost создаёт job до запуска лаунчера, находит cs2.exe среди PID job, живость по handle, выход лаунчера и смерть игры — отдельные события"
    requirement: "TMO-02"
    verification:
      - kind: integration
        ref: "crates/host/tests/win_session.rs#stand_in_launcher_game_and_console_flow_through_one_session"
        status: pass
    human_judgment: false
  - id: D2
    description: "netcon, хвост console.log и pipe лаунчера приходят в один канал с меткой источника; команды только через netcon; BOM, смещение, обрывки строк"
    requirement: "LCH-04"
    verification:
      - kind: integration
        ref: "crates/host/tests/win_session.rs#console_log_tail_starts_at_the_launch_offset"
        status: pass
      - kind: unit
        ref: "crates/host/src/win/console_log.rs#tail_starts_at_the_offset_and_joins_partial_lines"
        status: pass
    human_judgment: false
  - id: D3
    description: "Принятие cs2.exe по PID только для единственного кандидата с ожидаемым путём, созданного после лаунчера и не в нашем job; принятая игра завершается вместе с сессией"
    requirement: "TMO-02"
    verification:
      - kind: unit
        ref: "crates/host/src/win/watch.rs#adoption_takes_only_a_single_fresh_candidate_with_the_expected_path"
        status: pass
      - kind: integration
        ref: "crates/host/tests/win_session.rs#a_game_outside_the_job_is_adopted_by_pid_and_terminated_with_the_session"
        status: pass
    human_judgment: false
  - id: D4
    description: "Ложный READY на меню-карте (S3c) не доходит до машины; настоящие строки консоли и фикстуры S3 никогда не разбираются как метки чужого nonce"
    requirement: "LCH-04"
    verification:
      - kind: unit
        ref: "crates/pipeline/tests/transcripts.rs#ready_before_the_demo_is_dropped_in_the_s3c_transcript"
        status: pass
      - kind: unit
        ref: "crates/pipeline/tests/transcripts.rs#real_console_lines_are_never_markers"
        status: pass
      - kind: integration
        ref: "crates/host/tests/win_session.rs#marker_before_the_demo_is_held_back_in_a_live_session"
        status: pass
    human_judgment: false
  - id: D5
    description: "Строка запуска HLAE собрана в одном месте по таблице спайков; пути проверяются до запуска; не-ASCII имя демки отвергается"
    requirement: "LCH-06"
    verification:
      - kind: unit
        ref: "crates/host/src/win/mod.rs#game_line_follows_the_spike_decisions"
        status: pass
      - kind: unit
        ref: "crates/host/src/win/mod.rs#demo_name_must_be_plain_ascii"
        status: pass
    human_judgment: true
    rationale: "Настоящий HLAE.exe и cs2.exe в плане не запускались (запрет на старт игры); строка проверена только текстом, живой запуск — 01-07/фаза 4"
  - id: D6
    description: "Host::run запускает утилиту с дедлайном и отменой, строки stdout доходят до on_line; default_host() — WinHost на Windows"
    requirement: "TMO-01"
    verification:
      - kind: unit
        ref: "crates/host/src/runner.rs#bounded_run_kills_on_deadline"
        status: pass
      - kind: unit
        ref: "crates/host/src/runner.rs#bounded_run_stops_on_cancel"
        status: pass
      - kind: unit
        ref: "crates/host/src/lib.rs#default_host_is_win_host_on_windows"
        status: pass
    human_judgment: false

duration: 11min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 06: WinHost — настоящий Host для Windows Summary

**`WinHost` ведёт HLAE+CS2 в собственном Job Object: игра ищется среди PID job (запасной путь — принятие по PID единственного кандидата), живость по handle, консоль из netcon + `console.log` + pipe лаунчера в одном канале с воротами «демка идёт», утилиты через `Host::run` с дедлайном; `default_host()` на Windows теперь `WinHost`.**

## Performance

- **Duration:** около 11 мин
- **Started:** 2026-10-10T11:36:41Z
- **Completed:** 2026-10-10T11:47:42Z
- **Tasks:** 3 (1 tracer, 2 TDD)
- **Files changed:** 11 (7 новых, 4 изменённых), 2181 строка

## Accomplishments

- **Сессия (Task 1, tracer).** `Job::new` до `spawn`, `assign_child` сразу после; потоки: наблюдатель (250 мс: выход лаунчера → `LauncherExited`, `cs2.exe` в `Job::pids()` → `GameAppeared`, `liveness()` → `GameExited` / `Note` при `Unknown`, `active_count()==0` → `JobEmpty`), netcon (`127.0.0.1`, `connect_timeout` 2 с, чтение с таймаутом 500 мс и накоплением обрывков, переподключение), читатели pipe лаунчера. `WinSession::terminate` идемпотентна и вызывается из `Drop`. Сквозной тест на `cmd`/`ping` и TCP-заглушке консоли зелёный, 3 запуска подряд.
- **Реальный запуск (Task 2).** `Host::launch`: проверка `HLAE.exe`, `x64/AfxHookSource2.dll`, `cs2.exe` до запуска; строка `-customLoader -noGui -autoStart -noConfig -hookDllPath … -programPath … -cmdLine "-insecure -steam -novid -allow_third_party_software -condebug -netconport N -afxFixNetCon -windowed -w W -h H +demo_ui_mode 0 +playdemo ИМЯ"`. Хвост `console.log` с BOM/смещением, принятие по PID (`pick_adoptee`, `assign_pid`, режим «без job» через `open_owned`/`kill`).
- **Ворота демки.** Метка с nonce задачи до `Host activate: Playing Demo` / `playing demo from` в канал не попадает (вместо неё `Note «метка до подтверждения демки отброшена…»`); на фикстуре S3c первый READY отброшен, второй (на демке) проходит; на s3a/s3b (битая и пустая демка) ворота остаются закрытыми.
- **Утилиты (Task 3).** `run_bounded` с дедлайном и отменой на шаге 100 мс, `Child::kill` своего процесса, дочитывание pipe не дольше 2 с, `CREATE_NO_WINDOW`; `WinHost::run` и `UnsupportedHost::run` вызывают его; `default_host()` переключён.

## Task Commits

1. **Task 1: Сессия WinHost сквозь стенд-лаунчер (tracer)** — `4212567` (feat)
2. **Task 2: Настоящий запуск HLAE** — RED `0e7d3a9` (test; 5 падающих тестов на заглушках `pick_adoptee`/`Tail`), GREEN `d5145d9` (feat)
3. **Task 3: Утилиты с дедлайном и WinHost по умолчанию** — RED `c29f5b2` (test; 6 падающих), GREEN `1273f16` (feat)

**Tracer gate:** `<verify>` задачи 1 автоматический, перепрогнан после `cargo fmt` и после правки `STOP_GRACE` (3 запуска подряд зелёные) — «Tracer verified end-to-end — expanding». Режим `blocking-human` в задаче не задан.

**Plan metadata:** отдельный коммит `docs(01-06)` (SUMMARY, STATE, ROADMAP).

## Files Created/Modified

- `crates/host/src/win/mod.rs` — `WinHost`, `WinSession`, `Shared`, `Sink` (с воротами), `SessionParams`/`launch_session`, строка запуска HLAE, `check_demo_name`, 4 юнит-теста
- `crates/host/src/win/watch.rs` — поток наблюдения, `Candidate`, `pick_adoptee`, принятие по PID
- `crates/host/src/win/netcon.rs` — подключение, чтение, `free_port`
- `crates/host/src/win/console_log.rs` — `Tail` (BOM, смещение, пересоздание файла), поток хвоста
- `crates/host/src/win/process.rs` — `OwnedProc::open_owned`, `kill`
- `crates/host/src/gate.rs` — `DemoGate`, `Verdict`, `is_demo_playing`, `MARKER_TAG`
- `crates/host/src/runner.rs` — `run_bounded`
- `crates/host/src/lib.rs`, `unsupported.rs` — `pub mod gate/runner`, `default_host`, `UnsupportedHost::run`
- `crates/host/tests/win_session.rs` — 4 сквозных теста на настоящих процессах Windows
- `crates/pipeline/tests/transcripts.rs` — 6 тестов: настоящие строки консоли и фикстуры против парсера и ворот

## Decisions Made

См. `key-decisions` во frontmatter. Главное — как реализованы поправки из спайков:

| Решение спайка | Реализация |
|---|---|
| `abs_playdemo=fails` | `WinHost::launch` отвергает не-ASCII/пробельные/«;»/кавычечные имена демки типизированной ошибкой `HostError::Spawn` с русским текстом **до** запуска любого процесса. Fallback-копия в ASCII-путь не делалась: `LaunchSpec.demo_name` — это имя файла в `game/csgo`, копирование остаётся в `install_files` приложения (01-07, TMO-06) |
| ложный READY (S3c) | ворота в `Sink`, отбрасывание с `Note` (не удержание); машина 01-04 не менялась |
| `quit` может зависнуть (S2) | `terminate()` безусловно завершает job; машина вызывает `Action::Terminate` по `game_close`/`close_settle` |
| `hlae_pipe_eof=at-game-exit` | читатели pipe без `join` |
| `fix_netcon=on` | `FIX_NETCON = true` |
| `netcon_bind=0.0.0.0` | см. «Threat Flags» |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] Новый модуль `host::gate` (в плане его нет)**
- **Found during:** Task 2 (указание оркестратора: ворота READY на уровне WinHost, транскрипт-тест на фикстурах)
- **Issue:** хост не зависит от `pipeline`/`renderer`, а ворота должны разбирать строки меток и проверяться на фикстурах кроссплатформенно.
- **Fix:** чистый модуль `crates/host/src/gate.rs` (тег `CS2C` продублирован константой, тест `gate_marker_tag_is_the_renderer_tag` сверяет с `renderer::MARKER_TAG`); `Sink::line` применяет ворота ко всем каналам.
- **Files modified:** `crates/host/src/gate.rs`, `crates/host/src/lib.rs`, `crates/host/src/win/mod.rs`
- **Commit:** `0e7d3a9`, `d5145d9`

**2. [Rule 1 - Bug] Поток наблюдения выходил по `stop`, не дождавшись выхода игры**
- **Found during:** Task 1 (первый прогон сквозного теста: после `terminate` не приходили `GameExited`/`JobEmpty`)
- **Fix:** добавлен `STOP_GRACE = 3 с` — поток после остановки сессии ещё наблюдает, затем выходит.
- **Files modified:** `crates/host/src/win/watch.rs`
- **Commit:** `4212567`

**3. [Rule 3 - Blocking] `UnsupportedHost::run` по плану вызывает `run_bounded`**
- **Issue:** тест 01-01 `unsupported_host_refuses_to_launch` ждал `Err(Unsupported)` от `run`.
- **Fix:** тест обновлён: несуществующая программа даёт `HostError::Spawn(_)`; `launch` по-прежнему `Unsupported`.
- **Files modified:** `crates/host/src/unsupported.rs`
- **Commit:** `1273f16`

### Уточнения сверх плана (не отклонения от контракта)

- Читатели pipe лаунчера (stdout/stderr) сделаны в Task 1, а не в Task 2: pipe с `Stdio::piped()` нельзя оставлять непрочитанными (иначе дочерний процесс встанет), поэтому они нужны уже сквозному тесту.
- `SessionParams` и `WinHost::launch_session` публичны ради тестов с настоящими процессами (nonce для ворот, `expected_exe` для принятия); `launch_with(cmd, game_image, port, console_log)` из плана сохранён как упрощённый вход.
- Дополнительные тесты сверх обязательных: `marker_before_the_demo_is_held_back_in_a_live_session`, `a_game_outside_the_job_is_adopted_by_pid_and_terminated_with_the_session` (своя копия `ping.exe` вместо `cs2.exe`), `run_returns_within_the_drain_limit_when_a_grandchild_holds_the_pipe`, `run_reports_a_missing_program_as_spawn`, юнит-тесты `Tail`, `gate`, строки запуска, `check_demo_name`, транскрипты s3a/s3b/s3e.
- `cargo fmt -p host` (ширина по умолчанию), как в 01-01. `cargo fmt -p pipeline` изменил посторонний `crates/pipeline/src/lib.rs` — я откатил этот файл (`git checkout -- <файл>`), в коммиты он не попал.

**Total deviations:** 3 (1 Rule 2, 1 Rule 1, 1 Rule 3). Контракт трейтов `Host`/`Session` не менялся.

## Issues Encountered

- В этой среде нет `python`; правки делались `Edit`, `sed` и `perl`. Heredoc в Bash съедает пару обратных слэшей (`'\\'` → `'\'`) — дважды ловил `unterminated character literal`, исправлено правкой файла.
- Linux-сборка локально не проверялась: установлен только `x86_64-pc-windows-msvc`. `gate.rs` и `runner.rs` — чистый `std` (в `runner.rs` `CommandExt` под `cfg(windows)`), `win` под `cfg(windows)`; CI на ubuntu проверит.

## Known Stubs

Нет.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: netcon-all-interfaces | crates/host/src/win/netcon.rs | CS2 слушает консольный порт на `0.0.0.0` без пароля (S2: 10 из 10 запусков; параметра привязки в строке запуска нет). Митигации T-01-17: хост подключается только к `127.0.0.1` и к случайному порту этого запуска, строки из сокета недоверенные и никогда не исполняются хостом, команды уходят только через `Session::send` (`console_line_ok`; машина шлёт фиксированные шаблоны), метки принимаются только с nonce задачи и только после подтверждения демки, после смерти игры консоль не переподключается, окно экспозиции — десятки секунд сессии. Брандмауэр Windows на входящие не проверялся. **Остаточный риск:** другой локальный процесс или хост в сети может подключиться к порту и отправить команды в консоль CS2 |
| threat_flag: adoption-by-pid | crates/host/src/win/watch.rs | Запасной путь принимает процесс по PID. Митигация T-01-19: единственный кандидат, путь образа = `LaunchSpec.cs2_exe`, `creation_time` не раньше лаунчера, не в нашем job; неизвестный `IsProcessInJob` трактуется как «не принимать»; по имени образа ничего не завершается. **Остаточный риск:** собственный `cs2.exe` пользователя с тем же путём, запущенный уже после нашего лаунчера и в окне 5 с после выхода HLAE без игры в job, был бы принят — это плановое правило, при `cs2_in_job=yes` ветка не срабатывает (игра находится в job за 0.3–0.5 с) |

Прочие митигации плана: T-01-18 (`console_line_ok` в `send`, тест `BadCommand`), T-01-20 (флаг stop, таймауты 500 мс/250 мс, `Drop` → `terminate`, без `join`), T-01-21 (дедлайн/отмена на шаге 100 мс, дочитывание ≤ 2 с, тесты), T-01-22 (чтение с offset, тест `console_log_tail_starts_at_the_launch_offset`).

## Не проверено на реальной игре

В плане не запускались `HLAE.exe` и `cs2.exe` (запрет оркестратора). Не подтверждено живьём: что HLAE корректно стартует со `stdin = null` и piped stdout/stderr (спайк-стенд ставил piped, но `stdin` не менял); что пересоздание `console.log` игрой при старте ловится по «длина меньше смещения» (если новый файл за 250 мс дорастёт до старого размера, начало лога пропустится — метки это не затрагивает); режим «принята без job» (`open_owned` + `kill`) покрыт кодом, но в тесте принятие дошло до успешного `assign_pid` (ветка с отказом живьём не гонялась). Первый настоящий прогон — 01-07/фаза 4.

## Next Phase Readiness

01-07 может переключать приложение на `pipeline::record` с `host::default_host()`: `LaunchSpec` заполняется `hlae_dir`, `cs2_exe`, `demo_name` (ASCII, файл уже скопирован в `game/csgo`), `size`, `nonce`, `console_log = game/csgo/console.log`; склейка ffmpeg — через `Host::run` с `CmdSpec.deadline`. Копирование демки в `game/csgo` и удаление после записи (TMO-06) остаются на стороне приложения.

## Self-Check: PASSED

- Файлы: `gate.rs`, `runner.rs`, `win/{netcon,watch,console_log}.rs`, `tests/win_session.rs`, `pipeline/tests/transcripts.rs` на месте.
- Коммиты `4212567`, `0e7d3a9`, `d5145d9`, `c29f5b2`, `1273f16` найдены в истории; `git rev-list --count` от `plan_head_before` = 5.
- `cargo test --release -p host --test win_session` — 4 passed; `--lib` — 41 passed (`bounded_run` — 4 passed, `adoption_…` — ok); `-p pipeline --test transcripts` — 6 passed; `cargo test --release --workspace` и `cargo build --release --workspace` зелёные; `cargo clippy -p host --all-targets` без предупреждений.
- `grep -rniE "taskkill|tasklist|\.join\(\)" crates/host/src/win` и `grep -rn "\.output()" crates/host/src` — пусто.
