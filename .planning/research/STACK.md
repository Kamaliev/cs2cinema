# Stack Research — v1.1 «Сервер и Telegram-бот»

**Domain:** Telegram bot + job server + outbound-only remote runner, added to an existing sync Rust workspace (`std::thread`, `ureq` 3, `rusqlite` 0.32, `serde`)
**Researched:** 2026-10-11
**Confidence:** HIGH for versions, library capabilities and Telegram local-mode behaviour (crates.io API, library source, Telegram docs, tdlib source, and a compile-check of the recommended API calls); MEDIUM for deployment recipes (Docker volume permissions, long `getFile` timeouts, Windows hosting) which need one spike each (listed at the end).

Confidence tags: **[HIGH]** = primary source (crate source/manifest, official docs, tdlib source) or reproduced locally; **[MEDIUM]** = sound inference from a primary source or one good secondary source; **[LOW]** = guess or single community claim, do not build on it without a spike. Scope: only what is NEW for v1.1. The existing validated stack (demo/highlights/director/renderer/FACEIT/`store`/`pipeline::job`/`host`) is not re-researched.

---

## Decision summary (read this first)

| # | Question | Decision | Confidence |
|---|----------|----------|------------|
| 1 | Can tokio be avoided? | **Yes, completely.** No `tokio`, no `async` anywhere in the workspace for v1.1. | HIGH |
| 2 | Telegram library | **`frankenstein` 0.52.1, feature `client-ureq`** (blocking, typed, built on the `ureq` 3 the workspace already uses). Not `teloxide`, not hand-rolled. | HIGH |
| 3 | HTTP server for the runner API | **`tiny_http` 0.12.0** on `std::thread` workers, bound to `127.0.0.1`/tailnet, fronted by a TLS terminator (Tailscale Serve or Caddy). | MEDIUM (works, but the crate is stagnant, see risk below) |
| 4 | Runner–server transport | **Plain HTTP/1.1 request/response: long-poll `claim` (<=25 s) + progress POST that doubles as heartbeat and carries cancel back.** Not WebSocket, not SSE. | HIGH |
| 5 | File transfer | **Bounded chunks (8 MiB) with `Range` for download and `Upload-Offset` PUT for upload, SHA-256 verified at finish.** | HIGH (design), MEDIUM (chunk size, tune in spike) |
| 6 | Local `telegram-bot-api` | **Linux + Docker (`aiogram/telegram-bot-api`, tag `10.3`, `TELEGRAM_LOCAL=1`)**, one shared volume with `cs2-server`; server reads `getFile` paths from disk and sends results as `file://` URIs. Windows = WSL2/Docker or build from source (no official binary). | HIGH (mechanism), MEDIUM (volume permissions) |
| 7 | Runner auth + TLS | **256-bit random bearer token per runner, only its SHA-256 stored on the server; TLS terminated by Tailscale Serve (preferred) or Caddy; runner uses `ureq` + `platform-verifier` as today.** No in-process TLS server. | HIGH |
| 8 | Server SQLite | **`rusqlite` 0.32 `bundled` — the exact workspace version; hand-rolled `PRAGMA user_version` migrations like `store`.** Do NOT bump to 0.40 inside the server crate. | HIGH (reproduced) |

### Tokio: avoided, and what that costs

**Why it can be avoided (evidence):**
- `frankenstein` ships a first-class blocking client: `client-ureq = ["trait-sync", "dep:ureq", "dep:multipart", "dep:mime_guess", "dep:serde_json"]` (crate manifest, 0.52.1). It reuses the workspace `ureq` 3.4.2 (one HTTP/TLS stack, `ring` already in `Cargo.lock`). [HIGH]
- The load is one runner, a handful of whitelisted users, one job at a time. One blocking thread long-polling `getUpdates`, one ingestion worker, one delivery worker, and ~8 `tiny_http` worker threads is far below any thread-count concern.
- All state-owning code that the server shares a style with (`store::Store` = `Arc<Mutex<Connection>>`, `pipeline::Progress`) is synchronous; async would need `spawn_blocking` bridges around rusqlite everywhere.
- Measured dependency cost (fresh probe crates, `cargo tree`, counting unique crate versions that are NEW on top of the baseline `ureq + rusqlite 0.32 + serde + serde_json + sha2` = 57 crates):

| Candidate server stack | New crates | tokio in tree |
|------------------------|-----------:|---------------|
| **frankenstein `client-ureq` + `tiny_http` + `ctrlc` + `getrandom`** (recommended) | **34** | no |
| frankenstein `client-reqwest` + `axum` 0.8 + `tower-http` + `tokio` | 91 | yes (second HTTP stack: reqwest/hyper next to ureq) |
| `teloxide` 0.17 + `axum` + `tokio` | 115 | yes |

**What avoiding it costs (be honest):**
1. `tiny_http` is stagnant: 0.12.0 was released 2022-10-06, last commit 2023-05, repo last pushed 2024-08. It had a request-smuggling advisory (RUSTSEC-2020-0031, patched in >= 0.8.0, we use 0.12). Mitigation: it never faces the internet directly (TLS terminator in front, bind to loopback/tailnet), and the API is tiny. [HIGH on facts, MEDIUM on risk assessment]
2. No built-in `Range` serving, graceful shutdown, per-read timeouts or body-size limits: ~150 lines of hand-written code (`Range: bytes=N-` -> `206` + `Content-Range`, `Upload-Offset` check, size caps). The chunked design (below) makes per-read timeouts unnecessary.
3. No WebSocket/SSE push: cancel latency = progress-POST interval (2-5 s). Acceptable: the requirement is "cancel in seconds".
4. `frankenstein` `client-ureq` depends on the archived `multipart` 0.18 crate (RUSTSEC-2023-0050, "unmaintained", not a vulnerability) and a second `rand` (0.8 next to the workspace's 0.9). Accepted: used only for multipart uploads we mostly avoid (we send `file://` URIs instead). [HIGH]

**What accepting tokio would cost:** +57 crates over the sync option, a second TLS/HTTP stack, `spawn_blocking` around every SQLite call, and a rule that async must never leak into `wire`/`runner`/`app`/`cli`. The deviation would be legal under the PROJECT.md constraint (server crate only) but is not justified by the load.

**Escape hatch (decide later, do not build now):** switch ONLY `crates/server` to `axum` 0.8.9 + `tokio` 1.53.2 (+ `tower-http` 0.7.1 for `ServeFile` Range support) if one of these triggers fires: more than one runner, WebSocket/SSE push becomes a requirement, or `tiny_http` shows a real bug. Keep the protocol HTTP/1.1 request/response and keep `wire` runtime-agnostic so the swap does not touch runner, app or CLI.

**Hard rule for the workspace:** `tokio`/`reqwest`/`hyper`/`async-*` must not appear in the dependency tree of `app`, `cli`, `runner`, `wire` or any existing crate. Verify in CI with `cargo tree -p cli -i tokio` (must print "nothing to print"). Note: `Cargo.lock` already lists a `tokio 1.53.2` entry with only `pin-project-lite`; it is not in the build graph of any binary today (checked with `cargo tree -i tokio` -> nothing), keep it that way.

---

## Recommended Stack

### Core Technologies (new in v1.1)

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `frankenstein` | 0.52.1 (`default-features = false`, `features = ["client-ureq"]`) | Telegram Bot API client in `crates/server` | Complete typed wrapper of Bot API **10.3** (README), which equals the current Bot API (changelog: 10.3, 2026-08-24) and the `aiogram/telegram-bot-api:10.3` image. Blocking `Bot` with configurable `api_url` and `ureq::Agent` (needed for local server and long timeouts). Published 2026-09-25, repo pushed the same day. Compile-checked locally: `Bot::builder().api_url(..).request_agent(..).build()`, `get_updates`, `get_file`, `send_document` with `FileUpload::String("file:///...")` all resolve on this toolchain. |
| `tiny_http` | 0.12.0 | Runner-facing HTTP API in `crates/server` | Only mature **blocking** server in the ecosystem. `Server` is `Send + Sync`, `Request` is `Send` ("dispatch your requests to worker threads"), body is a streaming `Read` (`as_reader`, `body_length`), responses can stream any `Read` with a known length. Handles `Expect: 100-continue`. No runtime. |
| `ureq` | 3.4.2 (already in workspace; `features = ["json","platform-verifier"]`) | Runner-side client (`crates/runner`), also what `frankenstein` uses | Already vetted in the repo (`faceit`, `app::update`). Blocking `Read`-based body streaming is the pattern used in `faceit::download` (`body_mut().with_config().limit(u64::MAX).reader()`). **Defaults to NO timeouts at all** (every `timeout_*` is `None`, only `await_100` = 1 s), so the runner MUST set explicit ones (see Protocol). |
| `rusqlite` | **0.32.1 `bundled`** (workspace version, do not change) | Server state: users, whitelist, jobs, leases, runner tokens, outbox | Same crate/pattern as `crates/store` (`Arc<Mutex<Connection>>`, WAL, `user_version` migrations). **Reproduced locally:** having `rusqlite 0.32` and `rusqlite 0.40` in one dependency graph fails with `libsqlite3-sys ... links to the native library sqlite3, but it conflicts` -> the server crate cannot use the newer rusqlite (latest on crates.io is 0.40.2) unless the whole workspace, incl. `store`, is upgraded in a separate task. |
| `serde` / `serde_json` | 1.0.229 / 1.0.151 (workspace) | `wire` protocol types, server config | Already everywhere; protocol is JSON for control + raw bytes for files. |
| `sha2` | 0.10.x (workspace; latest on crates.io is 0.11.0 — do **not** bump) | SHA-256 of demos/results (streamed), hashing runner tokens | Already used in `app::update`. Hardware-accelerated SHA-256 is ~GB/s, hashing a 500 MB file adds seconds, not minutes. BLAKE3 1.8.7 is faster but adds a dependency for no user-visible gain. |
| `getrandom` | 0.4.3 (already in `Cargo.lock`) | Generate 256-bit runner tokens | `getrandom::fill(&mut [0u8; 32])` (verified in docs.rs for 0.4.3). OS CSPRNG, no `rand` needed. MSRV 1.85 is fine (toolchain is 1.99). |
| `ctrlc` | 3.5.2, `features = ["termination"]` | SIGINT/SIGTERM handling in `cs2-server` | In Docker the server is PID 1 and **ignores SIGTERM without a handler**; the handler sets an `AtomicBool` and calls `Server::unblock()`. One tiny dependency, works on Windows too. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `chrono` | 0.4.45 (workspace, `default-features = false`, `features = ["clock"]`) | Timestamps in server log/DB | Already in workspace; do not add `time`. |
| `thiserror`/`anyhow` | — | — | **Do not add.** Workspace convention is `Result<T, String>` / `Box<dyn Error>`. |
| `tracing`, `log`, `env_logger` | — | — | **Do not add.** The workspace has no logging crate. Write a 30-line `logln!` (timestamp + level to stderr; systemd/Docker collect it) plus a `redact()` helper that strips the bot token and bearer tokens. A logging framework is not worth 10+ crates for one daemon. |
| `toml`/`clap` | — | — | **Do not add.** Secrets via env (`TELEGRAM_BOT_TOKEN`, `CS2_ADMIN_ID`), the rest via flags/env or a small `server.json` read with `serde_json`; hand-written arg parsing like `cs2-cli`. |
| `rusqlite_migration` | 2.6.0 | — | **Do not add**: targets newer rusqlite and would re-introduce the `links = "sqlite3"` conflict; `store` already has a working hand-rolled migrator (`Store::migrate`) to copy. |
| `subtle` | 2.6.1 (already in lock) | constant-time compare | **Not needed.** Look the token up by `sha256(token)` in an indexed column; a 256-bit random token has nothing to time-attack. |
| `argon2`/`bcrypt` | — | — | **Not needed.** Tokens are high-entropy random, not passwords; a fast hash is correct. |

### Infrastructure / external programs (not Cargo crates)

| Tool | Version | Purpose | Why |
|------|---------|---------|-----|
| `telegram-bot-api` (tdlib) | built from `master`; Docker tag `aiogram/telegram-bot-api:10.3` (amd64/arm64/386/arm/ppc64le, pushed 2026-10-10) | Local Bot API server: lifts 20 MB download / 50 MB upload to unlimited / 2000 MB | Telegram docs ("Using a Local Bot API Server"): *download files without a size limit; upload files up to 2000 MB; upload by local path with `file://`; `getFile` returns the absolute local path*. Cloud limits verified in the docs: `getFile` "maximum file size to download is 20 MB"; multipart uploads "10 MB photos, 50 MB other files". Needs `api_id`/`api_hash` from my.telegram.org. Boost licence. |
| Docker + Compose | any current | Run `telegram-bot-api` (+ optionally `cs2-server`) on a Linux host | There is **no official prebuilt binary** and no GitHub release in `tdlib/telegram-bot-api` (releases/tags API returned `[]`); the unofficial `aiogram` image is the de-facto way. |
| Tailscale | current | Private network between the Windows runner and the server; `tailscale serve` gives a real `*.ts.net` HTTPS cert | Runner stays behind NAT with outbound-only traffic; server port is never public. Windows needs *unattended mode* (service starts before login). Direct path in most networks; falls back to DERP relay (slower) on double NAT / strict firewalls. |
| Caddy 2 | current | Alternative TLS terminator when the server has a public IP/domain | Automatic Let's Encrypt; `reverse_proxy 127.0.0.1:PORT`. |
| systemd unit or Compose service | — | Supervise `cs2-server` | Restart on failure; `EnvironmentFile` for the bot token. |

---

## Protocol decisions (runner <-> server)

### Transport: HTTP long-poll, not WebSocket, not SSE

| Criterion | Long-poll + progress POST (chosen) | WebSocket | SSE |
|-----------|-----------------------------------|-----------|-----|
| Fits blocking `ureq` runner | Native | Needs `tungstenite` 0.30 (sync, exists) + reconnect/ping state machine | `ureq` can read a streaming body, but reconnect + `Last-Event-ID` is hand-written |
| Fits `tiny_http` server | Native | `Request::upgrade` exists but framing/ping/close all hand-written | Chunked streaming works; proxies buffer unless `flush_interval -1` |
| Behind NAT / proxies / Cloudflare | Works anywhere HTTP works; Cloudflare read timeout is 125 s, keep waits <= 25-30 s | Idle-timeout tuning per hop | Same as long-lived response |
| Reconnect semantics | A retry loop with backoff **is** the reconnect logic | Separate state machine | Separate state machine |
| Cancel latency | 2-5 s (rides back in the progress POST response) | Instant | Near-instant |
| Testability | In-process server on port 0 + `ureq` | Harder | Harder |

Concrete shape (names are suggestions for the architecture phase; `wire` crate owns the types):
- `GET /v1/claim?wait=25` -> `200` + job JSON, or `204`. Runner sets `timeout_recv_response` = 40 s, `timeout_connect` = 10 s.
- `POST /v1/jobs/{id}/events` (progress/log/heartbeat) -> response includes `{"cancel": bool, "lease": ...}`. Timeout 15 s.
- `POST /v1/jobs/{id}/fail` with `{code: "TAKE_TIMEOUT", message, hint}` — `FailCode::as_str()` and the Russian `hint` already exist in `pipeline::record::failure`; the **server never depends on `pipeline`**, it forwards the strings.
- Every request carries `Authorization: Bearer <token>`.

### File transfer (hundreds of MB): bounded chunks, resumable, checksummed

`ureq` 3 has no per-read stall timeout (only whole-request timeouts), and a stalled upload would pin a `tiny_http` worker thread. Therefore **do not stream a whole file in one request**:

- **Download (server -> runner):** `GET /v1/jobs/{id}/demo` with `Range: bytes=N-M`, 8 MiB per request, per-request `timeout_global` 60 s. Server answers `206` + `Content-Range`, `ETag` = sha256 of the file. Runner writes to `<name>.part`, resumes from `metadata().len()`, verifies the full SHA-256 against the job offer, then renames. (This mirrors the `.part` + rename pattern of `faceit::download`.)
- **Upload (runner -> server):** `HEAD /v1/jobs/{id}/result` -> `Upload-Offset: N`; then `PUT` of the next 8 MiB chunk with `Upload-Offset: N` and `Content-Length` (send a `Vec<u8>`/`&[u8]` so the length is known, `ureq` `AsSendBody` supports `Vec<u8>`, `&[u8]`, `File`). Server appends only if the offset equals the current `.part` size, otherwise `409` + current size (runner re-syncs). Finish with `POST .../finish {sha256, size}`; server verifies and renames. This is the core of the tus resumable-upload idea without the library; do not add a tus crate for one client.
- **Why 8 MiB:** keeps each request well under Cloudflare's 100 MB Free/Pro body limit (so a Cloudflare Tunnel would also work), bounds the damage of a drop to one chunk, makes per-request timeouts meaningful. Tune in the spike (4-32 MiB).
- **Integrity:** SHA-256 computed while writing on both sides (`sha2::Sha256::update` per chunk); mismatch -> delete `.part`, report a typed failure. TCP alone does not protect against a truncated-then-renamed file.
- **Memory:** one chunk buffer (8 MiB) per transfer; never `read_to_end` a file.

### Auth, tokens and TLS

- **Runner token:** 32 random bytes from `getrandom::fill`, hex- or base64url-encoded, shown once at creation (`cs2-server runner add <name>` prints it; the user pastes it into the app's runner settings / `cs2-cli runner --token`). Server stores only `sha256(token)` plus name, created/last-seen, `revoked_at`. Lookup by hash column; no constant-time compare needed.
- **Never log:** the bearer token, the bot token, or any URL containing the bot token. **Trap:** the Bot API URL is `http://host:8081/bot<TOKEN>/method`, and HTTP/library errors can echo the URL; wrap every Telegram error through `redact()` before logging or sending to chat. [HIGH that the URL contains the token; MEDIUM that `ureq` errors echo it, redact defensively]
- **Telegram whitelist:** Telegram numeric user IDs in SQLite, seeded by `CS2_ADMIN_ID`; ignore (and rate-limit the log of) anyone else. The runner API is separate from Telegram identity.
- **TLS:** the Rust server speaks plain HTTP on loopback/tailnet. Do not enable `tiny_http`'s `ssl-rustls` feature (ships an old rustls and ties TLS renewal to our process). Preferred: **Tailscale Serve** (`tailscale serve` proxies a local port with an auto-provisioned Let's Encrypt cert on `<host>.<tailnet>.ts.net`; renewal handled by the daemon; resumes after reboot). Alternative with a public IP + domain: **Caddy** automatic HTTPS. The runner keeps `ureq` + `platform-verifier`, so Let's Encrypt chains validate through the Windows trust store with zero extra code. Plain `http://` should be accepted by the runner only for loopback/tailnet addresses and shown as a warning in settings.
- **Runner secrets on disk:** the token lives in the runner's `settings.json` next to the FACEIT key (same trust model as today); the settings page must mask it and the journal must never print it (PROJECT.md Security constraint).
- Cloudflare Tunnel works only because of the 8 MiB chunks (100 MB body limit on Free/Pro, 125 s read timeout, 30 s write timeout); it is not recommended as primary because Tailscale needs no public ingress at all.

---

## Telegram: local Bot API, concretely

**How it behaves (verified in Telegram docs and tdlib source `Client.cpp`):**
- Start with `--local` (Docker image: `TELEGRAM_LOCAL=1`), plus `TELEGRAM_API_ID`/`TELEGRAM_API_HASH`. Default port 8081 (`--http-port`). Data dir `/var/lib/telegram-bot-api` in the image.
- In local mode `getFile` returns `file_path` = **absolute path on the Bot API server's own filesystem**, and the file is already on disk after the call (the call blocks while the server downloads it from Telegram). I grepped `Client.cpp`/`HttpConnection.cpp`/`ClientManager.cpp`/`Query.cpp`: there is no `/file/bot<token>/...` HTTP route in the open-source server, so **`cs2-server` must share a filesystem with the Bot API container** (same host, same volume mounted at the same absolute path) rather than download over HTTP. [HIGH from source grep; the docs say the same: "without the need to download the file"]
- Sending: pass the string `file:///data/outbox/<job>/highlights.mp4` as the `document`/`video` field (`FileUpload::String`); the server accepts `file://` URIs only in local mode (`get_input_file`). Multipart upload of the same file also works (up to 2000 MB) but costs a pointless copy over loopback.
- **Migration step:** a bot token already used with the cloud API must be `logOut`-ed from `api.telegram.org` once before first use on the local server; after that it cannot return to the cloud server for 10 minutes (docs: "You must log out the bot before running it locally"). Use a fresh bot or do this once, manually.
- Updates: use `getUpdates` long polling (`timeout = 30`), not webhooks. Local mode allows plain-HTTP/local webhooks, but webhooks would add an inbound endpoint and a second listener for no gain.
- Timeouts: `frankenstein`'s default agent has `timeout_global` = 500 s. Build **two** `Bot`s from the same URL: one for `getUpdates` (global ~60 s) and one "heavy" bot (`getFile` of a ~1 GB demo and `sendVideo`/`sendDocument` of a large result block until Bot API finishes the transfer; set 30 min) — [MEDIUM, size the timeout in a spike].
- Threads: never call `getFile`/`send*` from the `getUpdates` loop; hand them to an ingestion/delivery worker via `mpsc`, so `/status` and `/cancel` stay responsive during a 2 GB transfer.
- Sending video: prefer `sendDocument` as the always-works fallback and `sendVideo` with `supports_streaming`, `width`, `height`, `duration` (from `ffprobe`, already a dependency) for inline playback.
- Disk: Bot API keeps downloaded files in its data dir. Move (`rename`) the demo into server storage right after `getFile` and delete leftovers on a retention sweep; budget disk for demo + result + Bot API copy.

**Recommended deployment (Linux host):**

```yaml
# docker-compose.yml (sketch; paths/uids are the spike)
services:
  bot-api:
    image: aiogram/telegram-bot-api:10.3
    restart: unless-stopped
    environment:
      TELEGRAM_API_ID: ${TELEGRAM_API_ID}
      TELEGRAM_API_HASH: ${TELEGRAM_API_HASH}
      TELEGRAM_LOCAL: "1"
    ports: ["127.0.0.1:8081:8081"]        # never publish to the internet: URL contains the bot token
    volumes: ["botdata:/var/lib/telegram-bot-api"]
  cs2-server:
    image: cs2-server:local                # or a systemd service on the host with the same bind mount path
    restart: unless-stopped
    environment:
      TELEGRAM_BOT_TOKEN: ${TELEGRAM_BOT_TOKEN}
      BOT_API_URL: http://bot-api:8081
    volumes: ["botdata:/var/lib/telegram-bot-api", "serverdata:/data"]
    ports: ["127.0.0.1:8080:8080"]         # fronted by tailscale serve / Caddy
volumes: { botdata: {}, serverdata: {} }
```

**If the server must be Windows:** (a) run the same Compose file under Docker Desktop/WSL2 and run `cs2-server` in WSL2 too (path namespaces differ between Windows and WSL, so keep both processes on the Linux side); or (b) build `telegram-bot-api` from source with MSVC (README: needs OpenSSL, zlib, C++17 compiler, gperf, CMake 3.10+; supported compiler MSVC 19.1+; the tdlib build-instruction generator offers a Windows/PowerShell path). No official Windows binary exists. (b) is considerably more work for no benefit. Recommendation to the author: **put the server on Linux** (cheap VPS or home box). [HIGH that no official binary exists; MEDIUM on effort of (b)]

---

## SQLite for the server

- `rusqlite` 0.32.1 `bundled`, same `Connection` setup as `store`: `PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;` plus `busy_timeout`.
- Migrations: copy the `store` approach (a slice of SQL steps, step `k` raises `user_version` to `k+1` inside one transaction). Add a test that opens at every old version, like `store`'s `user_version` tests.
- One writer: either `Arc<Mutex<Connection>>` (as `store`) or a dedicated DB thread owning the connection and receiving commands over `mpsc` (matches the "actor" idea in ARCHITECTURE.md). Both are sync; pick the actor if lease/state-machine transitions need to be atomic across bot and API threads.
- Separate database file and schema from the desktop `cs2cinema.db`; reuse the *pattern*, not `crates/store` itself (it carries app-specific job/FACEIT tables).
- Back up with `VACUUM INTO` or file copy while WAL is checkpointed; at this scale that is a cron line, not a library.

---

## Workspace integration (which crate may depend on what)

| Crate (new/changed) | Allowed new dependencies | Must NOT depend on |
|---------------------|--------------------------|--------------------|
| `wire` (new) | `serde`, `serde_json` only | anything with I/O, `pipeline`, `host`, any HTTP crate |
| `server` (new, lib + bin `cs2-server`) | `wire`, `frankenstein` (`client-ureq`), `tiny_http`, `ureq`, `rusqlite` 0.32 (bundled), `sha2`, `getrandom`, `ctrlc`, `serde*`, `chrono` | `pipeline`, `host`, `demo`, `positions`, `geometry`, `app`, `store` — keeps the Linux build small and free of Windows-only code and `source2-demo` |
| `runner` (new) | `wire`, `ureq`, `sha2`, `serde*` | `frankenstein`, `tiny_http`, `rusqlite`; stays sync, driven from a `std::thread` (app queue thread or CLI loop) |
| `app`, `cli` | `runner`, `wire` | any server-side crate |
| `pipeline`, `host`, `store`, ... | unchanged | `wire`/`server`/`runner` (the DAG stays one-directional; the runner adapts `pipeline::Progress` into `wire` events at the edge) |

Version-pinning rules: use the workspace's `ureq` 3 (`json`, `platform-verifier`), `rusqlite` 0.32, `sha2` 0.10; add `frankenstein`'s `ureq` requirement is `3.0.0` with `rustls` so Cargo unifies on the single 3.4.2 already in `Cargo.lock` (confirmed: probe resolves one `ureq` v3.4.2). The new server crate adds `rand 0.8` (via `multipart`) and `bon`/`serde_with`/`mime_guess` (via `frankenstein`) to the lockfile; they never reach `app`/`cli`.

CI: add a job that builds `cs2-server` on Linux (the real target) and runs `cargo tree -p cli -i tokio` / `-i reqwest` (must be empty). The existing `crates/cli/tests/pipeline.rs` contract is untouched because no existing crate gains a dependency.

---

## Installation

```toml
# crates/wire/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# crates/server/Cargo.toml
[dependencies]
wire = { path = "../wire" }
frankenstein = { version = "0.52", default-features = false, features = ["client-ureq"] }
tiny_http = "0.12"
ureq = { version = "3", features = ["json", "platform-verifier"] }   # same as faceit/app
rusqlite = { version = "0.32", features = ["bundled"] }               # MUST match crates/store
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
getrandom = "0.4"
ctrlc = { version = "3.5", features = ["termination"] }
chrono = { version = "0.4", default-features = false, features = ["clock"] }

# crates/runner/Cargo.toml
[dependencies]
wire = { path = "../wire" }
ureq = { version = "3", features = ["json", "platform-verifier"] }
sha2 = "0.10"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

```bash
# Server host (Linux): local Bot API
docker compose up -d bot-api
# one-time, only if the bot token was ever used with api.telegram.org (a fresh bot does not need it)
# After logOut the bot can start on the local server at once, but cannot return to the cloud server for 10 minutes.
curl https://api.telegram.org/bot<TOKEN>/logOut
```

Verified API sketch (compiled against frankenstein 0.52.1 / tiny_http 0.12.0 / ureq 3.4.2):

```rust
let agent = ureq::Agent::new_with_config(
    ureq::config::Config::builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .build(),
);
let heavy = Bot::builder().api_url("http://127.0.0.1:8081/bot<TOKEN>").request_agent(agent).build();
let p = GetUpdatesParams::builder().timeout(30u32).allowed_updates(vec![AllowedUpdate::Message]).build();
let sent = SendDocumentParams::builder().chat_id(chat).document(FileUpload::String("file:///data/outbox/42/highlights.mp4".into())).build();
let server = tiny_http::Server::http("127.0.0.1:8080")?;   // share via Arc, N worker threads call recv()
```

---

## Alternatives Considered

| Recommended | Alternative | When to use the alternative |
|-------------|-------------|-----------------------------|
| `frankenstein` `client-ureq` | `teloxide` 0.17.0 (2025-07-11) | A conversational bot with dialogue state machines and many commands. It needs tokio + reqwest, costs +115 crates in our probe (vs +34), and our bot has about five commands (`/start`, `/player`, `/status`, `/cancel`, document upload). Overkill. |
| `frankenstein` `client-ureq` | Hand-rolled Bot API over `ureq` + `serde_json` | Only if `frankenstein` is abandoned. It is the thin typed layer we would write anyway (Update/Message/Document/File/SendDocument types, multipart); maintenance is active and tracks Bot API releases within weeks. Keep our own `trait` seam (`Telegram` with 4-5 methods) so swapping later is cheap. |
| `tiny_http` (sync) | `axum` 0.8.9 + `tokio` 1.53.2 + `tower-http` 0.7.1 | The escape-hatch triggers above (multi-runner, push channel, real `tiny_http` defect). Best-in-class HTTP, built-in Range via `tower-http::ServeFile`, timeouts/limits layers. Confined to `crates/server`. |
| `tiny_http` | `rouille` 3.6.2 (2023-04) | Adds routing/input helpers on top of `tiny_http`, but is itself stagnant and routing for ~8 endpoints is a `match` on `(method, path)`. |
| `tiny_http` | `hyper` 1.12 / `reqwest` 0.13 | Async-only; same tokio cost as axum with less convenience. |
| Long-poll | WebSocket (`tungstenite` 0.30 / `tokio-tungstenite` 0.30), SSE | Add only if sub-second cancel or live progress streaming becomes a requirement; table above. |
| Own chunk protocol | tus protocol / `rustus` | Several clients, browser uploads. For one runner a 100-line `HEAD`+`PUT offset` is simpler than a spec implementation. |
| SHA-256 (`sha2`) | `blake3` 1.8.7 | Only if hashing multi-GB files becomes a measurable bottleneck. |
| Tailscale Serve | Caddy auto-HTTPS | Server has a public IP and a domain, and you do not want the runner PC on a tailnet. |
| Linux + Docker for `telegram-bot-api` | Build from source on Windows | Only if the server must be a native Windows process. |

## What NOT to Use

| Avoid | Why | Use instead |
|-------|-----|-------------|
| `tokio`, `reqwest`, `hyper`, `async-std` anywhere outside a (future, optional) server crate | Violates the stack constraint; second HTTP/TLS stack; `+57` crates over the sync option (measured) | `ureq` + `std::thread` |
| `teloxide` | Needs tokio/reqwest; +115 crates; dialogue framework we do not use | `frankenstein` |
| `frankenstein` `client-reqwest` | Pulls tokio + reqwest 0.13 + hyper | `client-ureq` |
| Cloud `api.telegram.org` for file transfer | 20 MB download / 50 MB upload limits (verified in Bot API docs) | Local `telegram-bot-api --local` |
| `rusqlite` >= 0.33 in the server crate | `libsqlite3-sys` `links = "sqlite3"` conflict with `store` (reproduced) | `rusqlite` 0.32 until a workspace-wide upgrade |
| `sha2` 0.11 / `rand` 0.10 / `getrandom` older duplicates | Version splits inside the workspace for no gain | `sha2` 0.10, `getrandom` 0.4 |
| `tiny_http` `ssl-rustls` / `ssl-openssl` features | Old TLS stack inside our process, cert renewal becomes our job | Tailscale Serve or Caddy in front |
| One giant streaming request per file | `ureq` has no stall timeout; a dead upload pins a `tiny_http` thread; breaks behind 100 MB-body proxies; no resume | 8 MiB `Range`/`Upload-Offset` chunks |
| Downloading Telegram files over HTTP from the local Bot API | There is no HTTP file route in the open-source server | Read the absolute `file_path` from the shared volume |
| Publishing port 8081 or 8080 on a public interface | 8081 URL contains the bot token in clear; 8080 is plain HTTP | Bind `127.0.0.1`, front 8080 with TLS terminator |
| `unwrap()` on `Bot`/`ureq` errors that may contain URLs in logs | Leaks the bot token (URL path) | `redact()` all external error text before logging or replying |
| Storing runner tokens in plaintext on the server | A DB leak equals runner takeover | Store `sha256(token)` only |
| A logging/CLI/config framework (`tracing`, `clap`, `toml`) | Not used anywhere in the workspace; weight without need | `eprintln!`-style helper, hand-written flags, env + JSON |

## Stack Patterns by Variant

**If the server host is Linux (recommended):**
- Compose: `bot-api` + `cs2-server`, shared `botdata` volume mounted at the same path in both, `tailscale serve --bg 8080` on the host.
- Because the `file_path` the Bot API returns is valid verbatim inside `cs2-server`, no path translation layer is needed.

**If the server host is Windows:**
- Run both under WSL2/Docker; do not mix a native Windows `cs2-server` with a containerized Bot API (paths like `/var/lib/telegram-bot-api/...` do not exist on Windows). Native Windows is possible only with a self-built `telegram-bot-api.exe`.

**If the server has a public IP + domain and no tailnet:**
- Caddy: `example.org { reverse_proxy 127.0.0.1:8080 }`; same token auth; consider Caddy `request_body { max_size 16MB }` as a free body-size cap.

**If a second runner or push-cancel is ever required:**
- Move `crates/server` to axum + tokio (confined), keep `wire`, `runner` and the long-poll endpoints unchanged; add WebSocket as an optional upgrade, not a replacement.

## Version Compatibility

| Package A | Compatible with | Notes |
|-----------|-----------------|-------|
| `frankenstein` 0.52.1 (`client-ureq`) | `ureq` 3.x | Its own dep is `ureq = "3.0.0"` (`rustls`); unifies with workspace `ureq` 3.4.2 `platform-verifier` (one copy in the tree, verified via `cargo tree`). |
| `frankenstein` 0.52.1 | Bot API 10.3 / `aiogram/telegram-bot-api:10.3` | README states "up-to-date with version 10.3"; Telegram changelog lists 10.3 (2026-08-24) as latest. Pin the Docker tag to `10.3` rather than `latest`. |
| `rusqlite` 0.32.1 | `libsqlite3-sys` 0.30.x | Only one `sqlite3`-linking package may exist in the graph; bump `store` and `server` together in a dedicated task if ever needed. |
| `getrandom` 0.4.3 | Rust >= 1.85 | Toolchain is 1.99 / edition 2024. |
| `tiny_http` 0.12.0 | std threads, no runtime | MSRV ~1.57. |
| `ctrlc` 3.5.2 (`termination`) | Linux SIGTERM, Windows | Required for clean `docker stop`. |
| `axum` 0.8.9 / `tokio` 1.53.2 / `tower-http` 0.7.1 / `tungstenite` 0.30.0 | — | Only for the escape hatch; not part of the baseline. |

## Open spikes (each one task, before or at the start of the phase that needs it)

1. **Bot API shared volume permissions** [MEDIUM]: files created by the `telegram-bot-api` user must be readable (and movable) by `cs2-server`, and `cs2-server` must be able to place `outbox/` files that Bot API can read for `file://` uploads. Check uid/gid, `chmod`, and `rename` across the same volume.
2. **Large `getFile`/`send*` timeouts** [MEDIUM]: measure a 500 MB-1.5 GB demo through the local server; confirm `getFile` blocks until the file is on disk and that a 30-minute agent timeout is sufficient; confirm what Telegram clients show for a 400 MB MP4 sent via `sendVideo` vs `sendDocument`.
3. **`tiny_http` under load shape** [MEDIUM]: 8 MiB `PUT` chunks, `Expect: 100-continue` from `ureq`, keep-alive reuse, 25 s long-poll on a worker, dropped-connection behaviour (does the worker get an `Err` promptly behind Tailscale/Caddy?). If it misbehaves, trigger the axum escape hatch before building more on top.
4. **Chunk size and timeouts** [MEDIUM]: 4/8/16/32 MiB over the real Windows-runner-to-server link (including DERP-relayed Tailscale); set `timeout_global` per chunk accordingly.
5. **Tailscale Serve on the server + unattended mode on the Windows runner** [MEDIUM]: confirm the runner reaches `https://<host>.<tailnet>.ts.net` through `ureq` `platform-verifier` after a reboot, with no user logged in.
6. **Token redaction** [HIGH priority, small]: unit test that an error from `ureq`/`frankenstein` for a URL containing `bot<TOKEN>` never reaches the log, the DB or a chat message unredacted.

## Sources

- crates.io API (queried 2026-10-11): `frankenstein` 0.52.1 (published 2026-09-25; features `client-ureq`/`client-reqwest`), `tiny_http` 0.12.0 (2022-10-06), `rouille` 3.6.2 (2023-04-24), `axum` 0.8.9 (2026-04-14), `tokio` 1.53.2, `teloxide` 0.17.0 (2025-07-11), `tungstenite` 0.30.0, `ureq` 3.4.2, `rusqlite` 0.40.2 (latest; workspace uses 0.32.1), `sha2` 0.11.0 (latest; workspace 0.10), `getrandom` 0.4.3, `ctrlc` 3.5.2, `tower-http` 0.7.1, `blake3` 1.8.7 — HIGH.
- `frankenstein` repo `Cargo.toml`, `README.md`, `src/client_ureq.rs` (raw GitHub, master) — blocking client, `Bot::builder().api_url().request_agent()`, default 500 s agent timeout, multipart streaming via `multipart` 0.18 — HIGH.
- Local compile-check and `cargo tree` probes (scratch crates; frankenstein+tiny_http+rusqlite 0.32+ctrlc resolves and `cargo check` passes on Rust 1.99; dependency counts 34 / 91 / 115; `rusqlite 0.32 + 0.40` fails on `links = "sqlite3"`) — HIGH.
- docs.rs `ureq` 3.4.2 (`ConfigBuilder` defaults: no timeouts; `AsSendBody` impls), `tiny_http` 0.12.0 (`Server`, `Request`), `getrandom` 0.4.3 (`fill`) — HIGH.
- Telegram Bot API docs, https://core.telegram.org/bots/api — "Using a Local Bot API Server", `getFile` (20 MB), multipart upload limits (10 MB photos / 50 MB other), `logOut` (10-minute rule), `getUpdates` timeout; changelog https://core.telegram.org/bots/api-changelog (Bot API 10.3, 2026-08-24) — HIGH.
- https://github.com/tdlib/telegram-bot-api README and `telegram-bot-api/Client.cpp` (`--local`, `file://` input in `get_input_file`, absolute `file_path` in `json_store_file`; no HTTP file route found by grep) — HIGH; GitHub releases/tags API returned empty (no official binaries) — HIGH.
- Docker Hub `aiogram/telegram-bot-api` (tags `latest`/`10.3`, multi-arch, `TELEGRAM_LOCAL`, `TELEGRAM_API_ID/HASH`, data dir `/var/lib/telegram-bot-api`) — HIGH for tags, MEDIUM for env semantics (README via search summary).
- RustSec advisory-db: RUSTSEC-2020-0031 (`tiny_http` request smuggling, patched >= 0.8.0), RUSTSEC-2023-0050 (`multipart` unmaintained) — HIGH.
- Cloudflare docs (Workers/limits, connection limits, error 524): 100 MB request body on Free/Pro, 125 s proxy read timeout — MEDIUM (secondary search summary).
- Tailscale docs (Serve HTTPS certs, unattended mode on Windows, DERP fallback) — MEDIUM (search summary, not hands-on).
- Workspace files read: `.planning/PROJECT.md`, `.planning/codebase/STACK.md`, `ARCHITECTURE.md`, `Cargo.toml`/`Cargo.lock`, `crates/store` (migrations, WAL), `crates/faceit` (`download` streaming pattern), `crates/pipeline/src/record/failure.rs` (`FailCode::as_str`, `RecordFailure.hint`), sibling `.planning/research/ARCHITECTURE.md` (decisions D1, D3, D7 are consistent with this document).

---
*Stack research for: Telegram bot + job server + outbound-only remote runner (v1.1)*
*Researched: 2026-10-11*
