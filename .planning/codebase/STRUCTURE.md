---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
# Codebase Structure

**Analysis Date:** 2026-10-10

## Directory Layout

```
cs2cinema/                       # repo root (cargo workspace)
├── Cargo.toml                   # workspace members list (12 crates)
├── Cargo.lock
├── README.md                    # user docs; "Устройство (по крейтам)" section maps crates
├── install.ps1                  # Windows install script
├── crates/                      # all Rust code
│   ├── demo/                    # .dem container + protobuf decoding
│   ├── cs2/                     # match model (rounds, kills, players)
│   ├── highlights/              # kill/grenade scoring and selection
│   ├── director/                # camera planning and shot timeline
│   ├── positions/               # per-tick player poses via source2-demo
│   ├── geometry/                # map collision meshes, BVH raycast
│   ├── faceit/                  # FACEIT API client, demo download, stats
│   ├── renderer/                # HLAE scripts, ffmpeg graph, clip assembly
│   ├── pipeline/                # orchestration: analyze -> plan -> render
│   ├── store/                   # SQLite: jobs, FACEIT cache, demo registry
│   ├── app/                     # desktop app (eframe/egui)
│   └── cli/                     # command-line tool (cs2-cli)
├── maps/                        # prebuilt map meshes (*.mesh) + maps.json
├── scripts/
│   └── update_maps.py           # refreshes map geometry data
├── out/                         # generated montage output (gitignored)
├── target/                      # cargo build output (gitignored)
└── .github/workflows/
    ├── ci.yml                   # tests on Linux and Windows; builds cs2-cli.exe
    └── release.yml              # tag v* -> Windows zip + GitHub Release
```

## Directory Purposes

**`crates/demo/`:**
- Purpose: Read CS2 demo files and decode them.
- Contains: Rust modules and protobuf sources.
- Key files: `crates/demo/src/lib.rs` (public API: `read_demo`, `parse`, `Parsed`, `PlayerInfo`), `crates/demo/src/parse.rs`, `crates/demo/src/command.rs`, `crates/demo/src/events.rs`, `crates/demo/src/bits.rs`, `crates/demo/build.rs`, `crates/demo/proto/demo.proto`, `crates/demo/proto/events.proto`.

**`crates/cs2/`:**
- Purpose: Domain model of a match.
- Contains: Single library module plus report module.
- Key files: `crates/cs2/src/lib.rs` (`Match`, `Kill`, `Grenade`, `Round`, `Player`, `PlayerId`), `crates/cs2/src/report.rs`.

**`crates/highlights/`:**
- Purpose: Choose and score highlight moments.
- Key files: `crates/highlights/src/lib.rs` (`find`, `score_kill`, `score_grenade`, `for_player`, `Config`).

**`crates/director/`:**
- Purpose: Camera direction and shot timeline.
- Key files: `crates/director/src/lib.rs` (`Timeline`, `Shot`, `direct`, `direct_all`, `shoot`, `Order`, `Config`), `crates/director/src/rig.rs` (camera rigs), `crates/director/src/grenade.rs` (grenade shots).

**`crates/positions/`:**
- Purpose: Player position, view angle, team, and alive state per tick.
- Key files: `crates/positions/src/lib.rs` (`track`, `Track`, `eye_angles`, `grenade_flight`), `crates/positions/tests/real_demo.rs` (uses a real demo; requires `CS2_TEST_DEMO`).

**`crates/geometry/`:**
- Purpose: Map collision geometry and line-of-sight.
- Key files: `crates/geometry/src/lib.rs` (`Mesh::from_bytes`, `Mesh::raycast`, `segment_clear`, `load_map`, `fetch`, `default_dir`, `Stamp`, `latest_release_tag`).

**`crates/faceit/`:**
- Purpose: FACEIT Data API access.
- Key files: `crates/faceit/src/lib.rs` (`Client`, `parse_match_id`, `demo_urls`, `download`), `crates/faceit/src/stats.rs` (`MatchStats`).

**`crates/renderer/`:**
- Purpose: Output writers for HLAE and ffmpeg.
- Key files: `crates/renderer/src/lib.rs` (`render`, `Output`, `Config`, `record_order`, `ffmpeg_args`, `console_safe`). Склейка живёт в `pipeline::assemble`, файла `renderer/src/assemble.rs` больше нет.

**`crates/host/`:**
- Purpose: Граница с ОС — единственный крейт, который запускает процессы, открывает сокеты и вызывает Win32 (`Host`, `Session`, `win::WinHost` с Job Object, `win::notify`, `fake::FakeHost`, `unsupported`).
- Key files: `crates/host/src/lib.rs`, `crates/host/src/win/`, `crates/host/src/fake.rs`.

**`crates/pipeline/`:**
- Purpose: Single entry point shared by the app and CLI.
- Key files: `crates/pipeline/src/lib.rs` (`Stage`, `Progress`, `Quiet`, `Source`, `analyze`, `plan_player`, `find_player`, `PlanOptions`, `Plan`, `CANCELLED`, `console_name`, `absolute_slash`), `crates/pipeline/src/job.rs` (`run_job`, `JobSpec`, `Outcome`), `crates/pipeline/src/gamedir.rs`, `crates/pipeline/src/assemble.rs`, `crates/pipeline/src/manifest.rs`, `crates/pipeline/src/acceptance.rs`, `crates/pipeline/src/record/` (автомат записи, дедлайны, коды сбоев), `crates/pipeline/src/precheck/` (предпроверка окружения, `nvenc_check`).

**`crates/store/`:**
- Purpose: SQLite persistence.
- Key files: `crates/store/src/lib.rs` (`Store`, `Job`, `JobKind`, `next_queued`, `start_job`, `finish_job`, `cancel_job`, `demo_path`, `remember_demo`, `requeue_auto`, `open_read_only`), `crates/store/src/journal.rs` (разбор журнала: попытки и исходы).

**`crates/app/`:**
- Purpose: Desktop application (eframe/egui, Windows tray).
- Contains: 13 Rust files under `src/`, all flat (no nested modules).
- Key files:
  - `crates/app/src/main.rs`: `fn main`, window options, tray setup, module declarations.
  - `crates/app/src/app.rs`: `App`, `Page`, `Msg`, `Reporter`, `enqueue`, `cancel_job`, `poll`, sidebar.
  - `crates/app/src/jobs.rs`: `runner` (исполнитель задачи поверх `pipeline::job::run_job`), `job_spec`, `job_config`, `recover_after_crash`, `JobReporter`, `acceptance_jobs`.
  - `crates/app/src/queue.rs`: поток очереди `cs2cinema-queue` — `Queue`, `QueuePolicy` (авто-повтор, пауза), `PauseReason`, `Summary`; итог очереди уходит в трей (`host::win::notify`).
  - `crates/app/src/jobs_page.rs`: job list UI.
  - `crates/app/src/match_page.rs`: match view UI.
  - `crates/app/src/player.rs`: player view and summary.
  - `crates/app/src/settings.rs` and `crates/app/src/settings_page.rs`: settings model and UI.
  - `crates/app/src/status.rs`: busy status and progress bar state.
  - `crates/app/src/ui.rs`: shared widgets and `icon_rgba`, `open` helpers.
  - `crates/app/src/update.rs`: release check and self-update.
  - `crates/app/src/mock.rs`: test data, compiled only in debug builds (`#[cfg(debug_assertions)]`).

**`crates/cli/`:**
- Purpose: Command-line tool `cs2-cli`.
- Key files: `crates/cli/src/main.rs` (`parse_args`, `Args`, `USAGE`, подкоманда `assemble <папка задачи>`), `crates/cli/src/record.rs` (`record`, `retry`, `ConsoleProgress`), `crates/cli/src/acceptance.rs` (`acceptance check|state|job|set`), `crates/cli/tests/pipeline.rs`, `crates/cli/tests/assemble.rs`.

## Key File Locations

**Entry Points:**
- `crates/app/src/main.rs`: desktop binary.
- `crates/cli/src/main.rs`: CLI binary.

**Configuration:**
- `Cargo.toml`: workspace members and resolver.
- `crates/*/Cargo.toml`: per-crate dependencies. `crates/positions/Cargo.toml` pins `source2-demo = "=0.5.9"`.
- `crates/app/src/settings.rs`: persisted user settings (paths for HLAE, output root, FACEIT key, GPU encode flag).
- `.gitignore`: ignores `/target`, `/out`, `/maps`, `*.dem`, `*.dem.zst`.
- `.gitattributes`: line-ending and LFS attributes.

**Core Logic:**
- `crates/pipeline/src/lib.rs`: stage sequence.
- `crates/director/src/lib.rs`: shot planning.
- `crates/renderer/src/lib.rs`: output file generation.
- `crates/pipeline/src/job.rs`: `run_job` — задача целиком (общая для окна и консоли).
- `crates/app/src/queue.rs`: очередь задач в своём потоке.
- `crates/host/src/`: граница с ОС (`Host`, `win::WinHost`, `win::notify`, `fake::FakeHost`).

**Testing:**
- Inline `mod tests` in `crates/cs2/src/lib.rs`, `crates/director/src/lib.rs`, `crates/faceit/src/lib.rs`, `crates/geometry/src/lib.rs`, `crates/highlights/src/lib.rs`, `crates/pipeline/src/lib.rs`, `crates/positions/src/lib.rs`, `crates/renderer/src/lib.rs`, `crates/store/src/lib.rs`.
- Integration tests: `crates/cli/tests/pipeline.rs`, `crates/positions/tests/real_demo.rs`.

**Generated and runtime data:**
- `out/<match-id or label>/`: one folder per montage job containing `plan.json`, `highlights.cfg`, `cs2c_NN.cfg`, `campath_NN.xml`, `assemble.sh`, `assemble.bat`, `assemble.ffgraph`, `clips/takeNNNN/{video.mp4,audio.wav}`, and `highlights.mp4`. Example: `out/1-3e7db9e3-.../`. A reference check run lives in `out/check/`.
- `maps/`: `*.mesh` geometry for each map plus `maps/maps.json`. Gitignored, populated by `geometry::fetch` or `scripts/update_maps.py`.

## Naming Conventions

**Files:**
- Rust modules: `snake_case.rs`, one module per concept (`jobs_page.rs`, `settings_page.rs`, `match_page.rs`).
- Crate directories: short lowercase nouns (`demo`, `cs2`, `director`), no prefixes. Crate names in `Cargo.toml` match directory names, except the CLI package which is `cli` and produces `cs2-cli`.
- Generated per-take folders: `clips/take0000/` (4-digit index). Per-player HLAE scripts: `cs2c_NN.cfg`, `campath_NN.xml` (2-digit index).

**Directories:**
- Crate folders under `crates/` use `src/` and optionally `tests/`, `proto/`.
- Output folders under `out/` are named `<label>_<player>` after `pipeline::console_name` sanitization.

**Code:**
- Types: `PascalCase` (`Timeline`, `PositionSource`, `JobKind`).
- Functions: `snake_case` (`analyze`, `plan_player`, `record_order`).
- Constants: `SCREAMING_SNAKE_CASE` (`CANCELLED`, `USAGE`).
- User-facing text (CLI usage, UI labels, stage titles) is Russian; keep new user strings consistent with that.

## Where to Add New Code

**New pipeline stage or orchestration step:**
- Implementation: `crates/pipeline/src/lib.rs` (add a `Stage` variant and update `Stage::ALL` and `Stage::title`).
- Progress reporting: call `check(p)?` and `p.stage(...)` at the start of the stage.

**New highlight rule or scoring:**
- Implementation: `crates/highlights/src/lib.rs`; add tests in its `mod tests`.

**New camera behavior or shot type:**
- Implementation: `crates/director/src/lib.rs` or `crates/director/src/rig.rs` (rigs) and `crates/director/src/grenade.rs` (grenade-related shots).
- Tests: inline `mod tests` in `crates/director/src/lib.rs`.

**New output file or HLAE/ffmpeg format:**
- Implementation: `crates/renderer/src/lib.rs` (`Output`, `render`) for HLAE files; `crates/pipeline/src/assemble.rs` for ffmpeg assembly.

**New demo event or entity field:**
- Protobuf source: `crates/demo/proto/*.proto` (compiled by `crates/demo/build.rs`).
- Parsing: `crates/demo/src/events.rs` or `crates/demo/src/command.rs`.
- Model: `crates/cs2/src/lib.rs`.

**New desktop page:**
- Implementation: new `crates/app/src/<name>_page.rs`; register in `crates/app/src/main.rs` module list; add a `Page` variant in `crates/app/src/app.rs` and a nav item in `App::sidebar`.

**New background job type:**
- Store: add a `JobKind` variant and SQL handling in `crates/store/src/lib.rs`.
- Execution: `crates/app/src/jobs.rs` (`runner`, `job_spec`) поверх `pipeline::job::run_job`; порядок и политика — `crates/app/src/queue.rs`.
- UI: `crates/app/src/jobs_page.rs`.

**New CLI option:**
- Implementation: `crates/cli/src/main.rs` (`Args`, `parse_args`, `USAGE` text).

**New FACEIT endpoint or stats:**
- Implementation: `crates/faceit/src/lib.rs` (client) and `crates/faceit/src/stats.rs` (stats types).

**New map or geometry data:**
- Mesh files: `maps/` (gitignored), refreshed by `scripts/update_maps.py`; loader in `crates/geometry/src/lib.rs`.

**Shared utilities:**
- Console-safe names and output paths: `crates/pipeline/src/lib.rs` (`console_name`, `absolute_slash`) and `crates/renderer/src/lib.rs` (`console_safe`).
- UI helpers: `crates/app/src/ui.rs`.

## Special Directories

**`target/`:**
- Purpose: cargo build artifacts.
- Generated: Yes. Committed: No (`.gitignore`).

**`out/`:**
- Purpose: montage job output and reference check runs.
- Generated: Yes. Committed: No (`.gitignore`).

**`maps/`:**
- Purpose: map collision meshes and index (`maps.json`).
- Generated: Yes (fetched or built). Committed: No (`.gitignore`).

**`crates/demo/proto/`:**
- Purpose: protobuf definitions compiled at build time.
- Generated: Source is hand-written; Rust bindings are generated by `crates/demo/build.rs`. Committed: Yes.

**`.planning/`:**
- Purpose: GSD planning artifacts, including this codebase map in `.planning/codebase/`.
- Generated: Yes. Committed: Per project policy.

**Demo files (`*.dem`, `*.dem.zst`):**
- Purpose: Source match demos.
- Generated: No (downloaded or provided). Committed: No (`.gitignore`).

---

*Structure analysis: 2026-10-10*
