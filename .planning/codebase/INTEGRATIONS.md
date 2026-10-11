---
last_mapped_commit: 469e893c8d02fc142516cdc684fdbe62da39b50f
last_mapped_at: 2026-10-10
---
# External Integrations

**Analysis Date:** 2026-10-10

## APIs & External Services

**FACEIT Data API (`https://open.faceit.com/data/v4`):**
- Purpose: resolve a player by nickname, fetch match history, match statistics, and demo URLs for a match.
- Client: `crates/faceit/src/lib.rs` (`DATA_API` constant, `Client::get`, `player`, `history`, `match_stats`, `match_stats_json`, `demo_urls`). Response models in `crates/faceit/src/stats.rs`.
- Auth: API key sent with each request, read from env var `FACEIT_API_KEY` (`Client::from_env`) or entered in the Settings page and stored in `settings.json` (`crates/app/src/settings.rs`). Helper text points to `https://developers.faceit.com/` (`crates/app/src/settings_page.rs`).
- Error mapping: 401/403 -> "check FACEIT_API_KEY"; 404 -> not found (`crates/faceit/src/lib.rs`, `check`).
- Transport: `ureq` 3 agent with `RootCerts::PlatformVerifier` (OS trust store), `http_status_as_error(false)` so status codes are handled manually.

**FACEIT Downloads API (`https://open.faceit.com/download/v2/demos/download`):**
- Purpose: obtain a signed demo resource URL and download the `.dem.zst` demo.
- Client: `crates/faceit/src/lib.rs` (`DOWNLOAD_API`, `Client::download`, POST request). Requires a key with Downloads API access; otherwise FACEIT returns an access error.
- Local result: compressed demo is saved next to the decompressed `.dem` (decompression via `zstd` in `crates/demo`). Demo cache per match is in SQLite table `demos` (`crates/store/src/lib.rs`, `demo_path`, `remember_demo`).

**FACEIT web pages (links only, no API):**
- Match room links `https://www.faceit.com/en/cs2/room/{id}` and `.../ru/cs2/room/{id}/scoreboard` are parsed (`parse_match_id` in `crates/faceit/src/lib.rs`) and opened in the browser (`crates/app/src/match_page.rs`, `ui::open_url`).

**GitHub REST API (`https://api.github.com/repos/Kamaliev/cs2cinema/releases/latest`):**
- Purpose: in-app update check - compares the latest release tag to the compiled-in `CS2CINEMA_VERSION`.
- Client: `crates/app/src/update.rs` (`REPO = "Kamaliev/cs2cinema"`, `agent()` with platform verifier). No auth token is used; unauthenticated, subject to GitHub rate limits.
- Only active when the build has `CS2CINEMA_VERSION` set (CI release builds, see `.github/workflows/release.yml`).

**GitHub Releases (distribution):**
- `install.ps1` downloads `https://github.com/Kamaliev/cs2cinema/releases/latest/download/cs2cinema-windows-x64.zip` and the matching `.sha256` file, verifies the hash, and installs to `%LOCALAPPDATA%\cs2cinema`.
- Published by `.github/workflows/release.yml` on `v*` tags (uses `GITHUB_TOKEN` with `contents: write`).

**awpy-data (map geometry, GitHub):**
- Repo: `https://github.com/pnxenopoulos/awpy-data` (`DATA_REPO` in `crates/geometry/src/lib.rs`).
- Latest version is found by running `git ls-remote`-style tag listing through the `git` binary (`crates/geometry/src/lib.rs`, around line 293; newest numeric tag equals the CS2 ClientVersion).
- Release zip is downloaded via `ureq` with platform verifier and extracted with `zip` (`crates/geometry/src/lib.rs`, `fetch` at line ~371). Stored in `maps/` (default; overridable by `CS2CINEMA_MAPS`). `maps/maps.json` and `maps/*.mesh` are present in the repo working tree and gitignored (`.gitignore` contains `/maps`).
- Refresh behavior: CLI checks at most once per day (`README.md`, "Карты" section).

**Source2Viewer-CLI (map extraction):**
- Downloaded automatically by `scripts/update_maps.py` (per `README.md`) to extract meshes from a local CS2 install. Not called from Rust code.

**Local game-file access (CS2 install):**
- Steam library discovery: `crates/app/src/settings.rs` (`find_cs2`, `find_hlae`) reads `steamapps/libraryfolders.vdf` under common Steam roots (`C:/Program Files (x86)/Steam`, `D:/SteamLibrary`, ...); the console takes folders from `--cs2`/`--hlae` or `CS2_DIR`/`HLAE_DIR`.
- Reads `steamapps/common/Counter-Strike Global Offensive/game/csgo/...` and `game/bin/win64` paths; `pipeline::gamedir::install_files` puts `highlights.cfg`, `cs2c_NN.cfg`, and `cs2c_NN_K.xml` there and records them in `installed.json`, so `cleanup_installed`/`recover` can remove exactly our files after a job or a crash.
- `scripts/update_maps.py` reads `steam.inf` for `ClientVersion` (`README.md`).

## External Tools (process integrations)

**Counter-Strike 2 (`cs2.exe`):**
- Launched by HLAE with `-insecure -steam -novid -allow_third_party_software -netconport {port} -windowed -w {w} -h {h} +demo_ui_mode 0 +playdemo {demo}` (`host::win::WinHost::launch`, `crates/host/src/win/`).
- Commands are sent to the game console over a local TCP netcon socket on `127.0.0.1` (`crates/host/src/win/netcon.rs`, ephemeral port); console output is also read from `game/csgo/console.log` (`-condebug`, `console_log.rs`).
- Sends `exec highlights` and other console lines; the command dialect is centralized in `crates/renderer/src/lib.rs` (`Dialect`, `preamble`).
- Process state: the game runs in a Job Object created by `host::win::WinHost` and is tracked by handle (`Liveness`); a foreign `cs2.exe` is only detected (`FOREIGN_GAME_RUNNING`) and never stopped; our own orphans after a crash are removed by process identity (`pipeline::gamedir::kill_orphans`). Earlier versions polled and killed `cs2.exe` with system utilities by name — "было до фазы 4", removed.
- Demo files: CS2 reads `.dem` only; `.dem.zst` is decompressed by the app/CLI before playback (`README.md`).

**HLAE - advancedfx (`HLAE.exe`, `x64/AfxHookSource2.dll`):**
- Required for recording. Discovered by `find_hlae` in `crates/app/src/settings.rs` (common paths, drive scan) or set in Settings (`settings.json`) or `--hlae`/`HLAE_DIR` in the console. `pipeline::precheck` requires `HLAE.exe` and `x64/AfxHookSource2.dll`.
- CLI-side command: `HLAE.exe -customLoader -noGui -autoStart -hookDllPath ... -programPath ...cs2.exe -cmdLine "..."` is documented in `README.md`; the app launches HLAE directly via `Command::new(HLAE.exe)` with `current_dir(hlae_dir)`.
- Recording: HLAE writes clips through `startmovie` and `mirv_streams settings add ffmpeg cs2c "..."` (`crates/renderer/src/lib.rs`, around line 167). The ffmpeg that HLAE uses is configured in `ffmpeg/ffmpeg.ini` inside the HLAE folder; `pipeline::precheck::gather::hlae_ffmpeg` parses its `Path=` key.
- Output: clips are written under the result folder's `clips/` (path set by `crates/renderer/src/lib.rs`). A take is finished by console markers (`pipeline::record::markers`) and then validated (`pipeline::record::verify`, `clips/manifest.json`); the old wait by stable folder size was removed ("было до фазы 4").
- Download: `https://github.com/advancedfx/advancedfx/releases` (link only; `crates/app/src/settings_page.rs`).

**ffmpeg / ffprobe (7+):**
- `ffmpeg` runs the assembly graph (`assemble.ffgraph`) through generated `assemble.sh`/`assemble.bat` (for manual re-runs) and, in the app and `cs2-cli assemble`, through `pipeline::assemble::assemble_stage` — the only place that starts it, via `Host::run` with a deadline and cancel.
- `ffprobe` returns clip durations and validates takes (`pipeline::record::verify`), also through `Host::run`.
- Presence check: part of `pipeline::precheck` (a missing ffmpeg is `PRECHECK_FAILED`, class "окружение").
- NVENC check: a short test encode via `pipeline::precheck::nvenc_check` (`host::Host::run`); `pipeline::job` uses the result to choose the encoder.
- Caption font: Arial Bold from `C:\Windows\Fonts` by default, overridable by `CS2CINEMA_FONT` (`crates/pipeline/src/lib.rs`, `crates/cli/src/main.rs`).
- Install hint: `winget install Gyan.FFmpeg` (`README.md`).

**Windows shell helpers:**
- `cmd /c start` or equivalent for opening URLs/files (`crates/app/src/ui.rs`, ~line 275); `xdg-open` on non-Windows (~line 281).
- `explorer`/shortcut creation via `WScript.Shell` COM in `install.ps1`.

**git:**
- Used by `crates/geometry/src/lib.rs` (~line 293) to list tags of the awpy-data repository.

## Data Storage

**Databases:**
- SQLite (bundled via `rusqlite` 0.32.1 `bundled` feature).
  - File: `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` (`crates/app/src/app.rs`, line ~154, `settings::data_dir().join("cs2cinema.db")`).
  - Mode: `PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;` (`crates/store/src/lib.rs`, line ~210).
  - Schema (`crates/store/src/lib.rs`, `CREATE TABLE IF NOT EXISTS ...`):
    - `jobs` - montage jobs: source kind/value, hero name and SteamID, map, status, stage, log, result.
    - `faceit_cache` - key/value JSON cache for FACEIT responses (`cache_get`, `cache_put`).
    - `demos` - `match_id` to local demo path (`demo_path`, `remember_demo`).
  - API: `Store::open`, `Store::in_memory` (tests), job lifecycle methods (`add_job`, `next_queued`, `start_job`, `finish_job`, `cancel_job`, `requeue`, `reset_running`).
  - Client/ORM: none; hand-written SQL via `rusqlite`.
  - Crash recovery: `reset_running` marks jobs left in running state after a restart (`crates/store/src/lib.rs`).

**File Storage:**
- Local filesystem only.
- Demos: user-chosen path; `.dem` and `.dem.zst` written next to the source or into the match output folder (`out/` in dev, `out\<match>\` per README).
- Outputs per run: `plan.json`, `highlights.cfg`, `cs2c_NN.cfg`, `cs2c_NN_K.xml`, `campath_NN.xml`, `assemble.sh`, `assemble.bat`, `assemble.ffgraph`, `clips/`, final `highlights.mp4` (`crates/renderer/src/lib.rs`, `crates/cli/src/main.rs`).
- Repo-local sample output: `out/` (gitignored). Map meshes: `maps/` (gitignored).

**Settings:**
- `%LOCALAPPDATA%\cs2cinema\settings.json` - JSON via `serde_json` (`crates/app/src/settings.rs`, `data_dir()`, `settings_path()`). Contains HLAE dir, CS2 dir, output dir, FACEIT key and nickname (values never committed).

**Caching:**
- SQLite `faceit_cache` table (see above). No external cache service.

## Authentication & Identity

**Auth Provider:**
- No user accounts or login system in the application.
- FACEIT: API key only (Bearer-style header set in `crates/faceit/src/lib.rs`, `get`/`post`). Key source: `FACEIT_API_KEY` env var or `settings.json`.
- GitHub: unauthenticated public API for release checks; `GITHUB_TOKEN` used only inside GitHub Actions workflows (`.github/workflows/release.yml`).
- Steam identity: player is matched to a demo by SteamID64 from FACEIT (`game_player_id` in `crates/faceit/src/stats.rs`), falling back to nickname. No Steam Web API calls are made.

## Monitoring & Observability

**Error Tracking:**
- None. No Sentry or similar service detected.

**Logs:**
- Job logs are stored in SQLite (`jobs` log via `append_log` in `crates/store/src/lib.rs`) and surfaced in the Jobs page (`crates/app/src/jobs_page.rs`, "Журнал").
- Stderr/stdout of ffmpeg and HLAE captured by `host::Host::run` / the game session (`record.log`, `CmdOutput`) with first-line error extraction (`pipeline::precheck::nvenc_check`, `pipeline::assemble::ffmpeg_error`).
- Queue summary notification: `host::win::notify` shows a tray balloon "N готово / M упало" (`Shell_NotifyIconW` with `NIF_INFO`) when the queue empties or pauses, so it works while the window is hidden in the tray.

**Analytics:**
- None.

## CI/CD & Deployment

**Hosting / Distribution:**
- GitHub repository `Kamaliev/cs2cinema`; releases are the distribution channel (`install.ps1`, `README.md`).

**CI Pipeline:**
- GitHub Actions, `.github/workflows/ci.yml`:
  - Triggers: push to `main`, pull requests, manual dispatch.
  - Matrix: `ubuntu-latest`, `windows-latest`.
  - Steps: `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `cargo test --release --workspace`, `cargo build --release -p cli`, upload `cs2-cli-windows-x64` artifact (`actions/upload-artifact@v4`).
  - Demos are in Git LFS and are not downloaded in CI; real-demo tests are skipped without `CS2_TEST_DEMO`.
- `.github/workflows/release.yml`:
  - Triggers: tags `v*`, manual dispatch with optional tag.
  - Builds `cli` and `app` on `windows-latest` with `CS2CINEMA_VERSION` set from the tag.
  - Packages `cs2cinema-windows-x64.zip` plus `.sha256`, publishes via GitHub Release (`permissions: contents: write`).

## Environment Configuration

**Required env vars (for features, none are required for building):**
- `FACEIT_API_KEY` - needed for any FACEIT lookup or demo download (CLI and app; app can use settings instead).

**Optional env vars:**
- `CS2CINEMA_MAPS`, `CS2CINEMA_FONT`, `CS2_DIR`, `CS2CINEMA_PAGE`, `CS2CINEMA_MOCK`, `CS2_TEST_DEMO`, `CS2_TEST_MAPS`, `PROTOC`, `CS2CINEMA_VERSION` (compile time). See `STACK.md` for details.

**Secrets location:**
- FACEIT key: environment variable `FACEIT_API_KEY` or `%LOCALAPPDATA%\cs2cinema\settings.json` (plain text file; no OS keychain integration detected).
- No `.env` loading is implemented in the code. A `.env` file is not part of the repo structure checked for this map; its contents were not read.

## Webhooks & Callbacks

**Incoming:**
- None. The app does not listen on any public port. The only local listener is the transient `127.0.0.1` ephemeral TCP port used as the CS2 netcon channel (outbound connection from the app to the game; see `crates/host/src/win/netcon.rs`).

**Outgoing:**
- FACEIT Data API and Downloads API (HTTPS, see above).
- GitHub API for update checks and awpy-data tag/release fetch (HTTPS and `git`).
- No webhooks configured.

---

*Integration audit: 2026-10-10*
