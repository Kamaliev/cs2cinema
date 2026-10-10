# Pitfalls Research

**Domain:** Automated CS2 demo-to-montage recording (HLAE + CS2 + ffmpeg) driven by a Rust desktop app on Windows. Milestone: "Recording reliability".
**Researched:** 2026-10-10
**Overall confidence:** MEDIUM-HIGH. Code-level findings are HIGH (read directly from `launch.rs` / `jobs.rs`). HLAE release and compatibility facts are HIGH (GitHub releases/wiki). Windows API facts are HIGH (Microsoft docs, Rust std docs). CS2-internal behaviour (what the game prints, how it reacts to Steam state, overlays, locked sessions) is MEDIUM or LOW because Valve does not document it; those items are tagged and must be verified by a short spike on the author's machine before they become hard failures.

Confidence tags used below: **[HIGH]** verified in this repo's code or official docs, **[MEDIUM]** community knowledge or reasoned inference from documented behaviour, **[LOW]** plausible but unverified, treat as a hypothesis to confirm with a spike.

How to read the "Phase" lines: the roadmap does not exist yet, so phases are named by theme. Suggested order (see the mapping table at the end): **Seams** (trait-wrapped processes + `pipeline` extraction + structured stage errors) -> **Preflight** -> **Session** (launch/own/watch the game) -> **Takes** (discovery + validation) -> **Cancel/Teardown** -> **Assemble** -> **Resume** (retry/crash recovery) -> **Soak** (unattended 5-match acceptance).

---

## Critical Pitfalls

### Pitfall 1: Liveness check that fails open (and, in another branch, fails closed)

**What goes wrong:**
`game_alive()` returns `true` when `tasklist` cannot be spawned (`Err(_) => true`). But `Command::output()` returns `Ok` even when `tasklist` runs and *fails* (non-zero exit, policy block, localized "ERROR: ..." text on stderr, empty stdout). The code never looks at `status`, so that case is `contains("cs2.exe") == false`, i.e. "the game is dead". The same function is therefore wrong in both directions: a spawn failure waits forever, a tool failure aborts a healthy recording with "game closed, last clip never appeared". Also, it answers "is *any* `cs2.exe` running", not "is *our* `cs2.exe` running", so a user's own CS2 keeps the wait loop alive after our game crashed. **[HIGH, code]**

**Why it happens:**
Polling a CLI is the quickest way to ask "is it running", and the error branch was written to avoid false aborts. `Output.status` is easy to ignore. Localized Windows prints the "no tasks" message in the system language, so people match on the happy-path string only.

**Consequences:**
`wait_for_clips` has no overall deadline: when the take directory never appears and `alive()` is true, the loop runs forever (only user cancellation ends it). This is the "job just waits" symptom from the milestone brief. **[HIGH, code: the `else if !alive()` branch is the only exit when the dir is absent]**

**How to avoid:**
- Track the game by **handle/PID**, not by image name: `OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION)` once the PID is known, then `WaitForSingleObject(h, 0)` / `GetExitCodeProcess`. No subprocess, no parsing, no localization.
- Model liveness as a tri-state `Alive | Dead | Unknown`. `Unknown` is bounded (for example 3 consecutive unknowns -> fail with "cannot determine whether CS2 is running"), never silently mapped to `Alive`.
- Put the process query behind the planned trait (`GameProcess::is_running(pid)`), with a fake that can return `Unknown` for tests.
- Give *every* wait loop a deadline owned by the stage, not by the callee (see Pitfall 6).

**Warning signs:**
Jobs sitting in "Recording" with no new log line for many minutes; `tasklist` visible in Process Explorer as a short-lived child every second; the same job succeeds with the user's own CS2 closed and hangs with it open.

**Phase to address:** Seams (trait + tri-state), Session (handle-based tracking), Takes (deadline in wait loop).

---

### Pitfall 2: Killing "cs2.exe" by image name, and not killing the game on non-cancel failures

**What goes wrong:**
`kill_game()` runs `taskkill /IM cs2.exe /F`, which terminates every CS2 on the machine, including the user's live match. Conversely, `kill_game()` is only called when the error equals `CANCELLED` (`jobs.rs:163`). On timeout/failure paths the code sends a best-effort console `quit` and returns, so a hung or frozen CS2 stays alive, then blocks the next queued job ("CS2 already running" from Steam, or from our own preflight). **[HIGH, code]**

**Why it happens:**
The launcher process (`HLAE.exe`) exits right after starting `cs2.exe`, so the `Child` handle returned by `Command::spawn` is useless as a game handle, and image-name tools are the easy replacement. Rust's `Child` does not kill on drop, so forgetting teardown is silent. **[HIGH, Rust std docs: dropping a `Child` leaves the process running]**

**How to avoid:**
- Establish ownership once, at start: preflight guarantees **zero** `cs2.exe` processes; after spawning HLAE, poll for the new `cs2.exe` and bind its PID. Verify it is ours (`-netconport <our port>` in its command line, via a process-listing crate such as `sysinfo`, same user) and record `PID + creation time` (PID reuse protection).
- Put HLAE.exe and therefore its child cs2.exe into a **Windows Job Object** with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Children created with `CreateProcess` inherit the job by default, so closing the job handle (including when the app itself crashes) terminates the game. Assign right after spawn; HLAE spends noticeable time before it creates cs2, so the race is small, but verify empirically and fall back to PID-diff + `TerminateProcess(handle)`. **[HIGH for Job Object semantics (Microsoft docs); MEDIUM that HLAE's CreateProcess call does not use breakaway, verify in spike]**
- A `GameSession` RAII guard: `Drop` does console `quit` -> bounded wait (5 s) -> `TerminateProcess` on the owned handle -> wait for exit. Every exit path (success, error, cancel, panic in the worker) goes through it.
- Never call `taskkill /IM`. If a foreign `cs2.exe` exists, refuse to start and tell the user to close it.

**Warning signs:**
Next job fails at start immediately after a failed job; Steam shows CS2 as "running" with no window; user reports their match closed when they cancelled a montage.

**Phase to address:** Session (ownership, job object, guard), Cancel/Teardown (all paths), Preflight (foreign-instance refusal).

---

### Pitfall 3: Preflight that checks file existence but not the facts that actually break the launch

**What goes wrong:**
`check_paths` verifies three files exist. The documented and observed failure causes are different: Steam not running or not logged in, CS2 already running (or a zombie `cs2.exe` from a previous session, which makes Steam silently refuse a second copy), CS2 mid-update, HLAE older than the installed CS2 build, `ffmpeg`/`ffprobe` missing, wrong ffmpeg inside HLAE's own `ffmpeg` folder, unusable output paths, no disk space. Several of these only show up as "game did not start" 150 seconds later. **[HIGH that checks are missing; MEDIUM for the Steam-side specifics]**

**Why it happens:**
Existence checks are cheap. The real checks need Steam state, process enumeration, and a compatibility judgment about two moving third-party products.

**How to avoid (concrete preflight list, each with its own error text):**
1. **No foreign CS2:** enumerate processes (not `tasklist`) for `cs2.exe`; any hit -> block, do not kill. Also treat leftovers from a previous crashed app run specially (Pitfall 14).
2. **Steam running and logged in:** `steam.exe` process present; Steam keeps `HKCU\Software\Valve\Steam\ActiveProcess` (`pid`, `ActiveUser`), where `ActiveUser == 0` means no logged-in user. **[MEDIUM, widely used but undocumented by Valve; verify on this machine]** Block with "Start Steam and sign in" before anything else. Offline mode is acceptable (HLAE's README even recommends it to freeze updates).
3. **CS2 fully installed, not updating:** read `steamapps/appmanifest_730.acf` in the library that contains the CS2 dir; `StateFlags == 4` means fully installed, anything else means update pending/in progress. Record `buildid` plus `steam.inf` `ClientVersion`/`PatchVersion` into the job record. **[MEDIUM]**
4. **HLAE vs CS2 compatibility** (Pitfall 4 for the full treatment): version read from `HLAE.exe`/`AfxHookSource2.dll` file version, warn-level check, plus a hard runtime detector.
5. **Tools:** resolve `ffmpeg` and `ffprobe` to absolute paths once (PATH of a tray/autostart-launched GUI app can differ from the terminal), run `-version` with a timeout, and also check HLAE's own `ffmpeg\bin\ffmpeg.exe` (Pitfall 12).
6. **Paths:** the output dir must be ASCII, without spaces/quotes/semicolons (the current `console_safe` check exists but runs after planning; run it first) and not under a Cyrillic user profile. Check `cs2_dir`, `hlae_dir` for non-ASCII as a **warning** (Pitfall 9).
7. **Disk space** on *both* volumes (Pitfall 13).
8. **Window geometry:** requested `-w -h` fits the primary monitor client area (Pitfall 16).
- Return **all** failures at once as a list ("3 problems"), not the first one; the user fixes the environment in one pass.
- Run the same preflight in the UI (a "Check environment" button / status panel) and at job start, and **re-run it immediately before spawning HLAE** in a long queue (Steam can update CS2 between jobs).

**Warning signs:**
"Game does not start" tickets whose root cause turns out to be Steam signed out, a leftover `cs2.exe`, or an update downloading.

**Phase to address:** Preflight (primary). Seams must land first so preflight is testable with fakes.

---

### Pitfall 4: A CS2 update silently breaks AfxHookSource2, and the failure looks like "game does not start"

**What goes wrong:**
HLAE hooks undocumented engine internals. After a CS2 patch the hook may crash the game at startup, hook a subset of features, or run but behave wrongly (broken sniper scopes, broken animations on some demos). HLAE ships a new build after most CS2 updates, but not instantly. **[HIGH]**

Evidence: HLAE releases 2.192.3 (2026-09-23), 2.192.4 (09-24), 2.192.5 (09-26), 2.192.6 (09-26, regression fix), 2.192.7 (10-08) track CS2 updates 1.41.8.2/.3/.5; 2.191.0 (2026-07-10) tracked 1.41.6.8. The wiki "Old HLAE versions for specific CS2 versions" lists four *breaking* CS2 updates in twelve months (1.40.8.8, 1.41.2.9, 1.41.4.1, 1.41.6.7). Steam auto-updates CS2 as soon as Valve ships, so the author's machine will regularly sit in a window where CS2 is newer than the installed HLAE. The README's own advice is to use Steam offline mode to avoid updates that break HLAE during production.

**Why it happens:**
There is no machine-readable compatibility contract. Static version comparison is therefore always a heuristic.

**How to avoid:**
- **Heuristic, warn-level, in preflight:** show HLAE version (file version of `HLAE.exe`/`AfxHookSource2.dll`) next to CS2 `buildid`/`steam.inf` version in Settings and in every job's header. Optionally compare `AfxHookSource2.dll` file date with `client.dll`/`steam.inf` date of CS2: CS2 files newer than the HLAE build by a lot -> "HLAE may be outdated, check releases" with a link. Never hard-fail on the heuristic alone (HLAE often keeps working across minor CS2 updates).
- **Definitive, hard-fail runtime detectors (these are what prove a mismatch):**
  - `cs2.exe` exits within N seconds of start and the netcon port never opened -> "CS2 exited N s after start, before the console opened. Typical cause: HLAE incompatible with CS2 build X (HLAE vX)".
  - Console is up but `mirv_cmd`/`mirv_streams` print an "Unknown command" style line (hook not loaded): send a harmless probe (`mirv_cmd print`) right after connecting and fail fast if the answer shows the command is unknown. **[MEDIUM: exact text must be captured once in a spike]**
- Store `{cs2_build, hlae_version, outcome}` per job so "worked last week, broke after build X" is answerable from the DB.
- Pin the last-known-good HLAE version in README and the settings page, together with the CS2 build it was verified with; a known-good pair is what the 5-match acceptance run should record.
- Offer (documentation only, no automation) the "Steam offline mode / do not update" workflow for production runs.

**Warning signs:**
Sudden 100% failure rate across all jobs on the same day; CS2 window appears and disappears within seconds; Steam's "update queued" for app 730; HLAE GitHub issue titled like "CS2: <date> update semi-broke HLAE".

**Phase to address:** Preflight (heuristic + version display), Session (startup-death and probe detectors), Soak (record the known-good pair).

---

### Pitfall 5: Treating "console port connected" as "game ready", and sending `exec` on a timer

**What goes wrong:**
The netcon socket accepts connections long before the demo has loaded. The code already compensates with a `CS2C_READY` marker via `mirv_cmd addAtTick 64`, which is a good idea, but three weak points remain: **[HIGH, code]**
1. `Console::connect` does not check that the game is alive; if CS2 died at startup the app waits up to `max(load_wait, 150 s)` before reporting "console did not open".
2. After a failed connect the code logs "open the console and run exec highlights" and *continues* into `wait_for_clips`, which (Pitfall 1) can wait indefinitely. The error is downgraded to a log line.
3. `exec highlights` is **re-sent every 30 s** (up to 4 times) if no take directory exists yet. The first take starts at `shot.start_tick`, which can be far into the demo; the first take directory may legitimately not exist for longer than 30 s. Re-running the script resets scheduled commands (`mirv_cmd clear`) and, if it seeks, restarts shot 0, producing duplicate/misnumbered takes (HLAE numbers takes by recording order, so any duplicate shifts every later `takeNNNN` against its shot). **[MEDIUM: depends on exactly what `highlights.cfg` does; verify]**

Also: if `exec` cannot find the file, CS2 prints a "couldn't exec ... unable to read file" line (seen in community reports as `exec: couldn't exec '...cfg', unable to read file`). Nobody is reading for it. **[MEDIUM: pattern from Steam community threads]**

**Why it happens:**
Console output is the only side channel, so code leans on it with ad-hoc retries instead of a state machine with explicit evidence per step.

**How to avoid:**
- Explicit state machine with per-state evidence and deadline: `Spawned -> ProcessSeen (PID) -> ConsoleOpen -> DemoLoaded (CS2C_READY) -> ScriptAccepted -> Take[i] started -> Take[i] done -> Closed`. Each transition logs a line with elapsed time; each timeout names the state ("stuck in DemoLoaded").
- `connect` loop must poll process liveness on every iteration and bail with the startup-death diagnosis.
- Make `exec` **idempotent** or send it once: have the cfg's first command be a marker (`echo CS2C_SCRIPT_START`) so "script was accepted" is observable; retry only if that marker never arrives, and never after it did. Retry on the *marker*, not on the *take directory*.
- Scan console lines for known failure patterns and surface them verbatim in the error: `couldn't exec`, `Unknown command`, demo load failures (capture exact strings from a failing run into a test fixture).
- Optionally add `-condebug` and tail `game/csgo/console.log` as a second source of evidence (survives a dropped netcon socket). **[MEDIUM: verify the flag still works in CS2]**
- Keep the netcon socket open but treat a closed socket as an event ("console connection lost at state X"), not as "no more markers".

**Warning signs:**
Log shows "repeating exec highlights (2/4)" on a healthy long-demo run; duplicate takes; takes count != shots; run only works when the first shot is early in the demo.

**Phase to address:** Session (state machine, probes, idempotent exec), Takes (marker-vs-file cross-check).

---

### Pitfall 6: Timeouts that are per-callee instead of per-stage, and "alive but not progressing"

**What goes wrong:**
Each wait has its own cap, and some have none (`wait_for_clips` when the directory never appears). Also a *live* process is not necessarily *progressing*: a frozen CS2 with a crash dialog (WER/Crashpad), a shader-compile on the first launch after an update, a game stuck in the main menu because `playdemo` failed, all keep `cs2.exe` alive. **[HIGH for missing deadline; MEDIUM for the others]**

**Why it happens:**
"alive" is a cheap boolean; "making progress" needs a definition per stage.

**How to avoid:**
- Define a **progress watchdog** per stage with two clocks: *idle* (no new evidence for T_idle: no console line, no file growth, no CPU time growth of the owned process via `GetProcessTimes`) and *hard cap*. Failing on either yields an error naming the stage, the idle duration, and the **last 10 console lines**.
- Sensible starting values (tune from the first soak run): process start-to-console 60 s idle; demo load 150 s hard but extend while the process burns CPU (first start after an update may compile shaders for minutes); take: `shot_secs * K + slack`, with idle 30 s.
- Detect "alive but wedged" cheaply: `IsHungAppWindow` on the game window or `NOT RESPONDING` state; count as no progress.
- On timeout, take diagnostics before killing: screenshot of the window (optional), tail of console, `steam.inf` build, HLAE version. Put them in the job journal.

**Warning signs:**
Job in a stage for much longer than the plan's recording time; CPU of `cs2.exe` at 0%; Windows Error Reporting dialog visible on the desktop.

**Phase to address:** Session and Takes (watchdog), Seams (clock injected so tests run in milliseconds).

---

### Pitfall 7: Taking `-insecure`, VAC and the "genuine Steam" requirement lightly

**What goes wrong:**
The launch line uses `-insecure -steam ... -allow_third_party_software`. HLAE is technically a hack and its README warns that joining VAC-protected servers with it probably results in a VAC ban; a 2017 community thread reports a ban after launching via the custom loader with `-insecure` (replies argued coincidence, so the evidence is anecdotal; the safe reading is "the risk is real but unproven"). Guides note VAC-secured servers refuse insecure clients by default. **[HIGH for README warning; LOW for the ban anecdote]** HLAE also requires a genuine Steam installation and .NET Framework 4.6.2+.

The failure mode for the product: the app never joins servers, but a hurried "just start CS2 normally to check" UX, or a user double-clicking the same shortcut, mixes insecure and secure sessions. And if CS2 was started normally first, the second start via HLAE is refused or goes to the wrong instance (Pitfall 3).

**How to avoid:**
- Keep the launch line in **one** place, with a comment on why each flag exists, and never expose a "launch CS2" button that skips `-insecure`/HLAE or joins matchmaking.
- Add a console watchdog: if the console ever shows connecting to a server (anything other than the demo), fail the job and close the game.
- Document in the README: "this app only plays demos; do not use this CS2 instance for online play".
- Preflight detects missing .NET Framework 4.6.2+ only if cheap (registry `Release` >= 394802); otherwise rely on the HLAE exit code (Pitfall 8).

**Warning signs:**
Console lines about connecting or VAC; user complaints about being unable to join games right after a run (zombie insecure instance).

**Phase to address:** Preflight (docs + status), Session (single launch-line module, "no server connect" guard).

---

### Pitfall 8: The launcher (`HLAE.exe`) is invisible: dialogs, exit codes and the throwaway `Child`

**What goes wrong:**
`launch()` drops the `Child` (`_launcher`) and never checks whether `HLAE.exe` itself failed. With `-noGui` HLAE can still raise a modal dialog (missing hook DLL, .NET problems, injection failure, antivirus quarantine of `AfxHookSource2.dll`) that nobody sees because the process is hidden or minimised; the job then waits for a console that will never open. Rust does not reap or kill children automatically. **[MEDIUM: specific dialogs unverified; HIGH for the dropped handle]**

**How to avoid:**
- Keep the launcher `Child`, call `try_wait()` during the first seconds; a non-zero exit code or a launcher that stays alive far longer than expected is a distinct, named failure ("HLAE launcher exited with code N" / "HLAE launcher still running after 20 s, probably showing a dialog").
- Capture launcher stdout/stderr (piped, read on threads, bounded) into the journal; HLAE's CLI may print the reason.
- Enumerate top-level windows owned by the launcher PID in the first seconds; any visible window means "dialog" -> include its title/text in the error (optional, `EnumWindows`).
- Verify `AfxHookSource2.dll` after any AV event: preflight compares size/hash with the value recorded when it last worked (Defender false-positive quarantine of injection tools is a classic cause of "it worked yesterday").
- Pass arguments as `Command::args` entries (already done), but test quoting with a path containing spaces and non-ASCII characters against the real HLAE once (HLAE parses `-cmdLine` itself, so Rust's MSVCRT-style quoting must be confirmed).

**Warning signs:**
No `cs2.exe` ever appears; HLAE process remains in Task Manager; launcher window flashes.

**Phase to address:** Session.

---

### Pitfall 9: Paths and names: Cyrillic, spaces, `.dem.zst`, and the one directory CS2 reads from

**What goes wrong:**
- **Demo format:** CS2 plays only uncompressed `.dem` from `game/csgo`; `.dem.zst`/`.gz` are not understood. The pipeline already decompresses into `game/csgo/<name>.dem` (good), but nothing removes it afterward, so every job leaves a 200 MB-1 GB file in the game folder and, if the job dies, a stale one. **[HIGH, code + comment]**
- **Demo name:** `console_name` maps everything non-`[A-Za-z0-9_-]` to `_`. An all-Cyrillic label becomes `______`; two different matches could share a name and a stale copy could be played. Names are only safe because they are sanitised; keep it that way and add a unique suffix (job id). **[HIGH, code]**
- **Cyrillic/spaces in directories:** the author's home is under `C:\Users\<name>`; `find_hlae()` deliberately scans `Downloads`/`Desktop`/`Documents`, which are exactly where non-ASCII folder names occur. "Move HLAE to a path without special characters" is a recurring support answer in the HLAE community; the in-game ffmpeg child, `mirv_streams record name`, and `mirv_campath load` paths all pass through console parsing that splits on `:` and loses quotes (the renderer already documents this; `console_safe` rejects whitespace, quotes, `;`). **[MEDIUM]**
- **Config location:** `exec highlights` resolves from `game/csgo/cfg`; the CS2 layout differs from CS:GO (users report `exec` failing when files are put in the old location or named `x.cfg.cfg`). Campath XML is loaded relative to the game exe folder (`../../csgo/cfg/...`), a convention that only the real game confirms. **[MEDIUM]**

**How to avoid:**
- Preflight: out dir must pass `console_safe` **and** be ASCII; default the out dir to a short ASCII path (for example `C:\cs2cinema\out`), never under the profile; warn (not block) for non-ASCII HLAE/CS2 dirs, and hard-block if HLAE's smoke test fails (Pitfall 8).
- Verify installed files after copy: for each `cs2c_*.cfg/xml`, `highlights.cfg` and the demo, compare size to the source; fail with "file X not copied to game/csgo/cfg" before launch.
- Console detector for `couldn't exec` (Pitfall 5) so "cfg not picked up" is a named failure, not a silent no-op.
- Delete the demo copy and our `cs2c_*` files in teardown (success and failure), guarded by a manifest of what this job installed; do not glob-delete `cfg` contents.
- Use `+playdemo` with the plain sanitised name; if a name ever needs quoting, use the safe subset only.

**Warning signs:**
`exec` prints "couldn't exec"; `game/csgo` contains many `cs2cinema_*.dem`; failures correlated with the demo label or with the user profile directory.

**Phase to address:** Preflight (path rules), Session (install manifest + cleanup), Seams (single `cs2_path` helper replacing the four ad-hoc normalisers noted in CONCERNS).

---

### Pitfall 10: Old demos vs. current CS2 build ("the demo loads but is wrong or does not load")

**What goes wrong:**
FACEIT demos can be weeks or months old. CS2 demos are tied to the game build that produced them; after some updates old demos fail to load, desync, or play with broken animations (HLAE's own list of version-specific builds mentions "broken knife animations in some demos"). The result looks like "demo did not load" or "marker never arrived". **[MEDIUM, no official compatibility statement found]**

**How to avoid:**
- The demo parser already reads the header; store the demo's build/protocol in the job and compare with the installed CS2 `steam.inf` in preflight. Show "demo is N updates older than the installed game; playback may fail" as a **warning**, with the exact console failure line (captured in a spike) mapped to a specific message when it actually happens.
- Distinguish "demo failed to load" (console error, game stays at menu) from "demo loaded, script did not run" through the `CS2C_READY` marker; that is already half done, finish it with the failure-pattern scanner.

**Warning signs:**
Older matches fail while recent ones succeed; the game sits in the main menu; the READY marker never arrives although the console is open.

**Phase to address:** Preflight (warning), Session (distinct failure state), Soak (include one older demo).

---

### Pitfall 11: Take discovery by directory mtime and "quiet for N seconds"

**What goes wrong:**
`wait_for_clips` decides a take is complete when the newest mtime in `clips/takeNNNN` is `>= since` and the (mtime, size) pair has not changed for 3 s (with the finished marker) or 20 s (without). Only the **last** take is examined, and not for validity. **[HIGH, code]** Failure modes:
- HLAE creates the take folder at `record start` and writes continuously; the in-game ffmpeg finalises the mp4 (moov atom, possibly a faststart rewrite) *after* `record end`. On a slow disk, Defender scanning, or a paused game, 3 s of quiet can be a pause in the middle of finalisation, giving a truncated mp4 that passes the mtime test. **[MEDIUM]**
- `since` is `SystemTime::now()` compared to filesystem mtimes: exFAT/FAT drives have 2-s granularity, and clock adjustments (NTP) or antivirus touching files break `t >= since`.
- Files being touched by indexers/AV can keep resetting "quiet" so the 20 s rule never fires.
- A take in the middle of the sequence can be missing/empty (script chain broke, `AFXERROR` writing) while the last one exists; discovery never looks.
- Leftover takes from a previous attempt shift numbering (the code moves old `clips` aside to avoid this, which is good, but see Pitfall 15 for the rename).

**How to avoid:**
- Discovery by **identity, not time**: expected set `take0000..take{N-1}` mapped to shots via the recorded `take_of` table; no `since`, no mtime.
- Completion by **evidence**: (a) console marker `CS2C_TAKE_DONE i` as the "recording ended" signal (progress hint), (b) `ffprobe` of `video.mp4` succeeds (a mp4 without a moov atom fails, which is exactly the "still being written" signal), (c) duration within tolerance of the planned shot length, expected fps/resolution, `nb_frames > 0`, (d) `audio.wav` header consistent: while being written the RIFF/data sizes are not final; check `file size - 44 == data chunk size` and duration within tolerance of the video. **[MEDIUM for (d), derived from the WAV format]**
- Poll validation with backoff until valid or until the take's idle/hard timeout; the error names the take: "take 7 of 12: video.mp4 unreadable (moov atom not found)".
- Validate **every** take before assembly (this is also the Assemble phase's input gate), not only the last, and run this validation again on resume (Pitfall 15).
- A take-set invariant: number of take directories == number of shots, no gaps. A mismatch is a failure with a diff ("expected 12, found 11: missing take0005").

**Warning signs:**
Assembled video with a frozen/short segment; ffmpeg assembly failing at the start with "moov atom not found"; recording "succeeded" in under the planned time.

**Phase to address:** Takes (primary), Assemble (gate), Seams (a `TakeStore` trait with fakes producing partial files).

---

### Pitfall 12: In-game ffmpeg problems that produce empty takes while every marker still arrives

**What goes wrong:**
HLAE streams frames to its own ffmpeg (`hlae\ffmpeg\bin\ffmpeg.exe` according to the HLAE docs; this repo reads `Path=` from `ffmpeg\ffmpeg.ini`). If that ffmpeg is missing/misconfigured, the console fills with `AFXERROR: Failed writing image ...` while `record start/end` and our `echo` markers still work and take folders still appear. The job "records" for the full duration and then fails at assembly with an empty/invalid mp4. **[HIGH for the AFXERROR behaviour (mirv_streams wiki); HIGH that markers are independent of it (cfg structure)]**

Also: NVENC availability is probed with **PATH ffmpeg** (`has_nvenc(ffmpeg)`) while the take encoder is HLAE's ffmpeg; those are different binaries and may have different NVENC support. NVENC "builds in" but fails at runtime with an old driver ("Driver does not support the required nvenc API version"), which `nvenc_check` handles for one binary only. If a driver update or session limit changes mid-queue, the decision made once per job can be stale. **[HIGH, code]**

**How to avoid:**
- Scan console output for `AFXERROR` continuously; the first one fails the stage immediately ("HLAE could not write frames: <next lines>") instead of recording for 10 more minutes.
- Preflight probe: run **HLAE's own** `ffmpeg.exe` through `nvenc_check` and a tiny x264 encode, and use the answer for the take preset. Separate probes for take encoder (HLAE ffmpeg) and assembly encoder (PATH ffmpeg).
- Decide encoder **once per job at preflight**, store it in the job; if the take preset is NVENC and the first take is invalid, the error says "NVENC take failed, re-run with CPU encoder" (and the retry path can switch automatically, see Pitfall 15).
- Verify the first take early: after take 0 is done, `ffprobe` it right away and abort the whole run on a bad first take rather than after all takes. Early validation of take 0 is the cheapest large win in wasted recording time.
- `has_nvenc` uses `Command::output()` with no timeout; a wedged GPU driver would block the preflight forever. Run it with a deadline (Pitfall 17).

**Warning signs:**
`AFXERROR` in the journal; `video.mp4` of 0 or a few KB; recordings that finish "too fast".

**Phase to address:** Preflight (probes), Session (AFXERROR scanner), Takes (first-take early check).

---

### Pitfall 13: Disk space is only noticed when a write fails inside someone else's process

**What goes wrong:**
Space is consumed in at least four places: the demo copy in `game/csgo` (hundreds of MB), the takes in `out_dir/clips` (potentially GBs at high bitrate/ProRes-like presets), `clips_old_*` leftovers from retries (Pitfall 15), and the final video plus ffmpeg temp. On exhaustion, the failure happens inside HLAE's ffmpeg or the game, which prints an error to its console at best; the app sees truncated takes or a silent stall. The game and the output may be on different volumes, so a single free-space check can pass while the other volume fills. **[MEDIUM]**

**How to avoid:**
- Preflight estimate: `sum(shot_secs) * bitrate_of_take_preset * 1.3 + demo_size_unpacked + final_video_estimate + margin`, checked against free space on **each distinct volume** (`GetDiskFreeSpaceExW`, or `sysinfo::Disks`; std has no API). Block with the numbers ("need ~6.4 GB on D:, have 2.1 GB").
- Re-check free space between takes (cheap) and stop with "disk full" before the game starts failing writes.
- Cleanup policy: remove `clips_old_*` older than N days or beyond the last 1-2; demo copies deleted at teardown; keep an eye on the DB-recorded sizes.
- Handle `ERROR_DISK_FULL`/`StorageFull` from our own writes with the same message.

**Warning signs:**
Free space dropping job over job; `clips_old_*` folders accumulating; `AFXERROR` or ffmpeg "No space left on device".

**Phase to address:** Preflight (estimate), Takes (recheck), Resume (cleanup policy).

---

### Pitfall 14: Crash recovery that marks everything "failed", and orphaned games from the previous run

**What goes wrong:**
On startup `reset_running` marks interrupted jobs `failed`. That loses the one useful fact: the stage reached and which takes were already on disk. Worse, if the app crashed while CS2 was recording, the game is an **orphan**: still running, holding the netcon port, still writing takes. The next job's preflight (Pitfall 3) then correctly refuses with "CS2 already running", forever, until the user kills it by hand; or, without that preflight, a second launch fights the first. **[HIGH for `reset_running`, MEDIUM for the orphan scenario]**

**How to avoid:**
- New terminal state `interrupted` (distinct from `failed`/`cancelled`) with `stage_at_interrupt`, `takes_done`, `out_dir`, and the **owned process identity** (`pid`, `creation_time`, `netcon_port`) persisted at spawn time (write to SQLite right after PID bind).
- On startup, reconcile before touching the queue: if a persisted `(pid, creation_time)` still matches a live `cs2.exe`, it is verifiably ours -> offer "close orphaned game" (or do it automatically, since identity is proven), then mark the job `interrupted`. Never decide by name alone. A Job Object with KILL_ON_JOB_CLOSE (Pitfall 2) makes most orphans disappear automatically with the app.
- Re-queue policy: `interrupted` jobs are shown with an explicit "Resume from recording" action (and optionally auto-resume once); journal says "interrupted by app restart during Record, take 4/12 on disk, resuming".
- Write job state transitions in a SQLite transaction before side effects (state "about to launch" persisted before spawning), so a crash between spawn and persist cannot lose ownership.

**Warning signs:**
After a crash/reboot the first job fails with "CS2 already running"; many `failed` jobs with the same "restart" reason; users re-queueing everything from scratch.

**Phase to address:** Resume (state model + reconcile), Session (persist identity).

---

### Pitfall 15: Retry/resume that trusts file existence, shifts take numbering, or collides with locked files

**What goes wrong:**
- The "already recorded" shortcut checks that `video.mp4` and `audio.wav` exist and that `plan.hash` matches. Existence is not validity; a truncated take (Pitfall 11) is accepted as recorded and the failure repeats at assembly forever. `plan.hash` covers only cfg file names/contents, not the demo identity, frame size, encoder, HLAE/CS2 build or the exact take table. **[HIGH, code]**
- HLAE numbers takes by recording order, so recording "only the missing takes" breaks the mapping between `takeNNNN` and shot index unless the cfg chain is regenerated for the subset and the mapping table persisted. The current design (all-or-nothing re-record into a fresh `clips`) is the safe baseline. **[MEDIUM]**
- Retry right after a cancel/failure renames `clips` -> `clips_old_<ts>`. On Windows, `rename` fails with a sharing violation/access denied while the dying game, HLAE's ffmpeg child, Defender, or the Explorer preview still hold handles in that tree. The retry fails immediately with "could not set aside previous takes". **[MEDIUM]**
- Each failed attempt adds another `clips_old_*`, so retries eat disk (Pitfall 13).
- `ui::open_path` after every successful job pops the video player over the desktop **in the middle of a queue**, stealing focus from the next job's CS2 window. **[HIGH, code; MEDIUM on impact]**

**How to avoid:**
- Persist a per-take ledger in SQLite (`job_id, take_idx, shot_id, path, probe_json, validated_at`). "Recorded" means: every ledger row validated by `ffprobe` against the plan, and the ledger's `plan_fingerprint` (shot table + demo id + frame size + encoder + HLAE version + CS2 build) equals the current one.
- Resume policy: valid take set -> skip recording; any invalid or missing take -> re-record **all** into a fresh `clips` dir (or implement subset recording only when the take table is regenerated and tested with the fake recorder).
- Before rename: wait for owned process exit, then retry the rename with backoff (up to ~10 s); fall back to creating a new sibling directory (`clips_run2`) and pointing the plan at it instead of failing.
- Cap/garbage-collect old attempt directories.
- Do not open the result automatically while other jobs are queued; open only when the queue becomes empty or on user click.
- Retry classification: `failed(env)` (needs the user), `failed(transient)` (auto-retry once, for example one startup failure), `failed(content)` (demo/plan). Do not auto-retry env failures.

**Warning signs:**
Retry fails faster than the original attempt with a file-system error; assembly fails identically after a "successful" resume; windows pop up over a recording.

**Phase to address:** Resume (ledger + fingerprint + rename strategy), Takes (validation), Soak (open-path behaviour).

---

### Pitfall 16: Window size, DPI and focus changes the recording without any error

**What goes wrong:**
The game is launched `-windowed -w W -h H`. If W x H is larger than the usable monitor area (title bar and taskbar included) or DPI scaling alters the size, the client area may differ from the request; the takes then have a different resolution than the assembly graph assumes, and `concat` with mismatched sizes/fps/sample rates fails or scales badly. Focus loss (a toast, another window, the auto-opened player) can throttle the game. The generated cfg already contains `engine_no_focus_sleep 20`, which shows this was hit before. **[MEDIUM]**

**How to avoid:**
- After take 0, `ffprobe` width/height/fps/pix_fmt/sample rate and compare to the plan; mismatch -> fail early with numbers.
- The assembly graph normalises every input (`scale`, `setsar=1`, `fps`, `format`, `aresample`) so a harmless 1-pixel difference does not kill the run, but only after the mismatch is logged.
- Preflight check the target frame size against the primary monitor's work area; if larger, warn and propose the largest fitting 16:9 size.
- Document "do not use the PC for fullscreen things during recording" and keep the window unobstructed; no auto-opened player (Pitfall 15).

**Warning signs:**
Takes of unexpected sizes; ffmpeg errors about differing parameters; stutter in a take recorded while the user used the PC.

**Phase to address:** Takes (parameter check), Assemble (normalisation).

---

### Pitfall 17: Blocking `Command::output()` makes cancellation and timeouts a lie

**What goes wrong:**
`Command::output()` blocks until the child exits and gives no cancellation hook. In this codebase it is used for `tasklist` (every second), `taskkill`, `nvenc_check`, plus `git ls-remote` in `geometry`. If any of these wedge (AV scanning, a hung GPU driver, offline git, blocked network), the worker thread is stuck and the "cancel within seconds" requirement fails even though the polling loops check the flag. The ffmpeg runner is already the right pattern (spawn + `try_wait` loop + kill on cancel). **[HIGH, code + CONCERNS]**

**How to avoid:**
- One `run_with_deadline(cmd, deadline, cancelled) -> Result<Output, RunError{Spawn, Timeout, Cancelled, Failed(status)}>` helper: spawn with piped stdout/stderr read on threads (otherwise a full pipe deadlocks the child, as the existing ffmpeg comment already notes), `try_wait` loop at 50-100 ms, kill + wait on timeout or cancel. Use it for **every** external process (ffmpeg, ffprobe, git, nvenc probe); ban raw `.output()` outside it (a clippy `disallowed_methods` rule on `std::process::Command::output` enforces it).
- Prefer in-process APIs to eliminate subprocess polling entirely (process list, disk space, handle waits).
- Cancellation must also reach pre-record stages (download, parse, plan): pass the cancel flag into long-running pipeline calls or run them in a separate cancellable thread and abandon it.
- `Child::kill` only terminates the one process (no descendants). For ffmpeg that is fine; for the game use the job object.
- Fake process runner implements `Timeout`/`Cancelled` so the cancellation test runs in milliseconds.

**Warning signs:**
Cancel button shows "cancelling..." for more than a few seconds; UI worker thread stack shows a wait on process; behaviour differs offline.

**Phase to address:** Seams (helper + trait + lint), Cancel/Teardown (apply everywhere).

---

### Pitfall 18: ffmpeg filter graphs: escaping, command-line length and fonts

**What goes wrong:**
- ffmpeg filter strings have three escaping levels (option value, filtergraph, and shell/batch). `drawtext` text with `'`, `:`, `,`, `;`, `[`, `]`, `\` or `%` breaks the graph; `%{...}` triggers drawtext's text expansion unless `expansion=none`. The ffmpeg manual itself calls this cumbersome and recommends `textfile` (and the `/option` file-loading syntax) instead of inline text. The repo escapes by hand (`'\\' -> '/'`, `':' -> '\\:'`). **[HIGH, ffmpeg docs + CONCERNS]**
- Cyrillic nicknames/titles need a font with those glyphs; `textfile` must be UTF-8 **without BOM** (a BOM renders as a glyph); a missing `fontfile` produces an error only at run time, after recording. The font path itself must have its drive colon escaped (`C\:/Windows/Fonts/arial.ttf`) and is fetched from a hard-coded `C:\Windows\Fonts` in the UI code. **[MEDIUM]**
- Windows `CreateProcess` limits the whole command line to 32,767 characters; a graph for 15-30 shots with overlays can approach it if passed with `-filter_complex "<string>"` instead of from a file. **[MEDIUM]**
- The generated `.bat` is a second consumer with a different quoting regime (`%`, `^`, `&`, non-ASCII under the OEM code page). **[HIGH, CONCERNS]**
- Output file overwrite prompts: with stdin set to null, ffmpeg without `-y` aborts instead of overwriting a previous partial file.
- Writing straight to `highlights.mp4` means a cancelled or crashed run leaves a partial file that looks like a result.

**How to avoid:**
- Move all dynamic text into **files** (`drawtext=textfile=...:expansion=none`) and the whole graph into a script file (`-/filter_complex file` or `-filter_complex_script`), so the only escaped things are file paths, handled by one `escape_filter_path()` function (drive colon, backslashes, quotes).
- Property-style integration test that feeds nasty titles (quotes, colons, commas, `%`, Cyrillic, emoji, very long) to a **real ffmpeg** against `testsrc` clips (the repo already has a no-CS2 recipe for dummy takes) and asserts exit code 0 and a probed duration.
- Resolve the font at preflight (`%WINDIR%\Fonts`, not `C:\`), fall back through a list (Segoe UI, Arial, Tahoma), and test glyph coverage for the actual title text; fail in preflight, not after recording.
- Write to `highlights.part.mp4`, then rename atomically after a successful `ffprobe`; always pass `-y` for the `.part` path; delete `.part` on cancel.
- `.bat`: generate UTF-8 with `chcp 65001`, quote every path, double `%`; it is a convenience, not the product path (the app runs ffmpeg directly through `Command`, which passes UTF-16 arguments).
- Keep one ffmpeg runner module (CONCERNS already lists five call sites).

**Warning signs:**
Assembly fails only for some players' nicknames; "Invalid argument"/"No such filter" with the nickname in the message; boxes/blank glyphs where Cyrillic text should be.

**Phase to address:** Assemble.

---

### Pitfall 19: A/V assembly assumptions about takes

**What goes wrong:**
Takes are `video.mp4` plus separate `audio.wav`. Drift appears when the wav and the video differ by a few frames (sampler/slowmo, frame dropping), when fps differs between takes (the generator records at different fps per shot and slows down with `setpts`), or when `concat` receives mismatched timebases. NVENC output needs correct `pix_fmt` (yuv420p) and the ffmpeg-version-dependent preset names (`p1`-`p7`, `-tune`), which differ between ffmpeg 4.x and 6.x/7.x builds. **[MEDIUM]**

**How to avoid:**
- ffprobe-based validation yields per-take `{fps, frames, duration_v, duration_a}`; assembly uses `atrim`/`apad`/`aresample=async=1` to clamp audio to video length per take, and the validation fails clearly when they differ by more than N frames.
- Check the encoder choices against `ffmpeg -encoders` and the actual probe (Pitfall 12) instead of assuming flags.
- Keep a **golden assembly test** with synthetic takes (different fps, durations, audio offsets) in CI, so changing the graph never reaches a real run unverified.

**Warning signs:**
Audio gradually out of sync toward the end; clicks at cuts; ffmpeg warnings about "non-monotonous DTS".

**Phase to address:** Assemble.

---

### Pitfall 20: Windows sleep, lock screen and display timeout during long unattended queues

**What goes wrong:**
A 5-match queue can run for an hour or more. The system may sleep or turn off the display, and a locked session / screensaver can interrupt the windowed DirectX game. `SetThreadExecutionState` prevents idle sleep and display-off only while called, is **per thread**, is cleared when the thread ends, and Microsoft explicitly notes it does not stop the screen saver and cannot prevent the user from sleeping the machine. Locking (Win+L or "require sign-in after the display turns off") moves the session to a secure desktop where a windowed game's presentation can stall. **[HIGH for API semantics (Microsoft docs); MEDIUM-LOW for the exact effect on CS2 frame capture, verify with an overnight spike]**

**How to avoid:**
- Hold a power request for the whole Record and Assemble stages: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)` **from the worker thread that runs the job** (not the UI thread), with a guard that clears it on every exit path. (Alternative: `PowerCreateRequest`/`PowerSetRequest`, which is not thread-bound.)
- Show "keep the PC awake and unlocked" in the UI while a queue is running; document disabling the screensaver with sign-in-on-resume for unattended runs.
- Detect a stalled take caused by locking via the progress watchdog (Pitfall 6) and name it: "no progress for 60 s; was the session locked or asleep?". Optional: log session lock/unlock events (`WM_WTSSESSION_CHANGE`) into the journal so the post-mortem can correlate.
- Resume after sleep: wall-clock jumps break `Instant`-based timers differently (Instant keeps ticking over sleep on Windows in current std, but cannot be assumed); a sleep during a take makes the take bad -> the ledger/validation (Pitfall 11/15) catches it and the job retries.

**Warning signs:**
Failures clustered overnight or after lunch; takes whose duration is shorter than planned; Event Viewer Kernel-Power sleep entries during a job.

**Phase to address:** Soak (guard + docs), Session (stall detection naming).

---

### Pitfall 21: GPU/driver overlays and injectors that fight HLAE

**What goes wrong:**
Anything that injects into a game or hooks the swap chain (Steam overlay, Discord overlay, NVIDIA App/GeForce Experience overlay and Instant Replay, AMD overlay, MSI Afterburner/RTSS, Xbox Game Bar capture) can conflict with HLAE's hook: crashes at startup, black or corrupted frames, unexpected FPS caps, or toast pop-ups drawn into the game window. The launch line already includes `-allow_third_party_software`, which suggests CS2 itself guards against injected software. Notifications from the overlays can also steal focus. **[MEDIUM, community consensus; no official list found]**

**How to avoid:**
- Do not try to detect all overlays. Provide a short, explicit "recording environment" checklist in the README and Settings, and detect only cheap high-signal cases: processes `RTSS.exe`, `Discord.exe` (overlay on?), `GameBar`, `NVIDIA Overlay`/`nvcontainer` show a **warning** in preflight (not a block).
- Make the symptoms diagnosable: when the game exits during startup, the error text lists the warning processes seen at preflight ("startup crash; overlays running: Discord, RTSS").
- First-frame sanity check for black frames is optional (ffmpeg `blackdetect` on take 0 for the first seconds) and cheap; add only if soak shows black takes.

**Warning signs:**
Game crashes immediately only when Discord/Afterburner is running; black takes while audio is fine.

**Phase to address:** Preflight (warnings), Soak (checklist validated on the author's machine).

---

### Pitfall 22: Back-to-back launches: Steam and the OS need time to forget the last game

**What goes wrong:**
In a queue, job N+1 starts seconds after job N's `quit`. Steam may still consider CS2 running (the "zombie `cs2.exe`" advice is a top cause of "CS2 won't start"), the process may still be writing logs/handles, the netcon port of the old process may linger, and VRAM may not be released. The existing code frees a port per launch (good) but starts immediately. **[MEDIUM]**

**How to avoid:**
- After teardown, wait on the owned process handle for exit, then a short cool-down (3-5 s, tune in soak), then re-run the foreign-`cs2.exe` check and confirm Steam still reports a logged-in user before the next spawn.
- In the soak phase run the 5 matches **twice** in a row without restarting the app; the second pass finds leaks in teardown that the first pass cannot.

**Warning signs:**
Every second job in a queue fails to start; failures vanish when a minute is inserted between jobs.

**Phase to address:** Session (cool-down + exit confirmation), Soak.

---

## Moderate Pitfalls

### Pitfall 23: Console parsing based on substring matches
**What goes wrong:** `marker()` rejects lines containing `echo` or `addAtTick`; a nickname or title containing those substrings would be dropped, and the console echoes command text in some forms. Output can be split mid-line or arrive with prefixes; `String::from_utf8_lossy` already protects against encodings.
**Prevention:** Use unique, unlikely tokens with a per-run nonce (`CS2C_READY_<runid>`, `CS2C_TAKE_DONE_<runid> <i>`), match on whole tokens, and ignore the echoed command by matching the token only where it appears as *output*. Keep a ring buffer of the last 200 lines for the failure report.
**Phase:** Session.

### Pitfall 24: Console port exposure
**What goes wrong:** The Source netcon (documented for dedicated servers; `-netconpassword` exists there) is an unauthenticated command channel. If the game binds beyond loopback, other machines on the LAN can drive the game. The bind address in CS2 is unverified. **[LOW]**
**Prevention:** In the spike, check with `netstat -ano` whether the port is bound to `127.0.0.1` or `0.0.0.0`; if the latter, add `-netconpassword <random>` if CS2 honours it and authenticate, or document the risk. The port is random per run (already done via `free_port`, which has a small bind-then-release race; acceptable).
**Phase:** Session (spike).

### Pitfall 25: Single-client netcon and zombie reader threads
**What goes wrong:** A second `try_connect` while the first socket is still open may be refused or may steal output; each `Console::open` spawns a reader thread that is never joined.
**Prevention:** One `Console` per game session behind the session guard; reconnect closes the old socket first; the reader thread ends with the socket (document and test with the fake).
**Phase:** Session.

### Pitfall 26: `since`/wall-clock reliance and DST/NTP
**What goes wrong:** Any logic comparing filesystem mtimes with `SystemTime::now()` is fragile (Pitfall 11). Elapsed-time budgets must use monotonic `Instant`.
**Prevention:** Use `Instant` for deadlines, never mtimes for decisions.
**Phase:** Takes.

### Pitfall 27: First-run-after-update slowness mistaken for a hang
**What goes wrong:** The first CS2 start after an update can spend minutes on shader precompilation; a fixed 150 s cap kills a healthy start. **[MEDIUM]**
**Prevention:** Progress-based extension (owned process CPU time still growing and no crash dialog) up to a larger hard cap, and a visible log line "CS2 is still starting (CPU busy), 140 s".
**Phase:** Session.

### Pitfall 28: Folder/Steam library discovery assumptions
**What goes wrong:** `find_cs2()` parses `libraryfolders.vdf` only under hard-coded Steam roots; a Steam installed elsewhere (or a library on another drive without the standard names) is missed. HLAE discovery walks entire drives to depth 5 and can take long or find an old copy.
**Prevention:** Find Steam through the registry (`HKCU\Software\Valve\Steam\SteamPath`), then parse `libraryfolders.vdf` for every library; when more than one HLAE is found, prefer the newest `AfxHookSource2.dll` and show the choice; cache the discovery result and validate it each preflight rather than rescanning.
**Phase:** Preflight.

### Pitfall 29: `cmd /C start` for opening results
**What goes wrong:** Paths with `&`, `^`, `%` are parsed by `cmd`; it also hides launch failures.
**Prevention:** Open via `ShellExecuteW`/the `opener` crate; only on user action or when the queue drains (Pitfall 15).
**Phase:** Soak (UX cleanup), low priority.

### Pitfall 30: Windows Defender and AV latency
**What goes wrong:** Real-time scanning of fresh multi-GB mp4 files, the injected DLL, and `ffmpeg.exe` adds seconds of latency to file closes and renames, and can quarantine `AfxHookSource2.dll`.
**Prevention:** Validation loops use backoff, not fixed windows; rename/delete retried with backoff; the preflight "hook DLL unchanged" check (Pitfall 8). Suggest (do not automate) a Defender exclusion for the output folder and HLAE folder.
**Phase:** Takes, Resume.

---

## Minor Pitfalls

### Pitfall 31: Unwrap/expect on I/O in the record path
**What goes wrong:** Panics in the worker thread kill the whole job runner (and any game still running).
**Prevention:** `catch_unwind` at the job boundary that routes through the `GameSession` guard and marks the job failed with "internal error"; convert I/O unwraps in `launch.rs`/`assemble.rs` to `?` with context.
**Phase:** Seams.

### Pitfall 32: Error strings carry no stage and no evidence
**What goes wrong:** `Result<_, String>` loses structure. The UI cannot colour/route errors and tests cannot assert on categories.
**Prevention:** A `RecordError { stage, kind, message_ru, evidence: Vec<String> }` enum with stable kinds (`SteamNotRunning`, `GameAlreadyRunning`, `HlaeOutdated`, `ProcessDiedAtStart`, `ConsoleNeverOpened`, `DemoNotLoaded`, `ScriptNotExecuted`, `MarkerTimeout`, `TakeInvalid{idx}`, `DiskFull`, `Cancelled`...) and a single `messages` module for Russian text. This is the foundation for "every point of failure is distinguishable" in the milestone's Active requirements.
**Phase:** Seams.

### Pitfall 33: HLAE discovery picks an old portable copy
**What goes wrong:** Portable HLAE is often extracted multiple times (Downloads, Desktop, root); the depth-first walk returns the first hit, which may be a stale version.
**Prevention:** Collect all candidates, sort by `AfxHookSource2.dll` version/date, show the chosen one in Settings.
**Phase:** Preflight.

### Pitfall 34: Tests that only cover the happy path with fakes
**What goes wrong:** Fakes written from the developer's mental model encode the same wrong assumptions as the code (for example, that the launcher stays alive).
**Prevention:** Build fake scenarios from **recorded real transcripts** (console output of a real success and a few real failures, collected once in the spike) and keep the transcript files in `tests/fixtures`.
**Phase:** Seams, then update after each real failure seen in Soak.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Polling `tasklist` once per second | No new dependencies | Spawn cost, localization, fails open/closed, wrong-instance matches | Never for ownership; acceptable only as a last-resort fallback behind the trait |
| `taskkill /IM cs2.exe /F` | One line | Kills the user's own game; leaves HLAE children | Never |
| Directory-mtime take detection | No dependency on markers | Truncated takes, FS granularity, AV touches | Never as the primary signal; fine as a progress hint |
| Hand-escaped `drawtext` strings | No temp files | Failures only for exotic nicknames, after recording | Only for static, known-safe literals |
| `Result<_, String>` errors | Fast to write | No categories, no tests on failure kinds, UI cannot guide | Not past the Seams phase |
| Re-sending `exec highlights` on a timer | Masks races on slow loads | Duplicated or shifted takes | Only if the cfg is proven idempotent and the retry is keyed on an "accepted" marker |
| `Command::output()` | Short code | Uncancellable, unbounded | Only inside `run_with_deadline` |
| Fixed 20 s "quiet" window | Simple | Wrong on slow disks, wrong with AV | Never; replace with ffprobe validation |
| Existence-only "already recorded" check | Fast retry | Poisoned retries with truncated takes | Never |
| Global `ffmpeg` from PATH | Works in a terminal | Differs for tray/autostart launches | Never; resolve once and store the absolute path |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| HLAE launcher (`HLAE.exe -customLoader -noGui -autoStart`) | Dropping the `Child`; assuming it stays alive; ignoring exit code and dialogs | Keep the handle, `try_wait` in the first seconds, capture stdout/stderr, treat non-zero or lingering as named failures |
| AfxHookSource2 | Assuming the hook loaded because the game started | Probe `mirv_*` commands after connect; fail fast on "unknown command"; record HLAE and CS2 build per job |
| CS2 netcon | Treating a TCP connect as readiness; no auth; one global sleep | READY marker via `mirv_cmd addAtTick`, nonce tokens, session-owned single connection, connection-lost event |
| `exec <cfg>` | Not noticing "couldn't exec"; wrong folder; double `.cfg` | Install manifest + size check; scan console for the error; script-start marker |
| `mirv_streams` in-game ffmpeg | Believing markers imply data | Scan `AFXERROR`; probe take 0 early; verify HLAE's own ffmpeg, not PATH's |
| Steam | Assuming it is running and logged in; ignoring pending updates | Process + registry check; `appmanifest_730.acf` `StateFlags`; offline mode as a documented option |
| `tasklist`/`taskkill` | Name-based, spawn-based, fail-open | PID/handle-based, Job Object, tri-state liveness |
| ffmpeg assembly | Inline filter strings; direct output file; stdout/stderr pipes unread | Filter script + `textfile`; `.part` + rename; reader threads (already done in `run_ffmpeg`) |
| ffprobe | Used only at the end | Per-take gate immediately after each take, plus first-take early check |
| SQLite job store | `reset_running` -> failed | `interrupted` state with stage, take ledger, process identity |
| Windows power | Ignoring sleep/lock | `SetThreadExecutionState` from the worker thread with RAII guard |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Spawning `tasklist` every second for the whole recording | Constant process churn; slow polls under AV | Handle wait (`WaitForSingleObject`) or in-process process listing | Any long recording; worse with AV |
| Full-file hashing in `plan_hash` and geometry | Slow job start with large outputs | Hash metadata and cfg contents only | Large `out_dir` content |
| Re-reading whole take directories each second | Disk I/O contention while recording | Poll `metadata` of expected files only; use exponential backoff | Long takes on HDD |
| Recording at a resolution/fps above what the GPU sustains while also encoding | Dropped frames, slow-motion artefacts, longer takes | Preflight picks conservative defaults; early take-0 check compares expected vs actual duration | High fps replays with NVENC and a busy GPU |
| Disk growth from `clips_old_*` and demo copies | Space runs out on the 3rd-5th queued match | Cleanup policy + space estimate (Pitfall 13) | Roughly the 3rd-5th retry-heavy job on a small drive |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Unauthenticated netcon possibly reachable beyond loopback | Remote command execution in the game process | Verify bind address; use a password if supported; random port per run (done) |
| `cmd /C start <path>` | `&`, `^`, `%` in paths interpreted by cmd | `ShellExecuteW` or the `opener` crate |
| Mixing the insecure instance with online play | VAC ban risk (HLAE README) | Dedicated launch path only; "no server connect" console guard; README warning |
| Trusting `ffmpeg` found on PATH | A planted `ffmpeg.exe` earlier on PATH runs with user rights | Resolve once, show the resolved absolute path in Settings, allow explicit override |
| Titles/nicknames in filter strings | Filter injection (reads files via drawtext `textfile` if attacker controls the string) | Text only ever through a file the app writes; no user text in the filter string |
| `git ls-remote` of map data without pinning | Wrong data on network issues | Pin release, offline fallback (already in CONCERNS) |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Error text says "game did not start" | User cannot tell Steam/HLAE/demo apart | State name + evidence + next action ("Steam is not signed in: sign in and press Retry") |
| Hidden failures downgraded to log lines | User sees a spinner for 20 minutes | Failure is an error with a stage; log shows the last console lines |
| One problem reported per attempt | Fix-retry-fix loops | Preflight lists all problems at once |
| Auto-opened video in the middle of a queue | Steals focus from the next recording | Open when the queue is empty or on click |
| Cancel appears to do nothing | User kills the game manually | Show "cancelling: closing the game" with the stage; guarantee seconds-level latency |
| Resume is invisible | User re-queues and re-downloads | "Resume from recording" with takes found (7/12 valid) |
| No record of the working HLAE/CS2 pair | "Worked last week" mysteries | Show tested pair in Settings and per job |

## "Looks Done But Isn't" Checklist

- [ ] **Preflight:** Often missing Steam sign-in state and pending-update check — verify with Steam signed out and with an update queued (by pausing the download).
- [ ] **Cancellation:** Often only tested in the poll loops — verify cancel during each of: Steam wait, HLAE launch, console connect, demo load, mid-take, take finalisation, assembly, and during a blocked external process (fake that never exits).
- [ ] **Ownership:** Often kills the wrong CS2 — verify that with a second unrelated `cs2.exe` (a renamed dummy executable) neither cancel nor failure touches it.
- [ ] **Teardown on failure:** Often only on cancel — verify the game is gone after timeout, process-died, and app panic.
- [ ] **Liveness:** Often fails open — verify with the process query forced to `Unknown` that the job fails with a named error in bounded time.
- [ ] **Take validation:** Often checks only existence — verify with a deliberately truncated mp4, a zero-byte mp4, an `audio.wav` with an unfinished header, and a missing middle take.
- [ ] **Resume:** Often skips recording on a stale/invalid ledger — verify with a corrupted take and a changed plan fingerprint.
- [ ] **Crash recovery:** Often leaves an orphan game — verify by killing the app mid-take and restarting.
- [ ] **ffmpeg escaping:** Often tested with ASCII titles — verify with quotes, colons, commas, `%`, Cyrillic, emoji, 200-character titles against real ffmpeg.
- [ ] **NVENC:** Often probed on the wrong binary — verify the probe uses HLAE's ffmpeg for takes and PATH ffmpeg for assembly, and fallback to x264 works.
- [ ] **Disk:** Often one volume — verify with the game and output on different drives and a small output drive.
- [ ] **Sleep/lock:** Often forgotten — verify with a one-hour queue and display timeout set to 1 minute.
- [ ] **Unattended queue:** Often one job tested — verify 5 matches, then 5 again without restarting the app.
- [ ] **Diagnostics:** Often missing the console tail — verify every failure message contains the stage, elapsed time, and the last console lines.

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Orphaned CS2 after app crash | LOW | Verify `(pid, creation_time)` from DB; close that process; mark job `interrupted`; offer resume |
| HLAE outdated after CS2 update | MEDIUM | Update HLAE (releases usually follow within days) or pin Steam offline mode; re-run preflight; jobs marked `failed(env)` become retryable |
| Truncated or empty take | MEDIUM | Re-record the whole job into a fresh clips dir; keep the failed attempt for diagnostics until space is needed |
| Wrong path (Cyrillic/spaces) | LOW | Preflight error names the path; user picks an ASCII output dir; no re-planning needed |
| Disk full during recording | MEDIUM | Free space, delete `clips_old_*`, re-record; add the estimate numbers to the error |
| Assembly failure from title escaping | LOW | Re-run assembly only (takes valid); fix by moving text to files; no re-recording |
| NVENC failure | LOW | Switch preset to CPU encoder, re-record only if takes were NVENC; otherwise re-assemble |
| Sleep/lock interrupted a take | MEDIUM | Validation fails take N; re-record all; ensure the power-request guard is active |
| Locked files block rename on retry | LOW | Backoff and sibling directory fallback; wait for owned process exit |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1 Liveness fails open/closed | Seams, Session, Takes | Fake returns `Unknown`: job fails with named error in bounded time; no `tasklist` in process trace |
| 2 Name-based kill, no teardown on error | Session, Cancel/Teardown | Foreign dummy `cs2.exe` survives cancel/failure; game gone after every failure path |
| 3 Shallow preflight | Preflight | Table-driven tests per check with fakes; manual runs with Steam off, CS2 on, update pending |
| 4 CS2 update vs HLAE | Preflight, Session, Soak | Version pair displayed and stored; startup-death and unknown-command detectors have transcript fixtures |
| 5 Readiness and exec retries | Session | Transcript-driven test of the state machine incl. long-demo with late first shot; no duplicate takes |
| 6 Per-callee timeouts, wedged-but-alive | Session, Takes | Stage watchdog tests with injected clock; idle and hard cap both exercised |
| 7 `-insecure`/VAC | Preflight, Session | README note; console guard test: connect-to-server line aborts the job |
| 8 Launcher invisibility | Session | Fake launcher exit-code and linger scenarios |
| 9 Paths, names, `.dem.zst`, cfg location | Preflight, Session, Seams | Tests with Cyrillic/space paths; install manifest and cleanup test; `couldn't exec` fixture |
| 10 Old demo vs build | Preflight, Session, Soak | Warning displayed for old demo; one old demo in soak |
| 11 mtime take discovery | Takes | Truncated/zero/missing-middle-take scenarios fail with take index |
| 12 Empty takes from in-game ffmpeg | Preflight, Session, Takes | `AFXERROR` fixture fails fast; first-take early probe |
| 13 Disk space | Preflight, Takes, Resume | Estimate test; two-volume test; GC of `clips_old_*` |
| 14 Crash recovery, orphans | Resume, Session | Kill app mid-take: restart reconciles, job `interrupted` with stage |
| 15 Retry trusts existence | Resume, Takes | Ledger + fingerprint tests; rename-with-backoff test using a locked file |
| 16 Window size/focus | Takes, Assemble | Take-0 parameter check; mixed-size input assembly test |
| 17 Blocking `output()` | Seams, Cancel/Teardown | Lint bans raw `.output()`; hung-process fake cancelled in <2 s |
| 18 ffmpeg escaping | Assemble | Real-ffmpeg integration test with hostile titles; `.part` + rename |
| 19 A/V assumptions | Assemble | Golden synthetic-takes test with different fps and audio offsets |
| 20 Sleep/lock | Soak, Session | One-hour queue with 1-minute display timeout completes |
| 21 Overlays | Preflight, Soak | Warning list; soak on the author's machine with the checklist applied |
| 22 Back-to-back launches | Session, Soak | 5+5 queue without restart |
| 23-34 Moderate/minor | As listed per item | Covered by the same test infrastructure |

## Suggested Phase Ordering Implied by the Pitfalls

1. **Seams first.** Pitfalls 1, 17, 31, 32, 34 are enabling: the process/runner trait with deadline + cancel, structured `RecordError`, injected clock, and transcript-driven fakes. Nothing else can be tested without them.
2. **Preflight before Session.** It removes the largest share of the reported "game does not start" causes cheaply (Pitfalls 3, 7, 9, 13, 21, 28).
3. **Session** (own the process, state machine, watchdog, diagnostics) before **Takes** (identity-based discovery and validation), because take validation needs the markers and AFXERROR scanner.
4. **Cancel/Teardown** can start with Session (guard) but needs its own verification pass over every stage.
5. **Assemble** hardening is independent of CS2 and can run in parallel with Session/Takes (it only needs synthetic takes).
6. **Resume** after Takes (it depends on the take ledger and fingerprint).
7. **Soak** last: 5+5 queue, sleep/lock, overlays, old demo; its output is the recorded known-good HLAE/CS2 pair.

**Research flags for phases:**
- Session: needs a short **real-machine spike** before design is frozen: capture console transcripts (success, missing cfg, missing hook, demo not found), confirm job-object inheritance through HLAE, netcon bind address, `-condebug` support, `mirv_cmd`-unknown text.
- Preflight: Steam registry values and `appmanifest_730.acf` flags should be confirmed on the author's machine (MEDIUM confidence).
- Takes: confirm HLAE take folder timing (when directories and files appear, WAV header behaviour) with a one-shot recording.
- Assemble, Resume, Seams: standard patterns, unlikely to need research.

## Sources

- HLAE repository README (VAC warning, genuine Steam requirement, Steam offline mode advice, .NET requirement): https://github.com/advancedfx/advancedfx — HIGH
- HLAE releases (CS2 adjustments 2.191.0, 2.192.3-2.192.7 through 2026-10-08): https://github.com/advancedfx/advancedfx/releases — HIGH
- HLAE wiki, old versions for specific CS2 versions (four breaking updates in twelve months): https://github.com/advancedfx/advancedfx/wiki/Old-HLAE-versions-for-specific-CS2-versions — HIGH
- HLAE wiki, `mirv_streams` for CS2 (`AFXERROR: Failed writing image`, ffmpeg location): https://github.com/advancedfx/advancedfx/wiki/Source2%3Amirv_streams — HIGH
- HLAE wiki, `mirv_cmd` (`addAtTick`, `add`, `clear`, `save/load` relative to the exe folder): https://github.com/advancedfx/advancedfx/wiki/Source%3Amirv_cmd — HIGH
- Valve Developer Community, Source dedicated server command-line options (`-netconport`, `-netconpassword`; documented for SRCDS, not the CS2 client): https://developer.valvesoftware.com/wiki/Command_line_options_(Source_Dedicated_Server) — MEDIUM (applicability to the CS2 client unverified)
- Rust `std::process::Child` (no kill on drop; `kill`, `try_wait`, `wait`; no descendant handling): https://doc.rust-lang.org/std/process/struct.Child.html — HIGH
- Microsoft Learn, Job Objects (`KILL_ON_JOB_CLOSE`, inheritance by child processes, breakaway, nesting): https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects — HIGH
- Microsoft Learn, `SetThreadExecutionState` (per-thread, no screensaver prevention, cannot prevent user sleep): https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate — HIGH
- Microsoft Learn, `tasklist` (filters, CSV, no exit-code contract described): https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/tasklist — HIGH
- FFmpeg filtergraph escaping (three levels; recommends `textfile` for drawtext): https://ffmpeg.org/ffmpeg-filters.html#Notes-on-filtergraph-escaping — HIGH
- csdb.gg, "CS2 Not Launching" (zombie `cs2.exe` makes Steam refuse a second start): https://csdb.gg/fix/cs2-not-launching/ — LOW-MEDIUM (community guide)
- Steam community CS2 discussions on `exec` failures (`couldn't exec ... unable to read file`, double `.cfg`, CS2 folder layout): https://steamcommunity.com/app/730/discussions/0/3821922030309529559 — LOW-MEDIUM (user reports; exact thread was one of several search hits)
- LHM guide (HLAE + Workshop Tools start the game with `-insecure`; secure servers refuse insecure clients): https://guide.lhm.gg/docs/lhm-basics/games-configurations/counter-strike/ — MEDIUM
- Steam community thread on a VAC ban after launching with HLAE custom loader and `-insecure` (anecdotal, disputed): https://steamcommunity.com/discussions/forum/9/135509024342895798 — LOW
- Repo code read for this document: `crates/app/src/launch.rs`, `crates/app/src/jobs.rs` (record flow, retry shortcut, `kill_game` only on cancel), `crates/pipeline/src/lib.rs` (`console_name`), `crates/app/src/ui.rs` (`open_path`), `.planning/codebase/CONCERNS.md`, `.planning/PROJECT.md` — HIGH

---
*Pitfalls research for: automated CS2 demo-to-montage recording (HLAE + CS2 + ffmpeg), Windows, Rust desktop app*
*Researched: 2026-10-10*
