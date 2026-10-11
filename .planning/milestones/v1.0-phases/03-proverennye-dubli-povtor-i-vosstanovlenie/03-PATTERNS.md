# Phase 3: Проверенные дубли, повтор и восстановление - Pattern Map

**Mapped:** 2026-10-10
**Files analyzed:** 22 new/modified files
**Analogs found:** 22 / 22 (all analogs are git-tracked sources under `crates/`; no gitignored mirror paths)

Conventions for every file (CLAUDE.md): Russian user strings and doc comments, English identifiers; `Result<T, String>` at pipeline/store boundary; sentence-named tests; new `#[serde(default)]` on every field added to a persisted struct; `cfg(windows)` code needs a non-Windows stub; external processes only via `Host::run` (never `Command::output()` on the record path); JSON files written atomically (tmp + rename). Line numbers below are as of the working tree on 2026-10-10 and will drift once edits start; re-locate by symbol.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/pipeline/src/manifest.rs` (NEW: Manifest, TakeEntry, TakeStatus, EnvSnapshot, load/save atomic, `missing()`) | model | file-I/O | `crates/app/src/launch.rs` `Installed` + `write_installed`/`note_take_done` | role-match |
| `crates/pipeline/src/record/verify.rs` (NEW: ffprobe call, `judge`, `within_tolerance`) | service | request-response | `crates/app/src/launch.rs` `nvenc_check` (+ `jobs.rs::run_ffmpeg`) | role-match |
| `crates/pipeline/src/record/deadlines.rs` (+ `take_finalize`, tolerance consts) | config | transform | itself (`Deadlines`, `scaled`, `ffprobe` field) | exact |
| `crates/pipeline/src/record/failure.rs` (+ `FailCode::TakeInvalid`, `RetryNoData`, `msg_take_invalid`) | model/catalog | transform | itself (`FailCode`, `msg_*`) | exact |
| `crates/pipeline/src/record/machine.rs` (`Action::Verify`, `Input::TakeInvalid`) | state machine | event-driven | itself (`step`, `Action`, `Input`) | exact |
| `crates/pipeline/src/record/mod.rs` (`RecordRequest.verify`, pending probes, manifest writes, `processes.json`) | driver | event-driven | itself (`record()` loop, `shutdown()`, `mark_partial`) | exact |
| `crates/pipeline/src/record/log.rs` (rotation `record.log` -> `record.log.N`) | utility | file-I/O | itself (`RecordLog::open`) | exact |
| `crates/pipeline/src/precheck/mod.rs` (`Report` env facts, `#[serde(default)]`) | model | transform | itself | exact |
| `crates/host/src/lib.rs` (`ProcIdentity`, `OrphanOutcome`, `Session::processes`, `Host::kill_orphan`) | trait | request-response | itself (`Host`/`Session` traits with default methods) | exact |
| `crates/host/src/win/process.rs` + `win/mod.rs` (`kill_if_same`, `WinSession::processes`) | utility (FFI) | request-response | itself (`OwnedProc::open_owned/creation_time/image_path/kill/wait`) | exact |
| `crates/host/src/fake.rs` (ffprobe responder, args matcher, truncated take, orphan kills) | test fake | event-driven | itself (`Scenario` builders, `programs`, `run_result`) | exact |
| `crates/host/src/unsupported.rs` | stub | n/a | itself | exact |
| `crates/store/src/lib.rs` (migration 4, `Interrupted`, `attempt`/`resume`, `start_job` keeps log, `reset_running`) | model/migration | CRUD | itself (`MIGRATIONS[2]`, `Cancelling`, `requeue`, `reset_running`) | exact |
| `crates/app/src/jobs.rs` (resume branch, `record.json`, `JobReporter` prefix/durations, temp-name assembly) | controller | request-response | itself (`run`, `JobReporter`, `run_ffmpeg`, `plan_hash`) | exact |
| `crates/app/src/launch.rs` (`recover` manifest-aware, orphan kill pass) | service | file-I/O | itself (`recover`, `cleanup_installed`, `Installed`) | exact |
| `crates/app/src/app.rs` (startup order, paused after interrupt, cached `resumable`/old logs in `refresh_jobs`) | controller | event-driven | itself (`reset_running` call, `refresh_jobs`) | exact |
| `crates/app/src/jobs_page.rs` (Interrupted badge, Повторить/Заново/Открыть папку, "Попытка N") | component | request-response | itself (`Act` enum, `JobStatus` match, `Act::Retry`) | exact |
| `crates/renderer/src/lib.rs` (`ffmpeg_args_with`, optional `rechain`) | utility | transform | itself (`ffmpeg_args`, `FILES`) | exact |
| `crates/pipeline/src/lib.rs` (`write_demo_copy`, `pub mod manifest`) | utility | file-I/O | itself (`Analysis::write_demo`) | exact |
| `crates/pipeline/tests/record.rs` (TST-02 scenarios) | test | event-driven | itself (`run`, `fail_of`, `game_exit_mid_take_...`) | exact |
| `crates/store/src/lib.rs` tests (migration 4, Interrupted) | test | CRUD | itself `version_two_database_gains_precheck_and_last_good` | exact |

## Pattern Assignments

### `crates/pipeline/src/record/verify.rs` (service, request-response)

**Analog:** `crates/app/src/launch.rs` `nvenc_check` (a bounded external tool call through `Host::run`) and `crates/app/src/jobs.rs::run_ffmpeg`.

**Core pattern** (launch.rs `nvenc_check`):
```rust
let cmd = CmdSpec { program: ffmpeg.to_path_buf(), args: args.map(String::from).into(), cwd: None, deadline };
match host.run(&cmd, cancel, &mut |_| {}) { ... }
```
For ffprobe: `program: ffprobe_path` (reuse `"ffprobe"` path passed by app, `jobs.rs` Inputs.ffprobe), `deadline: d.ffprobe`, `cancel = &|| p.cancelled()`. `Host::run` delivers stdout line by line (trimmed, empties dropped): collect into one `String` (cap 64 KiB), parse with `serde_json`. `out.code != Some(0)` -> use `stderr_tail` (last lines) in the Russian reason, exactly as `run_ffmpeg`:
```rust
if out.code != Some(0) {
    let tail: Vec<&str> = out.stderr_tail.iter().rev().take(6).map(String::as_str).collect();
    return Err(format!("ffmpeg завершился с ошибкой: {}", ...));
}
```
**Error mapping:** `HostError::Cancelled` -> `Input::Cancel`; `Timeout`/`Spawn` -> invalid with host error text. `judge()` is a pure fn returning `Result<TakeFacts, String>`; unit-test it in the same file (sentence names). Audio is checked on `audio.wav`, video on `video.mp4` (RESEARCH Pattern 2: `TakePlan::FILES`).

---

### `crates/pipeline/src/manifest.rs` (model, file-I/O)

**Analog:** `crates/app/src/launch.rs` `Installed` (lines ~95-110): serde struct with `#[serde(default)]` on later fields, fixed file-name const, tolerant read.
```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Installed {
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub done_takes: Vec<u32>,
}
pub const INSTALLED_FILE: &str = "installed.json";
```
Tolerant read pattern (launch.rs `note_take_done`): `std::fs::read_to_string(..).ok().and_then(|t| serde_json::from_str::<T>(&t).ok())` - manifest `load` returns `Option`/default on garbage, never panics; validate every `dir` against `^take\d{4}$` before joining. Add `version: u32` field. `save` = write `manifest.json.tmp` then `std::fs::rename`. Status enum `#[serde(rename_all = "lowercase")]` (`pending/recorded/validated/invalid`). Tests: round trip, unknown-field tolerance, `missing()`, bad dir name rejected.

---

### `crates/pipeline/src/record/failure.rs` (+ `TakeInvalid`)

**Analog:** itself. Enum (lines 14-34) with doc-commented variants; add:
```rust
    /// Дубль записан, но не прошёл проверку ffprobe (нет звука, обрезан, другое разрешение).
    TakeInvalid,
```
Then extend `as_str()` (`"TAKE_INVALID"`), `hint()`, and add `pub fn msg_take_invalid(n: u32, m: u32, reason: &str) -> String` next to `msg_take_timeout(n, m, kind, limit)` (line 244) - number of the take in message AND hint (D-04). `RetryNoData` (message "нет данных, запустите задачу заново") follows the Phase 2 `PrecheckFailed` precedent (doc comment + variant). Search for every `match` on `FailCode` (compiler finds them); `stop_for` in `record/mod.rs` falls through to `Stop::Graceful` for unknown codes.

---

### `crates/pipeline/src/record/deadlines.rs`

**Analog:** itself. Field + default + scaled (three places):
```rust
/// `ffprobe` на одном файле.
pub ffprobe: Duration,            // default: ffprobe: unit(30),
...                               // scaled(): ffprobe: d(self.ffprobe),
```
Add `take_finalize` the same three ways; `pub const TAKE_TOLERANCE_REL: f32 = 0.10; pub const TAKE_TOLERANCE_MIN_SECS: f32 = 0.5;` as module-level consts beside the struct (D-01), with `within_tolerance` in `verify.rs`.

---

### `crates/pipeline/src/record/machine.rs` (+ `Action::Verify`, `Input::TakeInvalid`)

**Analog:** itself - pure `Machine::step(now, Input) -> Vec<Action>`; `Finish(Err(RecordFailure))` is produced inside the machine via `RecordFailure::new(code, state, message, elapsed)` (failure.rs:131). Emit `Action::Verify(index)` where `TAKE_END n` (nonce-checked) is accepted; `Input::TakeInvalid{index,message}` -> `Finish(Err(TakeInvalid))` with `state = RecordState::Take`. Keep machine free of IO (all ffprobe in `mod.rs` driver).

---

### `crates/pipeline/src/record/mod.rs` (driver)

**Analog:** itself - the `record()` poll loop (~lines 241-328), `shutdown()`, `mark_partial(clips: &Path, done: &[u32]) -> Vec<String>` (line 366). New `RecordRequest.verify: Option<Verify>` (None = legacy tests / CLI skip verification). Pending-probe list is polled inside the existing loop (no blocking sleep): `session.poll(TICK)` advances the fake's virtual clock. After `Finish(Ok)` + `shutdown()`, drain remaining probes once; failure converts Ok -> Err(TakeInvalid). Write `processes.json` on `SessionEvent::GameAppeared`, delete after confirmed exit. All `RecordRequest { .. }` literals (tests/record.rs `request()`, jobs.rs tests) need the new field.

---

### `crates/pipeline/src/record/log.rs` (rotation)

**Analog:** itself, `RecordLog::open` (lines 25-40):
```rust
let file = OpenOptions::new().create(true).append(true).open(out_dir.join("record.log"))?;
let mut log = RecordLog { file };
let used = log.file.metadata().map(|m| m.len() > 0).unwrap_or(false);
```
Change: before opening, if `record.log` exists and non-empty, `rename` to `record.log.{N}` with N = smallest unused integer >= 1 found by scanning `out_dir` (not from `jobs.attempt`). Update the module doc (it currently promises append). Existing test `record_log_is_written_on_success_and_on_failure_with_source_tags` (tests/record.rs ~480-535) asserts two blocks in one file: update to first -> `record.log.1`, new -> `record.log`.

---

### `crates/host/src/lib.rs` + `win/process.rs` + `win/mod.rs` + `unsupported.rs` + `fake.rs`

**Analog:** itself; Phase 2 added trait methods with default bodies so `UnsupportedHost` and old fakes compile (`Host::processes/free_space/stuck_games`, `Session::windows/wait_exit`). Copy that: new methods get defaults
```rust
pub struct ProcIdentity { pub pid: u32, pub created: u64, pub image: String }
// Session:  fn processes(&self) -> Vec<ProcIdentity> { Vec::new() }
// Host:     fn kill_orphan(&self, id: &ProcIdentity) -> OrphanOutcome { OrphanOutcome::Unsupported }
```
Windows kill: one handle - `OwnedProc::open_owned(pid)` -> compare `creation_time()` (FILETIME u64, `process.rs:~137`) and `image_path()` file name (allow-list cs2.exe/hlae.exe/ffmpeg.exe) -> `kill(1)` -> `wait(3 s)`; never by name alone (TMO-05, guard test `no_process_is_ever_stopped_by_image_name` in `crates/pipeline/tests/own_processes.rs`).

**Fake additions** (fake.rs Scenario builders are `pub fn x(mut self, ..) -> Self`): existing `programs` matcher only matches program name (`fake.rs ~465`: `.find(|(p, _, _)| name.contains(p.as_str()))`) - add an args-substring variant `program_for("ffprobe", "take0002/video.mp4", lines, result)`, plus an auto-responder synthesizing valid ffprobe JSON (duration from planned take, size from `LaunchSpec.size`) so legacy happy tests need no scripting, and `truncated_take(n)` / `finalize_lag(Duration)` scenarios. Existing builders to mimic: `run_result(r)`, `program(..)` (line 167), `clips_dir(dir)`, `take_dirs_on_start()`.

---

### `crates/store/src/lib.rs` (migration 4, `Interrupted`, attempt)

**Analog:** itself; migration list + doc comment (lines ~233-262) - add a 4th entry, only ADD columns, and extend the version comment:
```rust
"ALTER TABLE jobs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0;
 ALTER TABLE jobs ADD COLUMN resume INTEGER NOT NULL DEFAULT 0;",
```
`JobStatus::Interrupted` ("interrupted", title "прервана") added the way `Cancelling` was (5e3bea7); `parse` maps unknown to `Failed`. `start_job` (line 360) stop clearing `log`, add `attempt = attempt + 1`. `requeue` (line 475) keeps the field-reset list as template for `requeue_resume`. `reset_running` (line ~493):
```rust
"UPDATE jobs SET status = 'failed', error = 'прервано: приложение закрылось во время выполнения', phase = '', updated_at = ?1 WHERE status = 'running'"
```
-> `status = 'interrupted'`, keep `stage`, copy `phase` into `fail_state`, `fail_code = 'INTERRUPTED'`. Update tests asserting `user_version == 3` (lines ~710-771) and `Failed` after `reset_running` (`crash_recovery_and_caches`, `crash_recovery_turns_cancelling_into_cancelled_and_running_into_failed`, `typed_failure_round_trips_and_retry_clears_it`, `precheck_report_is_cleared_on_start_and_requeue`). New migration test copies `version_two_database_gains_precheck_and_last_good` (line ~739).

---

### `crates/app/src/jobs.rs`

**Analog:** itself.
- `JobReporter` (lines ~84-110): `stage()` calls `store.set_stage`; `log()` writes `format!("{}  {m}", chrono::Local::now().format("%H:%M:%S"))` to DB and sends raw `m` to `Msg::Log` (status.rs parses raw `"запись: дубль "` - keep prefix `[этап #N]` DB-only). Add `attempt` and `Mutex<(String, Instant)>` for stage durations (must stay `Sync`).
- `event()` already calls `launch::note_take_done` per `TakeDone` - superseded by manifest; keep fallback.
- `record_request(s, out_dir, demo_name, plan, job_id)` (line 136) takes `&pipeline::Plan`; change to `(takes, nonce, frame)` so fresh run and resume share it. Resume must use `record.json.frame`, not `Settings.frame_size()`.
- `run_ffmpeg(host, dir, args, secs, deadline, p)` (line 415): reuse unchanged; output arg last token `highlights.mp4` -> checked replace with `highlights.part.mp4`, then `fs::rename`; on failure delete only `.part` (remove the `remove_file(&out)` of the real result).
- `plan_hash(out_dir, nonce)` (line 376): reuse as manifest `plan_hash`; test `plan_hash_ignores_the_nonce` is the test-style template.

---

### `crates/app/src/launch.rs`

**Analog:** itself. `recover(cs2_dir, out_dir)` (line 236) currently derives `done` from `Installed.done_takes`:
```rust
let done = std::fs::read_to_string(out_dir.join(INSTALLED_FILE)).ok().and_then(|t| serde_json::from_str::<Installed>(&t).ok()).map(|r| r.done_takes);
let mut notes = cleanup_installed(cs2_dir, out_dir);
if let Some(done) = done { notes.extend(pipeline::record::mark_partial(&out_dir.join("clips"), &done)); }
```
Change `done` to manifest `recorded ∪ validated` dirs (fallback to `done_takes` if no manifest) so validated takes are never renamed `.partial`. Add orphan-kill pass before cleanup (reads `processes.json`, calls `Host::kill_orphan`). Test style: `recover_marks_unfinished_takes_partial` (line ~376), `recover_without_a_record_keeps_finished_takes` (~394).

---

### `crates/app/src/app.rs`, `crates/app/src/jobs_page.rs`

**Analog:** themselves. `jobs_page.rs` action dispatch (lines ~75-95):
```rust
Some(Act::Retry(id)) => {
    app.jobs.paused = false;
    if let Err(e) = app.store.requeue(id) { app.status.finish(Err(e)); }
    app.refresh_jobs();
    jobs::run_next(app);
}
```
Split into `Act::Retry(id)` (-> `requeue_resume`) and `Act::Redo(id)` (today's body). Button site (lines ~213-218): `JobStatus::Failed | JobStatus::Cancelled => { if ui.add_enabled(!app.status.busy, egui::Button::new("Повторить")).clicked() { *act = Some(Act::Retry(j.id)); } }` -> add `| JobStatus::Interrupted`, "Повторить" gated by `app.jobs.resumable.contains(&j.id)`, "Заново", and `ui::open_path(d)` for "Папка". Both `match j.status` blocks are exhaustive: compiler finds all sites. Filesystem lookups (`record.json` exists, `record.log.*` scan) go into `refresh_jobs()`, like `JobsState.sizes`, never per frame. Startup (`app.rs` ~179-188, `reset_running` ~158): order = kill orphans -> `launch::recover` -> `reset_running`; if it returned >0 set `jobs.paused = true` before `jobs::run_next` (D-16); generalize banner text "после отмены".

---

### `crates/renderer/src/lib.rs`

**Analog:** itself. `ffmpeg_args` (only caller `pipeline::plan_player` + renderer test ~line 724) stays as a wrapper over new `ffmpeg_args_with(t, cfg, dirs, out_name)`. Inputs line to parametrize: `let inputs = (0..t.shots.len()).flat_map(|i| [clip_path(take_of[i]), audio_path(take_of[i])]).collect();`. Do NOT touch `crates/renderer/src/assemble.rs` (CLI legacy `shot_NN`; `crates/cli/tests/pipeline.rs` must pass).

---

### `crates/pipeline/tests/record.rs` (TST-02)

**Analog:** itself. Helpers `takes(n)`, `request(name, n)`, `run(name, sc, n) -> (Outcome, Capture, FakeHost, RecordRequest)`, `fail_of(r)`, `secs(n)`; `Capture` collects events and can cancel after a `RecordState` (`Capture::cancelling_after(state).watching(&fake)`, `.forced()`).

Test style to copy:
```rust
#[test]
fn game_exit_mid_take_fails_with_game_exited() {
    let sc = Scenario::happy(3).game_dies_at(secs(20), Some(0xC0000005));
    let (r, ..) = run("game-dies", sc, 3);
    let f = fail_of(r);
    assert_eq!(f.code, FailCode::GameExited);
    assert_eq!(f.state, RecordState::Take);
    assert!(f.message.contains("дубль 2 из 3"), "{}", f.message);
}
```
New tests: `truncated_take_fails_with_take_invalid_naming_the_take` (fake scenario `truncated_take`, `req.verify = Some(..)`), `take_without_audio_is_invalid`, `ffprobe_runs_right_after_take_end_not_before`, `finalizing_take_is_retried_until_the_file_is_ready` (moov-not-found lag), `last_take_is_verified_after_shutdown`, cancel in each state (loop over `RecordState` with `Capture::cancelling_after`), and the existing scenarios already covering launcher/game-not-started/console/demo/script/marker/game-died - only add the missing ones after a `grep -n "^fn "` gap check. Note: existing tests (35, ~0.03 s) run with `verify: None` so they stay unchanged.

## Shared Patterns

### External process with deadline and cancel
**Source:** `crates/app/src/jobs.rs::run_ffmpeg` (line 415) and `launch.rs::nvenc_check`
**Apply to:** `verify.rs` ffprobe, ffprobe on `.part`, version snapshots (ffmpeg/ffprobe `-version`) for `EnvSnapshot`.
```rust
let cmd = CmdSpec { program, args, cwd: Some(dir.to_path_buf()), deadline };
let out = host.run(&cmd, &|| p.cancelled(), &mut on_line).map_err(|e| ffmpeg_error(&e, deadline))?;
```

### Persisted-JSON struct
**Source:** `crates/app/src/launch.rs` `Installed` (+ `write_installed`)
**Apply to:** `manifest.json`, `record.json`, `processes.json`: `version` field, `#[serde(default)]` on all later fields, tolerant read (`.ok().and_then(..)`), atomic write, treat contents as untrusted (path/dir validation).

### Additive trait methods with defaults
**Source:** `crates/host/src/lib.rs` (Phase 2 `processes/free_space/stuck_games`)
**Apply to:** `Session::processes`, `Host::kill_orphan` (+ `UnsupportedHost`, fake scripted outcomes).

### Additive store change
**Source:** `crates/store/src/lib.rs` `MIGRATIONS` + `Cancelling` precedent
**Apply to:** migration 4, `Interrupted`; always update `user_version` assertions and add a "vN database gains ..." test.

### Russian messages through one catalog
**Source:** `crates/pipeline/src/record/failure.rs` (`msg_*`, `FailCode::hint`)
**Apply to:** `TakeInvalid`, `RetryNoData`, verify reasons ("audio.wav: нет аудиопотока", "длительность 1.2 с, ожидалась 5.0 с (допуск ±0.5 с)").

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `renderer::rechain` (optional D-07 subset wave) | utility | transform | No existing text-surgery on generated cfgs; derive from `render` tail code (`lib.rs ~199-246`) with a byte-identical round-trip test. HLAE numbers new takes max+1, so subset re-record is optional; baseline = re-record all unless all validated. |
| stage-duration summary parser (`format_stage_line`/`parse_stage_line`) | utility | transform | No existing log-line round-trip pair; write both in one module with a round-trip unit test. |

## Metadata

**Analog search scope:** `crates/pipeline`, `crates/host`, `crates/store`, `crates/app/src`, `crates/renderer/src/lib.rs`
**Files scanned:** ~15 targeted reads/greps (git ls-files verified all are tracked)
**Pattern extraction date:** 2026-10-10
