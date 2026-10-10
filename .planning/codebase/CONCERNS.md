---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
<!-- refreshed: 2026-10-10 -->

# Codebase Concerns

**Analysis Date:** 2026-10-10

## Working-Tree State

**Large uncommitted change set:**
- Issue: 20 tracked files are modified (about 2,580 insertions and 730 deletions), spanning `crates/app/src/*`, `crates/director/src/{lib,grenade,rig}.rs`, `crates/renderer/src/lib.rs`, `crates/pipeline/src/lib.rs`, `crates/positions/src/lib.rs`, `crates/positions/tests/real_demo.rs`, `crates/highlights/src/lib.rs`, `crates/cli/src/main.rs`, and `README.md`. Recent commits (`469e893`, `19de671`) cover the job queue and director work that is still being edited.
- Impact: Behaviour on `main` and in the working tree diverge. Regressions are hard to bisect, and the committed history does not match what is being run.
- Fix approach: Split the diff into logical commits (renderer/ffgraph, director, app job queue) and run `cargo test --workspace` before each commit.

**CRLF/LF churn:**
- Issue: Git warns "LF will be replaced by CRLF" for nearly every modified source file, so line-ending normalisation is not configured consistently.
- Files: `README.md`, `crates/app/src/*.rs`, `crates/cli/src/main.rs`, `crates/renderer/src/lib.rs` and others.
- Impact: Noisy diffs; risk of whole-file diffs in review.
- Fix approach: Add a `.gitattributes` (e.g. `* text=auto eol=crlf` or `eol=lf`) and renormalise once in a dedicated commit.

## Tech Debt

**Hand-built ffmpeg filter graph and batch script:**
- Issue: `crates/renderer/src/lib.rs` builds `assemble.ffgraph` and a Windows `.bat` as string concatenation. Text escaping for `drawtext` is manual (`'\\'` to `/`, `':'` to `"\\:"`, around lines 415-435), and the bat generator converts `/` to `\` (line 578).
- Files: `crates/renderer/src/lib.rs`, `crates/renderer/src/assemble.rs`
- Impact: Titles or paths containing quotes, commas, `%`, `;` or non-ASCII characters can produce a broken graph that only fails at ffmpeg runtime. The `.bat` output is untested on paths with spaces beyond what the unit test covers (`crates/renderer/src/lib.rs` ~line 665).
- Fix approach: Centralise escaping in one function with property tests (round-trip through ffmpeg `-filter_complex` in an integration test), and quote all paths in the generated bat.

**Duplicated ffmpeg/ffprobe invocation:**
- Issue: ffmpeg is spawned from at least five places with separate argument sets and separate handling of `-hide_banner`, `no_window`, and error reporting: `crates/app/src/app.rs:559`, `crates/app/src/jobs.rs:314` (`run_ffmpeg`), `crates/app/src/launch.rs:137`, `crates/renderer/src/assemble.rs:121,207,313`, and `crates/cli/src/main.rs`.
- Impact: Behaviour (progress reporting, error text, window hiding) differs by caller. Fixes must be applied several times.
- Fix approach: One shared `ffmpeg` runner module in `crates/renderer` used by app, cli and tests.

**Duplicated Windows-path normalisation:**
- Issue: Path-to-forward-slash conversion is reimplemented in `crates/cli/src/main.rs:348-353` (`replace('\\', "/")`, `\\?\` prefix stripping), `crates/pipeline/src/lib.rs:273-279`, `crates/app/src/launch.rs:30` (`replace("\\\\", "/")` on a parsed config value), and `crates/renderer/src/lib.rs:421,578`.
- Impact: Inconsistent handling of UNC paths, `\\?\` verbatim prefixes and escaped backslashes; a fix in one place does not reach the others.
- Fix approach: A single `cs2_path` helper (e.g. in `crates/geometry` or a new small crate) used everywhere.

**Localised UI and user-facing strings mixed with code identifiers:**
- Issue: Error and help text is hard-coded in Russian across crates (`crates/app/src/launch.rs:99`, `crates/cli/src/main.rs:31,332,420`, `crates/app/src/settings_page.rs:58`). There is no i18n layer.
- Impact: Non-Russian users cannot read errors; changing wording requires touching logic files.
- Fix approach: Keep language choice as a deliberate product decision, but move strings into a `messages` module so they are not scattered.

**Large files with mixed responsibilities:**
- Issue: `crates/director/src/lib.rs` (1,328 lines), `crates/director/src/grenade.rs` (853), `crates/renderer/src/lib.rs` (797), `crates/director/src/rig.rs` (665), `crates/app/src/app.rs` (617), `crates/geometry/src/lib.rs` (519), `crates/cs2/src/lib.rs` (513), `crates/store/src/lib.rs` (442), `crates/app/src/match_page.rs` (440), `crates/cli/src/main.rs` (430).
- Impact: Hard to review; the director crate in particular mixes shot selection, camera rigs and grenade logic.
- Fix approach: Split `director/src/lib.rs` into `shots.rs`, `selection.rs`, and `plan.rs`; move CLI subcommands into one module each.

**Process and Windows-specific code in the app crate:**
- Issue: `crates/app/src/launch.rs` contains HLAE launch, `tasklist`/`taskkill` polling, and config-file writing. `crates/app/src/ui.rs:275-281` opens files via `cmd /C start` and `xdg-open`.
- Impact: Launch logic cannot be exercised without a Windows desktop with CS2 installed.
- Fix approach: Put process control behind a trait with a fake implementation for tests.

## Known Bugs and Failure Modes

**Game liveness check defaults to "alive" on error:**
- Symptoms: If `tasklist` fails, `game_alive()` returns `true`, so waiting loops keep polling until the 20-second or other timeout elapses, or indefinitely if the caller has no timeout.
- Files: `crates/app/src/launch.rs:249-262` (`game_alive`), used by `wait_for_clips` in `crates/app/src/launch.rs` around line 290-320.
- Trigger: `tasklist` unavailable or blocked by policy.
- Workaround: Cancel the job from the queue (`crates/app/src/jobs.rs`).

**Killing CS2 kills every instance:**
- Symptoms: `kill_game()` runs `taskkill /IM cs2.exe /F`, which terminates any running CS2 process, including a player's own unrelated match.
- Files: `crates/app/src/launch.rs:268-276`.
- Trigger: Job cancellation or error recovery while the user is playing.
- Workaround: None. Consider filtering by PID that the app launched.

**Missing or LFS-pointer demo data is only partly handled:**
- Symptoms: The test `lfs_pointer_gives_clear_error` in `crates/cli/tests/pipeline.rs` shows the case is known, but the app path does not surface it with the same clarity.
- Files: `crates/cli/tests/pipeline.rs:116`, `crates/pipeline/src/lib.rs`.
- Trigger: A demo checked out via Git LFS without `git lfs pull`.

**Clip-discovery fragility:**
- Symptoms: Clips are matched by directory modification time (`since: SystemTime`) and "stable for N seconds" (3 s after the finish marker, 20 s otherwise). A slow disk or a paused game can produce a truncated or missing take.
- Files: `crates/app/src/launch.rs` (`wait_for_clips`), `crates/renderer/src/assemble.rs` (`ffprobe` checks).
- Trigger: Heavy disk load, or HLAE writing a take across a directory rename.

**Preview recording is not validated before the full run:**
- Symptoms: The `assemble` code path generates a synthetic `testsrc` clip for checks (`crates/renderer/src/assemble.rs:313-316`), but real HLAE output is not probed for frame count before assembly.
- Files: `crates/renderer/src/assemble.rs`.

## Security Considerations

**Shell-like invocation of `cmd /C start`:**
- Risk: `crates/app/src/ui.rs:275-276` passes a path to `cmd /C start "" <path>`. Paths containing `&`, `^` or `%` are interpreted by `cmd`.
- Files: `crates/app/src/ui.rs`.
- Current mitigation: The path comes from local files the app itself produced; no network input reaches it.
- Recommendations: Use `explorer.exe <path>` or the `opener` crate, which avoids `cmd` parsing.

**Subprocess arguments built from user-entered paths:**
- Risk: Folder settings (`cs2_dir`, `hlae_dir`, ffmpeg path) are typed into the UI and passed to `Command::new` (`crates/app/src/launch.rs:166-171`). Arguments are passed as separate argv entries, which is safe, but the ffmpeg filter string embeds titles and font paths (`crates/renderer/src/lib.rs:432`).
- Current mitigation: `Command` avoids shell expansion; font path escaping is manual.
- Recommendations: Validate that the ffmpeg path resolves to an executable before use; add tests for titles containing `'`, `:` and `,`.

**Remote data fetched by `git ls-remote`:**
- Risk: `crates/geometry/src/lib.rs:293-306` runs `git ls-remote --tags` against the awpy-data repository and trusts the newest numeric tag to pick map data.
- Files: `crates/geometry/src/lib.rs`.
- Current mitigation: Only numeric tags are parsed (`newest_tag`); downloaded content is not executed.
- Recommendations: Pin the expected release, verify a checksum for downloaded map files, and fail clearly when offline.

**Demo files are large and user-supplied:**
- Risk: `.dem` and `.dem.zst` are git-ignored (`.gitignore`), but parsing malformed demos depends on the `demo` crate's bit reader (`crates/demo/src/bits.rs`) and protobuf build script (`crates/demo/build.rs`).
- Current mitigation: `.gitignore` keeps demo binaries out of the repository.
- Recommendations: Fuzz the demo parser; bound allocations driven by length fields.

**Possible credential exposure:** None found in tracked source. `.env` and similar files were not read during this analysis; confirm they are ignored before committing.

## Performance Bottlenecks

**Fixed-interval polling loops:**
- Problem: The job worker sleeps 250 ms (`crates/app/src/jobs.rs:218`) and 300 ms (`crates/app/src/jobs.rs:353`); `launch.rs` sleeps 1 s in two loops (lines 193, 312). Each poll of `game_alive` spawns `tasklist`.
- Files: `crates/app/src/jobs.rs`, `crates/app/src/launch.rs`.
- Cause: Spawning a process per poll is expensive, and each `tasklist` call takes tens of milliseconds.
- Improvement path: Use the child-process handle HLAE returns, or `WaitForSingleObject` on the CS2 PID, instead of polling `tasklist`.

**Whole-file hashing on every check:**
- Problem: `crates/geometry/src/lib.rs` (around line 286-292) reads every output file fully to compute an FNV-style hash over names and bytes.
- Files: `crates/geometry/src/lib.rs`.
- Cause: Content is re-read even for large video outputs.
- Improvement path: Hash only metadata (size and mtime) or use a streaming hash for files above a threshold.

**Full-demo in-memory processing:**
- Problem: Demo events and positions are collected in memory (`crates/positions/src/lib.rs`, `crates/demo/src/events.rs`), and `crates/director/src/lib.rs` builds per-tick structures for the whole match.
- Files: `crates/positions/src/lib.rs`, `crates/director/src/lib.rs`.
- Cause: No streaming; memory grows with match length and player count.
- Improvement path: Stream by round, or window the position data to the selected shots.

**Synchronous ffmpeg in UI-adjacent paths:**
- Problem: `crates/app/src/app.rs:559` runs `ffmpeg` directly from the app layer.
- Files: `crates/app/src/app.rs`.
- Improvement path: Route all ffmpeg calls through the job queue (`crates/app/src/jobs.rs`), which already has progress reporting.

## Fragile Areas

**External process orchestration (HLAE, CS2, ffmpeg):**
- Files: `crates/app/src/launch.rs`, `crates/app/src/jobs.rs`, `crates/renderer/src/assemble.rs`
- Why fragile: Depends on HLAE's `AfxHookSource2.dll` layout (`crates/app/src/launch.rs:99`), CS2 console output markers for "take finished", directory names, and `steam.inf` format (`crates/geometry/src/lib.rs:275-279`). Any upstream change breaks recording silently or after a long timeout.
- Safe modification: Change one detector at a time and run a recording against a real CS2 install (the mocks in `crates/cli/tests/pipeline.rs` do not cover HLAE or CS2).
- Test coverage: No automated test drives HLAE or CS2.

**Director heuristics (grenades, peeks, rigs):**
- Files: `crates/director/src/grenade.rs`, `crates/director/src/rig.rs`, `crates/director/src/lib.rs`
- Why fragile: Scoring and threshold constants (`score`, `gap`, KAST, zoom distance) are hard-coded and tuned on specific demos. Unwrap-style `expect("залп не пуст")` in `crates/director/src/lib.rs:378,511` relies on an invariant of the grouping loop.
- Safe modification: Add a regression test with a fixed demo before changing constants. Keep `expect` messages describing the invariant.
- Test coverage: Unit tests exist in `crates/director/src/*.rs`, but use synthetic data for the thresholds.

**Unwrap and expect density:**
- Files (non-test code, approximate counts): `crates/director/src/lib.rs` (16), `crates/director/src/rig.rs` (10), `crates/director/src/grenade.rs` (12), `crates/renderer/src/lib.rs` (15), `crates/renderer/src/assemble.rs` (14), `crates/geometry/src/lib.rs` (14), `crates/store/src/lib.rs` (38, mostly in tests from line ~390).
- Why fragile: A malformed demo or unexpected empty set panics the whole app rather than failing a single job.
- Safe modification: Convert `unwrap()` on I/O and parsing to `?` with context; keep `expect` only for invariants.
- Test coverage: The panic path is not tested.

**Job queue and cancellation:**
- Files: `crates/app/src/jobs.rs` (373 lines), `crates/store/src/lib.rs` (`add_job`, `next_queued`, `start_job`, `set_stage`, `append_log`)
- Why fragile: Jobs are persisted to SQLite while a worker thread runs them. Cancellation sets a flag that is checked only inside poll loops (`wait_for_clips` gets `cancelled`), so a job blocked in a synchronous `Command::output()` cannot be cancelled. On startup `reset_running` (`crates/store/src/lib.rs:340`) marks interrupted jobs as `failed`; they are not resumed.

- Test coverage: `crates/store/src/lib.rs` tests cover queue ordering and logs; no test covers crash recovery or cancellation mid-process.

**Font loading from a fixed Windows path:**
- Files: `crates/app/src/ui.rs:84` (`C:\Windows\Fonts\...`)
- Why fragile: Hard-coded system drive; falls back silently to egui defaults if the file is missing, which may render Cyrillic incorrectly.
- Safe modification: Resolve `%WINDIR%` via environment variable.

**Git LFS and awpy-data dependency:**
- Files: `crates/geometry/src/lib.rs`, `crates/pipeline/src/lib.rs`
- Why fragile: Map data is pulled through the `git` CLI at runtime. Requires `git` on PATH and network access.

## Scaling Limits

**Single-match processing:**
- Current capacity: One demo at a time per job; the queue is serial.
- Limit: Long matches (several hours of demo) multiply memory use from the in-memory position store.
- Scaling path: Process rounds independently and persist intermediate results in SQLite.

## Dependencies at Risk

**HLAE (third-party, Windows-only):**
- Risk: Requires a current HLAE build with `x64\AfxHookSource2.dll` (`crates/app/src/launch.rs:99`); the code checks for it but a CS2 patch can break the hook.
- Impact: Recording stops working until HLAE updates.
- Migration plan: Surface the HLAE version in the app and pin the tested version in the README.

**CS2 console and file layout:**
- Risk: Take folders, console markers and `steam.inf` `ClientVersion` are undocumented internals.
- Impact: Silent recording failures after game updates.
- Migration plan: Record the `ClientVersion` alongside each job and warn when it differs from the map-data release tag.

**ffmpeg on PATH:**
- Risk: Several call sites assume `ffmpeg` is on PATH (`crates/app/src/jobs.rs:314`) and the app shows only an error string if it is missing.
- Impact: Jobs fail after the recording step has already run.
- Migration plan: Check ffmpeg, ffprobe and HLAE at startup (`crates/app/src/status.rs` could host the check).

## Missing Critical Features

**No end-to-end test against real CS2 or HLAE:**
- Problem: Automation relies on manual runs. `crates/positions/tests/real_demo.rs` exercises a real demo, but depends on local data that is not in the repository.
- Blocks: Safe refactoring of the recording and assembly stages.

**CI coverage:**
- Problem: `.github/workflows/ci.yml` runs `cargo test --release --workspace` and `release.yml` exists; the uncommitted changes listed above are not yet covered by a commit-level gate.
- Blocks: Catching regressions before merge of the uncommitted changes.

## Test Coverage Gaps

**Desktop app (`crates/app`):**
- What's not tested: Job queue state machine, `wait_for_clips`, `kill_game`, settings persistence, UI layout. Only `crates/app/src/app.rs` contains a single unwrap-style assertion.
- Files: `crates/app/src/jobs.rs`, `crates/app/src/launch.rs`, `crates/app/src/settings.rs`
- Risk: Cancellation and polling regressions go unnoticed.
- Priority: High

**Renderer and ffgraph generation:**
- What's not tested: Escaping of titles and paths in filter strings against a real ffmpeg run.
- Files: `crates/renderer/src/lib.rs`, `crates/renderer/src/assemble.rs`
- Risk: Broken montage output that only appears when a user types an unusual title.
- Priority: High

**Director scoring thresholds:**
- What's not tested: Behaviour on real demos across maps; only synthetic data is used.
- Files: `crates/director/src/grenade.rs`, `crates/director/src/rig.rs`
- Risk: Tuned constants regress silently.
- Priority: Medium

**Faceit integration:**
- What's not tested: Network failure, rate limiting and malformed responses in `crates/faceit/src/lib.rs` and `crates/faceit/src/stats.rs`.
- Files: `crates/faceit/src/lib.rs`, `crates/faceit/src/stats.rs`
- Risk: Summary page shows wrong or empty stats without a clear error.
- Priority: Medium

---

*Concerns audit: 2026-10-10*
