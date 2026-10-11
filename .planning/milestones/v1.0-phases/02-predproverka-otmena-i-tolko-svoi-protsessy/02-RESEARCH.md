# Phase 2: Предпроверка, отмена и только свои процессы - Research

**Researched:** 2026-10-10
**Domain:** Windows process ownership (Job Object, handles, windows), environment pre-flight checks, cancellation/cleanup in a Rust `std::thread` pipeline, SQLite migration
**Confidence:** HIGH for the process/window/Job Object mechanics (compiled and run on the author's machine this session); MEDIUM for Steam `StateFlags` semantics and disk-size constants (one authoritative-ish doc, local samples); LOW only where marked `[ASSUMED]`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

Решения фазы 1 (D-01..D-12 в `01-CONTEXT.md`) остаются в силе: трейт `Host` в крейте `crates/host`, Job Object, типизированная `RecordFailure`, маркеры с nonce. Нумерация ниже своя для фазы 2.

#### Отчёт предпроверки
- **D-01:** Предпроверка идёт первым этапом задачи записи, до скачивания, парсинга и планирования, и показывается в карточке задачи. В настройках есть кнопка «Проверить окружение», которая запускает тот же отчёт вручную. Один код на оба входа. Задача «только план» предпроверку пропускает.
- **D-02:** Старт блокирует только `fail`. `warn` (сборка CS2 изменилась, мало места с запасом) попадает в журнал и в карточку, запись идёт. Режима «строго» нет: подтверждение по `warn` ломает работу очереди без присмотра (цель фазы 4).
- **D-03:** Свободное место: оценка по плану (сумма длительностей дублей × эмпирический размер сырого видео и звука) с запасом ×1.5. `fail`, если места меньше оценки; `warn`, если меньше оценки с запасом. Проверяются диск результатов и диск CS2 (копия демки, cfg). Константа размера лежит в одном месте и уточняется по первым реальным записям.
- **D-04:** Пара (сборка CS2, версия HLAE) последней удачной записи хранится в новой таблице SQLite — **Reversibility:** one-way — новая миграция `user_version` на машине пользователя, откат только миграцией вниз. Запись пары идёт после удачной задачи. Если сборка CS2 из `appmanifest_730.acf` изменилась с последней удачной записи, предпроверка даёт `warn` (DIF-01). Пока Steam обновляет CS2 (по `StateFlags`), предпроверка даёт `fail` (DIF-02).

#### Политика уборки (TMO-06)
- **D-05:** Копия демки в `game/csgo` удаляется при любом исходе задачи: успех, сбой, отмена. Оригинальная демка не трогается.
- **D-06:** Установленные `cs2c_*.cfg`, `campath_*.xml` и `highlights.cfg` в папке игры удаляются при любом исходе. Копии остаются в папке задачи для разбора.
- **D-07:** Частичный дубль (начат и оборван) не удаляется, а помечается: папка `takeNNNN` переименовывается в `takeNNNN.partial`. Имя не совпадает с `take\d+`, поэтому склейка и поиск готовых дублей его не берут. Повторная запись пишет чистый `takeNNNN`.
- **D-08:** После краха самого приложения уборка делается при следующем запуске приложения по записям задач в базе со статусом `running` или `cancelling`: удалить установленные файлы, пометить частичные дубли. Для этого список установленных файлов должен быть доступен после перезапуска (в базе или в папке задачи, форма на усмотрение планировщика).

#### Мгновенный провал (LCH-05)
- **D-09:** Любая первая строка `AFXERROR` в консоли сразу завершает задачу кодом `HLAE_ERROR`, в любой момент сессии от запуска до закрытия (включая запись дубля). Предохранитель от ложных тревог: список подстрок-исключений в `settings.json` (по умолчанию пуст), редактируется руками, без отдельного экрана.
- **D-10:** Модальное окно ошибки HLAE/AfxHookSource2 и заголовки окон игры ловятся двумя путями: снимок окон при истечении дедлайна фазы запуска и опрос раз в секунду, пока `HLAE.exe` жив. Опрашиваются верхние окна процессов из своего job. Текст окна и заголовки включаются в сообщение ошибки. Для этого в `Session` появляется способ получить список окон, у фейкового хоста есть сценарий с окном.

#### Отмена и очередь
- **D-11:** «Отменяется» — отдельный статус задачи в базе и в UI — **Reversibility:** costly — новый вариант `JobStatus` в строках задач, старые версии приложения его не поймут. Кнопка «Отмена» становится неактивной, статус виден в списке и в карточке, переживает перезапуск (по нему при старте добирается уборка из D-08). Цели: «отменяется» не позже чем через 3 с после нажатия, чистое состояние примерно за 10 с, в любом состоянии, включая ожидание игры, консоли, демки, дубля и ffmpeg.
- **D-12:** Повторное нажатие «Отмена» в статусе «Отменяется» немедленно завершает свой job без поэтапности и переходит к уборке. Это единственный выход, если игра зависла на выходе (в спайке один `quit` из 16 завис после `Source2Shutdown`).
- **D-13:** После отмены очередь ждёт, пока пользователь не нажмёт «Продолжить». После сбоя очередь продолжает работу сама. Сбой одной задачи не останавливает ночную очередь.
- **D-14:** Перед стартом следующей задачи ждём подтверждения выхода своего `cs2.exe` по handle и добавляем паузу 3 с. Если свой `cs2.exe` не ушёл за 30 с после завершения job, следующая задача падает кодом `PREV_GAME_STUCK` с подсказкой. Чужие процессы не трогаем ни при каких условиях (PRE-05, TMO-05).

### Claude's Discretion
- **Состав и имена проверок предпроверки.** Идентификаторы проверок, тексты сообщений и подсказок по-русски, формат отчёта в карточке и на странице настроек, коды `fail` для чужого `cs2.exe` (`FOREIGN_GAME_RUNNING` или аналог) и `PREV_GAME_STUCK`.
- **Определение, что Steam запущен.** По снимку процессов (PRE-04), точный способ выбирает исследование фазы.
- **Семантика `StateFlags` в `appmanifest_730.acf`** и источник номера сборки CS2: проверить на машине автора при исследовании.
- **Job Object + IOCP.** Нужна ли очередь завершения для событий «job пуст» вместо опроса, решает исследование (в фазе 1 есть работающий наблюдатель с опросом).
- **Дедлайны на оставшиеся внешние вызовы.** `ffmpeg -version`, `ffprobe`, проба NVENC (`launch::nvenc_check` сейчас вызывает `Command::output()` без дедлайна, замечание WR-03 код-ревью фазы 1) идут через `Host::run` с дедлайном и отменой.
- **Закрытие игры.** Поэтапно: `quit` → ожидание ~5 с → завершение своего job → проверка выхода → пауза. Учесть замечания WR-01 и WR-05 код-ревью фазы 1 (неудача отправки `quit`, ожидание `JobEmpty` вместо фиксированной паузы).

### Deferred Ideas (OUT OF SCOPE)
- Режим «строго» (блокировка по `warn`) — отвергнут для этой фазы, может вернуться как настройка позже.
- Снимок экрана модального окна HLAE в папку задачи — не нужен для текста ошибки, возможное улучшение диагностики.
- Ротация и ограничение размера `record.log` — фаза 3.
- Манифест и проверка дублей через `ffprobe` (TAK-02..05) — фаза 3.
- Кнопка «Повторить», автоповтор и приёмка на 5 матчах — фаза 4.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PRE-01 | Единый отчёт ok/warn/fail с сообщением и подсказкой | New `pipeline::precheck` (`Report`/`Item`/`Level`), pure `evaluate` over gathered facts; persisted as JSON on the job row; same code from the Settings button |
| PRE-02 | Предпроверка первой, до скачивания/парсинга/планирования; «только план» пропускает | Insert at the top of `app::jobs::run` (`jobs.rs:164`) guarded by `job.kind != JobKind::Plan`; space check is **two-pass** (see Pitfall 1) |
| PRE-03 | Инструменты: HLAE.exe, AfxHookSource2.dll, cs2.exe, ffmpeg/ffprobe/ffmpeg.ini `-version` в таймаут | File checks via `std::fs`; `-version` probes via `Host::run` with `CmdSpec.deadline`; `ffmpeg` major >= 7 (renderer emits `-/filter_complex`) |
| PRE-04 | Steam запущен, по снимку процессов | `host` process snapshot (ToolHelp, already in `host::win::process::snapshot_processes`), match `steam.exe` case-insensitively; verified process names on this machine |
| PRE-05 | Чужой `cs2.exe` — не стартовать, не убивать | Name-only match in the snapshot (path of a foreign cs2 is readable but must not be required); own stuck game tracked separately -> `PREV_GAME_STUCK` |
| PRE-06 | Свободное место: диск результатов и диск CS2, оценка по плану | `GetDiskFreeSpaceExW` (verified), nearest-existing-ancestor fallback, shared-volume summation, estimate constants from local measurements |
| PRE-07 | Демка существует и не LFS-указатель; выходной путь без пробелов, доступен для записи | Header sniff (`PBDEMS2\0`, zstd `28 B5 2F FD`, gzip `1F 8B`, LFS text `version https://git-lfs`); `renderer::console_safe` + real write probe |
| LCH-05 | Модальное окно HLAE и заголовки окон в ошибке; первая `AFXERROR` завершает сразу | `Session::windows()` via `EnumWindows` + `GetWindowThreadProcessId` filtered by job PIDs (verified on a stand-in MessageBox); `Machine` fails immediately on first non-ignored AFXERROR |
| TMO-03 | Отмена: «отменяется» <= 3 с, чисто <= ~10 с в любом состоянии; нет `Command::output()` без дедлайна | Cancel is already polled every 100 ms; add staged shutdown in the driver, `JobStatus::Cancelling`, remove the two `nvenc_check` `.output()` calls (remaining `.output()` sites listed in Pitfalls) |
| TMO-04 | Поэтапное закрытие игры | Driver-level shutdown: `quit` -> wait -> `Session::terminate` -> `Session::wait_exit` -> settle; success path keeps waiting for `JobEmpty` (WR-05) |
| TMO-05 | Только свои процессы: Job Object `KILL_ON_JOB_CLOSE`, запасной (PID, время создания), без `taskkill /IM` | Already implemented in Phase 1 (`Job`, `pick_adoptee`, `open_owned`); Phase 2 locks it with a source-grep guard test and adds own-stuck-game tracking |
| TMO-06 | Политика уборки | `installed.json` recovery record in the job folder; delete demo copy + cfg/xml after confirmed exit; rename unfinished `takeNNNN` -> `takeNNNN.partial` |
| TMO-07 | Следующая задача стартует после подтверждения выхода `cs2.exe` и паузы | Wait on the `cs2.exe` handle (`OwnedProc::wait`) at the end of `record()`, 3 s settle, process-wide stuck-game registry checked by the next precheck |
| DIF-01 | Предупреждение при смене сборки CS2 | New SQLite table `last_good`; build id from `appmanifest_730.acf` `"buildid"`, patch from `steam.inf` `PatchVersion` |
| DIF-02 | Не стартовать, пока Steam обновляет CS2 | `StateFlags` bitmask from `appmanifest_730.acf` (bit table cited); local value is `4` |
</phase_requirements>

## Summary

Phase 1 already delivered almost everything Phase 2 needs at the OS level: a Job Object with `KILL_ON_JOB_CLOSE` (`crates/host/src/win/job.rs`), handle-based liveness (`OwnedProc::wait`), a process snapshot (`snapshot_processes`), the PID-adoption fallback with a creation-time filter (`pick_adoptee`), and `Host::run` with a deadline. No `taskkill`/`tasklist` string exists anywhere in `crates/` (grep: empty). So TMO-05 is mostly *locking in* and *extending* (own-stuck-game tracking), not building. The real work in Phase 2 is: (1) a pure, fake-testable pre-flight report; (2) a *driver-level staged shutdown* with exit confirmation (the machine currently kills the job instantly on cancel and does not wait for the process to disappear); (3) window enumeration for LCH-05; (4) cleanup/recovery files; (5) one SQLite migration (step 3) plus a new `JobStatus`; (6) UI/queue plumbing (card report, "Отменяется", "Продолжить", settings button).

Three findings change the plan shape. **First**, PRE-06's estimate needs the plan, but D-01/PRE-02 put the pre-check *before* planning: the space check must be two-pass (a coarse floor before download, the plan-based estimate right after `plan_player`, same report). **Second**, the AfxHookSource2 `ErrorBox` is a modal `MessageBoxA` shown *inside the game process* (not in `HLAE.exe`), and HLAE.exe exits after ~3 s; so "poll while HLAE.exe is alive" (D-10) is too narrow — poll once per second in every state before `Closing`, over all PIDs in the job. **Third**, the literal "quit -> ~5 s -> terminate" (TMO-04) is right for cancel/failure but wrong for the *success* path, where killing the job early truncates the last take that HLAE's ffmpeg is still flushing (WR-01/WR-05); the success path must wait for `JobEmpty` (30 s window) and only then terminate.

IOCP is **not needed**: the sequence works in `windows-sys` 0.61.2 (verified, below) but the existing 250 ms poll of `active_count()` plus a wait on the game handle meets every timing target (3 s / 10 s / 30 s). Recommend not adding a completion-port thread.

**Primary recommendation:** Add `pipeline::precheck` (pure evaluation over a gathered `Facts` struct, two-pass space check), extend `Host`/`Session` with `processes()`, `free_space()`, `windows()`, `wait_exit()`, move shutdown/cleanup into a driver-level staged procedure in `pipeline::record` that ends only after the `cs2.exe` handle is confirmed signalled, and ship one store migration (`last_good` table + `precheck` column) with `JobStatus::Cancelling` as a string value. Keep polling, add no new crates.

## Project Constraints (from CLAUDE.md)

- Rust workspace, edition 2024 per crate, `std::thread` (no tokio), SQLite via `rusqlite`; do not change the stack, embed in existing crates. [VERIFIED: `.claude/CLAUDE.md` Constraints]
- Windows only for recording; non-Windows must still `cargo test --workspace` (stub `UnsupportedHost`). CLI `cs2-cli` must keep building and passing `crates/cli/tests/pipeline.rs`. [VERIFIED: CLAUDE.md]
- One recording job at a time; external tools have unknown console markers — every detector needs a timeout and diagnostics. [VERIFIED: CLAUDE.md]
- User-facing strings and doc comments in Russian; identifiers in English. Errors: `Result<T, String>` at the pipeline/store boundary; cancellation sentinel `pipeline::CANCELLED`. New long steps report via `Progress::stage/log` and call `check(p)?` at boundaries. [VERIFIED: CLAUDE.md]
- No `unwrap()`/`expect()` outside tests; unsafe blocks carry `// SAFETY:` comments (Phase 1 convention, see `host/src/win/*`). [VERIFIED: 01-REVIEW.md "Unsafe в host/src/win"]
- Tests are named as sentences; scenario tests run on `FakeHost` with virtual time. Run `cargo clippy --workspace` locally; format app files only with rustfmt `max_width=160,use_small_heuristics=Max` (STATE.md 01-07). [CITED: .planning/STATE.md]
- GSD workflow: all repo edits go through a GSD command (not relevant to research, relevant to execution).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Process snapshot, window enumeration, disk free space, exit-by-handle wait | `host` crate (`win/`, behind `Host`/`Session`) | `fake.rs`, `unsupported.rs` | Only `host` calls Win32 (crate doc rule); everything else stays testable on fakes |
| Pre-check evaluation (levels, messages, hints) | `pipeline::precheck` (pure) | `app` (collects fs facts, renders) | Pure `Facts -> Report` runs on ubuntu CI; `app` only gathers settings paths and draws the card |
| Machine decisions (AFXERROR immediate fail, modal dialog fail, deadline messages) | `pipeline::record::machine` | — | The machine is the pure "what next" layer; drivers feed it `Input::Windows` |
| Staged shutdown, exit confirmation, partial-take marking | `pipeline::record` driver (`record()`) | `host::Session::{terminate,wait_exit}` | Waiting needs virtual time on the fake and real handles on Windows; the machine stays I/O-free |
| Installed-files recovery record, cleanup of demo/cfg copies | `app::launch`/new `cleanup` module (next to `install_files`) | `pipeline` (if shared with CLI later) | `install_files` lives in `app/src/launch.rs:96-110`; CONTEXT places cleanup "in `jobs.rs` where `install_files` is" |
| Job status `cancelling`, `last_good` pair, pre-check JSON | `store` (migration step 3) | `app` | One-way schema change; store has no workspace deps |
| Cancel/Continue UX, card report, settings button | `app` (`jobs_page`, `settings_page`, `app.rs`) | — | egui only |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `windows-sys` | 0.61.2 (locked) | Win32 FFI in `host/src/win/` | Already the project's FFI layer; `Cargo.lock` pins `0.61.2` [VERIFIED: Cargo.lock `name = "windows-sys" version = "0.61.2"`] |
| `rusqlite` | 0.32.1 (bundled) | Migration step 3, `last_good`, `precheck` column | Existing store; `MIGRATIONS` array + `PRAGMA user_version` [VERIFIED: crates/store/src/lib.rs:224-257] |
| `serde` / `serde_json` | existing | Report JSON on the job row, `installed.json`, `settings.json` field | Already used for every persisted structure |

**No new crates are recommended.** Required `windows-sys` feature additions in `crates/host/Cargo.toml` (currently `Win32_Foundation`, `Win32_Security`, `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Diagnostics_ToolHelp`): add `Win32_UI_WindowsAndMessaging` (EnumWindows, GetWindowTextW, GetClassNameW, IsWindowVisible, GetWindowThreadProcessId, EnumChildWindows), `Win32_Storage_FileSystem` (`GetDiskFreeSpaceExW`; also `GetFileVersionInfoW` for the HLAE.exe file version), and only if IOCP were adopted: `Win32_System_IO` + `Win32_System_SystemServices`. All function names verified present in `windows-sys-0.61.2/src/Windows/Win32/...` this session and the combination compiled and ran in a scratch crate. [VERIFIED: scratch crate build + run on this machine]

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `std::fs`, `std::process`, `std::net` | std | File checks, write probe, ACF line scan | Everything that is not a Win32 call |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| 250 ms poll of `Job::active_count()` + handle wait | Job Object IOCP thread (`JOBOBJECT_ASSOCIATE_COMPLETION_PORT`) | Verified to work (below) but adds a blocking thread, a shutdown `PostQueuedCompletionStatus`, and `unsafe` for no timing gain: every target is >= 3 s. Skip; keep as a documented fallback |
| Hand-written `appmanifest` scan | A VDF parser crate | Needs only 4 top-level keys; a crate is a supply-chain addition for ~15 lines |
| `GetFileVersionInfoW` for HLAE version | Parse `AfxHookSource2_changelog.xml` first `<version>` | Changelog gives hook version `0.41.7` [VERIFIED: file read], file-version gives `2.192.7.0` [VERIFIED: PowerShell `(Get-Item ...).VersionInfo.FileVersion`]. Record both; the pair key can be (buildid, HLAE file version) |

**Installation:** nothing to `cargo add`; edit `crates/host/Cargo.toml` features.

**Version verification:** `windows-sys` 0.61.2 is already in `Cargo.lock`; registry source for 0.61.2 was inspected locally. `cargo 1.99.0 / rustc 1.99.0` on this machine. [VERIFIED: `cargo --version`]

## Package Legitimacy Audit

No new external packages are introduced. The only package touched (feature flags only) was checked:

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| windows-sys | crates.io | first published 2021-10-18 | ~35.1M/wk | github.com/microsoft/windows-rs | OK | Approved (already a dependency; add features only) |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none
[VERIFIED: `gsd-tools query package-legitimacy check --ecosystem crates windows-sys` -> `"verdict": "OK"`]

## Architecture Patterns

### System Architecture Diagram

```
  UI click "Собрать ролик"                       Settings: "Проверить окружение"
          │                                                   │
          ▼                                                   ▼
  app::jobs::run_next ──► run(job) ─────────────────► precheck::run(host, Inputs)  (manual: same fn)
          │                  │                                   │
          │        kind==Plan? skip                              ▼
          │                  │                    gather Facts: fs (hlae/cs2/ini/acf/steam.inf),
          │                  │                    Host::run -version x3 (deadline+cancel),
          │                  │                    Host::processes() (steam.exe? cs2.exe? stuck own?),
          │                  │                    Host::free_space(out_root/cs2_dir) [floor estimate],
          │                  │                    demo header sniff / out path write probe
          │                  │                                   │
          │                  │                    evaluate(Facts) -> Report{items ok/warn/fail}
          │                  │                                   │ fail? ──► store.fail_job(PRECHECK_FAILED / FOREIGN_GAME_RUNNING / PREV_GAME_STUCK) ─► JobDone
          │                  ▼                                   │ warn/ok ► journal + store.set_precheck(json)
          │          analyze → plan_player (Download/Parse/Positions/Geometry/Plan)
          │                  │
          │                  ▼   pass 2: precheck::check_space(plan.takes, demo_bytes)  (fail ► same path)
          │          install_files + write installed.json (recovery record)
          │                  │
          │                  ▼
          │          pipeline::record(host, req, p) ──────────────────────────────────────────────┐
          │              loop (100 ms): poll event ─► Machine::step ─► actions                    │
          │                 every 1 s: session.windows() ─► Input::Windows(list) ─► Machine       │
          │                 cancel flag ─► Input::Cancel     force flag ─► Input::ForceCancel     │
          │              Finish(Ok|Err) ─► SHUTDOWN (driver):                                     │
          │                 Graceful: send quit ─ wait ≤5 s (failure/cancel) / ≤30 s JobEmpty (ok)│
          │                 → Session::terminate → Session::wait_exit(≤N s) → settle 3 s          │
          │                 → mark unfinished takeNNNN ► takeNNNN.partial                          │
          │                 → if cs2 still alive after 30 s: register in stuck-game registry       │
          │                                                                                        ▼
          │          cleanup: delete demo copy + cfg/xml (from installed.json), any outcome ◄── RecordReport / RecordFailure
          │                  │
          │                  ▼
          │          Assemble via Host::run(ffmpeg, deadline, cancel)  (unchanged from Phase 1)
          ▼
   finish(): Done → store.set_last_good(cs2_build, hlae_ver) ; Failed → fail_job ; Cancelled → cancel_job + queue paused
```

### Recommended Project Structure
```
crates/host/src/
├── lib.rs            # + ProcInfo, WindowInfo; Host::{processes,free_space,stuck_games}; Session::{windows,wait_exit}
├── fake.rs           # + Scenario: windows_at(), free_space(), processes(), exit_after_terminate()/stuck
├── unsupported.rs    # stubs return Unsupported / empty
└── win/
    ├── windows.rs    # NEW: EnumWindows over job PIDs (unsafe, // SAFETY:)
    ├── disk.rs       # NEW (or in process.rs): GetDiskFreeSpaceExW
    └── mod.rs        # WinSession::{windows,wait_exit}; static STUCK_GAMES registry
crates/pipeline/src/
├── precheck/         # NEW: mod.rs (Report/Item/Level/Facts/evaluate), steam.rs (acf + steam.inf parse), space.rs (estimate)
└── record/
    ├── machine.rs    # + Input::{Windows,ForceCancel}; AFXERROR immediate; dialog fail; WR-01/WR-05 fixes
    ├── mod.rs        # driver: windows poll, shutdown(), partial-take marking, exit confirmation
    ├── deadlines.rs  # + tool_probe, nvenc_probe, quit_wait, kill_confirm, exit_confirm, window_poll
    └── failure.rs    # + FailCode::{PrecheckFailed,ForeignGameRunning,PrevGameStuck}; messages/hints
crates/store/src/lib.rs  # MIGRATIONS[2]: last_good table + jobs.precheck column; JobStatus::Cancelling; recovery queries
crates/app/src/
├── jobs.rs           # precheck first; two-pass space; cleanup guard; Cancelling flow; queue pause
├── launch.rs         # nvenc_check -> Host::run; install_files writes installed.json; cleanup fns
├── jobs_page.rs      # Cancelling badge, disabled button, "Продолжить" banner, precheck block in card
├── settings.rs       # + afxerror_ignore: Vec<String> (serde default)
└── settings_page.rs  # + "Проверить окружение" + report view
```

### Pattern 1: Pure evaluation over gathered facts
**What:** `precheck::run` = `gather(host, inputs)` (side effects, bounded by deadlines and the cancel closure) then `evaluate(&Facts) -> Report` (pure). All Russian text lives in one catalog like `record/failure.rs`.
**When to use:** every PRE-xx check; makes each branch a one-line test on ubuntu.
**Example:**
```rust
// Source: pattern of crates/pipeline/src/record/failure.rs (message catalog) + this phase's design
pub enum Level { Ok, Warn, Fail }
pub struct Item { pub id: &'static str, pub level: Level, pub message: String, pub hint: String }
pub struct Report { pub items: Vec<Item> }
impl Report {
    pub fn worst(&self) -> Level { /* max by Fail > Warn > Ok */ }
    pub fn first_fail(&self) -> Option<&Item> { self.items.iter().find(|i| i.level == Level::Fail) }
}
```

### Pattern 2: Driver-level staged shutdown (not in the machine)
**What:** After `Machine` emits `Finish(..)`, the driver runs `shutdown(mode)`: Graceful = `send("quit")` if console is up, keep calling `session.poll(TICK)` (so late console lines still reach `record.log` and the failure tail) until `GameExited`/`JobEmpty` or the window closes; then `session.terminate()`; then `session.wait_exit(kill_confirm)`; then settle. Immediate (force-cancel, `HLAE_ERROR` from a modal dialog) skips straight to `terminate()`.
**Why driver, not machine:** the machine is I/O-free; virtual-time `FakeSession::poll` already advances the clock, so the whole procedure is testable in microseconds. Build `RecordFailure` *after* shutdown so the tail includes lines that arrived during it (a 1272-line AFXERROR storm lands in 5 s — 01-SPIKES).
**Budget (cancel):** quit 5 s + kill confirm <= 3 s + file cleanup <1 s ≈ 8-9 s < the ~10 s target. "Отменяется" status is set synchronously by the click handler, so the 3 s target is met trivially.

### Pattern 3: Two-pass pre-check (same report)
**What:** Pass 1 runs before download/parse/plan with everything that needs no plan, and a *floor* space estimate for both disks. Pass 2 (`check_space`) runs right after `plan_player` (and before `install_files`) with `Σ game_secs × MB/s` and the real decompressed demo size (`Analysis.data.len()` is available in `pipeline::Analysis`). Both write into the same stored report (replace the `space_*` items).
**Why:** PRE-06 literally requires "оценка объёма по плану" while PRE-02/D-01 require the check *before planning*. Doing only pass 1 gives a meaningless estimate; doing only pass 2 lets a missing HLAE.exe cost a multi-minute download/parse/plan first (today `launch::check_paths` runs after planning at `jobs.rs:210`).

### Pattern 4: Recovery record beside the job's files
**What:** `install_files` writes `<out_dir>/installed.json` = `{"cs2_dir": ..., "files": [abs paths of copied cfg/xml + demo copy], "done_takes": []}`; the `Progress::event` handler appends each `TakeDone` index (so a crash leaves enough to know which takes finished). Startup recovery (before `run_next`, next to `reset_running` at `app.rs:158`): for each job in `running`/`cancelling` read `out_dir/installed.json`, delete listed files (only paths that resolve under `<cs2_dir>/game/csgo`), rename every `clips/takeNNNN` not in `done_takes` to `takeNNNN.partial`.
**Why a file, not a DB column:** survives the DB being opened in-memory fallback (`app.rs:150-153`), is inspectable, and needs no extra migration column.

### Anti-Patterns to Avoid
- **Killing/matching by image name** (`taskkill /IM`, `tasklist`): none exist now (grep empty); add a guard test that scans `crates/**/*.rs` (excluding the test itself) for `taskkill`/`tasklist`. PRE-05 may match `cs2.exe` by name *only to detect*, never to terminate.
- **Holding `Shared.game`'s mutex while waiting:** `WinSession::game_liveness` locks it; the watcher thread locks it every 250 ms (`watch.rs find_game/game_state`). A long `WaitForSingleObject` under the lock stalls the watcher. Wait in slices of <=100 ms, re-locking each slice.
- **Calling `SendMessageTimeoutW`/`WM_GETTEXT` on foreign windows:** blocks the driver loop up to the timeout per child and hurts cancel latency. `GetWindowTextW` returned the dialog text of a child `Static` in another process in the spike (non-blocking, reads the cached caption).
- **Deleting the demo copy while `cs2.exe` is alive:** the engine holds the `.dem`; deletion fails with a sharing violation. Clean up only after `wait_exit` confirmed.
- **Scaling cancel budgets with "Запас по времени":** `Deadlines::scaled` multiplies every field (`deadlines.rs`); keep `quit_wait`/`kill_confirm` outside the scaled set, or the 10 s cancel target breaks at ×5.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Own-process ownership | Name-based kill, PID files | `host::win::Job` (+ `pick_adoptee`/`open_owned` fallback) | Already done; name-based kills hit the user's own `cs2.exe` |
| Exit confirmation | `tasklist` loop / sleeps | `OwnedProc::wait(timeout)` on the game handle (`WaitForSingleObject`) | Handle wait is exact; error -> `Unknown`, never `Alive` (TMO-02) |
| Free disk space | `dir`/`fsutil`/`wmic` output parsing | `GetDiskFreeSpaceExW` `lpFreeBytesAvailableToCaller` | Verified: 140.6 GB on C:; fails (0) on a missing path -> walk to nearest existing ancestor |
| Process list | `tasklist` text | `snapshot_processes()` (ToolHelp) | Exists, tested (`snapshot_finds_the_current_process`) |
| Utility with a deadline | `Command::output()` | `Host::run(&CmdSpec{deadline}, cancel, on_line)` | Kills the child on deadline/cancel, drains pipes <= 2 s (`runner.rs`) |
| Window text from the dialog | OCR / screenshots | `EnumWindows` + `GetWindowTextW` on the dialog and its `Static` children | Verified below; screenshot is explicitly deferred |
| Time in tests | `sleep` | `FakeHost` virtual clock (only `poll` advances it) | Existing; cancel/shutdown scenarios stay microsecond-fast |
| Steam state parse | Full VDF parser/crate | 15-line scan of top-level `"key"\t\t"value"` lines | Only `StateFlags`, `buildid`, `TargetBuildID`, `BytesToDownload/BytesDownloaded` needed; `find_cs2` already scans `libraryfolders.vdf` the same way (`launch.rs`) |

**Key insight:** every dangerous primitive (kill, wait, enumerate) is already behind `Job`/`OwnedProc`; the risk in this phase is *orchestration* (ordering, who waits for whom, what survives a crash), not new OS calls.

## Runtime State Inventory

Not a rename/refactor phase, but the phase changes persisted state and installs files outside the repo, so the equivalent audit:

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | SQLite `cs2cinema.db`, `PRAGMA user_version` currently **2** (`MIGRATIONS` has two entries). `JobStatus::parse` maps unknown strings to `Failed` (`_ => JobStatus::Failed`) so an older build shows `cancelling` as "ошибка" | Add `MIGRATIONS[2]` (new table `last_good`, `ALTER TABLE jobs ADD COLUMN precheck TEXT`); `cancelling` needs no DDL (status is TEXT). **One-way**; add a migration test from a v2 DB like the existing v0/v1 tests |
| Live service config | `settings.json` at `%LOCALAPPDATA%\cs2cinema\settings.json` has legacy `load_wait_secs`; struct is `#[serde(default)]` | Add `afxerror_ignore: Vec<String>`; existing files keep loading (extend `old_settings_file_with_load_wait_still_loads`) |
| OS-registered state | None: no tasks/services registered. App crash closes the Job handle -> `KILL_ON_JOB_CLOSE` kills the game (handle is not inheritable: `CreateJobObjectW(null, null)`), so orphan games exist only in the `adopted_without_job` path | None for Phase 2; RET-04 (Phase 3) remains for the adopted case |
| Secrets/env vars | `faceit_key` lives in `settings.json`; nothing in the pre-check report may echo it | Keep keys out of report text/log |
| Build artifacts / installed files | **Game folder pollution now:** six `cs2cinema_*.dem` copies (190-433 MB each) plus `cfg/cs2c_*.cfg`, `cs2c_*_K.xml` left in `game/csgo` from earlier runs [VERIFIED: `ls`/`stat` of `Counter-Strike Global Offensive/game/csgo`] | Phase 2 stops creating new leftovers; optionally a one-time sweep of legacy `cs2cinema_*.dem` is a discretion item (not required by D-05/D-06) |

## Common Pitfalls

### Pitfall 1: The space estimate needs the plan, the pre-check runs before the plan
**What goes wrong:** Implementing PRE-06 strictly as "before download" yields a made-up number; implementing it after planning violates PRE-02's "первой".
**How to avoid:** two-pass design (Pattern 3). Pass 1 floors (single-place constants): results disk >= 3 GiB, CS2 disk >= 1 GiB (largest local clips folder 1.80 GB, demo copy <= 433 MB — measured below). Pass 2 uses the plan. Flag to the user in the plan (it is a reading of D-01/D-03, see Open Questions Q1).
**Warning signs:** a test that asserts the check uses `plan.takes` but is invoked before `plan_player`.

### Pitfall 2: Own stuck game vs foreign game
**What goes wrong:** After a hung `quit`, the previous job's `cs2.exe` is still in the snapshot. A name-only check reports `FOREIGN_GAME_RUNNING` ("убейте вручную чужой процесс") for a game the app itself owns, or the app terminates it by name.
**How to avoid:** keep a process-wide registry (static in `host::win`, Mutex<Vec<{pid, creation_time, OwnedProc}>>) of games that survived `wait_exit(30 s)`. `Host::stuck_games()` returns the still-alive ones (re-check by handle). Precheck order: stuck-own -> `PREV_GAME_STUCK`; other `cs2.exe` names -> `FOREIGN_GAME_RUNNING`. The registry must be process-wide because `jobs.rs:216` currently calls `default_host()` per task.
**Warning signs:** two consecutive cancels with a hung game produce "foreign" instead of "stuck".

### Pitfall 3: `quit` + 3 s on the success path truncates the last take (WR-01/WR-05)
**What goes wrong:** `machine.rs:343-348` (`on_send_failed` for `quit` -> `Terminate` + `finish_ok`) and `check_deadline` (`GameExited` + `close_settle` -> `finish_ok`, then `Guard::drop` -> `TerminateJobObject`) can kill HLAE's ffmpeg mid-flush; the take is later "assembled" with a broken `video.mp4`.
**How to avoid:** on the success path: failed `quit` send -> stay in `Closing`, resend on next `ConsoleUp`; `GameExited` -> wait for `JobEmpty` up to `game_close` (30 s) before declaring success; only if the host never reports `JobEmpty` (adopted-without-job) fall back to game-exit + `close_settle`. The 5 s `quit_wait` applies to cancel/failure shutdown only (nothing valuable to flush there). Document the deviation from the literal TMO-04 text (Open Question Q2).

### Pitfall 4: Cancel is "instant" in the machine but nothing confirms the process is gone
**What goes wrong:** `Input::Cancel` -> `Action::Terminate` + `Finish(Cancelled)` (`machine.rs step`), then `WinSession::terminate` returns immediately; the next task or `delete demo` can race a still-dying `cs2.exe` (GPU teardown can take seconds).
**How to avoid:** `Session::wait_exit(timeout)` (Windows: slices of `OwnedProc::wait`, plus `Job::active_count()==0`); fake: `Scenario::exit_after_terminate(Option<Duration>)` where `None` means "never exits" for the `PREV_GAME_STUCK` test.

### Pitfall 5: `.partial` rename and file deletion fail on locked files
**What goes wrong:** `fs::rename(clips/take0007, take0007.partial)` fails while HLAE's ffmpeg (child in the job) or an indexer still holds `video.mp4`. Deleting `cs2cinema_*.dem` fails while `cs2.exe` lives.
**How to avoid:** do both strictly after `wait_exit`+`active_count()==0`; retry 3 × 500 ms; on persistent failure keep the entries in `installed.json` and log "уборка отложена" (next start retries). Never block the job's result on cleanup success.

### Pitfall 6: Windows enumeration noise
**What goes wrong:** the job's PIDs own many invisible helper windows (`IME`, `MSCTFIME UI`, `WSH-Timer` in the stand-in run). Treating any window as an error is wrong.
**How to avoid:** modal candidate = visible, class `#32770`, top-level, `pid ∈ Job::pids()`. Titles for the deadline snapshot = visible top-level windows of job PIDs with non-empty title. Record the full window list as `Note` lines in `record.log` during the first real runs to learn what the real HLAE/CS2 normal state looks like (cannot be learned without launching the game; a foreign `cs2.exe` is running on the author's machine right now — PID 16528, observed — so no real launch was attempted).

### Pitfall 7: Poll window too narrow (D-10 says "while HLAE.exe is alive")
**What goes wrong:** `AfxHookSource2` shows its `ErrorBox` from the game process during DLL load; `HLAE.exe` exits with code 0 after 2.4-2.9 s (01-SPIKES S1) while the game is blocked on the dialog and never opens netcon, so the failure would only surface as `CONSOLE_NOT_OPEN` after 90 s.
**How to avoid:** poll windows once per second in every non-terminal state (Launcher..Take), over all job PIDs, not only the launcher's.

### Pitfall 8: AFXERROR immediate-fail loses context
**What goes wrong:** failing on the very first `AFXERROR` line builds the tail before the storm of repeated lines; the existing summary logic ("самая частая повторяющаяся AFXERROR", plan 01-08) has nothing to count.
**How to avoid:** decide immediately (code `HLAE_ERROR`), but assemble `console_tail` after `shutdown()` finished (Pattern 2). Shutdown for HLAE_ERROR can be Immediate (a broken game will not process `quit`), but still drains `poll` briefly (<=1 s virtual) to keep the storm shape.

### Pitfall 9: Remaining `Command::output()`/blocking calls
Inventory (grep this session, non-test):
- `crates/app/src/launch.rs:138` `nvenc_check` — on the recording path, called twice per record job (`jobs.rs:184,186`). **Must change** (WR-03).
- `crates/app/src/app.rs:567` `thumbnail()` — UI thumbnail on the Jobs page, not the recording path; recommended to route through `Host::run` too (cheap), else leave and note.
- `crates/geometry/src/lib.rs:293` `git ls-remote` inside `load_map` (planning stage when maps are stale and `walls` on) — before recording but on the task path; not interruptible by cancel. Recommend: accept or add a bounded helper; decide in planning (Q4).
- `crates/renderer/src/assemble.rs:121,207,299,313` — CLI-only (`crates/cli/src/main.rs:424`), out of scope until Phase 4.
- `ureq` FACEIT download — explicitly accepted limitation (REQUIREMENTS "Out of Scope": cancel works at the next stage boundary).

### Pitfall 10: Cancel while not in `record()`
`analyze` (download, parse) and `plan_player` only check cancel at stage boundaries (`pipeline::check`). The "<= 10 s clean" target is only guaranteed once recording/ffmpeg/probe stages run; for download/parse the status flips to `Отменяется` immediately and completes at the next boundary. State this limit in the plan so the verifier does not expect more.

### Pitfall 11: Partial `highlights.mp4` after cancelling ffmpeg
`run_bounded` kills ffmpeg on cancel, leaving a truncated `highlights.mp4` in the job folder (TAK-06 temp-name rename is Phase 3). Phase 2 may delete it on cancel/failure; at minimum it must not be mistaken for a result (the card only shows `result` for `Done`).

## Code Examples

### Job Object + IOCP sequence (verified; **not recommended for adoption**)
```rust
// Source: scratch crate in session scratchpad, windows-sys =0.61.2, run on this machine (Windows 11)
// features: Win32_Foundation, Win32_System_JobObjects, Win32_System_IO, Win32_System_SystemServices
let job = CreateJobObjectW(null(), null());                 // + KILL_ON_JOB_CLOSE via SetInformationJobObject
let port = CreateIoCompletionPort(INVALID_HANDLE_VALUE, null_mut(), 0, 1);
let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT { CompletionKey: 0x1234 as *mut _, CompletionPort: port };
SetInformationJobObject(job, JobObjectAssociateCompletionPortInformation, (&raw const assoc).cast(),
                        size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32);
AssignProcessToJobObject(job, child_handle);
GetQueuedCompletionStatus(port, &mut msg, &mut key, &mut ov, 200 /*ms*/);  // ov carries the PID for NEW/EXIT
// Observed output (verbatim):
// +0.00s msg=6(NEW_PROCESS) key=0x1234 pid=28108
// +0.02s msg=6(NEW_PROCESS) key=0x1234 pid=51504
// +2.06s msg=7(EXIT_PROCESS) key=0x1234 pid=51504
// +2.07s msg=7(EXIT_PROCESS) key=0x1234 pid=28108
// +2.07s msg=4(ACTIVE_PROCESS_ZERO) key=0x1234 pid=0
```
`JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO: u32 = 4u32` and `JOB_OBJECT_MSG_EXIT_PROCESS: u32 = 7u32` live in `Win32_System_SystemServices` [VERIFIED: windows-sys-0.61.2 `System/SystemServices/mod.rs:2434,2437`]. Conclusion: works, adds no capability the existing 250 ms `active_count()` poll lacks.

### Enumerate windows of the job's processes (verified on a stand-in MessageBox)
```rust
// Source: scratch crate; stand-in = wscript MsgBox(16) inside a Job; features Win32_UI_WindowsAndMessaging
unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    unsafe { (*(lparam as *mut Vec<HWND>)).push(hwnd) }; 1
}
let mut all: Vec<HWND> = vec![];
EnumWindows(Some(collect), (&raw mut all) as LPARAM);
for h in all {
    let mut pid = 0u32; GetWindowThreadProcessId(h, &mut pid);
    if !job_pids.contains(&pid) { continue; }
    // IsWindowVisible(h), GetClassNameW(h,..), GetWindowTextW(h,..); EnumChildWindows(h, ..) for Static text
}
// Observed (verbatim):
// HWND 0x9171254 pid=32208 visible=true class="#32770" title="AfxHookSource2 - Error" owner=0x0
//    child class="Button" GetWindowTextW="ОК"
//    child class="Static" GetWindowTextW="AFXERROR: ffmpeg path invalid. Тест кириллицы"
// HWND 0x6821398 pid=32208 visible=false class="WSH-Timer" ...   (plus invisible "IME"/"MSCTFIME UI" helpers)
```
Message text with Cyrillic round-trips. Use `GetWindowTextW` only.

### Free disk space (verified)
```rust
// Source: scratch crate; features Win32_Storage_FileSystem
GetDiskFreeSpaceExW(wide_path.as_ptr(), &mut avail_to_caller, &mut total, &mut total_free);
// C:/ -> ok=1 avail=140.6 GB total=499.2 GB ; ".../Counter-Strike Global Offensive" -> ok=1 140.6 GB
// D:/ (no such drive) -> ok=0 ; C:/no/such/dir -> ok=0   => walk up to the nearest existing ancestor; on failure => Warn "не удалось определить", not Fail
```
Use `lpFreeBytesAvailableToCaller` (honors quotas). When `out_root` and `cs2_dir` share a volume (same path prefix/drive letter), compare `free` against the **sum** of both needs.

### Steam state from `appmanifest_730.acf` (local file + cited bit table)
```text
"StateFlags"   "4"            <- FullyInstalled, idle
"buildid"      "25815307"
"TargetBuildID" "0"  "BytesToDownload" "0"  "BytesDownloaded" "0"
```
[VERIFIED: read of `C:\Program Files (x86)\Steam\steamapps\appmanifest_730.acf`]. Bit meanings [CITED: docs.linuxgsm.com/steamcmd/errors, page notes the table is from 2015 and "probably outdated"]: 1 Uninstalled, 2 UpdateRequired, 4 FullyInstalled, 8 Encrypted, 16 Locked, 32 FilesMissing, 64 AppRunning, 128 FilesCorrupt, 256 UpdateRunning, 512 (listed as UpdateRunning again; other sources call it UpdatePaused `[ASSUMED]`), 1024 UpdateStarted, 2048 Uninstalling, 4096 BackupRunning, 65536 Reconfiguring, 131072 Validating, 262144 AddingFiles, 524288 Preallocating, 1048576 Downloading, 2097152 Staging, 4194304 Committing, 8388608 UpdateStopping.

Recommended mapping (D-02/DIF-02): **fail** when any "in-progress" bit is set (256, 1024, 2048, 4096, 65536, 131072, 262144, 524288, 1048576, 2097152, 4194304, 8388608) or bit 4 is missing (not fully installed) or bit 32/128 (files missing/corrupt); **warn** when only 2 (UpdateRequired) or 512 is set, or `BytesToDownload != BytesDownloaded` with `TargetBuildID != 0`; ignore 64 (AppRunning). Tolerate a missing/unparsable manifest with `warn` ("не удалось определить состояние Steam"), never `fail`. Locate the manifest as `Path(cs2_dir)/../../appmanifest_730.acf` (= `<library>/steamapps/`) — settings value on this machine is `C:/Program Files (x86)/Steam\steamapps/common/Counter-Strike Global Offensive` [VERIFIED: settings.json].

### CS2 build identity (verified values)
`steam.inf` (`game/csgo/steam.inf`): `ClientVersion=2000930`, `PatchVersion=1.41.9.0`, `SourceRevision=11106093`. `geometry::game_version` already parses `ClientVersion` (`geometry/src/lib.rs:286`). Pair key recommendation: **`buildid` from the manifest** (changes with every Steam update, which is what can break the HLAE hook) + `PatchVersion` for the human-readable message; HLAE side: `HLAE.exe` file version `2.192.7.0` and hook version `0.41.7` (first `<version>` in `AfxHookSource2_changelog.xml`).

### Source-grep guard for TMO-05
```rust
// Source: new test, pattern of existing sentence-named tests
#[test]
fn no_process_is_ever_stopped_by_image_name() {
    // walk crates/**/*.rs except this file and examples/; assert none contain "taskkill" or "tasklist"
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `tasklist`/`taskkill /IM cs2.exe` in `launch.rs` | Job Object + handle wait | Phase 1 (plan 01-06/01-07) | Already removed [VERIFIED: grep empty] |
| `Command::output()` for NVENC probe | `Host::run` with deadline + cancel | This phase (WR-03) | Probe can no longer hang a task |
| Fixed `close_settle` 3 s after game exit | wait for `JobEmpty` (30 s) | This phase (WR-05) | No truncated last take |
| Machine kills job instantly on cancel | driver staged shutdown + exit confirmation | This phase | Clean state is *verified*, not assumed |

**Deprecated/outdated:** the `Host::run`-less NVENC probe in `launch.rs`; the generic `reset_running` message "прервано: приложение закрылось во время выполнения" for `cancelling` jobs (they must become `cancelled`).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The real AfxHookSource2 `ErrorBox` dialog is a standard `#32770` MessageBox owned by `cs2.exe` (source shows `MessageBoxA`, stand-in verified; real HLAE error never triggered) | Pitfall 6/7, LCH-05 | Detector misses a non-standard window; fallback is the deadline snapshot of titles, and `HLAE_ERROR` via the AFXERROR console line |
| A2 | Statuses 512 = UpdatePaused (source lists 512 as UpdateRunning too) | Steam mapping | A paused update is classed as fail instead of warn (or vice versa) |
| A3 | If `ffmpeg.ini` is absent HLAE falls back to `<hlae>/ffmpeg/bin/ffmpeg.exe` | PRE-03 | Wrong hint text only; the check fails either way when neither exists |
| A4 | Size constants: ~10 MB per game-second at 1080p (NVENC `-qp 16`), scaled by pixel area, + 0.192 MB/s audio WAV; floors 3 GiB / 1 GiB | PRE-06 | Too low -> false "ok" then a disk-full mid-record; too high -> false `fail`. Local data: 6.3, 8.6, 8.9 MB/s over 3 projects (46-69 takes). The x264 (non-NVENC take) path is larger and untested `[ASSUMED]` |
| A5 | Benign `AFXERROR` lines do not occur in healthy runs (only two distinct texts seen, both from the stand-in's missing `mirv_streams record fps`) | LCH-05 / D-09 | False HLAE_ERROR; mitigated by the `afxerror_ignore` setting |
| A6 | A paused queue after cancel is in-memory only (not restored on restart) | D-13 | After restart queued jobs auto-start; confirm with user (Q3) |
| A7 | `GetFileVersionInfoW` works for `HLAE.exe` (PowerShell read of the same field returned `2.192.7.0`) | DIF-01 | Fall back to the changelog hook version |

## Open Questions

1. **Two-pass space check vs "pre-check first" (D-01/D-03).**
   - What we know: the plan does not exist before download/parse; ROADMAP criterion 2 says "по оценке плана".
   - What's unclear: whether the user accepts pass 1 = floor, pass 2 = plan-based, both in one report.
   - Recommendation: adopt two-pass; surface in plan-checking as an interpretation of D-01/D-03, not a contradiction.
2. **TMO-04 "quit → ~5 s → terminate" on the success path.**
   - What we know: quit→empty job took 1.3-8.4 s in spikes; one run hung; WR-05 wants `JobEmpty`, not a fixed pause.
   - Recommendation: 5 s for cancel/failure, 30 s (`game_close`, existing) waiting for `JobEmpty` on success, then terminate. Needs a one-line confirmation from the user.
3. **Queue pause after cancel (D-13): do "Повторить"/new jobs auto-resume it? Persist across restart?**
   - Recommendation: pause is in-memory; any explicit user enqueue/retry also clears it (it is a deliberate action); ui_phase contract should fix wording.
4. **`geometry::latest_release_tag` (`git ls-remote`, no deadline) and `thumbnail()`:** in or out of TMO-03's "путь записи"?
   - Recommendation: route `thumbnail` through `Host::run` (trivial); leave `git` as a known, documented gap unless the user wants it bounded.
5. **Legacy `cs2cinema_*.dem` copies (six, ~2 GB) already in `game/csgo`:** one-time sweep?
   - Recommendation: out of scope (not in D-05/D-06); mention in the UAT note.
6. **Real-HLAE verification.** The modal-window detector and the "no visible `#32770` in a healthy run" assumption need one real recording with window lines logged to `record.log` (Phase 1 UAT is still `[pending]` for all 6 tests). Cannot be run by research (a foreign `cs2.exe` is live).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | build/tests | ✓ | cargo/rustc 1.99.0 | — |
| `windows-sys` 0.61.2 (offline registry) | host win code | ✓ | 0.61.2 | — |
| ffmpeg / ffprobe on PATH | PRE-03, assemble | ✓ | 9.0.2 (Gyan full build, WinGet) | — |
| HLAE | PRE-03, DIF-01 | ✓ | `HLAE.exe` 2.192.7.0; hook 0.41.7; `ffmpeg\ffmpeg.ini` `Path=` -> WinGet ffmpeg.exe | — |
| CS2 + Steam | PRE-04/05, DIF-01/02 | ✓ | `steam.exe` running (PID 31764), `steamwebhelper.exe`, `steamservice.exe`; `appmanifest_730.acf` `StateFlags 4`, `buildid 25815307`; `PatchVersion=1.41.9.0` | — |
| Foreign `cs2.exe` right now | PRE-05 manual test | ✓ (user's) PID 16528, created 2026-10-10 17:18, image path readable via `QueryFullProcessImageNameW` | — | Do not touch |
| Python | — | ✗ | — | not needed (shell/Rust used) |
| `gh` CLI | — | ✗ | — | not needed |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** none relevant.
Real-game tests (Job/IOCP/windows) run only under `cfg(windows)` with stand-in processes (`cmd`/`ping`/`wscript`), mirroring `crates/host/tests/win_session.rs`.

## Security Domain

`security_enforcement` is enabled (ASVS level 1, block on high) in `.planning/config.json`.

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication / V3 Session | no | local single-user desktop app |
| V4 Access Control | partial | Process control only through handles the app created (Job / `open_owned` after path+creation-time match); never by name |
| V5 Input Validation | yes | Untrusted text: console lines, window titles/text (come from another process) -> sanitise with the existing `record::log::clean` before journal/DB/UI; `cleanup`/`installed.json` paths validated before deletion; `afxerror_ignore` is substring match only (no regex from settings) |
| V6 Cryptography | no | none |
| V12 Files and Resources | yes | Deletion restricted to `<cs2_dir>/game/csgo` and `.../cfg` with exact name patterns (`cs2cinema_*.dem`, `cs2c_*.cfg|xml`, `highlights.cfg`) after canonicalising; `installed.json` is treated as untrusted input (it lives in a user-writable folder); write probe uses a unique temp file name and removes it |
| V14 Configuration | yes | No new secrets; report text must not echo `faceit_key` |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Killing the user's own `cs2.exe` | Tampering/DoS | Job Object + handle kill only; foreign process => precheck `fail`, never terminate (PRE-05); grep guard test |
| PID reuse (kill/adopt the wrong process) | Tampering | Keep `OwnedProc` handle open (never re-open by PID later); adoption requires path match + `created >= launcher_created` (existing `pick_adoptee`); stuck registry stores handle + creation time |
| Path traversal via `installed.json` / job folder | Tampering | Canonicalise; require prefix `<cs2_dir>/game/csgo`; refuse `..`, reparse points not resolved under the prefix |
| Hostile window text/title into UI/log (control chars, huge strings) | Tampering | `clean()` + length cap (IN-04 in 01-REVIEW: current `clean` does not truncate — cap window text to ~300 chars here) |
| `SendMessage` to a hung foreign window blocking the app | DoS | Avoid `SendMessage*`; `GetWindowTextW` only; PID filter against the job |
| Netcon on `0.0.0.0` without password | Info disclosure/Tampering | Residual risk from Phase 1 (S2); unchanged, mention in threat notes — shorter sessions via faster failure/cancel in this phase reduce exposure |
| Old app version reading `cancelling` | Integrity | `JobStatus::parse` falls back to `Failed`; documented one-way (D-11) |

## Sources

### Primary (HIGH confidence)
- Local code read this session: `crates/host/src/{lib,fake,runner}.rs`, `crates/host/src/win/{mod,job,process,watch}.rs`, `crates/pipeline/src/record/{mod,machine,deadlines,failure}.rs`, `crates/pipeline/src/lib.rs`, `crates/store/src/lib.rs`, `crates/app/src/{jobs,launch,jobs_page,settings,settings_page,app}.rs`, `crates/renderer/src/lib.rs`
- `.planning/phases/01-.../01-SPIKES.md`, `01-REVIEW.md`, `01-VERIFICATION.md`, `01-UAT.md`; `.planning/{REQUIREMENTS,ROADMAP,STATE}.md`; `02-CONTEXT.md`
- Scratch crate (session scratchpad `spike2`) compiled against `windows-sys =0.61.2`, run on the author's machine: IOCP message sequence, window enumeration of a stand-in MessageBox, `GetDiskFreeSpaceExW`, foreign-process query/wait (`PROCESS_QUERY_LIMITED_INFORMATION|SYNCHRONIZE` on PID 16528 returned the image path and `WAIT_TIMEOUT`)
- Local files: `appmanifest_730.acf`, `steam.inf`, `%LOCALAPPDATA%\hlae\ffmpeg\ffmpeg.ini`, `AfxHookSource2_changelog.xml`, `settings.json`, job output folders (clip sizes)
- `windows-sys-0.61.2` registry source (function/const signatures)
- `gsd-tools query package-legitimacy check` (windows-sys: OK)

### Secondary (MEDIUM confidence)
- [AfxHookSource2 `main.cpp` (advancedfx)](https://raw.githubusercontent.com/advancedfx/advancedfx/main/AfxHookSource2/main.cpp) — `ErrorBox(...)` for startup/hook failures, and a `MessageBoxA` loop when `-insecure` is missing (fetched and summarised by the fetch tool; quoted excerpt not independently re-read)
- [LinuxGSM SteamCMD errors page](https://docs.linuxgsm.com/steamcmd/errors) — `StateFlags` bit table; page itself says it is from 2015 and probably outdated

### Tertiary (LOW confidence)
- LaunchBox help article (via search result summary): `StateFlags` 4 = installed, 6 = update required, 1 = uninstalled — consistent with the table above

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new crates; features and signatures verified against the locked registry source and by compiling.
- Architecture: HIGH for host/pipeline mechanics (existing code read in full); MEDIUM for UI/queue details (UI-SPEC will fix wording).
- Pitfalls: HIGH for those observed in code/spikes (1-5, 7, 9); MEDIUM for 6 (needs one real run) and 8.
- Steam/disk constants: MEDIUM (cited table, three local samples).

**Research date:** 2026-10-10
**Valid until:** 2026-11-09 (HLAE/CS2 update cadence is fast; re-check the HLAE `ErrorBox` behaviour and `StateFlags` if either is updated before execution)
