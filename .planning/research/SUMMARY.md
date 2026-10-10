# Project Research Summary

**Project:** cs2cinema — milestone "Reliable recording"
**Domain:** Windows desktop automation of a game (CS2) through an injector (HLAE) to record and assemble video; single-user job queue in Rust
**Researched:** 2026-10-10
**Confidence:** MEDIUM-HIGH

## Executive Summary

cs2cinema already turns a CS2 demo into a montage; this milestone is about making the *record* stage reach `highlights.mp4` for 5 FACEIT matches in a row without manual intervention, or fail fast with a clear cause. Research across four dimensions (stack, features, architecture, pitfalls) agrees on the root problem: the current launcher (`crates/app/src/launch.rs`) has no ownership of the processes it starts, waits are unbounded or mtime-based, errors fail open (a `tasklist` failure means "alive"), and missing console markers are logged and ignored instead of ending the job. Comparable tools (cs-demo-manager's HLAE launcher) share many of these weaknesses, so there is no off-the-shelf pattern to copy; the fix is an explicit process-ownership and state-machine design.

The recommended approach keeps the existing stack (Rust, egui, `std::thread`, rusqlite, no tokio) and adds a small leaf crate `crates/host` that owns HLAE + CS2 + HLAE's ffmpeg inside a Windows Job Object (`windows-sys`, `KILL_ON_JOB_CLOSE`), feeds a single `crossbeam-channel` event bus (job events, netcon console, `console.log` tail, modal-dialog sniffer), and exposes a `GameHost`/`CommandRunner` trait with fakes. The record stage becomes a pure state machine in `pipeline::record` driven by bounded, cancellable waits; takes are confirmed by nonce-stamped console markers plus ffmpeg child exit plus `ffprobe`, never by directory mtime. Preflight runs first and reports every environment failure at once. Store schema v2 adds attempts, checkpoints and a takes ledger so a job can be retried from the record stage and recovered after an app crash.

Key risks are the HLAE/CS2 runtime behaviours nobody documents: whether `cs2.exe` inherits the job through HLAE's loader, whether `-afxFixNetCon` fixes the flaky console port, what the console prints when `playdemo` or a cfg fails, and whether an absolute `+playdemo` path works through HLAE's `-cmdLine`. These must be settled by short spikes on the author's machine before the host design is frozen; each spike has a named fallback (adopt-by-PID, console.log tailing, copy-then-delete demo).

## Key Findings

### Recommended Stack

Keep everything that exists. Add, under `cfg(windows)` in a new leaf crate, Win32 Job Objects and handle-based waits via `windows-sys` 0.61; a `crossbeam-channel` 0.5 event bus; `shared_child` 1.1 for one-shot ffmpeg/ffprobe runs with timeouts and cross-thread kill; `thiserror` 2.0 for the typed failure taxonomy; `which` for tool discovery; std `File::try_lock` (Rust ≥1.89) as the single-recorder lock. Launch line gains `-insecure` (mandatory for the hook), `-noConfig`, `-afxFixNetCon`, `-afxDisableSteamStorage`, `-condebug`. Console markers become anchored and nonce-stamped: `CS2C <nonce> READY|SCRIPT|TAKE_START n|TAKE_END n|ALL_DONE`.

**Core technologies:**
- `windows-sys` 0.61 (Job Objects, `WaitForSingleObject`, IOCP job notifications): owns the whole process tree, gives liveness, cancel and cleanup from one handle — replaces `tasklist` polling and `taskkill /IM`
- `crossbeam-channel` 0.5: one event bus with `recv_timeout` slices ≤100 ms so every wait is bounded and `Progress::cancelled()` is honoured
- `shared_child` 1.1 + a `run_with_deadline` helper: no more `Command::output()` that blocks cancellation
- `thiserror` 2.0: `RecordFailure` enum with stage, elapsed time, last console lines
- `ffprobe` (already required): validation of every take before assembly
- Do NOT use: `taskkill /IM`, `tasklist`, mtime clip discovery, `sysinfo`, `process-wrap`, `command-group`, `duct`, `tokio`, `mockall`

### Expected Features

FEATURES.md groups the milestone into categories with stable IDs: PRE (preflight), LCH (launch diagnostics), TMO (timeouts and cancellation), TAK (take discovery and validation), RET (retry and recovery), OBS (observability), TST (testability), ACC (acceptance).

**Must have (table stakes):**
- Preflight that runs *first* (before demo analysis) and reports all failures: Steam running, no foreign `cs2.exe` (refuse, never kill), HLAE and ffmpeg paths exist and run, HLAE's own `ffmpeg.ini` path valid, CS2 fully installed, ASCII output path, free disk per volume
- Typed failure taxonomy: `PreflightFailed`, `GameNotStarted`, `ConsoleNotOpen`, `DemoNotLoaded`, `ScriptNotExecuted`, `TakeTimeout`, `Stalled`, `TakeInvalid`, `GameExited{phase}`, `HlaeError`, `HlaeIncompatible`, `Interrupted`, `Cancelled`, `AssembleFailed` — each with stage, elapsed and last console lines; a missing marker is a failure, not "proceed anyway"
- Every wait has a deadline; liveness by handle; an error means *unknown*, never *alive*
- Cancel works within seconds in every state; staged teardown (graceful quit → job kill) with verification
- Expected take set derived from the plan; each take validated by marker + ffmpeg child exit + `ffprobe`
- Retry from the record stage without re-download/re-plan (`record.json` immutable plan, `clips/attempt.json` per attempt); recovery after app crash marks jobs `interrupted` with the stage reached, and kills orphaned game processes by persisted (pid, creation_time)
- Job log keeps evidence across retries (today `start_job` wipes it); full console transcript goes to a file; UI shows a phase ladder with the failed step
- Fake-driven tests for "does not start", "timeout", "cancel"; acceptance run of 5 matches, run twice without restarting the app

**Should have (competitive / single-user conveniences):**
- Soft HLAE-vs-CS2 compatibility warning (`steam.inf` PatchVersion vs HLAE changelog "Adjusted to CS2 update")
- Auto-retry capped at one, launch-class failures only, reported separately in acceptance
- Power guard (`SetThreadExecutionState` on the worker thread) for overnight queues
- Fail fast on first `AFXERROR` and on modal `Error - AfxHookSource2` dialogs

**Defer (v2+):**
- Generic plugin/host abstraction beyond HLAE, multi-machine render farm, non-Windows hosts, streaming demo parsing, i18n of messages

### Architecture Approach

Record and assemble move out of `crates/app/src/jobs.rs` and `launch.rs` into `pipeline::record` behind the existing `Progress` trait, shared by app and CLI. External processes sit behind traits in a new leaf crate `crates/host` (`GameHost`, `GameSession`, `CommandRunner`) with `WinHost`/`StdRunner` real implementations and in-memory fakes, so pipeline and CLI tests can drive failure scenarios without CS2. The record stage is a pure `Machine` (`step(now, Input) -> Vec<Action>`) with explicit states Preflight → Launch → ConsoleOpen → DemoLoaded → ScriptRun → Take(n) → AllDone and explicit failure states, driven by one loop that polls the event bus and `Progress::cancelled()` on a 100 ms tick. `Progress` gains a typed `event()` default method so the UI stops parsing log strings; `store` stays free of pipeline types. The DB is the index and the disk is the truth: a checkpoint is trusted only if its artifacts validate.

**Major components:**
1. `crates/host` — process ownership (Job Object), console reader (netcon + `console.log` tail), dialog sniffer, bounded command runner; traits + fakes; non-Windows stub for CI
2. `pipeline::record` — failure taxonomy, marker parser, pure state machine, driver loop, take manifest and verification, preflight
3. `renderer` markers — `CS2C_SCRIPT`, `CS2C_TAKE_START/END` nonce-stamped lines in generated cfgs; assemble behind `CommandRunner`, write `.part` then rename
4. `store` v2 — `attempt`, `checkpoint`, `phase`, `fail_*` columns, `job_events` table, takes ledger; `interrupted` state
5. `app` — thin: enqueue, show phase ladder and typed failures, retry button; `cli` reuses the same `pipeline::record`

### Critical Pitfalls

1. **Fail-open liveness and unbounded waits** (`game_alive` returns true on error; `wait_for_clips` has no overall deadline; `Console::connect` costs up to 150 s on a dead game) — replace with handle waits, per-phase deadline table, "error = unknown"
2. **Killing by image name** (`taskkill /IM cs2.exe`, also what cs-demo-manager does) — own the tree with a Job Object; preflight refuses if a foreign CS2 is running
3. **Silent HLAE failures** — `AFXERROR ... CreateProcessW` then hundreds of `Failed writing image` with markers still arriving gives empty takes; modal dialogs block forever — fail fast on first `AFXERROR`, sniff `#32770` dialogs owned by job PIDs, preflight HLAE's ffmpeg
4. **Trusting mtime / "Finishing recording ... done"** — HLAE queues capture shutdown on the render thread; completion = marker + child exit + `ffprobe`; the "already recorded" shortcut must validate, not just check file existence
5. **Retry that re-sends `exec highlights` on a timer** — duplicates takes and shifts numbering; send once, wait on markers with deadlines
6. **Crash recovery that marks jobs `failed`** — loses the stage and leaves an orphaned CS2 that blocks every later preflight; persist (pid, creation_time) at spawn, mark `interrupted`, kill orphans at startup
7. **Blocking `Command::output()`** — cannot be cancelled; route every external call through `run_with_deadline`

## Implications for Roadmap

Granularity is *coarse* (3-5 phases). The build order below merges the four files' suggestions into phases that are each testable alone, with the acceptance run last.

### Phase 1: Spikes and seams
**Rationale:** Three runtime facts (job inheritance through HLAE, `-afxFixNetCon`, console transcripts of real failures, absolute `+playdemo`) decide the host design; the seams (traits, fakes, typed failures, markers) make everything after testable.
**Delivers:** Spike notes with decisions and fallbacks; `crates/host` traits + fakes; `pipeline::failure` taxonomy; `Progress::event`; renderer nonce markers; launch-line flags; `run_with_deadline`.
**Addresses:** TST-1/3, LCH markers, OBS typed events
**Avoids:** Building the Job Object path on an unverified assumption; string-parsing of logs in the UI

### Phase 2: Record state machine and Windows host
**Rationale:** With seams in place, the pure machine and its fake-driven tests can be written before the Win32 code; WinHost then slots in.
**Delivers:** `pipeline::record` machine + driver with per-phase deadlines and cancel in every state; take manifest and `ffprobe` verification; `WinHost` (Job Object, IOCP watcher, console reader, dialog sniffer, AFXERROR fast-fail); fake-game test binary for CI.
**Uses:** `windows-sys`, `crossbeam-channel`, `shared_child`, `thiserror`
**Implements:** Components 1 and 2

### Phase 3: Preflight, assemble, store v2 and retry
**Rationale:** Independent of the host internals; can run in parallel with Phase 2 work and lands the user-visible reliability features.
**Delivers:** Preflight that runs first and reports all failures; assemble behind `CommandRunner` with `.part` rename; store v2 (attempts, checkpoints, job_events, takes ledger, `interrupted`); retry from record; orphan kill at startup; job log kept across retries; phase ladder in the job card.
**Addresses:** PRE-*, RET-*, OBS-*

### Phase 4: Cutover and acceptance
**Rationale:** Only after app and CLI run on the new path can old `tasklist`/`taskkill` code be deleted and the 5-match target measured.
**Delivers:** App and CLI on `pipeline::record`; `launch.rs` internals removed; power guard; soft HLAE compatibility warning; fault drills (kill CS2 mid-take, pull ffmpeg from PATH, foreign CS2 running); 5 FACEIT matches to mp4, then 5 more without restart; tuned deadlines and `ffprobe` tolerance.

### Phase Ordering Rationale

- Spikes first because LOW-confidence HLAE/CS2 behaviours gate the host design; each has a fallback so the phase cannot block.
- Seams before machine so the machine is unit-tested with fakes and no clock; WinHost is then a thin adapter.
- Preflight/store/retry are decoupled from Win32 and parallelisable (parallelization is enabled in config).
- Cutover last so app and CLI never diverge again and dead code is removed in one step.
- Acceptance last and twice (with and without restart) because recovery is part of the target.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1:** HLAE loader behaviour, netcon bind/password, Steam registry `ActiveUser` and `appmanifest_730.acf` `StateFlags` semantics — verify on the machine
- **Phase 2:** Job Object + IOCP details in `windows-sys` 0.61 (function names confirmed, call sequence not yet exercised)

Phases with standard patterns (skip research-phase):
- **Phase 3:** SQLite migrations, preflight checks, file rename — well-documented
- **Phase 4:** mechanical cutover and manual acceptance

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH (versions), MEDIUM (fit) | crates.io on 2026-10-10; several crates already in `Cargo.lock` |
| Features | MEDIUM-HIGH | cs-demo-manager source, OBS/Jenkins/GitHub Actions primary docs; HLAE internals LOW |
| Architecture | HIGH (structure), MEDIUM (Win32), LOW (HLAE runtime) | Derived from the actual code; Win32 calls not yet exercised |
| Pitfalls | HIGH (code findings, HLAE releases, Win32) | CS2 internals MEDIUM-LOW |

**Overall confidence:** MEDIUM-HIGH

### Gaps to Address

- Job inheritance through HLAE's loader: spike S1; fallback adopt-by-PID after preflight guarantees no other `cs2.exe`
- Whether `-afxFixNetCon` fixes this machine's console flakiness: spike with it on/off; fallback `console.log` tail as the primary channel
- Exact console text on `playdemo`/cfg/hook failures: capture transcripts in spike S2 before freezing the marker parser
- Absolute `+playdemo` through `-cmdLine` and quoting: spike; fallback copy-then-delete at teardown (today six copies, ~1.9 GB, are never deleted)
- Deadline values (console 90 s, take idle 60 s as starting points) and `ffprobe` duration tolerance: tune in acceptance
- HLAE version detection: only a soft warning is feasible; the hard check is the hook banner + no `AFXERROR`
- FACEIT download (`ureq`) has no cancel hook: accept for this milestone, cancel takes effect at the next stage boundary
- `SetThreadExecutionState` is per thread and does not stop the screen saver: call it on the worker thread; lock screen effects on a windowed game unverified

## Sources

### Primary (HIGH confidence)
- HLAE source (`hlae/Loader.cs`, AfxHookSource2 changelog, releases 2.192.3–2.192.7) — launch flags, `-afxFixNetCon`, job breakaway, dialog titles
- cs-demo-manager source — HLAE launcher, generate-video, kill/abort, take discovery
- Microsoft docs — Job Objects, `IsProcessInJob`, `SetThreadExecutionState`, `tasklist`
- docs.rs — `windows-sys` 0.61.2, `crossbeam-channel` 0.5.17, `shared_child` 1.1.2, `thiserror` 2.0.21, `std::fs::File::try_lock`
- Local machine — `console.log` (AFXERROR storm), HLAE install, `steam.inf`, Steam files
- This repo — `launch.rs`, `jobs.rs`, `pipeline`, `store`, `ui.rs`, `.planning/codebase/CONCERNS.md`

### Secondary (MEDIUM confidence)
- `steamlocate` crate docs — Steam app discovery fields
- OBS obs-websocket, Jenkins "Restart from Stage", GitHub Actions cancel ladder — patterns for completion events, retry, cancellation

### Tertiary (LOW confidence)
- Community claims about `-conclearlog`, Steam registry `ActiveUser`, `appmanifest_730.acf` `StateFlags` — validate on the machine
- Valve issue #3603 (`-netconport` on Windows) — context for `-afxFixNetCon`

---
*Research completed: 2026-10-10*
*Ready for roadmap: yes*
