# Architecture Research — v1.1 «Сервер и Telegram-бот»

**Domain:** Telegram bot + job server + pull-based remote runner, added to an existing Rust workspace (egui app + `cs2-cli` over `pipeline::job::run_job`)
**Researched:** 2026-10-11
**Confidence:** HIGH for integration points (read from `pipeline::job`, `Progress`, `store`, `app::queue`, `app::jobs`); MEDIUM for protocol and lease design (standard pull-queue patterns, not yet exercised on this code); MEDIUM for Telegram local Bot API behaviour (confirmed from the tdlib README; file-system sharing details need a spike); library choices for server/bot are left to STACK.md.

## Summary of decisions

| # | Decision | Why (one line) |
|---|----------|----------------|
| D1 | Three new crates: `wire` (serde protocol types only), `server` (lib + bin `cs2-server`), `runner` (client + loop). No new dependency edges into `pipeline`/`host` from `server`. | Server must build on Linux fast and never drag in `demo`/`source2-demo`/`host`. |
| D2 | Server SQLite is the source of truth for *what was asked, who asked, queue order, cancel intent, what to tell the user*. Runner state is the source of truth for *what is on disk / in the game and what the outcome was*. Reconciled by a `hello` handshake. | Either side can be offline for hours; neither may lose a recording. |
| D3 | Runner is **pull-only over HTTPS**: `claim` (long-poll), `progress` (doubles as heartbeat), `finish`. Server never connects to the runner. Cancel and lease info ride back in the responses. | NAT-friendly, one transport, no WebSocket reconnect state machine. |
| D4 | Leases with **epoch fencing**, but lease loss is soft: it re-queues the job for display and fairness, it never kills local work, and the same runner can re-take it by presenting the epoch. | A network blip must not throw away a 10-minute recording. |
| D5 | In the **app**, a remote job is a normal row in the local `store` (`JobSource::File(<fetched demo>)` + new `external_ref` column) executed by the existing `app::queue` thread. In **`cs2-cli runner`**, the same loop runs `run_job` directly (headless executor). Both are plugged into `runner` via an `Executor` trait. | App gets crash recovery, `Interrupted`/«Повторить», auto-retry, pause policy, cards and journal for free; CLI stays store-free. |
| D6 | Progress is forwarded by a `Progress` **tee** (typed `stage`/`event`/`log`), not by polling the store log. Terminal result comes from the queue's *settled* hook (after retry policy), not from a raw `failed` row. | Avoids string-parsing and the `failed → auto-retry → queued` race. |
| D7 | Files move over plain HTTP: server→runner `GET` with `Range` + ETag=sha256; runner→server `HEAD` offset + streaming `PUT` of the remainder + sha256 verified at `finish`. Telegram side: local Bot API `--local` gives a filesystem path (move, don't copy) and accepts `file://` uploads. | Multi-hundred-MB demos and videos survive drops without restarting from zero. |
| D8 | Runner claims **only when it is idle**; local queue has priority (claim only if `next_queued()` is empty and nothing runs and the queue is not paused). One remote job in flight. Upload is sequential in v1.1 (no overlap with the next recording). | Preserves "one recording at a time" with zero changes to the recording policy. |

## Standard Architecture

### System overview

```
                       Telegram cloud
                            │  (users upload demos here, not to us)
                            ▼
┌───────────────────────────────────────────────────────────── server machine ─┐
│  telegram-bot-api --local (2 GB, getFile → absolute path, file:// upload)    │
│        ▲ HTTP (localhost)                       shared volume / same host    │
│        │                                                 │                   │
│  ┌─────┴──────────── cs2-server (crate `server`) ─────────┴────────────────┐ │
│  │  bot/        whitelist, /player /status /cancel /retry, status message  │ │
│  │  core/       PURE state machine + lease rules (no IO, unit tested)      │ │
│  │  api/        runner-facing HTTP: hello claim progress demo result finish │ │
│  │  db/         rusqlite, one writer (actor thread), migrations            │ │
│  │  blobs/      incoming demos, results, .part files, retention sweeper    │ │
│  │  delivery/   result → Telegram (retry, file_id kept), failure → chat     │ │
│  └───────────────────────────────▲─────────────────────────────────────────┘ │
└──────────────────────────────────┼───────────────────────────────────────────┘
                                   │ HTTPS, runner → server only (bearer token)
                         `wire` types: serde JSON + raw bytes for files
                                   │
┌──────────────────────────────────┼───────────── Windows machine (CS2+HLAE) ──┐
│  crate `runner`                  │                                           │
│   ┌──────────────────────────────┴─────────────┐                             │
│   │ Link (ureq client, backoff, hello)         │                             │
│   │ Loop: Idle → Fetch → Execute → Upload → Report                           │
│   │ State file `runner_state.json` (atomic)    │                             │
│   │ ProgressTee: Progress → wire::Progress     │                             │
│   └───────────────┬────────────────────────────┘                             │
│        Executor trait                                                         │
│   ┌───────────────┴──────────────┐     ┌───────────────────────────────┐     │
│   │ app: LocalQueueExecutor      │     │ runner: HeadlessExecutor      │     │
│   │ add_job(external_ref) →      │     │ pipeline::job::run_job(       │     │
│   │ app::queue thread → run_job  │     │   WinHost, JobSpec, tee)      │     │
│   └──────────────────────────────┘     └───────────────────────────────┘     │
│   existing: pipeline::job, host, store, app::queue (one recording at a time)  │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Crate layout and dependency edges

```
wire        serde, serde_json            (no workspace deps)           NEW
server      wire, rusqlite, <async http + telegram client per STACK>   NEW  (lib + bin cs2-server)
runner      wire, pipeline, host, ureq, sha2, serde_json               NEW
app         + runner                                                   MODIFIED
cli         + runner                                                   MODIFIED
```

- Name it `wire`, not `proto`: `crates/demo/proto/` already holds protobuf files and `proto` reads as "protobuf" in this repo.
- `wire` has its **own** `Stage`/`Phase`/`FailClass`/`JobState` enums; `runner` maps `pipeline::Stage` → `wire::Phase`. `server` must not depend on `pipeline` (it would pull `demo`, `source2-demo`, `host`, `geometry`). Fail codes travel as the string from `FailCode::as_str()` plus the already-computed hint, so the server needs no copy of the `FailCode` table.
- `server` is **not** built on `store`: `store` is the desktop app's schema (queue columns, FACEIT cache, demo registry). Server has its own DB file and migrations list, following the same `MIGRATIONS: &[&str]` + `PRAGMA user_version` pattern for consistency.
- `server` exposes a lib so `runner` integration tests can start it in-process on a loopback port (dev-dependency) with a fake chat sink; the Telegram part is behind a `Chat` trait.
- Async (if STACK.md picks tokio/axum) stays inside `server`. `wire` and `runner` remain synchronous (`ureq`, `std::thread`) as the workspace constraint requires. Keep rusqlite behind a single DB actor thread (channel in, result out) so no `Mutex<Connection>` is held across `.await`.
- Release: `cs2-server` is a separate artifact (Linux build in CI); `release.yml` ships the Windows zip as before.

### Component responsibilities

| Component | Owns | Does not own |
|-----------|------|--------------|
| `wire` | Message structs, `PROTOCOL_VERSION`, `JobState` + `is_terminal()`, path constants, size/limit constants | Any logic beyond (de)serialisation helpers |
| `server::core` | Job state machine, lease/epoch rules, queue ordering, cancel rules — pure functions `apply(job, event, now) -> Result<(job, Effects)>` | IO, Telegram, HTTP |
| `server::bot` | Whitelist, commands, receiving a file (`getFile` → move to blobs), the single editable status message per job | Job truth (reads/writes via core) |
| `server::api` | Runner auth, long-poll claim, progress/heartbeat, ranged demo download, resumable result upload, finish | User-facing text |
| `server::delivery` | Send video / failure text, retry with backoff, remember Telegram `file_id` | State transitions |
| `server::blobs` | Layout, atomic rename, checksum, retention sweeper, free-space guard | — |
| `runner::Link` | HTTP calls, backoff, `hello` reconcile, transfer with resume | Recording policy |
| `runner::Loop` | The runner state machine, `runner_state.json`, claim gating, heartbeat thread | How a job is executed |
| `runner::Tee` | `impl Progress`: forwards to an inner `Progress` (app `JobReporter` / console) and to the uplink; ORs remote cancel into `cancelled()` / `force_cancelled()` | Persistence |
| `Executor` (trait) | "run this assigned task to a settled outcome" + readiness | Network |

## Job lifecycle and state machine

### Server states (persisted in `server.db`)

```
 receiving ──► queued ──claim──► assigned ──first progress──► running ──► uploading ──► done ──► (delivery: pending→sent|failed)
     │            ▲  │              │                            │            │
     │            │  └─ cancel ─► cancelled                       │            │
     │            └──── lease expired (soft, reversible) ◄────────┴────────────┘
     └─ fetch error ─► failed                      any leased state ──finish(Failed)──► failed
                                                   any non-terminal ──cancel──► cancelling(flag) ──► cancelled
```

| State | Meaning | Entered by | Leaves by |
|-------|---------|-----------|-----------|
| `receiving` | Bot is pulling the file from Telegram (local Bot API downloads on `getFile`) | Message with document | file moved into `blobs/` → `queued`; error → `failed` (code `UPLOAD_RECEIVE`) |
| `queued` | Waiting for the runner; also the place a job returns to on lease loss | receive done; lease expiry; manual `/retry` | `claim` → `assigned`; `/cancel` → `cancelled` |
| `assigned` | Lease granted (`epoch`++), runner is fetching the demo / prechecking | `claim` | first `progress` → `running`; lease expiry → `queued` |
| `running` | Runner reports `phase` (Fetch, pipeline `Stage`s, Record with take n/m) | `progress` | `finish`; lease expiry |
| `uploading` | Local job done, result being streamed to server | `progress{phase:Upload}` | `finish(Done)` after server verifies size+sha256 |
| `done` | Result stored on server | `finish(Done)` | terminal; `delivery` is a separate column |
| `failed` | Carries `fail_code`, `fail_class`, `fail_state`, `hint`, `message`, `tail`, `elapsed_ms` — verbatim from the runner | `finish(Failed)`, receive error | `/retry` creates **a new attempt** (back to `queued`) |
| `cancelled` | User intent honoured | `/cancel` | terminal |

- `cancel_requested: none | soft | force` is a **flag**, not a state (mirrors the store's `Cancelling` status and the app's second-press "force" in `CancelToken`). Second `/cancel` sets `force`.
- Delivery is orthogonal: `delivery ∈ {pending, sent, failed}` plus `tg_file_id`. A `done` job with `delivery=failed` is retried by the delivery worker and by `/resend`. Never block the runner on Telegram.
- Retry of execution failures is **the user's decision** (`/retry`), not the server's: the runner already applied its own policy (`QueuePolicy` auto-retry once for launch-class failures, pause on repeated environment failure). The server auto-requeues only on lease loss.

### Mapping to existing states

| Server | Local `store::JobStatus` (app mirror) |
|--------|----------------------------------------|
| `assigned` (fetching) | no local row yet (fetch happens before `add_job`) |
| `assigned`/`running` | `Queued`/`Running` |
| cancel_requested | `Cancelling` |
| `failed` | `Failed` (+ `fail_*` columns → `wire::Failure`) |
| `cancelled` | `Cancelled` |
| runner restart mid-job | `Interrupted` → auto `requeue_resume` (see Resumption) |
| `uploading`/`done` | local `Done`, remote sync tracked in `runner_state.json` |

## Where job state lives, and what happens when things break

**Server DB (authoritative for intent):** `users`, `jobs`, `job_events` (capped ring of log lines per job for `/log` and failure tails), `runners` (id, name, `token_sha256`, `last_seen`, version, `status: idle|busy|paused(reason)`), and nothing for transfer progress — the size of the `.part` file *is* the upload offset.

**Runner state (authoritative for execution):**
- App: the existing SQLite `jobs` row (status, `out_dir`, `result`, `fail_*`, journal) — **plus** one new nullable unique column `external_ref TEXT` (`"srv:<job_id>"`), added as migration 6. This makes "enqueue the remote job" idempotent (`INSERT ... ON CONFLICT(external_ref)`), and lets recovery find the local job for a server job.
- Both app and CLI: `runner_state.json` in the data dir, written atomically (temp + rename), one entry per server job the runner currently holds: `{server_job_id, epoch, phase: Fetching|Executing|Finished{outcome}|Uploading|Reported, demo_path, demo_sha256, local_job_id?, out_dir?, result_path?, result_sha256?}`. This is the **outbox**: a finished outcome stays here until `finish` is acknowledged. Headless mode has no store, so this file *is* its recovery state; `out_dir` there lets it call `pipeline::gamedir::kill_orphans`/`recover` on restart exactly like `jobs::recover_after_crash`.
- Write order for idempotency: persist `Fetching` → download+verify demo → persist `Fetched` → `add_job`/start → persist `Executing{local_job_id}`.

| Event | What happens |
|-------|--------------|
| **Runner offline before claim** | Jobs stay `queued`; `runners.last_seen` ages; bot answers `/status` with "раннер не в сети с HH:MM, перед вами N задач". No user-visible failure. |
| **Runner offline during Execute** | Local recording continues (it never needs the server). Tee buffers: latest phase/take + last 200 log lines; terminal outcome goes to the outbox. Server lease expires after TTL → job returns to `queued` with note `lease_lost`; chat gets one message after a 2-minute grace ("раннер не отвечает, задача ждёт"). |
| **Runner reconnects** | `hello{holding:[{job_id,epoch}]}`. Server answers per job: `keep` (epoch matches and job not terminal → re-lease, no new epoch), `abandon` (cancelled / unknown / epoch superseded → runner cancels local job, drops upload), or `cancel`. Then outbox flush (`finish`, pending upload). |
| **Connection lost mid-upload** | Retry loop: `HEAD result` → offset → `PUT` remainder. Never restarts from zero unless the server's `.part` is gone. |
| **Connection lost mid-download** | Runner keeps `demo.dem.part`; resumes with `Range: bytes=N-`; verifies sha256 against the value from the assignment before use. |
| **Runner process/PC restarts mid-record** | See Resumption. |
| **Server restarts** | All state in SQLite; leases are absolute timestamps; runner sees connection errors, backs off, re-`hello`s. In-flight `.part` files survive. Long-poll claims are simply re-issued. |
| **Server DB lost / restored from old backup** | Runner `hello` presents unknown jobs → `abandon` → runner closes them locally (result discarded) and the bot's users get "задача потеряна" only if the server knows them. Accept; document backup of `server.db`. |
| **Stale runner writes after being superseded** | `epoch` mismatch → `409 Conflict`; runner drops the job from the outbox. Protects future multi-runner without building it now. |

## Leases and heartbeats

- `lease_until` is an absolute server-clock timestamp. Runner clocks are never trusted.
- Heartbeat = `POST /v1/jobs/{id}/progress` every **10 s** (even with no new events), sent from a **dedicated uplink thread**, never from the pipeline thread. TTL = **45 s** (≥4 missed heartbeats). Idle runners heartbeat through the `claim` long-poll (25 s) plus `runners.last_seen`.
- A sweeper on the server (every 10 s) moves expired `assigned|running|uploading` jobs to `queued` with `lease_lost=true`, `attempt` unchanged, `epoch` unchanged.
- **Soft loss is reversible:** a `progress`/`finish`/`hello` with the *current* epoch re-leases the job even if the sweeper already requeued it, as long as nobody else claimed it (claim bumps the epoch). With one runner this means a dead runner never "holds" anything (state is honest: `queued`, runner offline) and a recovered one carries on without redoing work.
- The server never auto-fails a job because the runner is silent. A silent runner only changes what the bot says. Admin can `/cancel`. (Personal system, one runner; a hard "give up after N hours" can be a config knob later.)
- If the runner's heartbeat fails for longer than TTL it keeps executing; it only stops when it receives `cancel`/`abandon` or the local pipeline ends.

## Mapping a remote job to `JobSpec`

`JobSpec` and `JobConfig` already do not depend on `Store` or `Settings`, so no change in `pipeline` is required.

| `JobSpec` field | Remote value |
|-----------------|--------------|
| `job_id` | local store id (app) or a monotonically increasing local counter persisted in `runner_state.json` (headless) — **not** the server id |
| `attempt` | local attempt counter (store `start_job` / headless counter); server `attempt` is a separate display number |
| `record` | `true` |
| `resume` | `false` for a fresh task; `true` on restart-recovery (`Interrupted` → `requeue_resume`) |
| `source` | `DemoSource::File(<out_root>/remote/<stem>-srv<job_id>.dem)` for uploaded demos; `DemoSource::Faceit(link)` for a chat-pasted FACEIT link (optional feature; runner's own `faceit_key` is used, the server never holds it) |
| `hero_steam` / `hero_name` | from the user's pinned player: `/player` stores `(steam64?, nickname)`; `find_player` already tries steam id first (non-zero) then case-insensitive name/substring, so a nickname-only pin works with `hero_steam = 0` |
| `known_demo` | `None` for File source (already local) |
| `last_good` | runner-local (`store.last_good()` in app; headless keeps it in `runner_state.json` or passes `None`) |
| `out_dir` | `None` fresh; from the stored job on resume |
| `cfg: JobConfig` | **always runner-local** (HLAE/CS2/ffmpeg paths, fps, frame, GPU, deadlines margin). The server sends no paths and no config in v1.1; `Assignment.options` exists (empty, `#[serde(default)]`) for later knobs (e.g. `order`) |

Details that matter:
- **Unique labels.** `out_dir` is `out_root/console_name("{label}_{player}")` and `label` for a File source is the file stem. Name the fetched file `<sanitized stem>-srv<id>.dem` so two jobs with the same original name never share an output folder (and `clips/manifest.json` is never reused across users).
- **Sanitise anything from chat** before it touches the file system or HLAE scripts: Telegram file names (strip path separators, cap length, ASCII-fold), and the `/player` nickname (length cap; `renderer::console_safe`/`console_name` already protect scripts, but reject control characters at the bot).
- Local store `label` for remote rows: `"@<user> · <orig name>"` so the existing jobs page shows where it came from without UI changes; `kind = Record`.

## Forwarding progress

`runner::Tee<'a> { inner: &'a dyn Progress, uplink: &Uplink }` implements `Progress`:

| `Progress` callback | Tee action |
|---------------------|------------|
| `stage(s)` | `inner.stage(s)`; `uplink.phase(Phase::Pipeline(map(s)))` |
| `event(&record::Event)` | `inner.event(e)`; `State{Take, take:(n,m)}` / `TakeDone{n,m}` → `uplink.take(n,m)`; `State{state,..}` → record sub-state; `Fact(s)` → log line |
| `log(msg)` | `inner.log(msg)`; push to ring buffer (cap 200 lines, each ≤ 300 chars) |
| `cancelled()` | `inner.cancelled() \|\| remote_cancel.load()` |
| `force_cancelled()` | `inner.force_cancelled() \|\| remote_force.load()` |
| `artifact(a)` | `inner.artifact(a)`; `OutDir` is recorded in `runner_state.json` |

- The uplink thread owns the heartbeat: every 10 s, or immediately (debounced to ≥ 2 s) on phase/take change, it posts `Progress{epoch, seq, phase, take, log_since_last, runner_status}` and applies the reply `{lease_until, cancel: none|soft|force}`. Progress is **lossy by design** (only the latest state matters); terminal outcomes are **reliable** (outbox).
- In the app, the `runner` closure in `jobs.rs` (`runner(settings, host) -> Runner`) wraps `rep` in the tee when `job.external_ref` is set (lookup in a small `RemoteRegistry: server_job_id → Uplink`). No change to `JobReporter` itself. Typed events avoid parsing the `"записан дубль N/M"` strings (those must stay unchanged for `status.rs`).
- Terminal outcome in the app: a new optional hook `QueueConfig.on_settled: Option<Arc<dyn Fn(i64) + Send + Sync>>` called at the end of `Worker::run_one` **after** the policy verdict and only when the verdict is not `AutoRetry`. The runner then re-reads the row and sends `finish`. This is the only reliable "settled" signal; reading `Failed` from the store directly can observe the state between `jobs::finish` and `requeue_auto`.
- Failure payload built from the store row: `fail_code`, `fail_state`, `fail_hint`, `fail_tail`, `fail_elapsed_ms`, `error`, plus `wire::FailClass` from `pipeline::record::failure::classify`. Headless: from `RunError.fail` directly. Bot text = Russian template around `hint`: `«Не удалось записать ролик: {message}. {hint} (код {code})»`.
- Assemble percentage: skip in v1.1 (stage "Склейка" is enough); revisit if users want a progress bar.

## Coexistence with the local queue (one recording at a time)

- The app's `cs2cinema-queue` thread stays the **only** thing that starts recordings. Remote jobs enter through `store.add_job` like local ones, so exclusivity and FIFO are inherited.
- **Claim gating** (checked by the runner before every claim, and reported in heartbeats as `runner_status`):
  `idle` only if: no running job, `store.next_queued()` is `None`, queue not paused, no unreported outbox entry, free disk ≥ threshold (demo size + headroom), `pipeline::precheck` last result not failing for environment reasons. Otherwise `busy` or `paused(reason)`; `PauseReason` text (already Russian with a hint) is forwarded so the bot can say why nothing moves — this is the core value applied to the chat.
- Local-first policy: local jobs queued by the author are never starved because the runner doesn't claim while `next_queued()` is `Some`. A remote job already in the store is ahead of later local ones by id (FIFO); accepted.
- Cancelling a remote card in the app UI → job `Cancelled` locally → settled hook → `finish(Cancelled, by="runner")`; bot says "отменено на стороне раннера". The queue pauses as it does today (`PauseReason::Cancelled`), so the runner stops claiming until the author resumes. No special case.
- Headless (`cs2-cli runner`): single loop, claim gating = "not executing". Auto-retry policy is the app queue's; headless v1.1 reports the first failure and lets the user `/retry`. (Moving `QueuePolicy` out of `app` into a shared crate is a possible later refactor; not needed now.)

## Cancellation from the chat

1. `/cancel [id]` (or inline button) → server `core`: `receiving|queued` → `cancelled` immediately, files deleted; leased states → `cancel_requested=soft`, status message becomes "Отменяю…".
2. Runner learns on the next heartbeat (≤ 10 s): `Tee.cancelled()` flips → pipeline's `check(p)?` and Host waits stop within seconds (v1.0 guarantee) → `finish(Cancelled)`.
3. Second `/cancel` → `force` → `force_cancelled()` → close the game immediately (existing D-12 path).
4. If the runner is offline, the server finalises `cancelled` after 60 s anyway (user intent wins) and remembers a `directive: abandon` delivered at the next `hello`; the runner then cancels or discards whatever it still holds (including a finished result and pending upload).
5. In the app mirror: `abandon`/`cancel` map to `CancelToken`-equivalent via the tee flags for a running job, and to `store.cancel_job(id)` for a job still queued locally.

## Resumption after runner restart (reuse of Interrupted / Retry)

Startup order in the app (extends the existing sequence): `recover_after_crash` (kills orphan game, cleans game folder, `reset_running` → `Interrupted`) → `queue::start(paused = Interrupted…)` → `runner::start`.

1. Runner loads `runner_state.json`, lists store rows with `external_ref` and status `Interrupted`.
2. For each (there is at most one): `store.requeue_resume(id)` then `queue.resume()` — the existing «Повторить» path, which resumes from `record.json` without download/parse/plan. This is done **before and independent of** connecting to the server (recording needs no network).
3. If the resumed run fails with `RETRY_NO_DATA` (no/corrupt `record.json`), the runner does one automatic `store.requeue(id)` («Заново») — it still has the fetched demo locally. A second failure is reported as a normal failure.
4. Server learns about it through `hello` (`keep`) and a log line "возобновлено после перезапуска раннера". `attempt` on the server increments; the bot status message is edited, not re-sent.
5. If a local job is already `Done` and `runner_state` says `Finished/Uploading`, the runner skips execution and goes straight to upload/finish.
6. Headless: same logic over `runner_state.json` + `pipeline::job::resume`.

Queue policy tweak (small, in `queue.rs`): the `Interrupted` pause must not wedge an unattended runner. Keep the pause for local jobs; for jobs with `external_ref` the runner clears it right after step 2 (via the existing `Queue::resume`). No change to `QueuePolicy` itself.

## File transfer design

### Telegram ↔ server (local Bot API, `--local`)
- Incoming: on a document message the bot calls `getFile`; in local mode the Bot API server downloads from Telegram and returns an **absolute path on its own disk** (tdlib README). `server` **moves** the file into `blobs/incoming/<job_id>/demo.dem` (`rename` when on the same filesystem — free for 2 GB; copy+fsync+delete across volumes), verifies size against `Document.file_size`, computes sha256, and stores it. This requires `telegram-bot-api` and `cs2-server` to share a filesystem (same host or shared volume/user). Document this deployment requirement; it is the main deployment constraint.
- Outgoing: `sendVideo` with `file://<abs path>` (local mode supports local path/URI upload, up to 2000 MB) with `supports_streaming=true`; keep the returned `file_id` so `/resend` needs no disk. Fallback `sendDocument` if the container is rejected for video. If result > 2000 MB: fail delivery with a clear message (and keep the file for manual retrieval).
- Telegram renders status updates by **editing one message** per job; throttle edits to stage/take changes and ≥ 5 s apart.

### Server ↔ runner (HTTP, streaming, resumable, checksummed)

```
GET  /v1/jobs/{id}/demo         Range: bytes=N-     → 206, ETag: "<sha256>", Content-Length
HEAD /v1/jobs/{id}/result                          → Upload-Offset: <bytes on disk>, Upload-Length if known
PUT  /v1/jobs/{id}/result       Upload-Offset: N, Content-Length: rest, X-Epoch: e  → 204 (appends to result.part at N; N must equal current size else 409 with correct offset)
POST /v1/jobs/{id}/finish       {epoch, outcome: Done{size, sha256, shots, secs} | Failed{…} | Cancelled}
```
- Server verifies on `finish(Done)`: `.part` size == `size`, sha256 == `sha256` (re-hash on finalize, ~seconds per GB), then `rename` to `results/<job_id>/highlights.mp4` and `uploading → done`. Mismatch → `409 {need_upload_from: offset}` or `422` and the runner restarts the upload from 0.
- Runner hashes while writing the demo (`sha2` already in `app`), hashes the result once before upload, and records both in `runner_state.json`.
- One long streaming request per attempt (not per-chunk): fewer round trips; on error the client re-`HEAD`s and continues. Timeouts must be **idle/stall timeouts** (e.g. 60 s without a byte), never a global deadline (a 2 GB transfer legitimately takes minutes).
- Auth on every call: `Authorization: Bearer <runner token>`; the server stores only `sha256(token)`. Provision with `cs2-server runner add <name>` (prints token once). TLS is required if the server is on the open internet (reverse proxy such as Caddy, or a private network like Tailscale); plain HTTP only on a trusted link. Tokens must never be logged (redact in `ureq` error text and in `Debug` impls, as `JobConfig` already does for the FACEIT key).

## Server storage layout and cleanup

```
<data>/
  server.db                      # jobs, users, runners, job_events
  blobs/
    incoming/<job_id>/demo.dem[.part]     # moved from telegram-bot-api's dir
    results/<job_id>/highlights.mp4[.part]
    tmp/
```
- Free-space guard: refuse new `receiving` below a threshold (reply "на сервере нет места") and cap per-user queued jobs (e.g. 3) so friends cannot starve the queue.
- Retention sweeper (hourly): demo deleted `demo_ttl` (default 24 h) after the job becomes terminal; result deleted `result_ttl` (default 24 h) after `delivery=sent` (the `file_id` remains valid for `/resend`); `.part` files older than 24 h deleted; terminal `jobs` rows kept (cheap, history for `/history`), `job_events` pruned after 30 days.
- A demo is never deleted while its job is non-terminal (retry/«Заново» may need it, even though the runner keeps its own copy).
- Also delete the original in telegram-bot-api's working dir after the move (rename does this implicitly).
- Runner side (hardening phase): remote demos and finished job folders pruned by count (keep last N, default 5) after `finish` is acknowledged; raw takes are the big files, `highlights.mp4` + `record.json` are enough to keep for «Повторить».

## Protocol surface (v1, JSON unless noted)

| Endpoint | Purpose | Notes |
|----------|---------|-------|
| `POST /v1/runner/hello` | `{protocol, name, app_version, status, holding:[{job_id,epoch}]}` → `{server_version, directives:[{job_id, keep\|abandon\|cancel}], poll_secs}` | Version mismatch → `426` with a Russian message "обновите раннер" |
| `POST /v1/runner/claim` | `{status}`; long-poll ≤ 25 s → `Assignment{job_id, epoch, lease_until, demo:{size,sha256,orig_name}\|faceit_link, player:{steam64?,name}, options:{}, requested_by}` or `204` | Only idle runners call it; claim = transition `queued → assigned` + `epoch++` in one transaction |
| `POST /v1/jobs/{id}/progress` | `{epoch, seq, phase, take?, log[], status}` → `{lease_until, cancel}` | Doubles as heartbeat; `seq` idempotency; 409 on stale epoch |
| `GET/HEAD/PUT` demo/result | see File transfer | |
| `POST /v1/jobs/{id}/finish` | terminal outcome | Idempotent; repeat returns the same 200 |

`#[serde(default)]` on every optional field; unknown enum variants deserialise to `Unknown` so a newer server does not crash an older runner; `PROTOCOL_VERSION` bumped only for breaking changes (lockstep builds make this easy, but app and server are updated separately in practice).

## Server data model (sketch)

```
users(tg_id PK, name, role{admin|member}, player_steam, player_name, created_at)
jobs(id, user_id, chat_id, status_msg_id, state, cancel_requested, attempt, epoch, runner_id, lease_until, lease_lost,
     source_kind{file|faceit}, demo_path, demo_size, demo_sha256, orig_name, faceit_link, player_steam, player_name,
     phase, take_n, take_m, result_path, result_size, result_sha256, shots, secs,
     fail_code, fail_class, fail_state, fail_hint, fail_message, fail_tail, fail_elapsed_ms,
     delivery, tg_file_id, created_at, updated_at, finished_at)
job_events(job_id, seq, ts, kind, text)           -- capped
runners(id, name, token_sha256, last_seen, version, status, pause_reason)
```
Whitelist: first admin id from config file; others via `/allow <id>` / `/deny <id>`. Unknown users get one reply showing their own Telegram id (so a friend can send it to the author) and are then ignored.

## Patterns to Follow

### Pattern 1: Pure core, IO at the edges (matches `QueuePolicy`)
**What:** All state transitions and lease decisions are `fn apply(&Job, Event, now) -> Result<(Job, Vec<Effect>), Reject>`; HTTP handlers and the bot translate in/out and execute `Effect`s (notify, delete blob).
**When:** Everything in `server::core`. **Trade-off:** one extra layer, but lease/cancel races (the hard part) become table-driven unit tests with a virtual clock — same discipline as `FakeHost` virtual time.

### Pattern 2: Outbox for terminal facts, best-effort for progress
**What:** Terminal outcome persisted in `runner_state.json` before the first send; re-sent until acked (idempotent endpoint). Progress is overwritten, never queued.
**When:** Any message whose loss would lose a result. **Trade-off:** two delivery classes, but no unbounded queues while offline.

### Pattern 3: Epoch fencing + reversible lease loss
**What:** Claim bumps `epoch`; writes with other epochs are rejected; lease expiry only re-queues.
**Trade-off:** slightly more state than a plain TTL, but it makes the "runner back after 5 minutes" case a non-event and prepares multi-runner without building it.

### Pattern 4: Executor trait as the only seam to execution
```rust
pub trait Executor: Send + Sync {
    /// Current readiness for claim gating (Idle / Busy / Paused(reason)).
    fn readiness(&self) -> Readiness;
    /// Held jobs after a restart (for `hello`), already auto-resumed internally.
    fn held(&self) -> Vec<Held>;
    /// Run an assigned task to a settled outcome; returns when settled (not when merely queued).
    fn run(&self, task: &Assigned, uplink: &Uplink) -> Settled; // Done{result} | Failed(Failure) | Cancelled
}
```
App implements it over `store` + `Queue` + the settled hook; `runner` ships `HeadlessExecutor` over `pipeline::job::run_job`. Tests use a scripted executor against an in-process `server`.

## Anti-Patterns to Avoid

### Anti-Pattern 1: Server depends on `pipeline`/`store`
Pulls protobuf, `source2-demo`, Win32-gated code into a Linux daemon and couples release cadence. Use `wire` DTOs and strings.

### Anti-Pattern 2: Deriving "finished" from the store row
`Failed` can be momentary (auto-retry). Use the queue's settled hook / `run_job` return value.

### Anti-Pattern 3: Server pushes work to the runner (inbound connection, WebSocket as the only channel)
Breaks NAT-friendliness and adds a reconnect state machine; polling with long-poll is simple, debuggable with `curl`, and the latency (≤ 10 s for cancel) is irrelevant here.

### Anti-Pattern 4: Treating lease expiry as failure
It would kill good recordings on a Wi-Fi blip and spam users. Expiry changes presentation and queue position only.

### Anti-Pattern 5: Global request timeouts on transfers; whole-file buffering
Use stall timeouts and streaming (`Read`/`Write` copy loops with fixed buffers); never `Vec<u8>` a 2 GB demo (on either side, nor `Body::to_vec` in `ureq`).

### Anti-Pattern 6: Sending config or paths from server to runner
The runner is a trust boundary: it executes HLAE/CS2 and writes into the game folder. Server supplies only demo, player and (later) a whitelist of option enums.

### Anti-Pattern 7: Reusing the user's file name as a path
Path traversal and `out_dir` collisions. Name by job id, sanitise the stem.

## Scaling Considerations

| Scale | Adjustment |
|-------|-----------|
| Author + a few friends, one runner (target) | Single process, SQLite, FIFO. Nothing else needed. |
| Several runners | Already shaped for it: `claim` picks the oldest `queued`, epoch fencing, per-runner tokens; add `runners` selection and a capability field. |
| Public bot | Out of scope (whitelist by design). |

First bottleneck is **server disk** (2 GB demos × queue depth + results), then **uplink bandwidth** between machines — hence retention TTLs, per-user queue caps and the free-space guard rather than any scaling work.

## Integration Points

### External services

| Service | Pattern | Notes |
|---------|---------|-------|
| Telegram (via local `telegram-bot-api`) | Long-poll updates; `getFile` returns local path; `sendVideo` by `file://` path | Bot must be `logOut`-ed from the cloud API before switching to a local server; server and bot-api must share a filesystem; handle absolute `file_path` (client libs that assume a URL break) |
| FACEIT | Unchanged, runner-side only | Key stays on the runner |
| TLS/ingress for runner API | Caddy or private network | Not in code; document |

### Internal boundaries: new vs modified vs untouched

| Crate | Status | Change |
|-------|--------|--------|
| `wire` | **NEW** | DTOs, enums, constants |
| `server` | **NEW** | lib + `cs2-server` bin; admin subcommands `runner add`, `submit <demo> --player …` (test hook that lets phases 1–2 run end-to-end without Telegram), `serve` |
| `runner` | **NEW** | `Link`, `Loop`, `Tee`, `Executor`, `HeadlessExecutor`, transfer, `runner_state.json` |
| `store` | MODIFIED (small) | Migration 6: `jobs.external_ref TEXT UNIQUE NULL`; `add_remote_job` / `job_by_ref`; `Job.external_ref` field |
| `app` | MODIFIED | `queue.rs`: `on_settled` hook + read-only `idle()` snapshot for gating; `jobs.rs`: tee wrapper in `runner` closure, `LocalQueueExecutor`; `settings.rs`/`settings_page.rs`: server URL, token, "режим раннера" toggle, connection state; `app.rs`/`main.rs`: start runner thread after queue; `jobs_page.rs`: show label/origin, runner status chip; tray tooltip shows connection |
| `cli` | MODIFIED | `cs2-cli runner --server URL --token …` (reuse the `JobConfig` builder used by `record`); USAGE text; must keep `tests/pipeline.rs` green |
| `pipeline` | UNTOUCHED | `run_job`, `JobSpec`, `Progress`, `FailCode`/`classify` already sufficient. (If anything: make `FailClass` `serde`-able — avoid; map in `runner`.) |
| `host`, `demo`, `cs2`, `highlights`, `director`, `positions`, `geometry`, `renderer`, `faceit` | UNTOUCHED | |
| workspace / CI | MODIFIED | add members; CI builds+tests `server`/`wire`/`runner` on Linux and Windows; `runner` tests that need `WinHost` are `cfg(windows)` (use `FakeHost`); release workflow ships `cs2-server` as a separate artifact |

## Suggested build order (phases)

Dependencies: `wire` → `server::core/db/api` → `runner` → (bot ∥ app integration) → hardening. The Telegram bot is deliberately *not* first: the riskiest parts are the protocol, leases and resumability, which can be tested without Telegram via `cs2-server submit`.

1. **Protocol + server core.** `wire` crate; `server` lib: DB + migrations, pure state machine with a virtual clock, lease sweeper, runner auth/tokens, `hello/claim/progress/finish`, blob layout, ranged download, resumable upload, `submit` admin command. Tests: transition table, lease loss/re-lease, epoch fencing, cancel races, upload resume/sha mismatch, all in-process over loopback.
2. **Runner crate + `cs2-cli runner`.** `Link`, backoff, `hello` reconcile, `Tee`, `HeadlessExecutor` on `run_job`, `runner_state.json` outbox, transfer with resume. Tests: scripted executor + in-process server; `FakeHost` scenarios (launch failure → `finish(Failed)` with code+hint); kill/restart loop; network drop mid-upload. **First end-to-end on real CS2:** `submit` → runner → `highlights.mp4` on server (this is the live recording check v1.0 deferred).
3. **Telegram bot.** `Chat` trait + real client against local Bot API: whitelist, receive demo (`getFile` move), `/player`, `/status`, `/cancel`, `/retry`, status-message editing, delivery of video/failure with hints, `/resend`; `telegram-bot-api` + server deployment recipe (compose). Can start in parallel with phase 2 once phase 1 lands, using a fake runner.
4. **App runner mode.** `store` migration, `LocalQueueExecutor`, queue `on_settled` hook and gating, settings page, restart recovery (`Interrupted` → auto-resume), tray/UI status. Needs phase 2's `runner` crate. Highest regression risk for existing behaviour → keep changes minimal and covered by existing `queue.rs` tests plus new ones for remote jobs.
5. **Hardening and ops.** Retention sweeper and disk guards (server + runner), per-user caps, offline/restart drills (kill runner mid-record, unplug network mid-upload, restart server mid-job, cancel while runner offline), token rotation, log redaction audit, deployment docs (TLS/Tailscale, backups), version-mismatch UX.

**Ordering rationale:** the protocol and lease semantics are the load-bearing design and cheapest to fix before UI exists; headless runner before app integration gives a real end-to-end recording earlier and keeps the app (priority per project decisions) untouched until the runner library is proven; bot work is mostly independent glue and parallelisable.

**Research flags for phases:**
- Phase 3: needs a short spike — local `telegram-bot-api` file layout (is the `getFile` path readable by the server user, does the file persist after move, same-filesystem rename), `sendVideo` by `file://` with ~1 GB, Telegram client library support for absolute `file_path` and custom API URL.
- Phase 4: needs careful reading of `queue.rs` tests before adding the hook; standard otherwise.
- Phase 1–2: standard patterns, unlikely to need more research; the resumable PUT with `ureq` 3 streaming bodies and idle timeouts deserves a 1-hour spike.

## Open questions

- Server OS and TLS/ingress choice (affects deployment doc, not code).
- Whether the bot should accept FACEIT links (needs only `DemoSource::Faceit`; cheap, no upload) — recommended as a stretch item in phase 3.
- Whether to overlap result upload with the next recording (throughput gain; adds a second concurrent state in the outbox). Deferred past v1.1.
- Whether `/player` stores nickname only or also resolves steam64 from the demo's player list on first use (nickname + `find_player` fallback is enough for v1.1).

## Sources

- Codebase (read): `crates/pipeline/src/job.rs` (`run_job`, `JobSpec`, `DemoSource`, `Outcome`), `crates/pipeline/src/lib.rs` (`Progress`, `Stage`, `Artifact`, `find_player`, `RunError`), `crates/pipeline/src/record/mod.rs` (`Event`), `crates/pipeline/src/record/failure.rs` (`FailCode`), `crates/store/src/lib.rs` (`JobStatus`, `NewJob`, `requeue_*`, `reset_running`, migrations), `crates/app/src/queue.rs` (`Queue`, `QueueView`, `Runner`, `Worker::run_one`), `crates/app/src/jobs.rs` (`job_spec`, `runner`, `recover_after_crash`, `CancelToken`, `JobReporter`), `.planning/PROJECT.md`, `.planning/codebase/ARCHITECTURE.md`, `STRUCTURE.md` — HIGH
- tdlib/telegram-bot-api README (local mode: 2000 MB uploads, upload by local path / `file://`, `getFile` returns absolute local path, `logOut` before switching): https://cdn.jsdelivr.net/gh/tdlib/telegram-bot-api@master/README.md — HIGH for the capability, MEDIUM for file-persistence details (spike)
- Files appear on the Bot API server's disk only after `getFile`: community report (Railway forum), https://station.railway.com/questions/telegram-bot-api-local-mode-not-using-mo-314249de — LOW/MEDIUM, verify in the phase 3 spike

---
*Architecture research for: server + Telegram bot + remote runner on the cs2cinema workspace*
*Researched: 2026-10-11*
