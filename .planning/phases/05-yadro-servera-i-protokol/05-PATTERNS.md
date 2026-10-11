# Phase 5: Ядро сервера и протокол - Pattern Map

**Mapped:** 2026-10-11
**Files analyzed:** 23 (new) + 2 (modified)
**Analogs found:** 15 / 25 (остальные берутся из RESEARCH.md: нет аналога в кодовой базе)

All analog paths verified git-tracked (`crates/store/src/lib.rs`, `crates/host/src/fake.rs`, `crates/cli/tests/pipeline.rs`).

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match |
|---|---|---|---|---|
| `Cargo.toml` (members += wire, server) | config | - | itself, lines 1-17 | exact |
| `.github/workflows/ci.yml` (+ build cs2-server) | config | - | itself | exact |
| `crates/wire/Cargo.toml` | config | - | `crates/store/Cargo.toml` | exact |
| `crates/wire/src/lib.rs` | model (serde DTO) | request-response | `crates/pipeline/src/record/verify.rs` (serde default/untagged) | role-match |
| `crates/server/Cargo.toml` | config | - | `crates/store/Cargo.toml` | exact |
| `crates/server/src/lib.rs`, `main.rs` | entrypoint | - | `crates/cli/src/main.rs` main() | role-match |
| `server/src/db.rs` (open, MIGRATIONS) | service | CRUD | `crates/store/src/lib.rs` 205-340 | exact |
| `server/src/core.rs` (decide + tx API) | service | CRUD/state machine | `crates/store/src/lib.rs` (`with`, `start_job`, `finish_job`) | role-match |
| `server/src/clock.rs` (Clock, ManualClock) | utility | - | `crates/host/src/fake.rs` (virtual `now`) | role-match |
| `server/src/config.rs` | config | transform | `crates/cli/src/main.rs` parse_args + `faceit::Client::from_env` | partial |
| `server/src/cli.rs` (serve/submit/runner/job) | controller | request-response | `crates/cli/src/main.rs` main/parse_args; `cli/src/acceptance.rs` subcommand dispatch | exact |
| `server/src/http.rs`, `api.rs`, `blobs.rs`, `auth.rs`, `secret.rs`, `log.rs` | various | streaming/event | none | none (use RESEARCH) |
| `server/tests/core_states.rs` | test | state machine | `store` `mod tests` (job_lifecycle) + `fake.rs` virtual-time tests | role-match |
| `server/tests/protocol_http.rs`, `secrets.rs`, `deps.rs`, `common/mod.rs` | test | request-response | `crates/faceit` Client (ureq agent config); `crates/cli/tests/pipeline.rs` layout | partial |

## Pattern Assignments

### `Cargo.toml` (root) - modified
Add to `members` after `"crates/cli"`:
```toml
    "crates/cli",
    "crates/wire",
    "crates/server",
]
```
Existing file: `[workspace] resolver = "2"`, members list lines 4-17. Do not touch other crates (no existing crate gains a dependency).

### `crates/wire/Cargo.toml`, `crates/server/Cargo.toml`
**Analog:** `crates/store/Cargo.toml` (edition 2024, short dep lines):
```toml
[package]
name = "store"
version = "0.1.0"
edition = "2024"

[dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```
Server: `rusqlite` MUST be `0.32` with `bundled` (same as store, links = "sqlite3"). Full dep list is in RESEARCH "Installation". Only path dep allowed: `wire = { path = "../wire" }`. Add `[[bin]] name = "cs2-server" path = "src/main.rs"`.

### `server/src/db.rs` (service, CRUD) - copy from `crates/store/src/lib.rs`
**Shared handle + error alias** (lines 18, 213-224):
```rust
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone)]
pub struct Store { conn: Arc<Mutex<Connection>>, }
fn err(e: rusqlite::Error) -> String { format!("база данных: {e}") }
```
**Migrations** (lines 264-290, 328-338) - copy verbatim, including the Russian rule comment:
```rust
// Новая версия — только новым шагом в конце списка и только добавлением колонок или таблиц.
// Элемент k поднимает схему до версии k+1; шаг идёт в одной транзакции со своим `user_version`.
const MIGRATIONS: &[&str] = &[ "CREATE TABLE ..." ];
pub fn schema_version() -> usize { MIGRATIONS.len() }

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
    for (k, sql) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
        let tx = conn.unchecked_transaction().map_err(err)?;
        tx.execute_batch(sql).map_err(|e| format!("миграция базы до версии {}: {e}", k + 1))?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", k + 1)).map_err(err)?;
        tx.commit().map_err(err)?;
    }
    Ok(())
}
```
**open / in_memory** (lines 293-326): `create_dir_all(parent)` then `Connection::open`, `init`. DIFFERENCES for server: pragmas must be `journal_mode=WAL; synchronous=NORMAL; busy_timeout=5000; foreign_keys=ON` on every connection (store sets only WAL + FK); refuse start if `user_version > schema_version()` (store's `open_read_only` shows the version check style at lines 309-312); all writes use `transaction_with_behavior(Immediate)`; server has no `SCHEMA` const, schema lives only in MIGRATIONS step 1.
**Lock helper** (lines 340-343) - DEVIATE: store uses `lock().map_err(|_| "база данных занята")`; server must use `lock().unwrap_or_else(|e| e.into_inner())` (RESEARCH anti-pattern), same shape otherwise:
```rust
fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
    let conn = self.conn.lock().map_err(|_| "база данных занята".to_owned())?;
    f(&conn).map_err(err)
}
```
**Time**: store's `now()` (lines 217-219) is seconds; server must use ms from `Clock`, not a free function.
**Migration tests** (store tests lines 804-1040): copy the style - `user_version(&s)` helper, apply `MIGRATIONS[..k]` manually, reopen, assert `schema_version()`, assert "повторное открытие ничего не меняет".

### `server/src/clock.rs` (utility) - style from `crates/host/src/fake.rs`
Fake host keeps virtual time in shared state, `now: Duration::ZERO`, read by `fn now(&self)` (lines 465-490, 610-612), cloneable handle (`#[derive(Clone)] struct FakeHost { state: Arc<Mutex<...>> }`), and a guard against runaway time (lines 452-456):
```rust
fn check_virtual(now: Duration) {
    if now > MAX_VIRTUAL { panic!("фейк: виртуальное время перевалило за 10 ч — сценарий завис"); }
}
fn lock(h: &Arc<Mutex<HostState>>) -> MutexGuard<'_, HostState> {
    h.lock().unwrap_or_else(|e| e.into_inner())   // poison-tolerant lock
}
```
Apply: `trait Clock: Send + Sync { fn now_ms(&self) -> i64 }`, `SystemClock`, `ManualClock(Arc<AtomicI64>)` with `advance(Duration)`, `Clone`. Same poison-tolerant lock idiom wherever a Mutex is used. Host's "host reports facts, caller decides" maps to: `decide()` pure, wrapper applies.

### `server/src/cli.rs` / `main.rs` (controller, request-response) - `crates/cli/src/main.rs`
**Subcommand dispatch before flag parsing** (lines 128-142) - copy for `serve | submit | runner add/list/revoke | job list/show/cancel | config show`:
```rust
fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("assemble") => return assemble_cmd(),
        Some("acceptance") => {
            return match env::args().nth(2).as_deref() {
                Some("check") => acceptance::check_cmd(), ...
                other => acceptance::usage_cmd(other),
            };
        }
        _ => {}
    }
```
**Hand-written flags** (lines 70-125): `let mut it = env::args().skip(1); while let Some(a) = it.next() { let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} требует значение")); match a.as_str() { "-h" | "--help" => return Err(String::new()), "--out" => ..., "--demo" => value("--demo")?.parse().map_err(|_| "--demo: нужно число")?, s if args.input.is_empty() => args.input = s.to_owned(), ... } }`.
**Error/exit contract** (lines 144-160): usage error prints `ошибка: {msg}\n` (skip if msg empty) then `USAGE`, `ExitCode::from(2)`; runtime failure `eprintln!("ошибка: {e}")`, `ExitCode::FAILURE`. USAGE is a Russian `const USAGE: &str = "\` block (line 10).
Note: `main.rs` in server should be thin (`run_cli(std::env::args())` per RESEARCH) but keep this exit-code contract. Secrets must never be printed in `ошибка:` lines (SEC-04).

### `server/src/config.rs` - partial analogs
`faceit::Client::from_env` (crates/faceit/src/lib.rs 55-59) for env-with-Russian-error:
```rust
let key = std::env::var("FACEIT_API_KEY")
    .map_err(|_| "задайте ключ FACEIT Data API в переменной окружения FACEIT_API_KEY")?;
```
Use the same message style (name the variable, never the value). JSON config file + `#[serde(deny_unknown_fields)]`, `#[serde(default)]` per RESEARCH Pattern 8. Serde-defaults analog: `crates/pipeline/src/record/verify.rs` lines 56-89 (`#[serde(untagged)]`, `#[serde(default)]`).

### `server/src/core.rs` (state machine) - test and API shape from store
Store transitions are methods returning `Result<T>` with the `with` closure; tests named as sentences using `Store::in_memory()` (`job_lifecycle`, lines 642-665):
```rust
#[test]
fn job_lifecycle() {
    let s = Store::in_memory().unwrap();
    let a = s.add_job(&new_job(JobKind::Record)).unwrap();
    assert_eq!(s.next_queued().unwrap().unwrap().id, a, "очередь — по порядку постановки");
```
Apply: `Core::in_memory(ManualClock)`, test helper `fn new_job(...)` at top of test module, assertion messages in Russian. `decide()` itself has no analog; use RESEARCH Patterns 1-4 (claim SQL in Pattern 2 / Code Examples).

### `server/src/wire` DTOs (`crates/wire/src/lib.rs`)
Derives per CLAUDE.md: `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`; `#[serde(default)]` on optional fields; enums with `#[serde(other)] Unknown`. Failure codes (`FailCode`) travel as strings, wire must not depend on `pipeline`.

### `server/tests/protocol_http.rs` / `common/mod.rs`
ureq agent config to copy from `crates/faceit/src/lib.rs` 41-52 (`http_status_as_error(false)`; skip the TLS line for loopback tests):
```rust
let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
```
Status mapping style: `faceit::check` (lines 139-147) - `match status { 200..=299 => Ok(()), 401 | 403 => Err(format!(...)), s => Err(...) }`. Test layout: top-level private helper `fn`s at file top, sentence-named `#[test]`s (see `crates/cli/tests/pipeline.rs` lines 1-30: module `//!` Russian doc, helpers `varint`, `command`, then tests). No unix-only test code (CI runs ubuntu + windows).

### `.github/workflows/ci.yml` - modified
Existing steps: `cargo test --release --workspace` (already covers new crates on both OSes) and `cargo build --release -p cli`. Add one step after "Build cs2-cli" (runs on both OSes; optionally upload the Linux binary):
```yaml
      - name: Build cs2-server
        run: cargo build --release -p server
```
Matrix is `[ubuntu-latest, windows-latest]`; keep it. Upload step pattern uses `if: runner.os == 'Windows'`; mirror with `runner.os == 'Linux'` and `path: target/release/cs2-server` if an artifact is wanted.

## Shared Patterns

### Error handling
**Source:** `crates/store/src/lib.rs:18,222` - `pub type Result<T> = std::result::Result<T, String>;` + `fn err(e) -> String { format!("база данных: {e}") }`. Apply to all server modules; no anyhow/thiserror; Russian messages; API error JSON `{"error":"<ascii_code>","message":"<русский>"}`.

### Poison-tolerant locks
**Source:** `crates/host/src/fake.rs:458-461` - `unwrap_or_else(|e| e.into_inner())`. Apply to the DB mutex, worker channel, upload guard.

### Migrations
**Source:** `crates/store/src/lib.rs:264-338` (see db.rs above). Apply to `server/src/db.rs` only.

### Tests
Sentence-named tests (`job_lifecycle`, `rejects_garbage`), private helpers at top of the test module, Russian assertion messages, virtual clock instead of `sleep` (fake.rs style).

### Module docs/comments
Each `lib.rs` has a Russian `//!` doc stating purpose and design rule; `///` on public items; identifiers English.

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `server/src/http.rs` (dispatcher + bounded pool + 503) | service | request-response | No HTTP server in repo (only ureq clients); use RESEARCH Pattern 7 / dispatcher snippet |
| `server/src/api.rs` (router, handlers) | controller | request-response | none; RESEARCH Patterns 5-6 |
| `server/src/blobs.rs` (chunk append, .part, Range, sha256) | service | file-I/O streaming | none server-side; sha2 used in `crates/app` for update verify, but not a streaming-upload analog; RESEARCH Pattern 5 and "Streaming chunk append" |
| `server/src/auth.rs` (token issue/hash) | service | CRUD | none; RESEARCH "Token issue / hash" |
| `server/src/secret.rs`, `log.rs` (Secret, redact, LogSink) | utility | transform | no logging abstraction exists (only println!/eprintln!); RESEARCH SEC-04 and sentinel test skeleton |
| `server/tests/deps.rs` (no forbidden path deps) | test | - | none; read `crates/server/Cargo.toml` via `include_str!` and assert only `../wire` path dep |
| `server/tests/secrets.rs` | test | - | none; RESEARCH sentinel skeleton |

## Metadata

**Analog search scope:** root `Cargo.toml`, `.github/workflows/ci.yml`, `crates/store`, `crates/host/src/fake.rs`, `crates/cli/src`, `crates/cli/tests`, `crates/faceit`, `crates/pipeline/src/record`
**Files scanned:** ~10
**Pattern extraction date:** 2026-10-11
