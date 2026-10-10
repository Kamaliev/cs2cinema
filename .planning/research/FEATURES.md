# Feature Research

**Domain:** Reliability of the "record" stage in a single-user desktop job queue that drives a game (HLAE + CS2) and then ffmpeg, on Windows, in Rust (egui, `std::thread`, SQLite)
**Milestone:** Subsequent ("Надёжность записи") on the existing cs2cinema app. The montage itself already works.
**Researched:** 2026-10-10
**Confidence:** MEDIUM-HIGH. HIGH for what cs-demo-manager (CSDM) actually does, because its source was read directly. HIGH for the OBS, Jenkins, GitHub Actions and Win32 job-object facts, which come from primary docs. MEDIUM for generalisations across tools. LOW for HLAE/AfxHookSource2 internals, because the HLAE wiki for CS2 is thin (see "Research flags").

> Note on confidence tags: the `classify-confidence` seam could not be run (no `node` on PATH in this environment). Tiers below are assigned by hand using the rule "primary source read directly = HIGH, search-engine summary only = MEDIUM, own inference = LOW".

---

## What comparable tools actually do (evidence summary)

This section is the evidence base for the feature tables below. Each row says what a real tool does, so the categorisation is not guesswork.

| Concern | Real tool behaviour | Source / confidence |
|---------|--------------------|---------------------|
| Preflight | CSDM runs `assertVideoGenerationIsPossible` before every video job. It checks: sequences not empty, HLAE installed, FFmpeg or VirtualDub installed, Steam running. Each failure is a distinct typed error (`HlaeNotInstalled`, `FfmpegNotInstalled`, `InvalidFfmpegExecutable`, `SteamNotRunning`, `DemoNotFound`, `InvalidDemoPath`, `MissingPlayerSlotError`...). | CSDM source, HIGH |
| Preflight (disk) | CSDM only warns in the UI when the raw-file requirement exceeds 40 GB. It does not hard-check free space. | CSDM video guide, HIGH |
| Failure taxonomy | CSDM separates `HlaeError` (HLAE exit code != 0, or the window titled "Error - AfxHookSource" exists in the cs2 process via `tasklist /v`), `GameError` (cs2 not running after HLAE exits, or cs2 exit code != 0), `CounterStrikeNoResponse` (game did not answer on its WebSocket), `CounterStrikeAlreadyRunning`, `CounterStrikeExecutableNotFound`, `StartCounterStrikeError`. CSDM stops at about this granularity: it has no "demo loaded but script did not run" or "take marker missing" classes. | CSDM source, HIGH |
| Timeouts | CSDM has no wall-clock timeouts around the game. It waits for the HLAE process `exit` event and then for the game to close. A wedged game blocks the job until the user cancels. Only fixed `sleep(2000)` settle delays exist. | CSDM source (`start-counter-strike-with-hlae.ts`, `generate-video.ts`), HIGH |
| Cancellation | Cooperative `AbortSignal` (`throwIfAborted` between steps). On abort it kills HLAE, VirtualDub, ffmpeg and CS processes **by image name**, waits 2 s because "files may still be locked", then deletes the job's raw files. | CSDM source, HIGH |
| Process ownership | CSDM kills **all** `cs2.exe`/`csgo.exe` by name, and it also does this *before* launch (`killCounterStrikeProcesses()` in `startCounterStrikeWithHlae`), so a player's own running game is killed. A 2 s sleep after the kill avoids a Source engine error on relaunch. This is the exact behaviour the cs2cinema project wants to avoid. | CSDM source, HIGH |
| Take discovery | CSDM gives each sequence its own output folder and takes the **last** `take*` directory by glob (`getHlaeTakeFolderPath`). It asserts the folder, the video file and the wav file exist and throws `RawFilesNotFoundError` / `WavFileNotFound`. It does not probe duration or frame count. | CSDM source, HIGH |
| Queue | CSDM's video queue is sequential, **paused by default**, with pause/resume, cancel one, remove one/all. Each video has a unique ID and its own output folder, so retries never overwrite. | CSDM video guide, HIGH |
| Retry/recovery | CSDM docs do not describe retry or crash recovery for video jobs. A failed job is rebuilt by the user. | CSDM video guide, MEDIUM (absence of evidence) |
| Troubleshooting guidance | The only HLAE start-failure advice in CSDM docs: "update HLAE, game updates can break compatibility". HLAE's own wiki has a page pairing specific CS2 builds with the last working HLAE release ("HLAE versions for some breaking CS2 updates"), and the HLAE README suggests Steam offline mode to avoid breaking updates mid-project. The HLAE issue tracker has "Error - AfxHookSource2", "CS2 ... update broke HLAE" and "Launching CS2 using HLAE resulted in a crash" issues. | CSDM docs, HLAE wiki, HLAE issue titles; HIGH/MEDIUM |
| Explicit completion signal | OBS (obs-websocket 5.x) emits `RecordStateChanged {outputActive, outputState, outputPath}`; `outputPath` is set when the record **stops**, and `StopRecord` returns `outputPath`. The client never has to scan a directory or guess from mtimes. | obs-websocket protocol.md, HIGH |
| Staged cancellation | GitHub Actions runner: SIGINT/Ctrl-C to the step, wait 7.5 s, SIGTERM/Ctrl-Break, wait 2.5 s, kill the process tree; the server enforces a 5-minute hard cap. Documented caveat: children of a shell step may be killed without ever seeing a signal. Job- and step-level `timeout-minutes` bound every run. | GitHub docs and signal-handling demo, HIGH/MEDIUM |
| Process-tree ownership (Windows) | A Win32 job object groups processes. Children created with `CreateProcess` join the parent's job by default. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills every process in the job when the last handle closes (including when the owning app crashes). `TerminateJobObject` kills the set. Processes created via `Win32_Process.Create` are not included. | Microsoft Learn, HIGH |
| Restart a failed run from a stage | Jenkins Declarative Pipeline "Restart from Stage": any top-level stage that ran, inputs identical (SCM, parameters, stashes), creates a *new* build, earlier stages are skipped, stashes are kept only if `preserveStashes` is set. | jenkins.io, HIGH |
| Failed-item retry in batch encoders | Adobe Media Encoder: failed queue items are retried by "reset status" after fixing the cause (e.g. disk full), community-reported. Watch-folder mode checks that a file is no longer growing before picking it up. HandBrake users historically asked for queue persistence after a crash. | Adobe/HandBrake forums, LOW-MEDIUM |
| Cloud highlight services | Allstar renders in the cloud from match data and FACEIT/Leetify trigger it; there is no local game process, so no comparable preflight/launch failure surface. It is useful only as a contrast: its "failure handling" is invisible to the user. | FACEIT help centre, Leetify blog, MEDIUM |
| Tests around the launcher | The file listing of CSDM shows unit tests for sequence builders and filename placeholders but none beside the launcher modules. cs2cinema's plan (fake process host, scenarios as tests) goes beyond it. | CSDM tree listing, MEDIUM |

### What the existing cs2cinema code already does (so requirements do not re-invent it)

| Already present | Where | Gap this milestone closes |
|-----------------|-------|---------------------------|
| `check_paths` (HLAE.exe, AfxHookSource2.dll, cs2.exe) | `launch.rs` | No ffmpeg/ffprobe, Steam, foreign-CS2, disk, version-compat checks. Runs *after* the slow demo analysis, not first. |
| Free TCP port per launch | `launch::free_port` | Good, keep. |
| `CS2C_READY` and `TAKE_DONE` console markers, `exec highlights` retry (30 s x 4) | `jobs.rs::record` | If `CS2C_READY` never arrives the code logs "должна была загрузиться" and **continues**. Console connect failure also continues. Neither is a failure. |
| `LOAD_CAP` 150 s, `load_wait` setting | `jobs.rs` | Only the load wait is bounded; spawn, process appearance, per-take and overall waits are not. |
| `game_alive()` via `tasklist` | `launch.rs` | Returns `true` on error; spawns a process per poll; cannot tell "our" cs2 from another. |
| `kill_game()` = `taskkill /IM cs2.exe /F` | `launch.rs` | Kills every instance; no graceful `quit` first; no verification the process is gone. |
| `wait_for_clips` by folder mtime + quiet period 3/8/20 s | `launch.rs` | Discovery by time, not by expected take set. |
| `plan.hash` + `video.mp4`/`audio.wav` exist check = "already recorded" shortcut | `jobs.rs` | Existence only, no ffprobe, no per-take status. It is the seed of the manifest idea. |
| Old `clips/` renamed to `clips_old_<ts>` | `jobs.rs` | Good, keep (it also quarantines stale takes). |
| `requeue`, `cancel_job`, `reset_running` (running -> failed with fixed text) | `store` | `start_job` wipes the log on every start; no attempt number, no failure code, no stage-at-failure column; restart recovery only marks `failed`. |
| ffmpeg progress thread with cancel + `child.kill()` | `jobs.rs::run_ffmpeg` | Good pattern; `nvenc_check`, `tasklist`, `taskkill` still use blocking `Command::output()`. |
| Demo path remembered per FACEIT match | `store.remember_demo` | Enables "retry without re-download". |

---

## Feature Landscape

Feature IDs are stable handles for REQUIREMENTS.md. Categories map 1:1 to proposed requirement sections.

### Table Stakes (the user keeps babysitting the queue without these)

#### Preflight (PRE)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| PRE-1 | **Preflight runner returning a structured report** (list of checks, each `ok / warn / fail`, message in Russian, hint on how to fix) | CSDM throws one typed error per missing prerequisite; the project goal is "show what exactly is wrong". One report beats one error at a time (user fixes everything in one pass). | LOW | Pure function over a `Host` trait (fs + process list) so it is testable. Same function powers the automatic pre-record gate and a "Проверить окружение" button in settings. Keep `fail` blocking, `warn` non-blocking. |
| PRE-2 | **Run preflight first**, before download/parse/plan for any job that will record | Today `check_paths` runs after minutes of demo analysis. Fail in seconds. | LOW | Depends on PRE-1. Plan-only jobs skip the record-specific checks. |
| PRE-3 | **Tooling checks**: HLAE.exe, `x64\AfxHookSource2.dll`, cs2.exe exist; `ffmpeg` and `ffprobe` resolve to real executables and answer `-version` (with a short timeout) | CSDM: `HlaeNotInstalled`, `FfmpegNotInstalled`, `InvalidFfmpegExecutable`. Existing code only checks 3 paths and assumes ffmpeg on PATH. | LOW | Also check HLAE's own `ffmpeg\ffmpeg.ini` target (`hlae_ffmpeg`): HLAE records through that ffmpeg, not the PATH one. Reuse for the HLAE-version display (PRE-9). |
| PRE-4 | **Steam is running** | CSDM `assertSteamIsRunning`; the launch line uses `-steam`. | LOW | Process-name check via a process snapshot (not `tasklist` text). |
| PRE-5 | **No foreign CS2 is running**: refuse to start and say so (do not kill it) | CSDM instead kills any CS process silently. cs2cinema's explicit requirement is the opposite. CS2 also cannot run two instances reliably. | LOW | Fail with "CS2 уже запущен (PID n). Закройте его — приложение не трогает чужие процессы". Depends on process-snapshot capability in the `Host` trait. Enables TMO-5 (our PID = the only new cs2). |
| PRE-6 | **Free disk space** on the output drive (and the CS2 drive where the demo is copied) with an estimate from the plan | Project requirement. A full disk produces truncated takes that look like a game problem. | LOW-MED | Estimate = planned seconds x take bitrate x margin, plus demo size copy. Needs the plan, so it is a second, "post-plan" preflight tier (cheap tier first: tools/processes; plan-aware tier: disk). CSDM only warns above 40 GB, so a hard floor is an upgrade. |
| PRE-7 | **Input and path sanity**: demo exists and is not a Git-LFS pointer, output path has no spaces (existing `console_safe`), output dir writable | CSDM: `DemoNotFound`, `InvalidDemoPath`. CONCERNS.md notes the LFS pointer case is only surfaced in the CLI. | LOW | Move the existing `console_safe` and LFS checks into the preflight report. |

#### Launch diagnostics (LCH)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| LCH-1 | **Explicit record state machine** with named states: `Preflight -> SpawnLauncher -> AwaitGameProcess -> AwaitConsole -> AwaitDemoReady -> ScriptStarted -> RecordingTake(n) -> Closing -> Validated` | The user's stated pain: "game does not start and I do not know which of N reasons". Each state = one failure point. CSDM has only ~4 states and no demo/script/take states. | MEDIUM | Single enum drives log lines, the job `stage` text, UI progress and error codes. Foundation for most other features. |
| LCH-2 | **Typed failure with code, state, human message and hint** (`RecordFailure { code, state, detail, last_lines }`) | CSDM's typed error classes (`HlaeError`, `GameError`, `CounterStrikeNoResponse`...). Needed so retry policy (RET-*) and UI can branch without string matching. | MEDIUM | Minimum code set, each distinguishable in log and error text: `LauncherSpawnFailed`, `LauncherExitedNonZero`, `GameNeverAppeared`, `HlaeErrorWindow`, `GameDiedEarly`, `ConsoleNeverOpened`, `DemoNeverReady`, `ScriptNeverStarted` (no take marker/file), `TakeMarkerMissing(n)`, `GameDiedMidRecord(n)`, `TakeTruncated(n)`. Persisted in SQLite (new columns), not just in the error string. |
| LCH-3 | **A missing readiness marker is a failure, not a warning** | Current code logs "демка должна была загрузиться" and proceeds to `exec highlights` anyway, which is how "demo not loaded" becomes "no takes appeared 4 minutes later". | LOW | Remove the proceed-anyway branches for `CS2C_READY` and console connect failure (keep the manual hint text inside the error). Depends on LCH-1/2. |
| LCH-4 | **Console transcript ring buffer** (last N lines from the netcon port plus HLAE stdout/stderr), attached to every failure and flushed to `record.log` in the job folder | The requirement says failure must include "последние строки журнала". CSDM logs HLAE stdout only to a debug logger. | LOW-MED | Needs the console reader thread to feed a bounded buffer. Also records which markers were seen with timestamps. |
| LCH-5 | **Detect the HLAE/AfxHook error window** and window titles of the game process on any launch-phase timeout | CSDM detects a window titled `Error - AfxHookSource` (via `tasklist /v`). The HLAE tracker has "Error - AfxHookSource2" reports. This is the one signal that explains "HLAE hook failed after a CS2 update". | MEDIUM | Prefer `EnumWindows` filtered by our PID over parsing `tasklist /v` output. Dump all top-level window titles of the cs2 process into the failure detail (cheap and very diagnostic). Window title text is localisation-sensitive: match on the stable `AfxHookSource` substring. |
| LCH-6 | **Distinguish "launcher exited" from "game exited"** | HLAE.exe is only a launcher and exits right after starting cs2 (documented in `launch.rs`), so launcher exit code and game liveness are separate facts. CSDM relies on HLAE's exit code, then the game's exit code. | MEDIUM | Depends on TMO-5 (own process identity). Record both: launcher exit code, game exit code (when obtainable). |

#### Timeouts and cancellation (TMO)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| TMO-1 | **Every wait has a deadline and a name** (`Deadline { state, limit }`): launcher spawn, game process appears, console opens, demo ready, first take starts, each take completes, game closes, ffprobe, ffmpeg | CSDM has no timeouts at all and can hang indefinitely; GitHub Actions bounds every step. Project requirement. | MEDIUM | On expiry: fail with state + elapsed + limit + transcript tail (LCH-4) + window titles (LCH-5). Defaults come from existing constants (`LOAD_CAP` 150 s, `EXEC_RETRY` 30 s) and are overridable in settings only for demo-load. Per-take limit derives from the shot's planned duration x factor + constant. |
| TMO-2 | **Liveness by handle, error = unknown, not alive** | `game_alive()` returns `true` when `tasklist` fails. Spawning `tasklist` per second is also slow. | MEDIUM | Open the process handle once; `WaitForSingleObject(h, 0)` / `try_wait`. A query error is counted; N consecutive errors = "dead/unknown" failure. Depends on TMO-5. |
| TMO-3 | **Cancel reacts in seconds in every state**, target <= 3 s to reach "cancelling" and <= ~10 s to a clean state | Project requirement; GitHub Actions budgets about 10 s of grace before the forced kill. CONCERNS.md: a job blocked in `Command::output()` cannot be cancelled. | MEDIUM | One cancel token passed to every wait; all waits poll at <= 250 ms. Replace blocking `Command::output()` (nvenc probe, ffprobe, tasklist/taskkill) with spawn + bounded poll. Needs `Host` trait (TST-1) to test with a fake clock. |
| TMO-4 | **Staged termination of the game**: console `quit` -> wait ~5 s -> terminate our process (or job object) -> verify gone -> short settle delay | GitHub Actions' SIGINT -> SIGTERM -> kill-tree ladder; CSDM's `sleep(2000)` after killing "to avoid Source engine error on next launch". | MEDIUM | Same routine used by cancel, timeout, failure and normal completion, so one code path ends the game. "Verify gone" matters: the next job must not start while file handles on `game/csgo` are still held. |
| TMO-5 | **Own-process identity**: control only the cs2 the app launched | Project requirement; CSDM kills by name and kills the user's own game. | MEDIUM-HIGH | Two layers. (a) Job object with `KILL_ON_JOB_CLOSE` around the HLAE launcher so cs2 (a `CreateProcess` child) inherits membership and dies if the app crashes; `TerminateJobObject` kills only that set. (b) Fallback/identity record: because PRE-5 guarantees zero cs2 at start, the new cs2.exe whose parent chain/creation time matches the launch is "ours"; store `(pid, creation_time)` to defeat PID reuse before any kill. **Needs a spike**: confirm HLAE spawns cs2 via `CreateProcess` so job membership propagates. |
| TMO-6 | **Cleanup on cancel/fail**: game closed, partial takes marked (not deleted), copied demo and installed `cs2c_*`/`highlights.cfg` handled | Project text: "временные файлы помечены". CSDM deletes raw files after a 2 s wait; here recording is expensive, so mark instead (existing `clips_old_<ts>` precedent). | LOW-MED | Write `clips/INCOMPLETE` or a status field in the manifest (TAK-3). The demo copied into `game/csgo` can be gigabytes: remove it when the game is confirmed closed. |
| TMO-7 | **Sequencing hygiene between jobs**: the next job starts only after the previous cs2 is verified gone and a short cooldown elapsed | "5 in a row" fails most often on job 2 if job 1's game is still releasing files (CSDM comment: starting CS while the old one is still exiting gives a Source engine error). | LOW | Part of TMO-4's "verify gone". |

#### Take discovery and validation (TAK)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| TAK-1 | **Expected take set from the plan**: `take0000 .. take{N-1}`, each with `video.mp4` and `audio.wav` | CSDM uses per-sequence folders and an explicit file set. cs2cinema already generates the plan, so it knows N. | LOW | Replace `newest_mtime`/`since` discovery. Old takes are already quarantined to `clips_old_<ts>` before launch, so any `takeNNNN` present is from this run. Detect gaps, duplicates and unexpected extra takes (HLAE numbering shifted) and report which index is wrong. |
| TAK-2 | **Explicit completion per take**: console `TAKE_DONE n` marker as primary, "file exists and size stable" as secondary | OBS emits an explicit stop event with the path rather than making the client scan. Current code already parses `TAKE_DONE`, it just does not require it per take. | MEDIUM | Missing marker for take n after its deadline = `TakeMarkerMissing(n)`, not silent success after a 20 s quiet period. Keep a file-stability check as a second confirmation, not the trigger. |
| TAK-3 | **Take manifest** (`clips/manifest.json`): per take expected duration, status (`pending / recorded / validated / invalid`), file sizes, probed duration, plus plan hash, resolution, CS2 build and HLAE version | The existing `plan.hash` + file-exists shortcut is a proto-manifest. Needed for fail-fast diagnostics, retry (RET-*) and recovery after restart. | MEDIUM | Written atomically (temp + rename) after each take. Source of truth for "what can I skip on retry". |
| TAK-4 | **ffprobe validation of each take before assembly**: has video stream and audio stream, frame count/duration > 0, duration within tolerance of the planned shot duration (e.g. +-max(0.5 s, 5 %)), resolution as configured | Project requirement; CONCERNS.md "Clip-discovery fragility" and "preview recording is not validated". CSDM does existence checks only. | MEDIUM | Validate each take as soon as it completes (fail fast, cheaper than discovering at assembly). Error names the take and the shot: "дубль 7 (убийство в раунде 12): 3.1 с из 8.0 с". Needs a shared ffprobe runner with timeout (also removes one of five duplicated ffmpeg call sites). |
| TAK-5 | **No take is read while HLAE may still be writing it** | Adobe Media Encoder's watch folder had to add a "file still growing" check; HLAE writes the mp4 after the recording ends (noted in `wait_for_clips`). | LOW | Combine TAK-2 marker + size stability + ability to open the file exclusively/for read. |
| TAK-6 | **Final output is written to a temp name and renamed on success** (`highlights.mp4.partial` -> `highlights.mp4`) | A killed ffmpeg must never leave something that looks like a finished montage. | LOW | Also lets the acceptance run (ACC-2) trust `highlights.mp4` existence. |

#### Retry and recovery (RET)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| RET-1 | **Persist stage-at-failure, failure code and attempt number** on the job; keep the log across attempts (separator lines) | Today `start_job` sets `log = ''`, erasing the evidence of the previous failure exactly when the user retries. | LOW | New columns: `failed_stage`, `error_code`, `attempt`. Jenkins creates a new build per restart; here one job with numbered attempts is simpler for a single user. |
| RET-2 | **"Повторить" restarts from the Record stage** without re-download and without re-planning | Project requirement; Jenkins "Restart from Stage" with identical inputs. | MEDIUM | Needs: demo path (already remembered via `store.remember_demo`), and the **plan persisted as a loadable artifact** (not only `plan.json` as output; hash it). Stages become explicit: `Download -> Analyze/Plan -> Record -> Assemble`. Depends on RET-1, TAK-3. |
| RET-3 | **Retry from Assemble when all takes validate** | After an ffmpeg failure the expensive recording must not repeat. The `recorded` shortcut already does half of it by file existence. | LOW | Replace file-existence with manifest + TAK-4 status. Depends on TAK-3/4. |
| RET-4 | **Restart recovery**: on startup, kill our orphaned game (pid + creation time from DB), then classify each `running` job by its stage and set status `interrupted` with the stage and reason, not generic `failed` | Project requirement ("возобновляются или честно помечаются с причиной"). The current `reset_running` writes one fixed message. | MEDIUM | Depends on persisting `(pid, creation_time)` (TMO-5) and the manifest. Minimal honest version: new status `interrupted`, stage-aware message, one-click Retry which resumes at the right stage (RET-2/3). |
| RET-5 | **Failure of one job does not stop the queue**; the queue ends on its own | A "5 matches in a row" run is meaningless if job 2 failing halts job 3-5 without notice, or if the queue needs a click. CSDM's queue starts paused by default. | LOW | Already close to current behaviour; make it an explicit requirement and test. |
| RET-6 | **Retrying never overwrites evidence**: failed attempts keep their takes in `clips_old_<ts>` and their transcript in `record.log.N` | CSDM gives each video a unique ID/folder for this reason. | LOW | Reuses the existing quarantine rename. |

#### Observability (OBS)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| OBS-1 | **Timestamped, stage-tagged, attempt-tagged job log** persisted incrementally | Exists partially (`append_log`). Needed to answer "how long did demo load take / which take hung". | LOW | Add elapsed-in-state to every state change line; log per-phase durations. |
| OBS-2 | **Failure card in the job UI**: code, state, message, hint, last transcript lines, buttons "Повторить" and "Открыть папку" | Core value statement: "объясняет, что именно пошло не так". | LOW-MED | Depends on LCH-2/4. |
| OBS-3 | **Environment snapshot per job** (CS2 build from `steam.inf`, HLAE version, ffmpeg version, NVENC yes/no, free disk, resolution) stored in the job folder and in the manifest | When something breaks after a CS2 update, the first question is "what changed". CONCERNS.md already proposes recording `ClientVersion` per job. | LOW-MED | HLAE version detection is the uncertain part (see Research flags). |
| OBS-4 | **Queue-end summary** in the tray notification: N done / M failed with the first failure reason | The user walks away during 5 matches. The tray already exists. | LOW | |

#### Testability enablers (TST) - required by the project, they unlock the rest

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| TST-1 | **`Host` trait** over process spawn/list/kill/wait, console connect, file system, clock; real Windows impl plus fake | Project requirement. CSDM has no tests beside the launcher; CONCERNS.md: no test drives HLAE/CS2. | MEDIUM-HIGH | The injected **clock** is what makes timeout tests run in milliseconds. Everything above (TMO, LCH, TAK, RET) is unit-testable only through this seam. |
| TST-2 | **Scenario tests with the fake**: launcher fails to spawn, game never appears, HLAE error window, console never opens, demo never ready, script never starts, take marker missing, take truncated, game dies mid-record, liveness query errors, cancel at every state (assert cancel latency), restart with an orphan game and an interrupted job | Project requirement. | MEDIUM | Each scenario asserts both the failure code (LCH-2) and the clean end state (no live fake game, partial takes marked). |
| TST-3 | `pipeline` owns Record and Assemble stages behind `Progress`; app and CLI both call it | Project requirement; removes the app/CLI divergence. | MEDIUM-HIGH | Structural; sequence it before behavioural work or the fixes land twice. |

#### Acceptance run (ACC)

| ID | Feature | Why Expected | Complexity | Notes |
|----|---------|--------------|------------|-------|
| ACC-1 | **Fixed acceptance set**: 5 FACEIT matches on 5 different maps stored as a fixture list (URLs), queued in one action | Project success criterion; reproducible. | LOW | Choose maps with different geometry/loading time; include one long match (overtime) to exercise per-take and load timeouts. |
| ACC-2 | **Machine-checkable pass criteria**: all 5 `done`; each `highlights.mp4` passes ffprobe (video+audio, duration within tolerance of the plan); no cs2.exe left; no manual action recorded; auto-retry count reported separately | "Without manual intervention" needs a definition or it is argued after the fact. | LOW-MED | Depends on TAK-4, TAK-6, OBS-1. |
| ACC-3 | **Fault drills run once by hand before the acceptance run**: cancel in each state; kill app mid-record then restart; end cs2 from Task Manager mid-record; start with a foreign CS2 running; temporarily hide `AfxHookSource2.dll`; unplug ffmpeg from PATH | Proves PRE/TMO/RET work on the real stack; fakes cannot prove HLAE behaviour. | LOW | A checklist, not code. |

---

### Differentiators (valuable for one user, not expected by default)

| ID | Feature | Value Proposition | Complexity | Notes |
|----|---------|-------------------|------------|-------|
| DIF-1 | **Compatibility memory**: store the (CS2 build, HLAE version) pair of the last successful recording; preflight warns when CS2's build changed since then ("CS2 обновился с 1.41.4.1, последняя удачная запись была на ... — если запись не стартует, обновите HLAE") | HLAE/CS2 mismatch is the most documented cause of "game does not start" (CSDM troubleshooting, HLAE version-pairing wiki page, HLAE issues). A machine-readable table of known-good pairs does not exist, so learn it locally. | LOW-MED | Depends on OBS-3. Warn only, never block. Reads `steam.inf` (already parsed in `geometry`). |
| DIF-2 | **Steam update in progress check** (`appmanifest_730.acf` state not "fully installed") before launching | A mid-queue Steam update breaks HLAE and corrupts timing; the HLAE README itself suggests Steam offline mode to dodge updates. | LOW-MED | MEDIUM-LOW confidence on the exact manifest `StateFlags` semantics; verify on the author's machine. |
| DIF-3 | **Circuit breaker for the queue**: after 2 consecutive failures with the same environment-class code (e.g. `HlaeErrorWindow`, `GameNeverAppeared`), pause the queue and say why instead of burning through remaining jobs | If HLAE is broken by an update all queued jobs will fail identically; each costs minutes of launch timeouts. | LOW | Depends on LCH-2 codes classified as `environment` vs `transient` vs `job-specific`. |
| DIF-4 | **Bounded auto-retry (max 1) for transient, launch-class failures only** (`GameNeverAppeared`, `ConsoleNeverOpened`, `DemoNeverReady`, `GameDiedEarly`), after full cleanup (TMO-4), new port, cooldown; every retry visible in the log and counted in ACC-2 | Real-world game launches are flaky; one silent retry is often the difference between 4/5 and 5/5. | MEDIUM | **Decision to flag for requirements:** instrument first, decide after the first runs show the failure rate. Never auto-retry preflight failures, cancellations, or `TakeTruncated` more than once. Depends on LCH-2, TMO-4, RET-1. |
| DIF-5 | **Re-record only missing/invalid takes** (partial resume) | A CS2 launch + demo load is the most expensive part (up to 150 s). If take 11 of 12 is bad, repeating 1-10 is waste. | HIGH | Needs the script generator to emit a subset (`highlights.cfg` for shot K..M) and the manifest to merge takes from two runs. Defer until full-stage retry (RET-2) proves insufficient. |
| DIF-6 | **Progress-aware (stall) timeouts** in addition to absolute ones: "no new console line and no growth of the take file for N s" | A long demo legitimately takes longer; an absolute cap is either too loose to catch hangs or too tight for big demos. | MEDIUM | Complements TMO-1. Needs the transcript feed (LCH-4). |
| DIF-7 | **Adaptive timeouts from history**: p95 of recorded demo-load and per-take times x margin | Removes hand-tuned constants. | MEDIUM | Depends on OBS-1 phase durations stored in SQLite. Only worthwhile after enough history exists. |
| DIF-8 | **Screenshot of the game window at failure** | Shows dialogs/black screens that no marker reveals. | MEDIUM | LOW confidence: capturing DirectX/Vulkan windows via GDI often returns black. Window-title dump (LCH-5) gives most of the value for a fraction of the cost. |
| DIF-9 | **"Copy diagnostics" button** (job error, state, transcript tail, environment snapshot) | Makes asking for help (HLAE Discord, issue) one click. | LOW | Depends on OBS-2/3. |
| DIF-10 | **Pause queue after current job** | CSDM has pause/resume; useful when the user wants to play. | LOW | Cooperates with PRE-5 (user starts own CS2 -> next job would fail preflight anyway). |
| DIF-11 | **Per-job attempt history view** | See what changed between attempt 1 and 2. | LOW-MED | Depends on RET-1/6. |

---

### Anti-Features (deliberately NOT built)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| **Kill CS2 by image name** (CSDM style, including before launch) | Simplest way to get a clean start | Kills the player's own match; explicitly forbidden by the project. | PRE-5 refuses to start; TMO-4/5 control only our process. |
| **Proceed when a readiness marker is missing** (current "должна была загрузиться") | Hides a slow load | Converts "demo did not load" into "no takes appeared" minutes later, the exact undiagnosable state. | LCH-3: marker timeout is a named failure. |
| **Unbounded or infinite retry loops** ("retry until it works") | Looks like it satisfies "without manual intervention" | Hides root causes, burns GPU/time, makes the 5-in-a-row result meaningless. | DIF-4: max 1, transient codes only, counted and visible. |
| **Auto-installing/auto-updating HLAE or ffmpeg** (CSDM does both) | Removes setup friction | Needs release parsing, download, unpack, rollback; the app's problem is detecting and explaining, and the author owns the machine. | PRE-3/DIF-1 detect and explain; user updates HLAE. |
| **Auto-downgrade HLAE to match a CS2 build** | The HLAE wiki lists the pairs | Brittle, third-party binaries, no machine-readable source. | DIF-1 warning with a pointer to the HLAE wiki page. |
| **Screen-capture fallback (OBS/desktop capture) when HLAE fails** | A second route to a video | Different quality/timing, no per-shot determinism, a second pipeline to maintain; not what the product is. | Fail fast with a clear reason. |
| **Generic plugin system / pluggable recorder backends** | "Support other recorders" | No second backend exists; extra abstraction without a user. | The `Host` trait exists for testing only, not for extensibility. |
| **Multi-machine render farm, parallel CS2 instances, job priorities/scheduling** | Faster batches | CS2 supports one recording instance at a time (project constraint); single user. | Serial queue (already). |
| **Generic workflow/DAG engine, external queue (Redis, Temporal)** | "Proper job system" | Four fixed stages; SQLite already persists jobs. | Explicit stage enum + SQLite columns. |
| **Remote API / webhooks / Discord notifications** | Notifications while away | Single user on own PC; scope creep. | Tray notification (OBS-4). |
| **Windows service / daemon that survives closing the app** | Queue runs headless | Different lifecycle, installer, permissions; the project excludes installer/release. | Recovery on next start (RET-4). |
| **Heavy telemetry / crash upload** | Learn failure rates | Single user; local log + manifest are enough. | OBS-1/3 data in SQLite. |
| **OCR or vision-based game state detection** | Detect "main menu vs in demo" | Fragile, slow; markers via the console port already work. | Console markers + window titles + process facts. |
| **Silent deletion of partial takes on failure** | Tidy disk | Recording is expensive, evidence is lost; CSDM deletes but its jobs are cheap to redo. | Mark and quarantine (TMO-6, RET-6); manual cleanup button later if disk pressure appears. |

---

## Feature Dependencies

```
TST-1 Host trait + fake clock
    ├──enables──> TST-2 scenario tests (for every feature below)
    └──requires──> TST-3 pipeline owns Record/Assemble (so tests hit one code path)

PRE-1 preflight report ──requires──> TST-1 (process list, fs)
    ├── PRE-2 run first
    ├── PRE-3 tooling, PRE-4 Steam, PRE-5 no foreign CS2, PRE-7 inputs
    └── PRE-6 disk (needs the plan: second tier)

LCH-1 state machine ──requires──> TST-1
    ├── LCH-2 typed failures ──requires──> LCH-1
    │       ├── LCH-3 marker miss = failure
    │       ├── DIF-3 circuit breaker, DIF-4 auto-retry
    │       └── OBS-2 failure card
    ├── LCH-4 transcript ring buffer ──enhances──> LCH-2, TMO-1, DIF-6
    └── LCH-5 error window / titles ──requires──> TMO-5 (know which PID)

TMO-5 own-process identity ──requires──> PRE-5 (zero cs2 at start makes identification unambiguous)
    ├── TMO-2 handle-based liveness
    ├── TMO-4 staged termination ──enables──> TMO-3 cancel, TMO-6 cleanup, TMO-7 hygiene
    └── RET-4 orphan kill on restart (needs pid + creation_time persisted)

TMO-1 deadlines ──requires──> LCH-1 (deadline per state), TST-1 (fake clock)

TAK-1 expected set ──requires──> plan (exists)
    ├── TAK-2 explicit completion per take
    ├── TAK-4 ffprobe validation ──requires──> shared ffprobe runner with timeout
    └── TAK-3 manifest ──requires──> TAK-1, TAK-4
            ├── RET-2 retry from Record
            ├── RET-3 retry from Assemble
            ├── RET-4 restart recovery
            └── DIF-5 partial re-record

RET-1 attempts/failure columns ──requires──> LCH-2
    └── RET-2 ──requires──> persisted loadable plan + remembered demo path

OBS-3 environment snapshot ──enables──> DIF-1 compatibility memory

ACC-2 pass criteria ──requires──> TAK-4, TAK-6, OBS-1;  ACC-1/3 need all behavioural features done

DIF-4 auto-retry ──conflicts──> Anti-feature "unbounded retry" (hence cap + visibility)
TMO-5 job object ──conflicts-with-nothing, but──> assumption must be spiked (HLAE spawns via CreateProcess)
```

### Dependency Notes

- **Everything is gated by the `Host` seam (TST-1/TST-3).** Without it deadlines and cancellation cannot be tested without CS2, and CONCERNS.md lists exactly that as the top gap.
- **PRE-5 is a prerequisite of TMO-5.** "Our cs2" is trivially identifiable only if no other cs2 existed at launch; this is also why refusing (not killing) a foreign CS2 is the correct and the cheaper design.
- **TMO-5 is a prerequisite of TMO-2, TMO-4, LCH-5, LCH-6, RET-4.** Per-process identity is the single most load-bearing low-level piece.
- **TAK-3 (manifest) is the join point** between validation, retry and restart recovery; build it before RET-2/3/4.
- **RET-2 needs a loadable plan.** Today the plan is recomputed from the demo; either persist/deserialise `plan.json` (preferred, with hash) or accept a re-parse and satisfy only "no re-download". This is the one requirement-wording decision to settle.
- **DIF-4 vs "without manual intervention":** if auto-retry is excluded, acceptance depends on raw launch reliability; if included, report retries separately so the number stays honest.

---

## MVP Definition

### Launch With (this milestone's must-haves)

In dependency order:

- [ ] **TST-1 / TST-3** `Host` trait, fake, clock; Record+Assemble moved behind `pipeline` - enabler for everything
- [ ] **PRE-1..PRE-7** preflight report, run first - cheapest visible win; independent of the launch rewrite (can start in parallel with TST-1)
- [ ] **LCH-1..LCH-4, LCH-6** state machine, typed failures, marker miss = failure, transcript buffer
- [ ] **TMO-1..TMO-7** deadlines, handle-based liveness, cancel everywhere, staged termination, own-process identity, cleanup, inter-job hygiene
- [ ] **LCH-5** window-title dump on launch timeouts (cheap, very diagnostic)
- [ ] **TAK-1..TAK-6** expected take set, explicit completion, manifest, ffprobe validation, temp-then-rename output
- [ ] **RET-1..RET-6** attempts and failure columns, retry from Record/Assemble, restart recovery with `interrupted` status, queue continues
- [ ] **OBS-1..OBS-4** log, failure card, environment snapshot, tray summary
- [ ] **TST-2** scenario suite
- [ ] **ACC-1..ACC-3** fault drills, then the 5-match run

### Add After Validation (once the acceptance run shows real failure modes)

- [ ] **DIF-1** compatibility memory - after OBS-3 data exists
- [ ] **DIF-3** circuit breaker - cheap; add if any environment-class failure repeats
- [ ] **DIF-4** bounded auto-retry - decide from the measured launch failure rate
- [ ] **DIF-2** Steam-update check, **DIF-9** copy diagnostics, **DIF-10** pause queue

### Future Consideration

- [ ] **DIF-5** partial re-record - only if full-stage retry proves too slow in practice
- [ ] **DIF-6/7** stall and adaptive timeouts - only after enough phase-duration history
- [ ] **DIF-8** window screenshot - only if title dumps leave unexplained failures

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| TST-1/3 Host seam + pipeline ownership | HIGH (enabler) | MEDIUM-HIGH | P1 |
| PRE-1..7 Preflight | HIGH | LOW-MED | P1 |
| LCH-1/2/3 state machine, typed failures, marker miss = failure | HIGH | MEDIUM | P1 |
| LCH-4 transcript buffer | HIGH | LOW-MED | P1 |
| LCH-5 window titles / error window | HIGH | MEDIUM | P1 |
| TMO-1 deadlines | HIGH | MEDIUM | P1 |
| TMO-2 handle liveness | HIGH | MEDIUM | P1 |
| TMO-3/4 cancel + staged termination | HIGH | MEDIUM | P1 |
| TMO-5 own-process identity (+ job object spike) | HIGH | MEDIUM-HIGH | P1 |
| TMO-6/7 cleanup + inter-job hygiene | MEDIUM-HIGH | LOW-MED | P1 |
| TAK-1/2/4 expected set, completion, ffprobe | HIGH | MEDIUM | P1 |
| TAK-3 manifest | HIGH | MEDIUM | P1 |
| TAK-5/6 stable read, temp-then-rename | MEDIUM | LOW | P1 |
| RET-1/2/3 attempts, retry from Record/Assemble | HIGH | MEDIUM | P1 |
| RET-4 restart recovery | HIGH | MEDIUM | P1 |
| RET-5/6 queue continues, evidence kept | MEDIUM | LOW | P1 |
| OBS-1..4 | MEDIUM-HIGH | LOW-MED | P1 |
| TST-2 scenarios | HIGH | MEDIUM | P1 |
| ACC-1..3 | HIGH | LOW-MED | P1 |
| DIF-1 compatibility memory | MEDIUM-HIGH | LOW-MED | P2 |
| DIF-3 circuit breaker | MEDIUM | LOW | P2 |
| DIF-4 bounded auto-retry | MEDIUM-HIGH | MEDIUM | P2 (decision) |
| DIF-2, DIF-9, DIF-10, DIF-11 | MEDIUM | LOW | P2-P3 |
| DIF-5 partial re-record | MEDIUM | HIGH | P3 |
| DIF-6, DIF-7, DIF-8 | LOW-MEDIUM | MEDIUM | P3 |

**Priority key:** P1 = needed to hit "5 matches in a row without manual steps", P2 = add when the data says so, P3 = later.

---

## Competitor / Peer Feature Analysis

| Capability | cs-demo-manager (video) | OBS + obs-websocket automation | Jenkins / GitHub Actions (job runners) | HandBrake / Adobe Media Encoder queues | Our approach |
|-----------|-------------------------|-------------------------------|-----------------------------------------|----------------------------------------|--------------|
| Preflight | Yes, typed per-prerequisite errors (HLAE, FFmpeg, Steam, demo); disk only a UI warning | Client checks connection and scene by itself | Runner labels / tool installers | Source/presets validated on add | Full structured report (PRE-1..7) incl. foreign-CS2 refusal and disk floor |
| Failure taxonomy | ~6 launch error classes | `outputState` values incl. stopping/stopped, success only by event | Exit code + step | Per-item status | 10+ named launch/record codes mapped to states (LCH-2) |
| Timeouts | None around the game | Client-defined | Job and step `timeout-minutes` | None / user stops | Per-state deadlines, later stall-aware (TMO-1, DIF-6) |
| Cancellation | AbortSignal; kills by name, 2 s wait, deletes raw files | `StopRecord` returns the file path | SIGINT -> 7.5 s -> SIGTERM -> 2.5 s -> kill tree, 5 min cap | Cancel item | Console `quit` -> terminate our process/job -> verify gone (TMO-4) |
| Process ownership | By name (kills user's game) | N/A (OBS is the owner) | Process tree of the step | N/A | Pid + creation time, job object (TMO-5) |
| Completion signal | Process exit, then glob | Explicit event with `outputPath` | Exit code | Item status | Console marker + manifest + ffprobe (TAK-2..4) |
| Retry | User re-adds | Client-defined | Jenkins "Restart from Stage" with identical inputs | Adobe "reset status" after fixing cause | Stage-granular retry from persisted inputs (RET-2/3) |
| Crash recovery | Not documented | Client-defined | Jenkins durability / pipeline resume (not researched in depth) | HandBrake queue persistence requested historically | Orphan kill + `interrupted` status + stage-aware resume (RET-4) |
| Observability | Debug logger | OBS log | Full console log per run | Activity log | Job log + failure card + `record.log` + env snapshot (OBS-1..3) |
| Tests around process control | Sequence builders only (from file listing) | - | - | - | Fake `Host`, scenario suite (TST-1/2) |

---

## Research flags (verify with a spike before or during planning)

| Question | Why it matters | Confidence now |
|----------|----------------|----------------|
| Does HLAE spawn cs2.exe via `CreateProcess` so that a job object around the launcher captures it? | Decides whether TMO-5 can use `KILL_ON_JOB_CLOSE`, or must rely on pid + creation-time identity only | LOW (Microsoft doc is clear that `CreateProcess` children join the job; HLAE internals for CS2 are not documented) |
| How does HLAE.exe behave on failure: exit code, error dialog, stdout? | LCH-2 `LauncherExitedNonZero` and `HlaeErrorWindow` thresholds | MEDIUM (CSDM treats exit != 0 and the "Error - AfxHookSource" window as HLAE errors) |
| Is console `quit` reliable to close CS2 in every state (loading screen, error dialog)? | TMO-4 stage 1; the terminate fallback covers it, but the grace period needs a value | MEDIUM |
| How to read the HLAE version programmatically (file version resource vs. folder name)? | OBS-3 / DIF-1 | LOW (no documented method found; the HLAE wiki pairing page does not say how to detect versions) |
| `steam.inf` `ClientVersion` for the installed CS2 | OBS-3 / DIF-1 (already parsed in `crates/geometry`) | MEDIUM-HIGH |
| `appmanifest_730.acf` `StateFlags` meaning for "updating" | DIF-2 | LOW-MEDIUM |
| Per-take duration tolerance and the extra tail/head HLAE adds around a shot | TAK-4 thresholds, avoids false `TakeTruncated` | LOW (calibrate on the first real runs) |
| Typical demo-load time on large FACEIT demos | TMO-1 defaults | MEDIUM (existing 150 s cap is the author's own experience) |

## Sources

Primary, read directly (HIGH):
- cs-demo-manager video guide, recording workflow, requirements, queue, troubleshooting: https://cs-demo-manager.com/docs/guides/video
- cs-demo-manager source (files read in full): `src/node/counter-strike/launcher/start-counter-strike-with-hlae.ts`, `src/node/video/generation/generate-video.ts`, `.../assert-video-generation-is-possible.ts`, `.../get-sequence-raw-files.ts`, `.../move-hlae-files-to-output-folder.ts`, `src/node/counter-strike/kill-counter-strike-processes.ts`, `.../assert-counter-strike-is-running.ts`, launcher `errors/*.ts` (names and messages); repository tree: https://github.com/akiver/cs-demo-manager (raw files under https://raw.githubusercontent.com/akiver/cs-demo-manager/main/src/node/)
- obs-websocket protocol (`RecordStateChanged`, `StopRecord`): https://github.com/obsproject/obs-websocket/blob/master/docs/generated/protocol.md
- Win32 job objects (inheritance, `KILL_ON_JOB_CLOSE`, `TerminateJobObject`, nesting): https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
- Jenkins "Restart from Stage": https://www.jenkins.io/doc/book/pipeline/running-pipelines/
- HLAE wiki, CS2 build to HLAE release pairing: https://github.com/advancedfx/advancedfx/wiki/Old-HLAE-versions-for-specific-CS2-versions
- cs2cinema code and planning docs: `.planning/PROJECT.md`, `.planning/codebase/CONCERNS.md`, `crates/app/src/launch.rs`, `crates/app/src/jobs.rs`, `crates/store/src/lib.rs`

Secondary, search summaries (MEDIUM/LOW):
- GitHub Actions cancellation sequence (SIGINT, 7.5 s, SIGTERM, 2.5 s, kill tree, 5 min cap) and the signal-handling demo: https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-cancellation, https://github.com/ringerc/github-actions-signal-handling-demo
- HLAE issue titles ("Error - AfxHookSource2", "CS2 ... update broke HLAE", "Launching CS2 using HLAE resulted in a crash") and README advice on Steam offline mode: https://github.com/advancedfx/advancedfx (issue list snapshot via https://opencollective.ecosyste.ms/projects/17595/issues)
- Adobe Media Encoder failed-item reset and watch-folder stability check (forum answers): https://community.adobe.com/questions-506/how-to-restart-failed-queue-994788
- HandBrake queue persistence request: https://forum.handbrake.fr/viewtopic.php?p=27931
- Allstar / FACEIT highlights (cloud contrast): https://support.faceit.com/hc/en-us/articles/17380248744860-Highlights-FAQ, https://leetify.com/blog/linking-allstar/

---
*Feature research for: reliability of a game-driven recording queue (cs2cinema, HLAE + CS2 + ffmpeg, Windows)*
*Researched: 2026-10-10*
