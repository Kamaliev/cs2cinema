# Phase 4: Очередь без присмотра и приёмка - Pattern Map

**Mapped:** 2026-10-11
**Files analyzed:** 26 new/modified files
**Analogs found:** 26 / 26 (all analogs are git-tracked `crates/*` sources; verified `git ls-files` for jobs.rs, fake.rs; no mirror paths)

Conventions (CLAUDE.md): Russian user strings/doc comments, English identifiers; `Result<T, String>` at pipeline/store boundary; `Box<dyn Error>` in cli; sentence-named tests; `#[serde(default)]` on any field added to a persisted struct; `cfg(windows)` code needs a non-Windows stub (`UnsupportedHost` / trait default); external processes only via `Host::run`; no `tasklist`/`taskkill`. Line numbers are as of 2026-10-11 and drift after edits; re-locate by symbol. App files are formatted with rustfmt `max_width=160,use_small_heuristics=Max` (not `cargo fmt -p app`).

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/pipeline/src/job.rs` (NEW: `JobConfig`, `JobSpec`, `Outcome`, `RunError`, `run_job`, resume path) | service/orchestrator | request-response | `crates/app/src/jobs.rs` `run`/`resume_job`/`RunError` (MOVE) | exact (move) |
| `crates/pipeline/src/assemble.rs` (NEW: `assemble_stage`, `run_ffmpeg`, `part_args`, ...) | service | request-response | `crates/app/src/jobs.rs` lines ~588-760 (MOVE) | exact (move) |
| `crates/pipeline/src/gamedir.rs` (NEW: `install_files`, `cleanup_installed`, `note_take_done`, `recover`, `kill_orphans`) | service | file-I/O | `crates/app/src/launch.rs` (MOVE) | exact (move) |
| `crates/pipeline/src/lib.rs` (+ `Progress::artifact`, `Artifact`, `pub mod job/assemble/gamedir`) | trait | event-driven | `Progress::event` default method in same file (lines 49-64) | exact |
| `crates/pipeline/src/record/failure.rs` (+ `FailClass`, `classify`) | model | transform | `FailCode` + `as_str` in same file | exact |
| `crates/pipeline/src/record/mod.rs` (`verify` mandatory) | driver | event-driven | itself (`RecordRequest.verify: Option<Verify>`) | exact |
| `crates/pipeline/tests/job.rs` (NEW; tests moved from app) | test | request-response | `crates/app/src/jobs.rs` tests (`healthy_settings`, `running_job`) + `crates/pipeline/tests/record.rs` | exact |
| `crates/app/src/queue.rs` (NEW: `QueuePolicy`, `PauseReason`, `Verdict`, `Summary`, worker loop) | service/thread | event-driven | `crates/app/src/jobs.rs` `run_next` + `pauses_queue` | role-match |
| `crates/app/src/jobs.rs` (shrinks to adapters) | controller | request-response | itself (`JobReporter`, `finish`, `job_failure`) | exact |
| `crates/app/src/launch.rs` (DELETE after move) | - | - | - | - |
| `crates/app/src/app.rs` / `jobs_page.rs` / `settings_page.rs` (shared queue view, acceptance button, banners) | component | event-driven | `PauseReason`/`pause_banner`, `Act` enum | exact |
| `crates/store/src/lib.rs` (migration 5, `requeue_auto`, `set_batch`, `open_read_only`) | model/migration | CRUD | `MIGRATIONS[3]`, `requeue_with`, `Store::open` | exact |
| `crates/store/src/journal.rs` (`StartKind::Auto`, `attempts`) | utility | transform | `StartKind` + `separator`/`SEPARATOR_MARK` | exact |
| `crates/host/src/lib.rs` (`Host::notify` default no-op) | trait | request-response | `Host::kill_orphan` default method (lines ~327-330) | exact |
| `crates/host/src/win/notify.rs` (NEW) + `win/mod.rs` + `Cargo.toml` feature `Win32_UI_Shell` | utility (FFI) | event-driven | `crates/host/src/win/process.rs` (SAFETY comments, `last_error`) | role-match |
| `crates/host/src/fake.rs` (scenario sequence, `notifications()`) | test fake | event-driven | itself (`FakeHost::new`, `launches()`, `orphan_kills`) | exact |
| `crates/host/src/unsupported.rs` | stub | n/a | itself | exact |
| `crates/cli/src/main.rs` (+ `record`/`retry`/`acceptance` dispatch) | controller | request-response | itself (`assemble_cmd`, `main` dispatch, `USAGE`) | exact |
| `crates/cli/src/record.rs` (NEW: `ConsoleProgress`, args -> `JobSpec`) | controller | request-response | `assemble_cmd` arg loop + `pipeline::Quiet` | role-match |
| `crates/cli/src/acceptance.rs` (NEW: set parsing, `evaluate`, `state`) | service | batch | `crates/pipeline/src/record/verify.rs` (`judge`, `parse_probe`) + `faceit::parse_match_id` | role-match |
| `crates/cli/Cargo.toml` (+ `pipeline`, `store`) | config | n/a | itself | exact |
| `acceptance/matches.txt` (NEW) | config/data | n/a | none (see No Analog) | none |
| `crates/app/src/jobs.rs` tests -> `queue.rs` tests | test | event-driven | `jobs.rs` tests (`running_job`, `typed_failure_reaches_the_job_row`) | exact |
| `crates/store/src/lib.rs` tests (migration 5) | test | CRUD | `version_three_database_gains_attempt_and_resume` (~line 780) | exact |

## Pattern Assignments

### `crates/pipeline/src/job.rs` (orchestrator, request-response) - MOVE of `app/jobs.rs::run`

**Analog:** `crates/app/src/jobs.rs`. Move symbols per the RESEARCH move table; replace `Settings`/`Store`/`JobReporter` with `JobConfig`/`JobSpec`/`&dyn Progress`.

**RunError to move verbatim** (jobs.rs 61-79):
```rust
/// Ошибка задачи: текст для строки задачи и, если это сбой записи, его типизированное описание.
#[derive(Debug)]
pub struct RunError { pub message: String, pub fail: Option<RecordFailure> }
impl From<String> for RunError { fn from(message: String) -> Self { RunError { message, fail: None } } }
impl From<&str> for RunError { fn from(message: &str) -> Self { RunError::from(message.to_owned()) } }
```

**Entry/branch pattern** (jobs.rs ~341-346):
```rust
fn run(host: &dyn Host, job: &Job, s: &Settings, store: &Store, p: &JobReporter) -> Result<JobOutcome, RunError> {
    // «Повторить»: всё берётся из record.json в папке задачи, без скачивания, разбора и плана (RET-02, D-08)
    if job.resume { return resume_job(host, job, s, store, p); }
```
Becomes `pub fn run_job(host: &dyn Host, spec: &JobSpec, p: &dyn Progress) -> Result<Outcome, RunError>` branching on `spec.resume`.

**Store side effects become `Progress::artifact`** (calls to replace: `store.remember_demo`, `set_out_dir`, `set_precheck`, `set_last_good`, `last_good()`, `demo_path()`). Inputs go into `JobSpec { known_demo, last_good, out_dir_hint, attempt, job_id }`.

**JobReporter.event decorator** (jobs.rs 143-151) that calls `launch::note_take_done(dir, n.saturating_sub(1))` on `Event::TakeDone` must become a small wrapper `Progress` inside `record_takes` (pipeline) because `launch` moves to `gamedir`:
```rust
if let (pipeline::Event::TakeDone { n, .. }, Some(dir)) = (e, self.out_dir.get()) {
    // номер дубля в событии с единицы, в `installed.json` и в имени `takeNNNN` — с нуля
    launch::note_take_done(dir, n.saturating_sub(1));
}
```
`JobConfig` replaces `Settings` fields: `hlae_dir, cs2_dir, out_root, fps, chronological, flybys, walls, detailed, gpu_encode, frame/resolution, time_margin, afxerror_ignore, faceit_key`; derive `Debug, Clone, Default`. Test helper `healthy_settings(tag)` (jobs.rs 978-988, builds a fake HLAE/CS2 tree under `temp_dir`) becomes `healthy_config(tag) -> JobConfig` in `pipeline/tests/job.rs`.

---

### `crates/pipeline/src/gamedir.rs` (service, file-I/O) - MOVE of `app/launch.rs`

**Analog:** `crates/app/src/launch.rs` (642 lines; 1-359 non-test). Move `Installed`, `install_files`, `is_ours`, `cleanup_installed`, `remove_with_retry`, `note_take_done`, `kill_orphans`, `finished_takes`, `recover`; keep `recover_after_crash` in app (uses `Store`) as thin glue. `nvenc_check`/`has_nvenc`/`hlae_ffmpeg` go to `pipeline::precheck`. `find_cs2`/`find_hlae`/`cs2_exe` -> `pipeline::discover` (discretion). `no_window` -> `ui.rs`; `old_logs` -> `jobs_page.rs`. Keep the `is_ours` path guard intact (security). Bounded external-call template (launch.rs `nvenc_check`):
```rust
let cmd = CmdSpec { program: ffmpeg.to_path_buf(), args: args.map(String::from).into(), cwd: None, deadline };
match host.run(&cmd, cancel, &mut |_| {}) { ... }
```
Tests move with the code (~14 in launch.rs) using `pipeline::host::fake`. Grep gate after move: `grep -rn "tasklist\|taskkill" crates` must be empty.

---

### `crates/pipeline/src/assemble.rs` (service) - MOVE of `app/jobs.rs` 588-760

Symbols: `assemble_stage` (637), `run_ffmpeg` (728), `part_args` (588), `check_take_for_assembly` (606), `take_invalid_failure` (680), `ffmpeg_error` (717), `const PART_NAME = "highlights.part.mp4"` (602). Signature to preserve:
```rust
fn run_ffmpeg(host: &dyn Host, dir: &Path, args: &[String], secs: f32, deadline: Duration, p: &dyn Progress) -> Result<(), String>
```
Error style: Russian text built from `stderr_tail` (`out.stderr_tail.iter().rev().take(6)`), `HostError` mapped via `ffmpeg_error(e, deadline)`. Do NOT use `Command::output()`; this is the "duplicated ffmpeg invocation" fix so `renderer::assemble` can be deleted (DC-1).

---

### `crates/pipeline/src/lib.rs` (+ `Progress::artifact`)

**Analog:** `Progress` trait (lib.rs 49-64): every non-core method has a default so `Quiet` and tests need no change.
```rust
pub trait Progress: Sync {
    fn stage(&self, s: Stage);
    fn log(&self, msg: &str);
    fn cancelled(&self) -> bool { false }
    fn force_cancelled(&self) -> bool { false }
    /// Типизированное событие записи; по умолчанию — строка журнала.
    fn event(&self, e: &record::Event) { self.log(&e.to_string()) }
}
```
Add `fn artifact(&self, _a: &Artifact) {}` (default no-op) with `pub enum Artifact { Demo { match_id: String, path: PathBuf }, OutDir(PathBuf), Precheck(String), LastGood { cs2_build: String, hlae_version: String } }`. `JobReporter` (app) implements it by calling the corresponding `Store` setters. `CANCELLED`/`check(p)?` unchanged.

---

### `crates/pipeline/src/record/failure.rs` (+ `FailClass`, `classify`)

**Analog:** `FailCode` enum (failure.rs 14-36) and `as_str`. Add next to it:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailClass { Launch, Environment, Other }
pub fn classify(code: FailCode, state: RecordState) -> FailClass {
    use FailCode::*;
    match code {
        GameNotStarted | ConsoleNotOpen | DemoNotLoaded => FailClass::Launch,
        GameExited if state < RecordState::Take => FailClass::Launch,
        PrecheckFailed | ForeignGameRunning | PrevGameStuck | HlaeError | LauncherFailed => FailClass::Environment,
        ScriptNotRun | TakeTimeout | GameExited | TakeInvalid | RetryNoData | Cancelled => FailClass::Other,
    }
}
```
(`RecordState` derives `PartialOrd, Ord` in order Precheck..Done, machine.rs 25-37.) Exhaustive `match` on `FailCode` so a new code forces a classification decision. Test: sentence names, e.g. `game_exit_before_take_is_launch_but_mid_take_is_not`.

---

### `crates/app/src/queue.rs` (service/thread, event-driven)

**Analog:** `run_next` (jobs.rs 295-338) for the per-job protocol, `pauses_queue` (290) for the policy test.

**Per-job protocol to copy into the worker** (start_job -> separator -> run -> close_stage -> finish -> report):
```rust
let attempt = app.store.start_job(job.id)?;
// журнал не стирается: каждая попытка открывается разделителем с номером, датой и способом запуска (D-10)
let kind = if job.resume { StartKind::Retry } else if attempt <= 1 { StartKind::First } else { StartKind::Redo };
let _ = app.store.append_log(job.id, &journal::separator(attempt, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), kind));
...
let host = pipeline::host::default_host();
let rep = JobReporter::new(store.clone(), job.id, ui, cancel, attempt);
let result = run(&*host, &job, &s, &store, &rep);
rep.close_stage();            // длительность последнего этапа — до итога попытки
let _ = finish(&store, job.id, &result);
```
Phase 4 change: kind becomes `if job.resume && job.auto_retries > 0 { StartKind::Auto } else if job.resume { Retry } ...`; wrap the run in `std::panic::catch_unwind(AssertUnwindSafe(..))` producing `RunError::from(format!("внутренняя ошибка: {payload}"))`; the open-player call (`ui::open_path`) stays in the worker/UI glue, not `run`.

**Policy shape** (pure, no clock/IO; RESEARCH Pattern 2 / skeleton): `QueuePolicy { streak: Option<(String, u32)> }`, `fn after(&mut self, auto_retries_done: u32, o: &Finished) -> Verdict`, `enum Verdict { Next, AutoRetry, Pause(PauseReason) }`. `PauseReason` today is `Copy {Cancelled, Interrupted}` (app.rs 134-142); it becomes non-Copy with `Environment { code, jobs, message, hint }` and `pause_banner` (+ its test in jobs_page.rs) change accordingly.

**Unit-test style to replace** (jobs.rs 790-795):
```rust
#[test]
fn only_a_cancelled_job_pauses_the_queue() {
    assert!(pauses_queue(&Err(pipeline::CANCELLED.to_owned())));
    assert!(!pauses_queue(&Err("сбой записи".to_owned())));
    assert!(!pauses_queue(&Ok(PathBuf::from("highlights.mp4"))));
}
```
Worker integration tests copy the `running_job` / `new_job` helpers (jobs.rs 801-826) and use `FakeHost`, `Store::in_memory`, zero auto-retry pause:
```rust
fn new_job() -> store::NewJob { store::NewJob { kind: JobKind::Record, source: JobSource::Faceit("1-abc".into()), label: "матч".into(), hero_name: "me".into(), hero_steam: 42, map: "de_nuke".into() } }
```
Thread model: `std::thread` + `std::sync::mpsc` command channel (`Cmd::{Wake, Resume}`) + `Arc<Mutex<QueueView>>`; cancel stays token-based (`CancelToken`, jobs.rs 38-60). Errors map poisoned lock like `Store::with` (`"база данных занята"`).

---

### `crates/store/src/lib.rs` (migration 5, `requeue_auto`, `set_batch`, `open_read_only`)

**Migration** - append one element (lib.rs 262-278; each element raises `user_version` by one, additive only):
```rust
"ALTER TABLE jobs ADD COLUMN auto_retries INTEGER NOT NULL DEFAULT 0;
 ALTER TABLE jobs ADD COLUMN batch TEXT;",
```
Also add `auto_retries`/`batch` to `Job` and `Store::row` (line 333); update `assert_eq!(user_version(&s), 4)` (~787) to 5.

**`requeue_auto`** - copy `requeue_with` (lines 517-525) exactly, adding `auto_retries = auto_retries + 1` and `resume = 1`; make manual `requeue`/`requeue_resume` reset `auto_retries = 0`:
```rust
c.execute("UPDATE jobs SET status = 'queued', stage = '', error = NULL, fail_code = NULL, ... resume = ?3, updated_at = ?2 WHERE id = ?1", params![id, now(), resume as i64])?;
```
**`set_batch`**: same one-statement `self.with(|c| { c.execute("UPDATE jobs SET batch = ?2 ... WHERE id = ?1", params![id, batch])?; Ok(()) })` shape (separate call so `NewJob` literals elsewhere do not change).

**`open_read_only`** - analog `Store::open` / `init` (281-299), but NO `PRAGMA journal_mode=WAL`, NO `SCHEMA`, NO `migrate`:
```rust
pub fn open_read_only(path: &Path) -> Result<Self> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
    if (v as usize) < MIGRATIONS.len() { return Err(format!("база версии {v}, нужна {}: запустите приложение", MIGRATIONS.len())); }
    Ok(Self { conn: Arc::new(Mutex::new(conn)) })
}
```
**Test** - copy `version_three_database_gains_attempt_and_resume` pattern (~777-790): build old DB with `SCHEMA` + `MIGRATIONS[0..k]`, set `PRAGMA user_version = k`, `Store::open`, assert version and new columns; add `requeue_auto_increments_and_manual_requeue_resets`.

---

### `crates/store/src/journal.rs` (`StartKind::Auto`, `attempts`)

**Analog:** same file lines 14-60.
```rust
pub enum StartKind { First, Retry, Redo, /* new */ Auto }
impl StartKind { pub fn title(self) -> &'static str { match self { ..., StartKind::Auto => "авто-повтор" } } }
pub fn separator(attempt: u32, when: &str, kind: StartKind) -> String { format!("{SEPARATOR_MARK}{attempt} · {when} · {} ────", kind.title()) }
```
`attempts(log: &str) -> Vec<(u32, StartKind)>` parses lines starting with `SEPARATOR_MARK` (reuse the const; match the title strings back to `StartKind` so reader and writer share one table). Test: round trip `separator` -> `attempts`.

---

### `crates/host/src/lib.rs` (`Host::notify`) and `crates/host/src/unsupported.rs`

**Analog:** default-method style of `Host::processes`/`kill_orphan` (lib.rs 320-330):
```rust
fn kill_orphan(&self, id: &ProcIdentity) -> OrphanOutcome {
    let _ = id;
    OrphanOutcome::Unsupported
}
```
Add `fn notify(&self, title: &str, text: &str) -> bool { let _ = (title, text); false }` with Russian doc comment explaining: баллон на отдельном потоке, не блокирует; `false` - не показан. `UnsupportedHost` needs no override (default).

### `crates/host/src/win/notify.rs` (NEW, FFI) + `win/mod.rs` + `Cargo.toml`

**Analog:** `crates/host/src/win/process.rs`: module doc, `use windows_sys::Win32::...`, `use super::last_error;`, every `unsafe` block preceded by `// SAFETY:` comment, RAII `Drop` for handles (`OwnedProc`). Wire like `WinHost::kill_orphan` (win/mod.rs 331-334) delegating to `process::kill_if_same`: add `fn notify(&self, t, x) -> bool { notify::balloon(t, x) }` inside `impl Host for WinHost`, `mod notify;` in win/mod.rs. Cargo feature edit (host/Cargo.toml line 9): append `"Win32_UI_Shell"` to the `windows-sys` features list (only `crates/host` may use `windows-sys`). Detached `std::thread::spawn` creates a message-only window, `Shell_NotifyIconW(NIM_ADD, NIF_ICON|NIF_TIP|NIF_INFO)`, sleeps ~8 s, `NIM_DELETE` + `DestroyWindow`. Truncate title to 63 and body to 255 UTF-16 units; sanitize with `record::log::clean`/control-char strip BEFORE the call (the callsite in app builds text). Return value of `Shell_NotifyIconW == 0` -> `false`, never panics.

### `crates/host/src/fake.rs` (scenario sequence, notification recording)

**Analog:** `FakeHost` internals: `HostState` (line ~411-420: `scenario`, `now`, `sent`, `launches`, `orphan_kills`), `FakeHost::new(scenario)` (451), `launches()` (496), `Host::launch` (501-507) reading `st.scenario` and `st.launches += 1`, `processes` (621), `kill_orphan` recorder (625-627: `st.orphan_kills.push(id.clone())`).
Add: `FakeHost::sequence(Vec<Scenario>)` - store `queue: Vec<Scenario>`; at the top of `launch` pop the next scenario (last one repeats) before reading `launch_error`; `notifications: Vec<(String, String)>` in `HostState`, `Host::notify` pushes and returns `true`, accessor `pub fn notifications(&self) -> Vec<(String, String)>` mirroring `sent()`/`launches()`. Scenario builders are by-value chainable (`Scenario::happy(2).console_never_up()`, `.launch_error(e)`, `.processes(vec![..])`).

---

### `crates/cli/src/main.rs` (+ `record` / `retry` / `acceptance`) and `crates/cli/src/record.rs`, `acceptance.rs`

**Analog:** `main()` dispatch and `assemble_cmd` (main.rs 114-141, 402-430).
```rust
fn main() -> ExitCode {
    if env::args().nth(1).as_deref() == Some("assemble") {
        return match assemble_cmd() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => { eprintln!("ошибка: {e}"); ExitCode::FAILURE }
        };
    }
```
Add the same `if` arms for `"record"`, `"retry"`, `"acceptance"` before `parse_args()`. Arg loop style to copy (hand-written, no clap):
```rust
let mut it = env::args().skip(2);
while let Some(a) = it.next() {
    let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} требует значение"));
    match a.as_str() {
        "--out" => out = value("--out")?.into(),
        "--fps" => fps = value("--fps")?.parse().map_err(|_| "--fps: нужно число")?,
        other => return Err(format!("неизвестный параметр {other}").into()),
    }
}
let plan = plan.ok_or("укажите --plan <plan.json>")?;
```
Errors: `Result<(), Box<dyn Error>>`, printed as `eprintln!("ошибка: {e}")` -> `ExitCode::FAILURE`; arg errors print `USAGE` and `ExitCode::from(2)`. Extend `const USAGE` (line 5) in Russian. `ConsoleProgress` implements `Progress` like `Quiet` (lib.rs 74-79) but prints with `eprintln!`/`println!`, `cancelled()` stays false (killing the CLI closes the Job Object handle). Host: `pipeline::host::default_host()`. `Cargo.toml` (cli) adds `pipeline = { path = "../pipeline" }`, `store = { path = "../store" }`. `crates/cli/tests/pipeline.rs` must remain green (uses library crates only). Default no-subcommand behaviour must remain unchanged.

**`acceptance.rs`:** `evaluate(facts: &[JobFacts], probe: &dyn Fn(&Path) -> Result<Probe, String>) -> Report` pure fn (inject probe closure; mirrors `judge` purity in `record/verify.rs`); reuse `pipeline::record::verify::{probe_args, parse_probe, within_tolerance}` with `Host::run(&CmdSpec{deadline,..})` for ffprobe and `Host::processes()` for "no cs2.exe". Set parsing via `faceit::parse_match_id` (strict). Tests: sentence names, synthetic rows, one file-based `Store` test for `open_read_only`.

---

### `crates/pipeline/tests/job.rs` / `queue.rs` tests

**Analog:** `crates/pipeline/tests/record.rs` (helpers `takes(n)`, `request(name, n)`, `Capture` progress impl lines ~60-140, test `fake_recording_walks_the_whole_ladder_and_returns_every_take` at 143). Style:
```rust
#[test]
fn fake_recording_walks_the_whole_ladder_and_returns_every_take() {
    let fake = FakeHost::new(Scenario::happy(3));
    let cap = Capture::default();
    let report = record(&fake, &req, &cap).expect("запись на фейке должна пройти");
    assert_eq!(report.takes.len(), 3, "по таймингу на каждый дубль");
```
Failure-row assertion style (jobs.rs ~846-856, stays in app):
```rust
let host = FakeHost::new(Scenario::happy(2).console_never_up());
let e = record_stage(&host, &request(&dir), &pipeline::Quiet).expect_err("консоль не открывается — запись падает");
finish(&store, id, &Err(e)).unwrap();
assert_eq!(j.fail_code.as_deref(), Some("CONSOLE_NOT_OPEN"));
```
Making `verify` mandatory: `request()` helpers set `verify: None` (jobs.rs ~830, tests/record.rs) - fix those two call sites.

## Shared Patterns

### Progress/cancel boundary
**Source:** `crates/pipeline/src/lib.rs` 49-72. **Apply to:** `job.rs`, `assemble.rs`, `gamedir.rs`, CLI `ConsoleProgress`. Long steps call `p.stage(..)`, `p.log(..)`, `check(p)?`; cancel is the `CANCELLED` sentinel; queue compares against it.

### Typed failure -> DB strings
**Source:** `crates/app/src/jobs.rs` `job_failure`/`finish`/`outcome_text` (lines 161-197). **Apply to:** worker (policy input: code + state read from `RecordFailure`, not parsed from strings), acceptance checker. Capture the code at finish time because `requeue_*` clears `fail_*`.

### Host-only external processes and FFI
**Source:** `Host::run`, `Host::processes`, `Host::kill_orphan` (host/lib.rs). **Apply to:** ffprobe in checker, ffmpeg in assemble, notification. Never `Command::output()`; windows-sys only in `crates/host`.

### Additive migrations and `#[serde(default)]`
**Source:** `store/src/lib.rs` `MIGRATIONS`, `manifest.rs`. **Apply to:** migration 5, any new persisted struct field.

### Untrusted text sanitation
**Source:** `pipeline::record::log::clean` / `one_line`. **Apply to:** notification text, journal outcome lines, acceptance report (never include FACEIT key).

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `acceptance/matches.txt` | data | n/a | No repo-level data file; format is `#` comments + one FACEIT link/id per line, parsed by `faceit::parse_match_id`; embedded with `include_str!` (override env `CS2CINEMA_ACCEPTANCE`). Author supplies 5 bo1 links on 5 maps. |
| `win/notify.rs` body (`Shell_NotifyIconW`) | FFI | event-driven | No existing tray/balloon code (`tray-icon` 0.19.3 has no `NIF_INFO`); follow `process.rs` style, use RESEARCH Q3 verified constants. |
| Worker thread with `catch_unwind` | thread | event-driven | No existing `catch_unwind`/panic guard; `std::thread::spawn` pattern only (`run_next`). |

## Metadata

**Analog search scope:** `crates/app/src`, `crates/pipeline/{src,tests}`, `crates/host/src`, `crates/store/src`, `crates/cli/{src,Cargo.toml}`
**Files scanned:** ~15 (targeted reads/greps; jobs.rs 1370 and fake.rs 1975 lines read by symbol)
**Pattern extraction date:** 2026-10-11
