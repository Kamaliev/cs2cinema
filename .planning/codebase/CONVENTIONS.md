---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
# Coding Conventions

**Analysis Date:** 2026-10-10

## Naming Patterns

**Files:**
- Rust source files use `snake_case.rs`: `crates/pipeline/src/lib.rs`, `crates/director/src/grenade.rs`, `crates/app/src/match_page.rs`, `crates/app/src/settings_page.rs`.
- Each library crate exposes its API from `src/lib.rs`; the binaries are `crates/cli/src/main.rs` and `crates/app/src/main.rs`.
- Page modules in the GUI crate are suffixed `_page.rs` (`jobs_page.rs`, `match_page.rs`, `settings_page.rs`).
- Integration tests live in `crates/<crate>/tests/*.rs` (`crates/cli/tests/pipeline.rs`, `crates/positions/tests/real_demo.rs`).

**Crates:**
- Crate directories and package names are short lowercase nouns with no prefix: `demo`, `cs2`, `highlights`, `director`, `renderer`, `positions`, `geometry`, `pipeline`, `store`, `faceit`, `app`, `cli`. Declared as workspace members in `Cargo.toml`.

**Functions and methods:**
- `snake_case`, verb-first for actions: `analyze`, `plan_player`, `load_positions`, `score_kill`, `for_player`, `utility_for`, `shoot_grenade`.
- Constructors: `new`, `build`, `open`, `in_memory`, `from_bytes`, `from_triangles`, `load`.
- Predicates and checks: `check` (returns `Result<()>`, see `crates/pipeline/src/lib.rs`), `is_*`/`has_*` where a boolean is returned.
- Unit tests are named as sentences describing behavior, not `test_*`: `chain_splits_by_gap_and_ace_dominates` (`crates/highlights/src/lib.rs`), `fancy_single_kill_is_showpiece`, `plain_single_kill_is_ignored`, `rejects_garbage` (`crates/faceit/src/lib.rs`), `takes_the_latest_take_and_counts_duration` (`crates/renderer/src/assemble.rs`), `job_lifecycle` (`crates/store/src/lib.rs`).

**Types:**
- `PascalCase` structs and enums: `Highlight`, `Kind` (`crates/highlights/src/lib.rs`), `PositionSource`, `NoPositions`, `Pose` (`crates/director/src/lib.rs`), `Progress`, `Stage` (`crates/pipeline/src/lib.rs`), `JobStatus`, `JobKind`, `JobSource`, `NewJob`, `JobOutcome` (`crates/store/src/lib.rs`).
- Newtype IDs wrap primitives: `PlayerId(i32)` in `crates/cs2`.
- Enum variants are `PascalCase` and used in `match` exhaustively: `Kind::Showpiece`, `Score::Epic`/`Great`/`Good` as described in `crates/director/src/lib.rs` module docs.
- Score/threshold constants are `SCREAMING_SNAKE_CASE`: `EPIC_SCORE`, `GREAT_SCORE` (`crates/director/src/lib.rs`), `CANCELLED` (`crates/pipeline/src/lib.rs`).

**Variables:**
- `snake_case`, short names are common in geometry and math code (`a`, `b`, `c`, `d` for triangle vertices in `crates/geometry/src/lib.rs`; `tr` / `TR` for tick rate in `crates/director/src/rig.rs`).

**Localization of identifiers and text:**
- Identifiers and code are English. User-facing strings, CLI usage text, doc comments and most inline comments are written in Russian. Examples: `const USAGE` in `crates/cli/src/main.rs`, `//!` module docs in `crates/pipeline/src/lib.rs` and `crates/director/src/lib.rs`, the `ошибка: {e}` error prefix in `crates/cli/src/main.rs`, test assertion messages such as `"demoparser2: 173 убийств после begin_new_match"` in `crates/positions/tests/real_demo.rs`.
- Follow this: new doc comments and user-facing messages should be in Russian to match the surrounding code; identifiers in English.

## Code Style

**Formatting:**
- No `rustfmt.toml` / `.rustfmt.toml` is committed. Code is formatted with rustfmt defaults plus a wide line budget: many lines in `crates/app/src/mock.rs` and `crates/store/src/lib.rs` run well past 100 columns, and struct literals are kept on one line.
- Multi-item `use` blocks are grouped inside braces and may span lines (`crates/cli/tests/pipeline.rs`).
- All workspace crates use edition 2024; keep the edition already set in each crate's `Cargo.toml`.
- Run `cargo fmt` before committing; there is no CI check enforcing it.

**Linting:**
- No `[lints]` table in any `Cargo.toml`, no `clippy.toml`, no crate-level `#![deny(...)]`/`#![warn(...)]`.
- The only explicit lint suppressions are `#[allow(clippy::too_many_arguments)]` on `crates/director/src/rig.rs:91` and `crates/director/src/rig.rs:434`. Prefer a struct parameter over adding more `allow`s.
- CI (`.github/workflows/ci.yml`) runs `cargo test` and `cargo build` only; clippy is not run. Run `cargo clippy --workspace` locally before large changes.

## Import Organization

**Order (as practiced):**
1. `std::...` imports (`use std::{env, error::Error, path::PathBuf, process::ExitCode};` in `crates/cli/src/main.rs`).
2. External crates (`use serde::{Deserialize, Serialize};`, `use prost::Message;`).
3. Workspace crates (`use cs2::{Match, PlayerId};`, `use highlights::{Highlight, Utility, score_kill};`, `use director::{NoPositions, Order, PositionSource};`).
4. `crate::` / `super::` items.

Blank line between groups. Test modules start with `use super::*;` (see `crates/geometry/src/lib.rs:396`, `crates/highlights/src/lib.rs:202`).

**Path aliases:** None. Crates are referenced by their package name (`cs2::`, `demo::`, `store::`).

**Re-exports:** Crate roots re-export the public surface with `pub use`, e.g. `pub use grenade::{grenade_victims, shoot_grenade};` in `crates/director/src/lib.rs` and `pub use parse::{Parsed, PlayerInfo, parse};` in `crates/demo/src/lib.rs`. Internal modules are private (`mod grenade;`, `mod bits;`, `mod command;`); only the public-facing ones are `pub mod` (`pub mod rig;`, `pub mod events;`, `pub mod proto`).

## Error Handling

The codebase uses three error styles. Pick by layer:

**Pipeline and store (`String` errors):**
- `pub type Result<T> = std::result::Result<T, String>;` in `crates/pipeline/src/lib.rs:10` and `crates/store/src/lib.rs:16`.
- Errors are converted with `.map_err(|e| e.to_string())` or `format!`. Callers display the string directly.
- Cancellation is a sentinel string: `pub const CANCELLED: &str = "отменено";` in `crates/pipeline/src/lib.rs`, checked with `check(p)?` between stages. The GUI compares against this constant to distinguish cancel from failure.

**Libraries that box errors (`Box<dyn Error>`):**
- `crates/geometry/src/lib.rs` (`from_bytes`, `load`, `load_map`, `fetch`, `latest_release_tag`), `crates/positions/src/lib.rs` (`track`), `crates/cli/src/main.rs` (`run`, `list_cameras`, `assemble_cmd`) return `Result<T, Box<dyn Error>>`.
- `crates/faceit/src/lib.rs:14` defines a private alias `type Result<T> = std::result::Result<T, Box<dyn Error>>;`. Follow this pattern when a crate needs a local alias.

**Binaries:**
- `crates/cli/src/main.rs` prints errors with `eprintln!("ошибка: {e}")`, prints usage on argument errors, and returns `ExitCode`. No `anyhow`/`thiserror` dependency exists in the workspace.

**Propagation:**
- `?` is the default. `unwrap()` / `expect()` appear mostly in tests and in test-only fixtures. Non-test uses should be limited to invariants that cannot fail.
- Missing optional data uses `Option` and `let ... else` (`let Some(data) = load() else { return };` in `crates/positions/tests/real_demo.rs`).
- Graceful degradation is preferred over hard failure for optional inputs: the director falls back to POV/chase cameras when there is no position source (`crates/director/src/lib.rs` module docs), and `--no-walls` / `--offline` flags skip geometry downloads (`crates/cli/src/main.rs` USAGE).

**Validation of external input:**
- Demo files are checked by magic bytes before parsing: `data.starts_with(b"PBDEMS2\0")` (`crates/positions/tests/real_demo.rs`). LFS pointer files produce an explicit error (see `lfs_pointer_gives_clear_error` in `crates/cli/tests/pipeline.rs`).

## Logging

**Framework:** None. No `log`, `tracing`, or `env_logger` crate is used.

**Patterns:**
- CLI output: `println!` for results and `eprintln!` for errors (38 occurrences of `println!`/`eprintln!` across `crates/`).
- Pipeline progress goes through the `Progress` trait, not stdout: `fn stage(&self, s: Stage)` and `fn log(&self, msg: &str)` in `crates/pipeline/src/lib.rs`. Implementations include a silent `Quiet` for tests and console, and a GUI sink that writes to the job log in `crates/store/src/lib.rs` (`append_log`).
- New long-running steps should report via `Progress::stage` / `log` and call `check(p)?` at stage boundaries so cancellation works.

## Comments

**When to Comment:**
- Module-level `//!` docs describe the purpose and the design rule of the module (`crates/director/src/lib.rs`, `crates/pipeline/src/lib.rs`, `crates/highlights/src/lib.rs`). Every library `lib.rs` has one.
- Item-level `///` docs on public types and traits, especially trait methods with non-obvious contracts (`PositionSource::raycast`, `Progress::cancelled` in `crates/pipeline/src/lib.rs`).
- Inline `//` comments explain why, including units and conventions: `/// pitch, yaw, roll в градусах (как в Source).` on `Pose.ang` in `crates/director/src/lib.rs`.
- Known unverified data is marked in comments as `TODO(verify)` (see README note about awpy-data paths).

**JSDoc/TSDoc:** Not applicable. Rust `///` doc comments are used instead; no `#![doc]` attributes or doc-tests are configured.

## Function Design

**Size:** Pipeline entry points are long orchestration functions that delegate to helpers; keep new logic in named helpers (`load_positions`, `make`, `plan_player`).

**Parameters:**
- Pass slices and borrowed data: `&[u8]`, `&Match`, `&Path`, `&dyn Progress`.
- Many-parameter functions are tolerated with explicit `#[allow(clippy::too_many_arguments)]` (`crates/director/src/rig.rs`). Prefer an options struct: `PlanOptions` in `crates/pipeline/src/lib.rs`, `Config` in `crates/highlights/src/lib.rs`, `Options` in `crates/geometry/src/lib.rs`.
- Dependency injection uses trait objects: `&dyn Progress`, `&dyn PositionSource`, `&dyn Fn`-style closures.

**Return Values:**
- `Result<T, String>` or `Result<T, Box<dyn Error>>` per the error style above. Optional lookups return `Option<T>` (`eye_pose`, `grenade_path`, `Store::job`).
- Constructors for test data use `Match::build(...)` rather than struct literals where the struct has derived defaults.

## Module Design

**Exports:**
- Public API goes through the crate root. Internal helpers are `fn` without `pub` or `pub(crate)` (only 3 `pub(crate)`/`pub(super)` uses in the workspace, so prefer private).
- Derives on data types are standard: `#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]` on `Pose` (`crates/director/src/lib.rs`), `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]` on `Kind` (`crates/highlights/src/lib.rs`). Use `serde` derives for anything persisted to JSON or SQLite.

**Barrel Files:** `lib.rs` acts as the barrel: `pub use` of selected items, `pub mod` for namespaces such as `rig`, `events`, `proto`, `stats`, `assemble`, `report`.

**Dependencies between crates:** Acyclic layering. `demo` (parsing) -> `cs2` (match model) -> `highlights` (scoring) -> `director` (shot plan) -> `pipeline` (orchestration) -> `app` / `cli`. Shared geometry in `geometry`, persistence in `store`, FACEIT API in `faceit`, video output in `renderer`. Keep new code in the lowest crate that owns the concept.

## Naming for Tests and Fixtures

- Test helpers are small private `fn`s at the top of the test module: `fn kill(tick, round, attacker, victim) -> Kill`, `fn wall() -> Mesh`, `fn new_job(kind) -> NewJob`, `fn varint(...)` in `crates/cli/tests/pipeline.rs`.
- Constants inside tests are short: `const TR: f32 = 64.0;` (`crates/director/src/rig.rs`).

---

*Convention analysis: 2026-10-10*
