# Phase 1: Новый путь записи с понятными сбоями - Research

**Researched:** 2026-10-10
**Domain:** Windows process ownership (Job Object) + console-marker state machine for HLAE/CS2 recording, in a Rust `std::thread` workspace
**Confidence:** MEDIUM-HIGH (code and Win32 mechanics verified this session; HLAE/CS2 runtime behaviour is the part that needs the spikes)

Provenance tags used below: `[VERIFIED: path:lines]` (file opened this session, value quoted verbatim), `[VERIFIED: run]` (executed this session), `[CITED: url]` (official doc / upstream source fetched this session), `[ASSUMED]` (needs user or spike confirmation, collected in the Assumptions Log).

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

### Граница трейта Host
- **D-01:** Трейт `Host` и его реализации живут в новом листовом крейте `crates/host` (без зависимостей на `pipeline`): трейты, `WinHost` под `cfg(windows)`, фейк в памяти, заглушка для не-Windows (CI). `pipeline` зависит от `host`; `app` и `cli` получают его через `pipeline`. `windows-sys` подключается только в `crates/host`. — **Reversibility:** costly — переезд в модуль `pipeline` потом тронет все импорты и тесты, но без миграций.
- **D-02:** Свой `cs2.exe` находится через Windows Job Object уже в фазе 1: `WinHost` создаёт job с `KILL_ON_JOB_CLOSE` до запуска `HLAE.exe`, список PID и дескриптор `cs2.exe` берёт из job; живость — ожидание на handle (TMO-02). Запасной путь, если спайк покажет, что `cs2.exe` не наследует job через лоадер HLAE: снимок процессов после старта → `OpenProcess` → `AssignProcessToJobObject`. Фаза 2 добавляет только поэтапное завершение и отмену. — **Reversibility:** costly — отказ от Job Object потребует другой модели владения процессом во всех состояниях машины.
- **D-03:** Трейт прячет процессы (запуск сессии игры, события: строки консоли, выход лаунчера, смерть игры; отправка команд; завершение; запуск утилит с дедлайном), консоль и часы (`now()`). Файловая система не абстрагируется: машина работает с реальными путями, фейк в тестах пишет во временную папку.
- **D-04:** Основной канал консоли — netcon (`-netconport`, плюс `-afxFixNetCon` после спайка). Запасной — хвост `console.log` (`-condebug`), чтобы маркеры и `AFXERROR` приходили даже при неоткрывшемся порте. Оба потока сливаются в одну шину событий с дедупликацией маркеров. Отправка команд — только через netcon; если порт не открылся в срок — `CONSOLE_NOT_OPEN`.

### Ошибка в карточке и БД
- **D-05:** `pipeline::record` возвращает типизированную ошибку `RecordFailure { code, state, message, hint, elapsed, console_tail }`. В `store` добавляются колонки `fail_code`, `fail_state`, `fail_hint`, `fail_tail` (миграция схемы); `error` остаётся человеческим сообщением по-русски. `store` не зависит от `pipeline`: в БД идут строки. Карточка и будущий автоповтор (DIF-03/04) читают код без разбора строк. — **Reversibility:** one-way — колонки в SQLite на машине пользователя; откат потребует миграции вниз.
- **D-06:** Карточка упавшей задачи показывает блок ошибки: код и состояние мелким шрифтом, сообщение, подсказку, прошедшее время, сворачиваемый хвост консоли. Плюс лестница состояний «лаунчер → игра → консоль → демка → скрипт → дубль N/M → закрытие» с отметкой пройденных и упавшего шага; лестница видна и пока задача выполняется.
- **D-07:** Хвост консоли в ошибке — около 30 строк консоли игры и stdout/stderr HLAE без шума: подряд идущие повторы сворачиваются в «×N» (например, `Failed writing image`), строки с `AFXERROR` и маркерами сохраняются всегда. Полная консоль всегда в `record.log`.
- **D-08:** Коды ошибок — `SCREAMING_SNAKE` по-английски, текст рядом по-русски. Базовый набор фазы 1: `LAUNCHER_FAILED`, `GAME_NOT_STARTED`, `CONSOLE_NOT_OPEN`, `DEMO_NOT_LOADED`, `SCRIPT_NOT_RUN`, `TAKE_TIMEOUT`, `GAME_EXITED`, `HLAE_ERROR`, `CANCELLED`. Расширяется в фазах 2–3.

### Дедлайны и маркеры
- **D-09:** Дедлайны — структура `Deadlines` в `pipeline::record` с именованными полями (лаунчер, появление игры, консоль, загрузка демки, скрипт, первый дубль, каждый дубль, закрытие игры, ffprobe, ffmpeg) и константами по умолчанию из research (консоль ~90 с, простой дубля ~60 с как отправные точки). В настройках `load_wait_secs` заменяется одним множителем «запас по времени» (1.0 по умолчанию), который умножает всю таблицу. Тесты передают свою таблицу. Значения подстраиваются в фазе 4.
- **D-10:** `exec highlights` отправляется ровно один раз после маркера `READY`. Повторной отправки по таймеру нет: если `SCRIPT` не пришёл в срок — `SCRIPT_NOT_RUN`, если `TAKE_START 0` не пришёл — `TAKE_TIMEOUT`. Повтор задачи целиком — дело фаз 3–4.
- **D-11:** Маркеры консоли переходят на формат с nonce задачи: `CS2C <nonce> READY|SCRIPT|TAKE_START n|TAKE_END n|ALL_DONE`. Рендерер получает nonce в `Config` и пишет маркеры в `highlights.cfg` и `cs2c_NN.cfg`; парсер игнорирует эхо команд и чужой nonce (старый `console.log`, прошлая задача). Тесты рендерера обновляются; `CS2C_TAKE_DONE` уходит. — **Reversibility:** costly — формат маркеров зашит в генерируемые cfg и в парсер; старые папки с cfg станут несовместимы с новым парсером.
- **D-12:** Дедлайн одного дубля (от `TAKE_START n` до `TAKE_END n`) выводится из плана: игровая длина шота × коэффициент записи (от `capture_fps`, подстраивается в фазе 4) + фиксированный запас на перемотку `demo_gototick` и закрытие ffmpeg. Считается вместе с ожидаемым набором дублей из `plan.json` до начала записи (TAK-01).

### Claude's Discretion
- **Спайки.** Пользователь не стал обсуждать. Решение: четыре спайка из роадмапа (Job Object через лоадер HLAE, `-afxFixNetCon`, текст консоли при реальных сбоях `playdemo`/cfg, абсолютный путь в `+playdemo` через `-cmdLine`) оформляются как пример-бинарник в `crates/host/examples/` (или один бинарник с подкомандами). Запускает их пользователь на своей машине при запущенном Steam; результат — заметка `.planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md` с выводом по каждому спайку и выбранным путём (основной или запасной). Планировщик ставит спайки первым планом и делает `WinHost` зависимым от их результата; при любом исходе у каждого спайка есть запасной путь из research, так что фаза не блокируется.
- **Как лестница состояний доходит до UI.** Рекомендация из research: `Progress` получает типизированный метод `event(&self, e: Event)` с реализацией по умолчанию, `store.set_stage` хранит имя состояния строкой; UI не парсит строки журнала.
- **Шина событий.** `crossbeam-channel` (research) или `std::mpsc` с `recv_timeout` — на усмотрение планировщика; требование одно: каждое ожидание в цикле машины — срез ≤100 мс с проверкой `Progress::cancelled()`.
- **Закрытие игры в фазе 1.** Достаточно `quit` по консоли и ожидания выхода по handle с дедлайном; при неответе — закрыть job. Полная поэтапная уборка — фаза 2.
- **Формат `record.log`.** Строки с меткой времени и источником (`netcon`, `console.log`, `hlae-stdout`, `hlae-stderr`, `app`); пишется инкрементально, сбрасывается при любом исходе.
- **Копии демки в папке CS2.** Поведение `install_files` не меняется в фазе 1 (уборка — фаза 2, TMO-06).

### Deferred Ideas (OUT OF SCOPE)
- Поэтапное завершение игры и отмена за секунды в любом состоянии — фаза 2 (TMO-03, TMO-04).
- Уборка копий демки и cfg в папке CS2 — фаза 2 (TMO-06).
- Модальное окно HLAE и `AFXERROR` как мгновенный сбой — фаза 2 (LCH-05); в фазе 1 `AFXERROR` лишь сохраняется в хвосте.
- Кнопка «Открыть папку» и полный блок повторов — фаза 3 (OBS-02, RET-*).
- Таймауты по прогрессу («нет новых строк N секунд») — v2 (DIF-06).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TST-01 | Внешние процессы, консоль, ФС и часы за трейтом `Host`; реальная Windows-реализация, фейк, заглушка для не-Windows | Section "Host trait design": `Host`/`Session` traits, `poll(max_wait)` makes the clock virtual in the fake, `WinHost` under `cfg(windows)`, `UnsupportedHost` elsewhere. CI runs `cargo test --release --workspace` on ubuntu and windows (see Test Strategy). |
| LCH-01 | Этап записи как явная машина состояний с именованными состояниями и состояниями сбоя | "Record state machine": pure `Machine::step(now, Input) -> Vec<Action>`, state list, transitions, deadlines table. |
| LCH-02 | Типизированная ошибка: код, состояние, сообщение по-русски, подсказка, хвост консоли; видна в карточке | "Typed failure": `RecordFailure`, Russian message/hint catalogue, SQLite migration (`PRAGMA user_version`), `RunError` bridge in `jobs.rs`, card block. |
| LCH-03 | Отсутствие маркера READY/SCRIPT/TAKE_START/TAKE_END в срок — ошибка, а не предупреждение | Deadline table + one-shot `exec highlights` (D-10); every marker wait is a state with a named deadline and a failure code. |
| LCH-04 | Консоль и stdout/stderr HLAE в буфер и в `record.log` при любом исходе | "record.log and console tail": `RecordLog` written by the driver (not by the machine), flushed per line, closed on every exit path incl. panic-safe `Drop`. HLAE stdout/stderr are probably empty (A2), pipes are still captured. |
| LCH-06 | Код выхода HLAE.exe и живость cs2.exe — отдельные факты | `SessionEvent::LauncherExited{code}` vs `Session::game_liveness()`; tri-state rendering «жива / не жива / неизвестно». |
| TMO-01 | Каждое ожидание имеет именованный дедлайн; по истечении — этап и прошедшее время | `Deadlines` struct with defaults, multiplier, derived per-take deadline; ffmpeg assemble deadline in `run_ffmpeg`. |
| TMO-02 | Живость по дескриптору процесса; ошибка проверки = «неизвестно» | `OpenProcess(SYNCHRONIZE\|PROCESS_QUERY_LIMITED_INFORMATION)` + `WaitForSingleObject(h, 0)`; mapping verified by a run this session (see Code Examples). |
| TAK-01 | Ожидаемый набор дублей выводится из плана до записи | `renderer::record_order` + `capture_fps` (both `pub`) -> `TakePlan` list computed in `plan_player`; M = `timeline.shots.len()`. |
</phase_requirements>

## Project Constraints (from CLAUDE.md)

Source: `C:\Users\kamal\cs2cinema\.claude\CLAUDE.md` (project instructions). Treated as locked.

- Stack is fixed: Rust cargo workspace, egui/eframe, `std::thread` (no tokio), rusqlite. Embed in existing crates; a new leaf crate `crates/host` is already a locked decision (D-01).
- Windows only for HLAE/CS2; one recording job at a time (CS2 allows one instance).
- External tools (HLAE, CS2) have undocumented console markers and folder layout: every detector needs a timeout and diagnostics.
- `cs2-cli` must keep building and `crates/cli/tests/pipeline.rs` must keep passing. CLI uses `renderer::Config::default()` ([VERIFIED: crates/cli/tests/pipeline.rs:110] `let out = renderer::render(&m, &timeline, &renderer::Config::default());`) so any new `renderer::Config` field needs a default.
- User-facing strings and doc comments in Russian; identifiers in English; tests named as sentences describing behaviour.
- Errors: `Result<T, String>` at the existing boundaries; cancellation is the sentinel `pipeline::CANCELLED`; no `anyhow`/`thiserror` in the workspace today.
- New long steps report through `Progress::stage`/`log` and call `check(p)?` at boundaries.
- Public API through crate root; prefer private items; derives `Serialize/Deserialize` for anything persisted.
- Edition 2024 in every crate; run `cargo fmt` and `cargo clippy --workspace` locally (CI runs only `cargo test`/`cargo build`).
- GSD workflow: file edits happen through GSD commands (this research is read-only for source).

## Summary

The phase replaces one function, `jobs::record` plus `launch::{launch, Console, game_alive, kill_game, wait_for_clips}`, with `pipeline::record`: an explicit state machine over a `host::Host` trait. Today's flow is an implicit state machine built from `Cell`/`RefCell` closures; liveness is `tasklist` text with `Err(_) => true`; a missing netcon or marker is downgraded to a log line; `exec highlights` is re-sent on a 30 s timer; completion is directory mtime stability; cancel runs `taskkill /IM cs2.exe /F`. Every one of these is read from the code in this session (Current Code Ground Truth below).

The design that fits the locked decisions and the existing crate DAG is: (1) new leaf crate `host` with two traits (`Host`, `Session`), a `WinHost` under `cfg(windows)` built on a Job Object, an in-memory `FakeHost` with a **virtual clock** (achieved by making the wait itself, `Session::poll(max_wait)`, part of the trait, so a 150 s deadline test runs in microseconds), and an `UnsupportedHost` for non-Windows; (2) `pipeline::record` containing a pure `Machine` (`step(now, Input) -> Vec<Action>`), a thin driver loop with a ≤100 ms slice, a marker parser, `Deadlines`, `RecordFailure`, `RecordLog` and a console-tail builder; (3) renderer markers with nonce; (4) a store migration plus `Progress::event`; (5) a small app/UI delta (error block + state ladder). The only new dependency is `windows-sys` (already in `Cargo.lock` at 0.61.2, verdict OK); `crossbeam-channel` and `thiserror` are deliberately not adopted because one `std::sync::mpsc` channel with several producers behind `Session::poll` is enough and the codebase has no `thiserror` convention.

Windows mechanics were exercised this session: a Job Object with `KILL_ON_JOB_CLOSE` compiles against `windows-sys` 0.61.2 with five features, a grandchild process lands in the job, liveness by `WaitForSingleObject(handle, 0)` returns `WAIT_TIMEOUT` then `WAIT_OBJECT_0` after `TerminateJobObject` ([VERIFIED: run], Code Examples). What is **not** verified is HLAE-specific: whether `cs2.exe` really ends up inside the job when HLAE's loader creates it. HLAE's `Loader.cs` creates it with `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_SUSPENDED` and no `CREATE_BREAKAWAY_FROM_JOB` [CITED: raw.githubusercontent.com/advancedfx/advancedfx/main/hlae/Loader.cs], and Microsoft documents that children inherit the job by default [CITED: learn.microsoft.com job-objects], so the expected result is "yes"; spike S1 turns this into an observation, and an adopt-by-PID fallback exists.

Two findings change the plan relative to the project-level research. First, `HLAE.exe` almost certainly prints nothing to stdout/stderr (its `Program.cs` has no console output, only modal dialogs) [CITED: hlae/Program.cs], so the "stdout/stderr HLAE" half of LCH-04 will mostly record an empty stream plus the exit code; the real diagnostics are the game console and the exit code. Second, because the nonce is embedded in the generated cfg files, the existing `plan_hash` shortcut in `jobs.rs` (hash of `highlights.cfg` + `cs2c_*`) would change on every plan and silently disable "takes already recorded"; the hash must normalise the nonce.

**Primary recommendation:** Build `crates/host` (traits + fake + stub + `Job` module) and `pipeline::record` (pure machine + driver) first and test them with the fake; run the S1-S4 spike harness on the author's machine in parallel (close the running `cs2.exe` first, one is running right now); then write `WinHost` from the spike outcome and cut `jobs.rs` over. Keep `Progress`, `CANCELLED`, `Result<T,String>` and `run_ffmpeg`'s external contract intact.

## Architectural Responsibility Map

Tiers adapted to this desktop app (the template's web tiers do not apply).

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| State machine, deadlines, failure taxonomy, marker parsing, take plan | `pipeline` (orchestration, no OS calls) | `renderer` (marker text, `record_order`, `capture_fps`) | Policy lives in one pure place; testable with no OS |
| Process ownership, liveness, console I/O, clock, bounded command runs | `host` (OS adapter, leaf crate) | none | Only crate allowed to touch Win32 / sockets / child processes |
| Marker emission into the game | `renderer` (generated `highlights.cfg`, `cs2c_NN.cfg`) | `pipeline` (sends READY and `exec highlights`) | Marker format is shared contract between generator and parser; nonce flows `pipeline -> renderer::Config` |
| Failure persistence (`fail_*`, `phase`, `out_dir` on failure) | `store` (strings only) | `app::jobs` (maps `RecordFailure` to strings) | `store` must not depend on `pipeline` (D-05) |
| Live state ladder, error card, "запас по времени" setting | `app` (egui) | `store` (rows are the transport) | `refresh_jobs` already reloads rows whenever a `Log`/`Stage` message arrives |
| `record.log`, console tail | `pipeline` driver (writes), `host` (supplies tagged lines) | none | Source tags come from the host; folding and formatting are policy |
| NVENC probe, HLAE/CS2 discovery, `install_files` | `app::launch` (unchanged in Phase 1) | none | Deferred to Phase 2/4 by CONTEXT |

## Current Code Ground Truth (what the new path replaces and what stays)

| Fact | Evidence | Consequence |
|------|----------|-------------|
| Launch line is built inline and the `Child` is dropped by the caller (`_launcher`) | `[VERIFIED: crates/app/src/launch.rs:163-164]` `"-insecure -steam -novid -allow_third_party_software -netconport {port} -windowed -w {w} -h {h} +demo_ui_mode 0 +playdemo {demo_name}"`; `[VERIFIED: crates/app/src/jobs.rs:194]` `let _launcher = launch::launch(...)?;` | HLAE exit code is never observed today (LCH-06). New launch line adds `-condebug` and `-noConfig`/`-afxFixNetCon` per spikes. |
| Liveness fails open | `[VERIFIED: crates/app/src/launch.rs:258]` `Err(_) => true, // не смогли спросить — считаем, что жива, и ждём дальше` | TMO-02: replace with handle wait; error -> `Unknown`. |
| Kill by image name | `[VERIFIED: crates/app/src/launch.rs:271-272]` `Command::new("taskkill"); c.args(["/IM", "cs2.exe", "/F"]);` and `[VERIFIED: crates/app/src/jobs.rs:164]` `launch::kill_game();` | Replaced by `Session::terminate()` (TerminateJobObject). Phase 2 owns the staged teardown. |
| Retry timer for `exec highlights` | `[VERIFIED: crates/app/src/jobs.rs:181-186]` `const LOAD_CAP: Duration = Duration::from_secs(150);` `const EXEC_RETRY: Duration = Duration::from_secs(30);` `const EXEC_RETRIES: u32 = 4;` | D-10 removes the retry. `LOAD_CAP` 150 s is the starting `demo_load` deadline; `EXEC_RETRY` 30 s the starting `script` deadline. |
| Missing netcon is downgraded to a log line and recording continues | `[VERIFIED: crates/app/src/jobs.rs:229]` `p.log(&format!("{e}. Откройте консоль игры (~) и выполните: exec highlights"));` | Becomes `CONSOLE_NOT_OPEN` (D-04). |
| Marker matching is a blacklist | `[VERIFIED: crates/app/src/jobs.rs:199]` `let marker = \|l: &str, m: &str\| l.contains(m) && !l.contains("echo") && !l.contains("addAtTick");` | Replaced by anchored, nonce-stamped parser. |
| READY is sent by the app, not the cfg | `[VERIFIED: crates/app/src/jobs.rs:206]` `c.send("mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C_READY\"")` | New text: `... \"echo CS2C <nonce> READY\"`; still sent by the machine after `ConsoleUp`. |
| Take completion marker | `[VERIFIED: crates/renderer/src/lib.rs:242]` `pub const TAKE_DONE: &str = "CS2C_TAKE_DONE ";` emitted at `[VERIFIED: crates/renderer/src/lib.rs:229]` `... echo {TAKE_DONE}{i}; {tail}` | There is no SCRIPT and no TAKE_START marker today; both are new cfg lines. |
| Clip completion by mtime stability | `[VERIFIED: crates/app/src/launch.rs:231]` `fn newest_mtime(dir: &Path)`; `wait_for_clips` at `launch.rs:280` | Not carried over. Phase 1 closing = `quit` + wait for the job to empty (ffmpeg child included, A5); fallback keeps a short settle. |
| UI parses journal strings | `[VERIFIED: crates/app/src/status.rs:64]` `if let Some(rest) = m.strip_prefix("записан дубль ")` and `status.rs:71` `m.strip_prefix("склейка: ")` | The new path must keep emitting `записан дубль N/M` until the UI reads typed events. |
| Stage progress bar maps the stage string to `Stage::ALL` | `[VERIFIED: crates/app/src/jobs_page.rs:209]` `let idx = Stage::ALL.iter().position(\|s\| s.title() == stage).unwrap_or(0) as f32;` | The record sub-state must NOT be stored in `jobs.stage` (it would reset the bar to index 0). Use a new `phase` column. |
| `finish_job` stores only the string; `out_dir` is written only on success | `[VERIFIED: crates/store/src/lib.rs:310]` `Err(e) => c.execute("UPDATE jobs SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1", ...)` | Failure rows have no `out_dir`, so the card cannot name `record.log`. Add `set_out_dir` right after the folder is computed. |
| Schema has no version; `Job` is built only in `Store::row` | `[VERIFIED: crates/store/src/lib.rs:161]` `const SCHEMA: &str = "` and `lib.rs:236` `Ok(Job {`; grep over `crates` finds no other `Job {` literal | Adding `Job` fields is safe; migration via `PRAGMA user_version`. |
| `Progress` has three methods | `[VERIFIED: crates/pipeline/src/lib.rs:43-50]` `pub trait Progress: Sync { fn stage(&self, s: Stage); fn log(&self, msg: &str); fn cancelled(&self) -> bool { false } }` | Add `fn event(&self, e: &Event)` with a default body; all current implementers (`Quiet`, `Reporter`, `JobReporter`) compile unchanged. |
| Cancel sentinel | `[VERIFIED: crates/pipeline/src/lib.rs:53]` `pub const CANCELLED: &str = "отменено";`; `[VERIFIED: crates/app/src/jobs.rs:80]` `Err(e) if e == pipeline::CANCELLED => store.cancel_job(job.id),` | `RecordFailure{code: Cancelled}` must lower to exactly this string at the `run()` boundary. |
| "Takes already recorded" shortcut hashes cfg contents | `[VERIFIED: crates/app/src/jobs.rs:290]` `fn plan_hash(out_dir: &Path) -> String` over `highlights.cfg` and names starting `cs2c_` | Nonce inside the cfgs breaks it unless normalised (Pitfall 1). |
| Old settings knob | `[VERIFIED: crates/app/src/settings.rs:24]` `pub load_wait_secs: u32,` default `load_wait_secs: 25,` (`settings.rs:54`); UI `settings_page.rs:98` `egui::DragValue::new(&mut d.load_wait_secs).range(5..=180)` | Replace by `time_margin: f32` (struct has `#[serde(default)]`, so old `settings.json` still loads). |
| CI matrix | `[VERIFIED: .github/workflows/ci.yml]` `os: [ubuntu-latest, windows-latest]`, `run: cargo test --release --workspace` | `host` must compile on Linux (stub) and fake-based tests must pass on both. |

**What stays:** `launch::{find_cs2, find_hlae, check_paths, install_files, hlae_ffmpeg, nvenc_check, free_port, no_window}`, `run_ffmpeg` (optionally moved onto `Host::run`), `plan_player`, `JobReporter` (extended), `CancelToken`, `Stage`, `CANCELLED`, `Result<T,String>`.
**What goes (Phase 1):** `jobs::record`, `launch::{launch, Console, game_alive, kill_game, wait_for_clips, newest_mtime, dir_size}`, the `LOAD_CAP/EXEC_RETRY/EXEC_RETRIES/RECONNECT_EVERY` constants, `renderer::TAKE_DONE`. (`launch.rs` itself is deleted in Phase 4; keep discovery functions meanwhile.)

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `windows-sys` | 0.61.2 (`cargo search` shows `windows-sys = "0.61.2"`; `Cargo.lock` already resolves 0.61.2) | Job Object, `OpenProcess`, `WaitForSingleObject`, `GetExitCodeProcess`, (fallback) ToolHelp snapshot | Raw bindings, no wrapper types, already in the lock via eframe's dependency tree; every function used was confirmed present in the downloaded 0.61.2 source this session (see Code Examples) |
| `std` only | Rust 1.99.0 (`rustc 1.99.0`) | `mpsc` channel, `TcpStream`, `Child::try_wait`, threads, `Instant` | Matches the project constraint; no new crates for the bus |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tempfile` (dev) | optional | temp dirs in tests | Existing tests use `std::env::temp_dir().join(format!("cs2cinema-pipeline-{}", std::process::id()))` ([VERIFIED: crates/pipeline/src/lib.rs test `analyze_player_plan`]); follow that, no new dev-dep needed |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `std::sync::mpsc` behind `Session::poll(max_wait)` | `crossbeam-channel` 0.5.17 (in lock) | `select!` is unnecessary: all producers feed one channel; keeping `mpsc` avoids a dependency and the trait hides the choice anyway (CONTEXT leaves this to the planner) |
| Hand-written `RecordFailure` + `Display` | `thiserror` 2.0.21 (in lock) | No workspace crate uses it; Russian messages need custom formatting anyway |
| `std::process::Command::spawn` + immediate `AssignProcessToJobObject` | raw `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_JOB_LIST` (atomic assignment) or `CREATE_SUSPENDED` + resume | `Child::main_thread_handle` is unstable in 1.99 ([VERIFIED: run] `error[E0658]` ... "see issue #96723"), so suspended-resume needs raw Win32. The race window is HLAE.exe's .NET start-up (hundreds of ms) before it creates `cs2.exe`; the S1 spike logs `IsProcessInJob` for cs2 and the adopt fallback covers misses. Use raw `CreateProcessW` only if S1 shows misses. |
| Job Object IOCP notifications | polling `QueryInformationJobObject` every 250 ms in the watcher thread | IOCP delivery "is not guaranteed" per Microsoft [CITED: learn.microsoft.com job-objects]; polling the PID list + `WaitForSingleObject(cs2 handle)` is simpler and sufficient for Phase 1. Revisit in Phase 2 |

**Installation (`crates/host/Cargo.toml`):**
```toml
[package]
name = "host"
version = "0.1.0"
edition = "2024"

[dependencies]

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
  "Win32_Foundation",
  "Win32_Security",                    # SECURITY_ATTRIBUTES in CreateJobObjectW signature
  "Win32_System_JobObjects",
  "Win32_System_Threading",            # OpenProcess, WaitForSingleObject; also gates JOBOBJECT_EXTENDED_LIMIT_INFORMATION
  "Win32_System_Diagnostics_ToolHelp", # only for the adopt-by-PID fallback
] }
```
Workspace: add `"crates/host"` to `members` in `Cargo.toml`; `crates/pipeline/Cargo.toml` gets `host = { path = "../host" }` and `pub use host;` in `pipeline/src/lib.rs` so `app` and `cli` need no direct edge (D-01).

**Version verification:** `cargo search windows-sys --limit 1` -> `windows-sys = "0.61.2"`; `Cargo.lock` has `name = "windows-sys"` `version = "0.61.2"` ([VERIFIED: Cargo.lock:4976-4977]). The five-feature set above compiled and ran in a scratch crate (`cargo run --offline`, [VERIFIED: run]).

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| `windows-sys` | crates.io | first published 2021-10-18 | ~35.0M/wk | github.com/microsoft/windows-rs | OK (`gsd-tools query package-legitimacy check --ecosystem crates windows-sys`, signals: exists, not deprecated, no postinstall) | Approved; already in `Cargo.lock` |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none
No other external packages are introduced. `crossbeam-channel`, `thiserror`, `shared_child`, `which` named in project-level research are intentionally **not** added in this phase.

## Architecture Patterns

### System Architecture Diagram

```
 app::jobs::run(job)  ── plan_player ──► Plan{ takes: Vec<TakePlan>, nonce, ... }   (renderer writes cfgs with nonce)
        │  install_files(...)   (unchanged)          │  store.set_out_dir(id, out_dir)
        ▼
 pipeline::record(&dyn Host, &RecordRequest, &dyn Progress) -> Result<RecordReport, RecordFailure>
        │
        │   open RecordLog(out_dir/record.log)   build LaunchSpec(nonce, port, console.log path)
        ▼
  ┌──────────────────────────── driver loop (worker thread, slice ≤ 100 ms) ───────────────────────────┐
  │  ev = session.poll(TICK)           cancel = progress.cancelled()                                    │
  │  input = Cancel | Event(ev) | Tick           now = host.now()                                       │
  │  actions = machine.step(now, input)          (pure: states, deadlines, markers, facts)              │
  │  for a in actions: Send(line)→session.send | Emit(Event)→progress.event + RecordLog | Terminate | Finish│
  └───────────────▲──────────────────────────────────────────────────────────────────────┬─────────────┘
                  │ SessionEvent (one merged mpsc channel)                              │ on any exit:
                  │                                                                      ▼ session.terminate() (idempotent; Drop too)
  ┌───────────────┴────────────── host::WinHost session threads ──────────────────────────────────────────┐
  │ watcher: Child::try_wait(HLAE) → LauncherExited{code}; job PID list → GameAppeared{pid};              │
  │          WaitForSingleObject(cs2 handle, slice) → GameExited{code}; ActiveProcesses==0 → JobEmpty      │
  │ netcon : connect every 1 s (2 s timeout) → ConsoleUp; read lines (500 ms read timeout) → Line{Netcon}  │
  │ console.log tail (-condebug): offset tail, BOM strip, partial-line safe → Line{ConsoleLog}             │
  │ HLAE stdout / stderr pipes → Line{HlaeStdout|HlaeStderr}                                                │
  └─────────────────────────────────────────────────────────────────────────────────────────────────────┘
        HLAE.exe ──(CreateProcessW suspended, no breakaway)──► cs2.exe ──► HLAE's ffmpeg.exe   [all in our Job Object]

 on Ok:  jobs.rs continues → run_ffmpeg (assembly, with deadline)          on Err(RecordFailure):
 on Err: RunError{message, fail} ──► store.fail_job(id, JobFailure{ code, state, hint, tail, elapsed_ms })
                                      Cancelled ──► pipeline::CANCELLED sentinel ──► store.cancel_job (unchanged)
 UI: refresh_jobs() (already triggered on every Log/Stage msg) → card reads jobs.phase (live ladder) / fail_* (error block)
```

### Recommended Project Structure
```
crates/host/                       NEW leaf crate (no workspace deps)
├── Cargo.toml
├── src/
│   ├── lib.rs        # traits Host/Session, LaunchSpec, CmdSpec/CmdOutput, SessionEvent, Source, Liveness, HostError, default_host()
│   ├── fake.rs       # FakeHost + Scenario (plain `pub mod`, NOT cfg(test): pipeline/cli tests use it); virtual clock
│   ├── unsupported.rs# UnsupportedHost (cfg(not(windows))): launch() -> Err(HostError::Unsupported)
│   ├── runner.rs     # StdRunner: bounded external command run (moved from jobs::run_ffmpeg); optional late plan
│   └── win/          # cfg(windows)
│       ├── mod.rs    # WinHost, WinSession (threads, stop flags)
│       ├── job.rs    # Job: create / assign / pids / active_count / terminate / Drop  (the ONLY unsafe in Phase 1, besides process.rs)
│       ├── process.rs# OwnedProc: open by pid, liveness(), exit_code(), image path check
│       ├── netcon.rs # connector+reader thread
│       └── console_log.rs # offset tail thread
└── examples/spikes.rs   # S1..S4 harness (cfg(windows) main; prints "Windows only" elsewhere)

crates/pipeline/src/
├── lib.rs            # + `pub use host;`, `pub mod record;`, Progress::event, Event, Plan{takes, nonce}
└── record/
    ├── mod.rs        # pub fn record(); RecordRequest, RecordReport, driver loop
    ├── machine.rs    # RecordState, Machine::step (pure), Input/Action
    ├── markers.rs    # parse_marker(line, nonce) -> Option<Marker>
    ├── failure.rs    # FailCode, RecordFailure, Facts, Russian message/hint catalogue
    ├── deadlines.rs  # Deadlines (+ default(), scaled(mult), take_run(&TakePlan))
    ├── log.rs        # RecordLog (record.log), Tail (ring + fold)
    └── takes.rs      # TakePlan (re-export of renderer::takes), expected files
crates/renderer/src/lib.rs   # + Config.nonce, markers, `pub fn takes()`; remove TAKE_DONE
crates/store/src/lib.rs      # + user_version migration, set_phase, set_out_dir, fail_job, Job.fail_*/phase
crates/app/src/{jobs.rs,jobs_page.rs,settings.rs,settings_page.rs}   # thin cutover + card + ladder + multiplier
```

### Host trait design (what the planner should freeze)

D-03 says the trait hides processes, console and the clock. The one non-obvious decision: **make the wait part of the trait**, not only `now()`. If the driver calls `session.poll(Duration)` and `host.now()`, the fake can advance a virtual clock inside `poll` and a test with a 150 s deadline finishes instantly. If the fake exposed only a `Receiver` plus real `Instant`, deadline tests would sleep.

```rust
// crates/host/src/lib.rs   (sketch; names are recommendations)
pub enum Source { Netcon, ConsoleLog, HlaeStdout, HlaeStderr }       // record.log source labels: netcon, console.log, hlae-stdout, hlae-stderr (+ `app` added by the driver)

pub enum SessionEvent {
    LauncherExited { code: Option<i32> },   // HLAE.exe finished; Windows codes may be negative as i32 -> render as 0x...
    GameAppeared { pid: u32 },              // a cs2.exe is in our job (or adopted)
    ConsoleUp,                              // netcon TCP connect succeeded
    ConsoleDown,                            // socket closed / read error
    Line { source: Source, text: String },  // one console line, lossy UTF-8, trimmed, BOM stripped
    GameExited { code: Option<u32> },       // cs2 handle signalled
    JobEmpty,                               // job ActiveProcesses == 0 after GameAppeared (incl. HLAE's ffmpeg)
}

pub enum Liveness { NotStarted, Alive, Exited(Option<u32>), Unknown(String) }   // Unknown carries the OS error text

pub trait Session: Send {
    fn poll(&mut self, max_wait: Duration) -> Option<SessionEvent>;  // blocks <= max_wait; fake advances virtual time
    fn send(&mut self, line: &str) -> Result<(), HostError>;         // netcon only; rejects '\n'/'\r'; NotConnected if port not up
    fn game_liveness(&mut self) -> Liveness;                         // handle wait with timeout 0; error => Unknown
    fn launcher_exit(&self) -> Option<Option<i32>>;                  // None = still running / not seen
    fn terminate(&mut self);                                         // idempotent; TerminateJobObject; also on Drop
}

pub trait Host: Send + Sync {
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError>;
    fn now(&self) -> Duration;                                       // monotonic; fake = virtual
    fn run(&self, cmd: &CmdSpec, cancel: &dyn Fn() -> bool) -> Result<CmdOutput, HostError>; // bounded external command (ffmpeg); see Plan 6
}
pub fn default_host() -> Box<dyn Host>;   // WinHost on windows, UnsupportedHost elsewhere
```
`LaunchSpec { hlae_dir, cs2_exe, demo_name, size, nonce, console_log: Option<PathBuf>, extra_game_args }`; the host picks the free port itself (today's `free_port()` bind-then-release race is accepted, [VERIFIED: crates/app/src/launch.rs:156] `pub fn free_port() -> u16`). `HostError` variants: `Unsupported`, `Spawn(String)`, `JobAssign(String)`, `NotConnected`, `Io(String)`, `Timeout`, `Cancelled`.

Design rules that make it testable:
- No blocking wait on `Session` other than `poll(max_wait)`; **all deadlines are policy in `pipeline::record`**.
- `Line.source` is carried through so the driver can write `record.log` with the right label (D-... "Формат record.log").
- The fake is a plain `pub mod` so `crates/pipeline` tests (and later `crates/cli/tests`) reach it (`crates/cli/tests/pipeline.rs` is an integration test in another crate).
- The fake reacts to `send()`: when it receives a line containing `CS2C <nonce> READY` it schedules the READY marker line at a scenario-defined virtual delay; `exec highlights` schedules `SCRIPT`, then `TAKE_START i` / `TAKE_END i` per `TakePlan` (with per-take virtual durations). A `Scenario` can drop any one marker, delay it past its deadline, emit the launcher exit with a code, never emit `GameAppeared`, kill the game mid-take, or emit duplicate / stale-nonce lines.

### Record state machine

States (ladder order, D-06): `Launcher -> Game -> Console -> Demo -> Script -> Take(n) -> Closing -> Done`, plus terminal failure via `RecordFailure`. Two facts are tracked outside the state (LCH-06): `launcher: Running | Exited(code)` and `game: Liveness`.

| State | Entered when | Leaves on | Deadline (default, before multiplier) | Failure code on expiry |
|-------|-------------|-----------|---------------------------------------|------------------------|
| `Launcher` | `host.launch` returned | `LauncherExited{0}` (else `LAUNCHER_FAILED` at once for non-zero) | `launcher` 30 s [STACK.md budget] | `LAUNCHER_FAILED` ("HLAE.exe не завершился за N с — вероятно, показывает окно") |
| `Game` | launcher exited 0 | `GameAppeared` (may already have arrived: advance immediately) | `game_appear` 20 s after launcher exit | `GAME_NOT_STARTED` (message carries HLAE exit code) |
| `Console` | `GameAppeared` | `ConsoleUp` -> machine emits `Send(READY command)` | `console` 90 s (D-09 ~90 s) | `CONSOLE_NOT_OPEN` |
| `Demo` | READY command sent | marker `READY` | `demo_load` 150 s (existing `LOAD_CAP`) | `DEMO_NOT_LOADED` |
| `Script` | READY seen -> machine emits `Send("exec highlights")` **once** | marker `SCRIPT` | `script` 30 s (existing `EXEC_RETRY` value, now a deadline, not a retry) | `SCRIPT_NOT_RUN` |
| `Take(0)` wait-start | SCRIPT seen | `TAKE_START 0` | `first_take` 90 s [ASSUMED] | `TAKE_TIMEOUT` (D-10: missing `TAKE_START 0` is `TAKE_TIMEOUT`) |
| `Take(n)` recording | `TAKE_START n` | `TAKE_END n` | `take_run(n)` derived (formula below) | `TAKE_TIMEOUT` |
| `Take(n)` seek (n>0) | `TAKE_END n-1` | `TAKE_START n` | `take_seek` 60 s (D-09 "простой дубля ~60 с") | `TAKE_TIMEOUT` |
| `Closing` | `TAKE_END M-1` (or `ALL_DONE`) -> `Send("quit")` | `JobEmpty` (or `GameExited` + settle) | `game_close` 30 s | on expiry: `Terminate`, log a warning, **not** a failure (CONTEXT: "при неответе — закрыть job") |
| any before `Done` | `GameExited` | - | - | `GAME_EXITED` (state = state at death) |
| any | `Input::Cancel` | - | - | `CANCELLED` (wins over timeouts) |

Rules: markers are accepted only for the expected `(kind, n)`; a duplicate (netcon + console.log both deliver it) or an out-of-order marker is logged and ignored (idempotent); the nonce is checked by the parser. `TAKE_START n` arriving while expecting `n+1`, etc. only appears in the failure message ("пришла метка дубля k вместо n"). `Closing` waiting on `JobEmpty` relies on A5 (HLAE's ffmpeg is a child of `cs2.exe`, hence inside the job, and a graceful `quit` finalises streams [CITED: STACK.md, from HLAE `main.cpp` HostStateRequest hook]). If S1 shows ffmpeg is not in the job, `Closing` waits `GameExited` then a 3 s settle.

**Deadline formula for one take (D-12, TAK-01):** `take_run = game_secs * max(1, capture_fps / 60) * 2 + 30 s` where `game_secs = (end_tick - start_tick) / tick_rate` of the shot and `capture_fps` comes from `renderer::capture_fps(shot, cfg)`. Rationale: renderer's own note says "игра с записью рендерит ~60 кадров/с, и лишние кадры там, где замедления нет, растягивают запись в разы" ([VERIFIED: crates/renderer/src/lib.rs:136-138] doc comment on `capture_fps`). Factor 2 and 30 s are [ASSUMED] starting values; per-take actual wall time and the ratio `wall / game_secs` must be logged by Phase 1 so Phase 4 can tune (A6). The multiplier from Settings scales the whole table.

`Deadlines` (D-09): `launcher, game_appear, console, demo_load, script, first_take, take_seek, take_base_slack, take_factor, game_close, ffprobe, ffmpeg`, plus `fn scaled(&self, k: f32) -> Deadlines` and `Deadlines::default()`; tests pass a millisecond table (e.g. `Deadlines::fast()`). `ffprobe` is defined now but first used in Phase 3; `ffmpeg` (assembly) default `max(600 s, video_secs * 15)` [ASSUMED] and is applied in `run_ffmpeg` / `Host::run`.

**Driver loop** (the only place that touches the session and `Progress`): `TICK = 100 ms`; each iteration `session.poll(TICK)`, `p.cancelled()`, `host.now()`; Cancel is delivered as `Input::Cancel` before the event. The machine never sees `Instant`; `now` is a `Duration` argument, so a unit test can step the machine through `console` deadline expiry in one call.

### Markers (D-11)

Format: `CS2C <nonce> READY | SCRIPT | TAKE_START n | TAKE_END n | ALL_DONE`. `nonce`: 8 lowercase hex chars (no `:`, no quotes — the renderer documents "консоль режет аргументы по `:`, а `mirv_cmd` теряет кавычки" [VERIFIED: crates/renderer/src/lib.rs:17-18]). Generate without a new crate: `std::collections::hash_map::RandomState::new().build_hasher().finish()` formatted `{:08x}` (randomly seeded per `RandomState`, per std docs [ASSUMED: std docs recalled, not re-fetched]).

Emission points in `renderer::render` (current line numbers):
- `highlights.cfg`: first command line after the header comments -> `echo CS2C <nonce> SCRIPT` (new).
- `cs2c_NN.cfg`: `at(&mut s, shot.start_tick, "mirv_streams record start")` ([VERIFIED: crates/renderer/src/lib.rs:202]) -> `"mirv_streams record start; echo CS2C <nonce> TAKE_START <i>"`; the end line ([VERIFIED: crates/renderer/src/lib.rs:229]) -> replace `echo {TAKE_DONE}{i}` with `echo CS2C <nonce> TAKE_END <i>`; for the last shot the `None` tail (`"mirv_cmd clear; engine_no_focus_sleep 20; demo_pause"`) gets `echo CS2C <nonce> ALL_DONE;` in front.
- READY: not in a cfg; sent by the machine after `ConsoleUp`: `mirv_cmd clear; mirv_cmd addAtTick 64 "echo CS2C <nonce> READY"` (same shape as today).
- Renderer tests to update (they assert on the old text): lines containing `echo CS2C_TAKE_DONE 0; exec cs2c_02; demo_gototick 4872`, `echo CS2C_TAKE_DONE 1; mirv_cmd clear`, and `echo CS2C_TAKE_DONE 0; exec cs2c_02"` ([VERIFIED: crates/renderer/src/lib.rs:651, 654, 686]). `renderer::Config` gets `pub nonce: String` with `Default` `"cli"` so `Config::default()` in the CLI and its test still compile and run.

Parser (`markers::parse_marker(line, nonce) -> Option<Marker>`): strip a leading UTF-8 BOM, strip an optional `MM/DD HH:MM:SS ` prefix, `trim()`, then require the **entire** remaining string to equal `CS2C <nonce> KIND [n]`. This rejects (a) the netcon echo of our own command (`...\"echo CS2C <nonce> READY\"` ends with a quote and starts with `mirv_cmd`), (b) HLAE's `mirv_cmd` trace line (`<n>: <tick> (eps=1) -> <command>`, per STACK.md), (c) stale lines from an older run (different nonce). Local evidence for the file format: `[VERIFIED: console.log:39,40]` lines look like `10/09 01:37:32 [CommandLine] command line arguments:` and the file starts with bytes `ef bb bf` (checked with `xxd`). Netcon lines carrying no timestamp is [CITED: .planning/research/STACK.md], not observed by me (A4); S3 transcripts settle it.

### Typed failure

```rust
// pipeline::record::failure
pub enum FailCode { LauncherFailed, GameNotStarted, ConsoleNotOpen, DemoNotLoaded, ScriptNotRun, TakeTimeout, GameExited, HlaeError, Cancelled }
impl FailCode { pub fn as_str(&self) -> &'static str /* "LAUNCHER_FAILED" ... D-08 */ }
pub struct RecordFailure {
    pub code: FailCode, pub state: RecordState,
    pub message: String,   // Russian, includes the facts line
    pub hint: String,      // Russian
    pub elapsed: Duration, // time spent in the failed state when the deadline fired
    pub console_tail: Vec<String>,   // folded, ~30 entries (Tail::render)
    pub launcher: LauncherFact, pub game: Liveness,   // extra fields for LCH-06 (rendered into message)
}
```
Proposed Russian copy (planner may polish; the catalogue must live in one module):

| Code | message | hint |
|------|---------|------|
| `LAUNCHER_FAILED` | «HLAE не запустился: {причина}» / «HLAE.exe завершился с кодом {код}» / «HLAE.exe не завершился за {N} с — вероятно, показывает окно с ошибкой» | «Проверьте папку HLAE в настройках и запустите HLAE.exe вручную — он покажет причину.» |
| `GAME_NOT_STARTED` | «CS2 не запустилась за {N} с после HLAE (HLAE.exe: код {код}, cs2.exe: не запускалась)» | «Убедитесь, что Steam запущен, CS2 не обновляется, а антивирус не блокирует AfxHookSource2.dll.» |
| `CONSOLE_NOT_OPEN` | «Игра запущена, но консольный порт не открылся за {N} с (cs2.exe: жива)» | «Закройте CS2 и повторите. Смотрите строки [console.log] в record.log: возможно, HLAE не подходит к текущему обновлению CS2.» |
| `DEMO_NOT_LOADED` | «Консоль открыта, но демка не загрузилась за {N} с» | «Демка могла быть повреждена или записана в другой версии CS2 — откройте её в игре вручную.» |
| `SCRIPT_NOT_RUN` | «Демка загружена, но скрипт highlights не выполнился: метка SCRIPT не пришла за {N} с» | «Проверьте, что в game\csgo\cfg лежат highlights.cfg и cs2c_*.cfg, а в пути результатов нет пробелов.» |
| `TAKE_TIMEOUT` | «Дубль {n} из {M}: метка {TAKE_START|TAKE_END} не пришла за {N} с» | «Если в консоли повторяется «Failed writing image», проверьте ffmpeg HLAE (Path в ffmpeg\ffmpeg.ini).» |
| `GAME_EXITED` | «Игра закрылась на этапе «{состояние}» (cs2.exe: не жива, код {код})» | «Игру могли закрыть вручную или она упала — см. последние строки консоли.» |
| `HLAE_ERROR` | «HLAE сообщил об ошибке: {первая строка AFXERROR}» | «Проверьте ffmpeg HLAE: Path в ffmpeg\ffmpeg.ini должен указывать на рабочий ffmpeg.exe.» |
| `CANCELLED` | «Отменено» (lowered to the sentinel `pipeline::CANCELLED`) | - |

`HLAE_ERROR` is in D-08 but never defined. Cheapest consistent definition that keeps `AFXERROR` fail-fast in Phase 2 (LCH-05): in Phase 1, when a **deadline** fires (any state) and the tail contains an `AFXERROR` line, report `HLAE_ERROR` instead of the timeout code, with the first `AFXERROR` line in the message (evidence: the author's `console.log` contains 1291 `AFXERROR` lines, first at line 930 `AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: CreateProcessW.` followed by `AFXERROR: Failed writing image for screen recording.` [VERIFIED: console.log:930-932]). Flagged as Open Question 2.

Facts line (LCH-06, success criterion 4): journal and message both show e.g. `HLAE.exe: код 0 · cs2.exe: жива` / `cs2.exe: не жива (код 0xC0000005)` / `cs2.exe: неизвестно (отказано в доступе)` / `HLAE.exe: ещё работает`. `Liveness::NotStarted` renders «не запускалась».

### Store, bridge and UI

- **Migration** (`Store::init`): after `execute_batch(SCHEMA)`, read `PRAGMA user_version`; if `< 1`, in one transaction `ALTER TABLE jobs ADD COLUMN fail_code TEXT; ... fail_state TEXT; ... fail_hint TEXT; ... fail_tail TEXT; ... fail_elapsed_ms INTEGER; ... phase TEXT NOT NULL DEFAULT ''` then `PRAGMA user_version = 1`. It works for fresh DBs too (they are created by `SCHEMA` without the columns, then migrated). `Job` gains `fail_code/fail_state/fail_hint/fail_tail: Option<String>`, `fail_elapsed_ms: Option<i64>`, `phase: String`; `row()` uses named gets so `SELECT *` picks them up. `start_job` and `requeue` must also null `fail_*` and reset `phase`. D-05 lists four columns; **`fail_elapsed_ms` is a fifth** (needed by D-06 "прошедшее время" as a separate card field) — extension, not contradiction; confirm in Open Question 3.
- **New store methods:** `set_phase(id, &str)`, `set_out_dir(id, &Path)` (call right after `out_dir` is computed in `run()` so failed cards can name `record.log`), `fail_job(id, &JobFailure)` where `JobFailure { message, code, state, hint, tail, elapsed_ms }` is plain strings (no `pipeline` type).
- **Bridge in `jobs.rs`:** `run()` returns `Result<JobOutcome, RunError>`; `struct RunError { message: String, fail: Option<pipeline::RecordFailure> }` with `impl From<String> for RunError` so every existing `?` on `Result<_, String>` keeps compiling. `run_next` match becomes: `Ok` -> `finish_job`; `Err(e) if e.message == pipeline::CANCELLED` -> `cancel_job`; `Err(e)` with `fail` -> `fail_job`; else `finish_job(Err)`. `Msg::JobDone { result: Err(String) }` is unchanged so the status bar and `poll()` keep working.
- **`Progress::event`:** `pub enum Event { State { state: RecordState, take: Option<(u32,u32)> }, Fact(String) }` (names are suggestions). Default body `self.log(&e.to_string())`. `JobReporter::event` overrides: `store.set_phase(id, state.code())`, appends the Russian journal line, and for take completion also emits the legacy line `записан дубль N/M` through `log` so `Status::push` parsing ([VERIFIED: crates/app/src/status.rs:64]) keeps feeding `app.status.takes` until the UI reads events.
- **Card ([VERIFIED: crates/app/src/jobs_page.rs:113-117]):** the `JobStatus::Failed` arm currently renders `ui::pill(ui, "ошибка", BAD)` plus a truncated label. Extend: a state-ladder row (all `RecordState::LADDER` titles; passed = those before `fail_state`/current, failed or current highlighted; take step shows `N/M` from `app.status.takes` while running), then for failed rows with `fail_code`: small `code · состояние` text, message, hint (MUTED), «прошло N с», and `egui::CollapsingHeader` with the monospace tail. The ladder also renders for the `running` job using `job.phase`. Keep the existing `Журнал` toggle.
- **Settings:** remove `load_wait_secs` + its `DragValue` row; add `time_margin: f32` default `1.0`, UI row «Запас по времени» (`DragValue` 0.5..=5.0, `×`). `Settings::default()` calls `crate::launch::find_cs2()` ([VERIFIED: crates/app/src/settings.rs:44]) so keep that wiring.

### record.log and console tail

- `RecordLog` is owned by the **driver** (not the pure machine and not the host): path `out_dir/record.log`; open in append mode and write a separator `==== запись {local time} nonce {nonce} ====` (RET-06 numbering is Phase 3; append avoids erasing evidence now); line format `HH:MM:SS.mmm [source] text` with sources `netcon`, `console.log`, `hlae-stdout`, `hlae-stderr`, `app` (the last one for state changes, facts, deadlines, sent commands); `write_all` + `flush` per line (hundreds to thousands of lines; an `AFXERROR` storm is the worst case) and a final `flush` in `Drop`, so every exit path (success, failure, panic unwinding, early `?`) leaves a complete file. Writing is verbatim: D-07 says the full console is always in `record.log`, folding applies only to the tail.
- `Tail`: ring of the last ~500 raw entries; `render(30)`: strip timestamp prefix, fold consecutive identical entries to `text ×N`, then take the last 30 folded entries, and **pin** the first `AFXERROR` line and the last marker line even if they fall outside the window (D-07 "сохраняются всегда"). Test with a synthetic 800-line `Failed writing image` storm preceded by the `CreateProcessW` line: tail must contain `CreateProcessW` and `Failed writing image for screen recording. ×800`.

### Cancellation and `pipeline::CANCELLED`

Cancel is a first-class `Input::Cancel` checked every slice (≤100 ms). In Phase 1: transition to failure `Cancelled`, `Action::Terminate` (TerminateJobObject), then return; the driver wraps `record` so that `session.terminate()` also runs on every other return path and in `Drop`. `jobs::run` maps `FailCode::Cancelled` to `Err(RunError::from(pipeline::CANCELLED.to_owned()))` so `run_next` still picks `cancel_job` and the UI string comparison in `app.rs` (`Err(e) if e == pipeline::CANCELLED`) is unchanged. `launch::kill_game()` call at `jobs.rs:164` disappears. The 3 s / 10 s cancel guarantees and staged teardown are Phase 2 (TMO-03/04).

### Pattern: pure machine test

```rust
// Source: this research (design); style follows crates/cli/tests/pipeline.rs sentence-named tests
#[test]
fn console_that_never_opens_fails_with_console_not_open_after_the_deadline() {
    let d = Deadlines::fast();                       // console = 90 ms etc.
    let mut m = Machine::new(takes(3), d.clone(), "ab12cd34".into());
    m.step(ms(0), Input::Event(SessionEvent::LauncherExited { code: Some(0) }));
    m.step(ms(10), Input::Event(SessionEvent::GameAppeared { pid: 7 }));
    let acts = m.step(ms(10) + d.console + ms(1), Input::Tick);
    assert!(matches!(finish(&acts), Err(f) if f.code == FailCode::ConsoleNotOpen && f.state == RecordState::Console));
}
```

### Test strategy (Validation Architecture section omitted: `workflow.nyquist_validation` is `false` in `.planning/config.json`)

- `cargo test --release --workspace` (CI command) must pass on ubuntu and windows with no CS2: `host::fake` + `pipeline::record` tests give success criterion 5.
- Minimum scenarios on the fake (requirement says success + missing marker; cheap to cover all codes): happy path with M=3 (assert state order and the `записан дубль N/M` lines); each marker dropped in turn (`READY`->`DEMO_NOT_LOADED`, `SCRIPT`->`SCRIPT_NOT_RUN`, `TAKE_START 0`/`TAKE_END k`->`TAKE_TIMEOUT`); launcher exit code 1 -> `LAUNCHER_FAILED` (message contains the code); never `GameAppeared` -> `GAME_NOT_STARTED`; `ConsoleUp` never -> `CONSOLE_NOT_OPEN`; `GameExited` mid-take -> `GAME_EXITED` with facts «не жива»; liveness `Unknown` rendered «неизвестно», never «жива»; stale-nonce and duplicate markers ignored; cancel in the middle of `Demo` -> `CANCELLED` and `terminate()` called; `record.log` exists after success and after failure and contains tagged lines.
- Parser unit tests incl. echo of the READY command, trace line, BOM + timestamp prefix, wrong nonce. Renderer tests updated; one new renderer test per marker.
- Store tests: migration on a DB created with the old schema; `fail_job` round trip; `start_job` clears `fail_*`.
- Golden transcripts: after S3, copy real console captures to `crates/pipeline/tests/fixtures/*.log` and add a replay scenario to the fake (PITFALLS #34: fakes encode the author's mental model).
- Keep `crates/cli/tests/pipeline.rs` green (nonce default).

### Anti-Patterns to Avoid
- **Blocking waits on the session** (`wait_for_exit()`): they defeat the virtual clock and cancellation.
- **Machine holding `Instant` or doing I/O:** keep it pure; the driver owns time, files, sockets.
- **Storing the record sub-state in `jobs.stage`:** breaks the progress bar (`jobs_page.rs:209`).
- **Parsing the journal text in the new UI:** use `jobs.phase` / `fail_*`.
- **Using `taskkill`/`tasklist` anywhere in the new path.**
- **Joining pipe-reader threads before the game is dead** (see Pitfall 4).

## Spikes (start of the phase; run by the author on his machine)

The harness lives in `crates/host/examples/spikes.rs` (subcommands `job`, `netcon`, `failures`, `abspath`), built on the real `win/job.rs` so the code is not throwaway. Output goes to `%LOCALAPPDATA%\cs2cinema\spikes\<timestamp>\` (transcripts + `summary.txt`); results are written by hand into `.planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md`. Preconditions on the author's machine (checked now): Steam is running, **a `cs2.exe` (PID 48660 at probe time) is currently running** and must be closed first, `C:` has 22 GB free, HLAE at `C:\Program Files (x86)\HLAE` with `x64\AfxHookSource2.dll`, CS2 `game\bin\win64\cs2.exe` present, a real demo exists in `game\csgo` (`1-3e7db9e3-8e92-4761-a1db-3729fb7de11c-1-1.dem`).

| # | Question | Procedure | Primary path if it passes | Fallback if it fails | Evidence quality before the spike |
|---|----------|-----------|---------------------------|----------------------|-----------------------------------|
| S1 | Does `cs2.exe` (and HLAE's ffmpeg) land in our Job Object when HLAE's loader creates it? | Create job (`KILL_ON_JOB_CLOSE`), spawn HLAE with the real launch line, `AssignProcessToJobObject` immediately, poll `JobObjectBasicProcessIdList` every 500 ms for 60 s printing pid + `QueryFullProcessImageNameW`; call `IsProcessInJob` for cs2; start a short take if possible to see `ffmpeg.exe`; then `TerminateJobObject` and verify no process remains. Also note: does the HLAE stdout pipe reach EOF when HLAE exits or only when cs2 exits? | `WinHost`: job-owned PID list + handle wait (D-02) | Adopt: ToolHelp snapshot for a new `cs2.exe` (image path equals expected, creation time after launch) -> `OpenProcess(PROCESS_SET_QUOTA\|PROCESS_TERMINATE\|SYNCHRONIZE\|PROCESS_QUERY_LIMITED_INFORMATION)` -> `AssignProcessToJobObject`; if even that is refused: PID-only mode (keep the handle for liveness, `TerminateProcess(handle)` for kill; still never by image name). Liveness (TMO-02) is satisfiable in every case | Documented + source: `Loader.cs` flags `CREATE_NEW_PROCESS_GROUP \| DETACHED_PROCESS \| CREATE_SUSPENDED`, no breakaway [CITED]; Microsoft default inheritance [CITED]; generic grandchild inheritance observed in my scratch run [VERIFIED: run]. HLAE-specific: not observed |
| S2 | Does `-afxFixNetCon` make the netcon port reliable and how fast? Which address does it bind? | 5 launches without and 5 with the flag (same demo); per launch: time from `GameAppeared` to first successful `connect`; at the end of each, `netstat -ano` line for the port (127.0.0.1 vs 0.0.0.0); `quit` and wait for job empty | Always pass `-afxFixNetCon` if with >= without | Netcon-only commands remain (D-04); if netcon is flaky even with the flag, the typed `CONSOLE_NOT_OPEN` failure is the Phase 1 outcome and `console.log` stays marker/AFXERROR evidence. Boot-cfg command injection is a deferred idea [LOW] | Flag semantics verified in source this session [CITED: AfxHookSource2/main.cpp: `WSAStartup(MAKEWORD(2, 0), ...)` under `FindParam(L"-afxFixNetCon")`]; its effect on this machine is unobserved. The author's old session used `-netconport 2121` with no fix flag ([VERIFIED: console.log:40]) |
| S3 | What does the console print on real failures, and when is READY lost? | Capture netcon + `console.log` transcripts for: (a) `+playdemo` of a non-existent name, (b) a zero-byte / truncated `.dem`, (c) `exec` of a missing cfg through netcon, (d) HLAE ffmpeg path broken (reproduce the observed `CreateProcessW` storm), (e) **late netcon connect**: connect after the demo is past tick 64 and see whether `mirv_cmd addAtTick 64` READY ever fires, vs `mirv_cmd addTick` | Parser needs only the nonce markers; transcripts become fixtures and optional fast-fail patterns | No failure-text parsing in Phase 1: rely on deadlines (`DEMO_NOT_LOADED` etc.). If (e) shows READY is lost on late connect, add the console.log line `ChangeGameUIState: CSGO_GAME_UI_STATE_LOADINGSCREEN -> CSGO_GAME_UI_STATE_INGAME` ([VERIFIED: console.log:762]) as an alternative "demo is running" evidence that triggers a fresh READY request | Known text, observed: `[VERIFIED: console.log:247]` `[InputService] exec: couldn't exec '{*}cfg/gamemode_competitive_server.cfg', unable to read file` (a different file; the same wording for `highlights.cfg` is an inference, A9); `[Demo] Requesting playback of '...'` and `[HostStateManager] Host activate: Playing Demo (...)` ([VERIFIED: console.log:680,767]). Text of failed `playdemo`: no sample anywhere |
| S4 | Does an absolute path in `+playdemo` survive HLAE `-cmdLine` quoting (spaces, Cyrillic)? | Launch with `+playdemo "C:/path with space/демо.dem"` through the real `-cmdLine`; check for `Host activate: Playing Demo (...)` | Informs Phase 2 (stop copying demos). **Not blocking Phase 1**: `install_files` stays | Keep copy-then-delete (TMO-06) | Absolute path plays when typed manually: `[VERIFIED: console.log:2401]` `[Demo] Requesting playback of 'C:/Users/kamal/Downloads/1-3e7db9e3-...-1-1.dem'`; through `-cmdLine`: unverified |

`01-SPIKES.md` structure: one block per spike with *command run*, *observed output (paste)*, *verdict PASS/FAIL*, *chosen path (primary/fallback)*, *consequence for `WinHost`*. `WinHost` implementation plans must reference it.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Own only our processes | Name-based kill, PID lists from `tasklist` | Job Object (`windows-sys`) + `OpenProcess` handles | `taskkill /IM` kills the user's own CS2; `tasklist` text is localised and fails open |
| Liveness | Polling a spawned `tasklist` | `WaitForSingleObject(handle, 0)` tri-state | One syscall; error maps to `Unknown` |
| Bounded wait with cancel | `thread::sleep` loops with ad-hoc caps | `Session::poll(≤100 ms)` + `Machine` deadlines | One place decides time |
| Wall-clock for tests | `thread::sleep` in tests, mocking `Instant` | Virtual clock inside the fake `poll` | Deadline tests run instantly |
| Log folding | Ad-hoc `dedup` at call sites | One `Tail` type with tests | The `AFXERROR` storm must not evict the cause |
| Nonce | A random-number crate | `RandomState` hash | No dependency for a non-secret run id |
| Schema versioning | Dropping the DB / checking columns by hand | `PRAGMA user_version` + transactional `ALTER TABLE` | Existing user DB must survive (D-05 one-way) |
| Marker parsing | `contains(...)` blacklist | Whole-line anchored parser | Echo, trace and stale lines all contain the marker text |

**Key insight:** every reliability bug in `launch.rs` comes from a decision being scattered across closures (time, liveness, retries, parsing). Collapse them into one pure function over `(now, Input)` and one adapter that reports facts.

## Common Pitfalls

### Pitfall 1: The nonce silently disables the "takes already recorded" shortcut
**What goes wrong:** `plan_hash` hashes the bytes of `highlights.cfg` and `cs2c_*`. With a per-plan nonce inside them, the hash differs every run, so an assembly failure followed by a retry re-records everything (hours on long plans).
**Why it happens:** the hash was designed when cfg content depended only on the plan.
**How to avoid:** hash with the nonce normalised: read each file, `replace(&nonce, "NONCE")` before feeding FNV (the plan knows `plan.nonce`); or store the nonce in a side file and skip it. Add a test: two `render()` calls differing only by nonce yield the same hash.
**Warning signs:** journal never prints «дубли этого плана уже записаны — сразу склеиваю» ([VERIFIED: crates/app/src/jobs.rs:151]).

### Pitfall 2: Late netcon connect makes READY unreachable
**What goes wrong:** READY is `mirv_cmd addAtTick 64 "echo ..."`; if the console connects after the demo has passed tick 64 (slow netcon, 90 s window), the command never fires and the job reports `DEMO_NOT_LOADED` although the demo plays.
**Why it happens:** same limitation exists today; netcon usually connects while loading.
**How to avoid:** S3(e) measures it. Until then log, on `DEMO_NOT_LOADED`, whether `console.log` showed `Host activate: Playing Demo` / `LOADINGSCREEN -> INGAME`; the message then says «демка идёт, но метка READY не пришла» (still a failure, now diagnosable).
**Warning signs:** `ConsoleUp` takes > 30 s before a `DEMO_NOT_LOADED`.

### Pitfall 3: Cancel/kill leaves HLAE's ffmpeg or reader threads behind
**What goes wrong:** killing only `cs2.exe` leaves a truncated take and an orphaned ffmpeg; threads blocked on a socket/pipe live on.
**How to avoid:** all processes in one job (`TerminateJobObject`); every thread has a stop flag and a read timeout (`TcpStream::set_read_timeout(500 ms)`); `terminate()` also `shutdown(Both)` on the socket; `WinSession::drop` calls `terminate()`.
**Warning signs:** after a cancelled job `tasklist` still shows `ffmpeg.exe`; thread count grows across jobs.

### Pitfall 4: Pipe handle inheritance holds HLAE's stdout open until the game dies
**What goes wrong:** HLAE creates `cs2.exe` with `bInheritHandles = true` ([CITED: Loader.cs]), so the write ends of the pipes we gave HLAE (`Stdio::piped`) may be inherited by `cs2.exe`; a reader thread then sees EOF only when the game exits, and a `join()` right after HLAE exits would hang. [ASSUMED, A3]
**How to avoid:** never `join` reader threads on the success path before `terminate()`/game exit; use detached threads that end on EOF or on the stop flag; S1 records when EOF arrives.

### Pitfall 5: Treating "HLAE.exe exited" as "game exited" (or vice versa)
**What goes wrong:** HLAE is a launcher that exits after `Sleep(2000)` + `ResumeThread` ([CITED: Loader.cs]); old comment in the code says the same ([VERIFIED: crates/app/src/launch.rs:249-250] `HLAE.exe — только лаунчер и завершается сразу после старта cs2`). A machine that fails on launcher exit would break every run.
**How to avoid:** two independent facts (LCH-06). `LauncherExited{0}` is a normal transition; only non-zero or "still running at deadline" is a failure. On failure HLAE shows a modal error dialog and returns only after it is closed ([CITED: Program.cs / STACK.md]), so an unattended failure appears as "launcher still running at the deadline" -> `LAUNCHER_FAILED` with that exact wording.

### Pitfall 6: Exit codes on Windows are `u32`
**What goes wrong:** `ExitStatus::code()` returns `i32`; a crash code such as `0xC0000005` prints as a negative number.
**How to avoid:** format with `{:#x}` after `as u32`; keep `Option` for "unknown".

### Pitfall 7: Marker emitted but file not finished (do not reintroduce mtime)
**What goes wrong:** `TAKE_END n` is emitted by the game's command queue, not when ffmpeg closes the file; HLAE's own "Finishing recording ... done." only queues shutdown (STACK.md, from source). In Phase 1, `Closing` therefore waits for `JobEmpty` after `quit`. Phase 3 adds `ffprobe` (TAK-04).
**Warning signs:** last take truncated after a `Closing` that hit its `game_close` deadline; log that case as a warning with the take number.

### Pitfall 8: `tempdir`/`out_dir` with spaces breaks cfg paths
Not new, but the checks stay: `renderer::console_safe` ([VERIFIED: crates/renderer/src/lib.rs:145-146]) is still called in `run()`; the nonce must pass the same character rule (hex only).

### Pitfall 9: `Job` fields and mock/test code
`Job` is only constructed in `Store::row`, so adding fields is safe; but `store` tests compare `Job` values only by selected fields, and `Job` derives `PartialEq` over `log: String` (can be large). No change needed, but do not add non-`PartialEq` fields.

### Pitfall 10: The test helper runs forever if the fake clock never advances
In the fake, `poll(max_wait)` must always advance virtual time by `max_wait` when no event is due, otherwise a missing-marker scenario loops forever instead of hitting the deadline. Add a safety cap (e.g. abort the test after 10 virtual hours).

## Code Examples

### Job Object, handle liveness, terminate (compiled and run this session)
```rust
// Source: [VERIFIED: run] scratch crate, windows-sys 0.61.2, rustc 1.99.0, Windows 11 (output: pids in job [cmd, ping]; wait(0)=258 then wait=0 after TerminateJobObject)
use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Foundation::*;                 // HANDLE, WAIT_TIMEOUT, WAIT_OBJECT_0
use windows_sys::Win32::System::JobObjects::*;         // CreateJobObjectW, AssignProcessToJobObject, ...
use windows_sys::Win32::System::Threading::*;          // OpenProcess, WaitForSingleObject, GetExitCodeProcess

unsafe {
    let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());            // null => failure
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    SetInformationJobObject(job, JobObjectExtendedLimitInformation,
        (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
        size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
    let child = std::process::Command::new("cmd").args(["/C", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
    let ok = AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);   // 0 => failure (GetLastError)
    // liveness by handle: SYNCHRONIZE is required by WaitForSingleObject [CITED: learn.microsoft.com WaitForSingleObject]
    let h = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, child.id());
    match WaitForSingleObject(h, 0) {
        WAIT_TIMEOUT => { /* Alive */ }
        WAIT_OBJECT_0 => { let mut code = 0u32; GetExitCodeProcess(h, &mut code); /* Exited(Some(code)) */ }
        _ /* WAIT_FAILED or h.is_null() */ => { /* Unknown(GetLastError text) — NEVER Alive */ }
    }
    TerminateJobObject(job, 1);          // kills cmd and its ping child; CloseHandle(job) with KILL_ON_JOB_CLOSE does the same
}
// PID list: QueryInformationJobObject(job, JobObjectBasicProcessIdList, buf, len, &mut ret) with a usize buffer;
// layout [VERIFIED: windows-sys-0.61.2 .../JobObjects/mod.rs:63-67] `pub struct JOBOBJECT_BASIC_PROCESS_ID_LIST { pub NumberOfAssignedProcesses: u32, pub NumberOfProcessIdsInList: u32, pub ProcessIdList: [usize; 1] }`
// ActiveProcesses: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION.ActiveProcesses via JobObjectBasicAccountingInformation.
```
Notes: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` needs the `Win32_System_Threading` feature ([VERIFIED: windows-sys-0.61.2 JobObjects/mod.rs:113] `#[cfg(feature = "Win32_System_Threading")]`); `HANDLE` is `*mut c_void` ([VERIFIED: Foundation/mod.rs:5119]); constants quoted: `WAIT_OBJECT_0 = 0u32`, `WAIT_TIMEOUT = 258u32`, `WAIT_FAILED = 4294967295u32`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 8192u32`, `PROCESS_SYNCHRONIZE = 1048576u32`, `PROCESS_QUERY_LIMITED_INFORMATION = 4096u32`. Wrap in a safe `Job` type with `Drop` (close handle); keep every `unsafe` block in `crates/host/src/win/` with a `// SAFETY:` comment (workspace has no other `unsafe` besides `crates/demo/build.rs`, per project research).

### Launch line (recommended, subject to spikes)
```text
HLAE.exe -customLoader -noGui -autoStart -noConfig -hookDllPath <abs>\x64\AfxHookSource2.dll -programPath <abs>\cs2.exe
         -cmdLine "-insecure -steam -novid -allow_third_party_software -condebug -netconport <port> [-afxFixNetCon]
                   -windowed -w W -h H +demo_ui_mode 0 +playdemo <name>"
```
`-noConfig` handled in `hlae/Program.cs` (`Array.Exists(argv, p => p == "-noConfig")`) [CITED]; `-condebug` mirrors the console into `game/csgo/console.log` (author's log is 4 MB / 29,781 lines [VERIFIED: file inspection]); `-afxFixNetCon` per S2. **Left out of Phase 1 on purpose:** `-afxDisableSteamStorage` (changes cloud-config behaviour; unrelated to the requirements; revisit in Phase 2) and `-console`.

### Migration
```rust
// Source: this research (pattern); uses rusqlite 0.32 API already in store
let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
if v < 1 {
    conn.execute_batch("BEGIN;
        ALTER TABLE jobs ADD COLUMN fail_code TEXT;
        ALTER TABLE jobs ADD COLUMN fail_state TEXT;
        ALTER TABLE jobs ADD COLUMN fail_hint TEXT;
        ALTER TABLE jobs ADD COLUMN fail_tail TEXT;
        ALTER TABLE jobs ADD COLUMN fail_elapsed_ms INTEGER;
        ALTER TABLE jobs ADD COLUMN phase TEXT NOT NULL DEFAULT '';
        PRAGMA user_version = 1;
        COMMIT;").map_err(err)?;
}
```

## State of the Art

| Old Approach (in repo) | Current Approach | Impact |
|------------------------|------------------|--------|
| `tasklist` text + `Err => true` | handle wait, tri-state | no false "alive", no process spawn per second |
| `taskkill /IM cs2.exe` | Job Object terminate | only our processes |
| mtime-stable folder = take done | nonce markers (+ job empty now, ffprobe in Phase 3) | no half-written reads |
| `exec` retried on a timer | one `exec`, `SCRIPT` ack, named deadline | no duplicate takes |
| blacklist substring marker match | anchored nonce parser | immune to echo/trace/stale lines |

**Deprecated/outdated:** `renderer::TAKE_DONE` / `CS2C_TAKE_DONE`; `launch::{game_alive, kill_game, wait_for_clips}`; `Settings::load_wait_secs`.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `cs2.exe` created by HLAE's loader lands in our job (no breakaway in practice; nested job context fine) | Spikes S1, Architecture | Medium: `WinHost` must use the adopt-by-PID fallback; design already covers it |
| A2 | `HLAE.exe` writes nothing to stdout/stderr (only `Program.cs` inspected; `Loader.cs`/`MainForm` not searched for console output) | Summary, record.log | Low: pipes are captured anyway; the stream may simply be empty |
| A3 | `cs2.exe` inherits the stdout/stderr pipe handles we give HLAE, so EOF comes only at game exit | Pitfall 4 | Low-Medium: affects when reader threads end; S1 observes it |
| A4 | Netcon lines arrive without the `MM/DD HH:MM:SS` prefix and one marker per line | Markers | Low: parser strips an optional prefix either way |
| A5 | HLAE's ffmpeg is a child of `cs2.exe` hence inside the job, and a graceful `quit` finalises streams, so `JobEmpty` after `quit` means files are closed | State machine `Closing` | Medium: last take may be truncated; Phase 3 `ffprobe` catches it; fallback is a 3 s settle |
| A6 | `take_run = game_secs * max(1, capture_fps/60) * 2 + 30 s`, `first_take` 90 s, `take_seek` 60 s | Deadlines | Medium: false `TAKE_TIMEOUT` on slow GPUs; tunable via the multiplier, retuned in Phase 4 |
| A7 | `RandomState`-based nonce is acceptable (not a secret) | Markers | Low |
| A8 | Starting deadlines: `launcher` 30 s, `game_appear` 20 s, `console` 90 s, `demo_load` 150 s, `script` 30 s, `game_close` 30 s, ffmpeg `max(600 s, video_secs*15)` | Deadlines | Medium: user decision D-09 says "подстраиваются в фазе 4"; confirm defaults |
| A9 | A missing `highlights.cfg` prints `[InputService] exec: couldn't exec '...highlights.cfg', unable to read file` | Spikes S3 | Low: only used as an optional fast-fail pattern |
| A10 | Late netcon connect loses the `addAtTick 64` READY | Pitfall 2 | Medium: causes false `DEMO_NOT_LOADED`; S3(e) decides |
| A11 | Effect of `-afxFixNetCon` on this machine's netcon flakiness | Spikes S2 | Medium |
| A12 | CS2 netcon binds to loopback only (not 0.0.0.0) | Security | Medium (LAN exposure); S2 checks with `netstat` |
| A13 | Defining `HLAE_ERROR` as "deadline fired while an `AFXERROR` line is in the tail" matches the user's intent for D-08 | Typed failure | Low: naming only; see Open Question 2 |

## Open Questions

1. **Do the starting deadlines in the table match the user's expectation?**
   - What we know: D-09 gives only console ~90 s and take idle ~60 s as starting points; others come from existing constants (`LOAD_CAP` 150 s, `EXEC_RETRY` 30 s) and the project research budget.
   - What's unclear: `first_take`, `take_seek`, `game_close`, ffmpeg deadline are my choices (A6, A8).
   - Recommendation: plan them as constants in `Deadlines::default()` with the multiplier; list them in `01-SPIKES.md` follow-up for review.

2. **What exactly is `HLAE_ERROR` in Phase 1?**
   - What we know: D-08 lists it; D-... defer AFXERROR fail-fast to Phase 2 and say Phase 1 only keeps `AFXERROR` in the tail.
   - What's unclear: the trigger.
   - Recommendation: "deadline expired and the tail contains `AFXERROR`" -> code `HLAE_ERROR`, hint about `ffmpeg.ini`. Alternative: reserve the code for Phase 2. Planner decides.

3. **Is a fifth column `fail_elapsed_ms` acceptable?**
   - D-05 names four columns; D-06 wants "прошедшее время" on the card. Recommendation: add the fifth (and `phase`); both are additive.

4. **`Host::run` and `run_ffmpeg` in Phase 1?**
   - TMO-01 lists ffprobe/ffmpeg deadlines; TMO-03 (no unbounded `Command::output`) is Phase 2.
   - Recommendation: Plan 6 (optional, last wave) moves the already-correct `run_ffmpeg` core into `host::runner::StdRunner` and gives it the `ffmpeg` deadline; if time is short, add the deadline inside `run_ffmpeg` (a few lines) and keep `Host::run` as a declared-but-unused API only if the planner wants TST-01 literal compliance.

5. **Where do `install_files` and the `record.log` header get the demo/cs2 paths?** Resolved in the design (stay in `app`, `RecordRequest` carries `cs2_exe`, `out_dir`), listed for the planner only.

## Environment Availability

Probed on the author's machine (2026-10-10).

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | build/tests | ✓ | rustc 1.99.0, cargo 1.99.0 | - |
| `windows-sys` source (offline) | `host` crate | ✓ | 0.61.2 in `~/.cargo/registry` | - |
| HLAE | spikes, `WinHost` | ✓ | `C:\Program Files (x86)\HLAE\HLAE.exe`, `x64\AfxHookSource2.dll` present; `ffmpeg\ffmpeg.ini` `Path=C:\Program Files (x86)\HLAE FFMPEG\ffmpeg\bin\ffmpeg.exe` | - |
| CS2 | spikes | ✓ | `...\Counter-Strike Global Offensive\game\bin\win64\cs2.exe`; `steam.inf` `PatchVersion=1.41.9.0` | - |
| Steam | spikes | ✓ running | `steam.exe` present in `tasklist` | - |
| Foreign `cs2.exe` | spikes | ⚠ **running now (PID 48660)** | - | Close it before S1-S4 (Phase 2 PRE-05 will refuse to start in this state) |
| ffmpeg / ffprobe on PATH | assembly | ✓ | 9.0.2 (gyan.dev full build, WinGet) | - |
| `node` on PATH | GSD tooling only | ✗ (found at `C:\Program Files\nodejs\node.exe`) | - | `export PATH="$PATH:/c/Program Files/nodejs"` for `gsd-tools` |
| Free disk | takes/spike transcripts | ✓ but tight | `C:` 22 GB free of 465 GB (six leaked demo copies in `game\csgo`, ~1.9 GB) | Out dir is under `%LOCALAPPDATA%\cs2cinema\out` on `C:`; Phase 2 PRE-06 |
| Linux CI | stub build | CI only | `ubuntu-latest` | - |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** `node` PATH only affects GSD helper commands.

## Security Domain

`security_enforcement` is enabled (`.planning/config.json`: `"security_enforcement": true`, ASVS level 1, block on high).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | local single-user app; netcon has no auth (see threat below) |
| V3 Session Management | no | - |
| V4 Access Control | partial | only processes in our Job Object are signalled; never by image name |
| V5 Input Validation | yes | `renderer::console_safe`, `pipeline::console_name` for demo/out names; nonce hex-only; `Session::send` rejects `\r`/`\n`; store uses `rusqlite::params!` (existing pattern) |
| V6 Cryptography | no | nonce is not a secret |
| V12 Files and Resources | yes | `record.log` and cfg files written only under `out_dir` / `game/csgo/cfg` built from sanitised names; no path taken from console output |
| V14 Configuration | yes | `-insecure` is mandatory for the hook; keep the launch line in one place with a comment; app only plays demos |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Command injection into the game console through demo/out-dir names | Tampering | sanitise (`console_name`, `console_safe`); `send()` rejects control characters; commands are fixed templates plus nonce |
| Unauthenticated netcon reachable beyond loopback | Spoofing / Elevation | S2 records the bind address with `netstat`; per-launch random port; if it binds `0.0.0.0`, document and prefer closing the window of exposure (short-lived session) [A12] |
| Stale / forged marker lines (old `console.log`, echo of our own command) | Spoofing | nonce + whole-line anchored parser + `console.log` read from the launch-time offset |
| Killing a foreign process | Denial of Service | Job Object handles only; no `taskkill /IM` (Phase 2 adds the refusal check) |
| `unsafe` FFI misuse (invalid handles, leaks) | Tampering / DoS | single `win/` module, `// SAFETY:` comments, RAII `Drop` for handles and the job |
| VAC risk of `-insecure` sessions | Info | launch line only plays demos; README note is Phase 2 documentation (PITFALLS #7) |

## Suggested Plan Decomposition (for the planner; waves respect dependencies)

1. **Plan 1 (Wave 1): `crates/host` skeleton + spike harness.** Traits, `HostError`, `UnsupportedHost`, `win/job.rs` + `win/process.rs`, `examples/spikes.rs`, workspace member, CI-green on Linux. Human checkpoint at the end: author runs S1-S4 and writes `01-SPIKES.md` (autonomous: false for the checkpoint task).
2. **Plan 2 (Wave 1, parallel): `host::fake` + virtual clock + `Scenario`.** Includes self-tests of the fake.
3. **Plan 3 (Wave 1, parallel): renderer markers + `renderer::takes()` + nonce in `Config`, test updates, `Plan{takes, nonce}` in `plan_player`, `plan_hash` nonce normalisation.**
4. **Plan 4 (Wave 2, needs 2 and 3): `pipeline::record`** — `markers.rs`, `deadlines.rs`, `failure.rs`, `log.rs` (`RecordLog`, `Tail`), `machine.rs`, driver, `Progress::event`/`Event`; all tests on the fake (success criterion 5).
5. **Plan 5 (Wave 2, parallel to 4): `store` migration + `set_phase/set_out_dir/fail_job`, tests.**
6. **Plan 6 (Wave 3, needs `01-SPIKES.md`): `WinHost`** — job, watcher thread, netcon thread, `console.log` tail, HLAE pipes, adopt fallback if S1 said so; windows-only smoke test with a tiny fake game if time allows.
7. **Plan 7 (Wave 4, needs 4, 5, 6): app cutover** — `jobs.rs` (`RunError`, `record` call, remove `launch::kill_game` use, `set_out_dir`, deadline on assembly), `JobReporter::event`, settings multiplier, `jobs_page` error block + ladder. Delete `jobs::record` and the dead `launch.rs` items; keep discovery. Manual verification: one real FACEIT job to `highlights.mp4`, plus one induced failure (wrong HLAE folder / broken `ffmpeg.ini`) showing the card and `record.log`.
8. **Optional Plan 8: `Host::run` / `StdRunner`** for `run_ffmpeg` (Open Question 4).

UI hint is **yes** for this phase: run `/gsd-ui-phase` (or fold the card/ladder layout into Plan 7) before implementing the card.

## Sources

### Primary (HIGH confidence)
- Repository files read this session: `crates/app/src/{launch.rs,jobs.rs,jobs_page.rs,status.rs,settings.rs,settings_page.rs,app.rs,mock.rs}`, `crates/pipeline/src/lib.rs`, `crates/store/src/lib.rs`, `crates/renderer/src/lib.rs`, `crates/director/src/lib.rs` (Shot/Timeline), `crates/cli/tests/pipeline.rs`, `Cargo.toml`, `Cargo.lock`, `.github/workflows/ci.yml`, `.planning/{PROJECT-level research,codebase maps,CONTEXT,REQUIREMENTS,ROADMAP,STATE,config.json}`
- `C:\Program Files (x86)\Steam\...\game\csgo\console.log` and `steam.inf` (local evidence cited by line)
- windows-sys 0.61.2 source in the local cargo registry (function signatures, struct layouts, constants, feature gates)
- Executed scratch programs (Job Object + liveness; `ChildExt::main_thread_handle` instability)
- Microsoft Learn: Job Objects https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects ; WaitForSingleObject https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject
- HLAE source: https://raw.githubusercontent.com/advancedfx/advancedfx/main/hlae/Loader.cs , https://raw.githubusercontent.com/advancedfx/advancedfx/main/hlae/Program.cs , https://raw.githubusercontent.com/advancedfx/advancedfx/main/AfxHookSource2/main.cpp
- `gsd-tools query package-legitimacy check --ecosystem crates windows-sys` -> OK

### Secondary (MEDIUM confidence)
- `.planning/research/{SUMMARY,ARCHITECTURE,STACK,PITFALLS,FEATURES}.md` (project-level research, itself sourced from HLAE source, cs-demo-manager, Microsoft docs) — relied on for deadline budgets, `ALL_DONE`/state design, HLAE ffmpeg/`quit` behaviour

### Tertiary (LOW confidence)
- Anything tagged `[ASSUMED]` in the Assumptions Log (A1-A13)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - one dependency, already locked, compile-checked
- Architecture: HIGH for structure and seams (derived from the code), MEDIUM for HLAE-specific runtime behaviour (spikes)
- Pitfalls: MEDIUM-HIGH - code-derived ones verified; HLAE/CS2 runtime ones assumed

**Research date:** 2026-10-10
**Valid until:** 2026-11-10 (HLAE/CS2 update cadence is days; re-check `AfxHookSource2_changelog.xml` if CS2 patches before execution)
