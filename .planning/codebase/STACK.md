---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
# Technology Stack

**Analysis Date:** 2026-10-10

## Languages

**Primary:**
- Rust (stable toolchain, 2021-style workspace, resolver 2) - all application and library code under `crates/*/src/`. Declared workspace members in `Cargo.toml`: `crates/demo`, `crates/highlights`, `crates/director`, `crates/cs2`, `crates/renderer`, `crates/faceit`, `crates/positions`, `crates/geometry`, `crates/host`, `crates/pipeline`, `crates/store`, `crates/app`, `crates/cli`.

**Secondary:**
- PowerShell - `install.ps1` (end-user installer, downloads release zip, verifies SHA-256, updates user PATH, creates Start Menu shortcut).
- POSIX shell and batch - generated assembly scripts written by `crates/renderer/src/lib.rs` (`assemble.sh`, `assemble.bat`) per output folder; not hand-maintained source.
- Python 3.11+ - optional `scripts/update_maps.py` (extracts map geometry from a local CS2 install; not needed for normal runs).
- Protocol Buffers - `crates/demo/proto/demo.proto` and `crates/demo/proto/events.proto` (CS2 demo message definitions, compiled at build time).
- Source2 Engine Constants/Text - `highlights.cfg`, `cs2c_NN.cfg`, `cs2c_NN_K.xml` camera paths generated for the CS2 console and HLAE (see `crates/renderer/src/lib.rs`). These are outputs, not source.
- Markdown - `README.md` (Russian-language user documentation).

## Runtime

**Environment:**
- Native binaries, no managed runtime. Primary target is Windows 11 x64 (`cfg(windows)` code paths in `crates/host/src/win/`, `crates/app/src/ui.rs`, `crates/app/src/app.rs`). Linux is built and tested in CI (`.github/workflows/ci.yml`) but the desktop app is Windows-oriented.
- Release binaries: `cs2-cli.exe` (crate `cli`) and `cs2cinema.exe` (crate `app`, built with `windows_subsystem = "windows"` in `crates/app/src/main.rs`).

**Package Manager:**
- Cargo (workspace at `Cargo.toml`).
- Lockfile: present - `Cargo.lock` at repo root (committed).
- No `rust-toolchain` / `rust-toolchain.toml` file pinned; CI uses `dtolnay/rust-toolchain@stable`.

## Frameworks

**Core (desktop UI):**
- `eframe` 0.31.1 (egui framework) with `glow` renderer, `default_fonts`, `wayland`, `x11` features - `crates/app/Cargo.toml`. Entry point `crates/app/src/main.rs`, app state in `crates/app/src/app.rs`.
- `tray-icon` 0.19.3 (Windows only, `default-features = false`) - system tray, `crates/app/Cargo.toml` under `[target.'cfg(windows)'.dependencies]`.
- `rfd` 0.15.4 - native file/folder pickers.

**Core (CLI):**
- Hand-written argument parsing in `crates/cli/src/main.rs` (no CLI framework crate listed in manifests).

**Domain libraries (internal workspace crates):**
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

**Testing:**
- Built-in `cargo test` harness. Integration test on a real demo: `crates/positions/tests/real_demo.rs` (skipped unless `CS2_TEST_DEMO` is set).
- Dev-dependency `prost` 0.14.4 in `crates/cli/Cargo.toml` and `demo`/`cs2`/`highlights` as dev-deps in `crates/positions/Cargo.toml`.

**Build/Dev:**
- `prost-build` 0.14.4 and `protoc-bin-vendored` 3.3.0 (build-dependencies of `crates/demo`, script `crates/demo/build.rs`). `protoc` is vendored, so no system protoc install is needed unless `PROTOC` is set (build.rs checks `PROTOC`).
- `Swatinem/rust-cache@v2` and `actions/upload-artifact@v4` in CI.

## Key Dependencies

**Critical (per crate manifests and `Cargo.lock`):**
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

**Infrastructure / external binaries (not Cargo crates, invoked as processes):**
- `ffmpeg` 7+ and `ffprobe` - clip assembly and duration probe. Started only through `host::Host::run` (deadline + cancel) from `pipeline::assemble` and `pipeline::record::verify`. NVENC availability check: `pipeline::precheck::nvenc_check`.
- `HLAE.exe` (advancedfx) with `x64/AfxHookSource2.dll` - records CS2 via `startmovie`. Launched by `host::win::WinHost::launch` inside a Job Object, path discovered by `find_hlae` in `crates/app/src/settings.rs` (or `--hlae`/`HLAE_DIR` in the console).
- Windows API (`windows-sys` 0.61 in `crates/host`, только `cfg(windows)`): Job Object, process handles, `Shell_NotifyIconW` for the tray balloon. The earlier `tasklist`/`taskkill` utilities were removed in phase 4 ("было до фазы 4"); the workspace has no calls to them.
- `cmd` / `xdg-open` - opening URLs in `crates/app/src/ui.rs`.
- `git` - queries the latest `awpy-data` release tag in `crates/geometry/src/lib.rs` (line ~293).

## Configuration

**Environment variables (names only; values are user-supplied):**
- `FACEIT_API_KEY` - FACEIT Data API key (read in `crates/faceit/src/lib.rs`, `crates/cli/src/main.rs`).
- `CS2CINEMA_MAPS` - folder of `*.mesh` map geometry; default `./maps` (`crates/geometry/src/lib.rs`).
- `CS2CINEMA_FONT` - caption font path; default Arial Bold from Windows (`crates/pipeline/src/lib.rs`, `crates/cli/src/main.rs`).
- `CS2_DIR` - CS2 install directory override (`crates/cli/src/main.rs`, `scripts/update_maps.py`).
- `CS2_TEST_DEMO`, `CS2_TEST_MAPS` - enable the real-demo tests (`crates/positions/tests/real_demo.rs`, `crates/pipeline/src/lib.rs`).
- `CS2CINEMA_VERSION` - baked into the app at compile time via `option_env!` (`crates/app/src/update.rs`); set by `.github/workflows/release.yml`.
- `CS2CINEMA_MOCK`, `CS2CINEMA_PAGE` - debug-only UI overrides in `crates/app/src/app.rs` and `crates/app/src/mock.rs`.
- `PROTOC` - optional override of the vendored protoc in `crates/demo/build.rs`.
- `LOCALAPPDATA`, `XDG_DATA_HOME`, `HOME`, `USERPROFILE`, `ProgramFiles(x86)` - used to locate the data dir and search paths.

**Application data files (runtime, not committed):**
- `%LOCALAPPDATA%\cs2cinema\settings.json` - user settings (resolved in `crates/app/src/settings.rs`, `data_dir()` and `settings_path()`).
- `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` - SQLite database (opened in `crates/app/src/app.rs`, schema in `crates/store/src/lib.rs`).
- Note: the FACEIT key is stored in `settings.json` as entered in the UI; the file path is shown in the Settings page (`crates/app/src/settings_page.rs`).

**Build:**
- `Cargo.toml` (workspace), per-crate `Cargo.toml` files under `crates/*/`.
- `.gitignore` excludes `/target`, `*.dem`, `*.dem.zst`, `/out`, `/maps`, `__pycache__/`, `.idea/`.
- `.gitattributes` - line-ending/LFS rules (demo files are stored in Git LFS per `.github/workflows/ci.yml` comment).

## Platform Requirements

**Development:**
- Rust stable with the MSVC toolchain on Windows (Visual Studio Build Tools, "Desktop development with C++", per `README.md`).
- Git and Git LFS (demos live in LFS).
- Optional: Python 3.11+ for `scripts/update_maps.py`.
- Recommended: `ffmpeg` 7+ on `PATH` (`winget install Gyan.FFmpeg`).

**Runtime (end user):**
- Windows 11 x64 (release artifacts are `cs2cinema-windows-x64.zip` and `cs2-cli.exe`).
- CS2 installed through Steam (app looks in `steamapps/common/Counter-Strike Global Offensive`, see `find_cs2` in `crates/app/src/settings.rs`).
- HLAE 2.192+ with `x64/AfxHookSource2.dll` (`README.md` lists 2.192.7 as tested).
- NVIDIA GPU with a recent driver for NVENC encoding (optional; software encoding is used otherwise per `pipeline::precheck::nvenc_check`).
- A FACEIT Data API key with Downloads API access for FACEIT features.

**Production / Distribution:**
- GitHub Releases on `Kamaliev/cs2cinema`: `release.yml` builds `cs2-cli` and `cs2cinema` on `windows-latest` on tags `v*` and packages `cs2cinema-windows-x64.zip` plus `.sha256`.
- Installer: `install.ps1` (downloads from `https://github.com/Kamaliev/cs2cinema/releases/latest/download/`, installs to `%LOCALAPPDATA%\cs2cinema`, adds to user PATH).

---

*Stack analysis: 2026-10-10*
