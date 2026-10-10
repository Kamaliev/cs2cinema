---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
# Testing Patterns

**Analysis Date:** 2026-10-10

## Test Framework

**Runner:**
- Built-in Rust test harness (`#[test]`), no external runner.
- Workspace: `Cargo.toml` at repo root, resolver 2, 12 member crates.
- Config: none beyond `Cargo.toml` (no `.config/nextest.toml`, no `[profile.test]` override observed).

**Assertion Library:**
- `std` macros only: `assert!`, `assert_eq!`, `assert_ne!` with a trailing message argument in many places (`assert_eq!(m.kills.len(), 173, "demoparser2: 173 убийств после begin_new_match");` in `crates/positions/tests/real_demo.rs`).
- No `pretty_assertions`, `rstest`, `proptest`, `insta`, `mockall`, or `tempfile` in any `Cargo.toml`.

**Dev Dependencies:**
- `crates/cli/Cargo.toml`: `[dev-dependencies] prost = "0.14.4"` (builds synthetic demo bytes for integration test).
- `crates/positions/Cargo.toml`: `[dev-dependencies]` pulls in `demo`, `cs2`, `highlights` path crates for the real-demo tests.

**Run Commands:**

```bash
cargo test --release --workspace                      # Run all tests (what CI runs; ubuntu-latest and windows-latest)
cargo test --release                                  # Run all tests (README; see README.md line 53)
cargo test --release -p positions --test real_demo -- --nocapture   # Real-demo checks (needs CS2_TEST_DEMO and CS2_TEST_MAPS)
cargo test --release -p highlights                    # One crate
cargo test -p store job_lifecycle                     # One test by name
cargo test -- --nocapture                             # Show println!/eprintln! output
cargo build --release -p cli                          # Build the CLI after tests (CI step)
```

- Watch mode: not configured (no `cargo-watch` in repo docs).
- Coverage: not configured (no `cargo-llvm-cov` or `tarpaulin` step in `.github/workflows/ci.yml`).

## Test File Organization

**Location:**
- Unit tests: co-located at the bottom of the source file in a `#[cfg(test)] mod tests { use super::*; ... }` block. Found in 17 files, including `crates/geometry/src/lib.rs:394`, `crates/director/src/rig.rs:537`, `crates/director/src/lib.rs:1016`, `crates/highlights/src/lib.rs:201`, `crates/pipeline/src/lib.rs:297`, `crates/positions/src/lib.rs:312`, `crates/renderer/src/lib.rs:585`, `crates/renderer/src/assemble.rs`, `crates/store/src/lib.rs:383`, `crates/faceit/src/lib.rs:149`, `crates/faceit/src/stats.rs`, `crates/cs2/src/lib.rs:418`, `crates/cs2/src/report.rs`, `crates/demo/src/bits.rs`, `crates/demo/src/events.rs`, `crates/director/src/grenade.rs`, `crates/director/src/rig.rs`.
- Integration tests: separate `tests/` directory per crate. Only two files exist:
  - `crates/cli/tests/pipeline.rs`: end-to-end on a synthetic demo.
  - `crates/positions/tests/real_demo.rs`: checks against a real FACEIT demo, skipped without env vars.
- `crates/app` (GUI, including `src/update.rs`) has no unit tests of its own beyond `crates/app/src/mock.rs` (test data for the Matches page, debug only via `CS2CINEMA_MOCK=1|details`).

**Naming:**
- Test functions are descriptive sentences in `snake_case`, not `test_` prefixed: `chain_splits_by_gap_and_ace_dominates`, `plain_single_kill_is_ignored`, `rejects_garbage`, `parses_room_urls_and_bare_ids`, `catalog_has_forty_distinct_cameras_across_categories`, `lfs_pointer_gives_clear_error`.
- Real-demo test names state the property: `match_model_agrees_with_reference_parser`, `shooters_look_at_their_victims`, `grenades_have_real_flights`, `planned_cameras_respect_walls`.
- Test file names in `tests/` are topic names: `pipeline.rs`, `real_demo.rs`.

**Structure:**

```
crates/<crate>/src/<module>.rs
  ...impl...
  #[cfg(test)]
  mod tests {
      use super::*;
      fn helper(...) -> T { ... }     // builders at top
      #[test]
      fn behavior_in_words() { ... }
  }

crates/<crate>/tests/<topic>.rs        // integration, uses the crate as an external user
```

## Test Structure

**Suite Organization (unit):**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn kill(tick: i32, round: u32, attacker: i32, victim: i32) -> Kill {
        Kill { tick, round, attacker: PlayerId(attacker), victim: PlayerId(victim),
               assister: None, weapon: "ak47".into(), headshot: false, /* ... */ distance: 10.0 }
    }

    #[test]
    fn fancy_single_kill_is_showpiece() {
        let m = match_with(vec![kill(1000, 1, 1, 2)]);
        // assertions on the derived highlight
    }
}
```

Source: pattern from `crates/highlights/src/lib.rs:201-260`.

**Patterns:**
- Setup: private builder functions in the test module (`fn kill`, `fn wall`, `fn new_job`, `fn match_with`). No shared fixtures module.
- Teardown: none explicit. Tests that touch the filesystem create unique dirs under `std::env::temp_dir()` (see `crates/renderer/src/assemble.rs:224-231`, which calls `fs::create_dir_all` and `fs::write`). Clean up manually if the test writes outside temp.
- In-memory database for store tests: `Store::in_memory().unwrap()` (`crates/store/src/lib.rs:383+`). Prefer this over file DBs in tests.
- Silent progress sink for pipeline tests: `analyze(&Source::File(demo), &Quiet).unwrap()` (`crates/pipeline/src/lib.rs:317`). Use `Quiet` (defined in `crates/pipeline/src/lib.rs`) instead of writing a new `Progress` impl.
- Assertions: plain `assert_eq!` on tuples to check several fields at once: `assert_eq!((j.status, j.shots, j.result.is_some()), (JobStatus::Done, Some(12), true));` (`crates/store/src/lib.rs`). Use a message when the number has a source.

**Skip pattern for environment-dependent tests:**
- Tests that need data outside the repo return early instead of failing:

```rust
fn load() -> Option<Vec<u8>> {
    let path = std::env::var_os("CS2_TEST_DEMO")?;
    let data = demo::read_demo(path).ok()?;
    data.starts_with(b"PBDEMS2\0").then_some(data)
}

#[test]
fn match_model_agrees_with_reference_parser() {
    let Some(data) = load() else { return };
    ...
}
```

Source: `crates/positions/tests/real_demo.rs:8-20`. A test that returns early reports as passed; run with `--nocapture` and check the printed output when verifying real-demo behavior.

## Mocking

**Framework:** None. No mocking crate. Dependency substitution is done with traits and hand-written stubs.

**Patterns:**
- Trait-based stubs: `NoPositions` (`crates/director/src/lib.rs`) implements `PositionSource` returning `None` everywhere; use it in tests and in the CLI with `--no-flybys`.
- Default trait methods act as no-op doubles: `PositionSource::raycast` returns `None` (no walls), `Progress::cancelled` returns `false`. Override only the methods a test needs.
- `Quiet` progress sink for silent runs (`crates/pipeline/src/lib.rs`).
- Synthetic input generation instead of mocks for parsers: `crates/cli/tests/pipeline.rs` encodes a fake demo with `varint`, `command`, and `inner` helpers using `prost` messages (`CDemoFileHeader`, `CDemoPacket`, `CMsgSource1LegacyGameEvent`).
- Fake assets in tests: `Mesh::from_triangles(vec![[a, b, c], [a, c, d]])` builds a 100x100 wall (`crates/geometry/src/lib.rs:398-405`) instead of loading a real `.mesh` file.
- Fake FACEIT data for the GUI: `crates/app/src/mock.rs` (debug builds only, `CS2CINEMA_MOCK=1` or `details`).

**What to Mock:**
- External services (FACEIT API, GitHub release lookup for map geometry): use the parsing/pure functions directly and test them with literal input (`parses_room_urls_and_bare_ids`, `rejects_garbage` in `crates/faceit/src/lib.rs`). Do not hit the network from tests.
- Position and geometry sources: implement `PositionSource` with a stub.

**What NOT to Mock:**
- The SQLite store: use `Store::in_memory()`; it is cheap and exercises the real SQL.
- Demo parsing in pipeline tests: use the synthetic demo generator in `crates/cli/tests/pipeline.rs` so the real parser runs.
- Video assembly logic: `crates/renderer/src/assemble.rs` tests operate on real directory listings it creates itself.

## Fixtures and Factories

**Test Data:**

```rust
// builder with defaults, override fields in the test
fn new_job(kind: JobKind) -> NewJob {
    NewJob { kind, source: JobSource::Faceit("1-abc".into()), label: "матч".into(),
             hero_name: "me".into(), hero_steam: 42, map: "de_nuke".into() }
}
```

**Location:**
- Inline in each test module. No `tests/fixtures/` or `testdata/` directory exists in the repo.
- Map geometry meshes live in `maps/` (`maps/de_nuke.mesh` etc., git-ignored by `/maps` in `.gitignore`), downloaded by the app/CLI. Tests that need them must use `CS2_TEST_MAPS=<dir>`.
- Real demos (`*.dem`, `*.dem.zst`) are Git LFS objects (`.gitattributes`) and are not checked in as working files; set `CS2_TEST_DEMO` to a local copy to enable real-demo tests.

## Coverage

**Requirements:** None enforced. No coverage threshold in CI.

**View Coverage:**

```bash
cargo install cargo-llvm-cov            # one-time, if not installed
cargo llvm-cov --workspace --release    # not part of CI; run manually
```

## Test Types

**Unit Tests:**
- Pure logic: scoring (`highlights`), camera catalog and rig math (`director/src/rig.rs`, `director/src/grenade.rs`), geometry (`geometry`), SQL store (`store`), URL/ID parsing (`faceit`), bit reading (`demo/src/bits.rs`), protobuf event handling (`demo/src/events.rs`), statistics (`faceit/src/stats.rs`), clip sequencing (`renderer/src/assemble.rs`).
- Assertions check invariants and counts (`assert!(names.len() >= 40)`, `assert_eq!(names.len(), PRESETS.len())` in `crates/director/src/rig.rs`).

**Integration Tests:**
- `crates/cli/tests/pipeline.rs`: runs synthetic demo bytes through container -> events -> highlights -> plan -> files. Runs on every CI build; no external input needed.
- `crates/positions/tests/real_demo.rs`: validates parsed positions, kill counts, round counts, camera planning against walls, and map alignment against a reference parser (demoparser2) on a known match (de_nuke). Skipped in CI because demos are LFS objects.

**E2E Tests:**
- Not used in the automated suite. Manual end-to-end runs are documented in `README.md` (`cs2-cli.exe C:\demos\match.dem --out out\test --length 60`) and the GUI is verified by screenshot or by running the app.

## Verification Notes (from README)

- `README.md` documents `cargo test --release` as the full test run and lists the environment variables for real-demo checks at lines ~206-208 and ~302-303.
- The CI workflow `.github/workflows/ci.yml` runs `cargo test --release --workspace` on `ubuntu-latest` and `windows-latest`, then builds `cs2-cli` and uploads the Windows binary.
- `.github/workflows/release.yml` handles release packaging (not analyzed for test steps).

## Common Patterns

**Async Testing:**
- Not applicable. No async runtime is used in tests; the pipeline is synchronous and the GUI runs jobs on threads with the `Progress` trait for cancellation.

**Error Testing:**

```rust
#[test]
fn rejects_garbage() {
    assert!(parse("not a room").is_err());   // pattern from crates/faceit/src/lib.rs:166
}

#[test]
fn lfs_pointer_gives_clear_error() {
    // feeds an LFS pointer text to the pipeline and checks the message (crates/cli/tests/pipeline.rs:116)
}
```

- Check the error message content when it is user-facing (LFS pointer case), otherwise check `is_err()` / `is_none()`.

**Cancellation Testing:**
- Implement `Progress` with a `cancelled()` override returning `true` and assert the pipeline returns `Err(CANCELLED)` (`crates/pipeline/src/lib.rs` `check`). Good template for new stage-based code.

## Recommendations for New Tests

- Put unit tests in the same file's `#[cfg(test)] mod tests`; add an integration test under `crates/<crate>/tests/` only when the test needs the public API across crates or the synthetic demo generator.
- Gate tests that need external files with an env var and an early `return`, as in `real_demo.rs`.
- Use `Store::in_memory()` and `Quiet` rather than new fixtures.
- Add `cargo clippy --workspace --all-targets` and `cargo fmt --check` to CI if the team wants enforcement; neither runs today.

---

*Testing analysis: 2026-10-10*
