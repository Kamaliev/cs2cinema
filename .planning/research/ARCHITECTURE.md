# Architecture Research

**Domain:** Automated CS2 demo-to-montage recording pipeline (HLAE + CS2 + ffmpeg), Rust desktop app on Windows, `std::thread` only
**Researched:** 2026-10-10
**Confidence:** HIGH for structure and patterns (derived from the actual code in `launch.rs`, `jobs.rs`, `pipeline`, `store`, `renderer`); MEDIUM for Win32 specifics (Job Objects, handle waits); LOW for HLAE/CS2 runtime behaviour that nobody has observed yet (process tree under `-customLoader`, console text on `playdemo` failure). Those are flagged as spikes in the build order.

## What the code does today (the facts this design starts from)

| Fact (source) | Consequence for the design |
|---|---|
| `jobs::record()` mixes launch, console connect, READY wait, `exec` retry, take-marker parsing and clip waiting in one function held together by `Cell`/`RefCell` closures (`jobs.rs:189-287`). | The sequence is an implicit state machine. Make it explicit and pure, then it is testable. |
| Liveness = `tasklist` text match, `Err(_) => true` (`launch.rs:251-265`). Kill = `taskkill /IM cs2.exe /F`. | Replace with an OS handle/Job Object we own. Liveness becomes an event, not a poll that can lie. |
| `Console::connect` and the READY loop use `thread::sleep(1s / 250ms)` and check `cancelled()` between sleeps (`launch.rs:184`, `jobs.rs:209`). OK for cancel latency, but each wait has its own ad-hoc bound. | One event loop with `recv_timeout(tick)` and a deadline per state. |
| Take completion = `mtime` stable for 3/8/20 s (`launch.rs:280-314`). Markers exist already: `CS2C_READY` (sent by the app via `mirv_cmd addAtTick 64`) and `CS2C_TAKE_DONE <i>` (emitted by `cs2c_NN.cfg`, `renderer/src/lib.rs:229`). There is **no** take-start marker and no "script began" marker. | Add `CS2C_SCRIPT` (top of `highlights.cfg`) and `CS2C_TAKE_START <i>` (next to `mirv_streams record start`). These are what make "cfg did not execute" distinguishable from "take did not finish". |
| The UI derives take progress by string-parsing log lines (`"записан дубль N/M"`, `"склейка: P%"`, `status.rs:63-73`). | Stringly-typed coupling. Introduce a typed event on `Progress` (default-implemented so nothing else breaks), keep emitting the legacy strings until the UI is migrated. |
| `Progress` has `stage/log/cancelled` only; failures are `String`s and cancel is the sentinel `CANCELLED`. | Keep `pipeline::Result<T> = Result<T,String>` at the outer boundary (do not churn 12 crates), but the record/assemble code returns a typed `Failure` that is lowered to a String + structured columns at the edge. |
| `store::reset_running` marks interrupted jobs `failed` with a generic text; `start_job` wipes `log`; `append_log` does `log = log \|\| ?` (O(n) per line, unstructured). | Add checkpoint + attempt + structured events; stop wiping history per attempt. |
| `plan_hash` + `clips/plan.hash` is a half-checkpoint ("takes already recorded for this plan") living in a loose file (`jobs.rs:147-152`). | Generalise into a `clips/attempt.json` + `record.json` manifest, make the DB the index and the disk the evidence. |
| CI runs `cargo test --workspace` on **ubuntu and windows** (`ci.yml`). | The new host crate must compile on Linux (stub impl) and the fake-based tests must run on both. |

## Standard Architecture

### System Overview

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ Front-ends                                                                     │
│  app: jobs.rs (thin)  ── JobReporter: Progress ──► Store + UI Msg channel       │
│  cli: main.rs (thin)  ── CliProgress: Progress ──► stdout                       │
└──────────────┬─────────────────────────────────────────────────────────────────┘
               │ pipeline::run_job(job_spec, resume_from, &dyn Progress, &Services)
┌──────────────▼─────────────────────────────────────────────────────────────────┐
│ pipeline (orchestration, pure logic, no OS calls)                              │
│                                                                                │
│  analyze ─► plan_player ─► manifest.rs (record.json) ─► checkpoint: Planned     │
│                                  │                                             │
│  preflight.rs ──► record/mod.rs (driver loop) ◄──► record/machine.rs (reducer)  │
│       │                │   ▲                         │ Actions                  │
│       │                │   │ Events                  ▼                          │
│       │                ▼   │                   Progress::event / log / stage    │
│       │         record/takes.rs (verify)  record/markers.rs (parse console)     │
│       └──► checkpoint: Recorded ──► assemble.rs ──► checkpoint: Assembled       │
└──────────────┬─────────────────────────────────────────────────────────────────┘
               │ traits only: GameHost, GameSession, CommandRunner
┌──────────────▼─────────────────────────────────────────────────────────────────┐
│ host (NEW leaf crate; no workspace deps)                                        │
│  WinHost (cfg(windows)): Job Object, netcon thread, job-watcher thread, probes  │
│  UnsupportedHost (cfg(not(windows))): compiles on CI/Linux, returns errors      │
│  StdRunner: ffmpeg/ffprobe spawn, pipe draining, timeout, cancel, kill          │
│  fake: FakeHost + Scenario, ReplayHost (golden console transcript), FakeRunner  │
└──────────────┬─────────────────────────────────────────────────────────────────┘
               ▼
        HLAE.exe ─► cs2.exe (in our Job Object)  ──netcon TCP──►  console lines
        ffmpeg / ffprobe (child processes)

store (SQLite): jobs (+attempt, checkpoint, resume_from, phase, fail_*) + job_events (structured log)
```

### Component Responsibilities

| Component | Lives in | Responsibility | Talks to |
|---|---|---|---|
| `GameHost` / `GameSession` / `CommandRunner` traits, `GameEvent`, `HostError`, `LaunchSpec`, `CmdSpec` | `crates/host/src/lib.rs` | The only seam to the OS. No workspace deps so `renderer` can use `CommandRunner` later. | implemented by `WinHost`, `fake::*`; consumed by `pipeline` |
| `WinHost` | `crates/host/src/win.rs` (`cfg(windows)`, `windows-sys` or `win32job` + `windows-sys`) | Spawn HLAE, put it in a Job Object (`KILL_ON_JOB_CLOSE`), discover/verify the cs2 PID, event threads, kill by job. Probes: Steam running, processes by image, free disk. | Win32, TCP netcon |
| `StdRunner` | `crates/host/src/runner.rs` | One implementation of "run ffmpeg/ffprobe": `stdin(null)`, `CREATE_NO_WINDOW`, drain stdout+stderr in threads, `try_wait` every 100 ms, hard timeout, cancel closure, kill on either. Replaces the 5 duplicated spawn sites. | OS processes |
| `fake` module | `crates/host/src/fake.rs` (plain `pub mod`, not `cfg(test)`, so `pipeline` and `cli` tests can use it) | `FakeHost` driven by a `Scenario`; `ReplayHost` replays a recorded console transcript; `FakeRunner` with scripted results. | tests |
| `FailKind`, `RecordPhase`, `Failure`, `Event` | `crates/pipeline/src/failure.rs` | Typed failure taxonomy (below); Russian `Display`; stable `code()` string persisted in the DB. | everything in pipeline; lowered by app |
| `Machine` (reducer) | `crates/pipeline/src/record/machine.rs` | Pure `step(now, Input) -> Vec<Action>`. Owns phases, per-phase deadlines, exec-retry count, take bookkeeping, console tail ring buffer. No I/O, no threads, no `Instant`. | nothing (pure) |
| Driver | `crates/pipeline/src/record/mod.rs` (`pub fn record(...)`) | The one loop: `recv_timeout`, cancel check, feed `Machine`, execute `Action`s against `GameSession`/`CommandRunner`/`Progress`. | `GameSession`, `CommandRunner`, `Progress` |
| `markers` | `record/markers.rs` | `parse_marker(&str) -> Option<Marker>`; includes the "ignore echo of our own command" rule currently inlined in `jobs.rs:199`. | Machine |
| `takes` | `record/takes.rs` | `TakeSpec {index, game_secs, files}`; `verify_take()` = files exist + `ffprobe` succeeds (a mp4 without `moov` fails ffprobe, so success also means "finalised") + duration/frame sanity. | `CommandRunner` |
| `install` | `record/install.rs` | Copy `highlights.cfg`/`cs2c_*` into `game/csgo/cfg`, decompress demo into `game/csgo`. Moved out of `launch.rs`, testable with a tempdir. | filesystem |
| `preflight` | `pipeline/src/preflight.rs` | `Vec<Check>` → `Vec<CheckResult {id, status: Ok/Warn/Fail, msg, hint}>`; runs **all** checks, never short-circuits, so the user sees every problem at once. | `GameHost` probes, `CommandRunner` (`ffmpeg -version`), fs |
| `RecordManifest` | `pipeline/src/manifest.rs` → `out_dir/record.json` | Everything the record+assemble stages need without re-parsing the demo: demo path/name, frame size, fps, shot count, per-take `{index, game_secs, expected_out_secs}`, `plan_hash`, ffmpeg args, `video_secs`. Written at the end of `plan_player`. | record, assemble, store (path only) |
| `assemble` | `pipeline/src/assemble.rs` | `pipeline::assemble(&dyn CommandRunner, &manifest, &dyn Progress)`: runs ffmpeg with `-progress pipe:1`, emits `склейка: P%` events. `jobs.rs::run_ffmpeg` is deleted. `renderer::assemble::assemble` (used by `cs2-cli assemble`) is migrated onto `CommandRunner` in a later phase. | `CommandRunner` |
| `Store` v2 | `crates/store/src/lib.rs` | Index of attempts/checkpoints and structured events (below). Knows strings, not `pipeline` types (no dependency inversion). | app |
| `JobReporter` | `crates/app/src/jobs.rs` | `impl Progress`: writes `job_events`, updates `phase`, forwards to the UI `Msg` channel. | Store, UI |

## Recommended Project Structure

```
crates/
├── host/                         NEW (leaf; windows-sys only under cfg(windows))
│   └── src/
│       ├── lib.rs                traits + plain data types
│       ├── win.rs                WinHost (Job Object, netcon, probes)
│       ├── unsupported.rs        non-Windows stub
│       ├── runner.rs             StdRunner (ffmpeg/ffprobe)
│       └── fake.rs               FakeHost, Scenario, ReplayHost, FakeRunner
├── pipeline/
│   └── src/
│       ├── lib.rs                existing + Progress::event (default method), run_job()
│       ├── failure.rs            FailKind, RecordPhase, Failure, Event
│       ├── manifest.rs           RecordManifest (record.json), plan_hash
│       ├── preflight.rs
│       ├── record/
│       │   ├── mod.rs            driver loop
│       │   ├── machine.rs        pure reducer + unit tests (no threads, no time)
│       │   ├── markers.rs
│       │   ├── takes.rs
│       │   └── install.rs
│       └── assemble.rs
├── store/                        schema v2 (migration by PRAGMA user_version)
├── app/src/
│   ├── jobs.rs                   shrinks to: pick job → resume_from → run_job → persist outcome
│   └── launch.rs                 shrinks to discovery only: find_cs2, find_hlae, hlae_ffmpeg
└── cli/                          calls pipeline::run_job with CliProgress + same StdRunner/WinHost
```

### Structure Rationale

- **`host` is a separate leaf crate, `record` is a module of `pipeline`.** The requirement says record/assemble live in `pipeline` behind `Progress`. `Progress` is defined in `pipeline`, so a separate `recorder` crate that depends on `pipeline` would create a cycle, and one that `pipeline` depends on could not see `Progress`. Splitting by *direction of knowledge* solves it: `host` knows nothing about jobs, stages or Russian messages (it only moves bytes and processes); `pipeline` knows the protocol and policy.
- **`Machine` has no I/O and no clock.** `step(now_ms: u64, input)` takes time as an argument, so "console opened at 90.1 s, deadline was 90 s" is a one-line test, no sleeping, no fake clock trait.
- **Fakes are a normal `pub mod`**, not `#[cfg(test)]`, because `crates/cli/tests/pipeline.rs` and `pipeline` integration tests must reach them across crates.
- **`store` stays free of `pipeline` types.** It persists `stage`, `phase`, `kind` as strings; `pipeline::FailKind::code()` and `Stage::title()` produce them. Avoids a `store → pipeline` edge and keeps the crate DAG intact.
- **`launch.rs` keeps discovery** (`find_cs2`, `find_hlae`, settings helpers) because those are UI/settings concerns, not process control.

## The seam (trait design)

```rust
// crates/host/src/lib.rs
pub enum GameEvent {
    LauncherExited { code: Option<i32> },   // HLAE.exe stub finished (it exits right after starting cs2)
    GameAppeared  { pid: u32 },             // a cs2.exe process is inside our job
    ConsoleUp,                              // netcon TCP connect succeeded
    Console(String),                        // one console line (lossy UTF-8, trimmed)
    ConsoleDown,                            // socket closed / read error
    GameExited    { code: Option<u32> },    // job active-process count hit zero after GameAppeared
}

pub trait GameHost: Send + Sync {
    /// Probes return Result: "could not find out" is NOT "alive" and NOT "absent".
    fn pids_by_image(&self, image: &str) -> Result<Vec<u32>, HostError>;
    fn steam_running(&self) -> Result<bool, HostError>;
    fn free_disk(&self, path: &Path) -> Result<u64, HostError>;
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn GameSession>, HostError>;
}

pub trait GameSession: Send {
    fn events(&self) -> &std::sync::mpsc::Receiver<GameEvent>;  // single merged channel
    fn send(&mut self, console_line: &str) -> Result<(), HostError>;
    fn terminate(&mut self);            // idempotent; TerminateJobObject; also called from Drop
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, spec: &CmdSpec, cancel: &dyn Fn() -> bool) -> Result<CmdOutput, HostError>;
    // CmdSpec { program, args, cwd, timeout: Duration, stdout_line: Option<&dyn Fn(&str)> }
    // HostError::{Spawn, Timeout{tail}, Cancelled, Io}
}
```

Why this shape:

- **One merged `Receiver<GameEvent>`** per session. Console lines, process exit and "launcher finished" all arrive on the same channel, so the driver blocks in exactly one place (`recv_timeout`). This is the central pattern: no `Cell`/`RefCell` closure web, no per-subsystem polling loops.
- **No blocking wait methods on `GameSession`.** All deadlines are policy and live in `Machine`. The host never decides how long to wait; it only reports facts. That is what makes timeouts testable.
- **`terminate()` is the only way to stop the game**, and it acts on the Job Object we created, never on an image name.
- **`CommandRunner::run` takes a cancel closure and a mandatory timeout**: the type system forces every ffmpeg/ffprobe call to be bounded and cancellable.

## Architectural Patterns

### Pattern 1: Single event channel + `recv_timeout` tick loop (the driver)

**What:** The job worker thread owns the loop. Producer threads (console reader, job watcher) only `tx.send(GameEvent)`. The driver wakes at least every `TICK = 200 ms`, checks `Progress::cancelled()`, converts wall time to `now_ms`, calls `machine.step`.
**When:** Every wait in the record stage (process start, console, demo load, script ack, take, finalise, quit).
**Trade-offs:** Cancel latency is bounded by `TICK` (plus action time). No tokio/async needed. Slight CPU wake-ups (5/s) are irrelevant.

```rust
pub fn record(host: &dyn GameHost, run: &dyn CommandRunner, m: &RecordManifest,
              opt: &RecordOptions, p: &dyn Progress) -> Result<RecordOutcome, Failure> {
    let mut session = host.launch(&opt.launch_spec(m))?;          // Drop => terminate()
    let mut mach = Machine::new(m.takes(), opt.timeouts.clone());
    let t0 = Instant::now();
    loop {
        let input = match session.events().recv_timeout(TICK) {
            Ok(ev) => Input::Game(ev),
            Err(RecvTimeoutError::Timeout) => Input::Tick,
            Err(RecvTimeoutError::Disconnected) => Input::Game(GameEvent::GameExited { code: None }),
        };
        let now = t0.elapsed().as_millis() as u64;
        let mut actions = mach.step(now, if p.cancelled() { Input::Cancel } else { input });
        for a in actions.drain(..) {
            match a {
                Action::Send(cmd)   => { if let Err(e) = session.send(&cmd) { mach.feed_send_error(now, e) } }
                Action::Emit(ev)    => p.event(&ev),
                Action::Probe(i)    => mach.feed_probe(now, i, takes::verify(run, m, i, || p.cancelled())),
                Action::Terminate   => session.terminate(),
                Action::Finish(r)   => { session.terminate(); return r; }
            }
        }
    }
}
```

### Pattern 2: Pure reducer with explicit phases and per-phase deadlines

**What:** `Machine` is a plain struct; `step` is a `match (phase, input)`. A deadline is set on phase entry (`self.deadline = now + timeouts.for_phase(phase)`); the `Tick` arm compares `now >= deadline` and transitions to `Failed(kind)`.
**When:** Always. This is the object the "does not start / timeout / cancel mid-wait" unit tests target directly, with no host at all.
**Trade-offs:** More ceremony than the current closure, but every transition is greppable and every failure has exactly one exit point.

State machine (record stage):

```
Preflight ──ok──► Launching ──GameAppeared──► AwaitConsole ──ConsoleUp──► AwaitDemo
   │fail             │deadline/LauncherExited      │deadline                  │READY marker
   ▼                 │ with no game                ▼                          ▼
PreflightFailed      ▼                       ConsoleNotOpen            AwaitScript  (send "exec highlights")
                  GameNotStarted                                          │ CS2C_SCRIPT / first CS2C_TAKE_START
 AwaitDemo deadline ─► DemoNotLoaded                                      │ retry exec every 30 s, max 4
 AwaitScript deadline ─► ScriptNotExecuted                                ▼
                                                                  Take(i): Armed ─TAKE_START─► Recording ─TAKE_DONE─► (Probe i queued)
 Take deadline ─► TakeTimeout(i)                                     └─ i == last, all probes ok ─► Closing
 Probe fail/timeout ─► TakeInvalid(i)                                Closing: send "quit", wait GameExited ≤15 s, else terminate ─► Done
 GameExited in any phase before Done ─► GameExited{phase}
 Cancel in any phase ─► Cancelling: terminate, wait GameExited ≤3 s ─► Cancelled
```

Phases and default deadlines (all in `RecordTimeouts`, with `RecordTimeouts::fast()` = milliseconds for tests; the numbers below are starting points to be tuned in the acceptance run, not facts):

| Phase | Entry condition | Deadline | Failure kind on expiry |
|---|---|---|---|
| Preflight | job reaches Record | n/a (each check has its own 5 s bound) | `PreflightFailed` |
| Launching | `host.launch` returned | 30 s to `GameAppeared` | `GameNotStarted` |
| AwaitConsole | `GameAppeared` | 90 s to `ConsoleUp` (HLAE + shader compile can be slow) | `ConsoleNotOpen` |
| AwaitDemo | `ConsoleUp`, READY `mirv_cmd` sent | `max(settings.load_wait_secs, 150 s)` to `CS2C_READY` (the existing `LOAD_CAP`) | `DemoNotLoaded` |
| AwaitScript | `exec highlights` sent | 30 s per attempt × 4 (existing `EXEC_RETRY`/`EXEC_RETRIES`) to first `CS2C_TAKE_START` | `ScriptNotExecuted` |
| Take(i) | `TAKE_START i` | `take.game_secs × K + seek slack` from the manifest (K ≈ 6, slack 30 s; tune) **and** idle watchdog: no console line and no byte growth in `take{i}` for 120 s | `TakeTimeout(i)` / `Stalled(i)` |
| Probe(i) | `TAKE_DONE i` | 30 s of retries (1 s apart) until `ffprobe` accepts the file | `TakeInvalid(i)` |
| Closing | all takes ok | 15 s | falls back to `terminate()`, logged as warning, not an error |
| Cancelling | cancel flag seen | 3 s then force | `Cancelled` (always wins over timeouts) |

### Pattern 3: Own the process tree with a Windows Job Object

**What:** `WinHost::launch` creates a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, spawns `HLAE.exe` via `std::process::Command` (with `CREATE_NO_WINDOW`), immediately `AssignProcessToJobObject(job, child_handle)` (child handle from `std::os::windows::io::AsRawHandle` on the `Child`). Everything HLAE spawns afterwards (cs2.exe) is in the job and cannot escape (we do not set `BREAKAWAY_OK`). The job-watcher thread polls `QueryInformationJobObject` every 500 ms: `JobObjectBasicProcessIdList` gives the PIDs (this is how `GameAppeared{pid}` is produced; no `tasklist`, no process spawn per poll), `JobObjectBasicAccountingInformation.ActiveProcesses == 0` after appearance produces `GameExited`. `terminate()` = `TerminateJobObject`. Dropping the `Job` handle kills the tree, so a panic or even a hard crash of the app does not leave an orphan game.
**When:** The game session only. ffmpeg is a single process; `Child::kill` + `wait` in `StdRunner` is enough.
**Trade-offs:** Needs a few dozen lines of `unsafe` behind `cfg(windows)` (or the `win32job` crate v2.0.3 for the job part: `Job::create`, `query_extended_limit_info`, `limit_kill_on_job_close`, `set_extended_limit_info`; it has no "enumerate PIDs" or "assign other process" convenience, so `windows-sys` is needed anyway — recommend `windows-sys` alone, features `Win32_Foundation`, `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Diagnostics_ToolHelp`, `Win32_Storage_FileSystem`). `AssignProcessToJobObject` can fail if the app itself runs inside a non-nestable job (pre-Windows 8, some launchers); treat as `HostError::JobAssign` and fall back to the PID path below.
**Fallback if cs2.exe does not land in the job** (race: HLAE created it before assignment, or HLAE hands off to another process): preflight already guarantees "no cs2.exe is running", so "the single new `cs2.exe` that appears after launch" is unambiguously ours. Find it via Toolhelp snapshot, `OpenProcess(PROCESS_TERMINATE|SYNCHRONIZE|PROCESS_SET_QUOTA)`, `AssignProcessToJobObject` it directly, then proceed as above. This is the **Spike 1** below; the design works either way, only `win.rs` changes.

### Pattern 4: Every thread has a stop flag and every blocking read has an exit

**What:** Console reader thread: `Arc<AtomicBool> stop`, `TcpStream::set_read_timeout(500 ms)` so the `read_until` loop can notice `stop`; `terminate()`/`Drop` sets the flag and calls `stream.shutdown(Both)`. Today the reader thread blocks until the socket dies (`launch.rs:207`) and leaks one thread per attempt. Connection attempts are made by the session's connector thread every 1 s with `connect_timeout(2 s)` until `ConsoleUp` — the *machine* decides when to give up, the thread just keeps reporting.
**When:** All `host` threads.
**Trade-offs:** Slightly more code in `win.rs`; removes thread leaks across the 5-job acceptance run.

### Pattern 5: Golden-transcript replay

**What:** During Spike 2, log the full console stream of one successful real run (and one with a deliberately wrong demo name) to `crates/pipeline/tests/fixtures/console_ok.log` / `console_demo_missing.log`. `ReplayHost` feeds those lines as `GameEvent::Console` with compressed timing. This tests `markers.rs` and the machine against **real** CS2/HLAE text without running CS2.
**When:** Phase 2 onward; refreshed after CS2/HLAE updates (a failing replay test after refresh = early warning of marker drift).
**Trade-offs:** Fixtures can go stale; that is the point.

### Pattern 6: Evidence-based checkpoints (DB is the index, disk is the truth)

**What:** The DB stores `checkpoint ∈ {'', planned, recorded, assembled}`. On resume, the checkpoint is trusted only if the artifacts validate: `Planned` ⇒ `record.json` loads and its `plan_hash` matches the hash of `highlights.cfg` + `cs2c_*` now on disk; `Recorded` ⇒ `clips/attempt.json` says `ok` **and** every expected take passes `verify_take`. If validation fails, fall back one stage (re-plan from demo, or re-record) and log why.
**When:** At the start of every attempt.
**Trade-offs:** A couple of file reads and `ffprobe` calls on resume; in return, no stale-checkpoint lies (the current `plan.hash` check is the seed of this).

### Pattern 7: Typed events on `Progress` via a default method

**What:** Add one method to the existing trait; every current implementer compiles unchanged.

```rust
pub trait Progress: Sync {
    fn stage(&self, s: Stage);
    fn log(&self, msg: &str);
    fn cancelled(&self) -> bool { false }
    /// NEW. Default falls back to `log`, so Quiet, the CLI and old tests keep working.
    fn event(&self, e: &Event) { self.log(&e.to_string()) }
}
pub struct Event { pub phase: Option<RecordPhase>, pub level: Level, pub kind: EventKind, pub msg: String }
pub enum EventKind { PhaseStart, PhaseOk { took_ms: u64 }, TakeProgress { done: u32, total: u32 }, Note, Fail(FailKind) }
```

`JobReporter::event` writes a `job_events` row, updates `jobs.phase`, and sends `Msg::Takes{done,total}` so `status.rs` stops string-parsing. Keep emitting the legacy `"записан дубль N/M"` line (via `log`) until `status.rs` and `jobs_page.rs` are migrated, then drop it.

## Failure taxonomy (maps 1:1 to the "failure point" requirement)

| `FailKind` (code) | Meaning | Detected by | Message (Russian, shown in job card) |
|---|---|---|---|
| `PreflightFailed{check}` | environment not ready; recording never started | `preflight.rs` | «Запись не начата: {check} — {hint}» (lists **all** failed checks) |
| `GameNotStarted` | HLAE failed or no cs2.exe inside the job | Launching deadline, or `LauncherExited{code!=0}` with no `GameAppeared` | «CS2 не запустилась за 30 с (HLAE завершился с кодом N)» |
| `ConsoleNotOpen` | cs2 alive, netcon never opened | AwaitConsole deadline | «Игра запущена (pid N), но консольный порт не открылся за 90 с» |
| `DemoNotLoaded` | console up, `CS2C_READY` never came | AwaitDemo deadline (game still alive) | «Игра запущена, консоль открыта, но демка не загрузилась за N с» |
| `ScriptNotExecuted` | demo plays, `highlights.cfg`/`cs2c_NN.cfg` not executed | AwaitScript after 4 retries | «Демка загружена, но скрипт highlights не выполнился (нет CS2C_SCRIPT)» |
| `TakeNotStarted(i)` / `TakeTimeout(i)` / `Stalled(i)` | marker start/end did not arrive in time | Take deadlines / idle watchdog | «Дубль i из N: не пришёл маркер конца за T с» |
| `TakeInvalid(i)` | marker arrived but file missing/truncated | `verify_take` | «Дубль i из N: файл неполный (ffprobe: …)» |
| `GameExited{phase}` | game died mid-run | `GameExited` before `Done` | «Игра закрылась на этапе «Демка» (код N)» |
| `Interrupted{stage,phase}` | app exited during the job | `recover_interrupted()` at next start | «Прервано на этапе «Запись», дубль 3 из 12: приложение было закрыто» |
| `Cancelled` | user cancel | cancel flag | «Отменено» (status `cancelled`, not `failed`) |
| `AssembleFailed` / `AssembleTimeout` | ffmpeg non-zero / hung | `CommandRunner` | «Склейка: ffmpeg завершился с ошибкой: …» |

Every `Failure` carries `tail: Vec<String>` (last ~20 console lines from the machine's ring buffer) and a path to the full transcript (see Logging).

## Data Flow

### Record-stage event flow (direction of every arrow matters)

```
 cs2.exe ──netcon TCP──► console reader thread ─┐
 HLAE.exe/cs2.exe ──Job Object accounting──► job-watcher thread ─┤──► mpsc<GameEvent> ──► DRIVER (job worker thread)
 HLAE launcher exit (Child::try_wait) ─────────────────────────┘                              │  recv_timeout(200ms)
                                                                                              │  + Progress::cancelled()
 UI cancel button ─► CancelToken(Arc<AtomicBool>) ─► JobReporter::cancelled() ────────────────┤
                                                                                              ▼
                                                                        Machine::step(now_ms, Input) -> Vec<Action>
                                                                                              │
       ┌────────────────── Action::Send ────► GameSession::send ────► netcon ► cs2 console   │
       ├────────────────── Action::Probe ───► CommandRunner(ffprobe) ──► Input::ProbeResult ──┘ (fed back)
       ├────────────────── Action::Emit ────► Progress::event ──► JobReporter ──► Store.job_events (+ jobs.phase)
       │                                                                      └──► UI Msg channel (take N/M, phase ladder)
       └────────────────── Action::Terminate / Finish ───► GameSession::terminate (TerminateJobObject)
```

### Job-level flow with checkpoints and retry

```
queued ──start_job(attempt+=1, resume_from)──► running
   resume_from = '' (fresh):      Download→Parse→Plan  ──write record.json──► checkpoint=planned
   resume_from = 'record':        load record.json, validate plan_hash       (skips Download/Parse/Plan)
                                  Preflight → record() ──all takes verified──► checkpoint=recorded
                                  assemble() ───────────highlights.mp4──────► checkpoint=assembled → done
   any stage fails:  status=failed, fail_stage/fail_phase/fail_kind set, checkpoint unchanged
   cancel:           status=cancelled, checkpoint unchanged
   app restart:      recover_interrupted(): running ⇒ failed(kind=interrupted, stage/phase preserved)
UI actions on failed/cancelled/interrupted jobs:
   «Повторить с записи»  enabled iff checkpoint ≥ planned and record.json validates  → requeue_from(id, 'record')
   «Повторить заново»    → requeue_from(id, '')
```

`jobs::run` becomes roughly:

```rust
let start = job.resume_from();                       // Plan | Record | Assemble
let manifest = if start == Plan || !validate(&out_dir) { plan_stage(...)?; checkpoint(Planned) } else { load(&out_dir)? };
if job.kind == JobKind::Plan { return outcome_plan }
if start <= Record || !recorded_ok(&out_dir, &manifest, &runner) {
    pipeline::preflight(&host, &runner, &settings, &manifest, p)?;
    pipeline::record(&host, &runner, &manifest, &opts, p)?;     // writes clips/attempt.json {attempt, status}
    checkpoint(Recorded)
}
pipeline::assemble(&runner, &manifest, p)?; checkpoint(Assembled)
```

Settings (timeouts, `load_wait_secs`, NVENC) are read at every attempt; the plan-immutable data (frame size, fps, shots, demo path) comes from the manifest so a retry cannot silently mix a new frame size with old cfgs.

### Store schema v2 (migration via `PRAGMA user_version`, current DBs are v0)

```sql
ALTER TABLE jobs ADD COLUMN attempt     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN checkpoint  TEXT    NOT NULL DEFAULT '';   -- '', planned, recorded, assembled
ALTER TABLE jobs ADD COLUMN resume_from TEXT    NOT NULL DEFAULT '';   -- '' | record | assemble
ALTER TABLE jobs ADD COLUMN phase       TEXT    NOT NULL DEFAULT '';   -- live RecordPhase code, survives crash
ALTER TABLE jobs ADD COLUMN fail_stage  TEXT;
ALTER TABLE jobs ADD COLUMN fail_phase  TEXT;
ALTER TABLE jobs ADD COLUMN fail_kind   TEXT;                          -- FailKind::code()

CREATE TABLE job_events (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id  INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,    -- foreign_keys=ON already set in init()
    attempt INTEGER NOT NULL,
    ts_ms   INTEGER NOT NULL,
    stage   TEXT NOT NULL,
    phase   TEXT NOT NULL DEFAULT '',
    level   TEXT NOT NULL,                                             -- info | warn | error
    kind    TEXT NOT NULL DEFAULT 'note',                              -- phase_start | phase_ok | take | note | fail
    msg     TEXT NOT NULL
);
CREATE INDEX job_events_job ON job_events(job_id, id);
```

Store API changes: `start_job(id)` stops wiping history and increments `attempt`; `append_log` → `append_event`; `set_stage` stays and `set_phase` is added; `finish_job(Err)` takes a `&FailInfo {stage, phase, kind, msg}`; `reset_running` → `recover_interrupted()` (status `failed`, `fail_kind='interrupted'`, message built from the *preserved* `stage`/`phase`/take counter); `requeue` → `requeue_from(id, ResumeFrom)`; `Job.log` is kept as a derived rendering of the latest attempt's events for UI compatibility, then removed. `delete_job` cascades events.

Recovery policy (opinionated): after a crash/close mid-job, **mark and offer** (one click «Повторить с записи»), do not auto-launch a game on app start. Auto-resume can be a later opt-in setting. Partial resume *within* the take chain (continue from take k) is an explicit non-goal: the cfg chain is sequential in one game session (`exec cs2c_NN` at each `end_tick`), so re-record the whole stage; the already-built `plan_hash` short-circuit covers the "takes are fine, assemble failed" case.

## Logging: making the failure point obvious

1. **Phase ladder in the job log.** Each phase emits `PhaseStart` and `PhaseOk{took_ms}`; the failing phase emits `Fail`. The job card renders the ladder (`Проверка ✓ 0.4 с · Запуск ✓ 6 с · Консоль ✓ 11 с · Демка ✗ 150 с`) from `job_events` where `kind in (phase_start, phase_ok, fail)`; the failed rung is highlighted. No log reading needed to see *where* it died.
2. **Last line of a failed job is always a fixed block**: kind, phase, elapsed vs deadline, pid (if any), last ~20 console lines, path to `logs/attempt-N/console.log`.
3. **Do not persist every console line in SQLite.** Persist markers, phase transitions, warnings and the failure tail; write the **full** console transcript (and the HLAE launcher exit code) to `out_dir/logs/attempt-{N}/console.log`. Keeps `job_events` small and gives a golden-transcript source for Pattern 5.
4. **Structured, not concatenated**: `append_event` is an `INSERT`, not `log = log || ?`.
5. **Attempt boundary**: events carry `attempt`; the UI shows the current attempt by default with a switcher.

## Cancellation guarantees (how "seconds, on any stage" is achieved)

| Stage | Mechanism | Latency bound |
|---|---|---|
| Record, any phase | driver `recv_timeout(200 ms)` + `Progress::cancelled()` ⇒ `Input::Cancel` ⇒ `Action::Terminate` (job kill) | ≤ 0.2 s + kill (~ms) + ≤ 3 s wait for `GameExited` |
| Preflight | each check bounded to 5 s; cancel checked between checks | ≤ 5 s worst case, typically ms |
| Probe / ffmpeg / ffprobe | `StdRunner` loop `try_wait` every 100 ms, checks cancel + timeout, `kill()` + `wait()` | ≤ 0.1 s |
| Plan / Parse | existing `check(p)?` between sub-stages; parse is CPU-bound and not interruptible mid-call | seconds on big demos (accepted) |
| Download (FACEIT) | **gap**: `faceit::Client::download` is a blocking `ureq` call with no cancel hook. Add a read-timeout and a chunked copy loop that checks `cancelled()` (small change in `faceit`, not in this design's core) | currently unbounded until HTTP timeout |
| Panics | wrap `jobs::run` in `catch_unwind`, convert to `failed(kind=panic)`; `GameSession: Drop` terminates the job | n/a |

After cancel/fail in Record: `clips/attempt.json` is rewritten with `status: "cancelled" | "failed:<kind>"` (it was `"running"` since start, so a hard crash leaves `"running"`, which resume treats as incomplete). The next attempt moves a non-`ok` `clips/` aside (`clips_attempt{N}`), as `jobs.rs:154` does today with `clips_old_{stamp}`.

## Anti-Patterns

### Anti-Pattern 1: Mocking at the `Command` level or in the middle of `launch.rs`
**What people do:** Add `#[cfg(test)]` branches inside `game_alive()` or wrap `Command` to return canned output.
**Why it's wrong:** It tests string parsing of `tasklist`, not the sequence/timeouts, and still cannot express "console opens after 80 s" or "game dies in take 3".
**Do this instead:** Fake at the event level: a `GameSession` that emits `GameEvent`s on a schedule (`Scenario`), plus a pure `Machine`. Time-dependent logic is tested by passing `now_ms`.

### Anti-Pattern 2: Poll-and-sleep waits with per-site timeouts
**What people do:** `loop { if cancelled() {..}; if probe() {break}; sleep(1s) }` repeated in each wait.
**Why it's wrong:** Every new wait re-invents bound, cancel and logging; `game_alive` returning `true` on error is exactly this class of bug.
**Do this instead:** One driver, one `recv_timeout`, deadlines in the machine, probes returning `Result`.

### Anti-Pattern 3: Identify the game by image name
**What people do:** `taskkill /IM cs2.exe /F`, `tasklist | find "cs2.exe"`.
**Why it's wrong:** Kills the user's own match; liveness of *a* cs2 is not liveness of *ours*.
**Do this instead:** Job Object (or at minimum a held PID handle); preflight refuses to start if another cs2.exe exists.

### Anti-Pattern 4: Completion by file mtime
**What people do:** "directory stopped changing for N seconds".
**Why it's wrong:** Slow disk ⇒ false "done"; paused game ⇒ false "done"; HLAE rename ⇒ miss.
**Do this instead:** Explicit marker (`TAKE_DONE i`) **and** `ffprobe` success on `take{i}/video.mp4` (+ `audio.wav` exists and non-empty). Expected take set comes from the manifest, not from the directory.

### Anti-Pattern 5: Errors as strings with a sentinel, progress as parsed log text
**What people do:** `if e == pipeline::CANCELLED`, `m.strip_prefix("записан дубль ")`.
**Why it's wrong:** Cannot persist a machine-readable failure point; UI breaks when wording changes.
**Do this instead:** `Failure{kind,phase,..}` + `Progress::event`; keep `CANCELLED` only as the lowering for the legacy `Result<T,String>` boundary.

### Anti-Pattern 6: Orphan helper threads
**What people do:** spawn a reader thread per connection and let it die with the socket.
**Why it's wrong:** Leaks across attempts; masks "console closed" as silence.
**Do this instead:** stop flag + read timeout + `shutdown`, `ConsoleDown` event.

## Scale Considerations

This is a single-user, single-machine tool; "scale" means queue length and accumulated artifacts, not users.

| Concern | 5 jobs (acceptance) | 50 queued jobs | Long-lived install |
|---|---|---|---|
| Concurrency | strictly 1 game; enforce in `start_job` with `UPDATE ... WHERE status='queued'` claim and preflight "no cs2.exe" | same | a second app instance would fight over CS2; preflight catches it |
| Disk | each attempt leaves GB of takes; failed attempts get `clips_attempt{N}` | needs a retention rule (keep last failed attempt only; delete older on new attempt after success) | surface disk usage in preflight (`free_disk`) |
| Log volume | tens of events/job | `job_events` small because console lines go to files | prune transcript files with job delete |
| Console fidelity | HLAE/CS2 update changes text | replay-fixture tests catch drift | pin tested CS2 `ClientVersion` + HLAE version in `record.json`/events and warn when different |

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---|---|---|
| HLAE.exe | spawned by `WinHost` with `-customLoader -noGui -autoStart -hookDllPath … -programPath … -cmdLine …` (existing args, unchanged), assigned to Job Object | Launcher exits shortly after starting cs2 (stated in the existing code comment); its exit code is an event, not a liveness signal. Flags were not verifiable from public docs in this research; they come from the working `launch.rs`. |
| CS2 console | TCP `127.0.0.1:<free_port>` via `-netconport`; per-launch free port (existing `free_port()`) | Output is not guaranteed UTF-8 (existing lossy decode kept). `echo` of our own command also arrives: `markers.rs` must ignore lines containing the command text (existing `!contains("echo") && !contains("addAtTick")` rule). |
| HLAE ffmpeg | `ffmpeg\ffmpeg.ini` `Path=` (existing `hlae_ffmpeg`) | Used for NVENC probe of takes; preflight reports which encoder will be used. |
| ffmpeg/ffprobe | only via `CommandRunner` | one implementation, one set of flags (`-hide_banner -nostats -loglevel error -progress pipe:1`), mandatory timeout. |
| Steam | probe only (`steam_running`: Toolhelp snapshot for `steam.exe`, or `HKCU\Software\Valve\Steam\ActiveProcess\pid` + liveness) | needed by `-steam`; fail-closed if probe errors. |

### Internal Boundaries

| Boundary | Communication | Notes |
|---|---|---|
| `app::jobs` ↔ `pipeline` | `pipeline::run_job(...)`/stage functions + `&dyn Progress` | `jobs.rs` keeps only: queue pick, `JobReporter`, outcome → `Store`. |
| `cli` ↔ `pipeline` | same functions with `CliProgress` (prints) and the same `WinHost`/`StdRunner` | CLI must still pass `crates/cli/tests/pipeline.rs`; it gets `FakeRunner`/`UnsupportedHost` in tests. Ctrl-C ⇒ optional later. |
| `pipeline` ↔ `host` | traits only | `host` has zero knowledge of stages/messages. |
| `pipeline` ↔ `store` | none directly | `app` translates `Failure`/`Event` to `store` calls. |
| `renderer` ↔ record protocol | `renderer` owns marker text constants (`TAKE_DONE`, new `SCRIPT`, `TAKE_START`, `READY`) | `pipeline::record::markers` imports them so cfg generation and parsing cannot drift; a unit test asserts every marker appears in generated cfgs. |
| `renderer::assemble` ↔ `CommandRunner` | later phase | `cs2-cli assemble` currently spawns ffmpeg itself; migrate after the app path is proven, not before. |

## Build Order (each phase independently testable; acceptance last)

Dependency graph: `S1,S2 (spikes) → 2 → 3 → 4`; `1 → 2`; `2 → 5`; `3 → 6`; `2 → 7` (needs only `FailKind`, not the host); `4,5,6,7 → 8 → 9`.

| # | Phase | Delivers | Testable without CS2 by | Depends on |
|---|---|---|---|---|
| 0 | Commit the dirty working tree as baseline | reproducible start (PROJECT.md decision) | existing `cargo test --workspace` | none |
| S1 | **Spike: process tree.** On the real machine: launch HLAE via a throwaway `host` example, assign to a Job Object, log `JobObjectBasicProcessIdList` over time. Question: is cs2.exe inside the job? Does `HLAE.exe` exit? What is its exit code? | decision: direct-job vs assign-after-appear fallback | manual, 15 min | none |
| S2 | **Spike: console transcripts.** Capture full netcon output for (a) a good run, (b) nonexistent demo name, (c) a cfg with a syntax error, (d) campath file missing. Save as fixtures. | golden transcripts; which console strings are reliable failure hints (hints only, never the only signal) | manual once, then fixtures run everywhere | none |
| 1 | **Seam + types.** `host` crate: traits, `GameEvent`, `HostError`, `UnsupportedHost`, `FakeHost`/`Scenario`, `FakeRunner`. `pipeline::failure` (`FailKind`, `RecordPhase`, `Failure`, `Event`) and `Progress::event` default method. Add renderer markers `SCRIPT` and `TAKE_START` to generated cfgs (+ test asserting presence). | compiles on Linux+Windows; nothing behaves differently yet | unit tests; CI both OSes | 0 |
| 2 | **Pure state machine** `record/machine.rs` + `markers.rs` + `RecordTimeouts`. | all failure kinds reachable and asserted: `GameNotStarted`, `ConsoleNotOpen`, `DemoNotLoaded`, `ScriptNotExecuted` (after 4 retries), `TakeTimeout`, `Stalled`, `GameExited{phase}`, cancel in **every** phase (table-driven test iterating phases), "cancel wins over a simultaneous deadline" | pure unit tests, `now_ms` injected, zero sleeps | 1 (+S2 fixtures for markers) |
| 3 | **Driver + fake scenarios.** `record/mod.rs`, `install.rs`, `takes.rs` with `FakeHost` (+ a fake game that writes dummy `take*/video.mp4` into a tempdir) and `FakeRunner` for ffprobe. | end-to-end record() in ms-scale timeouts: happy path N takes; never starts; console never opens; demo never loads; cancel mid-wait returns within ~TICK and `terminate()` was called exactly once; game crash in take 2; take file truncated ⇒ `TakeInvalid(2)`; `ReplayHost` over S2 transcript | `cargo test -p pipeline`, both OSes | 2 |
| 4 | **WinHost** (`win.rs`): Job Object, watcher thread, netcon with stop flag, probes. Per S1: direct job or assign-after-appear. | `cargo run -p host --example smoke`: launch real game with a tiny demo, observe events, kill; verify no stray cs2.exe, no leaked threads | manual smoke on the target machine (cannot be CI) | 1, S1 |
| 5 | **Preflight.** `preflight.rs`: Steam running, no cs2.exe, HLAE.exe + `x64\AfxHookSource2.dll`, cs2.exe path, `console_safe(out_dir)` (existing rule: no spaces), ffmpeg/ffprobe via `CommandRunner`, free disk (estimate from manifest `video_secs`), HLAE↔CS2 version check (see Research Flags). Run-all-report-all. | each check unit-tested with `FakeHost` returning `Err`/`Ok` combos, including "probe failed ⇒ check fails, not passes" | unit tests | 1 (parallel with 3/4) |
| 6 | **Assemble + StdRunner.** `host::StdRunner`, `pipeline::assemble`, delete `jobs::run_ffmpeg`; `takes::verify_take` real ffprobe path; manifest `record.json` written by `plan_player`. | ffmpeg hung ⇒ timeout; cancel kills child; existing dummy-takes + `assemble.bat` flow still validates the ffgraph | unit tests with `FakeRunner`; one real-ffmpeg integration test on dummy `testsrc` clips (the repo already generates them) | 3 |
| 7 | **Store v2 + checkpoints + job wiring.** Migration via `user_version`; `job_events`; `attempt/checkpoint/resume_from/phase/fail_*`; `recover_interrupted`; `requeue_from`; `JobReporter::event`; `jobs.rs` shrinks; UI: phase ladder, take N/M from typed event, «Повторить с записи». | `Store::in_memory` tests: attempt counter, checkpoint survives fail, interrupted keeps stage/phase, cascade delete, migration from a v0 DB created with the old SCHEMA; `run_job` tests with fakes: fail in record ⇒ retry from record skips analyze (counter on a fake analyzer) | `cargo test -p store -p pipeline` | 2 (can start before 3) |
| 8 | **Cut over and delete.** `app` uses `run_job` with `WinHost`/`StdRunner`; CLI uses the same; remove `launch::{game_alive,kill_game,wait_for_clips,Console,launch}` and the duplicated ffmpeg spawns; migrate `renderer::assemble` to `CommandRunner`. | `crates/cli/tests/pipeline.rs` still green; grep shows no `taskkill`/`tasklist` | CI | 4, 5, 6, 7 |
| 9 | **Acceptance run.** 5 distinct FACEIT matches/maps queued; tune the deadline table from measured numbers; record timings per phase from `job_events`. | the real criterion | manual, on target machine | 8 |

Ordering rationale: the two spikes are short and unblock the only parts with real uncertainty (process ownership and what the console actually says). Phases 1-3 produce the safety net before any Win32 code exists, so the risky `unsafe` work (4) lands under tests that already describe its contract. Phase 7 depends only on the failure taxonomy, so it can proceed in parallel with 3-6. Cutover (8) happens only when every piece has passed in isolation; the acceptance run (9) is strictly last because it is slow, needs the real machine, and its only job is to tune constants, not to discover design.

## Research Flags (what needs verification on the real machine)

| Item | Why uncertain | Resolve in |
|---|---|---|
| Is cs2.exe a descendant of the HLAE.exe process we spawn, and does it inherit our Job Object? | `-customLoader` internals not documented in what I could reach; web search returned nothing on these flags. The existing code comment says HLAE.exe exits after starting cs2, which suggests a handoff. | S1; fallback path designed (assign by PID) |
| Exact console text on `playdemo` failure and on cfg/campath errors | undocumented; only used as *hints* in the failure message, never as the sole detector (the detectors are our own markers + deadlines) | S2 |
| HLAE ↔ CS2 compatibility check source | HLAE has no obvious machine-readable "supported CS2 build". Realistic options: record `steam.inf` `ClientVersion` + `AfxHookSource2.dll` file version/mtime per successful job and warn when the CS2 build changed since the last success. Strict blocking is not feasible. | Phase 5 (decide minimal check), LOW confidence on any stronger check |
| Reasonable per-take deadline multiplier K and idle watchdog | depends on record fps vs game speed on this PC | Phase 9 tuning from `job_events` timings |
| ffprobe tolerance for "complete take" | HLAE finalises mp4 after `record end`; need real durations vs `game_secs` | Phase 6/9 |
| `windows-sys` current version | crates.io shows 0.61.2 (Oct 2025 metadata); `win32job` 2.0.3 (May 2025) | pin when adding the dependency |

## Sources

- Code read for this document (HIGH): `crates/app/src/launch.rs`, `crates/app/src/jobs.rs`, `crates/app/src/status.rs`, `crates/pipeline/src/lib.rs`, `crates/store/src/lib.rs`, `crates/renderer/src/lib.rs` (cfg generation, `TAKE_DONE`), `.planning/PROJECT.md`, `.planning/codebase/ARCHITECTURE.md`, `.planning/codebase/CONCERNS.md`, `.github/workflows/ci.yml`.
- `win32job` crate (Job Object wrapper; `limit_kill_on_job_close`, nested-job assignment caveat) — https://docs.rs/crate/win32job, https://github.com/ohadravid/win32job-rs (MEDIUM: summarised via search, API names not re-checked in source).
- Cargo's own job-object usage as a reference pattern — https://rust.googlesource.com/cargo/+/refs/heads/master/src/util/job.rs (MEDIUM).
- crates.io metadata for `windows-sys` (0.61.2) and `win32job` (2.0.3) — https://crates.io/crates/windows-sys, https://crates.io/crates/win32job (HIGH for version numbers at fetch time).
- Win32 `QueryInformationJobObject` / `JobObjectBasicProcessIdList` / `TerminateJobObject` semantics: from general Win32 knowledge, not re-fetched in this session (MEDIUM; verify during S1/Phase 4).
- HLAE command-line flags and CS2 `-netconport` behaviour: not verifiable from public sources in this session; taken from the working implementation in `launch.rs` (LOW as documentation, HIGH as "currently works on the author's machine").

---
*Architecture research for: automated CS2 demo-to-montage recording pipeline*
*Researched: 2026-10-10*
