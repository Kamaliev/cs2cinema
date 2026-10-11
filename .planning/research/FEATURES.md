# Feature Landscape

**Domain:** Telegram bot front-end for a remote video-rendering runner (render/queue bot, media-conversion bot pattern), whitelist of friends, one shared queue, one runner
**Milestone:** v1.1 "Сервер и Telegram-бот" (subsequent milestone on the existing cs2cinema app)
**Researched:** 2026-10-11
**Overall confidence:** MEDIUM-HIGH. HIGH for Telegram platform limits (read from core.telegram.org). MEDIUM for "what comparable bots do" (synthesised from the common conversion/render/queue bot pattern and Telegram conventions, not from a survey of specific bot sources). Project-fit judgments are read directly from the repo (`JobSpec`, `FailCode`, `store`, `pipeline::find_player`, `faceit`).

> Scope: only what the NEW bot/server/runner-protocol features need. Existing montage, recording, failure codes, local queue, auto-retry, tray are NOT re-researched; they appear only as dependencies.
>
> Ratings: **TS** = table stakes (missing = bot feels broken or unsafe), **DIFF** = differentiator (cheap delight, not expected), **ANTI** = do not build. Complexity: **S** = hours to a day, **M** = a few days, **L** = a week or more.

---

## Platform facts that shape every UX decision

| Fact | Consequence for the bot | Confidence |
|------|------------------------|------------|
| Cloud Bot API: bots download files only up to 20 MB, upload up to 50 MB. A local `telegram-bot-api` server lifts downloads to no limit and uploads to 2000 MB, returns an absolute local `file_path` from `getFile`, and lets the bot upload by local path / `file://` | Local server is mandatory (already decided). The bot never has to stream the demo over HTTP: it can move/hard-link the file from the local server's directory. Uploading the result can use a local path too | HIGH (core.telegram.org/bots/api, "Using a Local Bot API Server") |
| Telegram FAQ: about 1 message/second per chat sustained, 20 messages/minute in groups, 429 on excess | Progress messages must be throttled (edit at most every few seconds and only on change). Do not support group chats | HIGH (core.telegram.org/bots/faq) |
| Message edits do NOT produce a push notification; new messages do | Final result/failure must be a NEW message (the video itself), never an edit of the status message. Intermediate messages should be silent (`disable_notification`) | HIGH (long-standing Telegram behaviour; MEDIUM on exact current wording of docs) |
| Editing a message to identical text returns "message is not modified" | Status renderer must compare with last sent text before calling `editMessageText` | HIGH (well-known API error) |
| `file_unique_id` is stable across bots and users and is available without downloading the file; `file_id` of a file the bot uploaded can be re-sent without re-uploading | Cheap duplicate detection before the multi-minute download; "resend finished video" without keeping it on disk | HIGH (Bot API docs) |
| With the local server, `getFile` on a ~1-2 GB document blocks while the server downloads it from Telegram; the local server keeps its own copy in its working dir and never cleans it | Acknowledge the upload immediately, run download off the update loop, then move the file out and delete the local-server copy (otherwise disk doubles) | MEDIUM (observed behaviour of tdlib-based server; verify in the first phase) |
| Bots only see messages in a chat the user opened with them; private chat has no privacy-mode issues | Private chats only | HIGH |

---

## Table Stakes

Missing any of these = the bot feels broken, leaks the author's machine, or lets one friend jam the queue.

| # | Feature | Why expected | Complexity | Dependency on existing code | Notes |
|---|---------|--------------|------------|-----------------------------|-------|
| TS-1 | Whitelist gate on every update (message, callback, document) | Project requirement; bot is not public | S | None (new `users` table on server; admin ID from config) | Unknown user gets ONE short reply: "Нет доступа. Ваш ID: 123456789. Передайте его автору." Showing the ID lets a friend ask for access without any tooling. Rate-limit the reply (once per N minutes per unknown ID) so a stranger cannot make the bot spam |
| TS-2 | `/start` and `/help` | Every bot has them; first thing a friend types | S | None | `/start` for a whitelisted user: what to send (demo file or FACEIT link), how to set the player, current player if set. `/help` = same text plus command list. Russian, no jargon |
| TS-3 | `/player <nick or SteamID64>` (set), `/player` (show current) | Core requirement. Without a pinned player the job is meaningless | S | `JobSpec.hero_steam` + `hero_name`; `pipeline::find_player` already resolves steam_id first, then exact nick, then case-insensitive substring | Echo what was stored: "Игрок: s1mple". Accept a plain nick or a 17-digit SteamID64 (store as `hero_steam`; nick otherwise). A nick cannot be validated at set time (no demo yet) - say so: "Проверю по первой демке". If no player is set when a demo arrives: do NOT guess and do NOT queue; reply "Сначала /player ник" |
| TS-4 | Player is snapshotted into the job at enqueue time | A later `/player` must not change a job already in the queue | S | `JobSpec` is already a plain value passed to `run_job` | Store `hero_steam/hero_name` on the server job row, send them to the runner in the job payload |
| TS-5 | Accept a demo as a Telegram document (`.dem`, `.dem.zst`) | Core requirement | M | Existing `demo::read_demo` handles `.dem` / `.dem.zst`; `cs2-cli` tests check LFS pointer and magic `PBDEMS2\0` | Reply instantly ("Принял, скачиваю...") because the local server's `getFile` can take minutes. Download off the update loop. After download verify magic bytes `PBDEMS2\0` (and refuse `HL2DEMO` as "это демка CS:GO, нужна CS2"). Reject `.zip/.rar/.7z` with a clear message instead of failing later on the runner. Never trust the extension alone |
| TS-6 | Accept a FACEIT link (pasted or forwarded text) | FACEIT is the main demo source for this audience; manually saving and re-uploading a 300 MB demo is much worse UX than pasting a link. The pipeline already supports it, so it is nearly free | S (bot side) | `faceit::parse_match_id`, `DemoSource::Faceit`, `faceit::Client::demo_urls/download` | The FACEIT API key and the download stay on the RUNNER (key must not live on the shared server and not be sent to friends). Server only parses the id with the same logic and stores the link. If the runner has no key, the failure is a clear code from the runner, not a silent hang. Multi-demo matches (BO3): default to the first demo (`demo_index` 0) in v1.1, see DIFF-3 |
| TS-7 | Immediate acknowledgement with queue position | User must know the file arrived and where it stands | S | New server queue table | "Принято. Задача №17, перед вами 2." Position computed from queued+running jobs ahead |
| TS-8 | `/status` - my jobs and runner state | The universal "what is happening" command | S | `Stage::title` (Russian stage names) and `FailCode::as_str` reusable verbatim | Shows: each of my active jobs with stage, position; and one line on the runner ("раннер в сети / занят / не в сети / на паузе"). Last finished job result if no active ones |
| TS-9 | One status message edited in place for progress | Spamming a new message per stage is the classic bad render-bot behaviour and hits the 1 msg/s limit | M | `pipeline::Progress::stage` / `log` already emit stages; `Stage` enum (Скачивание, Разбор, Позиции, Геометрия, План, Запись, Склейка, Готово) | Edit only on stage change or when "take k of n" changes; coalesce to at most one edit per ~3-5 s; skip identical text; silent. Never one message per log line. The runner sends coarse progress events (stage + take counters), not the journal |
| TS-10 | Final result as a NEW message with the video | Edits give no push notification; the user must be pinged when the montage is done | S | Runner uploads `highlights.mp4`; existing `Outcome` has shot count and total length | Send via `sendVideo` with `supports_streaming`, explicit width/height/duration (otherwise wrong aspect/preview), caption "Игрок · карта · N моментов · M:SS". Fall back to `sendDocument` if the video send is rejected. Check the encoder output is H.264/yuv420p/mp4 so Telegram plays it inline (verify against current ffmpeg args in `renderer::assemble`) |
| TS-11 | Failure message with reason and hint | Core value: "останавливается быстро и объясняет" | M | `FailCode` + `FailCode::hint()` (Russian), failed stage, attempt number from the v1.0 job record | Must be a NEW message (push). Content: what failed in plain words, failed stage, hint, whether the author is informed. The hint texts today address the author (they mention `HLAE.exe` and paths) - for friends use a short friendly line per code class and show the technical hint only to the admin (see DIFF-4). Never include local paths such as `C:\Users\...` in friend-facing text |
| TS-12 | Distinct, user-actionable messages for user-caused failures | "Игрок не найден" and "это не демка CS2" are the most common friend errors and are fixable by the friend | S-M | `find_player` returns `None`, `plan_player` errors ("нет ни убийств, ни полезных гранат"). These are string errors today, not `FailCode` | Needs a new stable code class on the runner (for example PLAYER_NOT_FOUND, NO_HIGHLIGHTS, BAD_DEMO) so the bot can word them well. This is a small change in `pipeline::job` error mapping, not new recording logic |
| TS-13 | `/cancel` for a queued or running job of mine | Shared queue: a wrong file must be removable | M | Existing cancel path ("отмена за секунды", `CANCELLED` sentinel, Job Object) on the runner; server must forward a cancel message to the runner | Queued: remove on server. Running: send cancel to runner, then wait for the runner's confirmation, then reply. `/cancel` with exactly one active job cancels it; with several, show inline buttons. Only own jobs (admin: any) |
| TS-14 | `/queue` - the shared queue | A shared queue where nobody can see it breeds "is it stuck?" questions | S | None | Show first ~10 entries: position, who (first name or short alias), stage for the running one. Mark own entries. Do not show other users' nicks/players if privacy matters; names of friends are fine here |
| TS-15 | Runner offline handling: accept, warn, resume | The runner is the author's PC; it will be off often | S-M | Runner heartbeat over the existing outbound connection (new); v1.0 "pause after repeated environment failure" must surface as state "на паузе" | Do NOT reject jobs when offline: accept and say "Раннер сейчас не в сети. Задача в очереди, начну, когда он включится." Tell the user once per job, not repeatedly. `/status` shows last seen. Distinguish "не в сети" from "на паузе: <причина>" |
| TS-16 | Mid-job runner disconnect shown honestly | Home connection/PC sleep during a 20-minute recording is realistic | M | v1.0 resume (`pipeline::job::resume`, `record.json` manifest, status "Прервано") | Mark the job "связь потеряна, жду раннер" (once). When the runner returns within a lease window, resume; after the window, fail with a runner-lost code and offer retry. Never leave a job "running" forever |
| TS-17 | Per-user cap on active jobs (e.g. 3) | One friend dropping 10 demos must not jam a single-runner queue for hours | S | None | Reply "У вас уже 3 задачи в очереди. Дождитесь или /cancel." Also handle Telegram albums: several documents arrive as separate updates; send ONE aggregated reply rather than N |
| TS-18 | Demo size and free-disk check before accepting | 2 GB files + raw video pile up; a full server disk kills everything | S | Runner already has `space_pass` for its own disk | Reject files above a configurable cap (default 2 GB is already the Telegram ceiling; real CS2 demos are 100-600 MB so a soft cap like 1.5 GB is sensible). Reject with a clear message if free space on the server is below demo size + margin |
| TS-19 | Admin commands: `/adduser <id>`, `/removeuser <id>`, `/users` | Friends are added over time; editing a config file and restarting is unacceptable | S | None (users table in server SQLite). Admin IDs from config only (not from the DB) so admin cannot be removed by a bug | `/removeuser` cancels that user's queued jobs and notifies them; define explicitly what happens to a running one (recommend: let it finish, deliver, then block). Admin commands invisible and refused for non-admins |
| TS-20 | Retention / cleanup of stored demos and videos | Unattended server fills its disk otherwise | M | `store.remember_demo` / `demo_path` is the runner-side registry that already exists; server needs its own | Rules (see "Retention design" below). Includes deleting the local `telegram-bot-api` working copy after the file is taken |
| TS-21 | Secrets never reach chat or logs | Project security constraint | S | `FailCode` hints and journal lines can contain paths; FACEIT key lives in runner settings | Sanitise failure text; never echo tokens; runner token not printed in bot replies |
| TS-22 | Resumable demo download to runner and result upload from runner | A 600 MB - 2 GB transfer over a home connection WILL break; restarting from zero each time wastes tens of minutes | M-L | Pipeline expects a local `.dem` path (`DemoSource::File`, `known_demo`), so the runner can download to a cache dir first. Upload feeds the existing `Outcome` | Use HTTP Range for download and chunked/offset upload with checksum. This is a protocol feature but the user feels it directly as "never gets stuck on transfer". Show it as stages "Получение демки" / "Отправка ролика" in the status message |

---

## Differentiators

Cheap or moderate-cost things that make this bot noticeably better than a typical conversion bot.

| # | Feature | Value | Complexity | Dependency | Notes |
|---|---------|-------|------------|------------|-------|
| DIFF-1 | Demo stored once, jobs per player (dedupe by hash) | A whole team plays one match; each friend sends the SAME demo and wants their own reel. One stored copy, N jobs | M | Runner `known_demo` / `demo_path` cache | Key the demo store by `file_unique_id` early (before download) and by content hash after. Runner also caches the downloaded demo by that key so the second player's job skips the transfer. Retention is by reference count |
| DIFF-2 | Smart duplicate detection with outcomes | A 20-40 minute render must not be repeated by accident | S-M | New | Same user + same demo + same player: if already queued/running, answer "Уже в очереди, №3". If already done, answer "Этот ролик уже готов" with button "Прислать ещё раз" (reuse Telegram `file_id`) and "Сделать заново". A different player on the same demo is NOT a duplicate. For links: key = match id + player |
| DIFF-3 | Inline-button choice of map for multi-demo FACEIT matches | BO3 gives several demos; guessing the wrong one wastes a long render | M | `faceit::demo_urls` returns a list; `DemoSource::Faceit` has `demo_index` in the pipeline `Source` | Call `demo_urls` count on the runner side or ask the user to append the map number. Defer if the first phase is tight: default to demo 1 and say so |
| DIFF-4 | Two-tier failure reporting: friendly to the friend, full detail to the admin | Friends do not care about HLAE; the author needs the code, stage, hint and attempt history | S-M | `FailCode`, `JobFailure`, attempt journal from v1.0 | Friend: "Не удалось записать: игра не запустилась. Автор уже в курсе." Admin: separate message with code, stage, hint, attempts, and button to retry. Infrastructure codes (LauncherFailed, GameNotStarted, PrecheckFailed, ForeignGameRunning, PrevGameStuck, HlaeError) are admin-notify; user-caused codes (player not found, bad demo) are friend-only |
| DIFF-5 | Retry button on failure | Environment failures are the usual cause and are transient; one tap beats resending a 1 GB file | S-M | Server keeps the demo until the job is closed; runner has `resume` ("Повторить"/"Заново") | "Повторить" creates a new job reusing the stored demo (no re-upload). For runner-side resumable failures the runner-side `resume` (manifest-driven) is used. Only offered for retryable codes |
| DIFF-6 | Player listing on "player not found" | The friend's nick in the demo often differs from what they typed; 10 names with buttons fix it in one tap | M | `cs2::Match.players`, `find_player` (nick fuzzy), runner must return the roster in the failure payload | Failure carries the player list; bot shows inline buttons "Это я: <nick>"; pressing one sets `/player` and requeues. Strong UX win because player mismatch is the most likely friend-side error |
| DIFF-7 | Optional per-demo player override via caption | Send a demo with caption "s1mple" to override the pinned player for this job only | S | `JobSpec.hero_*` is per job already | Caption parsed as nick or SteamID64; does not change `/player` |
| DIFF-8 | Rough ETA, honest | Users ask "когда?" | S-M | `Outcome` shot count/length, plan `total_secs`; history of past job durations on server | After the Plan stage is known the recording time scales with total video seconds; before that, show only position and stage. Prefer a coarse range from the median of recent finished jobs ("обычно 25-40 минут") over false precision. Never promise a time while the runner is offline |
| DIFF-9 | "take k of n" in the status message | Recording is the long stage; a counter proves it is alive | S | Take loop in `pipeline::job::record_takes` and `Progress::log`; manifest | Runner emits a counter event; bot includes it in the one edited message |
| DIFF-10 | Access-request flow: stranger taps /start, admin gets [Разрешить]/[Отклонить] | Removes the "tell me your ID" round trip | S-M | New | Admin receives ID + display name with inline buttons; must still rate-limit to prevent request spam. Keep as an option, TS-1 text with the ID already works without it |
| DIFF-11 | Admin controls: `/pause`, `/resume` queue, `/runner`, `/stats` | The author can stop the queue before playing CS2 himself | S | v1.0 queue pause mechanism on the runner; `ForeignGameRunning` precheck | `/pause` is high value because recording and the author's own gameplay both need CS2. Maps to existing pause state, not new logic on the runner |
| DIFF-12 | Admin notification when the runner goes offline with waiting jobs / comes back | Queue silently stalls otherwise | S | Heartbeat from TS-15 | Notify only after a grace period (e.g. 10 min) and with jobs waiting; one message on offline, one on return. Do not broadcast to friends |
| DIFF-13 | "Send log" button for the admin | The journal is the diagnostic; opening the PC to read it defeats the point | S | `store.append_log`, per-job log, attempt journal | Sends the sanitized log as a text document. Admin only |
| DIFF-14 | Result caption and thumbnail | Preview that looks right and says what it is | S | `plan.json` has highlight kinds/shot count; ffmpeg can extract a frame | Caption limit is 1024 chars; keep it to one line plus counts |
| DIFF-15 | Register command menu with scopes (`setMyCommands`) | Friends see only their commands, admin sees the extra ones | S | None | `BotCommandScopeChat` for the admin ID |
| DIFF-16 | Local copy "ready to resend" via stored `file_id` | Lets the server delete the mp4 early (see retention) while "Прислать ещё раз" keeps working | S | None | Store `file_id` returned by the first successful `sendVideo`. Verify it still resolves; fall back to the on-disk file if within TTL |

---

## Anti-Features

| Anti-feature | Why avoid | What to do instead |
|--------------|-----------|-------------------|
| Public bot / open registration | Out of scope per PROJECT.md; a single home PC records, abuse is trivial | Whitelist only (TS-1, TS-19) |
| Daily/monthly quotas, tiers, payments | Whitelist is the trust model; quotas add state and complaints | The per-user active-job cap (TS-17) is enough. Revisit only if a friend abuses it |
| Group chat support | 20 msg/min per group, privacy mode, ambiguous "who owns the job", status-edit spam | Private chats only; ignore or politely refuse groups |
| Per-percent or per-log-line progress | Hits the 1 msg/s limit, buries chat, no extra information | Stage changes and take counters in one edited message (TS-9) |
| Exact ETA countdown | Recording time varies with game load; wrong ETAs destroy trust | Position + stage + coarse historical range (DIFF-8) |
| Sending raw journal or full stack text to friends | Leaks paths, internal terms, hint text addressed to the author | Friendly line to friends, full detail to admin (DIFF-4, TS-11) |
| In-chat montage tuning (density, camera order, chronological, encoder) | Violates "Улучшение режиссуры вне scope"; multiplies the test matrix | Fixed defaults. If wanted later, one `/settings` for a single flag |
| Multiple runners, load balancing, runner selection | Out of scope; one CS2 instance at a time | Design the job table so a `runner_id` column is possible later, but do not build scheduling |
| Server-side demo parsing or recording | Duplicates the pipeline; the server must stay a thin queue + Telegram | All analysis stays on the runner |
| Storing the FACEIT API key on the server | Shared host, more exposure, friends' access path | Key stays in runner settings; server passes only the link |
| Keeping demos and videos forever | Disk fills; Telegram already holds delivered videos | Retention below |
| Bot-side transcoding to fit size | Telegram accepts up to 2 GB via the local server; transcode wastes minutes | If the result is over the cap, fail with a clear message (rare) |
| Webhook-only bot design | Needs an inbound port on the server | Long polling against the local Bot API is simpler and works behind NAT (stack decision, noted for the architecture researcher) |
| Auto-deleting the user's messages or demo uploads in chat | Surprising; Telegram already stores them | Leave the chat alone |
| Web dashboard for the queue | New surface for little value with 3-5 users | `/queue` and `/status`; the runner app already has its own queue page |

---

## Retention design (answers "retention/cleanup")

| Asset | Where | Recommended rule | Why |
|-------|-------|------------------|-----|
| Demo uploaded via Telegram | Server | Keep while any job referencing it is queued/running/retryable. After the last job closes keep N days (default 3-7) for "сделай ещё одного игрока из той же демки". Then delete. Disk watermark (for example free space under 15 GB) evicts oldest unreferenced demos first. Never evict a demo that a queued/running job needs | A whole team often sends the same demo over several hours/days. DIFF-1 depends on this |
| Local `telegram-bot-api` working copy | Server | Move or hard-link into the demo store right after `getFile`, then remove the original | The local server never cleans up; without this the disk usage doubles |
| Result `highlights.mp4` | Server | Delete the local file shortly after successful delivery (hours to 1 day) but keep Telegram `file_id` for resend (DIFF-16). On delivery failure keep it and retry | Telegram stores the video; server disk is the scarce resource |
| FACEIT demo downloaded by the runner | Runner | Reuse via `store.remember_demo` / `demo_path`; delete by age (for example 14 days) | Already tracked, only cleanup is missing |
| Raw takes in the job folder (`clips/`, `clips_old_<ts>`) | Runner | After a SUCCESSFUL assemble and successful upload delete raw takes; keep `highlights.mp4`, `plan.json`, log for N days; on failure keep everything (retry needs it) | Raw video is tens of GB per job; the runner is now unattended so a full disk becomes a new failure class. `space_pass` already checks free space before a job; add the cleanup that makes it succeed next time |
| Job rows and logs | Server | Keep metadata (30-90 days), trim logs | For `/status`, duplicate detection and history |
| Users table | Server | Keep forever, soft-disable on `/removeuser` | Audit and re-adding |

Run cleanup on server start and on a daily timer. Log every deletion. Make it respect a dry-run flag during the first runs.

---

## Command set (answers "commands")

| Command | Who | Behaviour | Rating | Complexity |
|---------|-----|-----------|--------|------------|
| `/start` | whitelisted | Greeting, instructions, current player | TS | S |
| `/help` | whitelisted | Same plus command list | TS | S |
| `/player [nick or SteamID64]` | whitelisted | Show or set pinned player | TS | S |
| `/status` | whitelisted | My jobs + runner state | TS | S |
| `/queue` | whitelisted | Shared queue snapshot | TS | S |
| `/cancel` | whitelisted | Cancel my job; buttons if several | TS | M |
| `/adduser <id> [name]`, `/removeuser <id>`, `/users` | admin | Manage whitelist | TS | S |
| `/pause`, `/resume`, `/runner` | admin | Control queue and see runner | DIFF | S |
| `/last` | whitelisted | Resend my last finished video by `file_id` | DIFF | S |
| `/stats`, `/cleanup` | admin | Disk usage, manual cleanup | DIFF | S |
| `/broadcast` | admin | Message all users | ANTI | - |

No command for settings, quality, or camera.

---

## Feature Dependencies

```
Whitelist gate (TS-1) -> everything else
Users table + admin commands (TS-19) -> TS-1 (DB-backed list), TS-17 (per-user cap)

/player (TS-3) -> job snapshot (TS-4) -> JobSpec.hero_* on runner
Demo intake (TS-5, TS-6) -> server queue -> runner job payload -> pipeline::job::run_job
Demo store by hash (DIFF-1) -> duplicate detection (DIFF-2) -> retention (TS-20)

Runner heartbeat (TS-15) -> offline/pause messages (TS-15, DIFF-12), mid-job disconnect (TS-16)
Resumable transfer (TS-22) -> runner demo cache (DIFF-1) -> status stages "Получение/Отправка"

Runner progress events (stage, take k/n) -> status message edit (TS-9, DIFF-9) -> ETA (DIFF-8)
Runner failure payload (code, stage, attempt, roster) -> friendly failure (TS-11, TS-12) -> admin detail (DIFF-4) -> retry button (DIFF-5), player buttons (DIFF-6)

Cancel (TS-13) -> existing runner cancel path (Job Object, CANCELLED)
Retry (DIFF-5) -> kept demo (retention) + existing runner resume
```

Existing code the new features lean on (read from the repo, not modified in principle):

| Existing item | Used by |
|---------------|---------|
| `pipeline::job::JobSpec`, `DemoSource::{File, Faceit}`, `run_job`, `resume`, `Outcome` | Runner mode: server job maps 1:1 to `JobSpec` |
| `pipeline::Progress` (`stage`, `log`, `cancelled`) | Runner progress events and cancel signal to the server |
| `pipeline::Stage` + `title()` | Status message stage names (Russian) |
| `FailCode`, `FailCode::hint()`, `as_str()` | Failure payload and the friend/admin messages |
| `pipeline::find_player` (steam id, exact nick, substring) | `/player` semantics; player-not-found roster |
| `faceit::parse_match_id`, `Client::demo_urls/download` | Link intake and runner-side download |
| `store` job states (queued, running, failed, "Прервано", cancelled), `remember_demo` | Runner-side local queue stays; server needs its own mirror of states |
| v1.0 pause after repeated environment failure, `ForeignGameRunning` | "на паузе" runner state, `/pause` |
| `space_pass` (free disk check) | Runner disk guard; add post-success cleanup |

---

## MVP Recommendation

Phase-sized slices in dependency order (feeds the roadmap):

1. **Server skeleton + whitelist + intake**: TS-1, TS-2, TS-3, TS-4, TS-5, TS-6, TS-7, TS-17, TS-18, TS-19. A friend can be added, set a player, and drop a demo or link and see it queued.
2. **Runner protocol**: heartbeat, job payload to `JobSpec`, TS-22 transfers, TS-15/16 offline and disconnect states, cancel (TS-13), progress events.
3. **Result and failure delivery**: TS-8, TS-9, TS-10, TS-11, TS-12, TS-14, TS-21 plus the minimal new failure codes (player not found, no highlights, bad demo).
4. **Retention and polish**: TS-20 plus the cheap differentiators with best payoff: DIFF-2 (duplicate), DIFF-4 (admin detail), DIFF-5 (retry), DIFF-6 (player buttons), DIFF-9, DIFF-11 (`/pause`), DIFF-12.

Defer (explicitly): DIFF-3 (map choice), DIFF-8 (ETA beyond a coarse range), DIFF-10 (access-request flow), DIFF-13/14/15/16 unless time remains.

Do not build any ANTI item.

## Phase-ordering notes and research flags

- Failure codes for user-caused errors (TS-12) need a small change in `pipeline::job` before the bot can word them well: do it in the same phase that defines the runner failure payload, not later.
- Status message throttling (TS-9) and "final result is a new message" (TS-10, TS-11) are cheap if designed from day one and annoying to retrofit; put them in the first bot-facing phase.
- Retention (TS-20) must exist before the first friend uses the bot, because the local `telegram-bot-api` copy and the runner raw takes both fill disks quickly.
- Needs deeper phase research: TS-22 (resumable transfer protocol design), TS-5 (exact local `telegram-bot-api` file path behaviour on the chosen server OS), TS-10 (codec/container compatibility so Telegram plays inline).
- Standard patterns, unlikely to need research: whitelist, commands, `/player`, admin commands, queue listing.

## Sources

- Telegram Bot API, "Using a Local Bot API Server" (download no limit, upload 2000 MB, local `file_path`): https://core.telegram.org/bots/api#using-a-local-bot-api-server (HIGH, read directly)
- Telegram Bots FAQ, broadcast/flood limits (1 msg/s per chat, 20 msg/min in groups, cloud 20 MB download / 50 MB upload): https://core.telegram.org/bots/faq (HIGH, read directly)
- cs2cinema repo, read directly: `crates/pipeline/src/job.rs` (`JobSpec`, `DemoSource`), `crates/pipeline/src/record/failure.rs` (`FailCode`), `crates/pipeline/src/lib.rs` (`find_player`, `Stage`), `crates/store/src/lib.rs` (job API, `remember_demo`), `crates/faceit/src/lib.rs` (HIGH)
- Behaviour of comparable render/queue/conversion bots (single status message edited in place, final result as a new message, position in queue, per-user caps, whitelist-by-ID, retention): synthesised from established Telegram bot conventions and prior knowledge, not from a survey of specific bots (MEDIUM)
- `classify-confidence` seam was not run (no research-plan providers invoked); tiers assigned by hand: primary docs/repo = HIGH, synthesis = MEDIUM
