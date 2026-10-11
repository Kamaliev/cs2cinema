# Phase 2: Pre-check, Cancel and Own Processes Only - Pattern Map

**Mapped:** 2026-10-10
**Files analyzed:** 27 new/modified files
**Analogs found:** 26 / 27 (all analogs are git-tracked source under `crates/`; no gitignored mirror paths)

Conventions that apply to every file below (CLAUDE.md): Russian user strings and doc comments, English identifiers; `Result<T, String>` at the pipeline/store boundary; `unsafe` blocks carry `// SAFETY:`; tests are sentence-named; non-Windows must still `cargo test --workspace` (stub `UnsupportedHost`); `cfg(windows)` code gets a non-Windows stub.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/host/src/lib.rs` (add `ProcInfo`, `WindowInfo`, `Host::{processes,free_space,stuck_games}`, `Session::{windows,wait_exit}`) | trait/model | request-response | itself (`Session`/`Host` traits, lines 143-198) | exact |
| `crates/host/src/win/windows.rs` (NEW: `EnumWindows` over job PIDs) | utility (FFI) | request-response | `crates/host/src/win/process.rs` `snapshot_processes` | role-match |
| `crates/host/src/win/disk.rs` or in `process.rs` (NEW: `GetDiskFreeSpaceExW`) | utility (FFI) | request-response | `crates/host/src/win/process.rs` `image_path`/`snapshot_processes` | role-match |
| `crates/host/src/win/mod.rs` (`WinSession::{windows,wait_exit}`, `STUCK_GAMES` registry, `WinHost::{processes,free_space,stuck_games}`) | service | request-response | itself (`WinSession` impl, lines 352-430) | exact |
| `crates/host/src/win/process.rs` (stuck-game helpers, exit wait in slices) | utility | request-response | itself (`OwnedProc::wait`, `liveness_of_pid`) | exact |
| `crates/host/src/fake.rs` (`windows_at`, `free_space`, `processes`, `exit_after_terminate`) | test fake | event-driven (virtual time) | itself (`Scenario` builders + `FakeSession`) | exact |
| `crates/host/src/unsupported.rs` (stubs for new trait methods) | stub | request-response | itself | exact |
| `crates/host/Cargo.toml` (windows-sys features) | config | n/a | itself | exact |
| `crates/host/tests/win_session.rs` (windows enumeration / exit-wait stand-in tests) | test | event-driven | itself | exact |
| `crates/pipeline/src/precheck/mod.rs` (NEW: `Level/Item/Report/Facts/evaluate/run`) | service (pure) | transform | `crates/pipeline/src/record/failure.rs` (message catalog + typed struct) | role-match |
| `crates/pipeline/src/precheck/steam.rs` (NEW: acf + steam.inf parsing) | utility | file-I/O, transform | `crates/app/src/launch.rs` `find_cs2` (vdf line scan) and `hlae_ffmpeg` (ini scan) | role-match |
| `crates/pipeline/src/precheck/space.rs` (NEW: size estimate constants) | utility | transform | `crates/pipeline/src/record/deadlines.rs` (single-place constant table + pure fns) | role-match |
| `crates/pipeline/src/record/failure.rs` (+ `PrecheckFailed`, `ForeignGameRunning`, `PrevGameStuck`) | model/catalog | transform | itself (`FailCode`, lines 14-76) | exact |
| `crates/pipeline/src/record/deadlines.rs` (+ `tool_probe`, `nvenc_probe`, `quit_wait`, `kill_confirm`, `exit_confirm`, `window_poll`) | config | transform | itself (`Deadlines::table`, `scaled`) | exact |
| `crates/pipeline/src/record/machine.rs` (+ `Input::{Windows,ForceCancel}`, AFXERROR immediate fail, WR-01/WR-05) | state machine | event-driven | itself (`step`, `on_event`, `check_deadline`) | exact |
| `crates/pipeline/src/record/mod.rs` (driver: windows poll, `shutdown()`, partial-take marking, exit confirmation) | driver | event-driven | itself (`record()` loop, lines 241-328) | exact |
| `crates/pipeline/src/record/markers.rs` (AFXERROR ignore-list helper) | utility | transform | itself (`is_afxerror`, line 98) | exact |
| `crates/pipeline/tests/record.rs` (new scenarios: cancel staged, stuck game, AFXERROR, modal window) | test | event-driven | itself (`Capture`, `cancel_during_demo_load_...`) | exact |
| `crates/pipeline/tests/` guard test `no_process_is_ever_stopped_by_image_name` | test | file-I/O | `crates/pipeline/tests/record.rs` (sentence-named, std::fs) | partial |
| `crates/store/src/lib.rs` (`MIGRATIONS[2]`, `last_good`, `precheck` column, `JobStatus::Cancelling`, recovery queries, `set_cancelling`) | model/migration | CRUD | itself (`MIGRATIONS`, `fail_job`, `cancel_job`, `reset_running`, tests lines 590-610) | exact |
| `crates/app/src/jobs.rs` (precheck first, two-pass space, cleanup guard, Cancelling flow, queue pause, `PREV_GAME_STUCK`) | controller | request-response | itself (`run`, `run_next`, `finish`, `run_ffmpeg`) | exact |
| `crates/app/src/launch.rs` (`nvenc_check` -> `Host::run`; `install_files` writes `installed.json`; cleanup fns) | service | file-I/O | itself (`install_files`, `nvenc_check`) + `jobs.rs::run_ffmpeg` for `Host::run` usage | exact |
| `crates/app/src/cleanup.rs` (NEW, optional: recovery + partial-take marking) | service | file-I/O | `crates/app/src/jobs.rs` `plan_hash` (dir walk + FNV) and `launch.rs::install_files` | role-match |
| `crates/app/src/app.rs` (`cancel_job` two-press, startup recovery next to `reset_running`, queue-paused flag) | controller | event-driven | itself (`cancel_job` lines 351-364, `reset_running` line 158) | exact |
| `crates/app/src/jobs_page.rs` (badge, disabled button, "Продолжить", precheck block) | component | request-response | itself (`JobStatus` match, lines 95-176) | exact |
| `crates/app/src/settings.rs` (+ `afxerror_ignore: Vec<String>`) | config | CRUD | itself (`#[serde(default)]`, test line 95-100) | exact |
| `crates/app/src/settings_page.rs` ("Проверить окружение" + report view) | component | request-response | itself (`small_button` rows, lines 41-75) | exact |
| `crates/pipeline/src/lib.rs` (re-export `precheck`) | config | n/a | itself (`pub use record::...`) | exact |

## Pattern Assignments

### `crates/host/src/lib.rs` (trait extension: `Session::windows/wait_exit`, `Host::processes/free_space/stuck_games`)

**Analog:** same file. New methods follow the existing contract style: doc comment states the failure mode, errors are `HostError`, "unknown" is never "alive".

**Trait shape to extend** (lines 143-198):
```rust
pub trait Session: Send {
    fn poll(&mut self, max_wait: Duration) -> Option<SessionEvent>;
    fn send(&mut self, line: &str) -> Result<(), HostError>;
    fn game_liveness(&mut self) -> Liveness;
    fn launcher_exit(&self) -> Option<Option<i32>>;
    fn terminate(&mut self);
}
pub trait Host: Send + Sync {
    fn launch(&self, spec: &LaunchSpec) -> Result<Box<dyn Session>, HostError>;
    fn now(&self) -> Duration;
    fn run(&self, cmd: &CmdSpec, cancel: &dyn Fn() -> bool, on_line: &mut dyn FnMut(&str)) -> Result<CmdOutput, HostError>;
}
```
Add new value types next to `CmdOutput` with the same derive line `#[derive(Debug, Clone, PartialEq)]` (e.g. `WindowInfo { pid, class, title, visible }`, `ProcInfo { pid, exe_name }`). Give default method bodies where a stub is natural so `UnsupportedHost`/fakes compile. Add a `HostError` Display arm in Russian for any new variant (see lines 85-101). Add tests in `mod tests` named as sentences (see `host_error_turns_into_a_russian_string`, lines 247-251).

---

### `crates/host/src/win/windows.rs` (NEW; utility, request-response, unsafe FFI)

**Analog:** `crates/host/src/win/process.rs` `snapshot_processes` (lines ~172-215) for FFI idiom: `// SAFETY:` per block, `last_error()` in errors, `String::from_utf16_lossy`.

**Imports pattern** (process.rs lines 5-24):
```rust
use std::os::windows::ffi::OsStringExt;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use super::job::Job;
use super::last_error;
use crate::{HostError, Liveness};
```

**FFI + SAFETY style** (process.rs `snapshot_processes`):
```rust
// SAFETY: флаги и PID=0 (все процессы); результат сравниваем с INVALID_HANDLE_VALUE.
let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
if snap == INVALID_HANDLE_VALUE {
    return Err(HostError::Io(format!("CreateToolhelp32Snapshot: {}", last_error())));
}
```
**Core pattern to implement** (from RESEARCH, verified on this machine): `EnumWindows` callback pushes `HWND` into a `Vec` through `lparam`; filter by `GetWindowThreadProcessId` in `Job::pids()` (`crates/host/src/win/job.rs:127`); only `GetWindowTextW`, never `SendMessage*`; modal candidate = visible + class `#32770`; also read `Static` children via `EnumChildWindows`; cap text to ~300 chars. Tests live in `#[cfg(test)] mod tests` with the `snapshot_finds_the_current_process` style, plus `crates/host/tests/win_session.rs` for a stand-in (`wscript` MsgBox) inside a real job.

---

### `crates/host/src/win/mod.rs` (`WinSession::windows/wait_exit`, stuck registry)

**Analog:** same file, `impl Session for WinSession` (lines 360-426).

**Handle-wait must not hold the `game` mutex long** (Anti-pattern in RESEARCH; the existing accessor locks per call):
```rust
fn game_liveness(&mut self) -> Liveness {
    match lock(&self.shared.game).as_ref() {
        Some(game) => game.liveness(),
        None => Liveness::NotStarted,
    }
}
```
Implement `wait_exit(timeout)` as a loop of <=100 ms slices, re-locking each slice through `lock(&self.shared.game)` and calling `OwnedProc::wait(Duration::from_millis(100))`, plus `Job::active_count()==0` (job.rs:179). Return `Liveness`; map errors to `Unknown`.

**Idempotent terminate pattern to keep** (lines 407-426): `terminated` flag, `self.job.terminate(1)` with `sink.note(...)` on error, `adopted_without_job` fallback with `game.kill(1)`. Stuck registry: `static STUCK_GAMES: Mutex<Vec<...>>` (use `crate::win::lock`), entries hold the `OwnedProc` plus creation time; `stuck_games()` re-checks each by handle.

---

### `crates/host/src/fake.rs` (new scenario builders; fake windows, free space, processes, exit-after-terminate)

**Analog:** same file; builder style on `Scenario` (lines 43-84 fields, builders 114-215).

**Field + builder pattern** (copy exactly):
```rust
/// Через сколько после `quit` игра выходит; `None` — `quit` игнорируется.
pub quit_after: Option<Duration>,
...
pub fn ignore_quit(mut self) -> Self { /* sets quit_after = None */ self }
pub fn line_at(mut self, at: Duration, source: Source, text: &str) -> Self { ... }
```
Add `windows: Vec<(Duration, Vec<WindowInfo>)>` + `windows_at(at, list)`, `free_space: Option<(PathBuf, u64)>`, `processes: Vec<ProcInfo>`, and `exit_after_terminate: Option<Duration>` where `None` means "never exits" (drives the `PREV_GAME_STUCK` test).

**Virtual time rule** (fake.rs `FakeSession::poll`, lines 481-512): only `poll` advances `st.now`; `wait_exit` on the fake must be built on `poll`-style clock advancement (set `st.now = limit` when the deadline passes), never `sleep`. The 10 h guard `MAX_VIRTUAL` panics on a hung scenario; keep it.

**Quit scheduling pattern** (fake.rs `send`, lines ~520-540):
```rust
} else if line == "quit" && let Some(after) = sc.quit_after {
    self.push(now + after, Some(SessionEvent::GameExited { code: Some(0) }), None);
    self.push(now + after, Some(SessionEvent::JobEmpty), None);
}
```
Counters for assertions follow `FakeHost::terminated()` / `sent()` / `launches()` (lines 258-270).

---

### `crates/host/src/unsupported.rs` (stubs)

**Analog:** same file (lines 8-30). New `Host` methods return `Err(HostError::Unsupported)` or an empty list; `run` already delegates to `runner::run_bounded`. Add one sentence-named test beside `unsupported_host_refuses_to_launch`.

---

### `crates/pipeline/src/precheck/mod.rs` (NEW; pure `Facts -> Report`)

**Analog:** `crates/pipeline/src/record/failure.rs` (typed result struct + Russian catalog in one file). `gather(host, inputs)` does I/O bounded by deadlines and a cancel closure; `evaluate(&Facts) -> Report` is pure and testable on ubuntu.

**Typed code + hint catalog pattern** (failure.rs lines 14-76):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailCode { LauncherFailed, ..., HlaeError, Cancelled }
impl FailCode {
    pub fn as_str(self) -> &'static str { match self { FailCode::HlaeError => "HLAE_ERROR", ... } }
    pub fn hint(self) -> &'static str { match self { FailCode::LauncherFailed => "Проверьте папку HLAE в настройках и ...", ... } }
}
```
**Result-struct pattern** (failure.rs lines 79-120): struct of plain fields with `new(...)` filling `hint: code.hint().to_owned()`. Mirror for `Item { id, level, message, hint }`; derive `Serialize, Deserialize` (it is persisted as JSON on the job row, CLAUDE.md derive rule). Tool probes (`ffmpeg -version`, `ffprobe -version`, `ffmpeg.ini`) go through `Host::run` with `CmdSpec { program, args, cwd: None, deadline }` exactly like `run_ffmpeg` in `crates/app/src/jobs.rs:274-290`.

**Reuse existing checks, do not duplicate:** HLAE/hook/cs2 file checks and messages from `launch::check_paths` (launch.rs lines 95-107):
```rust
if !Path::new(hlae_dir).join("HLAE.exe").is_file() { return Err("Укажите папку HLAE в настройках (там должен лежать HLAE.exe)".into()); }
if !Path::new(hlae_dir).join("x64/AfxHookSource2.dll").is_file() { ... }
if !cs2_exe(cs2_dir).is_file() { ... }
```
Output path check reuses `renderer::console_safe(&pipeline::absolute_slash(&out_dir))` (jobs.rs lines 216-218). Demo header sniff: magic bytes `PBDEMS2\0` (see `lfs_pointer_gives_clear_error` in `crates/cli/tests/pipeline.rs`).

---

### `crates/pipeline/src/precheck/steam.rs` (NEW; acf / steam.inf scan)

**Analog:** `crates/app/src/launch.rs` `find_cs2` vdf line scan (lines 17-33) and `hlae_ffmpeg` ini scan (lines 117-120). Do not add a VDF crate.

**Line-scan idiom** (launch.rs lines 17-26):
```rust
for line in vdf.lines() {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("\"path\"") {
        let p = rest.trim().trim_matches('"').replace("\\\\", "/");
        libs.push(p.into());
    }
}
```
Use the same shape for `"StateFlags"`, `"buildid"`, `"TargetBuildID"`, `"BytesToDownload"`, `"BytesDownloaded"`; `steam.inf` `PatchVersion=`/`ClientVersion=` mirrors `geometry::game_version` (`crates/geometry/src/lib.rs:286`). Manifest path = `<cs2_dir>/../../appmanifest_730.acf`. Missing/unparsable -> `Warn`, never `Fail`. Tests: sentence-named, pure strings in, bit-mapping out.

---

### `crates/pipeline/src/precheck/space.rs` (NEW; estimate constants)

**Analog:** `crates/pipeline/src/record/deadlines.rs` — one table, one place, pure functions, finite-guard.

**Pattern** (deadlines.rs lines 76-120): constants in a table, `take_run(&self, t: &TakePlan) -> Duration` computes from `TakePlan.game_secs` and `capture_fps`; `fn finite(x: f64) -> f64` guards NaN. Follow it: `pub fn estimate_bytes(takes: &[TakePlan], size: (u32,u32)) -> u64` with `const MB_PER_GAME_SEC_1080P`, scaled by pixel area, `+ AUDIO_MB_PER_SEC`, margin `1.5`; `fail` if free < estimate, `warn` if free < estimate*1.5 (D-03). Pass-1 floors (3 GiB results / 1 GiB CS2) are constants in the same file. Shared-volume case sums both needs.

---

### `crates/pipeline/src/record/failure.rs` (new codes)

**Analog:** same file. Add variants to `FailCode`, an arm in `as_str` (`"PRECHECK_FAILED"`, `"FOREIGN_GAME_RUNNING"`, `"PREV_GAME_STUCK"`), an arm in `hint` (Russian), and `msg_*` fns in the same free-function style as lines 184-289:
```rust
pub fn msg_game_not_closed(limit: Duration) -> String {
    format!("игра не закрылась за {} с — завершаю свой job", secs(limit))
}
pub fn msg_hlae_error(afx: &str, state: RecordState, limit: Duration) -> String { ... }
```
All Russian text stays in this file; callers pass facts. Add an immediate-fail message variant for `HLAE_ERROR` (first AFXERROR, no deadline text) and one that appends window titles/text (cleaned through `record::log::clean`).

---

### `crates/pipeline/src/record/deadlines.rs` (new fields)

**Analog:** same file. A new field must be added in four places: the struct (line 17-47), `table` (lines 54-72), `scaled` (lines 88-107), and the sentence test. **Constraint from RESEARCH:** `quit_wait` (5 s) and `kill_confirm` must NOT be scaled by "Запас по времени" (would break the 10 s cancel target at x5); implement by copying them unchanged in `scaled` (`quit_wait: self.quit_wait`). `tool_probe`/`nvenc_probe` may be scaled.

---

### `crates/pipeline/src/record/machine.rs` (AFXERROR immediate fail, window input, WR-01/WR-05)

**Analog:** same file.

**Input/Action enums to extend** (lines 98-122): add `Input::Windows(Vec<WindowInfo>)` and `Input::ForceCancel`; keep `Action` unchanged where possible (shutdown lives in the driver, not the machine).

**Current AFXERROR capture (to change from "remember" to "fail now")** (lines 302-305):
```rust
SessionEvent::Line { text, .. } => {
    if self.afx.is_none() && markers::is_afxerror(&text) {
        self.afx = Some(log::clean(&text));
    }
```
Replace with: if not matched by the ignore substrings (settings, passed into `Machine::new`/a setter), `let f = self.fail(now, FailCode::HlaeError, self.entered, failure::msg_hlae_error_now(...)); acts.push(f);`.

**Fail helper** (lines 258-263): `fn fail(&mut self, now, code, base, message) -> Action` sets `finished = true` and returns `Action::Finish(Err(..))`.

**Cancel path today** (lines 227-236) emits `Action::Terminate` first and then `Finish`; with staged shutdown the machine only emits `Finish(Err(Cancelled))` and the driver decides graceful vs immediate.

**WR-01** (`on_send_failed`, lines 342-348): for `line == "quit"` stop doing `Terminate + finish_ok`; stay in `Closing` and resend on next `ConsoleUp`. **WR-05** (`check_deadline`, lines 514-530): `GameExited` + `close_settle` -> `finish_ok` becomes "wait for `JobEmpty` up to `game_close`, then success"; keep `close_settle` only as fallback when the host never reports `JobEmpty`.

Tests in this file: the `finish(&acts)` helper (line 625) and the `Action::Terminate` assertion style (line 743).

---

### `crates/pipeline/src/record/mod.rs` (driver: windows poll, staged `shutdown`, `.partial`, exit confirm)

**Analog:** same file, `record()` (lines 241-328).

**Loop skeleton to extend** (lines 262-326): `session.0.poll(TICK)` -> `p.cancelled()` -> `machine.step(now, input)` -> drain `queue: VecDeque<Action>`. Add: every 1 s (`now - last_win >= window_poll`) call `session.0.windows()` and feed `Input::Windows`; a second cancel signal (force flag, a new method on `Progress` or a second closure) maps to `Input::ForceCancel`.

**Terminal arms to rewrite** (lines 300-320):
```rust
Action::Finish(Err(mut f)) => {
    f.game = liveness.unwrap_or_else(|| session.0.game_liveness());
    f.launcher = machine.launcher();
    session.0.terminate();
    j.failure(host.now(), p, &mut f);
    return Err(f);
}
```
Insert `shutdown(mode)` before `j.failure(...)` so `console_tail` includes lines that arrive during shutdown (Pitfall 8); inside it keep calling `session.0.poll(TICK)` and `j.session_event(...)` so late lines reach `record.log`. Sequence: `send("quit")` if console up -> wait <=`quit_wait` (failure/cancel) or `game_close` for `JobEmpty` (success) -> `terminate()` -> `wait_exit(kill_confirm)` -> settle 3 s -> mark unfinished `takeNNNN` as `takeNNNN.partial` (retry 3 x 500 ms, then log "уборка отложена") -> if still alive after 30 s register stuck game.

**Guard (kept)** (lines 83-89): `Guard(Box<dyn Session>)` with `Drop -> terminate()`; keeps the safety net for panics.

**Journal usage:** `j.line(now, "app", "...")` / `j.emit(now, p, &Event::Fact(..))` for every shutdown step so the cleanup is visible in `record.log`.

---

### `crates/pipeline/src/record/markers.rs` (ignore-list helper)

**Analog:** same file line 98: `pub fn is_afxerror(line: &str) -> bool { line.contains("AFXERROR") }`. Add `is_ignored_afxerror(line, ignore: &[String]) -> bool` using substring match only (no regex from settings, ASVS V5). Test next to the existing one (lines 167-170).

---

### `crates/pipeline/tests/record.rs` (new scenarios)

**Analog:** same file. Reuse `Capture` (lines 45-103) incl. `cancelling_after(state)`; scenario tests use `FakeHost::new(Scenario::happy(n)....)` + `record(&fake, &req, &cap)`.

**Template** (lines 411-420):
```rust
#[test]
fn cancel_during_demo_load_returns_cancelled_and_terminates() {
    let fake = FakeHost::new(Scenario::happy(3).drop_marker(FakeMarker::Ready));
    let cap = Capture::cancelling_after(RecordState::Demo);
    let req = request("cancel", 3);
    let f = record(&fake, &req, &cap).expect_err("отмена");
    assert_eq!(f.code, FailCode::Cancelled);
    assert!(fake.terminated() >= 1, "сессия завершена");
}
```
New tests, sentence-named: first AFXERROR fails immediately with `HLAE_ERROR` (no deadline wait), modal window text lands in the message, cancel with `ignore_quit()` ends via terminate within the budget (assert on `fake.now()`), `exit_after_terminate(None)` produces a stuck game, `.partial` rename happens for an unfinished take (use `fresh(name, n)` at lines 427-432 for a clean dir and `read_log` for `record.log` assertions).

---

### `crates/store/src/lib.rs` (migration 3, `Cancelling`, `last_good`, recovery queries)

**Analog:** same file.

**Migration list** (lines 220-232): append a third element only; "добавлением колонок" rule:
```rust
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE jobs ADD COLUMN fail_code TEXT; ...",
    "ALTER TABLE jobs ADD COLUMN fail_elapsed_ms INTEGER;
     ALTER TABLE jobs ADD COLUMN phase TEXT NOT NULL DEFAULT '';",
    // NEW: "CREATE TABLE last_good (...); ALTER TABLE jobs ADD COLUMN precheck TEXT;",
];
```
Update the comment above it (version 3) and the two tests that hard-code `user_version == 2` (lines 583, 586, 601, 607) to 3; add a `version_two_database_gains_last_good` test cloned from `version_one_database_gains_elapsed_and_phase` (lines 590-610: `TempDb::new`, apply `SCHEMA` + `MIGRATIONS[..2]`, `PRAGMA user_version = 2`, open, assert, reopen idempotent).

**Status enum** (lines 44-70): add `Cancelling` to `JobStatus`, a `title()` arm (`"отменяется"`) and `parse` arm `"cancelling"`. Note `parse` falls back to `Failed` (old builds), which is the documented one-way risk (D-11).

**State-transition SQL style** (lines 390-407, 427-433):
```rust
pub fn cancel_job(&self, id: i64) -> Result<()> {
    self.with(|c| {
        c.execute("UPDATE jobs SET status = 'cancelled', stage = '', phase = '', updated_at = ?2 WHERE id = ?1", params![id, now()])?;
        Ok(())
    })
}
pub fn reset_running(&self) -> Result<usize> { ... WHERE status = 'running' ... }
```
Add `set_cancelling(id)`, `set_precheck(id, json)`, `set_last_good(build, hlae)`/`last_good()`, and make `reset_running` cover `cancelling` -> `cancelled` (not "ошибка") and return the affected job ids/out_dirs (new fn `interrupted_jobs()` returning `(id, out_dir)` before the reset) so startup recovery (D-08) can read `installed.json`. `next_queued` (line 328) must not pick `cancelling`.

---

### `crates/app/src/jobs.rs` (orchestration)

**Analog:** same file.

**Precheck insertion point** (`run`, line 164, before `pipeline::analyze` at line 173): guard `if job.kind != JobKind::Plan { precheck }`; on `fail` return `Err(RunError { message, fail: Some(RecordFailure::new(FailCode::PrecheckFailed, ...)) })` so `finish()` (lines 106-112) persists a typed failure via `job_failure`. Persist the report with `store.set_precheck(job.id, json)`; `warn` items go through `p.log(...)`.

**Typed error plumbing** (lines 36-58, 93-112): `RunError { message, fail: Option<RecordFailure> }` + `finish()` branches (`CANCELLED` sentinel -> `cancel_job`; typed -> `fail_job`; plain -> `finish_job`). Extend the cancel branch to the `Cancelling` -> `cancelled` path and set the in-memory `queue paused` flag via a `Msg` (see `Msg::JobDone` at line 158).

**Replace nvenc probe** (lines 180-190): `launch::nvenc_check(Path::new("ffmpeg"))` and `launch::has_nvenc(&f)` become `Host::run`-based with `nvenc_probe` deadline and `|| p.cancelled()`; the model is `run_ffmpeg` (lines 274-290):
```rust
let cmd = CmdSpec { program: "ffmpeg".into(), args: all, cwd: Some(dir.to_path_buf()), deadline };
let out = host.run(&cmd, &|| p.cancelled(), &mut on_line).map_err(|e| ffmpeg_error(&e, deadline))?;
```
and error mapping `ffmpeg_error` (lines 262-270; `HostError::Cancelled => pipeline::CANCELLED.to_owned()`).

**Cleanup guard around record** (lines 233-248): `install_files` + `record_stage` currently leave files behind. Wrap in a scope guard / `let r = ...; cleanup(...); r?` so demo copy + cfg/xml are removed on success, failure and cancel, after `shutdown` confirmed the exit. `install_files` result drives the `installed.json` write.

**Per-task host** (line 227 `let host = pipeline::host::default_host();`): stuck-game registry must be process-wide (static in `host::win`), because this builds a new host per task.

**Next-task wait (TMO-07/D-14)** belongs in `run_next` (lines 139-161) before `std::thread::spawn`, or at the start of `run` before `record`: wait on own game handle, 3 s pause, `PREV_GAME_STUCK` after 30 s.

---

### `crates/app/src/launch.rs` (`nvenc_check`, `install_files`)

**Analog:** same file.

**`install_files` today** (lines 108-123) returns `Result<(), String>` and discards the list of copied files. Change to also return/write the installed list: collect `cfg_dir.join(&name)` for each copy and the demo path `csgo.join(format!("{demo_name}.dem"))`, then write `out_dir/installed.json` (`serde_json::to_string_pretty`, same style as `settings.rs::save`). Cleanup fn must canonicalize and require the `<cs2_dir>/game/csgo` prefix and exact name patterns (`cs2cinema_*.dem`, `cs2c_*.cfg|xml`, `highlights.cfg`), treating `installed.json` as untrusted input (ASVS V12).

**`nvenc_check` today** (lines 138-157) is the WR-03 site:
```rust
let mut c = Command::new(ffmpeg);
c.args([... "-c:v", "h264_nvenc", "-f", "null", "-"]) ... ;
let out = c.output().map_err(|e| format!("ffmpeg не запустился: {e}"))?;
```
Keep the arg list and the stderr-line parsing (lines 150-156); swap `Command::output()` for `host.run(&CmdSpec{..deadline..}, cancel, &mut on_line)` collecting `CmdOutput.stderr_tail`. Signature gains `host: &dyn Host`, `deadline`, `cancel`. `no_window` (lines 8-11) is no longer needed for this call (runner handles it).

---

### `crates/app/src/app.rs` (cancel, startup recovery, queue pause)

**Analog:** same file.

**Cancel today** (lines 351-364):
```rust
pub fn cancel_job(&mut self, id: i64) {
    match &self.jobs.running {
        Some((rid, token)) if *rid == id => { token.cancel(); self.status.push("отменяю задачу…"); }
        _ => { if let Err(e) = self.store.cancel_job(id) { self.status.push(&e); } }
    }
    self.refresh_jobs();
}
```
Change: first press on a running job -> `store.set_cancelling(id)` synchronously (satisfies the 3 s target) + `token.cancel()`; second press when status is already `Cancelling` -> `token.force()` (new `AtomicBool` on `CancelToken` in `jobs.rs` lines 25-35). **Startup recovery:** at line 158 `let interrupted = store.reset_running().unwrap_or(0);` call the new cleanup of `running`/`cancelling` jobs first (read `installed.json`, delete files, mark `.partial`). **Queue pause:** add `paused: bool` to `JobsState`; `run_next` (jobs.rs line 140) returns early if paused; set on `Cancelled` result, cleared by "Продолжить" or explicit enqueue (A6/Q3: in-memory only).

---

### `crates/app/src/jobs_page.rs` (badge, buttons, precheck block)

**Analog:** same file. Every `match j.status` must gain a `Cancelling` arm (lines 95-121 text/badge, 126-127 ladder, 153-174 buttons, 205-206 ladder state, 273 icon). Button pattern (lines 153-157):
```rust
JobStatus::Queued | JobStatus::Running => {
    if ui.button("Отменить").clicked() { ... Act::Cancel(id) ... }
}
```
For `Cancelling`: `ui.add_enabled(false, egui::Button::new("Отменяется…"))` plus (D-12) the second press stays reachable via a separate `small_button("Принудительно")`; "Продолжить" banner uses `app.jobs.paused`. The `Delete` guard at line 176 (`j.status != JobStatus::Running`) must also exclude `Cancelling`. Precheck block reuses the failure-card layout used for `fail_code` (line 130 `if j.status == JobStatus::Failed && j.fail_code.is_some()`).

---

### `crates/app/src/settings.rs` (+ `afxerror_ignore`)

**Analog:** same file; struct is `#[serde(default)]` (line 5-6), `Default` impl lines 40-61, tolerance test lines 91-100:
```rust
#[test]
fn old_settings_file_with_load_wait_still_loads() {
    let s: Settings = serde_json::from_str(r#"{"hlae_dir":"C:/hlae","load_wait_secs":40,"fps":90}"#).unwrap();
    ...
}
```
Add `pub afxerror_ignore: Vec<String>` (default `vec![]`), and extend the test (or add a sibling) so a file without the key still loads.

---

### `crates/app/src/settings_page.rs` ("Проверить окружение")

**Analog:** same file; button rows use `ui.small_button("Найти CS2 в Steam").clicked()` (line 63) with state kept on `App`. The button spawns the shared precheck via `App::spawn` (`app.rs:220` detached thread -> `Msg`), reusing the exact `pipeline::precheck::run` used by `jobs::run`. Render items with the same ok/warn/fail coloring helper used for `path_row` bad hints (lines 174-190).

---

## Shared Patterns

### Own-process control (TMO-05)
**Source:** `crates/host/src/win/job.rs` (`Job::new/assign_*/pids/active_count/terminate`), `crates/host/src/win/process.rs` (`OwnedProc::open/open_owned/wait/kill`), `crates/host/src/win/watch.rs::pick_adoptee`
**Apply to:** every new termination/wait code. Never match or kill by image name. PRE-05 may detect `cs2.exe` by name in `snapshot_processes()` only to refuse to start.
```rust
// process.rs:96-110 -- unknown is never Alive
pub fn wait(&self, timeout: Duration) -> Liveness {
    let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
    // SAFETY: handle открыт нами и жив до конца `self`.
    let r = unsafe { WaitForSingleObject(self.handle, ms) };
    match r { WAIT_TIMEOUT => Liveness::Alive, WAIT_OBJECT_0 => { ... Liveness::Exited(..) } ... other => Liveness::Unknown(..) }
}
```
Guard test: a sentence-named test that scans `crates/**/*.rs` (excluding itself) for `taskkill`/`tasklist` (RESEARCH "Source-grep guard").

### Deadlines and cancel on every external call
**Source:** `crates/app/src/jobs.rs::run_ffmpeg` (lines 274-290) + `crates/host/src/runner.rs::run_bounded`
**Apply to:** `ffmpeg -version`, `ffprobe -version`, NVENC probes, any new tool call in precheck. Always `CmdSpec.deadline` + `&|| p.cancelled()`; map `HostError::Timeout/Cancelled/Spawn` to Russian text as in `ffmpeg_error` (lines 262-270).

### Progress / cancel boundaries
**Source:** `crates/pipeline/src/lib.rs` (`Progress::stage/log/event/cancelled`, `check(p)?`, `CANCELLED`), `crates/app/src/jobs.rs` `JobReporter` (lines 60-91)
**Apply to:** precheck run (log each warn/fail via `p.log`, `pipeline::check(p)?` between pass steps), shutdown, cleanup.

### Typed failure with Russian message + hint
**Source:** `crates/pipeline/src/record/failure.rs` (`FailCode::{as_str,hint}`, `RecordFailure::new`, `msg_*`) and `jobs.rs::job_failure` (lines 94-103)
**Apply to:** `PRECHECK_FAILED`, `FOREIGN_GAME_RUNNING`, `PREV_GAME_STUCK`, new `HLAE_ERROR` message.

### Untrusted text sanitising
**Source:** `crates/pipeline/src/record/log.rs::clean` (used in machine.rs line 304 `log::clean(&text)`)
**Apply to:** window titles and dialog text from foreign processes before they reach the journal, DB or UI; add a ~300 char cap (IN-04).

### Fake-time testing
**Source:** `crates/host/src/fake.rs` (`Scenario` builders, virtual clock via `poll`), `crates/pipeline/tests/record.rs` (`Capture`, `request`, `fresh`, `read_log`)
**Apply to:** every precheck/shutdown/cancel scenario; no `sleep` in tests; Windows-only real tests go into `crates/host/tests/win_session.rs` under `#![cfg(windows)]` with stand-in processes (`cmd`, `ping`, `wscript`).

### SQLite migration discipline
**Source:** `crates/store/src/lib.rs` lines 220-262 and tests 590-610
**Apply to:** the single new migration: append-only, additive, tested from a v2 database, `user_version` assertions bumped.

### Persisted JSON structures
**Source:** `crates/app/src/settings.rs` (`#[serde(default)]`, `serde_json::to_string_pretty`) and `plan_hash`/`clips/plan.hash` in `jobs.rs`
**Apply to:** `installed.json`, precheck report JSON, `afxerror_ignore`: serde derives, tolerant reading, never panic on malformed files.

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `crates/host/src/win/windows.rs` (EnumWindows / GetWindowTextW) | utility | request-response | No window-enumeration code exists in the repo; use `process.rs` FFI idiom for structure and the verified code in RESEARCH "Enumerate windows of the job's processes". Needs new `windows-sys` features (`Win32_UI_WindowsAndMessaging`, `Win32_Storage_FileSystem`). |
| Static `STUCK_GAMES` registry (process-wide) | state | n/a | No process-wide static state in `host` today besides `OnceLock<Instant>` in `unsupported.rs`; use `std::sync::Mutex` + `crate::win::lock` poisoning-tolerant helper (`win/mod.rs:41`). |
| Job Object IOCP | n/a | n/a | Explicitly not adopted (RESEARCH): keep 250 ms poll of `Job::active_count()`. |

## Metadata

**Analog search scope:** `crates/host`, `crates/pipeline` (src + tests), `crates/store`, `crates/app/src`, `crates/cli/tests`
**Files scanned/read:** about 20 (host lib/process/mod/fake/unsupported/Cargo, record deadlines/failure/machine/mod/markers/tests, store lib, app jobs/launch/settings/app/jobs_page/settings_page)
**Tracked-source gate:** all named analogs are under `crates/` and tracked (`git ls-files` confirmed for host, pipeline/src/record, store, app/src/{jobs,launch}.rs); none are gitignored `.gsd` mirrors.
**Pattern extraction date:** 2026-10-10
