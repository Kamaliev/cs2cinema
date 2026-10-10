# Phase 1: Новый путь записи с понятными сбоями - Pattern Map

**Mapped:** 2026-10-10
**Files analyzed:** 27 new/modified
**Analogs found:** 22 / 27 (5 need RESEARCH.md patterns: Win32 job, machine, fake host)

All analog paths below were checked as git-tracked (`git ls-files`); `crates/host/` does not exist yet (greenfield). No gitignored mirror paths are used.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `Cargo.toml` (members += `crates/host`) | config | n/a | same file, members list | exact |
| `crates/host/Cargo.toml` | config | n/a | `crates/pipeline/Cargo.toml` + RESEARCH "Installation" | role-match |
| `crates/host/src/lib.rs` (traits Host/Session, LaunchSpec, SessionEvent, HostError) | service (trait) | event-driven | `crates/pipeline/src/lib.rs:43-65` (Progress trait, Quiet) | role-match |
| `crates/host/src/fake.rs` (FakeHost, Scenario, virtual clock) | test-util | event-driven | `pipeline::Quiet` + `crates/cli/tests/pipeline.rs` fixtures | partial |
| `crates/host/src/unsupported.rs` | utility (stub) | request-response | `launch::game_alive` `cfg(not(windows))` arm (`launch.rs:261-264`) | role-match |
| `crates/host/src/win/job.rs`, `process.rs` | service (FFI) | request-response | none; RESEARCH "Code Examples" (compiled) | no analog |
| `crates/host/src/win/mod.rs` (WinHost/WinSession: launch line, threads) | service | event-driven | `crates/app/src/launch.rs:160-174` (`launch`) | exact (moved) |
| `crates/host/src/win/netcon.rs` | service | streaming | `launch.rs:176-229` (`Console`) | exact (moved) |
| `crates/host/src/win/console_log.rs` | service | streaming (file tail) | none; `Console::open` reader thread style | partial |
| `crates/host/src/runner.rs` (optional, Plan 8) | utility | request-response | `jobs.rs:309-340` (`run_ffmpeg`) | exact (moved) |
| `crates/host/examples/spikes.rs` | utility (harness) | batch | `crates/cli/src/main.rs` (hand-written subcommands, `ExitCode`) | role-match |
| `crates/pipeline/Cargo.toml` (+ `host`) / `lib.rs` (`pub use host`, `Progress::event`, `Event`, `Plan{takes, nonce}`) | config / trait | event-driven | `pipeline/src/lib.rs:43-65`, `:156-163` | exact |
| `crates/pipeline/src/record/mod.rs` (driver loop, `record()`) | service | event-driven | `jobs.rs:189-287` (old `record`) for behavior; `pipeline::check` for cancel | role-match |
| `.../record/machine.rs` (pure `step(now, Input)`) | service (pure) | transform | `highlights::score_kill` pure style; RESEARCH sketch | partial |
| `.../record/markers.rs` | utility | transform | `jobs.rs:199` (blacklist marker) -> replace | partial |
| `.../record/failure.rs` (FailCode, RecordFailure, RU catalogue) | model | transform | `store::JobStatus::{as_str,parse,title}` (`store/src/lib.rs:27-63`) | role-match |
| `.../record/deadlines.rs` | config | transform | `renderer::Config` + `Default` (`renderer/src/lib.rs:58-95`) | exact |
| `.../record/log.rs` (RecordLog, Tail) | utility | file-I/O | `Store::append_log` + flush-per-line; no file analog | partial |
| `.../record/takes.rs` / `renderer::takes()` | utility | transform | `renderer::record_order` / `capture_fps` (`renderer/src/lib.rs:136-142, 246-251`) | exact |
| `crates/renderer/src/lib.rs` (nonce in `Config`, markers, remove `TAKE_DONE`, tests) | generator | transform | same file `:149-242`, tests `:640-690` | exact |
| `crates/store/src/lib.rs` (migration, `set_phase`, `set_out_dir`, `fail_job`, `Job` fields) | model/persistence | CRUD | same file `:161-337`, tests `:382-` | exact |
| `crates/app/src/jobs.rs` (RunError, call `pipeline::record`, `plan_hash` normalise, `JobReporter::event`) | controller | request-response | same file `:39-86`, `:290-307` | exact |
| `crates/app/src/jobs_page.rs` (error block + ladder) | component | request-response | same file `:113-117`, `:174-183` | exact |
| `crates/app/src/status.rs` | component/state | event-driven | same file `:40-75` (legacy `записан дубль N/M` parsing) | exact |
| `crates/app/src/settings.rs`, `settings_page.rs` (`load_wait_secs` -> `time_margin`) | config/component | CRUD | same files (`settings.rs:24,54`; `settings_page.rs:98`) | exact |
| `record.log` (runtime artifact) | log | file-I/O | `JobReporter::log` timestamp format (`jobs.rs:52-55`) | partial |
| `crates/pipeline/tests/fixtures/*.log` + record tests | test | event-driven | `crates/cli/tests/pipeline.rs`, `renderer/src/assemble.rs` tests | role-match |

## Pattern Assignments

### `crates/host/src/lib.rs` (trait crate root; event-driven)

**Analog:** `crates/pipeline/src/lib.rs` lines 42-65 (trait with default methods, sentinel, silent impl)

```rust
/// Куда конвейер сообщает о ходе работы — и откуда узнаёт, что работу отменили.
pub trait Progress: Sync {
    fn stage(&self, s: Stage);
    fn log(&self, msg: &str);
    /// Пользователь отменил задачу: конвейер останавливается на ближайшей границе этапа.
    fn cancelled(&self) -> bool {
        false
    }
}
pub const CANCELLED: &str = "отменено";
pub fn check(p: &dyn Progress) -> Result<()> {
    if p.cancelled() { Err(CANCELLED.to_owned()) } else { Ok(()) }
}
pub struct Quiet;
impl Progress for Quiet { fn stage(&self, _: Stage) {} fn log(&self, _: &str) {} }
```
Copy: Russian `///` on each trait method with a non-obvious contract; default bodies; module `//!` doc explaining the design rule ("только этот крейт трогает Win32/сокеты/процессы"). Derives on data types: `#[derive(Debug, Clone, PartialEq)]` (as `store::Job`). `Event` for `Progress::event` gets a **default body** (`self.log(&e.to_string())`) so `Quiet`, `app::Reporter`, `JobReporter` compile unchanged. `HostError`: hand-written enum + `Display`/`impl From<HostError> for String` (no `thiserror`; workspace has none). Trait signatures to freeze: see RESEARCH "Host trait design" (`poll(max_wait)`, `send`, `game_liveness`, `launcher_exit`, `terminate`; `Host::{launch, now, run}`).

### `crates/host/src/win/mod.rs` + `netcon.rs` (moved from launch.rs)

**Analog:** `crates/app/src/launch.rs`

**Launch line to move** (lines 160-174) – add `-condebug`, `-noConfig`, `-afxFixNetCon` per spikes; keep in ONE place with a comment on `-insecure`:
```rust
let hook = Path::new(hlae_dir).join("x64/AfxHookSource2.dll");
let cmdline = format!(
    "-insecure -steam -novid -allow_third_party_software -netconport {port} -windowed -w {w} -h {h} +demo_ui_mode 0 +playdemo {demo_name}"
);
let mut cmd = Command::new(Path::new(hlae_dir).join("HLAE.exe"));
cmd.current_dir(hlae_dir)
    .args(["-customLoader", "-noGui", "-autoStart", "-hookDllPath"]).arg(hook)
    .arg("-programPath").arg(cs2_exe(cs2_dir))
    .args(["-cmdLine", &cmdline]);
cmd.spawn().map_err(|e| format!("не удалось запустить HLAE: {e}"))
```
Change: spawn, then `AssignProcessToJobObject` immediately; keep the `Child` (old code dropped it as `_launcher`, `jobs.rs:194`) and `try_wait` it for `LauncherExited{code}`. Use `launch::no_window` equivalent (CREATE_NO_WINDOW) for any utility child.

**Netcon reader to move** (lines 203-228) – keep: `connect_timeout(127.0.0.1:port, 2s)`, `try_clone` for a reader thread, `read_until(b'\n')` + `from_utf8_lossy(..).trim()`, `write_all(format!("{cmd}\n"))`. Add: `set_read_timeout(500 ms)` + stop flag (Pitfall 3), reject `\r`/`\n` in `send`, retry connect every 1 s (old loop `launch.rs:186-195`) driven by the machine's `console` deadline, NOT by an internal timeout.
```rust
let stream = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_secs(2))?;
let (tx, lines) = mpsc::channel();
let reader = stream.try_clone()?;
std::thread::spawn(move || {
    let mut r = BufReader::new(reader);
    let mut buf = Vec::new();
    while r.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
        if tx.send(String::from_utf8_lossy(&buf).trim().to_owned()).is_err() { break; }
        buf.clear();
    }
});
```
**Do NOT carry over:** `game_alive` (`launch.rs:251-265`, `Err(_) => true`), `kill_game` (`taskkill /IM`), `wait_for_clips`/`newest_mtime`/`dir_size`, `Console::drain` polling.

**`cfg(windows)` + stub pattern** (`launch.rs:251-264`):
```rust
#[cfg(windows)] { ... }
#[cfg(not(windows))] { true }
```
For `host`: `win` module `#[cfg(windows)]`, `unsupported.rs` `#[cfg(not(windows))]`, `default_host()` selects. `windows-sys` only under `[target.'cfg(windows)'.dependencies]`.

### `crates/host/src/win/job.rs` / `process.rs` (no analog)

Use RESEARCH.md "Code Examples -> Job Object, handle liveness, terminate" verbatim (compiled and run). Wrap in safe `Job` with `Drop`; every `unsafe` carries `// SAFETY:`; liveness is tri-state `Alive | Exited(code) | Unknown(os error)`, never "alive on error". Only existing `unsafe` in workspace is `crates/demo/build.rs`, so there is no local style to copy.

### `crates/host/src/fake.rs` (no strong analog)

Closest: `pipeline::Quiet` (silent impl) and test fixture style in `crates/cli/tests/pipeline.rs` (small private helper fns at the top: `varint`, `command`, `inner`). Must be a plain `pub mod` (not `cfg(test)`) so `pipeline` tests reach it. `poll(max_wait)` always advances virtual time by `max_wait` when nothing is due (Pitfall 10) with a 10-virtual-hour safety cap. Writes real files into `std::env::temp_dir().join(format!("cs2cinema-pipeline-{}", std::process::id()))` (existing test convention in `pipeline` test `analyze_player_plan`, and `tmp()` helper in `renderer/src/assemble.rs:221`).

### `crates/host/examples/spikes.rs` (harness)

**Analog:** `crates/cli/src/main.rs` (hand-written subcommand parse, `eprintln!("ошибка: {e}")`, `ExitCode`, `const USAGE` in Russian). Subcommands `job | netcon | failures | abspath`; `fn main()` under `#[cfg(windows)]` with a non-Windows `main` printing "только для Windows". Output to `%LOCALAPPDATA%\cs2cinema\spikes\<timestamp>\` (path resolution style: `crates/app/src/settings.rs` `data_dir()`). Build on the real `win/job.rs`.

### `crates/pipeline/src/record/failure.rs` (model with string codes)

**Analog:** `crates/store/src/lib.rs` lines 27-63 enum <-> string pattern (`as_str`, `parse`, `title`).
```rust
// store/src/lib.rs:33,39,54 (shape): private as_str/parse, pub title -> Russian
fn as_str(self) -> &'static str { ... }
fn parse(s: &str) -> Self { ... }
pub fn title(self) -> &'static str { ... }
```
Apply: `FailCode::as_str()` -> `"LAUNCHER_FAILED"` etc. (D-08), `RecordState::{code(), title()}` (title in Russian, ladder order `Launcher, Game, Console, Demo, Script, Take, Closing, Done`, `LADDER` const array like `Stage::ALL` at `pipeline/src/lib.rs:26`). `RecordFailure` derives `Debug, Clone`. `store` receives only strings. Cancellation boundary: `FailCode::Cancelled` lowers to `pipeline::CANCELLED.to_owned()` in `jobs::run`.

### `crates/pipeline/src/record/deadlines.rs`

**Analog:** `renderer::Config` + `Default` (`renderer/src/lib.rs:58-95`): documented pub fields, one `impl Default` with literal constants, struct-update in callers (`Config { encoder: .., ..Config::default() }`). Add `fn scaled(&self, k: f32)`, `Deadlines::fast()` for tests (ms). Starting values: launcher 30 s, game_appear 20 s, console 90 s, demo_load 150 s (old `LOAD_CAP`, `jobs.rs:181`), script 30 s (old `EXEC_RETRY`, `jobs.rs:183`), first_take 90 s, take_seek 60 s, game_close 30 s. Per-take: `game_secs * max(1, capture_fps/60) * 2 + 30 s` using `renderer::capture_fps(shot, cfg)` (`renderer/src/lib.rs:139`).

### `crates/pipeline/src/record/mod.rs` (driver)

**Analog (behavior):** old `jobs::record` `jobs.rs:189-287`; pipeline conventions: `&dyn Progress`, `check(p)?`. Log text to keep for the legacy status parser (`status.rs:64`): `p.log(&format!("записан дубль {}/{}", n + 1, last + 1))` (jobs.rs:261). Port allocation: keep `free_port()` (`launch.rs:156`) semantics inside the host. Drive loop: `session.poll(100ms)` -> `p.cancelled()` -> `host.now()` -> `machine.step`. The old code's anti-patterns to avoid: `Cell/RefCell` closure state, `thread::sleep(250ms)` loops, retry `exec` on a timer (`jobs.rs:269-278`), downgrade-to-log on missing console (`jobs.rs:229`).

Test naming and style (sentence names, Russian assert messages), e.g. `crates/store/src/lib.rs:390-421`:
```rust
#[test]
fn job_lifecycle() {
    let s = Store::in_memory().unwrap();
    assert_eq!(s.next_queued().unwrap().unwrap().id, a, "очередь — по порядку постановки");
```
Use `console_that_never_opens_fails_with_console_not_open_after_the_deadline` style from RESEARCH "Pattern: pure machine test".

### `crates/renderer/src/lib.rs` (nonce + markers)

**Analog:** itself. Config addition (default needed: CLI uses `Config::default()` at `crates/cli/tests/pipeline.rs:110`):
```rust
pub struct Config { ... pub dialect: Dialect, pub encoder: Encoder, /* + */ pub nonce: String }
impl Default for Config { fn default() -> Self { Self { ..., encoder: Encoder::default(), nonce: "cli".into() } } }
```
Emission points: `highlights.cfg` header `writeln!(s, ...)` block (`:160-190`) gets `echo CS2C <nonce> SCRIPT`; start via the existing closure `at(&mut s, shot.start_tick, "mirv_streams record start")` (`:202`) -> append `; echo CS2C <nonce> TAKE_START {i}`; end line (`:229`):
```rust
at(&mut s, shot.end_tick, &format!("mirv_streams record end; mirv_campath enabled 0; mirv_fov default; echo {TAKE_DONE}{i}; {tail}"));
```
-> `echo CS2C {nonce} TAKE_END {i}`; `None` tail (`:226`) gets `echo CS2C <nonce> ALL_DONE;` prefixed. Remove `pub const TAKE_DONE` (`:242`). Nonce hex only (passes `console_safe`, `:145`). New `pub fn takes(t, cfg) -> Vec<TakePlan>` next to `record_order` (`:247`), built on `record_order` + `capture_fps`.

**Tests to update** (`:651,654,686`): the three `echo CS2C_TAKE_DONE N; ...` asserts become `echo CS2C <nonce> TAKE_END N; ...`. Keep the loop asserting no `:/` in `mirv_cmd` lines (`:657-662`). Add one test per marker.

### `crates/store/src/lib.rs` (migration + failure columns)

**Analog:** itself. Schema applied in `init` (`:209-213`):
```rust
fn init(conn: Connection) -> Result<Self> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;").map_err(err)?;
    conn.execute_batch(SCHEMA).map_err(err)?;
    Ok(Self { conn: Arc::new(Mutex::new(conn)) })
}
```
Insert after `SCHEMA`: `PRAGMA user_version` migration block (RESEARCH "Migration"), adding `fail_code, fail_state, fail_hint, fail_tail, fail_elapsed_ms, phase`. Row mapping uses named gets (`:235-255`) – add `fail_code: r.get("fail_code")?` etc. (`Option<String>` / `Option<i64>`; `phase: r.get("phase")?`). Do not add non-`PartialEq` fields (`Job` derives `Debug, Clone, PartialEq`).

Method shape to copy for `set_phase`, `set_out_dir`, `fail_job` (`set_stage`, `:289-294`, and `finish_job` Err arm `:310`):
```rust
pub fn set_stage(&self, id: i64, stage: &str) -> Result<()> {
    self.with(|c| {
        c.execute("UPDATE jobs SET stage = ?2, updated_at = ?3 WHERE id = ?1", params![id, stage, now()])?;
        Ok(())
    })
}
```
`fail_job(id, &JobFailure{ message, code, state, hint, tail, elapsed_ms })` sets `status='failed', error=message, fail_*`. `start_job` (`:279`) and `requeue` (`:325`) must null `fail_*` and reset `phase=''`. Tests: add to `mod tests` (`:382`) using `Store::in_memory()` and `new_job(kind)` helper (`:386`); a migration test opens a `Connection` with the OLD `SCHEMA` first (extract the old DDL in the test), then calls `init`.

### `crates/app/src/jobs.rs` (cutover)

**Analog:** itself. `run_next` match to extend (`:75-84`):
```rust
let result = run(&job, &s, &store, &rep);
let _ = match &result {
    Ok(o) => store.finish_job(job.id, Ok(o)),
    Err(e) if e == pipeline::CANCELLED => store.cancel_job(job.id),
    Err(e) => store.finish_job(job.id, Err(e)),
};
rep.ui.send(Msg::JobDone { id: job.id, result: result.map(|o| o.result) });
```
-> `RunError { message, fail: Option<RecordFailure> }` + `impl From<String> for RunError` so existing `?` on `Result<_, String>` still compiles; `Msg::JobDone.result` stays `Result<_, String>` (`.map_err(|e| e.message)`).

`JobReporter` (`:40-59`): add `fn event(&self, e: &Event)` -> `store.set_phase(id, state.code())`, plus RU journal line, plus legacy `записан дубль N/M` via `log`. Do NOT put the record sub-state in `set_stage` (`jobs_page.rs:209` maps `stage` to `Stage::ALL`).

Call site: replace `jobs.rs:160-168` (`record(s, ...)`, `kill_game`) with `pipeline::record(&*host, &req, p)`; call `store.set_out_dir(job.id, &out_dir)` right after `out_dir` (`:103`). Keep `launch::{check_paths, install_files, nvenc_check, hlae_ffmpeg}` calls unchanged. Delete: `LOAD_CAP/EXEC_RETRY/EXEC_RETRIES/RECONNECT_EVERY` (`:180-186`) and `fn record` (`:189-287`).

`plan_hash` (`:290-307`) – normalise the nonce, FNV-1a stays:
```rust
for b in n.bytes().chain(std::fs::read(out_dir.join(&n)).unwrap_or_default()) {
    h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
}
```
-> read file, `String::from_utf8_lossy(..).replace(&nonce, "NONCE")` before hashing (Pitfall 1); add test that two renders differing only by nonce hash equal.

### `crates/app/src/jobs_page.rs` (error block + ladder)

**Analog:** itself. Failed arm to extend (`:113-117`):
```rust
JobStatus::Failed => {
    ui::pill(ui, "ошибка", BAD);
    let e = j.error.clone().unwrap_or_default();
    ui.add(egui::Label::new(RichText::new(&e).color(BAD)).truncate()).on_hover_text(e);
}
```
Add under the horizontal row (before meta, `:122`): ladder row (labels from `RecordState::LADDER` titles, passed/current/failed colors `OK`/`ACCENT`/`BAD`/`MUTED`), then for `fail_code.is_some()`: small `RichText::new(format!("{code} · {state}")).color(MUTED).size(12.5)`, message (BAD), hint (MUTED), «прошло N с», and `egui::CollapsingHeader` with `RichText::new(tail).monospace().small()` modelled on the journal frame (`:174-183`, `Frame::new().fill(ui::BG).corner_radius(..)`, `ScrollArea::vertical().id_salt(("log", j.id)).max_height(200.0)`). Ladder also for `JobStatus::Running` via `j.phase` and `app.status.takes` for `дубль N/M` (progress fn `:208-217`).

### `crates/app/src/settings.rs` / `settings_page.rs`

**Analog:** itself. Replace `pub load_wait_secs: u32` (`settings.rs:24`, default `25` at `:54`) with `pub time_margin: f32` default `1.0`; struct has `#[serde(default)]`, old `settings.json` still loads. UI: replace the `egui::DragValue::new(&mut d.load_wait_secs).range(5..=180)` row (`settings_page.rs:98`) with `DragValue::new(&mut d.time_margin).range(0.5..=5.0).prefix("×")`; label «Запас по времени». Keep `Settings::default()` calling `crate::launch::find_cs2()` (`settings.rs:44`).

## Shared Patterns

### Error type and cancellation sentinel
**Source:** `crates/pipeline/src/lib.rs:10,53-58`
**Apply to:** `pipeline::record`, `jobs::run`, host-to-string conversions
`Result<T, String>` at all public boundaries; `RecordFailure` is the one typed exception and lowers to strings at the `run()` boundary; cancel == exactly `CANCELLED` ("отменено"); `app.rs` compares `Err(e) if e == pipeline::CANCELLED`.

### Progress/log reporting
**Source:** `crates/app/src/jobs.rs:47-59` (`JobReporter`)
```rust
fn log(&self, m: &str) {
    let _ = self.store.append_log(self.id, &format!("{}  {m}", chrono::Local::now().format("%H:%M:%S")));
    self.ui.send(Msg::Log(m.to_owned()));
}
```
**Apply to:** record driver (every state change and fact via `p.log`/`p.event`), `record.log` line prefix `HH:MM:SS.mmm [source] text` (same chrono format, plus `%.3f`).

### Windows-only code with CI stub
**Source:** `crates/app/src/launch.rs:142-143,251-264` (`#[cfg(windows)]` / `#[cfg(not(windows))]`)
**Apply to:** all of `crates/host` (CI runs ubuntu + windows, `cargo test --release --workspace`).

### Defaults for new config fields
**Source:** `renderer::Config` `Default` (`renderer/src/lib.rs:80-95`)
**Apply to:** `Config.nonce`, `Deadlines`, `LaunchSpec` – every new field must default so `cs2-cli` and `crates/cli/tests/pipeline.rs:110` keep compiling.

### Input sanitising before the game console
**Source:** `renderer::console_safe` (`renderer/src/lib.rs:145-147`), `pipeline::console_name`, call at `jobs.rs:138-140`
**Apply to:** nonce (hex only), demo name, `Session::send` (reject `\r`/`\n`).

### Test style
**Source:** `crates/store/src/lib.rs:382-421`, `crates/renderer/src/assemble.rs:221-263`
Sentence-named tests, small private helpers at top of `mod tests`, Russian assert messages, temp dirs via `std::env::temp_dir().join(format!("...-{}", std::process::id()))`, no new dev-deps.

### Doc/comment language
Identifiers English; `//!`, `///`, inline comments and user-visible strings Russian. Run `cargo fmt`; wide lines tolerated.

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `crates/host/src/win/job.rs`, `process.rs` | FFI service | request-response | No Win32 code in workspace; use RESEARCH "Code Examples" (compiled and run) |
| `crates/host/src/win/console_log.rs` | file tail | streaming | No tailing code; offset-tail, BOM `ef bb bf` strip, partial-line safe, read from launch-time offset |
| `crates/host/src/fake.rs` virtual clock | test double | event-driven | No fakes with virtual time exist; follow RESEARCH design (`poll` advances time) |
| `crates/pipeline/src/record/machine.rs` | pure state machine | transform | No state machine in workspace; use RESEARCH state table + `step(now: Duration, Input) -> Vec<Action>` |
| `crates/pipeline/src/record/log.rs` (`Tail` fold) | utility | transform | No folding/ring buffer exists; test with 800-line `Failed writing image` storm |

## Metadata

**Analog search scope:** `crates/app/src` (launch, jobs, jobs_page, status, settings), `crates/pipeline/src/lib.rs`, `crates/renderer/src/{lib,assemble}.rs`, `crates/store/src/lib.rs`, `crates/cli/tests/pipeline.rs`, workspace/crate `Cargo.toml`
**Files scanned:** ~14
**Pattern extraction date:** 2026-10-10
