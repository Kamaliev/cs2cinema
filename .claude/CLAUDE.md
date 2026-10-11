<!-- GSD:project-start source:PROJECT.md -->

## Project

**cs2cinema**

cs2cinema — десктопное приложение для Windows (Rust, egui) и CLI `cs2-cli`, которые из демки CS2 (локальный `.dem` или матч FACEIT по ссылке) автоматически делают видеомонтаж: парсят демку, находят моменты игрока, планируют камеры, записывают дубли через HLAE в реальном CS2 и склеивают ролик через ffmpeg. Пользователь один — автор, на своём ПК, со своим CS2 и HLAE.

Этот этап («Надёжность записи») про то, чтобы задача из очереди либо доходила от ссылки FACEIT до `highlights.mp4` без ручного вмешательства, либо быстро падала с понятной причиной.

**Core Value:** Задача на монтаж из очереди доходит до готового `highlights.mp4` без ручного вмешательства, а если это невозможно — останавливается быстро и объясняет, что именно пошло не так.

### Constraints

- **Tech stack**: Rust cargo workspace, egui/eframe, `std::thread` без tokio, SQLite через rusqlite — не менять стек, встраиваться в существующие крейты
- **Platform**: только Windows — HLAE и запись CS2 нигде больше не работают
- **External tools**: HLAE и CS2 — внешние процессы с недокументированными маркерами консоли и структурой папок; любой детектор должен иметь таймаут и диагностику
- **Concurrency**: одна задача записи за раз — CS2 пишет только один экземпляр
- **Compatibility**: CLI `cs2-cli` должен продолжать собираться и проходить `crates/cli/tests/pipeline.rs`
- **Language**: пользовательские строки на русском

<!-- GSD:project-end -->

<!-- GSD:stack-start source:codebase/STACK.md -->

## Technology Stack

## Languages

- Rust (stable toolchain, 2021-style workspace, resolver 2) - all application and library code under `crates/*/src/`. Declared workspace members in `Cargo.toml`: `crates/demo`, `crates/highlights`, `crates/director`, `crates/cs2`, `crates/renderer`, `crates/faceit`, `crates/positions`, `crates/geometry`, `crates/host`, `crates/pipeline`, `crates/store`, `crates/app`, `crates/cli`.
- PowerShell - `install.ps1` (end-user installer, downloads release zip, verifies SHA-256, updates user PATH, creates Start Menu shortcut).
- POSIX shell and batch - generated assembly scripts written by `crates/renderer/src/lib.rs` (`assemble.sh`, `assemble.bat`) per output folder; not hand-maintained source.
- Python 3.11+ - optional `scripts/update_maps.py` (extracts map geometry from a local CS2 install; not needed for normal runs).
- Protocol Buffers - `crates/demo/proto/demo.proto` and `crates/demo/proto/events.proto` (CS2 demo message definitions, compiled at build time).
- Source2 Engine Constants/Text - `highlights.cfg`, `cs2c_NN.cfg`, `cs2c_NN_K.xml` camera paths generated for the CS2 console and HLAE (see `crates/renderer/src/lib.rs`). These are outputs, not source.
- Markdown - `README.md` (Russian-language user documentation).

## Runtime

- Native binaries, no managed runtime. Primary target is Windows 11 x64 (`cfg(windows)` code paths in `crates/host/src/win/`, `crates/app/src/ui.rs`, `crates/app/src/app.rs`). Linux is built and tested in CI (`.github/workflows/ci.yml`) but the desktop app is Windows-oriented.
- Release binaries: `cs2-cli.exe` (crate `cli`) and `cs2cinema.exe` (crate `app`, built with `windows_subsystem = "windows"` in `crates/app/src/main.rs`).
- Cargo (workspace at `Cargo.toml`).
- Lockfile: present - `Cargo.lock` at repo root (committed).
- No `rust-toolchain` / `rust-toolchain.toml` file pinned; CI uses `dtolnay/rust-toolchain@stable`.

## Frameworks

- `eframe` 0.31.1 (egui framework) with `glow` renderer, `default_fonts`, `wayland`, `x11` features - `crates/app/Cargo.toml`. Entry point `crates/app/src/main.rs`, app state in `crates/app/src/app.rs`.
- `tray-icon` 0.19.3 (Windows only, `default-features = false`) - system tray, `crates/app/Cargo.toml` under `[target.'cfg(windows)'.dependencies]`.
- `rfd` 0.15.4 - native file/folder pickers.
- Hand-written argument parsing in `crates/cli/src/main.rs` (no CLI framework crate listed in manifests).
- `demo` - CS2 `.dem` container and protobuf message decoding (`crates/demo/src/parse.rs`, `bits.rs`, `command.rs`, `events.rs`).
- `cs2` - match model: rounds, kills, players, reports (`crates/cs2/src/lib.rs`, `report.rs`).
- `positions` - player/entity position sampling built on `source2-demo` (`crates/positions/src/lib.rs`).
- `geometry` - map mesh loading (AWMH format) and wall geometry (`crates/geometry/src/lib.rs`).
- `director` - camera director: grenade, rig, shot planning (`crates/director/src/grenade.rs`, `rig.rs`, `lib.rs`).
- `highlights` - scoring and selection of highlight moments (`crates/highlights/src/lib.rs`).
- `renderer` - writes HLAE/CS2 scripts and ffmpeg filter graphs (`crates/renderer/src/lib.rs`); сама склейка — `pipeline::assemble`.
- `host` - граница с ОС: процессы, Job Object, сокет консоли игры, уведомление в трее (`crates/host/src/`).
- `pipeline` - orchestrates demo to plan to render (`crates/pipeline/src/lib.rs`).
- `faceit` - FACEIT Data API client (`crates/faceit/src/lib.rs`, `stats.rs`).
- `store` - SQLite persistence (`crates/store/src/lib.rs`).
- Built-in `cargo test` harness. Integration test on a real demo: `crates/positions/tests/real_demo.rs` (skipped unless `CS2_TEST_DEMO` is set).
- Dev-dependency `prost` 0.14.4 in `crates/cli/Cargo.toml` and `demo`/`cs2`/`highlights` as dev-deps in `crates/positions/Cargo.toml`.
- `prost-build` 0.14.4 and `protoc-bin-vendored` 3.3.0 (build-dependencies of `crates/demo`, script `crates/demo/build.rs`). `protoc` is vendored, so no system protoc install is needed unless `PROTOC` is set (build.rs checks `PROTOC`).
- `Swatinem/rust-cache@v2` and `actions/upload-artifact@v4` in CI.

## Key Dependencies

- `eframe` 0.31.1 / `glow` 0.16.0 - GUI and OpenGL rendering for `cs2cinema.exe`.
- `ureq` 3.4.2 (features `json`, `platform-verifier`) - blocking HTTP client used by `crates/faceit/src/lib.rs`, `crates/geometry/src/lib.rs` (where available), and `crates/app/src/update.rs`. `platform-verifier` makes TLS use the OS trust store.
- `rusqlite` 0.32.1 (feature `bundled`) - bundles SQLite, no system library needed. Used in `crates/store/src/lib.rs`.
- `prost` 0.14.4 - protobuf runtime for CS2 demo messages in `crates/demo`.
- `source2-demo` 0.5.9 (pinned with `=`, `default-features = false`, feature `cs2`) - Source 2 entity/string-table parsing for `crates/positions`.
- `zstd` 0.14.0 - decompression of `.dem.zst` demo archives (FACEIT demos are distributed compressed).
- `snap` 1.1.2 and `flate2` 1.1.10 - demo packet decompression in `crates/demo`.
- `zip` 2.4.2 (feature `deflate` only) - extraction of archives (`crates/app`, `crates/geometry`).
- `serde` 1.0.229 (derive) and `serde_json` 1.x - JSON for `plan.json`, settings, FACEIT responses, and the SQLite payloads.
- `chrono` 0.4.45 (feature `clock`, no default features) - timestamps in `crates/app`.
- `sha2` 0.10 - SHA-256 in `crates/app` (used for update/verification).
- `image` 0.25.10 - pulled in transitively (via eframe/egui and rfd stack); not a direct dependency of a workspace crate.
- `tray-icon` 0.19.3 - Windows tray integration.
- `ffmpeg` 7+ and `ffprobe` - clip assembly and duration probe. Started only through `host::Host::run` (deadline + cancel) from `pipeline::assemble` and `pipeline::record::verify`. NVENC availability check: `pipeline::precheck::nvenc_check`.
- `HLAE.exe` (advancedfx) with `x64/AfxHookSource2.dll` - records CS2 via `startmovie`. Launched by `host::win::WinHost::launch` inside a Job Object, path discovered by `find_hlae` in `crates/app/src/settings.rs` (or `--hlae`/`HLAE_DIR` in the console).
- Windows API (`windows-sys` 0.61 in `crates/host`, только `cfg(windows)`): Job Object, process handles, `Shell_NotifyIconW` for the tray balloon. The earlier `tasklist`/`taskkill` utilities were removed in phase 4 ("было до фазы 4"); the workspace has no calls to them.
- `cmd` / `xdg-open` - opening URLs in `crates/app/src/ui.rs`.
- `git` - queries the latest `awpy-data` release tag in `crates/geometry/src/lib.rs` (line ~293).

## Configuration

- `FACEIT_API_KEY` - FACEIT Data API key (read in `crates/faceit/src/lib.rs`, `crates/cli/src/main.rs`).
- `CS2CINEMA_MAPS` - folder of `*.mesh` map geometry; default `./maps` (`crates/geometry/src/lib.rs`).
- `CS2CINEMA_FONT` - caption font path; default Arial Bold from Windows (`crates/pipeline/src/lib.rs`, `crates/cli/src/main.rs`).
- `CS2_DIR` - CS2 install directory override (`crates/cli/src/main.rs`, `scripts/update_maps.py`).
- `CS2_TEST_DEMO`, `CS2_TEST_MAPS` - enable the real-demo tests (`crates/positions/tests/real_demo.rs`, `crates/pipeline/src/lib.rs`).
- `CS2CINEMA_VERSION` - baked into the app at compile time via `option_env!` (`crates/app/src/update.rs`); set by `.github/workflows/release.yml`.
- `CS2CINEMA_MOCK`, `CS2CINEMA_PAGE` - debug-only UI overrides in `crates/app/src/app.rs` and `crates/app/src/mock.rs`.
- `PROTOC` - optional override of the vendored protoc in `crates/demo/build.rs`.
- `LOCALAPPDATA`, `XDG_DATA_HOME`, `HOME`, `USERPROFILE`, `ProgramFiles(x86)` - used to locate the data dir and search paths.
- `%LOCALAPPDATA%\cs2cinema\settings.json` - user settings (resolved in `crates/app/src/settings.rs`, `data_dir()` and `settings_path()`).
- `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` - SQLite database (opened in `crates/app/src/app.rs`, schema in `crates/store/src/lib.rs`).
- Note: the FACEIT key is stored in `settings.json` as entered in the UI; the file path is shown in the Settings page (`crates/app/src/settings_page.rs`).
- `Cargo.toml` (workspace), per-crate `Cargo.toml` files under `crates/*/`.
- `.gitignore` excludes `/target`, `*.dem`, `*.dem.zst`, `/out`, `/maps`, `__pycache__/`, `.idea/`.
- `.gitattributes` - line-ending/LFS rules (demo files are stored in Git LFS per `.github/workflows/ci.yml` comment).

## Platform Requirements

- Rust stable with the MSVC toolchain on Windows (Visual Studio Build Tools, "Desktop development with C++", per `README.md`).
- Git and Git LFS (demos live in LFS).
- Optional: Python 3.11+ for `scripts/update_maps.py`.
- Recommended: `ffmpeg` 7+ on `PATH` (`winget install Gyan.FFmpeg`).
- Windows 11 x64 (release artifacts are `cs2cinema-windows-x64.zip` and `cs2-cli.exe`).
- CS2 installed through Steam (app looks in `steamapps/common/Counter-Strike Global Offensive`, see `find_cs2` in `crates/app/src/settings.rs`).
- HLAE 2.192+ with `x64/AfxHookSource2.dll` (`README.md` lists 2.192.7 as tested).
- NVIDIA GPU with a recent driver for NVENC encoding (optional; software encoding is used otherwise per `pipeline::precheck::nvenc_check`).
- A FACEIT Data API key with Downloads API access for FACEIT features.
- GitHub Releases on `Kamaliev/cs2cinema`: `release.yml` builds `cs2-cli` and `cs2cinema` on `windows-latest` on tags `v*` and packages `cs2cinema-windows-x64.zip` plus `.sha256`.
- Installer: `install.ps1` (downloads from `https://github.com/Kamaliev/cs2cinema/releases/latest/download/`, installs to `%LOCALAPPDATA%\cs2cinema`, adds to user PATH).

<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->

## Conventions

## Naming Patterns

- Rust source files use `snake_case.rs`: `crates/pipeline/src/lib.rs`, `crates/director/src/grenade.rs`, `crates/app/src/match_page.rs`, `crates/app/src/settings_page.rs`.
- Each library crate exposes its API from `src/lib.rs`; the binaries are `crates/cli/src/main.rs` and `crates/app/src/main.rs`.
- Page modules in the GUI crate are suffixed `_page.rs` (`jobs_page.rs`, `match_page.rs`, `settings_page.rs`).
- Integration tests live in `crates/<crate>/tests/*.rs` (`crates/cli/tests/pipeline.rs`, `crates/positions/tests/real_demo.rs`).
- Crate directories and package names are short lowercase nouns with no prefix: `demo`, `cs2`, `highlights`, `director`, `renderer`, `positions`, `geometry`, `pipeline`, `store`, `faceit`, `app`, `cli`. Declared as workspace members in `Cargo.toml`.
- `snake_case`, verb-first for actions: `analyze`, `plan_player`, `load_positions`, `score_kill`, `for_player`, `utility_for`, `shoot_grenade`.
- Constructors: `new`, `build`, `open`, `in_memory`, `from_bytes`, `from_triangles`, `load`.
- Predicates and checks: `check` (returns `Result<()>`, see `crates/pipeline/src/lib.rs`), `is_*`/`has_*` where a boolean is returned.
- Unit tests are named as sentences describing behavior, not `test_*`: `chain_splits_by_gap_and_ace_dominates` (`crates/highlights/src/lib.rs`), `fancy_single_kill_is_showpiece`, `plain_single_kill_is_ignored`, `rejects_garbage` (`crates/faceit/src/lib.rs`), `takes_the_latest_take_and_counts_duration` (`crates/renderer/src/assemble.rs`), `job_lifecycle` (`crates/store/src/lib.rs`).
- `PascalCase` structs and enums: `Highlight`, `Kind` (`crates/highlights/src/lib.rs`), `PositionSource`, `NoPositions`, `Pose` (`crates/director/src/lib.rs`), `Progress`, `Stage` (`crates/pipeline/src/lib.rs`), `JobStatus`, `JobKind`, `JobSource`, `NewJob`, `JobOutcome` (`crates/store/src/lib.rs`).
- Newtype IDs wrap primitives: `PlayerId(i32)` in `crates/cs2`.
- Enum variants are `PascalCase` and used in `match` exhaustively: `Kind::Showpiece`, `Score::Epic`/`Great`/`Good` as described in `crates/director/src/lib.rs` module docs.
- Score/threshold constants are `SCREAMING_SNAKE_CASE`: `EPIC_SCORE`, `GREAT_SCORE` (`crates/director/src/lib.rs`), `CANCELLED` (`crates/pipeline/src/lib.rs`).
- `snake_case`, short names are common in geometry and math code (`a`, `b`, `c`, `d` for triangle vertices in `crates/geometry/src/lib.rs`; `tr` / `TR` for tick rate in `crates/director/src/rig.rs`).
- Identifiers and code are English. User-facing strings, CLI usage text, doc comments and most inline comments are written in Russian. Examples: `const USAGE` in `crates/cli/src/main.rs`, `//!` module docs in `crates/pipeline/src/lib.rs` and `crates/director/src/lib.rs`, the `ошибка: {e}` error prefix in `crates/cli/src/main.rs`, test assertion messages such as `"demoparser2: 173 убийств после begin_new_match"` in `crates/positions/tests/real_demo.rs`.
- Follow this: new doc comments and user-facing messages should be in Russian to match the surrounding code; identifiers in English.

## Code Style

- No `rustfmt.toml` / `.rustfmt.toml` is committed. Code is formatted with rustfmt defaults plus a wide line budget: many lines in `crates/app/src/mock.rs` and `crates/store/src/lib.rs` run well past 100 columns, and struct literals are kept on one line.
- Multi-item `use` blocks are grouped inside braces and may span lines (`crates/cli/tests/pipeline.rs`).
- All workspace crates use edition 2024; keep the edition already set in each crate's `Cargo.toml`.
- Run `cargo fmt` before committing; there is no CI check enforcing it.
- No `[lints]` table in any `Cargo.toml`, no `clippy.toml`, no crate-level `#![deny(...)]`/`#![warn(...)]`.
- The only explicit lint suppressions are `#[allow(clippy::too_many_arguments)]` on `crates/director/src/rig.rs:91` and `crates/director/src/rig.rs:434`. Prefer a struct parameter over adding more `allow`s.
- CI (`.github/workflows/ci.yml`) runs `cargo test` and `cargo build` only; clippy is not run. Run `cargo clippy --workspace` locally before large changes.

## Import Organization

## Error Handling

- `pub type Result<T> = std::result::Result<T, String>;` in `crates/pipeline/src/lib.rs:10` and `crates/store/src/lib.rs:16`.
- Errors are converted with `.map_err(|e| e.to_string())` or `format!`. Callers display the string directly.
- Cancellation is a sentinel string: `pub const CANCELLED: &str = "отменено";` in `crates/pipeline/src/lib.rs`, checked with `check(p)?` between stages. The GUI compares against this constant to distinguish cancel from failure.
- `crates/geometry/src/lib.rs` (`from_bytes`, `load`, `load_map`, `fetch`, `latest_release_tag`), `crates/positions/src/lib.rs` (`track`), `crates/cli/src/main.rs` (`run`, `list_cameras`, `assemble_cmd`) return `Result<T, Box<dyn Error>>`.
- `crates/faceit/src/lib.rs:14` defines a private alias `type Result<T> = std::result::Result<T, Box<dyn Error>>;`. Follow this pattern when a crate needs a local alias.
- `crates/cli/src/main.rs` prints errors with `eprintln!("ошибка: {e}")`, prints usage on argument errors, and returns `ExitCode`. No `anyhow`/`thiserror` dependency exists in the workspace.
- `?` is the default. `unwrap()` / `expect()` appear mostly in tests and in test-only fixtures. Non-test uses should be limited to invariants that cannot fail.
- Missing optional data uses `Option` and `let ... else` (`let Some(data) = load() else { return };` in `crates/positions/tests/real_demo.rs`).
- Graceful degradation is preferred over hard failure for optional inputs: the director falls back to POV/chase cameras when there is no position source (`crates/director/src/lib.rs` module docs), and `--no-walls` / `--offline` flags skip geometry downloads (`crates/cli/src/main.rs` USAGE).
- Demo files are checked by magic bytes before parsing: `data.starts_with(b"PBDEMS2\0")` (`crates/positions/tests/real_demo.rs`). LFS pointer files produce an explicit error (see `lfs_pointer_gives_clear_error` in `crates/cli/tests/pipeline.rs`).

## Logging

- CLI output: `println!` for results and `eprintln!` for errors (38 occurrences of `println!`/`eprintln!` across `crates/`).
- Pipeline progress goes through the `Progress` trait, not stdout: `fn stage(&self, s: Stage)` and `fn log(&self, msg: &str)` in `crates/pipeline/src/lib.rs`. Implementations include a silent `Quiet` for tests and console, and a GUI sink that writes to the job log in `crates/store/src/lib.rs` (`append_log`).
- New long-running steps should report via `Progress::stage` / `log` and call `check(p)?` at stage boundaries so cancellation works.

## Comments

- Module-level `//!` docs describe the purpose and the design rule of the module (`crates/director/src/lib.rs`, `crates/pipeline/src/lib.rs`, `crates/highlights/src/lib.rs`). Every library `lib.rs` has one.
- Item-level `///` docs on public types and traits, especially trait methods with non-obvious contracts (`PositionSource::raycast`, `Progress::cancelled` in `crates/pipeline/src/lib.rs`).
- Inline `//` comments explain why, including units and conventions: `/// pitch, yaw, roll в градусах (как в Source).` on `Pose.ang` in `crates/director/src/lib.rs`.
- Known unverified data is marked in comments as `TODO(verify)` (see README note about awpy-data paths).

## Function Design

- Pass slices and borrowed data: `&[u8]`, `&Match`, `&Path`, `&dyn Progress`.
- Many-parameter functions are tolerated with explicit `#[allow(clippy::too_many_arguments)]` (`crates/director/src/rig.rs`). Prefer an options struct: `PlanOptions` in `crates/pipeline/src/lib.rs`, `Config` in `crates/highlights/src/lib.rs`, `Options` in `crates/geometry/src/lib.rs`.
- Dependency injection uses trait objects: `&dyn Progress`, `&dyn PositionSource`, `&dyn Fn`-style closures.
- `Result<T, String>` or `Result<T, Box<dyn Error>>` per the error style above. Optional lookups return `Option<T>` (`eye_pose`, `grenade_path`, `Store::job`).
- Constructors for test data use `Match::build(...)` rather than struct literals where the struct has derived defaults.

## Module Design

- Public API goes through the crate root. Internal helpers are `fn` without `pub` or `pub(crate)` (only 3 `pub(crate)`/`pub(super)` uses in the workspace, so prefer private).
- Derives on data types are standard: `#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]` on `Pose` (`crates/director/src/lib.rs`), `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]` on `Kind` (`crates/highlights/src/lib.rs`). Use `serde` derives for anything persisted to JSON or SQLite.

## Naming for Tests and Fixtures

- Test helpers are small private `fn`s at the top of the test module: `fn kill(tick, round, attacker, victim) -> Kill`, `fn wall() -> Mesh`, `fn new_job(kind) -> NewJob`, `fn varint(...)` in `crates/cli/tests/pipeline.rs`.
- Constants inside tests are short: `const TR: f32 = 64.0;` (`crates/director/src/rig.rs`).

<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->

## Architecture

## System Overview

```text

```

## Pattern Overview

- Workspace members are declared in `Cargo.toml` (`crates/demo`, `highlights`, `director`, `cs2`, `renderer`, `faceit`, `positions`, `geometry`, `host`, `pipeline`, `store`, `app`, `cli`).
- Data is transformed in one direction: demo bytes -> `demo::Parsed` -> `cs2::Match` -> `highlights::Highlight` -> `director::Timeline` -> `renderer::Output` files -> HLAE recording (`pipeline::record` over `host::Host`) -> `pipeline::assemble` MP4.
- Every process, socket and Win32 call goes through `host::Host` (`host::win::WinHost` on Windows, `host::fake::FakeHost` in tests); the game lives in a Job Object and is held by handle, not found by name (since phase 4).
- Domain types derive `serde::Serialize`/`Deserialize` so intermediate results persist as JSON (`plan.json`, `highlights.cfg`) and as SQLite rows (`crates/store/src/lib.rs`).
- Long-running work runs on `std::thread::spawn` threads, not an async runtime. No `tokio` dependency is declared in the workspace crates reviewed.
- Progress and cancellation are injected through the `pipeline::Progress` trait (`crates/pipeline/src/lib.rs:44`), with `pipeline::check` returning early when cancelled; `pipeline::Quiet` is the no-op implementation for the CLI.
- Position data is decoupled from direction via the `director::PositionSource` trait (`crates/director/src/lib.rs:33`); `director::NoPositions` (`crates/director/src/lib.rs:55`) provides a fallback so planning works without `positions`.

## Layers

- Purpose: Read `.dem` / `.dem.zst` files, decode protobuf messages and entity state.
- Location: `crates/demo/src/` (`lib.rs`, `parse.rs`, `command.rs`, `events.rs`, `bits.rs`, `proto` generated from `crates/demo/proto/*.proto` via `crates/demo/build.rs`); `crates/positions/src/lib.rs`.
- Contains: `demo::read_demo`, `demo::parse` (returns `Parsed`, `PlayerInfo`), `positions::track` (per-tick samples, flights, team/alive state). `positions` uses the external `source2-demo` crate pinned at `=0.5.9` (`crates/positions/Cargo.toml`).
- Depends on: `cs2` (for `GrenadeKind`, `PlayerId`), `director` (for `Pose`, `PositionSource`), `geometry`, `highlights`.
- Used by: `cli` and `pipeline`.
- Purpose: Build a match model from parsed demo data.
- Location: `crates/cs2/src/lib.rs`, `crates/cs2/src/report.rs`.
- Contains: `Match::from_parsed` (`crates/cs2/src/lib.rs:105`), `Match::build`, `Match::round`, `Match::player`, `Match::team_at`; types `Kill`, `Grenade`, `Round`, `Player`, `PlayerId`.
- Depends on: `demo`.
- Used by: `highlights`, `director`, `renderer`, `positions`, `pipeline`, `app`, `cli`.
- Purpose: Decide which kills and grenades are worth showing.
- Location: `crates/highlights/src/lib.rs`.
- Contains: `find` (`crates/highlights/src/lib.rs:74`), `score_kill`, `score_grenade`, `for_player`, `utility_for`; `Config` and `Highlight`/`Utility` types.
- Depends on: `cs2`.
- Used by: `director`, `pipeline`, `cli`.
- Purpose: Map collision meshes with BVH raycasting for wall checks (`geometry::Mesh::raycast`, `segment_clear`, `load_map`, `fetch`); FACEIT Data API client and demo download (`faceit::Client`, `demo_urls`, `download`, `parse_match_id`) plus player stats in `crates/faceit/src/stats.rs`.
- Location: `crates/geometry/src/lib.rs`, `crates/faceit/src/lib.rs`, `crates/faceit/src/stats.rs`.
- Depends on: no workspace crates (standalone). `geometry` reads `maps/*.mesh` and `maps/maps.json`.
- Used by: `positions`, `pipeline`, `cli`, `faceit` consumed by `pipeline` and `app`.
- Purpose: Turn highlights into a shot list: camera rigs, segments, speed/zoom keys, ordering, and grenade-specific shots.
- Location: `crates/director/src/lib.rs` (planning, `direct`, `direct_all`, `shoot`, `Timeline`), `crates/director/src/rig.rs` (camera rigs; referenced from README "Камеры"), `crates/director/src/grenade.rs` (`shoot_grenade`, `grenade_victims`).
- Contains: `Timeline`, `Shot`, `Segment`, `Camera`, `CamKey`, `ScaleKey`, `Order`, `Density`, `Config`, `Pose`, `PositionSource`, `look_at`.
- Depends on: `cs2`, `highlights`.
- Used by: `renderer`, `pipeline`, `positions`, `cli`.
- Purpose: Serialize a `Timeline` into the files HLAE and ffmpeg consume (the assembly itself moved to `pipeline::assemble` in phase 4; `crates/renderer/src/assemble.rs` no longer exists).
- Location: `crates/renderer/src/lib.rs` (`render`, `Output::write_to`, `record_order`, `ffmpeg_args`, `capture_fps`, `console_safe`).
- Contains: writers for `plan.json`, `highlights.cfg`, `cs2c_NN.cfg`, `campath_NN.xml`, `assemble.sh`, `assemble.bat`, `assemble.ffgraph`.
- Depends on: `cs2`, `director`.
- Used by: `pipeline`, `cli`.
- Purpose: The only crate that starts processes, opens sockets and calls Win32 (`host::Host` trait: launch the game session, run a command with deadline and cancel, `notify`, `now`).
- Location: `crates/host/src/lib.rs` (trait, `Session`, `Source`, `SessionEvent`), `crates/host/src/win/` (`WinHost` with a Job Object around HLAE/CS2, netcon, `console.log` watcher, `notify.rs` tray balloon via `Shell_NotifyIconW`), `crates/host/src/fake.rs` (`FakeHost`, virtual time), `crates/host/src/unsupported.rs`.
- Rule: the host reports facts, the caller decides (waits, deadlines, "is it a failure") — policy lives in `pipeline::record`.
- Used by: `pipeline`, `app`, `cli`.
- Purpose: Single entry for the full flow used by both UIs: one task from link to `highlights.mp4`.
- Location: `crates/pipeline/src/lib.rs` (`analyze`, `plan_player`, `load_positions`, `Progress`, `Stage`, `RunError`, `CANCELLED`), `job.rs` (`run_job`, `JobSpec`, `JobConfig`, `Outcome`, precheck/record/resume stages), `record/` (state machine, deadlines, failure classes `FailCode`/`FailClass`, markers), `precheck/` (environment checks, `nvenc_check`), `gamedir.rs` (files installed into the game folder, `installed.json`, cleanup after a crash), `assemble.rs` (`assemble_stage`, the only ffmpeg assembly, via `Host::run` with a deadline), `manifest.rs` (verified takes), `acceptance.rs` (acceptance set and batch naming).
- Depends on: `demo`, `cs2`, `highlights`, `director`, `renderer`, `faceit`, `positions`, `geometry`, `host`.
- Used by: `app`, `cli`.
- Purpose: SQLite application database for montage jobs, FACEIT response cache, and downloaded-demo registry.
- Location: `crates/store/src/lib.rs`, `crates/store/src/journal.rs` (parsing the job log: attempts and outcomes).
- Contains: `JobKind` (`Plan`, `Record`), `Store` (wraps `rusqlite::Connection` in `Arc<Mutex<...>>`), `next_queued`, `start_job`, `finish_job`, `cancel_job`, `requeue_auto`, `open_read_only` (used by `cs2-cli acceptance`); columns `auto_retries` and `batch` (migration 5); `Result<T> = std::result::Result<T, String>`.
- Depends on: `rusqlite`, `serde`. No workspace crates.
- Used by: `app`, `cli` (read only).
- Purpose: User interfaces.
- Location: `crates/app/src/` (egui/eframe desktop UI; `queue.rs` is the queue thread) and `crates/cli/src/` (`main.rs`, `record.rs`, `acceptance.rs`; Russian-language usage text).
- Contains: see Entry Points and STRUCTURE.md.
- Depends on: `pipeline`, `host`, `renderer`, `director`, `cs2`, `highlights`, `store`, `faceit`; `cli` also depends directly on `demo`, `positions`, `geometry`.
- Used by: end users only.

## Data Flow

### Primary Request Path: build a montage from a FACEIT link or demo file

### Desktop job lifecycle (`crates/app`)

### CLI path (`crates/cli/src/`)

- Pipeline state is passed by value between stages (`Analysis`, `Plan`, `Timeline`); there is no global pipeline state.
- Desktop state lives in `App` (`crates/app/src/app.rs:127`) with sub-structs `MatchesState`, `MatchState`, `JobsState`, and `Settings` (`crates/app/src/settings.rs`).
- Persistent state is SQLite only (`store`); settings are read and written by `crates/app/src/settings.rs`.

## Key Abstractions

- Purpose: Normalized match: players, rounds, kills, grenades, tick rate, map.
- Examples: `crates/cs2/src/lib.rs`
- Pattern: Built once from `demo::Parsed`, then read-only in all downstream crates.
- Purpose: Scored candidate moments (kill series, standout kills, grenades).
- Examples: `crates/highlights/src/lib.rs`
- Pattern: Pure scoring functions (`score_kill`, `score_grenade`) followed by filtering in `find`.
- Purpose: Ordered list of shots with camera keys and time scaling; serialized to `plan.json`.
- Examples: `crates/director/src/lib.rs:197` (`Timeline`), `:108` (`Shot`), `:89` (`Segment`)
- Pattern: Data-only structs with derived `Serialize`; `Timeline::total_secs` computes output length from `Shot::output_secs`.
- Purpose: Abstracts where player poses come from so the director can run with or without tick data.
- Examples: implemented by `positions::Track` (`crates/positions/src/lib.rs`); `director::NoPositions` is the null implementation.
- Pattern: Trait object passed as `&dyn PositionSource` into `direct_all`, `direct`, and `shoot`.
- Purpose: Stage/log/cancel callbacks decoupled from the UI.
- Examples: `pipeline::Quiet` (tests), `cli::record::ConsoleProgress` (console), `app::jobs::JobReporter` (desktop).
- Pattern: `&dyn Progress` threaded through every long operation; `check(p)?` at checkpoints.
- Purpose: Shared handle to the SQLite database.
- Examples: `crates/store/src/lib.rs`
- Pattern: `Arc<Mutex<Connection>>`, cloned into job threads.
- Purpose: Triangle mesh with BVH for wall-aware line-of-sight.
- Examples: `crates/geometry/src/lib.rs:53` (`from_bytes`), `:102` (`raycast`), `:132` (`segment_clear`)
- Pattern: Loaded from prebuilt `maps/*.mesh`; `geometry::fetch` re-downloads when `--refresh-maps` is used.

## Entry Points

- Location: `crates/app/src/main.rs` (`fn main`, line 19)
- Triggers: user launches `cs2cinema` executable (`crates/app/Cargo.toml` binary).
- Responsibilities: builds the eframe window, sets up tray icon (Windows), constructs `app::App::new`.
- Location: `crates/cli/src/main.rs` (+ `record.rs`, `acceptance.rs`)
- Triggers: `cs2-cli <FACEIT link | .dem path> [options]`, `cs2-cli record|retry ...`, `cs2-cli assemble <папка задачи>`, `cs2-cli acceptance check|state|job|set`.
- Responsibilities: argument parsing, pipeline invocation, plan and file output; CI builds it (`.github/workflows/ci.yml`, `cargo build --release -p cli`).
- `crates/cli/tests/pipeline.rs`, `crates/positions/tests/real_demo.rs`, and in-file `mod tests` blocks in most library crates. Real-demo tests are skipped unless `CS2_TEST_DEMO` is set (see `.github/workflows/ci.yml`).

## Architectural Constraints

- **Threading:** Multi-threaded using `std::thread`. The desktop app runs at most one pipeline job at a time because CS2 records only one game instance (`crates/store/src/lib.rs` module doc; the queue thread `crates/app/src/queue.rs` takes one task at a time). UI-side background work (`App::spawn`, `crates/app/src/app.rs:220`) runs in detached threads that report via `Msg`.
- **Global state:** `store::Store` is a shared `Arc<Mutex<Connection>>`. `crates/app/src/main.rs` installs global tray menu and tray-icon event handlers (`MenuEvent::set_event_handler`, `TrayIconEvent::set_event_handler`), which are process-wide.
- **Circular imports:** None observed. The crate graph is a DAG: `demo` -> `cs2` -> `highlights` -> `director` -> `renderer` -> `pipeline` -> `app`/`cli`, with `positions` and `geometry` feeding `director`/`pipeline`.
- **External tools:** `ffmpeg` and `ffprobe` must be on `PATH` (checked by `pipeline::precheck`; run through `Host::run` with a deadline in `pipeline::assemble`); HLAE is launched by path through `host::win::WinHost` (the game sits in a Job Object and is tracked by handle). Since phase 4 the app no longer polls or kills processes with system utilities (the old `launch.rs` with `tasklist`/`taskkill` was removed — "было до фазы 4"); a foreign `cs2.exe` is detected and left alone (`FOREIGN_GAME_RUNNING`), only our own orphans are removed by identity.
- **Network:** FACEIT API and CDN (`crates/faceit`), map geometry fetch from GitHub releases (`crates/geometry/src/lib.rs:292`, `:371`), update check (`crates/app/src/update.rs`).
- **Platform:** Windows is the primary target. `#![cfg_attr(windows, windows_subsystem = "windows")]` in `crates/app/src/main.rs`; several modules are Windows-only.
- **Language:** User-facing strings (CLI usage, UI labels, job titles, `Stage::title`) are in Russian; identifiers and code comments mix English and Russian.

## Anti-Patterns

### Duplicated ffmpeg invocation — закрыто (фазы 1–4)

### Record and assemble sequencing outside `pipeline` — закрыто (фаза 4)

## Error Handling

- Cancellation is an error value: `pipeline::CANCELLED` is matched in `app::jobs::finish` to choose `store.cancel_job` over `store.finish_job`.
- Task failures are typed: `pipeline::RunError.fail` carries a `RecordFailure` (`FailCode`, state, hint); `pipeline::record::failure::classify` maps it to `FailClass { Launch, Environment, Other }`, which `app::queue::QueuePolicy` uses for auto-retry and pause.
- Optional geometry and positions degrade gracefully: `--no-walls`, `--no-flybys`, and `--offline` skip those inputs (`crates/cli/src/main.rs` usage text) and `director::NoPositions` is used instead of `positions::Track`.
- Job failures are persisted (`store.finish_job(job.id, Err(e))`) and shown in the jobs page (`crates/app/src/jobs_page.rs`).

## Cross-Cutting Concerns

<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->

## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->

## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd-fast` for a trivial task inline, with no subagents and no PLAN.md
- `/gsd-quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd-debug` for investigation and bug fixing
- `/gsd-execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->

## Developer Profile

> Profile not yet configured. Run `/gsd-profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
