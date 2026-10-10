# Phase 4: Очередь без присмотра и приёмка - Research

**Researched:** 2026-10-11
**Domain:** Rust workspace refactor (app -> pipeline), unattended job-queue policy, Windows tray notification, acceptance tooling (SQLite + ffprobe)
**Confidence:** HIGH on code facts (everything below that is tagged VERIFIED was read this session); MEDIUM on Windows UI behaviour (hidden-window repaint, balloon rendering) - flagged as ASSUMED and covered by spikes/drills

<user_constraints>
## User Constraints

**There is NO `04-CONTEXT.md`.** The author chose to plan Phase 4 without `/gsd-discuss-phase`. Therefore:

### Locked Decisions
None for Phase 4 itself. Binding decisions inherited from earlier phases (they constrain this phase exactly like locked decisions):

- Phase 1 D-01: `Host` and its implementations live in leaf crate `crates/host`; **`windows-sys` is used only in `crates/host`**; `app` and `cli` reach it through `pipeline::host`. (01-CONTEXT.md)
- Phase 1 D-05: failures are typed `RecordFailure { code, state, message, hint, elapsed, console_tail }`; DB gets strings; "future auto-retry (DIF-03/04) reads the code without parsing strings".
- Phase 1 D-10: `exec highlights` is sent exactly once; no timer re-send.
- Phase 2 D-13: "После отмены очередь ждёт, пока пользователь не нажмёт «Продолжить». После сбоя очередь продолжает работу сама."
- Phase 2 D-14: next job waits for the previous own `cs2.exe` by handle + 3 s settle; stuck own game -> `PREV_GAME_STUCK`. Foreign processes are never touched (PRE-05, TMO-05).
- Phase 3 D-16: interrupted jobs are not resumed automatically; queue stands until "Продолжить".
- Phase 3 override (accepted): "Повторить" with any non-validated take re-records **all** takes (partial resume is DIF-05, v2).
- Project: Rust workspace, `std::thread` (no tokio), rusqlite, Windows-only recording, one recording job at a time, Russian user strings, `cs2-cli` must keep building and `crates/cli/tests/pipeline.rs` must pass.

### Claude's Discretion
Every open design choice of Phase 4 (listed as DC-1..DC-12 in "Design Choices Needing User Confirmation" below). Each has a recommendation; **none is confirmed by the user yet**.

### Deferred Ideas (OUT OF SCOPE)
From REQUIREMENTS.md v2 / Out of Scope: DIF-05..DIF-11 (partial re-record, progress-based timeouts, adaptive timeouts, screenshot on failure, "copy diagnostics", pause-after-current, attempt history UI); Linux/macOS recording; automatic killing of a foreign `cs2.exe`; tokio; cancelling a FACEIT download mid-transfer.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TST-03 | Record + assemble stages belong to `pipeline` behind `Progress`; app and `cs2-cli` call the same code | Q1 "Move map": whole job orchestration (`jobs.rs::run`, `resume_job`, `record_takes`, `assemble_stage`, `precheck_stage`, `run_ffmpeg`) and `launch.rs` install/cleanup/recover/nvenc move to `pipeline`; CLI gets `record`/`retry` subcommands calling `pipeline::run_job` |
| RET-05 | One failed job never stops the queue; queue ends by itself | Already true for typed failures (`app.rs` JobDone handler calls `run_next`); **two holes found**: job-thread panic wedges the queue, and queue advancement runs on the UI thread which does not repaint while the window is hidden to tray |
| DIF-04 | One automatic retry for "launch" class after full cleanup + pause, logged as "авто-повтор" | `classify(code, state)` table (DC-3), `requeue_auto`, `StartKind::Auto`, `jobs.auto_retries` column (DC-4) |
| DIF-03 | Pause queue after 2 consecutive same-code "environment" failures with explanation | Pure `QueuePolicy` + `PauseReason` with payload + banner (DC-5) |
| OBS-04 | Tray notification at queue end: "N готово / M упало" + first failure reason | `tray-icon` 0.19.3 has **no** balloon API; use `Shell_NotifyIconW(NIF_INFO)` inside `crates/host` behind `Host::notify` (DC-6) |
| ACC-01 | Fixed set of 5 FACEIT links on 5 maps, enqueued in one action | `acceptance/matches.txt` (repo, embedded) + "Поставить приёмочный набор" button + `jobs.batch` (DC-7) |
| ACC-02 | Machine check: 5 Done, ffprobe video+audio+duration vs plan, no `cs2.exe`, no manual actions, auto-retries counted separately; two series | `cs2-cli acceptance check` reading SQLite read-only + reusing `record::verify` (DC-8, DC-9) |
| ACC-03 | Manual drills once before acceptance | Drill table with expected code and machine-checkable clean state (DC-10) |
</phase_requirements>

## Project Constraints (from CLAUDE.md)

Extracted from `./.claude/CLAUDE.md` (project instructions):

- Stack is fixed: Rust workspace, egui/eframe, `std::thread` without tokio, SQLite via rusqlite. Embed into existing crates.
- Windows only for HLAE/CS2; **one recording job at a time**.
- External tools (HLAE, CS2) have undocumented console markers: **every detector needs a timeout and diagnostics**.
- `cs2-cli` must keep building and pass `crates/cli/tests/pipeline.rs`.
- User-facing strings and doc comments in **Russian**; identifiers in English.
- Errors: `Result<T, String>` at crate boundaries (pipeline/store); cancellation is the sentinel `pipeline::CANCELLED`; no `anyhow`/`thiserror`.
- Long steps report through `Progress::stage/log` and call `check(p)?` at boundaries.
- Format: rustfmt defaults with wide lines; per STATE.md decision 01-07, app files are formatted only with rustfmt `max_width=160,use_small_heuristics=Max`, not `cargo fmt -p app`. Run `cargo clippy --workspace` locally.
- GSD workflow: edits only through a GSD command.
- Note: CLAUDE.md STACK/ARCHITECTURE sections still claim `tasklist`/`taskkill` live in `launch.rs` - stale (see Q1).

## Summary

Phase 4 is mostly **integration and policy**, but research found four facts that change the plan shape:

1. **The CLI does not record at all today, and does not even depend on `pipeline`.** `cs2-cli` plans with its own inline copy of the analysis code, copies scripts into the game folder, prints instructions, and offers a legacy `assemble` subcommand built on `renderer::assemble` (old `shot_NN` layout, `-an`, `Command::output()` without deadline). So TST-03 is not "swap a call": the record/assemble orchestration that lives in `crates/app/src/jobs.rs` (about 700 non-test lines) and `launch.rs` (about 340 non-test lines) must move into `pipeline` behind a store-free configuration, and the CLI must gain a subcommand that calls it. `tasklist`/`taskkill` are **already gone from all code** (only docs mention them), so success criterion 1's deletion part is satisfied except for stale docs and the remnants of `launch.rs`.
2. **The queue is driven from the UI thread and the app hides the window to tray by default** (`minimize_to_tray: true` in the author's `settings.json`). Queue advancement (`Msg::JobDone` -> `run_next`) happens inside `App::update`. winit documents that a Windows window "has to be visible to receive WM_PAINT", and eframe repaints via `window.request_redraw()`, so a hidden window very likely never runs `update()` - the queue would stall after the current job until the user reopens the window. There is also no `catch_unwind`: a panic in the job thread leaves `jobs.running` set forever. For an *unattended* queue (and for OBS-04, which exists precisely for when the window is hidden) queue advancement, auto-retry timing and the final notification must move to a dedicated worker thread.
3. **`tray-icon` 0.19.3 cannot show a balloon/toast** (it only uses `NIF_ICON | NIF_MESSAGE | NIF_TIP`). The native route is `Shell_NotifyIconW` with `NIF_INFO`, which exists in `windows-sys` 0.61.2 (feature `Win32_UI_Shell`) and must live in `crates/host` (Phase 1 D-01) as `Host::notify`, which also makes it testable on `FakeHost`.
4. **Acceptance is cheap if the DB is the source of truth.** All needed facts are already persisted (`status`, `result`, `out_dir`, `attempt`, journal separators, `record.json.video_secs`, `plan.json.map`). Add two additive columns (`auto_retries`, `batch`), a `StartKind::Auto` journal kind, a read-only store open, and a `cs2-cli acceptance check` subcommand that reuses `record::verify` parsing and `Host::run` for ffprobe.

**Primary recommendation:** Execute in this order - (a) move orchestration into `pipeline::job` + CLI subcommands, (b) add `FailClass` + store/journal additions, (c) replace UI-driven `run_next` with a queue worker thread that owns `QueuePolicy` (auto-retry, env pause, panic guard, end-of-queue summary + `Host::notify`), (d) acceptance enqueue + `acceptance check`, (e) manual drills (ACC-03) with machine-checkable evidence, (f) two acceptance series, then tune deadlines/tolerance from the evidence.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Job orchestration (precheck -> analyze -> plan -> record -> assemble, resume) | `pipeline` (new `job` module) | `app`, `cli` as thin callers | TST-03: single implementation behind `Progress`; no Settings/Store types inside |
| Game-folder install/cleanup/orphan recovery | `pipeline` (new module) | `app` glue for `Store` (`recover_after_crash`) | pure filesystem + `Host`; reused by CLI |
| Failure classification (launch / environment / other) | `pipeline::record::failure` (next to `FailCode`) | - | classification must change together with codes; store only holds strings |
| Queue policy (auto-retry, env streak, pause reason) | `app` (new `queue.rs`, pure struct) | `store` columns | policy is queue-level, not recording-level; pure => unit-testable |
| Queue driver (advance, wait, retry pause, panic guard) | `app` worker thread | `store` | must not depend on UI repaint (hidden window) |
| Native balloon notification | `host` (`Host::notify`, `win/notify.rs`) | `app` builds text | Phase 1 D-01 keeps Win32 in `host`; fake records calls |
| In-app banners (env pause, queue summary) | `app` UI | - | persistent fallback if balloon is suppressed |
| Acceptance set (list of links) | repo file, embedded via `include_str!` | env override path | version-controlled, reproducible |
| Acceptance enqueue | `app` (button) | `faceit` client, `store` | needs FACEIT key + nickname from settings |
| Acceptance verification | `cli` (`acceptance check`) | `store` read-only, `pipeline::record::verify`, `Host::run` | independent of the running app; runs after each series |
| Manual drills | human checkpoint plan | `cs2-cli acceptance state` for clean-state evidence | fakes cannot prove HLAE/CS2 behaviour |

## Standard Stack

### Core
No new external crates are required. Feature/dependency edits only:

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `windows-sys` | 0.61.2 (already in `Cargo.lock`) | add feature `Win32_UI_Shell` to `crates/host` for `Shell_NotifyIconW` | already the only Win32 binding in the workspace [VERIFIED: crates/host/Cargo.toml:9] |
| `rusqlite` | 0.32.1 (workspace) | `Connection::open_with_flags(.., OpenFlags::SQLITE_OPEN_READ_ONLY)` for the checker | [VERIFIED: ~/.cargo/registry rusqlite-0.32.1/src/lib.rs:472,1171] |
| `store` / `pipeline` (workspace) | - | new `cli` dependencies | `cli` currently depends on neither [VERIFIED: crates/cli/Cargo.toml] |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tray-icon` | 0.19.3 (existing) | tray icon, menu, click events; `set_tooltip` | unchanged; **no balloon API** [VERIFIED: grep of tray-icon-0.19.3/src: only `NIF_ICON, NIF_MESSAGE, NIF_TIP`, no `NIF_INFO`] |
| `std::panic::catch_unwind` | std | keep the queue worker alive when a job panics | wrap the call to the moved `run_job` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `Shell_NotifyIconW` in `host` | PowerShell WinRT toast via `Host::run` | no new FFI, but needs an AUMID trick and **untrusted failure text would be passed to a shell** (injection risk); rejected |
| `Shell_NotifyIconW` in `host` | `notify-rust` / `winrt-notification` crate | new dependency + violates "windows-sys only in host" spirit; unnecessary |
| Worker thread for the queue | keep UI-driven `run_next` + repaint timer | cannot work if hidden window never repaints; also couples OBS-04 to repaint |

**Installation:** none (edit `crates/host/Cargo.toml` features, `crates/cli/Cargo.toml` dependencies).

**Version verification:** `windows-sys` 0.61.2 present in registry [VERIFIED: ~/.cargo/registry/src/*/windows-sys-0.61.2]; `Win32_UI_Shell = ["Win32_UI"]` in its Cargo.toml [VERIFIED: windows-sys-0.61.2/Cargo.toml:272]. Toolchain on this machine: cargo 1.99.0 / rustc 1.99.0 [VERIFIED: `cargo --version`].

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| windows-sys | crates.io | since 2021-10 | 35M/wk | github.com/microsoft/windows-rs | OK | Approved (already a dependency; only a feature is added) |
| rusqlite | crates.io | since 2014-11 | 3.3M/wk | github.com/rusqlite/rusqlite | OK | Approved (already a dependency; now also used from `cli` via `store`) |

**Packages removed due to SLOP verdict:** none
**Packages flagged SUS:** none
[VERIFIED: `gsd-tools query package-legitimacy check --ecosystem crates windows-sys rusqlite` returned `OK` for both]

## Architecture Patterns

### System Architecture Diagram

```
                        (UI thread, egui; may be hidden to tray)
  Jobs page / Match page ──► store.add_job / requeue ──► Cmd::Wake ──┐
  "Поставить приёмочный набор" ──► FACEIT stats+player ──► add_job×5 ┤
  "Продолжить" / "Отменить" ──► Cmd::Resume / CancelToken ───────────┤
                                                                     ▼
                   ┌──────────────── queue worker thread (new) ───────────────┐
                   │ loop:                                                    │
                   │   paused? / retry_at not reached? ──► wait (≤500 ms tick)│
                   │   job = store.next_queued()                              │
                   │   settings = Settings::load()   (applied copy on disk)   │
                   │   catch_unwind( pipeline::run_job(host, spec, reporter) )│
                   │   finish(store) ; journal outcome line                   │
                   │   verdict = QueuePolicy::after(job, outcome)             │
                   │     Next ────────────► loop                              │
                   │     AutoRetry ───────► journal "авто-повтор", requeue_auto,│
                   │                        retry_at = now + pause ──► loop    │
                   │     Pause(reason) ───► shared.paused = reason; notify     │
                   │   queue idle? ───────► Summary ──► Host::notify (balloon) │
                   │                        + Msg::QueueSummary to UI          │
                   └───────────────┬──────────────────────────────────────────┘
                                   │ Msg (queued while window hidden; read on show)
                                   ▼
                           UI: status bar, banners, cards

  pipeline::run_job (shared by app worker and `cs2-cli record|retry`)
   precheck ─► [analyze ─► plan ─► record.json] ─► space pass ─► record_takes
      │            │ (download via faceit)             │  install_files ─► pipeline::record (machine, ffprobe per take)
      │            └─► Progress::artifact(Demo)        │  cleanup_installed (always)
      └─► Progress::artifact(Precheck json)            ▼
                                              assemble_stage (manifest-validated takes, highlights.part.mp4 ─► rename)

  Acceptance (separate process, after each series):
   cs2-cli acceptance check ─► Store::open_read_only(db) ─► jobs of batch
        ├─ per job: status Done, result exists, record.json.video_secs, plan.json.map
        ├─ ffprobe highlights.mp4 via Host::run + record::verify::parse_probe  (video+audio, duration in tolerance)
        ├─ attempts from journal separators: first / auto-retry / manual
        └─ Host::processes(): no cs2.exe / HLAE.exe
```

### Recommended Project Structure
```
crates/pipeline/src/
├── job.rs            # NEW  JobConfig, JobSpec, Outcome, RunError, run_job (precheck→…→assemble), resume path
├── assemble.rs       # NEW  assemble_stage, run_ffmpeg, part_args, check_take_for_assembly (moved from app/jobs.rs)
├── gamedir.rs        # NEW  install_files, cleanup_installed, is_ours, Installed, note_take_done, recover, kill_orphans (moved from app/launch.rs)
├── record/failure.rs # + FailClass, classify(code, state)
├── precheck/…        # + nvenc_check / has_nvenc / hlae_ffmpeg (moved from launch.rs)
└── lib.rs            # + Progress::artifact(Artifact) default no-op
crates/host/src/
├── lib.rs            # + Host::notify(title, text) default no-op
├── win/notify.rs     # NEW  Shell_NotifyIconW(NIF_INFO) on a message-only window, own thread
└── fake.rs           # + notifications() recorder; + per-launch scenario sequence
crates/store/src/     # migration 5: auto_retries, batch; requeue_auto; set_batch; open_read_only; journal::StartKind::Auto
crates/app/src/
├── queue.rs          # NEW  QueuePolicy (pure), PauseReason{payload}, Summary, worker loop
├── jobs.rs           # shrinks to JobReporter, finish(), job_failure(), CancelToken, Settings→JobConfig glue
└── (launch.rs deleted: find_cs2/find_hlae → settings.rs or discover.rs, recover_after_crash → jobs.rs, old_logs → jobs_page.rs, no_window → ui.rs)
crates/cli/src/
├── main.rs           # + `record`, `retry`, `acceptance` dispatch; legacy `assemble` fate per DC-1
├── record.rs         # NEW  ConsoleProgress, arg parsing → JobSpec
└── acceptance.rs     # NEW  set parsing, evaluate(), report, `state` check
acceptance/matches.txt # NEW  one FACEIT link per line, '#' comments (author fills 5 links)
```

### Pattern 1: Store-free orchestration behind `Progress`
**What:** `pipeline::run_job(host, &JobSpec, &dyn Progress) -> Result<Outcome, RunError>` replaces `app::jobs::run`. Everything that used `Store` or `Settings` becomes an input or an output:

| Was (app) | Becomes |
|-----------|---------|
| `store.last_good()` | `JobSpec.last_good: Option<(String,String)>` |
| `store.demo_path(id)` | `JobSpec.known_demo: Option<PathBuf>` |
| `store.remember_demo`, `set_out_dir`, `set_precheck`, `set_last_good` | `Progress::artifact(Artifact::{Demo{match_id,path}, OutDir(PathBuf), Precheck(String /*json*/), LastGood{cs2_build,hlae_version}})` (default no-op) |
| `Settings` fields (`hlae_dir`, `cs2_dir`, `out_root`, `fps`, `chronological`, `flybys`, `walls`, `detailed`, `gpu_encode`, `resolution`→frame, `time_margin`, `afxerror_ignore`, `faceit_key`) | `JobConfig` (plain struct, `Default`) |
| `JobKind::Plan` vs `Record` | `JobSpec.record: bool` |
| `job.resume`, `job.attempt` | `JobSpec.resume`, `JobSpec.attempt` |
| `JobReporter.event` calling `launch::note_take_done` | small decorator `Progress` inside `record_takes` that calls `gamedir::note_take_done` on `Event::TakeDone` |
| `RunError`, `JobOutcome` | `pipeline::job::{RunError, Outcome}`; app converts `Outcome`→`store::JobOutcome` |

**When to use:** always; the app keeps only adapters (`JobReporter`, `finish`, `job_failure`).
**Example (shape, not final code):**
```rust
// Source: derived from crates/app/src/jobs.rs run()/resume_job() read this session
pub struct JobSpec {
    pub source: Source,              // File | Faceit{link,key,demo_index,dir}
    pub hero_steam: u64, pub hero_name: String,
    pub record: bool, pub resume: bool, pub attempt: u32, pub job_id: i64,
    pub known_demo: Option<PathBuf>, pub last_good: Option<(String, String)>,
    pub out_dir_hint: Option<PathBuf>,   // resume: job.out_dir
    pub cfg: JobConfig,
}
pub fn run_job(host: &dyn Host, spec: &JobSpec, p: &dyn Progress) -> Result<Outcome, RunError>;
```

### Pattern 2: Pure queue policy, thin worker
**What:** `QueuePolicy::after(&mut self, job: &JobFacts, outcome: &Outcome2) -> Verdict` has no I/O and no clocks; the worker owns sleeping, store writes and notification. Tests drive the policy with plain values and the worker with `FakeHost` + `Store::in_memory` + zero pause.
```rust
// Source: design; FailCode variants verified at crates/pipeline/src/record/failure.rs:16-36
pub enum FailClass { Launch, Environment, Other }
pub fn classify(code: FailCode, state: RecordState) -> FailClass {
    use FailCode::*;
    match code {
        GameNotStarted | ConsoleNotOpen | DemoNotLoaded => FailClass::Launch,
        // "игра умерла рано": only before the first TAKE_START; a mid-take death is NOT retried
        GameExited if state < RecordState::Take => FailClass::Launch,
        PrecheckFailed | ForeignGameRunning | PrevGameStuck | HlaeError | LauncherFailed => FailClass::Environment,
        ScriptNotRun | TakeTimeout | GameExited | TakeInvalid | RetryNoData | Cancelled => FailClass::Other,
    }
}
```
`RecordState` derives `PartialOrd, Ord` in declaration order `Precheck, Launcher, Game, Console, Demo, Script, Take, Closing, Done` [VERIFIED: crates/pipeline/src/record/machine.rs:25-37], so `state < Take` is valid.

### Pattern 3: Off-UI queue worker with shared view
**What:** one `std::thread` started in `App::new`, command channel `Cmd::{Wake, Resume}`, `Arc<Mutex<QueueView>>` with `{ running: Option<(i64, CancelToken)>, paused: Option<PauseInfo>, retry_at: Option<(i64, Instant)> }` read by the UI each frame. Settings for the next job come from `Settings::load()` (the applied copy on disk; draft values are not applied until saved). Cancel stays token-based (UI sets the flag directly), so a blocked worker still cancels.
**Anti-pattern avoided:** `JobsState.running/paused` mutated from `poll()`; those ~20 references (`app.rs`, `jobs_page.rs`, `settings_page.rs`) switch to the shared view.

### Pattern 4: Acceptance by reading the DB
`Store::open_read_only(path)` (no `PRAGMA journal_mode=WAL` write, no migrations; error when `user_version` < required) so a checker never mutates or upgrades the author's live DB while the app is open.

### Anti-Patterns to Avoid
- **Queue advancement inside `App::update`/`poll`** for an unattended queue (hidden window, panics).
- **Passing failure text to a shell** (PowerShell toast): failure messages embed console fragments from the game (untrusted).
- **Judging "no manual actions" from UI events:** derive it from persisted state (journal separators, statuses).
- **Reusing one out folder across series without checking between them:** `out_dir = out_root/console_name(label_hero)` is deterministic per (match, hero) [VERIFIED: crates/app/src/jobs.rs:362], so series 2 overwrites series 1's `highlights.mp4` (old takes go to `clips_old_<ts>`). Run `acceptance check` for series 1 **before** enqueuing series 2.

## Q1 - TST-03: what lives where today, and the minimal move

### Facts [all VERIFIED this session by Read]
- `crates/cli/Cargo.toml` dependencies: `demo, cs2, highlights, director, renderer, faceit, positions, geometry, serde_json` - **no `pipeline`, no `store`, no `host`**.
- `cs2-cli` main path (`crates/cli/src/main.rs:143-338`): inline download -> `demo::parse` -> `highlights` -> `director::direct*` -> `renderer::render` -> writes scripts into `<CS2>/game/csgo/cfg` -> **prints** "в консоли CS2: demo_ui_mode 0; playdemo …" - no launch, no record, no assemble.
- `cs2-cli assemble` (`main.rs:402-430`) -> `renderer::assemble::assemble` which expects `clips/shot_NN/takeMMMM` image sequences or video, builds a `-an` (no audio) graph, and calls `Command::output()`/`.status()` without deadline (`renderer/src/assemble.rs:121,207`). The current app layout is `clips/takeNNNN/{video.mp4,audio.wav}` + manifest, so this subcommand is **stale**.
- `tasklist`/`taskkill`: grep over `crates/` returns **nothing**. Remaining mentions are documentation only: `.claude/CLAUDE.md:84,320`, `.planning/codebase/{ARCHITECTURE,CONCERNS,INTEGRATIONS,STACK,STRUCTURE}.md`.
- `crates/app/src/launch.rs` (642 lines, 1-359 non-test) now contains only: `no_window`, `find_cs2`, `find_hlae`, `cs2_exe`, `Installed`/`install_files`/`is_ours`/`cleanup_installed`/`note_take_done`, `kill_orphans`, `finished_takes`/`recover`/`recover_after_crash`, `old_logs`, `hlae_ffmpeg`/`has_nvenc`/`nvenc_check`.
- `crates/app/src/jobs.rs` (1370 lines, 1-750 non-test): `CancelToken`, `RunError`, `JobReporter`, `job_failure`, `outcome_text`, `finish`, `record_request`, `record_stage`, `needs_precheck`, `precheck_inputs`, `job_demo`, `precheck_stage`, `space_pass`, `pauses_queue`, `run_next`, `run`, `remember_last_good`, `begin_plan`, `record_json_for`, `set_aside_clips`, `record_takes`, `retry_no_data`, `all_takes_validated`, `resume_job`, `part_args`, `check_take_for_assembly`, `assemble_stage`, `take_invalid_failure`, `plan_hash`, `deadlines`, `ffmpeg_error`, `run_ffmpeg`.
- `RecordRequest.verify` is still `Option<Verify>` ("`None` — без проверки, как раньше (старые тесты и CLI до фазы 4)") [VERIFIED: crates/pipeline/src/record/mod.rs:45-47]; only 1 test helper (`tests/record.rs`) and 1 app test pass `verify: None`.

### Move map
| Symbol(s) | From | To | Notes |
|-----------|------|----|-------|
| `run`, `resume_job`, `record_takes`, `space_pass`, `precheck_stage`, `precheck_inputs`, `job_demo`, `record_request`, `record_stage`, `begin_plan`, `record_json_for`, `set_aside_clips`, `retry_no_data`, `all_takes_validated`, `remember_last_good`, `plan_hash`, `deadlines`, `RunError` | `app/jobs.rs` | `pipeline::job` | Settings/Store replaced per Pattern 1 |
| `assemble_stage`, `run_ffmpeg`, `part_args`, `check_take_for_assembly`, `take_invalid_failure`, `ffmpeg_error`, `PART_NAME` | `app/jobs.rs` | `pipeline::assemble` | pure `Host` + filesystem |
| `Installed`, `install_files`, `is_ours`, `cleanup_installed`, `remove_with_retry`, `note_take_done`, `kill_orphans`, `finished_takes`, `recover` | `app/launch.rs` | `pipeline::gamedir` | `recover_after_crash` stays in app (uses `Store`) as thin glue |
| `nvenc_check`, `has_nvenc`, `hlae_ffmpeg` | `app/launch.rs` | `pipeline::precheck` (already hosts `hlae_ffmpeg` logic) | |
| `CancelToken`, `JobReporter`, `job_failure`, `outcome_text`, `finish` | `app/jobs.rs` | stay | UI/Store adapters |
| `find_cs2`, `find_hlae`, `cs2_exe` | `app/launch.rs` | recommend `pipeline::discover` (lets CLI default its paths) - discretion | CLI today has its own `cs2_csgo_dir` |
| `no_window`, `old_logs` | `app/launch.rs` | `ui.rs`, `jobs_page.rs` | then delete `launch.rs` |
| `pauses_queue`, `run_next` | `app/jobs.rs` | replaced by `queue.rs` | Pattern 2/3 |

Tests move with the code: ~25 tests in `jobs.rs` (retry/assemble/precheck/record-failure) and ~14 in `launch.rs` become `pipeline` unit/integration tests using `host::fake` (pipeline already depends on `host`); tests that need `Store`/`Reporter` (`typed_failure_reaches_the_job_row`, `journal_lines_carry_stage_and_attempt_and_stage_durations`, `finished_attempt_leaves_an_outcome_line…`, `plain_error_keeps_the_old_failed_row`) stay in `app`.

### CLI shape (DC-1)
Add `cs2-cli record <ссылка|.dem> --player <ник> [--hlae <папка>] [--cs2 <папка>] [--out <папка>] [--size ШxВ] [--fps N]` -> builds `JobSpec{record:true}` -> `pipeline::run_job(&*pipeline::host::default_host(), …, &ConsoleProgress)`, and `cs2-cli retry <папка задачи>` -> `resume:true` from `record.json`. Keep the current default behaviour (plan + print instructions) unchanged. Defaults from env (`CS2_DIR`, new `HLAE_DIR`) or `pipeline::discover`. Killing the CLI mid-run closes the Job Object handle, which kills the game tree (KILL_ON_JOB_CLOSE) [CITED: 01-CONTEXT.md D-02], so no Ctrl-C handler/crate is needed.
Fate of legacy `assemble`: recommend **replace** it with `cs2-cli assemble <папка задачи>` backed by `pipeline::assemble_stage` and delete `renderer::assemble` (+ its tests) - removes the "duplicated ffmpeg invocation" and the undeadlined `Command::output()`. Fallback: keep it, mark legacy in USAGE. Either passes `crates/cli/tests/pipeline.rs` (that test uses library crates only, not the binary) [VERIFIED: crates/cli/tests/pipeline.rs].

## Q2 - Queue, retry, pause (RET-05, DIF-03, DIF-04)

### Current behaviour [VERIFIED: crates/app/src/app.rs:518-546, jobs.rs:289-338]
- `Msg::JobDone` -> `pauses_queue(&result)` (true only for `CANCELLED`) sets `jobs.paused` + `PauseReason::Cancelled`; then `jobs.running = None; …; run_next(self)`.
- `run_next` returns early if `running.is_some() || status.busy || paused`; takes `store.next_queued()`; `start_job` (attempt+1, clears fail_*); journal separator by `job.resume`/attempt (`StartKind::Retry` if resume, `First` if attempt<=1, else `Redo`); spawns a thread running `run`; `finish()`; sends `Msg::JobDone`.
- `PauseReason` is a `Copy` enum `{Cancelled, Interrupted}` [VERIFIED: app.rs:134-142]; `pause_banner(reason, waiting)` formats it.
- Typed failures continue the queue (Phase 2 D-13) - RET-05 is behaviourally true **except**: (1) panic in the job thread never sends `JobDone` (no `catch_unwind`/`panic` anywhere in `crates/app/src`), leaving the job `running` and `jobs.running` set; (2) UI-thread dependency (below).
- Store: `start_job`, `requeue`, `requeue_resume`, `requeue_with` clear all `fail_*` columns [VERIFIED: store/src/lib.rs:388-397,517-525] - so streak/auto-retry state **cannot** be derived from `fail_code` of finished rows after a requeue; keep it in the policy (streak) and in a new column (retry budget).

### The hidden-window stall (ASSUMED consequence, high impact)
`App::update` closes-to-tray via `ViewportCommand::Visible(false)` when `minimize_to_tray` (author's `settings.json` has it `true`) [VERIFIED: app.rs:707-710; settings.json]. winit 0.30.13 comment: "The window technically has to be visible to receive WM_PAINT messages" [CITED: winit-0.30.13/src/platform_impl/windows/event_loop.rs:957], and `request_redraw` is `RedrawWindow(.., RDW_INTERNALPAINT)` [VERIFIED: winit-0.30.13/src/platform_impl/windows/window.rs:152-158]; eframe turns `ctx.request_repaint()` into `window.request_redraw()` [VERIFIED: eframe-0.31.1/src/native/run.rs:182-183, 250-262]. So with the window hidden `update()` very likely does not run, `poll()` does not drain `Msg::JobDone`, and `run_next` is not called for the next job. **Spike first (10 minutes, plan 04-01 Wave 0):** hide window to tray with 2 queued *plan* jobs and watch whether the second starts. Whatever the spike says, the worker design is recommended (OBS-04 and auto-retry timing must not depend on repaint).

### Classification (DC-3; recommended, needs confirmation)
| Class | Codes | Rationale |
|-------|-------|-----------|
| **Launch** (one auto-retry) | `GAME_NOT_STARTED`, `CONSOLE_NOT_OPEN`, `DEMO_NOT_LOADED` (named in DIF-04); `GAME_EXITED` **only when `state < Take`** ("игра умерла рано") | transient start problems; retry runs from `record.json` (no download/plan) |
| **Environment** (streak -> pause) | `PRECHECK_FAILED`, `FOREIGN_GAME_RUNNING`, `PREV_GAME_STUCK` (named by phase 1-2 semantics), plus recommended `HLAE_ERROR`, `LAUNCHER_FAILED` | deterministic until the user fixes something |
| **Other** (no retry, no streak; resets streak) | `SCRIPT_NOT_RUN`, `TAKE_TIMEOUT`, `GAME_EXITED` in `Take`/later, `TAKE_INVALID`, `RETRY_NO_DATA`, untyped errors (download, parse, ffmpeg assemble), `CANCELLED` | mid-take death must **not** be auto-retried, otherwise drill "завершить `cs2.exe` из диспетчера посреди дубля" would silently succeed instead of ending in an error |
Open points: `LAUNCHER_FAILED`/`HLAE_ERROR` could arguably be Launch; `SCRIPT_NOT_RUN` could arguably be Launch. Untyped errors (e.g. FACEIT demo expired/network) are Other - an unattended acceptance run can fail on them (see Pitfall 8).

### Auto-retry mechanics (DC-4)
- Budget: 1 per job (`jobs.auto_retries`, additive column; reset to 0 by manual "Повторить"/"Заново").
- Trigger: after `finish()` stored the failed attempt (so evidence - journal outcome line, `record.log.N` rotation, `clips_old_*` - is preserved), policy says `AutoRetry`: worker appends a journal line `авто-повтор: сбой <КОД> на этапе «<состояние>»; пауза N с, затем запись заново`, calls `store.requeue_auto(id)` (status `queued`, `resume=1`, `auto_retries+=1`, clears fail_*), sets `retry_at = now + pause`.
- "Полная уборка": already done by every record attempt - `record()` closes the session (terminate + empty job) and `record_takes` always runs `cleanup_installed`; the next `record()` waits for the previous own game (`Host::wait_previous_game`, D-14). The explicit **pause** is new: recommend `AUTO_RETRY_PAUSE = 20 s` (constant next to `Deadlines`, abortable by cancel, tunable at acceptance).
- Path: `resume=true` (re-uses `record.json`; with no validated takes it records all takes, `clips_old_<ts>` for leftovers - exactly Phase 3's fallback). Launch-class codes all occur after `record.json` is saved [VERIFIED: jobs.rs:411 saves it before `record_takes`].
- Journal: new `StartKind::Auto` with title "авто-повтор" so separator is `──── попытка N · дата · авто-повтор ────`; kind chosen in worker as `job.resume && job.auto_retries > 0`. Restart-safe: a queued job with `auto_retries>0` survives an app restart and still logs as auto.
- UI: queued card shows pill "авто-повтор через N с" from the shared view.

### Environment pause (DC-5)
- State: `env_streak: Option<(String code, u32 n)>`; update on every *final* job result: same env code -> n+1; any other result (Done, Other, Launch, different env code) -> reset. `n >= 2` -> `Verdict::Pause(PauseReason::Environment{ code, jobs, message, hint })`. `Cancelled` pauses (existing); streak resets.
- `PauseReason` becomes a non-`Copy` enum with payload; `pause_banner` and its test change. Banner text (Russian): «Очередь на паузе: 2 задачи подряд упали с кодом PRECHECK_FAILED (окружение). Последняя: <сообщение ≤200 симв.>. <подсказка> Исправьте причину и нажмите «Продолжить»».
- Pause stays in memory only (Phase 2 decision); after restart the streak restarts at 0 (acceptable; the first failing job re-triggers within 2 jobs).
- Optional extension (needs confirmation): also pause after 2 consecutive jobs that *exhausted* the Launch auto-retry with the same code (CS2 update breaking HLAE would otherwise burn ~2×(console 90 s + …) per job for the whole queue).

### Panic guard (RET-05 hole)
Wrap the call in `std::panic::catch_unwind(AssertUnwindSafe(..))`; on panic produce a failed job with untyped message «внутренняя ошибка: <payload>» and continue. `Store`/`Host` lock poisoning: `Store::with` maps a poisoned mutex to an error string, so later writes fail loudly instead of hanging [VERIFIED: store/src/lib.rs:313-316].

## Q3 - OBS-04 notification

- `tray-icon` 0.19.3 public methods: `set_icon, set_menu, set_tooltip, set_title, set_visible, rect, …` - no balloon [VERIFIED: tray-icon-0.19.3/src/lib.rs:361-452; internals use only NIF_ICON/NIF_MESSAGE/NIF_TIP at platform_impl/windows/mod.rs:16]. It also does not expose its HWND, so a balloon cannot be attached to its icon.
- **Recommended:** `Host::notify(&self, title: &str, text: &str)` (default no-op). `WinHost` creates a message-only window (class `STATIC`, parent `HWND_MESSAGE`), `Shell_NotifyIconW(NIM_ADD, NIF_ICON|NIF_TIP|NIF_INFO, dwInfoFlags=NIIF_INFO)`, keeps the icon ~8 s on a **detached thread** (so the worker is not blocked), then `NIM_DELETE` + `DestroyWindow` on the same thread. Verified API surface in windows-sys 0.61.2: `NIF_INFO = 16u32`, `NIF_ICON = 2u32`, `NIF_TIP = 4u32`, `NIM_ADD = 0u32`, `NIM_DELETE = 2u32`, `NIIF_INFO = 1u32`, `NOTIFYICONDATAW` fields `szInfo: [u16; 256]`, `szInfoTitle: [u16; 64]`, `HWND_MESSAGE = -3i32 as _`, `IDI_INFORMATION`, `LoadIconW`, `CreateWindowExW`, `DestroyWindow`, `Shell_NotifyIconW` [VERIFIED: windows-sys-0.61.2/src/Windows/Win32/UI/Shell/mod.rs:3905-3925,4031-4047,774; WindowsAndMessaging/mod.rs:71,90,270,1619,1743]. Needs `Win32_UI_Shell` added to `crates/host/Cargo.toml`.
- Text limits: title ≤63, body ≤255 UTF-16 units: «5 готово / 0 упало» or «3 готово / 2 упало · первая причина: #14 CONSOLE_NOT_OPEN — <сообщение>» truncated with «…». Sanitize with `record::log::clean`/`one_line` and strip control characters (message embeds console fragments).
- When to fire: (a) queue becomes idle (`next_queued()` is `None`, nothing running, not paused) after ≥1 job finished since the last notification - counters `done/failed/first_failure` reset after firing; (b) on `Pause(..)` (user is away - the pause explanation matters as much). Cancelled jobs count in neither N nor M but suppress nothing.
- Fallbacks (cheap, always on): `Msg::QueueSummary` -> status line + a dismissible banner on the Jobs page; tooltip via `TrayIcon::set_tooltip` when the UI next runs. Balloon failure (`Shell_NotifyIconW` returns 0) is logged to the job journal, never fatal.
- Test: `FakeHost::notifications()` returns recorded `(title, text)`; queue test asserts the text for «2 готово / 1 упало» and that the first reason is the *first* failure, not the last.
- Rendering on Win11 (legacy balloon shown as toast, Focus Assist) is **[ASSUMED]**; covered by a drill item.

## Q4 - Acceptance (ACC-01, ACC-02)

### Set storage and enqueue (DC-7)
- Source of truth: `acceptance/matches.txt` in the repo (one link or id per line, `#` comments). Parsed by `faceit::parse_match_id`, which is strict (digits prefix + 5-group hex uuid) [VERIFIED: crates/faceit/src/lib.rs:21-36], so the file cannot smuggle path/URL fragments. Embedded in both binaries with `include_str!` (override path via env `CS2CINEMA_ACCEPTANCE`, matching the project's env-override convention). Human step: author picks 5 recent matches on 5 maps where his nickname played (maps available as meshes: ancient, anubis, dust2, inferno, mirage, nuke, overpass… [VERIFIED: `ls maps`]).
- Enqueue = one button "Поставить приёмочный набор" on the Jobs page: background thread -> for each id `store.cache_get("stats:{id}")` or `client.match_stats_json` -> `MatchStats.find(player_id)` to confirm the author played and to get the map -> `NewJob{kind:Record, source:Faceit(id), label:"приёмка k/5", hero_name:nickname, hero_steam:player.steam_id, map}` -> `store.set_batch(id, "acc-<unix>-<pid>")` (separate call, so `NewJob` literals elsewhere do not change) -> `Cmd::Wake`. Hero resolution uses `find_player(steam, nick)` later, steam id first [VERIFIED: pipeline/src/lib.rs:194-207]. Demos: `demo_urls` takes index 0 [VERIFIED: jobs.rs:353], so the set must be bo1 matches.
- DB: migration 5 (additive, per the file's rule "только добавлением колонок"): `ALTER TABLE jobs ADD COLUMN auto_retries INTEGER NOT NULL DEFAULT 0; ALTER TABLE jobs ADD COLUMN batch TEXT;` [pattern VERIFIED: store/src/lib.rs:256-278].

### Checker `cs2-cli acceptance check [--db <path>] [--batch <name>|--last] [--set <file>]`
Reads `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` via `Store::open_read_only`. Per job in the batch, **all must hold** (exit code 0 only if so; also writes `acceptance-<batch>.json` next to the DB):

| # | Criterion | How |
|---|-----------|-----|
| 1 | batch has exactly the set's N jobs; sources equal the set's ids | `jobs.source`, set file |
| 2 | N distinct maps | `out_dir/plan.json` -> `Timeline.map` (written by `renderer::render`: `plan.json` is the serialized `Timeline` [VERIFIED: renderer/src/lib.rs:163]) |
| 3 | status `Done`; `result` is an existing `highlights.mp4` | `JobStatus::Done` |
| 4 | ffprobe: has video stream **and** audio stream; duration within tolerance of the plan | `Host::run` + `record::verify::{probe_args, parse_probe, within_tolerance}`; expected = `record.json.video_secs` (final video length from `renderer::ffmpeg_args`) [VERIFIED: manifest.rs:421-461]; start with the existing ±10% / ≥0.5 s and adjust from evidence (ROADMAP note) |
| 5 | no `cs2.exe` / `HLAE.exe` left | `Host::processes()` snapshot |
| 6 | "no manual actions" | every attempt separator in `jobs.log` is "первый запуск" or "авто-повтор" (no "Повторить"/"Заново"); no job `Cancelled`/`Interrupted`; `attempt == 1 + auto_retries` |
| 7 | auto-retries reported separately | per-job count + the failed first attempt's code from the journal outcome line `итог попытки N: сбой КОД …` (format fixed in `journal::outcome_line` / `outcome_text`) |
Add `journal::attempts(log) -> Vec<(u32, StartKind)>` (parse of the separator already exists as `SEPARATOR_MARK`) so the CLI does not re-implement text parsing.

### Two series (DC-9 - needs confirmation)
ROADMAP/ACC-02 wording: "в двух сериях: с перезапуском приложения между ними и без". Recommended reading: **Series A** (fresh app start) -> `acceptance check` -> **restart the app** -> **Series B** -> `acceptance check`; "без перезапуска" = inside each series the five jobs run back-to-back in one process (PITFALLS: "the second pass finds leaks in teardown that the first pass cannot"). Strict alternative (three runs: A, B without restart, C after restart) costs ≈50% more wall time. Restart evidence: `batch` embeds the app PID (`acc-<unix>-<pid>`), and `check --expect-restart-from <batchA>` asserts the PIDs differ.
Wall-time budget (assumption): ~5-12 min per match (download once, plan, record, assemble) -> 1-1.5 h per series, plus `clips_old_*` disk growth; PRE-06 guards free space.

## Q5 - ACC-03 manual drills (DC-10)

Lessons from Phases 2-3: both manual checkpoints returned one word ("approved") with no evidence [VERIFIED: 02-08-SUMMARY.md, 03-08-SUMMARY.md]. Phase 4 drills must demand **machine-checkable end state**: add `cs2-cli acceptance state` (no jobs `running`/`cancelling`, no `cs2.exe`/`HLAE.exe`, no `highlights.cfg`/`cs2c_*`/`cs2cinema_*.dem` in `<CS2>/game/csgo[/cfg]`, no `installed.json` left) and require pasting its output plus the card's `fail_code`.

| Drill | Procedure | Expected code / state | Evidence |
|-------|-----------|-----------------------|----------|
| D1 cancel in each state | Cancel at: precheck, download, parse, plan, launcher, game, console, demo, script, take, closing, assemble (ffmpeg) | "отменяется" ≤3 s, clean ≤~10 s; status «отменена»; queue paused with «Продолжить» | `acceptance state` clean; journal timestamps |
| D2 kill app mid-record | `taskkill /F /IM cs2cinema.exe` during a take, restart | orphan `cs2.exe` killed, job «прервана» with reached state, queue paused until «Продолжить»; «Повторить» finishes | log lines «завершён осиротевший cs2.exe»; `acceptance state` |
| D3 end `cs2.exe` in Task Manager mid-take | kill the game process | `GAME_EXITED`, state `take`, **no auto-retry**, game folder clean, next queued job proceeds | card + journal (one attempt only) |
| D4 foreign CS2 | start CS2 yourself, then queue 2 jobs | `FOREIGN_GAME_RUNNING` on both -> after the 2nd the queue **pauses** with the explanation banner (DIF-03); foreign process untouched | banner text; foreign `cs2.exe` still alive |
| D5 ffmpeg out of PATH | remove ffmpeg from PATH, restart app, queue 2 jobs | `PRECHECK_FAILED` naming ffmpeg; pause after 2 | precheck report in card |
| D6 (new) hidden window | close window to tray with ≥2 queued jobs | queue continues while hidden; balloon «N готово / M упало» at the end | balloon screenshot/description; DB shows all jobs finished |
| D7 (new, opportunistic) auto-retry | when a real launch-class failure occurs during acceptance, confirm journal shows «авто-повтор» and attempt 2 | - | `acceptance check` auto-retry column |
Structure: one `checkpoint:human-verify` plan with these rows as a checklist; each row has a required evidence field (not free-form "approved").

## Q6 - Test strategy (Nyquist disabled; condensed)

| Behaviour | Test type | Where | Notes / Wave 0 gap |
|-----------|-----------|-------|--------------------|
| `classify(code, state)` table incl. `GameExited` before/after `Take` | unit | `pipeline::record::failure` | new |
| `QueuePolicy`: streak 2 same env code pauses; other code/Done resets; launch retry once; Cancelled pauses | unit (pure) | `app/queue.rs` | replaces `only_a_cancelled_job_pauses_the_queue` |
| Worker: job1 launch failure -> auto-retry -> job2 runs; env pause after 2; panic -> failed job, queue continues; end-of-queue notification text | integration with `FakeHost` + `Store::in_memory` + zero pause | `app` tests | **gap:** `FakeHost` has a single `Scenario` for all launches [VERIFIED: fake.rs:411-520]; add per-launch scenario sequence (`FakeHost::sequence(vec![Scenario…])`) |
| `Host::notify` recorded | unit | `host::fake` | new default method + recorder |
| Migration 5 (old DB gains columns; `requeue` resets `auto_retries`; `requeue_auto` increments, sets resume) | unit | `store` tests (pattern: `version_three_database_gains_attempt_and_resume`) | extend |
| `StartKind::Auto` separator + `journal::attempts` round trip | unit | `store/journal.rs` | extend |
| `pause_banner` for `Environment{..}` | unit | `app/jobs_page.rs` | update existing test |
| Moved orchestration tests (retry/assemble/precheck/record) | integration | `pipeline/tests/job.rs` | moved from `app/jobs.rs`; `healthy_settings` becomes `JobConfig` helper |
| Acceptance `evaluate()` over synthetic rows + fake ffprobe | unit | `cli/src/acceptance.rs` | pure function takes `JobFacts` + probe closure; DB read covered by one `Store::in_memory`-> file test |
| Acceptance set parsing (comments, ids, URLs, garbage, duplicates, ≠5) | unit | `pipeline` or `cli` | new |
| `crates/cli/tests/pipeline.rs` | existing | cli | must stay green (uses library crates only) |
| `RecordRequest.verify` mandatory | compile | all | removes `Option`; 2 call-sites to fix |
Quick run: `cargo test -p pipeline -p store -p host -p app -p cli --release`; full: `cargo test --release --workspace` (as in CI).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| ffprobe parsing / tolerance in the checker | new JSON parsing | `pipeline::record::verify::{probe_args, parse_probe, within_tolerance, judge}` | already hardened against untrusted output (size cap, typed parse) |
| Running ffprobe/ffmpeg with deadline+cancel | `Command::output()` | `Host::run(&CmdSpec{deadline,..})` | TMO-03 forbids undeadlined calls on the record path |
| Process listing for "no cs2.exe left" | `tasklist` text | `Host::processes()` | requirement text forbids `tasklist` |
| FACEIT id extraction | regex/URL split | `faceit::parse_match_id` | strict validation already |
| Journal format/attempt parsing | ad-hoc string search in CLI | `store::journal` helpers (+ new `attempts`) | one writer/reader module |
| Balloon via PowerShell | script string with failure text | `Shell_NotifyIconW` in `host` | injection-free |
| Atomic JSON writes | `fs::write` | existing `atomic_write` helpers in `manifest.rs` | already used for `record.json`/`manifest.json` |

**Key insight:** every piece of acceptance evidence already exists on disk or in SQLite; the work is wiring and one DB-open mode, not new measurement.

## Runtime State Inventory

Not a rename/migration phase in the data sense, but the refactor and schema change touch persisted state:

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` is at `user_version` 4 (`MIGRATIONS` has 4 entries) [VERIFIED: store/src/lib.rs:262-278]; author's DB exists and WAL files are present | migration 5 additive (code edit **and** auto-applied data migration on next start); older app versions will still open it (columns only added) |
| Live service config | none | none - verified: no external service stores job names |
| OS-registered state | none (no scheduled tasks); tray icon is per-process | none |
| Secrets/env vars | `FACEIT_API_KEY` (env) and `faceit_key` in `settings.json`; new env `CS2CINEMA_ACCEPTANCE` (optional), `HLAE_DIR` (CLI) | never log the key; acceptance report must not include settings |
| Build artifacts | installed `cs2-cli.exe` and `cs2cinema.exe` under `%LOCALAPPDATA%\cs2cinema` are copies of an older build (`ls` shows `cs2-cli.exe`) [VERIFIED] | author must rebuild/replace before drills; checker must come from the same build as the app (schema version check) |

## Common Pitfalls

### Pitfall 1: Hidden window stops the queue
**What goes wrong:** with window hidden to tray, `update()` not invoked -> `JobDone` not drained -> next job never starts, no balloon.
**Why:** WM_PAINT is not delivered to hidden windows (winit comment); queue logic lives in `poll()`.
**How to avoid:** worker thread (Pattern 3); drill D6; spike in Wave 0.
**Warning signs:** DB shows `queued` jobs while no job `running` and window hidden.

### Pitfall 2: Auto-retrying a mid-take death
**What goes wrong:** `GAME_EXITED` retried regardless of state -> drill D3 passes silently / hides real crashes.
**How to avoid:** classify with `state < Take`.

### Pitfall 3: Requeue erases the failure evidence the policy needs
**What goes wrong:** `requeue_*` clears `fail_code`, so streak and "first failure" cannot be read back from rows.
**How to avoid:** capture the code in the worker at finish time (policy memory, `Summary`), keep evidence in journal outcome lines.

### Pitfall 4: Series 2 overwrites series 1
**What goes wrong:** same `out_dir` per (match, hero): `highlights.mp4` replaced; checker run late sees series 2 only.
**How to avoid:** run `acceptance check` between series; the checker also stores per-batch JSON report.

### Pitfall 5: Checker opens the live DB read-write
**What goes wrong:** `Store::open` sets WAL and runs migrations - a newer CLI upgrading the author's DB under a running older app.
**How to avoid:** `Store::open_read_only` + `user_version` check.

### Pitfall 6: Moving code but leaving `Settings`/`Store` types in pipeline
**What goes wrong:** `pipeline` gains a dependency on `app`/`store` types or cyclic imports.
**How to avoid:** `JobConfig`/`JobSpec`/`Artifact` boundary (Pattern 1); `store` stays free of pipeline types (Phase 1 D-05).

### Pitfall 7: Balloon dropped or invisible
**What goes wrong:** deleting the notify icon immediately after `NIM_ADD` can lose the balloon; Focus Assist/disabled notifications hide it; calling from a thread that exits.
**How to avoid:** keep icon ~8 s on its own thread, create+destroy window on that thread, always mirror into an in-app banner, include a drill item. [ASSUMED behaviour]

### Pitfall 8: Network/demo-expiry failures during acceptance
**What goes wrong:** FACEIT demo removed or transient download error -> untyped failure (Other class) -> series fails through no fault of the recorder.
**How to avoid:** choose recent bo1 matches; after the first successful download the demo is cached in `out_root/demos` + `demos` table; record this in the acceptance checklist. Optional follow-up (not in requirements): bounded retry for download.

### Pitfall 9: Stale docs claim `tasklist`/`taskkill`
**What goes wrong:** success criterion 1 is judged by grep; CLAUDE.md STACK/ARCHITECTURE and `.planning/codebase/*` still mention it.
**How to avoid:** include a docs task (regenerate/patch those sections) and a grep gate `grep -rn "tasklist\|taskkill" crates` = empty.

## Code Examples

### Journal kind for auto-retry (extends verified enum)
```rust
// Source: crates/store/src/journal.rs:16-24 (verbatim variants: First, Retry, Redo)
pub enum StartKind { First, Retry, Redo, /* new */ Auto }
impl StartKind {
    pub fn title(self) -> &'static str {
        match self { /* …existing arms…, */ StartKind::Auto => "авто-повтор" }
    }
}
```

### Policy skeleton
```rust
// Source: design; JobStatus variants verified at crates/store/src/lib.rs:47-57
pub enum Verdict { Next, AutoRetry, Pause(PauseReason) }
pub fn after(&mut self, auto_retries_done: u32, o: &Finished) -> Verdict {
    match o {
        Finished::Done => { self.streak = None; Verdict::Next }
        Finished::Cancelled => { self.streak = None; Verdict::Pause(PauseReason::Cancelled) }
        Finished::Failed { class: FailClass::Launch, .. } if auto_retries_done < 1 => Verdict::AutoRetry,
        Finished::Failed { class: FailClass::Environment, code, .. } => { /* n+1 if same code else 1; n>=2 => Pause(Environment{..}) */ }
        Finished::Failed { .. } => { self.streak = None; Verdict::Next }
    }
}
```

### Notify (shape only; compile-check required)
```rust
// Source: windows-sys-0.61.2 constants/fields verified (see Q3); function shape is [ASSUMED]
pub fn balloon(title: &str, text: &str) -> bool {
    // CreateWindowExW("STATIC", HWND_MESSAGE) → NOTIFYICONDATAW{ uFlags: NIF_ICON|NIF_TIP|NIF_INFO, dwInfoFlags: NIIF_INFO, szInfo, szInfoTitle }
    // Shell_NotifyIconW(NIM_ADD) → sleep ~8 s → Shell_NotifyIconW(NIM_DELETE) → DestroyWindow, all on one detached thread
}
```

### Migration 5 (additive)
```rust
// Source: pattern at crates/store/src/lib.rs:262-278 (each element raises user_version by one)
"ALTER TABLE jobs ADD COLUMN auto_retries INTEGER NOT NULL DEFAULT 0;
 ALTER TABLE jobs ADD COLUMN batch TEXT;",
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `tasklist`/`taskkill /IM` in `launch.rs` | Job Object + handle liveness + `kill_orphan` by (PID, creation time, image) | Phases 1-3 | already done; Phase 4 only fixes docs + leftovers |
| UI-thread-driven queue | worker thread owning policy | Phase 4 | needed for tray-hidden unattended runs |
| `Option<Verify>` on `RecordRequest` | mandatory verify | Phase 4 | removes the unvalidated path (flagged by 03-VERIFICATION) |

**Deprecated/outdated:**
- `cs2-cli assemble` / `renderer::assemble`: old `shot_NN` layout, no audio, undeadlined `output()`.
- `.claude/CLAUDE.md` and `.planning/codebase/*` statements about `tasklist`/`taskkill`.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | A window hidden via `ViewportCommand::Visible(false)` does not run eframe `update()` on Windows (derived from winit's WM_PAINT comment, not run) | Q2, Pitfall 1 | If wrong, the worker thread is still better but less urgent; plan can downgrade it |
| A2 | Legacy `NIF_INFO` balloon from a short-lived second tray icon is shown as a Win11 toast without an AUMID | Q3 | Falls back to in-app banner + tooltip; drill D6 detects |
| A3 | Deleting the notify icon ~8 s after `NIM_ADD` is enough for the balloon to display | Q3, Pitfall 7 | tune delay in the drill |
| A4 | Launch-class failures are transient enough that one retry after 20 s usually helps | Q2 | retry wasted (≈ +2-4 min per failing job); tune constant at acceptance |
| A5 | Series wall time ≈ 1-1.5 h each; disk growth acceptable | Q4 | schedule longer window |
| A6 | FACEIT keeps demos long enough for acceptance; bo1 only (`demo_index` 0) | Q4, Pitfall 8 | pick other matches |
| A7 | `HLAE_ERROR` and `LAUNCHER_FAILED` are environment-class (deterministic) | Q2 classification | wrong class = either pause too eagerly or retry uselessly |
| A8 | `record.json.video_secs` ≈ real `highlights.mp4` duration within ±10%/0.5 s (intro/outro cards, xfades included) | Q4 | tolerance needs widening; ROADMAP already says to tune it |
| A9 | `Settings::load()` at job start equals the applied settings (UI saves on "Сохранить") | Pattern 3 | if UI keeps unsaved applied state, worker would miss it - check `settings_page.rs` save path |
| A10 | `TrayIcon` handle is not needed by the worker (balloon is a separate icon) | Q3 | none |
All A-items need user confirmation or a spike before becoming locked decisions.

## Design Choices Needing User Confirmation

| ID | Choice | Recommendation |
|----|--------|----------------|
| DC-1 | TST-03 scope and CLI shape; fate of legacy `assemble`/`renderer::assemble` | Move orchestration to `pipeline::job`/`assemble`/`gamedir`; CLI gets `record`, `retry`, replaced `assemble <папка>`; delete `renderer::assemble`; make `verify` mandatory; delete `launch.rs` |
| DC-2 | Queue driver | Worker thread + shared view + panic guard; spike A1 first |
| DC-3 | Failure classes | Table in Q2 (Launch: 3 codes + early `GAME_EXITED`; Env: 5 codes) |
| DC-4 | Auto-retry parameters | 1 per job, 20 s pause, `resume` path, `auto_retries` column, reset on manual requeue |
| DC-5 | Env pause | 2 consecutive same code, in-memory, banner with last message+hint; optional extension to exhausted-launch codes |
| DC-6 | Notification | `Host::notify` native balloon + in-app banner; fire on idle and on pause |
| DC-7 | Acceptance set + enqueue | `acceptance/matches.txt` embedded, env override, Jobs-page button, `batch` column |
| DC-8 | Checker | `cs2-cli acceptance check` (read-only DB), criteria table, initial tolerance ±10%/0.5 s |
| DC-9 | "Two series" meaning | A -> restart -> B (inside each, no restart); strict 3-run variant only if author wants it |
| DC-10 | Drills | D1-D7 with machine-checkable `acceptance state` evidence |
| DC-11 | Deadline/tolerance tuning | final plan reads manifests/`record.log` from acceptance runs and adjusts constants (`Deadlines`, `TAKE_TOLERANCE_*`, `AUTO_RETRY_PAUSE`) |
| DC-12 | Docs | refresh CLAUDE.md STACK/ARCHITECTURE + `.planning/codebase/*` tasklist claims; README for `record`/`acceptance` |

## Open Questions

1. **Does `update()` run while the window is hidden to tray?**
   - Known: winit/eframe source suggests no. Unknown: actual behaviour on the author's machine.
   - Recommendation: 10-minute spike before committing to the worker refactor; keep the worker regardless.
2. **Which 5 matches?** Author must supply 5 FACEIT links on 5 distinct maps where his nickname (`wytom`) played; bo1; demos still downloadable. Human-action task.
3. **Should exhausted launch failures also trigger the pause?** (DC-5 extension.)
4. **Is `LAUNCHER_FAILED`/`HLAE_ERROR` Launch or Environment?** (A7.)
5. **Strict vs recommended reading of "две серии"?** (DC-9.)
6. **Should the acceptance button live on Jobs page or Settings?** Recommend Jobs page.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo/rustc | build/tests | ✓ | 1.99.0 | - |
| ffmpeg | assemble, acceptance ffprobe | ✓ (PATH via winget) | 9.0.2 | - (drill D5 removes it temporarily) |
| ffprobe | checker | ✓ | 9.0.2 | - |
| Steam | precheck, record | ✓ (`steam.exe` running at research time) | - | - |
| CS2 / HLAE | drills, acceptance | settings point to `...\Steam\steamapps\common\Counter-Strike Global Offensive` and `...\AppData\Local\hlae` (not probed for files) | HLAE 2.192.x per README | - |
| FACEIT API key + nickname `wytom` | acceptance enqueue | present in `settings.json` (key not read) | - | - |
| Python | none needed | ✗ | - | not required |
| sqlite3 CLI | none needed | ✗ | - | use `Store`/rusqlite |
| node (gsd-tools) | commit | ✓ (`/c/Program Files/nodejs`) | - | - |
**Missing dependencies with no fallback:** none for planning. Execution of drills/acceptance needs the author at the machine.

## Security Domain

`security_enforcement` is enabled (ASVS level 1).

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2/V3 Authentication/Session | no | single local user |
| V4 Access Control | partial | never touch foreign processes (PRE-05, TMO-05); `kill_orphan` full-identity rule unchanged; `is_ours` path guard moves intact |
| V5 Input Validation | yes | `faceit::parse_match_id` for the acceptance file; CLI paths canonicalized before use; ffprobe invoked via argv (`-i <path>`), never a shell |
| V7 Error handling/logging | yes | notification and journal text pass `record::log::clean`/`redact`; no FACEIT key in logs/reports |
| V10/V12 Files | yes | checker opens DB read-only; reads `out_dir` from own DB only; `processes.json`/`record.json` stay untrusted-input (already validated) |
| V6 Cryptography | no | none |

### Known Threat Patterns
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Console text (from game) flowing into a notification or shell | Tampering/Elevation | native API only, length cap, control-char strip; no shell |
| Runaway auto-retry loop | DoS | budget 1 per job; streak pause |
| Job-thread panic wedging queue | DoS | `catch_unwind`, failed status |
| Checker mutating/migrating live DB | Tampering | read-only open + version check |
| Malformed acceptance file (huge, binary) | DoS/Tampering | embedded `include_str!` or size-capped read, strict id parser, max N lines |
| Mid-take kill leaving foreign-looking leftovers | Integrity | existing `installed.json`/`.partial`/orphan recovery (verified Phase 3) |

## Sources

### Primary (HIGH confidence - read this session)
- `crates/app/src/{jobs.rs,launch.rs,app.rs,main.rs,jobs_page.rs,settings.rs}`; `crates/cli/src/main.rs`, `crates/cli/tests/pipeline.rs`, `crates/cli/Cargo.toml`
- `crates/pipeline/src/{lib.rs,record/{mod.rs,failure.rs,machine.rs,deadlines.rs,verify.rs},manifest.rs}`
- `crates/host/src/{lib.rs,fake.rs}`, `crates/host/Cargo.toml`; `crates/store/src/{lib.rs,journal.rs}`; `crates/renderer/src/assemble.rs`; `crates/faceit/src/{lib.rs,stats.rs}`
- `.planning/{ROADMAP.md,REQUIREMENTS.md,STATE.md,PROJECT.md}`, phase 1-3 CONTEXT/VERIFICATION/SUMMARY, `research/{SUMMARY,PITFALLS,FEATURES}.md`
- `~/.cargo/registry`: tray-icon-0.19.3 (no `NIF_INFO`), windows-sys-0.61.2 (Shell/WindowsAndMessaging items), winit-0.30.13 (windows `event_loop.rs`, `window.rs`), eframe-0.31.1 (`native/run.rs`, `glow_integration.rs`), rusqlite-0.32.1
- `gsd-tools query package-legitimacy check` (windows-sys, rusqlite: OK)

### Secondary (MEDIUM)
- none (no web sources were needed; everything is in-repo or in vendored crate sources)

### Tertiary (LOW)
- Behavioural expectations on Win11 balloon rendering and hidden-window repaint are training-knowledge-level and tagged ASSUMED.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - no new packages; APIs read from registry sources.
- Architecture (move map, worker, policy): HIGH for what exists and where; MEDIUM for the worker's necessity (A1).
- Pitfalls: MEDIUM-HIGH - code-derived except Windows UI behaviours.
- Acceptance design: MEDIUM - interpretation of "two series" and tolerance need confirmation.

**Research date:** 2026-10-11
**Valid until:** 2026-11-10 (stable codebase; re-check if Phase 4 planning slips past other edits to `jobs.rs`)
