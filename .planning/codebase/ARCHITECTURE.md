---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
<!-- refreshed: 2026-10-10 -->

# Architecture

**Analysis Date:** 2026-10-10

## System Overview

```text
┌──────────────────────────────────────────────────────────────────────────┐
│                         Entry points (binaries)                          │
│  `crates/app/src/main.rs` (egui desktop app)  `crates/cli/src/main.rs`   │
└───────────────┬──────────────────────────────────────┬───────────────────┘
                │ jobs, pages, settings                │ args -> pipeline
                ▼                                      ▼
┌───────────────────────────────┐        ┌─────────────────────────────────┐
│ App layer (`crates/app`)      │        │ Orchestration (`crates/pipeline`)│
│ app.rs, jobs.rs, launch.rs,   │───────▶│ analyze() -> plan_player() ->    │
│ *_page.rs, player.rs, ui.rs   │        │ Progress/Stage reporting         │
└──────┬─────────────┬──────────┘        └──┬──────────┬──────────┬────────┘
       │             │                      │          │          │
       ▼             ▼                      ▼          ▼          ▼
┌────────────┐ ┌────────────┐      ┌────────────┐ ┌──────────┐ ┌──────────────┐
│ store      │ │ faceit     │      │ demo       │ │ director │ │ renderer     │
│ SQLite     │ │ API + DL   │      │ .dem parse │ │ camera   │ │ HLAE scripts │
│ jobs/cache │ │            │      │ protobuf   │ │ planning │ │ ffmpeg graph │
└────────────┘ └────────────┘      └─────┬──────┘ └────┬─────┘ └──────────────┘
                                         ▼             ▲
                              ┌──────────────────┐  ┌─┴────────────┐
                              │ cs2 (match model)│─▶│ highlights   │
                              │ rounds, kills    │  │ scoring      │
                              └──────────────────┘  └──────────────┘
                                         ▲
                 ┌───────────────────────┴──────────┐
                 │ positions (ticks -> poses)       │   ┌──────────────┐
                 │ geometry (map meshes, raycast)   │──▶│ PositionSource│
                 └──────────────────────────────────┘   │ trait (director)│
                                                        └──────────────┘
```

## Pattern Overview

**Overall:** Layered Rust cargo workspace with a pipeline of pure-data stages. Each crate is a library with a narrow responsibility; two binaries (`app`, `cli`) are thin front-ends over the same `pipeline` crate. Stages exchange plain serializable structs (`Match`, `Timeline`, `Plan`), not shared mutable state.

**Key Characteristics:**
- Workspace members are declared in `Cargo.toml` (`crates/demo`, `highlights`, `director`, `cs2`, `renderer`, `faceit`, `positions`, `geometry`, `pipeline`, `store`, `app`, `cli`).
- Data is transformed in one direction: demo bytes -> `demo::Parsed` -> `cs2::Match` -> `highlights::Highlight` -> `director::Timeline` -> `renderer::Output` files -> HLAE recording -> `renderer::assemble` MP4.
- Domain types derive `serde::Serialize`/`Deserialize` so intermediate results persist as JSON (`plan.json`, `highlights.cfg`) and as SQLite rows (`crates/store/src/lib.rs`).
- Long-running work runs on `std::thread::spawn` threads, not an async runtime. No `tokio` dependency is declared in the workspace crates reviewed.
- Progress and cancellation are injected through the `pipeline::Progress` trait (`crates/pipeline/src/lib.rs:44`), with `pipeline::check` returning early when cancelled; `pipeline::Quiet` is the no-op implementation for the CLI.
- Position data is decoupled from direction via the `director::PositionSource` trait (`crates/director/src/lib.rs:33`); `director::NoPositions` (`crates/director/src/lib.rs:55`) provides a fallback so planning works without `positions`.

## Layers

**Demo ingestion (`demo`, `positions`):**
- Purpose: Read `.dem` / `.dem.zst` files, decode protobuf messages and entity state.
- Location: `crates/demo/src/` (`lib.rs`, `parse.rs`, `command.rs`, `events.rs`, `bits.rs`, `proto` generated from `crates/demo/proto/*.proto` via `crates/demo/build.rs`); `crates/positions/src/lib.rs`.
- Contains: `demo::read_demo`, `demo::parse` (returns `Parsed`, `PlayerInfo`), `positions::track` (per-tick samples, flights, team/alive state). `positions` uses the external `source2-demo` crate pinned at `=0.5.9` (`crates/positions/Cargo.toml`).
- Depends on: `cs2` (for `GrenadeKind`, `PlayerId`), `director` (for `Pose`, `PositionSource`), `geometry`, `highlights`.
- Used by: `cli` and `pipeline`.

**Domain model (`cs2`):**
- Purpose: Build a match model from parsed demo data.
- Location: `crates/cs2/src/lib.rs`, `crates/cs2/src/report.rs`.
- Contains: `Match::from_parsed` (`crates/cs2/src/lib.rs:105`), `Match::build`, `Match::round`, `Match::player`, `Match::team_at`; types `Kill`, `Grenade`, `Round`, `Player`, `PlayerId`.
- Depends on: `demo`.
- Used by: `highlights`, `director`, `renderer`, `positions`, `pipeline`, `app`, `cli`.

**Highlight scoring (`highlights`):**
- Purpose: Decide which kills and grenades are worth showing.
- Location: `crates/highlights/src/lib.rs`.
- Contains: `find` (`crates/highlights/src/lib.rs:74`), `score_kill`, `score_grenade`, `for_player`, `utility_for`; `Config` and `Highlight`/`Utility` types.
- Depends on: `cs2`.
- Used by: `director`, `pipeline`, `cli`.

**Geometry and faceit (`geometry`, `faceit`):**
- Purpose: Map collision meshes with BVH raycasting for wall checks (`geometry::Mesh::raycast`, `segment_clear`, `load_map`, `fetch`); FACEIT Data API client and demo download (`faceit::Client`, `demo_urls`, `download`, `parse_match_id`) plus player stats in `crates/faceit/src/stats.rs`.
- Location: `crates/geometry/src/lib.rs`, `crates/faceit/src/lib.rs`, `crates/faceit/src/stats.rs`.
- Depends on: no workspace crates (standalone). `geometry` reads `maps/*.mesh` and `maps/maps.json`.
- Used by: `positions`, `pipeline`, `cli`, `faceit` consumed by `pipeline` and `app`.

**Direction / camera planning (`director`):**
- Purpose: Turn highlights into a shot list: camera rigs, segments, speed/zoom keys, ordering, and grenade-specific shots.
- Location: `crates/director/src/lib.rs` (planning, `direct`, `direct_all`, `shoot`, `Timeline`), `crates/director/src/rig.rs` (camera rigs; referenced from README "Камеры"), `crates/director/src/grenade.rs` (`shoot_grenade`, `grenade_victims`).
- Contains: `Timeline`, `Shot`, `Segment`, `Camera`, `CamKey`, `ScaleKey`, `Order`, `Density`, `Config`, `Pose`, `PositionSource`, `look_at`.
- Depends on: `cs2`, `highlights`.
- Used by: `renderer`, `pipeline`, `positions`, `cli`.

**Output rendering (`renderer`):**
- Purpose: Serialize a `Timeline` into the files HLAE and ffmpeg consume, and assemble recorded takes.
- Location: `crates/renderer/src/lib.rs` (`render` at line 149, `Output::write_to`, `record_order`, `ffmpeg_args`, `capture_fps`, `console_safe`), `crates/renderer/src/assemble.rs` (`find_clip`, `assemble`, `ffmpeg_args`, `Options`, `Clip`, `Report`).
- Contains: writers for `plan.json`, `highlights.cfg`, `cs2c_NN.cfg`, `campath_NN.xml`, `assemble.sh`, `assemble.bat`, `assemble.ffgraph`.
- Depends on: `cs2`, `director`.
- Used by: `pipeline`, `app` (via `pipeline`), `cli`.

**Orchestration (`pipeline`):**
- Purpose: Single entry for the full flow used by both UIs.
- Location: `crates/pipeline/src/lib.rs`.
- Contains: `Stage` enum (`Download, Parse, Positions, Geometry, Plan, Record, Assemble, Done`), `Source` (`File` or `Faceit { link, key, demo_index, dir }`), `analyze`, `plan_player`, `load_positions`, `PlanOptions`, `Plan`, `find_player`, `CANCELLED` sentinel string.
- Depends on: `demo`, `cs2`, `highlights`, `director`, `renderer`, `faceit`, `positions`, `geometry`.
- Used by: `app`, `cli`.

**Persistence (`store`):**
- Purpose: SQLite application database for montage jobs, FACEIT response cache, and downloaded-demo registry.
- Location: `crates/store/src/lib.rs`.
- Contains: `JobKind` (`Plan`, `Record`), `Store` (wraps `rusqlite::Connection` in `Arc<Mutex<...>>`), `next_queued`, `start_job`, `finish_job`, `cancel_job`; `Result<T> = std::result::Result<T, String>`.
- Depends on: `rusqlite`, `serde`. No workspace crates.
- Used by: `app` only.

**Presentation (`app`, `cli`):**
- Purpose: User interfaces.
- Location: `crates/app/src/` (egui/eframe desktop UI) and `crates/cli/src/main.rs` (command-line tool, Russian-language usage text).
- Contains: see Entry Points and STRUCTURE.md.
- Depends on: `pipeline`, `renderer`, `director`, `cs2`, `highlights`, `store`, `faceit`; `cli` also depends directly on `demo`, `positions`, `geometry`.
- Used by: end users only.

## Data Flow

### Primary Request Path: build a montage from a FACEIT link or demo file

1. Input is a `pipeline::Source` (`crates/pipeline/src/lib.rs:78`): either a local file, or a FACEIT link that is resolved, downloaded, and cached via `faceit::Client` (`crates/faceit/src/lib.rs`).
2. `pipeline::analyze` (`crates/pipeline/src/lib.rs:84`) reads the demo (`demo::read_demo`), parses it (`demo::parse`), builds `cs2::Match::from_parsed`, and scores highlights with `highlights::find`. It returns an `Analysis`. Empty kill lists are a hard error.
3. `pipeline::plan_player` (`crates/pipeline/src/lib.rs:183`) resolves the hero (`find_player`), optionally loads position tracks and map geometry (`load_positions`, line 239), then produces the shot timeline via the `director` crate.
4. `renderer::render` (`crates/renderer/src/lib.rs:149`) converts the `Timeline` into `Output` (named text files), written by `Output::write_to` (line 123) into the job output directory (default `out/<id>`; see `out/` in the repo root for examples).
5. Recording is performed by HLAE outside this process. `crates/app/src/launch.rs` locates and starts `HLAE.exe` and CS2 helpers; `crates/app/src/jobs.rs:189` (`record`) drives the game and collects per-take files `clips/takeNNNN/video.mp4` and `audio.wav`.
6. Assembly: `renderer::assemble::assemble` (`crates/renderer/src/assemble.rs:199`) locates clips with `find_clip`, builds the ffmpeg argument list with `ffmpeg_args` (line 161), and runs ffmpeg to produce `highlights.mp4`. The `assemble.sh` / `assemble.bat` / `assemble.ffgraph` files written in step 4 provide the same graph for manual re-runs.

### Desktop job lifecycle (`crates/app`)

1. User action calls `App::enqueue(kind)` (`crates/app/src/app.rs:320`), which inserts a row via `store` (state `queued`).
2. Each frame, `App::poll` (`crates/app/src/app.rs:404`) drains `Msg` values sent through the `Reporter` channel (`Msg` enum at `crates/app/src/app.rs:32`).
3. `jobs::run_next` (`crates/app/src/jobs.rs:62`) starts the next queued job only when no job is running and the status bar is idle. It spawns one `std::thread` that calls `jobs::run` (line 89), which executes the full path: demo -> plan -> (record -> assemble).
4. Progress flows out through `JobReporter` (implements `pipeline::Progress`), which writes to `store` and sends `Msg::Stage`/`Msg::Log`. Cancellation uses a `CancelToken` stored in `app.jobs.running`; `App::cancel_job` (line 351) sets it.
5. On completion, `store.finish_job`, `store.cancel_job` (when the error equals `pipeline::CANCELLED`), and `Msg::JobDone` are sent; the UI refreshes via `App::refresh_jobs` (line 283).

### CLI path (`crates/cli/src/main.rs`)

1. `parse_args` builds `Args` (includes `order: director::Order`, `fps`, `size`, `maps`, flags such as `--no-flybys`, `--no-walls`, `--offline`).
2. The CLI calls `pipeline::analyze` and `pipeline::plan_player` (with `pipeline::Quiet` as the progress sink), then calls `renderer::render` and writes the output files directly, and exits with `ExitCode`.
3. `cs2-cli assemble --plan ... --clips ...` invokes `renderer::assemble::assemble` directly, skipping analysis.

**State Management:**
- Pipeline state is passed by value between stages (`Analysis`, `Plan`, `Timeline`); there is no global pipeline state.
- Desktop state lives in `App` (`crates/app/src/app.rs:127`) with sub-structs `MatchesState`, `MatchState`, `JobsState`, and `Settings` (`crates/app/src/settings.rs`).
- Persistent state is SQLite only (`store`); settings are read and written by `crates/app/src/settings.rs`.

## Key Abstractions

**`cs2::Match`:**
- Purpose: Normalized match: players, rounds, kills, grenades, tick rate, map.
- Examples: `crates/cs2/src/lib.rs`
- Pattern: Built once from `demo::Parsed`, then read-only in all downstream crates.

**`highlights::Highlight` / `Utility`:**
- Purpose: Scored candidate moments (kill series, standout kills, grenades).
- Examples: `crates/highlights/src/lib.rs`
- Pattern: Pure scoring functions (`score_kill`, `score_grenade`) followed by filtering in `find`.

**`director::Timeline` / `Shot` / `Segment`:**
- Purpose: Ordered list of shots with camera keys and time scaling; serialized to `plan.json`.
- Examples: `crates/director/src/lib.rs:197` (`Timeline`), `:108` (`Shot`), `:89` (`Segment`)
- Pattern: Data-only structs with derived `Serialize`; `Timeline::total_secs` computes output length from `Shot::output_secs`.

**`director::PositionSource` (trait):**
- Purpose: Abstracts where player poses come from so the director can run with or without tick data.
- Examples: implemented by `positions::Track` (`crates/positions/src/lib.rs`); `director::NoPositions` is the null implementation.
- Pattern: Trait object passed as `&dyn PositionSource` into `direct_all`, `direct`, and `shoot`.

**`pipeline::Progress` (trait):**
- Purpose: Stage/log/cancel callbacks decoupled from the UI.
- Examples: `pipeline::Quiet` (CLI), `app::jobs::JobReporter` (desktop).
- Pattern: `&dyn Progress` threaded through every long operation; `check(p)?` at checkpoints.

**`store::Store`:**
- Purpose: Shared handle to the SQLite database.
- Examples: `crates/store/src/lib.rs`
- Pattern: `Arc<Mutex<Connection>>`, cloned into job threads.

**`geometry::Mesh`:**
- Purpose: Triangle mesh with BVH for wall-aware line-of-sight.
- Examples: `crates/geometry/src/lib.rs:53` (`from_bytes`), `:102` (`raycast`), `:132` (`segment_clear`)
- Pattern: Loaded from prebuilt `maps/*.mesh`; `geometry::fetch` re-downloads when `--refresh-maps` is used.

## Entry Points

**Desktop app:**
- Location: `crates/app/src/main.rs` (`fn main`, line 19)
- Triggers: user launches `cs2cinema` executable (`crates/app/Cargo.toml` binary).
- Responsibilities: builds the eframe window, sets up tray icon (Windows), constructs `app::App::new`.

**CLI:**
- Location: `crates/cli/src/main.rs`
- Triggers: `cs2-cli <FACEIT link | .dem path> [options]` or `cs2-cli assemble ...`.
- Responsibilities: argument parsing, pipeline invocation, plan and file output; CI builds it (`.github/workflows/ci.yml`, `cargo build --release -p cli`).

**Test entry points:**
- `crates/cli/tests/pipeline.rs`, `crates/positions/tests/real_demo.rs`, and in-file `mod tests` blocks in most library crates. Real-demo tests are skipped unless `CS2_TEST_DEMO` is set (see `.github/workflows/ci.yml`).

## Architectural Constraints

- **Threading:** Multi-threaded using `std::thread`. The desktop app runs at most one pipeline job at a time because CS2 records only one game instance (`crates/store/src/lib.rs` module doc; `crates/app/src/jobs.rs:63`). UI-side background work (`App::spawn`, `crates/app/src/app.rs:220`) runs in detached threads that report via `Msg`.
- **Global state:** `store::Store` is a shared `Arc<Mutex<Connection>>`. `crates/app/src/main.rs` installs global tray menu and tray-icon event handlers (`MenuEvent::set_event_handler`, `TrayIconEvent::set_event_handler`), which are process-wide.
- **Circular imports:** None observed. The crate graph is a DAG: `demo` -> `cs2` -> `highlights` -> `director` -> `renderer` -> `pipeline` -> `app`/`cli`, with `positions` and `geometry` feeding `director`/`pipeline`.
- **External tools:** `ffmpeg` and `ffprobe` must be on `PATH` (`crates/app/src/jobs.rs:314`, `crates/renderer/src/assemble.rs:121`, `:207`); HLAE is launched by path (`crates/app/src/launch.rs:166`); Windows process control uses `tasklist`/`taskkill` (`crates/app/src/launch.rs:254`, `:271`).
- **Network:** FACEIT API and CDN (`crates/faceit`), map geometry fetch from GitHub releases (`crates/geometry/src/lib.rs:292`, `:371`), update check (`crates/app/src/update.rs`).
- **Platform:** Windows is the primary target. `#![cfg_attr(windows, windows_subsystem = "windows")]` in `crates/app/src/main.rs`; several modules are Windows-only.
- **Language:** User-facing strings (CLI usage, UI labels, job titles, `Stage::title`) are in Russian; identifiers and code comments mix English and Russian.

## Anti-Patterns

### Duplicated ffmpeg invocation

**What happens:** ffmpeg is spawned in three places: `crates/app/src/jobs.rs:314` (`run_ffmpeg`), `crates/app/src/app.rs:559`, and `crates/renderer/src/assemble.rs:207`/`:313`. `crates/app/src/launch.rs:137` builds yet another ffmpeg command.
**Why it's wrong:** Argument construction and error handling can drift between the desktop app, the CLI, and the assembly path.
**Do this instead:** Route all ffmpeg execution through `renderer::assemble` (or a single helper in `renderer`) and call it from `app` and `cli`.

### Record and assemble sequencing outside `pipeline`

**What happens:** `pipeline` covers analysis and planning (`pipeline::analyze`, `pipeline::plan_player`), but the record and assemble steps are sequenced in `crates/app/src/jobs.rs` (`run`, `record` at line 189). The CLI has its own sequence in `crates/cli/src/main.rs` (around lines 266-424) that calls `renderer::render` and `renderer::assemble::assemble` directly.
**Why it's wrong:** The CLI and the desktop app can diverge in ordering, NVENC selection, and cancellation handling. `Stage::Record` and `Stage::Assemble` exist in `pipeline` but are not driven from there.
**Do this instead:** Move the record and assemble steps behind a `pipeline` function that takes a `Progress` implementation, and keep `jobs.rs` and `cli` limited to store writes and presentation.

## Error Handling

**Strategy:** Library crates return `Result<T, String>` or `Box<dyn Error>` at boundaries (`store::Result`, `pipeline::Result`, `geometry::load_map`, `positions::track`). Errors are converted to user-facing Russian messages with `map_err` at the pipeline boundary (e.g., `crates/pipeline/src/lib.rs:94`).

**Patterns:**
- Cancellation is an error value: `pipeline::CANCELLED` is matched in `jobs::run_next` to choose `store.cancel_job` over `store.finish_job`.
- Optional geometry and positions degrade gracefully: `--no-walls`, `--no-flybys`, and `--offline` skip those inputs (`crates/cli/src/main.rs` usage text) and `director::NoPositions` is used instead of `positions::Track`.
- Job failures are persisted (`store.finish_job(job.id, Err(e))`) and shown in the jobs page (`crates/app/src/jobs_page.rs`).

## Cross-Cutting Concerns

**Logging:** `Progress::log` messages to the UI and to the job log in `store`; the CLI prints to stdout/stderr via `pipeline::Quiet` (no-op) or its own prints. No logging crate is declared in the crates reviewed.
**Validation:** Input checks at the boundary: `faceit::parse_match_id` (`crates/pipeline/src/lib.rs:94`), `renderer::console_safe` for names written into HLAE console scripts (`crates/renderer/src/lib.rs:145`), `pipeline::console_name` (line 291).
**Authentication:** FACEIT API key from the `FACEIT_API_KEY` environment variable or the settings UI (`faceit::Client::from_env`; the app passes `s.faceit_key` from `crates/app/src/settings.rs` into `pipeline::Source::Faceit`).

---

*Architecture analysis: 2026-10-10*
