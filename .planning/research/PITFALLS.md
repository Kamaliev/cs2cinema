# Pitfalls Research: v1.1 «Сервер и Telegram-бот»

**Domain:** Adding a Telegram bot + server (other machine) + outbound-connecting remote runner to an existing local Rust app (Windows runner records CS2 via HLAE; local self-hosted `telegram-bot-api --local` for 2 GB files; whitelist of author + friends; one shared queue).
**Milestone:** v1.1 (subsequent). Recording-reliability pitfalls (HLAE/CS2/ffmpeg/Job Object/take validation) live in `.planning/milestones/v1.0-research/PITFALLS.md` and are NOT repeated here; this file only covers the NEW surface and the seams where it touches the old one.
**Researched:** 2026-10-11
**Overall confidence:** MEDIUM-HIGH. Telegram local-mode facts and limits are HIGH (official Bot API docs and `tdlib/telegram-bot-api` README, fetched). Code-level findings are HIGH (read from this repo). Runner/server protocol, SQLite and deployment items are HIGH/MEDIUM (established engineering practice, not product-specific docs). A few telegram-bot-api operational details (token inside local file paths, bind address flag, cleanup behaviour, 10-minute `logOut` lockout) are MEDIUM because the README is thin; each is tagged and should be confirmed by a 30-minute spike with a real `telegram-bot-api` before the Bot phase is planned in detail.

Confidence tags: **[HIGH]** official docs fetched or verified in this repo's code; **[MEDIUM]** community knowledge / sound inference, verify in a spike; **[LOW]** plausible hypothesis.

**Phase vocabulary.** The roadmap does not exist yet, so phases are named by theme. Suggested order: **Protocol** (shared `protocol` crate: DTOs, wire `FailCode`, versioning, job state machine) -> **Server core** (server DB, lease/queue, runner HTTP API, storage, auth) -> **Bot** (local bot-api, ingestion, whitelist, `/player`) -> **Runner** (client mode, claim, download, run via `pipeline::job`, upload outbox, reconnect) -> **Delivery** (progress edits, result/failure messages) -> **Hardening** (input sanitising, quotas, fuzz, redaction) -> **Deploy/E2E** (TLS, systemd/service, soak, first live run). Note: several Hardening items (S7, S4) must land in the phase where the input is FIRST accepted, not at the end. Each pitfall says so.

---

## Critical Pitfalls

### Pitfall 1: Player nickname / map name / title from an untrusted demo is injected into the CS2 `highlights.cfg` (and ffmpeg graph)

**What goes wrong:**
`renderer::render` writes demo-derived strings into the generated config as `//` comment lines: the map name (`// Сгенерировано cs2-cli. Карта: {map}`) and `shot.title` (`// Шот N -> takeNNNN: {title}`, which is built from player names) (`crates/renderer/src/lib.rs`, `render`, around lines 162-207). A comment ends at the first newline. A demo whose player name or map string contains `\n` (or `\r`, U+2028) turns the rest of the "title" into a live console line when CS2 `exec`s the cfg: `quit`, `disconnect`, `connect <ip>` (with `-insecure`, see v1.0 Pitfall 7), `alias`/`bind`, `host_writeconfig`, `con_logfile`, HLAE `mirv_*` commands. The same strings also reach: ffmpeg `drawtext` (`text()` escapes `'`, `\`, `:` but not newlines, control chars or bidi overrides), generated `assemble.bat`/`.sh`, `out_dir`/job folder names if anyone builds them from a nick, the job title in the DB, log lines, and the Telegram caption/error text. In v1.0 the input was the author's own FACEIT demos; in v1.1 it is files sent through Telegram by friends (or by anyone who compromises a friend's account), and the nick additionally arrives as free text from the `/player` command. [HIGH, code-level; exploit impact on CS2 console MEDIUM]

**Why it happens:**
Strings were trusted because the only source was FACEIT. `console_safe()` and `console_name()` exist, but they guard paths and demo labels (mirv_cmd arguments), not free text that is "only" in comments. Comments feel inert.

**How to avoid:**
- One `sanitize_label(&str) -> String` applied once at the trust boundary (where `cs2::Match` is built from `demo::Parsed`, and where `/player` input is accepted): strip all control chars (C0, C1, DEL), U+2028/2029, bidi controls (U+202A-202E, U+2066-2069), zero-width joiners if you do not need them; collapse whitespace; hard-cap to ~32 chars. Keep the raw name only inside `Match` if needed for `find_player` matching, never for output.
- Better: remove demo-derived text from the cfg entirely. Comments carry only the shot index and `takeNNNN`; titles live in `plan.json` and the ffgraph text files. Zero injection surface instead of a filter.
- ffmpeg: `drawtext=textfile=...:expansion=none` with UTF-8 text files (already recommended in v1.0 Pitfall 18) so no demo text is ever inside a filter string or a `.bat`.
- Add a hostile-fixture test table that runs through `renderer::render` and asserts the output has no line outside the expected grammar: names `"a\nquit"`, `"x\r\nexec evil"`, `"; quit"`, `"\" ; disconnect"`, `"%{pid}"`, `"'"`, `"\\"`, 4000 chars, RTL override, emoji, lone surrogate-like invalid UTF-8 after lossy decode. Assert every cfg line starts with `//`, `echo CS2C`, `exec`, `mirv_`, `demo_`, `engine_`, or a known whitelist of prefixes.
- Treat `/player` text and Telegram `file_name` the same way (Pitfall 9).

**Warning signs:**
A cfg line in `out_dir` that does not start with a known prefix; CS2 console printing `Unknown command` for words from a nick; the game quitting/disconnecting mid-job on one specific demo.

**Phase to address:** Runner for the sanitiser and cfg change (before the first friend demo is accepted); Hardening adds the hostile-fixture suite. Do NOT postpone to the end.

---

### Pitfall 2: Poison demo: the parser crashes, hangs or eats RAM on the runner, and the job is re-assigned in a loop

**What goes wrong:**
The demo parser (`demo` + pinned `source2-demo =0.5.9` + prost) now processes files nobody on the team has looked at. Failure classes: panic (`unwrap`, slice index, integer overflow in debug), huge allocation from a length-prefixed field, infinite/very slow loop, stack overflow (not catchable), and a decompression bomb: `demo::read_demo` calls `zstd::stream::decode_all` on the whole file with no output cap and reads the entire file into a `Vec<u8>` first (`crates/demo/src/lib.rs`). A 100 KB zstd file can expand to many GB. [HIGH, code] This runs inside the runner process, which is the author's GUI app on the machine that has Steam, the FACEIT key and the CS2 install. A crash kills the app (or the job thread), the connection drops, the server's lease expires, the same job is handed to the same runner after restart, and it crashes again: a poison-pill crash loop that also blocks the shared queue for every friend.

**Why it happens:**
"It parses FACEIT demos" was the only test corpus. Failure accounting in v1.0 assumed environment problems (retry once on startup failure), not hostile content. Lease-based queues redeliver by design.

**How to avoid:**
- Hard limits on both sides: server rejects uploads over `max_demo_bytes` (config, e.g. 1.5 GB compressed) before `getFile`; runner caps decompressed output (`zstd::Decoder` + `Read::take(MAX_DEMO_RAW)`, e.g. 2 GB, error `DEMO_TOO_LARGE`) and streams instead of `read_demo`-ing the whole file where possible. Reject by magic bytes, not extension: `PBDEMS2\0`, zstd `28 B5 2F FD`, gzip `1F 8B`; anything else -> `DEMO_BAD_FORMAT` with a Russian hint.
- Crash isolation: run `analyze`/parse in a child process (`cs2-cli` already exists) with a timeout and a Job Object memory limit (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`; the Job Object infrastructure is built in v1.0), or at least `catch_unwind` at the job-thread boundary plus the caps above. Stack overflow and OOM kill only a child.
- Poison-pill accounting on the server: `attempts` counter per job; a job whose lease expired while in Parse/Plan, or whose runner reported a crash, is retried at most once, then `failed` with code `DEMO_PARSE_CRASH`. Runner reports "I crashed last time on job X" in its hello (persisted in the runner DB before starting a stage).
- Cheap mutation test in `cargo test`: take a small fixture demo, truncate at ~50 offsets, flip bytes at ~200 positions, run `demo::parse` inside `catch_unwind` with a thread timeout, assert no panic/timeout. (Full `cargo fuzz` is optional later.) Plus a zstd-bomb test (tiny input, cap trips).
- Do not parse demos on the server at all; the server only validates magic + size. Keeps the attack surface off the always-on host.

**Warning signs:**
Same job id appearing in consecutive runner "started" events; runner process restarting in a loop; memory spike on the Windows PC during the Parse stage; a job stuck in `leased` with `attempt >= 2`.

**Phase to address:** Runner (child-process/limits, attempt reporting), Server core (attempt cap), Hardening (mutation tests). Caps on `read_demo` go in Runner phase, before any friend demo.

---

### Pitfall 3: Duplicate or lost job assignment (non-atomic claim, no lease fencing, stale runner keeps working)

**What goes wrong:**
Classic queue bugs, made worse by a single CS2 instance and 5-30 minute jobs:
1. Claim implemented as `SELECT ... WHERE status='queued'` then `UPDATE`: two requests (a retried claim after a lost response, or runner reconnect racing the old connection's handler) both get the job.
2. Runner loses the connection mid-recording (NAT, Wi-Fi, server restart); the server's lease expires and re-queues; the runner reconnects, claims "a" job (the same one) while the previous recording is still running locally, and now two pipelines compete for the single CS2 or the first finishes and uploads a result for a lease that was revoked.
3. Claim response is lost in transit: the server believes the job is assigned, the runner never saw it; job sits `leased` until expiry with nobody working on it.
4. Server restarts and immediately requeues every leased job although the runner is alive and mid-recording.
5. Heartbeats come from the job thread and stop during a long blocking step (a 3-minute upload, a frozen egui frame), so a healthy runner loses its lease.

**Why it happens:**
"One runner" feels like it removes concurrency. It does not: the same runner talks over reconnecting connections and the server has multiple handler threads.

**How to avoid:**
- Atomic claim: a single `BEGIN IMMEDIATE` transaction that picks the oldest queued job and sets `status='leased', lease_token=<random>, lease_expires=now+L, attempt=attempt+1`. Return the token. Never read-then-write across statements (Pitfall 20).
- **Fencing token on every runner call** (progress, heartbeat, download, upload, complete, fail). Server rejects mismatches with 409 `LEASE_LOST`; the runner treats `LEASE_LOST` exactly like a user cancel: stop CS2 via the existing cancel path, discard output, do not upload.
- Idempotent claim: runner sends a `claim_request_id`; replaying it returns the same job/lease. Hello message carries `current_job_id + lease_token` (persisted in the runner DB); on reconnect the server re-attaches the lease instead of re-queuing.
- Runner refuses to claim while it has any non-terminal remote job (recording, assembling, uploading). Remote jobs enter the SAME single worker queue as local jobs; CS2 remains one-at-a-time.
- Lease length > longest silent period, and heartbeat from a dedicated thread (independent of the job thread and the UI), every 10-15 s, with the lease extended by heartbeat AND by upload progress. Server startup grace: do not expire leases for `grace` (e.g. 2 min) after boot so the runner can re-announce.
- Terminal states are sticky; late `complete` after `cancelled`/`failed` is rejected and the late upload deleted.

**Warning signs:**
Two `started` log lines for one job; `attempt` > 1 without a runner restart; "result for unknown lease"; CS2 "already running" preflight failures right after a network blip; jobs stuck in `leased` with a connected runner.

**Phase to address:** Protocol (state machine, fencing token in DTOs), Server core (atomic claim, expiry, startup grace), Runner (single-worker refusal, heartbeat thread, re-attach on hello).

---

### Pitfall 4: Result lost after upload failure (upload and completion are not an outbox)

**What goes wrong:**
The runner spends 20 minutes recording and assembling, then the 500 MB upload dies at 90% (Wi-Fi blip, reverse-proxy timeout, server restart, 413 from a proxy). The runner marks the job `failed`, cleans `out_dir` (v1.0 teardown deletes demo copies and may garbage-collect old attempts), or the app is closed. The friend gets "ошибка" for a video that exists. Variants: upload succeeded but the HTTP response was lost, so the runner retries and the server stores it twice or rejects as duplicate; the server accepted the bytes but crashes before the DB commit; truncated upload accepted as complete.

**Why it happens:**
Upload is treated as the last line of the pipeline instead of a durable state with its own retry policy.

**How to avoid:**
- State split on the runner: `recorded -> assembled -> uploading -> uploaded -> (delivered)`. `highlights.mp4` is deleted/GC-eligible ONLY after the server ACKs with matching size + SHA-256. Upload failures never produce `failed`; they produce `uploading(retry n)`, with a new `FailCode::UploadFailed` only after a long deadline (e.g. 6-24 h) and still keep the file.
- Outbox on startup: before claiming new work, the runner scans its DB for assembled-not-uploaded jobs and resumes the upload (reuses the v1.0 `interrupted` + resume machinery).
- Resumable, verified transfer (Pitfall 15): chunked PUT with offset or `Content-Range`, `HEAD`/`GET /uploads/{id}` returns received bytes; final `complete` carries `sha256` and `size`; server verifies while streaming, writes `.part`, fsyncs, atomically renames, and only then commits `status=uploaded` in one transaction.
- `complete` is idempotent per `(job, lease_token, sha256)`: replay returns 200 with the same body.
- Server separates `job_state` from `delivery_state` (Telegram send can fail independently); the stored video stays until delivery is confirmed or the retention TTL passes.
- The lease is kept alive during upload; the server must not requeue a job whose upload is progressing.

**Warning signs:**
`failed` jobs whose `out_dir` still contains a valid `highlights.mp4`; duplicates in Telegram; `.part` files accumulating on the server; friends reporting "бот сказал ошибка, а потом через час пришёл ролик".

**Phase to address:** Runner (outbox, resume), Server core (verified atomic receive), Protocol (`complete` shape, `FailCode::UploadFailed`).

---

### Pitfall 5: Failure message to the user fails to send (parse_mode, 4096/1024 limits, blocked user), so the failure is invisible

**What goes wrong:**
The core value of v1.1 is "the user always learns what happened". Telegram can reject exactly the message that explains a failure:
- `parse_mode=HTML/MarkdownV2` with an unescaped nickname, map or error text (`<`, `&`, `_`, `*`, `.`, `-`) -> `400 can't parse entities`; the notification is lost.
- Text over 4096 characters (`sendMessage`) or caption over 1024 (`sendVideo`/`sendDocument`), counted after entity parsing -> 400. A `FailCode` hint plus a log tail easily exceeds it. [HIGH, official docs]
- `403 Forbidden: bot was blocked by the user` or `chat not found` -> exception propagates and stalls the delivery loop (or crashes the bot thread), blocking every other friend's results.
- Replying to a message that was deleted (`reply_to_message_id` not found) -> 400 unless `allow_sending_without_reply`.
- A caption that works for a 3-char nick breaks for a 40-char one.

**Why it happens:**
Delivery code is written for the happy path; the error path only gets tested with ASCII.

**How to avoid:**
- Prefer NO `parse_mode` (plain text) for failures and for anything containing demo-derived text; where formatting matters use HTML with a single `escape_html()` and always keep a plain-text fallback: on `can't parse entities`, resend as plain text.
- One `fit(text, limit)` helper that truncates on a char boundary with a margin (e.g. 3500 of 4096, 900 of 1024) and appends `...`; never send raw log tails to friends (Pitfall 8). Long details -> second message or a document.
- Delivery loop treats per-chat errors as terminal for THAT delivery only: 403/blocked -> mark `delivery_state=undeliverable`, keep the video, log it, continue. Retry only transport errors/5xx/429 with `retry_after`.
- Use `reply_parameters` with `allow_sending_without_reply=true`; if the chat is private just send to `chat_id`.
- Table-driven tests through the fake Telegram (Pitfall 31) with nicknames containing `<b>`, `&`, `_`, 5000-char text, and a blocked user.

**Warning signs:**
A job `done` on the server with delivery never confirmed; bot log lines `Bad Request: can't parse entities`; `message is too long`; a friend says "я ничего не получил".

**Phase to address:** Delivery (primary). Protocol: failure text built from `FailCode` + short hint, not raw logs.

---

### Pitfall 6: Local `telegram-bot-api` set up wrong: not in `--local` mode, cloud/local mix-up, or the bot cannot read the file path it is given

**What goes wrong:**
The 2 GB capability depends on several things being right at once. [HIGH for the facts listed, from the README/docs]
- The server must be started with `--local` and both `--api-id` and `--api-hash` (from my.telegram.org, tied to the author's Telegram account). Without `--local` you can run a "local server" that still behaves with cloud-style limits and HTTPS-only rules; a stock Docker deploy can look like it works while limits do not change. [MEDIUM on the stock-image behaviour]
- The bot client library must be pointed at the local base URL (`http://127.0.0.1:8081`). Otherwise it silently keeps talking to `api.telegram.org` and the 20 MB / 50 MB limits return.
- Switching a bot token from the cloud to a local server requires `logOut` on the cloud first (otherwise updates are split between servers); after `logOut`/`close` Telegram imposes a waiting period (~10 minutes) before the token can log in again elsewhere. [MEDIUM on the exact duration, verify] Plan the cut-over; do not test this on the production token.
- In `--local` mode `getFile` returns an **absolute filesystem path** in `file_path` and the file is stored on the bot-api host; the HTTP `/file/bot<token>/...` download route does not apply. The server program must read that path directly, so it must be the same host (or the same container path via a shared volume), the same path namespace (a Docker container path like `/var/lib/telegram-bot-api/<id>/documents/file_0.dem` is NOT a host path), and have read permission (different users in Docker vs host is the common failure). Typical libraries still try HTTP download first; verify your chosen Rust client handles local paths or bypass it. [HIGH that path is returned; MEDIUM on library behaviour, per library]
- The file is only saved to disk after `getFile` is called, not when the update arrives. [MEDIUM, community]
- The per-bot working directory (`--dir`) holds the TDLib state; if it is not persisted (container recreated without a volume) you lose pending updates and local file paths become invalid.
- The path contains the bot-id and, per community reports, the token: logging `file_path` can leak the token. [MEDIUM, verify in spike]

**Why it happens:**
The local server is a hard-to-notice behavioural fork: everything "works" for small files against the wrong endpoint.

**How to avoid:**
- Spike first (≤1 h): run `telegram-bot-api --local --api-id --api-hash --dir <persistent> --http-ip-address 127.0.0.1` (confirm flag with `--help`), send a 300 MB file, call `getFile`, read the returned path from the server process, `sendVideo` with a `file://` URI. Write the working command line/compose file into the repo docs.
- Startup self-check in the server: `getMe` against the configured base URL; assert the URL is the local one (refuse to start with `api.telegram.org` when `local_mode=true`); `getFile` smoke test on first start; fail fast with a Russian message.
- Config key `bot_api_base_url` and `bot_api_files_root` (path mapping for containers: translate container prefix -> host prefix).
- On receipt: immediately **move/hardlink/copy the file into server-owned storage** (`storage/demos/<job_id>.dem.part -> .dem`) and record size + SHA-256; never keep depending on the bot-api working dir (Pitfall 12).
- Use `http://127.0.0.1` only; do not expose port 8081 (Pitfall 12/D6).

**Warning signs:**
`getFile` returns a `/file/bot...` style URL or "file is too big"; 413/400 on files > 50 MB; `ENOENT` on the returned absolute path; the bot works only when server and bot-api share a host; bot suddenly gets no updates after a container recreate.

**Phase to address:** Bot (spike + startup self-check), Deploy/E2E (volume layout, compose/systemd units).

---

### Pitfall 7: Whitelist enforced in one handler, not at every entry point; strangers can trigger disk use

**What goes wrong:**
The bot username is discoverable; strangers will message it. Gaps: whitelist checked in `/start` or on documents but not on callback queries, edited messages, forwarded files or inline buttons; whitelist keyed by `@username` (changeable, reusable) instead of numeric `from.id`; in group chats the check uses `chat.id` (a group id) so any member of an allowed group is trusted, or `from` is absent (channel posts); the bot is added to random groups (`my_chat_member`) and starts replying; unauthorised document triggers `getFile`, so a stranger can make the local server download 2 GB files (disk fill) before any check; the bot answers every stranger with a long message (spam amplification / flood limits); admin commands (`/allow`) gated by a DB flag a user can obtain.

**How to avoid:**
- Single `authorize(update) -> Auth{Admin|User(id)|Denied}` function called first in the one update dispatcher; every update kind goes through it; unknown kinds are dropped. Default deny.
- Key on numeric `from.id`. Config-file allow-list seeded by `admin_id`; admin commands compare against `admin_id` from config, not a DB column. Support `/allow <id>` / `/deny <id>` by admin only; persist.
- Check BEFORE any `getFile`/download. Check declared `document.file_size` against limits and quota before `getFile`.
- Private chats only in v1.1: ignore (and `leaveChat` for) groups/supergroups/channels. This removes the whole group-vs-private class (privacy mode, topic threads, shared chat id, 20 msg/min group limit) at no cost to the stated use case.
- Denied users: reply at most once per N minutes with a short "нет доступа, ваш ID: 123456, передайте автору" (useful for onboarding friends) and rate-limit per user; never echo user content.
- Log denied attempts at INFO with the numeric id only.

**Warning signs:**
Bot-api working dir growing with files from unknown senders; replies in a group; `callback_query` handled for a non-whitelisted id; log shows usernames used for auth.

**Phase to address:** Bot (dispatcher + authorize), Hardening (tests per update kind).

---

### Pitfall 8: Secrets in logs and in messages sent to friends (bot token, runner token, FACEIT key, local paths)

**What goes wrong:**
- Telegram URLs contain the token (`/bot<TOKEN>/getMe`). HTTP client errors (`ureq`, `reqwest`) often Display the URL; `{e}` or `{e:?}` in a log line or a job failure leaks it. Panic messages and `Debug`-derived config structs do too. [HIGH, common]
- Local `file_path` may contain the token (Pitfall 6).
- Runner token in a query string, in CLI args (visible in process lists), or in `settings.json` in plain text (the FACEIT key is already stored that way).
- The runner forwards its journal (`Progress::log` lines) to the server and the server forwards snippets to Telegram: those lines contain Windows paths with the author's username, the FACEIT API key in a signed demo URL, ffmpeg command lines, and demo-derived text. Friends see them. [HIGH risk: the existing `Progress::log` stream was written for the local UI]
- Accidentally committing `server.toml`/`.env`.

**How to avoid:**
- `Secret<String>` newtype with `Debug`/`Display` = `***`, used for every token in config structs; no `#[derive(Debug)]` on structs holding raw strings.
- A `redact()` pass over every error string that leaves the HTTP layer: replace the exact configured token values and the regex `bot\d+:[A-Za-z0-9_-]{30,}`; apply to logs, DB-stored errors, and chat messages. Unit test with a sentinel token: run a failing request against a closed port and assert the sentinel is absent from the error text and logs.
- Runner token only in the `Authorization: Bearer` header; configure via env var/file, not CLI flag; store hashed on the server (SHA-256 is fine for high-entropy random tokens), compare in constant time; support revoke and rotate.
- What crosses to the user is a closed set: `FailCode` + its Russian hint + stage name + short sanitised detail (≤ 200 chars, paths stripped to file names). Full journal stays on the runner (and optionally on the server for the admin only, `/log <job>` admin command).
- `.gitignore` for config/secret files; ship `server.example.toml` only. Warn at startup if the config file is world-readable (Unix mode check).

**Warning signs:**
`grep -r "bot[0-9]*:" logs/` hits; a Telegram message containing `C:\Users\...`; token visible in `ps`/Task Manager command line.

**Phase to address:** Protocol (what a failure message may contain), Server core + Bot (Secret type, redaction), Hardening (sentinel tests).

---

### Pitfall 9: Path traversal and reserved names via `document.file_name` and job identifiers

**What goes wrong:**
`file_name` is attacker-controlled: `..\..\x.dem`, absolute paths, `CON`/`NUL`/`AUX`, trailing dots/spaces, NTFS alternate streams (`a.dem:evil`), 300-char names, RTL override tricks, `%` and `;`. Using it (or a player nick, or a client-provided job id) as a path component on the server or runner is directory traversal or collision. On the runner, the server-supplied name would flow into `game/csgo/<name>.dem` and CS2's `playdemo <name>` (the v1.0 `console_name` already neutralises most of this, but only if it is always applied and made unique). Runner API paths like `/jobs/{id}/result` are another traversal vector if `id` is string-joined.

**How to avoid:**
- Storage names are server-generated: `<job_id>.dem` where `job_id` is an integer/UUID parsed strictly (`u64`/`Uuid`), never `Path::join(user_string)`. The original `file_name` is a display-only attribute (sanitised per Pitfall 1, length-capped), stored in the DB.
- Runner writes the demo to a runner-generated path (`<out_root>/jobs/<local_job_id>/demo.dem`) and a unique `cs2c_<job>.dem` in the game folder (suffix with job id, as v1.0 Pitfall 9 recommends). Never accept a path or filename from the server on the runner side.
- Canonicalise-and-check as a belt-and-braces: after building any path, `starts_with(root)` on the canonical form.
- Type detection by magic bytes (Pitfall 2), extension only as a hint (FACEIT gives `.dem.zst`; people also send `.zip`/`.rar`/`.bz2`: reply with a clear unsupported-format message rather than failing in the parser).
- If FACEIT links are ever accepted by the bot (not in v1.1 scope per the brief: "демка файлом"), restrict to the FACEIT API client path (`parse_match_id`) and never fetch arbitrary URLs from the server (SSRF).

**Warning signs:**
Files outside the storage root; names with `..`/`:`; `playdemo` failing only for some uploads; collisions when two users upload `match.dem`.

**Phase to address:** Server core and Bot (storage naming at first ingestion), Runner (game-folder name).

---

### Pitfall 10: Progress message edits trigger flood control (429) and stall the queue

**What goes wrong:**
Wiring `Progress::log`/`stage` straight to `editMessageText` fires dozens of edits per minute per chat. Telegram's documented guidance: about 1 message per second per chat (bursts tolerated), 20 messages per minute in groups, ~30 messages per second bot-wide; exceeding returns `429 Too Many Requests` with `parameters.retry_after`. [HIGH for the limits, official FAQ; edit-specific limits are not documented, treat them as the same bucket] Also: editing with identical text/markup returns `400 message is not modified` (treated as a fatal error by naive code); editing a message the user deleted returns `400 message to edit not found`; if a single worker thread does edits inline with `sleep(retry_after)`, one flooded chat delays everyone's results (including result delivery).

**How to avoid:**
- A per-chat **edit coalescer**: keep only the latest desired state; flush at most every 3-5 s (coarser, e.g. 10 s, in the Record stage); skip when text equals the last sent text; send an edit only on stage change or a meaningful percentage step (e.g. 10 %), not per log line.
- `retry_after` honoured per chat (schedule next flush at `now + retry_after + jitter`), not by sleeping the shared worker. Result delivery (`sendVideo`) has priority over progress edits.
- `not modified` -> success; `message to edit not found` -> stop editing, send one fresh status message; any other progress error is logged and swallowed (never fails the job).
- Do not use progress as a notification channel: the terminal states (done/failed/cancelled) are separate NEW messages (edits do not notify the user).
- Runner -> server progress is also throttled (stage changes + at most one update per 2-5 s); never stream raw console lines.

**Warning signs:**
`429` in bot logs; progress that jumps or lags minutes behind; result delivery late while progress edits retry; log spam of `message is not modified`.

**Phase to address:** Delivery (coalescer), Protocol (progress DTO = stage + percent + short text, not log lines).

---

### Pitfall 11: Sending the final video: timeouts, ambiguous failure, and video that is not inline-playable

**What goes wrong:**
- Upload through the local server takes minutes (the local server uploads to Telegram's data center at the host's upstream speed). An HTTP client with a generic 30-60 s total timeout aborts every large upload. Worse, a timeout is **ambiguous**: Telegram may still deliver the video, and a blind retry sends it twice (or N times). [HIGH reasoning; typical]
- `sendVideo` and `sendDocument` limits: 2000 MB with the local server, 50 MB on the cloud. [HIGH, official] Check size before sending.
- Inline playback needs H.264 + AAC MP4 in a playable pix_fmt (yuv420p); clients "support MPEG4 videos, other formats may be sent as document". Output from the NVENC path could be HEVC or 10-bit if presets drift; `moov` at the end of the file prevents streaming start until fully downloaded. Without `width`, `height`, `duration` parameters some clients show a square or 0:00 preview. `thumbnail` must be JPEG < 200 kB (and small side; larger is ignored). [HIGH for 200 kB/JPEG; MEDIUM for client behaviours]
- Caption > 1024 chars (Pitfall 5).
- `sendVideo` with a multipart body buffers a 1 GB file in memory in some HTTP clients.

**How to avoid:**
- Separate HTTP clients/timeouts: control calls (short, total 15-30 s) vs transfer calls (idle/read timeout only, e.g. 120 s of no progress; no total cap, or an hour). For `sendVideo`, send by `file://` URI of a file under the bot-api-visible root (local mode "upload by local path") so no multipart body crosses the HTTP client at all; permissions/path mapping per Pitfall 6.
- Delivery state machine `pending -> sending -> sent | uncertain | failed`. On timeout/connection reset during `sending` set `uncertain`, **do not auto-retry more than once**, tell the user ("не уверен, дошёл ли ролик, пришлю ещё раз по /resend"), and keep the file.
- Assembly flags in the runner: `-c:v libx264|h264_nvenc -pix_fmt yuv420p -c:a aac -movflags +faststart`; verify with `ffprobe` (codec_name h264, pix_fmt yuv420p) before upload; if not H.264, send as `document` rather than fail. Always pass `supports_streaming=true`, `width`, `height`, `duration` from the ffprobe result; generate a small JPEG thumbnail (`ffmpeg -vf scale=320:-1 -frames:v 1 -q:v 5`, assert < 200 kB).
- Pre-check size ≤ ~1900 MB; over the limit -> `FailCode::ResultTooLarge` with a hint ("снизьте качество/длину"), not a Telegram 400 after a 10-minute upload.
- Send `sendChatAction upload_video` every ~5 s during the send so the chat shows activity (cheap, optional).

**Warning signs:**
Every large video "fails" at exactly the client's timeout value; duplicate videos in chats; video shows without play preview or forces a download before playing; sends of a 1.5 GB file stalling the bot's other work.

**Phase to address:** Delivery (state machine, timeouts), Runner (assembly flags + ffprobe metadata in `complete`).

---

### Pitfall 12: Bot-api working directory and server storage fill the disk (no retention, no quotas)

**What goes wrong:**
Storage grows in four places: (1) telegram-bot-api's own working dir keeps every `getFile`'d demo and sent files (no automatic expiry found in the README or docs [MEDIUM, verify in spike]); (2) server `storage/demos` + `storage/videos`; (3) `.part` files from aborted uploads; (4) the runner's `out_dir/clips` and `game/csgo` demo copy (v1.0 Pitfalls 9/13 cover cleanup per job but not accumulation across hundreds of remote jobs). 5 friends × a few demos a day × 300 MB + 500 MB videos fills a small VPS in days. The whitelist stops strangers, not careless friends or a compromised account. When the disk fills, SQLite itself fails (`database or disk is full`), which also breaks the job state machine.

**How to avoid:**
- Quotas: max `N` active (queued+running) jobs per user (2-3), max total stored bytes per user, global storage cap; reject with a Russian message before `getFile`. Fair scheduling: round-robin across users rather than strict FIFO, so one friend sending 10 demos does not starve others.
- Retention: demo deleted when the runner confirms download (or at job terminal state); video deleted after delivery confirmed + short grace, or after `retention_hours`; `.part` swept on startup and hourly; a janitor for bot-api's working dir (delete files older than X after they were copied into server storage; never while a copy is in progress). Make the janitor config-driven and log what it removed.
- Check free space (`statvfs`/`GetDiskFreeSpaceExW` via `fs2`/`sysinfo`) before accepting a file and before accepting a runner upload (`Content-Length` + reserve); enforce the cap while streaming when length is unknown.
- Runner advertises `free_bytes` in hello/heartbeat; server does not hand out a job if the runner lacks room (reuse v1.0 disk estimate); runner cleans per-job directories after ACK (Pitfall 4) and keeps the last N for debugging.
- SQLite on a volume with its own headroom or `PRAGMA max_page_count`; monitor free space and warn the admin in Telegram at 80 %.

**Warning signs:**
`df` trend; `database or disk is full`; bot-api dir several GB bigger than `storage/`; `.part` files older than a day.

**Phase to address:** Server core (quotas, retention, janitor), Bot (pre-`getFile` checks), Runner (cleanup after ACK), Deploy/E2E (monitoring).

---

## Moderate Pitfalls

### Pitfall 13: NAT, idle timeouts and half-open TCP between runner and server
**What goes wrong:** Outbound-only connectivity is right, but home routers/CGNAT drop idle mappings in minutes, and reverse proxies close idle upstream connections (nginx default `proxy_read_timeout` 60 s). A half-open connection (the peer vanished, no FIN) looks healthy to both sides for hours; blocking reads never return; the runner shows "online", the server believes the runner is alive or dead depending on who notices first.
**Prevention:**
- Prefer **HTTPS request/response with bounded long-poll** (claim waits ≤ 20-25 s, below every proxy timeout) over a persistent WebSocket: it works with blocking `ureq` and `std::thread` (no tokio on the runner), reconnects for free, passes through NAT and proxies, and a dead connection costs at most one poll interval.
- Application heartbeat with a deadline on BOTH sides (runner: no successful heartbeat for 3 × interval = "offline" in UI; server: lease/online status from `last_seen`), plus per-request connect/read timeouts (never infinite). TCP keepalive is a supplement, not the mechanism.
- Reconnect: exponential backoff with jitter (1 s -> 60 s cap), reset after success; log state *transitions* ("offline since 14:02", "online again"), not every attempt; UI/tray shows runner status.
- Server shows runner status to users: `/status` -> "раннер онлайн / офлайн с 14:02, задачи ждут"; notify the user of a queued job once if the runner has been offline > N minutes; job TTL in queue (e.g. 24 h) -> `expired` with message.
**Warning signs:** "online" runner that never receives jobs; heartbeat gaps exactly at 5/10/15 min; runner threads blocked in `read`; jobs waiting while runner PC slept.
**Phase:** Protocol (long-poll + heartbeat design), Runner (backoff, status), Server core (last_seen, TTL).

### Pitfall 14: Runner machine asleep/locked/busy (author is playing CS2) makes jobs silently wait or fail with the wrong blame
**What goes wrong:** The runner is a desktop app on the author's PC. It sleeps, locks, or the author starts a real CS2 match. v1.0 preflight (foreign `cs2.exe`) correctly refuses, but now that failure reaches a friend as an "ошибка" even though nothing is wrong with their demo.
**Prevention:** Runner-side `paused` / `busy` states reported to the server (claim returns nothing; no lease). Remote jobs that hit environment-class `FailCode`s (`CS2_RUNNING`, Steam signed out, HLAE mismatch) are returned to the queue (`requeue`, `attempt` unchanged) and the user is told once "раннер занят/не готов, ролик будет позже", not failed; content-class failures (bad demo, no kills for the player) fail. Reuse the v1.0 failure classification (`failed(env)` / `transient` / `content`) as the wire contract. Runner holds the v1.0 power request ("keep awake") while connected and idle if the author enables "принимать задачи"; document that a sleeping PC = offline runner. A "не принимать задачи" toggle for when the author plays.
**Warning signs:** Friends see CS2-related failures at the times the author plays; jobs failing with environment codes in bursts.
**Phase:** Protocol (error classes on the wire), Runner (pause/busy, requeue semantics), Delivery (distinct messages).

### Pitfall 15: Large-file transfer done as one buffered request with defaults (body limits, proxy limits, whole-file memory)
**What goes wrong:** Typical stack defaults reject or buffer big bodies: axum/hyper default body limit (2 MB), nginx `client_max_body_size 1m` and request buffering to disk, Cloudflare free-plan proxy body limit (100 MB) if the server sits behind it, `Vec<u8>`/`to_bytes()` of a 500 MB body, ureq/reqwest `.send_bytes()` of a whole file. Upload speed on the author's home upstream (10-50 Mbps) means a 500 MB video takes 1.5-7 minutes; any total timeout < that fails. Demo download to the runner is the same in the other direction. `demo::read_demo` itself loads whole files into memory (Pitfall 2), fine for local use, not for transfer code.
**Prevention:** Stream everywhere (reader -> file with a fixed 64 KB-1 MB buffer; SHA-256 computed on the fly). Chunked protocol (e.g. 8-16 MB chunks, `PUT /uploads/{id}?offset=N`, `GET` status returns received size) gives resume and per-chunk cancel checks (ureq cannot abort an in-flight body, and cancel must work in seconds per v1.0). Explicit per-route limits and proxy config (`client_max_body_size`, `proxy_request_buffering off`, `proxy_read_timeout` ≥ chunk time). Demo download uses `Range` with resume. Idle-based timeouts, not total (Pitfall 11). Verify size + SHA-256 on both ends. No Cloudflare proxy in front of the upload route (or use chunks < 100 MB).
**Warning signs:** 413 on files just over a round number; RSS on the server tracking file size; uploads failing at the same elapsed time; works on LAN, fails over internet.
**Phase:** Protocol (chunk/resume contract), Server core + Runner (implementation), Deploy/E2E (proxy settings validated with a real 500 MB file).

### Pitfall 16: Telegram updates: `getUpdates` offset handling, double polling, and replays after crash
**What goes wrong:** With the local server, long polling is simplest (no TLS, no public port); webhooks add secret-token checks, port exposure and retries. Common long-polling mistakes: confirming (advancing `offset`) before the update is durably handled, so a crash loses a demo submission; not advancing, so the same update replays and the same demo is queued twice; two instances (a dev run + systemd service) polling one token -> `409 Conflict: terminated by other getUpdates request`; client read timeout shorter than the poll `timeout` (e.g. poll 50 s, client 30 s -> constant spurious errors); not setting `allowed_updates`; pending updates are dropped by Telegram after ~24 h. [HIGH for 409/offset semantics; official docs]
**Prevention:** Persist `last_update_id` in the SAME SQLite transaction that inserts the job (exactly-once ingestion); unique index on `(chat_id, message_id)` and keep `file_unique_id` for dedupe; poll `timeout=30-50` with client timeout +15 s; `allowed_updates=["message","callback_query"]`; single-instance lock file on startup; treat 409 as "another instance, exit" not retry-forever; process updates sequentially per user. If webhooks are chosen later, bind to loopback behind the proxy and verify `X-Telegram-Bot-Api-Secret-Token`.
**Warning signs:** same demo queued twice after a restart; 409 errors; the bot "forgets" a demo sent during a restart.
**Phase:** Bot.

### Pitfall 17: `file_id` vs path vs `file_unique_id`
**What goes wrong:** Code stores only the local path from `getFile` and loses the demo if the bot-api dir is cleaned or moved; or stores only `file_id` and tries to re-download later after a token/server migration (ids are scoped to the bot; across a cloud -> local switch re-validate). `file_id` is not stable as a dedupe key (`file_unique_id` is). Callback payloads are limited to 64 bytes, so a job's filename cannot go in `callback_data`.
**Prevention:** At ingestion record `file_id`, `file_unique_id`, `file_size`, sanitised name; call `getFile` once, copy into server storage, store the server path and hash, then treat the Telegram file as gone. Dedupe on `file_unique_id` + player (offer "уже есть такой ролик" and resend). Callback data = short opaque job id (`c:123`) validated against the clicking user's ownership.
**Phase:** Bot.

### Pitfall 18: `/player` semantics: ambiguous nick matching picks the wrong player, mutable state races, and late failure
**What goes wrong:** `pipeline::find_player` matches steamid first, then exact name, then **first substring hit** (`contains`). `"s1"` silently selects some teammate; two players with similar nicks produce a 20-minute recording of the wrong person that nobody notices. The stored `/player` may change after a job is queued (queued job silently uses the new value), a user with no `/player` set sends a demo and gets an error only after upload, and nicks differ between matches (FACEIT nick vs in-demo name). [HIGH, code]
**Prevention:** Pin by **SteamID64** where possible: `/player` accepts a nick, a SteamID64 or a steamcommunity profile URL; after the first successful resolution store the steamid and show "Игрок: nick (steamid)". Resolution happens in the cheap Parse stage; ambiguity or no match returns `FailCode::PlayerNotFound`/`PlayerAmbiguous` **before** CS2 launches, with up to 10 sanitised candidate nicks in the reply and a hint to use `/player` and `/retry`. Never silently take the first substring match in the remote path (exact only, unique-substring only). Snapshot `player` into the job row at enqueue time (immutable); a user without `/player` is asked at upload time ("сначала /player ник") and the demo is kept pending, not discarded.
**Phase:** Bot (command + snapshot), Protocol (`PlayerNotFound`/`PlayerAmbiguous` codes with candidates), Runner (strict resolution).

### Pitfall 19: Protocol / `FailCode` drift between runner and server (and accidental dependency of the server on Windows-only crates)
**What goes wrong:** The server renders `FailCode` hints, but `FailCode` lives in the runner-side `pipeline`/`host` crates. If the server depends on them it drags `demo`, `positions`, `windows-sys` and (via `app`/`pipeline`) egui-adjacent deps into a server that must build on any OS (CI is Linux; the server OS is undecided). Alternatively duplicated enums drift: a new runner code arrives that the server cannot deserialise and the job is stuck. The runner auto-updates itself (`update.rs`) so runner and server versions will diverge.
**Prevention:** New `crates/protocol` (serde-only, no `cfg(windows)` deps) holding wire DTOs, `FailCode` string form + Russian hint table + class (env/transient/content), job and delivery state enums, `PROTOCOL_VERSION`. `pipeline` maps its internal errors into it. Enums deserialise with an unknown-variant fallback (`#[serde(other)] Unknown`) and `#[serde(default)]` on new fields; hello carries `protocol_version` and `app_version`; the server answers too-old/too-new with a clear "обновите раннер" and refuses leases. Server rolled out first and supports N and N-1. CI job: `cargo build -p server` on Linux with no Windows deps in `cargo tree -p server`.
**Phase:** Protocol (first), Deploy/E2E (version-skew test).

### Pitfall 20: SQLite on the server: shared `Mutex<Connection>` in an async/threaded server, deferred transactions, and file-system placement
**What goes wrong:** The runner-side `store` is `Arc<Mutex<Connection>>` with WAL and `foreign_keys` pragmas but no `busy_timeout` (`crates/store/src/lib.rs`). Reusing that pattern in a server brings: holding a `std::sync::Mutex` across `.await` (deadlock or `!Send`), blocking rusqlite calls on the async reactor (bot polling, runner API and progress edits stall together), `SQLITE_BUSY` under concurrent writers, and the lease claim written as a deferred transaction (read then upgrade -> immediate BUSY that `busy_timeout` cannot retry). Also: `foreign_keys` is per connection; copying the DB file while in WAL mode yields a corrupt backup; WAL on network/bind-mounted Windows/macOS volumes misbehaves; schema changes with persistent data need tested migrations.
**Prevention:** One DB **actor**: a dedicated thread owning the connection and an mpsc request channel (works with or without tokio; no lock across await, no BUSY between our own writers), or a small pool where all writes use `BEGIN IMMEDIATE`. Set `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON` on every connection. Store ints (unix epoch, UTC), not local-time strings; large blobs on disk, paths in the DB. Job state transitions in one function with a CHECK-constrained `status` and table-driven tests (legal transitions only). Backup with `VACUUM INTO` / the backup API, not file copy. Keep the DB on a local filesystem/named volume. Reuse the `PRAGMA user_version` migration pattern already tested in `store`; take a backup before migrating. Server schema lives in its own module/crate so the runner binary does not carry it, with a separate DB file from the runner's `cs2cinema.db`.
**Warning signs:** `database is locked`; request latency spikes while a long query runs; a worker that hangs holding the connection.
**Phase:** Server core.

### Pitfall 21: Shared tokio decision: mixing runtimes and blocking calls
**What goes wrong:** The constraints allow `tokio` for the server if justified, while the runner/app remain `std::thread`. Mistakes: pulling tokio into the runner crates through a shared crate (`protocol` or a shared HTTP client) and bloating the app; blocking file/SQLite/`ffprobe` calls inside async handlers; mixing `ureq` (blocking) into the async server; unbounded concurrent uploads each buffering.
**Prevention:** Keep `protocol` runtime-free. Pick one model for the server and isolate the other: either all `std::thread` + a small blocking HTTP server (fewer deps, consistent with the codebase; fine for one runner and ≤10 users) or tokio with `spawn_blocking`/the DB actor and `tower` body limits. Record the decision in Key Decisions; do not let `tokio` leak into `crates/app`/`crates/pipeline`.
**Phase:** Protocol/Server core (decision at the start).

### Pitfall 22: Clock skew and time-based assumptions across three machines
**What goes wrong:** Server, runner PC and (via bot-api) Telegram have different clocks. Comparing runner timestamps with server time (lease expiry, "job age"), time-limited signed URLs/tokens (JWT `exp`, HMAC with timestamp) rejecting valid requests, and sleep/resume jumping wall clocks. TDLib inside `telegram-bot-api` needs a correct server clock; a badly skewed server clock breaks the Telegram connection with obscure errors. [MEDIUM]
**Prevention:** Only the server's clock decides leases, expiry and TTL; the runner reports durations and uses monotonic `Instant` for its own deadlines. Prefer opaque random tokens over time-signed ones; if signatures are used, allow generous leeway. Include server time in the hello response and log a warning when the runner's clock differs by > 60 s (diagnostic only). Enable NTP on the server (`timedatectl set-ntp true`; Windows time service) and document it.
**Phase:** Protocol, Deploy/E2E.

### Pitfall 23: Live-run feedback loop: logs and progress flowing server-ward become a firehose or a privacy leak
**What goes wrong:** Forwarding every `Progress::log` line (HLAE console markers, ffmpeg progress) multiplies network chatter, SQLite writes on the server, and edits (Pitfall 10); lines hold paths/usernames (Pitfall 8). The job journal on the server grows unbounded.
**Prevention:** Server receives structured events only: stage changes, percent, `FailCode`, a bounded ring of the last ~50 sanitised lines attached to a *failure* (admin-visible via `/log <id>`, never forwarded to friends). Full journal remains on the runner. Cap rows per job.
**Phase:** Protocol, Runner.

### Pitfall 24: Cancellation spans three systems and races with completion
**What goes wrong:** `/cancel` for a queued job is easy; for a running job it must reach the runner within seconds (v1.0 promise) through the heartbeat/poll response, but the runner may be offline; the job may complete at the same moment; the file may be mid-upload; Telegram edits keep going after cancel.
**Prevention:** Server sets `cancel_requested`; heartbeat response carries it; runner maps it to the existing `Progress::cancelled`/`CANCELLED` path. Server-side terminal state `cancelled` is sticky and wins over a late `complete` (late upload deleted, no video sent). If the runner is offline, job is cancelled on the server immediately and the runner learns via `LEASE_LOST` on reconnect (Pitfall 3). Only the owner (or admin) can cancel; callback buttons validated (Pitfall 17). Always `answerCallbackQuery` so the button spinner stops.
**Phase:** Protocol, Server core, Runner, Bot.

### Pitfall 25: Fairness and surprise on the shared queue
**What goes wrong:** One friend queues many demos; others wait hours with no estimate; the author's own interactive local jobs and remote jobs compete for the single CS2 with no stated priority.
**Prevention:** Per-user active-job cap (Pitfall 12), round-robin pick, queue position + rough ETA in `/queue` (use measured durations of the last jobs), explicit priority rule documented (local manual job > remote; or the runner finishes the current job first). The runner's local queue and remote jobs use one worker; the remote job appears in the app's job list with a "Telegram" source tag.
**Phase:** Server core (pick policy), Runner (single worker integration), Bot (`/queue`).

### Pitfall 26: Runner install/config UX and token provisioning
**What goes wrong:** Runner needs server URL + token; users paste them wrong, token shows in plain text in the egui field, the `settings.json` is world-readable on a shared PC, and a mistyped URL gives "connection error" with no hint (TLS error vs DNS vs 401 look alike).
**Prevention:** Distinct, Russian diagnostics per layer (DNS fail, connect refused, TLS cert error, 401 wrong token, 426 protocol too old); a "Проверить соединение" button doing hello; password-style field; token created by a server CLI command (`server runner add <name>` prints once) and shown once; revoke command. `ureq` `platform-verifier` is already in use: for a self-signed server cert document a pinned-CA option rather than disabling verification (never offer "ignore TLS errors").
**Phase:** Runner, Server core (CLI), Deploy/E2E.

---

## Minor Pitfalls

### Pitfall 27: Telegram quirks that bite once each
- A user must `/start` the bot before it can message them; whitelisting by ID before first contact is fine because they initiate by sending a demo, but `sendMessage` to a never-started id gives 403. **Prevention:** treat as undeliverable, no retry.
- `sendChatAction` expires after ~5 s; refresh or skip.
- Bot commands list (`setMyCommands`) in Russian; descriptions ≤ 256 chars.
- `message_thread_id` / topics only matter in groups (out of scope with private-only).
- Bot API server restarts drop the long poll; the client must reconnect without exiting.
**Phase:** Bot.

### Pitfall 28: Time-zone and locale in messages
Times in chat messages use the server TZ; friends may be elsewhere. Prefer relative text ("в очереди 12 мин") over absolute times; store UTC. **Phase:** Delivery.

### Pitfall 29: Logs: PII and size
User ids, file names, nicks in logs retained forever. Rotate (journald or size-based), keep ids not usernames, no demo-derived text unsanitised. **Phase:** Server core.

### Pitfall 30: Dev-run collisions
A developer run of the server against the production token causes 409s; a debug runner connected to the production server takes real jobs. **Prevention:** separate test bot token, `--dry-run` that never calls Telegram, environment name in hello (`prod`/`dev`) and server refusing mismatches. **Phase:** Server core, Deploy/E2E.

---

## Deployment Pitfalls (Windows service vs systemd, secrets)

### Pitfall 31 (D1): Server OS undecided, so portability bugs appear late
`Path` vs string concatenation, `\` vs `/`, file modes for secrets, case-sensitive filenames, line endings in generated configs, `cfg(windows)` deps leaking in (Pitfall 19). **Prevention:** build and run the server test suite on Linux CI from the first server commit; use `PathBuf` joins only; keep OS-specific bits behind small modules. **Phase:** Server core.

### Pitfall 32 (D2): systemd unit mistakes (Linux)
`Restart=always` without `StartLimitBurst`/`RestartSec` -> crash loops that spam Telegram admin alerts; secrets in `Environment=` (visible in `systemctl show`); running as root; no graceful shutdown (SIGTERM kills mid-upload, leaves `.part`); telegram-bot-api not started before the server (connection refused at boot -> exit). **Prevention:** non-root `User=`; `EnvironmentFile=` with mode 0600 or `LoadCredential=`; `ProtectSystem=strict` + `ReadWritePaths=` for storage; `Restart=on-failure`, `RestartSec=5`, `StartLimitIntervalSec/Burst`; `After=`/`Wants=` the bot-api unit AND an application-level retry-until-ready for `getMe`; handle SIGTERM (stop accepting, finish or abandon cleanly, `.part` swept at start); journald logging. **Phase:** Deploy/E2E.

### Pitfall 33 (D3): Windows-service gotchas (if the server lands on Windows) and the runner must NOT be a service
Services run in session 0 under `LocalSystem` by default: different profile (`%LOCALAPPDATA%` points elsewhere), no mapped drives, no inherited env vars, no desktop. A Rust binary needs the `windows-service` crate or a wrapper (WinSW/NSSM) to answer SCM start/stop. The **runner** must run in the author's interactive, unlocked session (CS2 window, HLAE, tray, Steam); making it a service breaks CS2 entirely. **Prevention:** server as service with an explicit data dir from config (not `%LOCALAPPDATA%`), logs to a file with rotation; runner via Task Scheduler "at log on" or the Startup folder; docs state both. **Phase:** Deploy/E2E.

### Pitfall 34 (D4): Secrets handling
Bot token, `api_hash`, runner tokens in the repo/Docker image layers/CLI args. **Prevention:** env/credential files outside the repo, `.gitignore`, example config only, tokens never as flags, Docker secrets or env from a 0600 file, rotation procedure documented (BotFather `/revoke`, `server runner revoke`). **Phase:** Deploy/E2E, Server core.

### Pitfall 35 (D5): Exposed ports and TLS
telegram-bot-api listening on all interfaces (default bind likely 0.0.0.0, verify), exposing the stats endpoint (`--http-stat-ip-address`), and runner API over plain HTTP across the internet leaking the token and demo/video bytes. **Prevention:** bind bot-api to loopback; only 443 (reverse proxy with automatic certs, e.g. Caddy) or a VPN (WireGuard/Tailscale) between runner and server; firewall everything else; the runner API rate-limits failed auth. **Phase:** Deploy/E2E.

---

## Testing Pitfalls (without Telegram and without CS2)

### Pitfall 36 (X1): Testing only with ASCII happy paths against a mock that is not the wire format
**What goes wrong:** A hand-rolled mock returns `Ok(())`; the real API returns `{"ok":false,"error_code":429,"parameters":{"retry_after":7}}`, `description` strings, multipart peculiarities, file paths. Bugs appear on the first real day.
**Prevention:**
- A narrow `BotApi` trait (`get_updates`, `send_message`, `edit_message_text`, `send_video`, `get_file`, `answer_callback`) with `FakeTelegram` that records calls and can inject: 429 + `retry_after`, 400 `message is not modified`, 400 `can't parse entities`, 403 blocked, 400 `message to edit not found`, timeout during `sendVideo` (the `uncertain` state), duplicate updates, 409 conflict. Virtual clock (the v1.0 injected-clock approach) so throttle tests run in ms. Same shape as `Host`/`FakeHost`.
- One small **wire-level fake** (a `127.0.0.1` HTTP server in tests) emitting the real JSON shapes so the HTTP/serde layer, base-URL config, redaction and timeouts are exercised.
- Real `telegram-bot-api` smoke is a **manual UAT checklist** with a separate test bot (never the production token; CI has no secrets).
**Phase:** Bot/Delivery (traits and fakes first), Hardening.

### Pitfall 37 (X2): Distributed behaviour untested (lease races, dropped connections, resume)
**Prevention:** In-process server on `127.0.0.1:0` plus real HTTP runner client in tests; a tiny TCP fault-injection proxy (delay, truncate, **stop forwarding without closing** to emulate half-open); injectable clock for lease expiry; two threads racing `claim`; kill the runner mid-upload and resume; server restart mid-job; model-style test: random sequence of {crash runner, crash server, drop connection, cancel, complete} must never leave a job unreachable (every job ends terminal or is resumable) and never produce two videos.
**Phase:** Server core + Runner.

### Pitfall 38 (X3): No end-to-end test of the whole chain
**Prevention:** One in-process test: fake Telegram -> bot -> server -> runner driven by `FakeHost` + dummy takes (the memory-documented no-CS2 recipe) -> assembled `highlights.mp4` -> upload -> `FakeTelegram` receives `sendVideo`. Includes failure variants per `FailCode` class and asserts the Russian message. Also keep `crates/cli/tests/pipeline.rs` passing (constraint) and add a Linux CI target for server tests. The first live run with real CS2 is the v1.0 debt acceptance and should be scripted as a checklist, not improvised.
**Phase:** Deploy/E2E (assembled from earlier fakes).

### Pitfall 39 (X4): Missing hostile-input tests
**Prevention:** Keep a `testdata/hostile/` table: nick injection strings (Pitfall 1), file names (Pitfall 9), truncated/bit-flipped demos and a zstd bomb (Pitfall 2), oversize uploads and wrong SHA (Pitfall 15), sentinel-token redaction (Pitfall 8), unauthorised updates of every kind (Pitfall 7). These are cheap, run in `cargo test`.
**Phase:** Hardening, but each fixture is added in the phase that introduces the input.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Reuse `pipeline::FailCode`/types directly in the server crate | No new crate | Server drags Windows/demo deps, version drift (P19) | Never; use `protocol` crate |
| Whole-file upload in one request | Simplest client code | Timeouts, 413, no resume, memory (P4, P15) | Only for a throwaway spike, not shipped |
| Reuse `Arc<Mutex<Connection>>` unchanged in the server | Zero new code | BUSY, await-lock deadlocks, stalled reactor (P20) | Acceptable only if the server is pure `std::thread` and writes use `BEGIN IMMEDIATE` |
| Poll Telegram with `offset` advanced on receipt | Fewer lines | Lost submissions on crash (P16) | Never |
| Forward raw `Progress::log` to chat | Instant "live log" | Flood control, leaks paths (P8, P10, P23) | Never to friends; admin-only debug |
| Skip TLS "temporarily" for runner API | No proxy setup | Token + demo bytes in cleartext, habit sticks | Only over a VPN/LAN, documented |
| Match players by first substring hit | Forgiving UX | Silent wrong-player montage (P18) | Never in the remote path |
| Delete result after "complete" without ACK | Saves disk | Lost videos on upload failure (P4) | Never |
| Parse demos in-process on the runner | No subprocess plumbing | A crash takes the GUI/runner down, poison loop (P2) | Only with caps + catch_unwind as a first step, child process by end of milestone |
| Single global FIFO | Trivial | One friend starves the rest (P25) | Only while the whitelist is the author alone |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| `telegram-bot-api --local` | Forgot `--local`, or client still points at `api.telegram.org`; bot reads container path from host | Startup self-check on base URL + `getFile` smoke test; path mapping config; copy file into own storage at once (P6) |
| `getFile` in local mode | Using HTTP download route; trusting the path to stay valid | Read the absolute path directly, copy, verify hash, forget the path (P6, P17) |
| `sendVideo` | 30 s client timeout; retry on timeout; non-H.264; no width/height/duration | Idle timeouts, `uncertain` state, ffprobe-verified H.264/yuv420p/faststart, metadata params, `file://` upload (P11) |
| `editMessageText` | Edit per log line; fatal on `not modified` | Coalescer, 3-5 s min interval, per-chat `retry_after` (P10) |
| `getUpdates` | Advance offset before durable handling; two pollers | Offset persisted in the job-insert transaction; single-instance lock (P16) |
| Runner -> server | Persistent socket assumed alive; infinite timeouts | Bounded long-poll + heartbeat deadlines + lease fencing (P3, P13) |
| ffmpeg `drawtext` | Demo text inside the filter string | `textfile=` + `expansion=none`, sanitised labels (P1) |
| CS2 console cfg | Demo-derived strings in comments | No demo text in the cfg at all (P1) |
| SQLite | Deferred transactions for claim; backups by file copy | `BEGIN IMMEDIATE`, DB actor, `VACUUM INTO` (P20) |
| Reverse proxy | Default `client_max_body_size`, buffering, 60 s timeouts, Cloudflare 100 MB | Chunked uploads, explicit limits, test with 500 MB (P15) |
| Existing `store` | Extending the runner DB with server tables | Separate server DB/module; runner DB only gains `remote_job_id`, `lease`, upload-outbox columns |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Progress edits per event | 429s, laggy status | Coalesce (P10) | Any recording with frequent stage lines; groups at 20 msg/min |
| Whole files in memory (`read_demo`, body to bytes) | RSS ≈ file size, OOM on small VPS | Stream with fixed buffers (P2, P15) | Files ≳ 500 MB or 2 concurrent uploads on a 1-2 GB VPS |
| Upstream bandwidth of the author's home line | 500 MB video = 1.5-7+ min; queue latency grows | Resumable chunks; do not claim the next job until `uploaded` (simple) or model "uploading" separately from "recording" (later) | Several jobs back-to-back; large bitrate presets |
| bot-api working dir growth | Disk full | Janitor + quotas (P12) | A few dozen demos |
| Long polling + blocking DB on one thread | Everything stalls during a write | DB actor / spawn_blocking (P20) | Concurrent upload + status commands |
| Per-chunk `fsync` | Slow receive | fsync once at the end + on `.part` rename | Disk with slow flush (HDD VPS) |
| Re-uploading the same demo | Waste, quota burn | Dedupe by `file_unique_id` + hash | Friends forwarding the same match |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Demo strings in cfg/ffmpeg/bat | Console command injection, `connect` to a server with `-insecure` (P1) | Sanitise at boundary; no demo text in cfg; textfile for drawtext |
| Unbounded parse of untrusted demos | Runner crash, OOM, poison loop (P2) | Caps, child process, attempt limit |
| Auth on one handler only / username-based | Strangers or group members use the bot (P7) | `authorize()` first, numeric ids, private chats only |
| `getFile` before authorisation | Disk fill by strangers (P7, P12) | Auth + size/quota check first |
| Tokens in URLs/errors/logs/chat | Bot takeover (P8) | `Secret`, `redact()`, sentinel tests, Authorization header |
| `file_name`/id used as path | Traversal, overwrite (P9) | Server-generated names, strict id parsing, canonicalise-and-check |
| Runner API on plain HTTP | Token and content sniffed (P35) | TLS (proxy) or VPN |
| bot-api port exposed | Anyone with the token (or stats) talks to it | Loopback bind, firewall |
| Runner token compare with `==`, stored plaintext | Timing leak, DB leak = takeover | Hash + constant-time compare, revoke/rotate |
| Friends' journals/paths forwarded | Leaks author's username/paths | Closed set of user-visible fields (P8, P23) |
| Auto-accepting URLs in the bot | SSRF | Out of scope for v1.1; if added, FACEIT-only allow-list |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Silent queue when runner offline | Friend waits hours with no word | `/status`, one-time "раннер офлайн" notice, job TTL (P13) |
| Environment failures blamed on the demo | Friend resends a good demo | Requeue env-class errors, say "раннер не готов" (P14) |
| Error shows `FailCode` only | Cryptic | Code + Russian hint + next step (`/player`, `/retry`) |
| No feedback on accept | Friend resends | Immediate "принято, в очереди №2, игрок X" with the pinned player |
| Wrong player picked | Wasted 20 min + wrong video | Strict matching, candidates list (P18) |
| Duplicate video after timeout | Confusion | `uncertain` state + `/resend` (P11) |
| Progress spam | Muted chat | One edited status message, separate final notification (P10) |
| Unsupported archive (`.rar`, `.zip`) | Opaque failure | Magic-byte check, clear message at upload (P9) |

## "Looks Done But Isn't" Checklist

- [ ] **Local Bot API:** works with a 1 GB file end to end (receive via `getFile` path AND send via `file://`), not only with small files against the wrong endpoint. Verify base URL assertion at startup.
- [ ] **Whitelist:** every update kind (message, edited, callback, forwarded, channel post, `my_chat_member`) tested with a non-whitelisted id; no `getFile` happens.
- [ ] **Failure delivery:** a nickname with `<b>&_*` and a 5000-char detail still produces a delivered message; a blocked user does not stall the loop.
- [ ] **Reconnect:** unplug the runner's network mid-record, mid-download and mid-upload; each resumes or fails cleanly; no duplicate recording; CS2 not double-launched.
- [ ] **Half-open:** a TCP proxy that stops forwarding without closing is detected within 3 heartbeat intervals.
- [ ] **Lease fencing:** a stale lease token cannot upload, complete or fail a job.
- [ ] **Upload integrity:** truncated body, wrong SHA, and replayed `complete` are all handled; `.part` swept on restart.
- [ ] **Output kept until ACK:** kill the runner at 90 % upload; after restart the same file is uploaded.
- [ ] **Quotas/retention:** disk usage returns to baseline after a day of jobs, including the bot-api working dir.
- [ ] **Injection:** hostile nicks leave the cfg grammar unchanged; `assemble` runs on a title with quotes/newlines.
- [ ] **Poison demo:** truncated demo, bit-flip demo and zstd bomb each end `failed` with a code, runner stays up, no re-lease loop.
- [ ] **Secrets:** grep logs, DB error columns and Telegram sends for the sentinel token after a forced failure.
- [ ] **Version skew:** old runner against new server gets a clear "обновите раннер".
- [ ] **Offline UX:** runner PC asleep -> `/status` says so, job expires with a message after TTL.
- [ ] **Autonomy preserved:** the desktop app and `cs2-cli` still work with no server configured; `crates/cli/tests/pipeline.rs` passes.
- [ ] **Server on Linux CI:** `cargo build -p server` has no Windows-only deps.

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Duplicate assignment (P3) | MEDIUM | Bump lease token, mark job `queued` with `attempt` kept, kill stale runner CS2 via cancel path, delete the extra upload |
| Lost result (P4) | LOW if file kept | Resume outbox; if local file gone, re-queue job (re-record) with apology message |
| Telegram delivery uncertain (P11) | LOW | `/resend` sends the stored file again; keep file until TTL |
| Token leaked (P8) | HIGH | Revoke at BotFather (`/revoke`) / rotate runner token, scrub logs, restart services, check bot-api working dir |
| bot-api dir lost (P6) | MEDIUM | Re-login (`logOut`/wait ~10 min), tell users to resend pending demos; jobs already copied to server storage unaffected |
| Poison demo loop (P2) | LOW | Mark job `failed(DEMO_PARSE_CRASH)`, admin `/block-hash`; fix parser after |
| Disk full (P12) | MEDIUM | Run janitor, delete expired videos/`.part`, then `VACUUM`; reject new uploads meanwhile |
| SQLite corruption/locked (P20) | MEDIUM | Restore from `VACUUM INTO` backup, replay: jobs terminal in DB win; in-flight leases are re-announced by the runner |
| Injection suspected (P1) | MEDIUM | Inspect cfg lines, rotate to no-demo-text cfg, review `Match` names, rerun the job |
| Wrong-player video (P18) | LOW | `/player` correction + `/retry`, jobs keep their snapshot |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1 Demo text injection | Runner (sanitise) + Hardening (fixtures) | Hostile-fixture test over `renderer::render` and ffgraph; cfg grammar assert |
| 2 Poison demo / bombs | Runner (limits, child process) + Server core (attempt cap) | Truncate/bit-flip/zstd-bomb tests; runner survives; job `failed` once |
| 3 Duplicate/lost assignment | Protocol + Server core + Runner | Racing-claim test; stale-token test; reconnect re-attach test |
| 4 Lost result | Runner (outbox) + Server core (atomic receive) | Kill-at-90 % test; replayed `complete` test |
| 5 Failure message undeliverable | Delivery | Fake Telegram with parse errors, 4096/1024 limits, 403 |
| 6 Local Bot API setup | Bot (spike) + Deploy/E2E | Startup self-check; 1 GB manual receive+send |
| 7 Whitelist gaps / pre-auth download | Bot + Hardening | Per-update-kind tests; no `getFile` for denied ids |
| 8 Secret leakage | Protocol + Server core + Hardening | Sentinel-token test across logs, DB, chat |
| 9 Path traversal / names | Server core + Bot + Runner | Hostile `file_name` table; canonical-path assert |
| 10 Edit flood | Delivery | Coalescer unit test with virtual clock and injected 429 |
| 11 Video send (timeouts, codec) | Delivery + Runner (assembly flags) | Timeout -> `uncertain`; ffprobe H.264/faststart assert; thumbnail < 200 kB |
| 12 Disk fill / retention | Server core + Bot + Runner | Quota rejections; janitor removes expired files; free-space check |
| 13 NAT / half-open | Protocol + Runner + Server core | TCP-proxy stall test; backoff test |
| 14 Runner busy/asleep | Protocol + Runner + Delivery | Env-class failure requeues with one notice |
| 15 Large-file transfer | Protocol + Server core + Runner + Deploy | 500 MB through the real proxy; memory flat |
| 16 getUpdates handling | Bot | Crash-between-poll-and-insert test; 409 handling |
| 17 file_id/path/unique_id | Bot | Dedupe test; path-gone test |
| 18 `/player` semantics | Bot + Protocol + Runner | Ambiguity and not-found return candidates before CS2 starts |
| 19 Protocol/FailCode drift | Protocol (first) | `cargo tree -p server` has no Windows deps; skew test |
| 20 SQLite concurrency | Server core | Concurrent claim stress; no `database is locked` |
| 21 tokio/blocking mix | Protocol/Server core (decision) | Key Decisions entry; `protocol` has no runtime dep |
| 22 Clock skew | Protocol + Deploy/E2E | Skewed-clock test; NTP documented |
| 23 Log firehose/leak | Protocol + Runner | Event cap; admin-only logs |
| 24 Cancel races | Protocol + Server core + Runner + Bot | Cancel-vs-complete test |
| 25 Fairness | Server core + Bot | Round-robin test; `/queue` ETA |
| 26 Runner config UX | Runner + Server core | Per-layer diagnostics; one-time token |
| 27-30 Minor Telegram/dev | Bot / Server core | Checklist |
| 31-35 Deployment | Deploy/E2E | Fresh-VM install from the doc; systemd restart test; port scan shows only 443 |
| 36-39 Testing | Bot/Delivery first, then Hardening, Deploy/E2E | Fakes exist before the code that needs them |

**Phase ordering implications for the roadmap**
1. Protocol first: it removes P19/P21/P22/P23 and fixes the state machine (P3/P4/P24) before two sides diverge.
2. Server core before Bot or Runner: atomic claim, verified receive, quotas (P3, P4, P12, P20).
3. The Bot phase begins with the `telegram-bot-api --local` spike (P6) because every Telegram-dependent design (file path vs HTTP, `file://` upload, timeouts) hinges on its results.
4. Input sanitisation (P1, P9) and parse caps (P2) belong to the phases that first accept foreign input (Bot, Runner), not a final "security" phase.
5. Fakes (P36-P38) are deliverables of the Bot/Delivery/Runner phases, mirroring `FakeHost` in v1.0.
6. Deploy/E2E last, but as a real exercise: fresh machine install, 500 MB through the proxy, then the first live CS2 run that also closes the v1.0 acceptance debt.

**Research flags for phases:** Bot (spike needed: local bot-api behaviour, Rust client choice and its local-path handling, `file://` upload, bind flags); Delivery (standard patterns once fakes exist); Server core (standard, but decide the threading model first); Runner (parse isolation design needs a short spike on Job Object memory limit for a child `cs2-cli`); Deploy/E2E (standard, needs a real machine).

## Sources

- Telegram Bot API docs, "Using a Local Bot API Server" and method reference (`sendVideo`: thumbnail JPEG < 200 kB, `supports_streaming`, caption 0-1024; `sendMessage` text 1-4096; 50 MB cloud limit): https://core.telegram.org/bots/api (fetched, HIGH)
- Telegram Bots FAQ, broadcast/flood limits (1 msg/s per chat, 20/min groups, ~30/s bulk; 429): https://core.telegram.org/bots/faq (fetched, HIGH)
- `tdlib/telegram-bot-api` README (`--local` benefits: unlimited download, 2000 MB upload, upload by local path, HTTP webhook, absolute `file_path`; mandatory `--api-id`/`--api-hash`; `logOut` migration steps; HTTP only, default port 8081): https://github.com/tdlib/telegram-bot-api (fetched, HIGH)
- Community notes on local-mode volumes and file saving after `getFile` (Railway threads, bigmike.help): MEDIUM, to be confirmed in the Bot spike
- This repo: `crates/renderer/src/lib.rs` (`render`, `console_safe`, `text`), `crates/demo/src/lib.rs` (`read_demo`, `decode_all`), `crates/pipeline/src/lib.rs` (`find_player`, `console_name`), `crates/store/src/lib.rs` (WAL pragmas, `Arc<Mutex<Connection>>`), `.planning/PROJECT.md`, v1.0 `PITFALLS.md` (HIGH)
- General engineering practice (lease/fencing queues, outbox pattern, SQLite WAL + `BEGIN IMMEDIATE`, systemd hardening, reverse-proxy body limits): MEDIUM-HIGH

---
*Pitfalls research for: Telegram bot + server + remote runner added to the cs2cinema CS2 montage app (v1.1)*
*Researched: 2026-10-11*
