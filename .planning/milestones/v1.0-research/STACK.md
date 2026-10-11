# Stack Research

**Domain:** Reliable automated CS2 demo recording (HLAE + CS2 + ffmpeg) driven from a Rust desktop app on Windows
**Milestone:** Subsequent ("Reliable recording") on the existing cs2cinema workspace (Rust 1.99, edition 2024, egui/eframe, `std::thread`, rusqlite, no tokio)
**Researched:** 2026-10-10
**Overall confidence:** HIGH for HLAE/CS2/Windows facts (read from HLAE `main` source and cross-checked against the author's own `console.log` and HLAE install on this machine); MEDIUM for crate fit and timeout values (need one spike each, listed at the end)

Confidence tags used below: **[HIGH]** = verified in primary source code or official docs AND/OR observed on this machine; **[MEDIUM]** = single good source or sound inference, verify in a spike; **[LOW]** = community claim or guess, do not build on it without a spike. (The GSD `classify-confidence` seam rates bare `webfetch`/`websearch` hits LOW and verified `websearch` MEDIUM; every [HIGH] item here was cross-verified against HLAE/CSDM source code or the local console log, not a bare web hit.)

## Executive recommendation

1. **Own the whole process tree with one Windows Job Object** (`windows-sys`, about 200 lines of isolated `unsafe`), created with `KILL_ON_JOB_CLOSE` and an I/O completion port. HLAE's loader creates `cs2.exe` without `CREATE_BREAKAWAY_FROM_JOB`, so `cs2.exe` and the `ffmpeg.exe` that HLAE spawns for each take both land in our job. That one object gives PID/handle ownership (no `taskkill /IM`), crash/exit notifications (no `tasklist` polling), exact "ffmpeg finished writing the take" detection, and guaranteed cleanup even if cs2cinema itself crashes.
2. **One event channel, many producer threads, `recv_timeout` consumer.** Job events, netcon lines, `console.log` tail, a modal-dialog sniffer and phase deadlines all push `Event`s into one `crossbeam-channel`; the orchestrator waits with `recv_timeout(<=100 ms)` and checks the existing `Progress::cancelled()` every slice. Cancel latency is about 100 ms, no trait change, no async.
3. **Evidence-based markers, not file mtimes.** HLAE prints `Recording to "<dir>"` and `Finishing recording ... done.`; our cfg prints `CS2C ...` markers with a per-run nonce. Completion of a take = marker AND ffmpeg child exited (job event) AND `ffprobe` passes. HLAE's own "Finishing recording ... done." is NOT proof the file is complete (capture shutdown is queued on the render thread).
4. **Fix the console channel first.** On Windows CS2, `-netconport` does not reliably open unless `WSAStartup` has run early; HLAE ships `-afxFixNetCon` for exactly this (since AfxHookSource2 0.25.0). The current launcher does not pass it. Add it, and add `-condebug` so `game/csgo/console.log` is a second, always-available evidence stream.
5. **Detect the hang-forever failure modes that are currently invisible**: AfxHookSource2 and HLAE.exe raise *modal* `MessageBox`/dialogs ("Error - AfxHookSource2", "Image injection problem. Continue waiting?", the missing-`-insecure` box that loops forever). Sniff top-level windows owned by job PIDs and fail fast with the dialog text.

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended | Conf. |
|------------|---------|---------|-----------------|-------|
| `windows-sys` (cfg(windows)) | 0.61.2 | Job Object, IOCP, `OpenProcess`/`WaitForSingleObject`, ToolHelp snapshot, `EnumWindows`, `GetDiskFreeSpaceExW`, `GetFileVersionInfoW` | Thin raw bindings, no wrapper types, already in `Cargo.lock` at 0.61.2 (transitive via eframe/shared_child), so zero new resolution. All needed functions confirmed present in 0.61.2 on docs.rs. Latest published `windows` is 0.62.2 (Oct 2025); `windows-core`/`windows-link` jumped to 0.100.0 (Sep 2026), so do not mix `windows` versions - stay on `windows-sys` 0.61. | HIGH |
| `crossbeam-channel` | 0.5.17 | Multi-producer event bus with `select!`/`recv_timeout`/`after` | Already in `Cargo.lock`. `std::sync::mpsc` has `recv_timeout` but no `select!`; we need several sources plus deadlines. Pure threads, no runtime. | HIGH |
| `shared_child` | 1.1.2 | One-shot external commands (`ffmpeg`, `ffprobe`, NVENC probe) with timeout and cross-thread kill | `wait_timeout`/`wait_deadline` plus `kill()` callable from another thread while a reader waits (std `Child` needs `&mut self` for both, which forces a mutex and deadlocks on cancel). Depends on `windows-sys ^0.61`, same as ours. Sync, no tokio. | HIGH |
| `thiserror` | 2.0.21 | Typed `RecordError { stage, cause }` | Already in lock. Lets tests assert "not started" vs "timeout" vs "cancelled" without string compares; map to Russian text only at the UI boundary. | HIGH |
| std `File::try_lock` | Rust >= 1.89 (toolchain is 1.99) | Single-instance "one recording at a time" lock file across app + CLI | Stable in std, uses `LockFileEx` on Windows; no `fs4`/`fs2` needed (`fs2` unmaintained since 2018). | HIGH |
| std `OpenOptionsExt::share_mode(0)` | std | Exclusive-open probe: "is anyone still writing this take file?" | Fails with `ERROR_SHARING_VIOLATION` while ffmpeg holds the file. Cheap secondary completion signal. | MEDIUM |

### Supporting Libraries

| Library | Version | Purpose | When to Use | Conf. |
|---------|---------|---------|-------------|-------|
| `steamlocate` | 2.1.1 (Aug 2026) | Locate Steam + CS2 (`find_app(730)`) incl. `install_dir`, `state_flags`, `last_updated`, `build_id` from `appmanifest_730.acf` | Preflight: replaces the hard-coded drive list in `find_cs2()`; `state_flags` tells "game is mid-update". Note `App` is `#[non_exhaustive]`. Fallback to the current scan if it returns `None`. | MEDIUM |
| `which` | 8.0.6 | Resolve `ffmpeg`/`ffprobe` to an absolute path once, log it, reuse it | Preflight + all spawns; avoids PATH drift between preflight and later `Command::new("ffmpeg")`, and shows the user *which* ffmpeg was picked. Honors PATHEXT on Windows. | HIGH |
| `tracing` + `tracing-subscriber` + `tracing-appender` | 0.1.44 / 0.3.23 / 0.2.5 | Developer log file in `%LOCALAPPDATA%\cs2cinema\logs\` | The app is `windows_subsystem = "windows"`, so stderr goes nowhere; panics and thread failures currently vanish. Use rolling non-blocking file appender + a panic hook. Keep the user-facing Russian job journal in SQLite exactly as is; tracing is for developers. `tracing` 0.1.44 is already in the lock. | MEDIUM |
| `serde_json` (existing) | 1.0.x | Per-take manifest (`clips/manifest.json`: expected duration, dir from "Recording to", ffprobe result, state) and `ffprobe -of json` parsing | Needed for "retry from record stage" and per-take validation; no new dependency. | HIGH |
| `tempfile` (dev) | 3.27.0 | Fake game folders / clip dirs in tests | Already in lock. | HIGH |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| In-repo `fake-game` helper binary (`[[bin]]` or `examples/`, found via `env!("CARGO_BIN_EXE_...")`) | Integration-test the real Job Object / timeout / cancel code on CI without CS2 | Small program that: listens on a netcon port and prints scripted lines, optionally spawns a child (stand-in for ffmpeg), optionally hangs, crashes with an exception code, or shows a `MessageBox`. Lets the three required scenarios ("does not start", "timeout", "cancel") run against real Win32 primitives. Gate the job tests with `#[cfg(windows)]`; the trait-level fake covers Linux CI. |
| Hand-written `GameHost` trait + `FakeHost` (scripted `Vec<Event>` over a crossbeam channel) | Unit-test the orchestrator state machine | Preferred over `mockall` 0.15: the host is an event source plus 4 commands, a scripted fake is shorter and keeps test intent readable. |
| `HLAE\AfxHookSource2_changelog.xml` (shipped in the HLAE folder) | Offline HLAE-vs-CS2 compatibility heuristic | Contains `<version>`, `<time>`, and lines like `Adjusted to CS2 update (1.41.8.5)`. Compare with `PatchVersion=` in `game/csgo/steam.inf`. |

## HLAE facilities (verified against advancedfx `main` and HLAE 2.192.7 / AfxHookSource2 0.41.7, released 2026-10-08)

### HLAE.exe command line [HIGH - `hlae/Program.cs`]

`HLAE.exe -customLoader -noGui -autoStart -noConfig -hookDllPath <abs>\x64\AfxHookSource2.dll -programPath <abs>\cs2.exe -cmdLine "<game args>" [-addEnv KEY=VALUE ...]`

- Current launcher passes everything except **`-noConfig`**. Add it (CSDM does): HLAE otherwise loads/writes `%APPDATA%\HLAE\hlaeconfig.xml` and can inherit stale custom-loader settings from the user's GUI sessions.
- `-addEnv` exists if env vars ever need forwarding (when used, HLAE rebuilds the environment from its own and appends).
- **Exit code**: `Main` returns `0` if the loader succeeded and `1` if it failed. On failure HLAE first shows a modal `ErrorDialogue` and only returns after the user closes it, so an unattended failure shows up as "HLAE.exe still alive after N seconds", not as exit code 1.
- **Lifetime**: loader does `CreateProcessW(cs2.exe, CREATE_SUSPENDED | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP, no breakaway)` -> runs `injector.exe` (x64) -> `Sleep(2000)` -> `ResumeThread` -> HLAE.exe exits. Expect HLAE.exe alive roughly 3-8 s. If the injector hits a problem it can pop `MessageBox("Image injection problem.\nContinue waiting?")`.
- **Consequence for process control**: `cs2.exe` is a grandchild of cs2cinema via a *short-lived launcher*; `Child` of HLAE.exe tells you nothing about the game. Because there is no `CREATE_BREAKAWAY_FROM_JOB`, the Job Object membership of HLAE.exe is inherited by `cs2.exe` (Microsoft Job Objects docs: children created with `CreateProcess` are associated with the job by default). [HIGH by source + docs; confirm empirically in spike 1]

### AfxHookSource2 launch arguments [HIGH - `AfxHookSource2/main.cpp`]

| Argument | Effect | Recommendation |
|----------|--------|----------------|
| `-insecure` | **Mandatory.** Without it the DLL shows `ErrorBox` ("Please add -insecure ...") then `TerminateProcess` on itself and, if that fails, loops `MessageBox("Please terminate the game manually in the taskmanager!")` **forever** | Always passed (already is). Preflight asserts it is in the effective command line (see `[CommandLine]` echo below). |
| `-afxFixNetCon` | Calls `WSAStartup` in `DllMain`. Reason: Valve bug csgo-osx-linux#3603 - on Windows `-netconport` socket creation fails with `WSANOTINITIALISED` because Winsock is initialised too late (only `-tools` or later Steam init fixes it). Added in AfxHookSource2 0.25.0 (2024-10-07). | **Add.** Likely a root cause of the observed intermittent "консоль игры не открылась". Harmless if Winsock already started (refcounted). [HIGH it exists; MEDIUM that it is the cause of the author's flakiness - spike 2] |
| `-afxDisableSteamStorage` | Returns null for `STEAMREMOTESTORAGE_INTERFACE_VERSION016`, so the game cannot read/write Steam Cloud config | **Add.** `console.log` on this machine shows `Saved 'cs2_user_convars.vcfg' to SteamRemoteStorage` at quit, i.e. recording sessions currently write the author's real cloud config. HLAE's own launcher and CSDM pass it. |
| `-afxThreadPoolSize N` | Capture thread pool size | Leave default. |

**There is no `-afxTrace` and no HLAE log file.** Grepping the whole HLAE source finds only the three `-afx*` args above, and no hook-side log writer. Diagnostics for HLAE come from: the game console (netcon stream + `console.log`), HLAE.exe's exit code, job events, and window titles. Do not plan around an HLAE trace flag.

### Console output HLAE produces (usable as markers) [HIGH - source `RenderSystemDX11Hooks.cpp` + lines observed in local `console.log`]

| Line (exact) | Meaning |
|--------------|---------|
| `\| AfxHookSource2 (Oct  8 2026 17:00:09)` (box banner) | The hook is loaded and running. Absent after `Source2Init OK` = game started vanilla (injection failed). Also gives the hook build date. |
| `Starting recording ... done.` then `Recording to "C:\...\take0003".` | Take started; **the quoted path is the authoritative take directory** (no need to guess `takeNNNN` numbering). |
| `Starting recording ... FAILED` + `Error: Failed to create directories for "<name>".` | Hard failure to start a take. |
| `Finishing recording ... done.` | Capture shutdown was *queued* (`EndCapture` pushes `ShutDown` onto the render thread's `AfterPresentOrContextLossReliable` queue). **Not** proof the mp4/wav are finished. |
| `AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: CreateProcessW.` followed by hundreds of `AFXERROR: Failed writing image for screen recording.` | HLAE could not start *its* ffmpeg (bad `ffmpeg.ini` path, AV blocking). **Observed on this machine.** HLAE keeps "recording" into the void, so today the pipeline would wait until stall/timeouts. Treat the first `AFXERROR:` as fail-fast; rate-limit/dedupe repeated lines when logging. |
| `AFXERROR: ... could not create path "..."` | Take folder not creatable. |
| `Could not find address for pattern: ...` | AfxHookSource2 signature scan failed = HLAE not compatible with this CS2 build. |
| Modal box titled `Error - AfxHookSource2` (`MessageBoxA`) | Fatal hook error (failed detour, missing interface, etc.) - blocks the game thread until dismissed. CSDM detects this by checking window titles; we should too. |
| `<n>: <tick> (eps=1) -> <command>` | `mirv_cmd` trace when a scheduled command fires (also contains the command text, so naive `contains("CS2C_...")` matches it; use anchored parsing, below). |
| `Warning: You probably forgot to set host_framerate / mirv_streams record fps` | Misconfigured take (`m_Override fps` 0). |

### Take mechanics [HIGH - source]

- Take dir = `<record name>\take` + 4 digits where the number is **max existing + 1** (`SuggestTakePath`, gaps are not reused). Existing code's "move old `clips/` aside so numbering restarts at 0000" is therefore correct but fragile; with the `Recording to "<dir>"` line we can map shot -> dir explicitly and stop depending on numbering.
- ffmpeg for takes: path from `<HLAE>\ffmpeg\ffmpeg.ini` `[Ffmpeg] Path=` (fallback `<HLAE>\ffmpeg\bin\ffmpeg.exe`), spawned by `cs2.exe` with `CREATE_NO_WINDOW` and piped stdin/stdout/stderr. On this machine: `Path=C:\Program Files (x86)\HLAE FFMPEG\ffmpeg\bin\ffmpeg.exe`. Preflight must check that file exists and runs.
- Finalisation is asynchronous and ordered by the render thread; on `quit` (`HostStateRequest::Start(HSR_QUIT)`) HLAE calls `AfxStreams_ShutDown()`, so a graceful `quit` finalises streams, while killing the job mid-take leaves a truncated mp4 (no `moov` atom -> `ffprobe` fails, which is exactly the check we want).
- `ffmpeg` child exit is the real "file closed" signal. It is a child of `cs2.exe`, hence inside our job.

### Optional, not recommended now [LOW]

AfxHookSource2 embeds a JS engine (Boa) with `mirv.onRecordStart({takeFolder})` / `onRecordEnd` events and `mirv.exec`. It is a possible future replacement for console-marker parsing, but needs a script-load command we did not verify in 0.41.7 and adds a moving part to every HLAE update. Console markers + job events are enough.

## CS2 facilities

### Launch options

Recommended `-cmdLine` (all [HIGH] unless marked):

```
-steam -insecure -novid -allow_third_party_software -console -condebug
-netconport <free port> -afxFixNetCon -afxDisableSteamStorage
-windowed -w <W> -h <H> +demo_ui_mode 0 +playdemo "<absolute path, forward slashes>"
```

| Option | Why | Conf. |
|--------|-----|-------|
| `-condebug` | Mirrors the console into `game/csgo/console.log`. **Already observed working on this machine** (4 MB log, 29,781 lines, with HLAE and `mirv_cmd` output). | HIGH |
| `-conclearlog` | Community-documented (cs2-chat-translator / chat-strike READMEs) to truncate `console.log` at start; not verified here. Do **not** rely on it: record the file length at launch and tail from that offset; if `len < offset` restart from 0. | LOW |
| `-netconport N` + `-afxFixNetCon` | TCP text console on 127.0.0.1. Per-launch free port (already done). See Valve issue #3603. | HIGH/MEDIUM |
| `-console` | Lets the author open the console by hand when a run stalls; harmless. | HIGH |
| `+playdemo "<abs path>"` | **Absolute paths work**: `console.log` shows `[Demo] Requesting playback of 'C:/Users/kamal/Downloads/1-...dem'` followed by `Host activate: Playing Demo` and `LOADINGSCREEN -> INGAME`. The current code copies every demo into `game/csgo/` (`install_files`/`write_demo`) and never deletes it: **six leaked copies, about 1.9 GB, in `game/csgo` right now, on a C: drive with 22 GB free of 465**. Stop copying (or delete after the run). Quoting through HLAE's `-cmdLine` needs one spike (spike 4). | HIGH that absolute path plays / MEDIUM on quoting |
| `+sv_lan 1` | Present in the author's manual test line; suppresses nothing proven. Skip. | LOW |

### Console log format and markers [HIGH - observed]

- Lines look like `MM/DD HH:MM:SS <text>`; multi-line messages have continuation lines without a timestamp; **first line starts with a UTF-8 BOM** - strip it. Netcon lines arrive without the timestamp. Parse both with the same function.
- Observed markers (use as phase-completion evidence, each with its own timeout):

| Phase | Evidence line (substring) |
|-------|---------------------------|
| Game process args applied | `[CommandLine] command line arguments:` and the next line (verify `-insecure`, `-netconport`, `-condebug` are present) |
| Engine up | `Source2Init OK` |
| Hook loaded | `AfxHookSource2 (` banner |
| Demo accepted | `[Demo] Requesting playback of '` / `[Demo] playing demo from '` |
| Demo active | `[HostStateManager] Host activate: Playing Demo (` |
| Demo loaded (UI) | `ChangeGameUIState: CSGO_GAME_UI_STATE_LOADINGSCREEN -> CSGO_GAME_UI_STATE_INGAME` |
| Demo ticking + HLAE alive | our own `CS2C <nonce> READY` via `mirv_cmd addAtTick 64 "echo ..."` (existing idea; keep) |
| cfg not found | `[InputService] exec: couldn't exec '{*}cfg/<name>.cfg', unable to read file` (**observed** for a missing cfg) -> fail fast "cfg did not install" |
| Graceful end | `HostStateRequest::Start(HSR_QUIT)`, `Source2Shutdown`, `ShutdownSource2Logging` |

- Failure of `playdemo` itself (missing/corrupt demo) has no sample in the local log; detect as "no `Host activate: Playing Demo` within the load timeout", report the last 20 log lines. [MEDIUM]

### Our own markers (change in `crates/renderer` cfg generation)

Make them anchored, nonce-stamped and unique per event:

```
echo CS2C <nonce> READY
echo CS2C <nonce> TAKE_START <n>      (right after mirv_streams record start)
echo CS2C <nonce> TAKE_END <n>        (right after mirv_streams record end; replaces CS2C_TAKE_DONE)
echo CS2C <nonce> ALL_DONE
```

- Parse by stripping an optional `MM/DD HH:MM:SS ` prefix and requiring the **whole remaining line** to be `CS2C <nonce> <KIND> [n]`. That rejects the `mirv_cmd` trace line (`... -> echo CS2C ...`) and the netcon echo of our own command, without the current "contains echo / addAtTick" blacklist.
- The nonce (random per launch) rejects stale lines left in an appended `console.log`.
- No `:` and no quotes in markers (the renderer already documents that the console splits on `:` and `mirv_cmd` drops quotes).
- Map shot n -> take dir by pairing `TAKE_START n` with the next `Recording to "<dir>"` line.

### Version facts for compatibility preflight [HIGH - observed]

- `game/csgo/steam.inf`: `PatchVersion=1.41.9.0`, `ClientVersion=2000930`, `VersionDate=Oct 08 2026`.
- `steamapps/appmanifest_730.acf`: `StateFlags 4` (fully installed and idle), `buildid 25815307`, `LastUpdated`.
- HLAE on this machine: 2.192.7 / AfxHookSource2 0.41.7; its changelog's most recent `Adjusted to CS2 update` is `1.41.8.5` (0.41.5), i.e. older than the installed patch `1.41.9.0`. A static check can only **warn**; the authoritative test is dynamic (hook banner present, no `AFXERROR`/pattern error/`Error - AfxHookSource2` dialog during startup).

## Windows process-control design (what the stack must provide)

```
cs2cinema (worker thread, one recording at a time, File::try_lock guard)
  GameSession::start()
    CreateJobObjectW + SetInformationJobObject(KILL_ON_JOB_CLOSE) + completion port
    Command(HLAE.exe ...).spawn()  ->  AssignProcessToJobObject(job, child handle)
  threads -> one crossbeam Sender<Event>
    [job-events]  GetQueuedCompletionStatus: NEW_PROCESS(pid) EXIT_PROCESS(pid)
                  ABNORMAL_EXIT_PROCESS(pid) ACTIVE_PROCESS_ZERO
    [netcon]      TcpStream lines (+ set_read_timeout, reconnect while no READY)
    [console.log] offset-based tail (BOM strip, truncation handling)
    [dialogs]     1 Hz EnumWindows over job PIDs: visible #32770 or any HLAE.exe window
    [exit-watch]  per tracked PID: WaitForMultipleObjects([proc, stop_event])
  orchestrator: loop { recv_timeout(min(100 ms, remaining)); check Progress::cancelled(); advance state machine }
  stop(): TerminateJobObject -> wait ActiveProcesses == 0 (<= 10 s) -> verify no stray cs2.exe -> drop job (safety net)
```

| Need | Primitive | Notes |
|------|-----------|-------|
| Own only our game | `AssignProcessToJobObject` on the `HLAE.exe` child immediately after `spawn()`. Race window (before HLAE spawns cs2) is hundreds of ms because HLAE is a .NET WinForms app; `process-wrap` closes it with `CREATE_SUSPENDED` + thread resume, which std cannot do. Cover the residual risk with a check: when cs2 appears, `IsProcessInJob(cs2, job)`; if false, assign it explicitly and log "adopted". | HIGH design / MEDIUM race analysis |
| Liveness without polling | IOCP messages `JOB_OBJECT_MSG_NEW_PROCESS` / `EXIT_PROCESS` / `ABNORMAL_EXIT_PROCESS` / `ACTIVE_PROCESS_ZERO` (PID is delivered in the overlapped pointer). `ABNORMAL_EXIT_PROCESS` = process died with an exception code = **crash**, distinct from a clean `quit`. Microsoft notes delivery is not strictly guaranteed, so also re-check `JobObjectBasicAccountingInformation.ActiveProcesses` on timeouts. | HIGH |
| Which PID is cs2 / ffmpeg | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE)` + `QueryFullProcessImageNameW`, compare to the expected `cs2.exe` path; `ffmpeg.exe` = anything else in the job. Keep the cs2 handle: gives exit code via `GetExitCodeProcess` *after* a signalled wait (never compare against 259 for liveness). | HIGH |
| Cancel | `TerminateJobObject` (kills game, HLAE, take ffmpeg in one call); `KILL_ON_JOB_CLOSE` covers cs2cinema crash or task-manager kill. Then mark current take dir `incomplete` in the manifest. | HIGH |
| Already-running check (preflight) | `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` + `Process32FirstW/NextW` for `cs2.exe` / `steam.exe` (30 lines). Report PID, image path, start time. | HIGH |
| Error dialogs | `EnumWindows` -> `GetWindowThreadProcessId` -> `IsWindowVisible` -> `GetClassNameW == "#32770"` (standard MessageBox class) or any visible window of `HLAE.exe` (it has no window of its own with `-noGui`); read text via `GetWindowTextW` on the title and `EnumChildWindows` static controls. | HIGH logic from source / MEDIUM implementation |
| Free disk space | `GetDiskFreeSpaceExW` on the output drive and on the CS2/game drive. | HIGH |
| HLAE version | `GetFileVersionInfoW` + `VerQueryValueW` on `HLAE.exe` (CSDM reads the same `VS_FIXEDFILEINFO`); first 3 components map to GitHub tags (`2.192.7`). | MEDIUM |

Isolate all of this in one module/crate with `#![cfg(windows)]` and a portable `trait GameHost` + `FakeHost` for Linux CI and for the `pipeline` scenarios the milestone asks for. The workspace currently has no `unsafe` outside `crates/demo/build.rs`, so keep it to one small, heavily commented file.

## Timeout and stall budget (starting values - tune during the 5-match acceptance run) [MEDIUM]

| Phase | Start condition -> end evidence | Budget | On expiry |
|-------|----------------------------------|--------|-----------|
| HLAE launcher | spawn -> HLAE.exe exit code 0 | 30 s | If alive with a visible window: report dialog text. Exit 1 = loader error. |
| Game process | HLAE spawn -> `cs2.exe` in job | 20 s after HLAE exit | "game did not start" (+ HLAE exit code) |
| Engine/hook | cs2 up -> `Source2Init OK` and hook banner | 90 s | "hook not loaded / HLAE incompatible with this CS2 build" |
| Console channel | cs2 up -> netcon connected (or marker read from `console.log`) | 60 s | degrade to `console.log` only, then fail if no command channel at all |
| Demo load | `exec`ed `+playdemo` -> `Host activate: Playing Demo` and `CS2C READY` | `max(load_wait, 150 s)` (existing `LOAD_CAP`) | "demo did not load" + last 20 console lines |
| cfg start | `exec highlights` -> `TAKE_START 0` | 30 s, 3 retries (existing `EXEC_RETRY`) | "cfg did not execute" (also on `couldn't exec`) |
| Each take | `TAKE_START n` -> `TAKE_END n` | `max(60 s, expected_secs * k)` where k comes from measured render speed; plus a **stall watchdog**: no marker and no growth of `take/video.mp4` for 45 s | "take n stalled/timeout" with elapsed vs expected |
| Take finalise | `TAKE_END n` -> ffmpeg child exit (job) -> exclusive-open OK -> `ffprobe` OK | 60 s | "take n incomplete" |
| Shutdown | `quit` -> `ACTIVE_PROCESS_ZERO` | 20 s | `TerminateJobObject` |

Stall detection must be liveness-based (progress signals), never a wall-clock guess for the whole recording; take length depends on GPU speed.

## Preflight checklist -> API mapping

| Check | How | Fails with |
|-------|-----|-----------|
| Steam running | ToolHelp scan for `steam.exe` (CSDM `assertSteamIsRunning` does the same) | "Запустите Steam" |
| No `cs2.exe` already running | ToolHelp scan (we never kill an unowned process) | PID + path + "закройте игру" |
| HLAE paths | `HLAE.exe`, `x64\AfxHookSource2.dll`, `ffmpeg\ffmpeg.ini` -> `Path=` exists | which file is missing |
| HLAE's ffmpeg runs | spawn `<ini Path> -version` via `shared_child` with 5 s timeout; NVENC probe as today | the **observed** `CreateProcessW` class of failure caught before launching CS2 |
| CS2 installed and idle | `steamlocate` -> `state_flags`; `steam.inf` readable; `cs2.exe` exists | "CS2 обновляется" |
| HLAE vs CS2 compatibility (soft) | `PatchVersion` (steam.inf) vs last `Adjusted to CS2 update (x)` in `AfxHookSource2_changelog.xml`; optional online check of GitHub releases via existing `ureq` (60 unauthenticated req/h) | warning, never blocks |
| `ffmpeg`/`ffprobe` | `which` -> absolute path, `-version` parse >= 7 | version/path shown |
| Disk | `GetDiskFreeSpaceExW`: output drive >= estimate (takes) + headroom; game drive >= small margin | numbers in message |
| Out dir has no spaces | existing `console_safe` check | existing message |
| Single recorder | `File::try_lock` on `%LOCALAPPDATA%\cs2cinema\record.lock` | "запись уже идёт (другой экземпляр)" |

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| Own Job Object via `windows-sys` | `process-wrap` 10.0.1 (`JobObject` wrapper, successor of `command-group`; uses `windows` 0.62.2) | If you only need "kill the tree on drop" and a blocking `wait()`. Its `JobObjectChild` hides the job handle, `wait()` cannot take a timeout, `try_wait` reports only the HLAE.exe launcher (not job emptiness), and `wait`/`kill` need `&mut self` - so no PID list, no ffmpeg-child awareness, no cancel from another thread. Its `CREATE_SUSPENDED` + resume trick is worth copying. |
| `crossbeam-channel` bus + slices of `Progress::cancelled()` | Add a cancel `Receiver` to `Progress` and `select!` on it | If sub-100 ms cancel ever matters. Costs a trait change touching CLI and tests. |
| `windows-sys` 0.61.2 | `windows` 0.62.2 | If you prefer RAII `Owned<HANDLE>` and `Result`s; heavier compile, and mixing with `windows-core` 0.100 is a trap right now. |
| ToolHelp scan in `windows-sys` | `sysinfo` 0.39.6 | If you want cross-platform process enumeration (not needed - Windows only) or CPU/mem stats. It is large, cannot wait on handles, and its process list is a snapshot - the opposite of event-driven. |
| `shared_child` | `wait-timeout` 0.2.1 (last release Feb 2025), plain `try_wait` loop | `wait-timeout` is fine for a single-threaded "run and timeout"; it lacks cross-thread `kill`. |
| `steamlocate` | Current hand-rolled `libraryfolders.vdf` scan; `winreg` 0.56 for `HKCU\Software\Valve\Steam` | Keep the scan as fallback when `find_app(730)` returns `None`. |
| Console markers + job events | HLAE JS (`mirv.onRecordStart/End`) | Only if console parsing proves unreliable; ties us to HLAE's scripting API. |
| `thiserror` typed errors | `anyhow` | `anyhow` loses the variant tests need to assert; use only at binary edges. |
| Hand-written fakes + `fake-game` binary | `mockall` 0.15.0 | Large trait surface with many call-order expectations; not the case here. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `taskkill /IM cs2.exe /F` | Kills any `cs2.exe`, including the user's own session or another tool's; PROJECT.md explicitly forbids it. Also leaves HLAE's ffmpeg child orphaned. | `TerminateJobObject` on the job we created |
| `tasklist /FI "IMAGENAME eq cs2.exe"` polling (current `game_alive`) | Spawns a process per second, parses localized text, and on error returns `true` ("alive") so a failing poll means waiting forever. Also cannot say *which* cs2. | IOCP job events + `WaitForSingleObject` on the owned handle; an error is a failure, never "alive" |
| Waiting on / killing the `Child` of `HLAE.exe` | HLAE.exe is a launcher that exits in seconds; `Child::kill` on it kills nothing useful. | Job Object |
| Clip discovery by folder mtime + "quiet for N seconds" (current `wait_for_clips`) | Passes on a half-written file after any 20 s pause, never notices an `AFXERROR` storm, depends on clock/ordering. | `Recording to "<dir>"` + `TAKE_START/END` markers + ffmpeg-child exit + `ffprobe` |
| Trusting HLAE's `Finishing recording ... done.` as "file complete" | Capture shutdown is queued on the render thread (verified in source). | ffmpeg-child exit (job) and `ffprobe` |
| `contains("CS2C_...") && !contains("echo")` marker matching | Breaks as soon as a trace format changes; matches stale log lines. | Anchored, nonce-stamped parser |
| Fixed `-netconport 29700` fallback | Can hit a dead socket from a previous game (the code already notes this). | Per-launch free port; fail if none |
| `-netconport` without `-afxFixNetCon` | Valve bug #3603: Winsock not initialised in time on Windows. | Pass `-afxFixNetCon` |
| Blocking `TcpStream` reads with no read timeout; unbounded `sleep(1s)` loops | Cancel latency and invisible hangs. | `set_read_timeout` + reader threads + `recv_timeout` slices |
| Copying the demo into `game/csgo` and never deleting it | 1.9 GB leaked locally; fills a 22 GB-free C:. | Absolute `+playdemo` path (or delete after run) |
| `Result<_, String>` for process errors | Cannot test "not started vs timeout vs cancel" structurally. | `thiserror` enum with `Stage` + cause, rendered to Russian at the UI edge |
| `WMIC` for process queries | Deprecated and removed from current Windows 11 builds. | ToolHelp / Job APIs |
| `command-group` 5.0.1 | Last release Nov 2023; superseded by `process-wrap`. | Own job module (or `process-wrap` if only kill-tree is needed) |
| `duct` 1.1.2 | Shell-pipeline sugar; kills only the direct child and hides handles. | `shared_child` for one-shots |
| `tokio` / `async-process` | Violates the project constraint (`std::thread`, no tokio) and buys nothing here. | Threads + crossbeam |
| `fs2` 0.4.3 / `fs4` | Unneeded since std `File::try_lock` (1.89). | std |

## Stack Patterns by Variant

**If the job-inheritance spike fails (cs2.exe not in the job):**
- Poll ToolHelp for `cs2.exe` whose image path equals the expected path and whose creation time is after launch, `OpenProcess`, `AssignProcessToJobObject` it (allowed unless it is already in a non-nestable job).
- Because: Microsoft documents breakaway as the only way out of an inherited job; HLAE's flags say it should not happen, but a third-party launcher/AV can change it.

**If `-afxFixNetCon` does not make netcon reliable:**
- Make `console.log` (offset tail) the *only* evidence stream and use a boot cfg (`+exec cs2c_boot` containing `mirv_cmd addAtTick 64 "exec highlights"`) instead of netcon for sending the start command; keep netcon as optional. [LOW - needs its own spike]
- Because: marker detection does not need netcon at all, only command injection does.

**If `console.log` is buffered and markers lag by seconds:**
- Keep netcon as the real-time marker source; use `console.log` for post-mortem and pre-netcon phases only. Because markers carry nonces, duplicates from two streams are idempotent.

**If a take stalls but processes are alive:**
- Watchdog on take-dir growth + marker age; on stall, `TerminateJobObject`, mark take `incomplete`, retry from record stage (manifest tells which takes already pass `ffprobe`, so only missing ones need re-recording if the cfg can be regenerated per subset - roadmap decision).

## Installation

```toml
# crates/<host crate or app>/Cargo.toml
[dependencies]
crossbeam-channel = "0.5.17"
shared_child = "1.1.2"
thiserror = "2.0"
which = "8.0"
steamlocate = "2.1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
tracing-appender = "0.2"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
  "Win32_Foundation",
  "Win32_Security",
  "Win32_Storage_FileSystem",          # GetDiskFreeSpaceExW, GetFileVersionInfoW
  "Win32_System_Diagnostics_ToolHelp", # process snapshot
  "Win32_System_IO",                   # completion port
  "Win32_System_JobObjects",
  "Win32_System_Threading",            # OpenProcess, WaitFor*, QueryFullProcessImageNameW
  "Win32_UI_WindowsAndMessaging",      # EnumWindows, GetWindowTextW, GetClassNameW
] }

[dev-dependencies]
tempfile = "3"
```

No `cargo add` for tokio, sysinfo, process-wrap, duct, mockall.

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| `windows-sys` 0.61.2 | `shared_child` 1.1.2 (`^0.61`), eframe stack (already resolves 0.52/0.59/0.61 side by side) | Do not add `windows` 0.62.x unless something forces it; `process-wrap` 10.0.1 would. |
| `crossbeam-channel` 0.5.17 | std threads, Rust 1.99 | Already locked. |
| `thiserror` 2.0.21 | existing 1.0.69 pulled by other crates | Both coexist in the lock; ours is 2.x. |
| `rusqlite` 0.32.1 (existing; latest 0.40.2) | `store` crate | No need to upgrade for this milestone. Add retry/stage columns with `PRAGMA user_version` migrations. |
| `File::try_lock` | Rust >= 1.89 | Toolchain is 1.99, edition 2024. |
| HLAE 2.192.7 / AfxHookSource2 0.41.7 | CS2 patch 1.41.9.0 (installed) | Hook is newer than the last `Adjusted to CS2 update` entry (1.41.8.5) - treat as unverified until the dynamic startup check passes. HLAE ships updates within days of CS2 patches (0.41.3-0.41.7 in 15 days), so compatibility drift is a recurring, not one-off, failure source. |
| ffmpeg | ffmpeg 7+ on PATH (assembly); HLAE's own ffmpeg via `ffmpeg.ini` (HLAE installer now ships 9.0.2-17) | Two different ffmpegs; preflight both. |

## Open spikes (resolve in the first phase before relying on them)

1. **Job inheritance**: launch HLAE.exe in a job, confirm `cs2.exe` and HLAE's `ffmpeg.exe` appear in `JobObjectBasicProcessIdList` and IOCP `NEW_PROCESS` events fire; confirm `TerminateJobObject` kills all three. [expected yes; HIGH by source]
2. **`-afxFixNetCon`**: run 20 launches with and without it, count netcon-open successes and time-to-open.
3. **`console.log` behavior**: write latency/flush behavior under load, share mode while open (can a second process read it), what `-conclearlog` really does.
4. **Absolute `+playdemo` through HLAE `-cmdLine`**: paths with spaces/Cyrillic, quoting via `Command::arg`.
5. **Render speed**: measure real-time ratio on the author's GPU at the planned fps to set take timeout factor `k`.
6. **Dialog sniffer**: trigger `Error - AfxHookSource2` (launch without `-insecure` in a throwaway test) and confirm the window is visible, class `#32770`, text readable.
7. **HLAE ffmpeg failure fast-fail**: reproduce the observed `CreateProcessW` AFXERROR (bad `ffmpeg.ini`) and confirm first-line detection beats the stall watchdog.

## Sources

- HLAE source, advancedfx `main` (read directly): `hlae/Program.cs`, `hlae/Loader.cs`, `hlae/LaunchCs2.cs`, `AfxHookSource2/main.cpp` (`-insecure`, `-afxFixNetCon`, `-afxDisableSteamStorage`, `HostStateRequest::Start` hook), `AfxHookSource2/Globals.cpp` (`ErrorBox` title), `AfxHookSource2/RenderSystemDX11Hooks.cpp` (`CAfxStreams::RecordStart/RecordEnd`, `EndCapture`), `shared/AfxOutStreams.cpp` (HLAE ffmpeg spawn/close), `shared/FileTools.cpp` (`SuggestTakePath`) - https://github.com/advancedfx/advancedfx - HIGH
- HLAE releases API (v2.192.7 published 2026-10-08, AfxHookSource2 0.25.0 release note for `-afxFixNetCon`) - https://api.github.com/repos/advancedfx/advancedfx/releases - HIGH
- Valve issue on `-netconport` / `WSANOTINITIALISED` - https://github.com/ValveSoftware/csgo-osx-linux/issues/3603 - HIGH on the diagnosis, issue still open
- CS Demo Manager source (prior art for HLAE launch flags `-noGui -autoStart -noConfig -afxDisableSteamStorage`, `Error - AfxHookSource` window detection, HLAE exit-code handling, Steam-running assertion, exe version parsing) - https://github.com/akiver/cs-demo-manager (`src/node/counter-strike/launcher/*`, `src/node/os/*`) - HIGH
- Local evidence on the author's machine (2026-10-09/10): `...\game\csgo\console.log` (banner, `Recording to`, `AFXERROR` storm, demo load and quit markers, absolute-path `playdemo`, `couldn't exec` line), `steam.inf`, `appmanifest_730.acf`, `C:\Program Files (x86)\HLAE\AfxHookSource2_changelog.xml`, `ffmpeg\ffmpeg.ini`, leaked `cs2cinema_*.dem` copies - HIGH
- Microsoft Learn, Job Objects (child inheritance, `BREAKAWAY` flags, nested jobs on Windows 8+, `KILL_ON_JOB_CLOSE`, completion-port notifications) - https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects - HIGH
- docs.rs `windows-sys` 0.61.2 (existence of every function listed under Installation features) - https://docs.rs/windows-sys/0.61.2/ - HIGH
- crates.io API (versions and dates as of 2026-10-10): windows 0.62.2, windows-sys 0.61.2, windows-core/link 0.100.0, crossbeam-channel 0.5.17, shared_child 1.1.2, process-wrap 10.0.1, command-group 5.0.1, sysinfo 0.39.6, duct 1.1.2, wait-timeout 0.2.1, thiserror 2.0.21, tracing 0.1.44, tracing-subscriber 0.3.23, tracing-appender 0.2.5, which 8.0.6, steamlocate 2.1.1, mockall 0.15.0, tempfile 3.27.0 - https://crates.io - HIGH
- docs.rs `shared_child` 1.1.2 (`wait_timeout`, `wait_deadline`, cross-thread `kill`) and `process-wrap` 10.0.1 + its `src/std/job_object.rs` (behavior of `JobObjectChild`) - https://docs.rs/shared_child , https://github.com/watchexec/process-wrap - HIGH
- docs.rs `steamlocate` 2.1.1 (`SteamDir::locate`, `find_app`, `App` fields) - MEDIUM
- Rust std docs, `File::try_lock` stable since 1.89.0 - https://doc.rust-lang.org/std/fs/struct.File.html - HIGH
- Community READMEs for `-condebug`/`-conclearlog` (handle1337/chat-strike, MeckeDev/cs2-chat-translator via web search) - MEDIUM for `-condebug` (also observed locally), LOW for `-conclearlog`
- Valve Developer Community pages returned HTTP 403, so no official launch-option reference was available; launch-option claims rest on HLAE source, CSDM, and the local console log.

---
*Stack research for: reliable CS2 recording via HLAE from a Rust/Windows desktop app*
*Researched: 2026-10-10*
