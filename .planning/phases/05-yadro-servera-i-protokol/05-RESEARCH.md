# Phase 5: Ядро сервера и протокол - Research

**Researched:** 2026-10-11
**Domain:** Synchronous Rust HTTP job server (tiny_http + rusqlite) with lease/epoch job queue, resumable chunked file transfer, token auth and secret redaction
**Confidence:** HIGH on tiny_http behaviour (source read + executed spike), HIGH on SQLite/claim mechanics (executed), MEDIUM on Linux-specific socket behaviour (spike ran on Windows; no Linux/WSL available locally, CI ubuntu is the first Linux run)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** HTTP-сервер — `tiny_http` 0.12, синхронно на `std::thread`, без tokio. Спайк в начале фазы проверяет `PUT` по 8 МиБ, `Expect: 100-continue` и обрывы. Если спайк провален — запасной выход `axum` 0.8 + `tokio` только внутри `crates/server`, решение записать в Key Decisions. — **Reversibility:** costly — смена HTTP-стека трогает все обработчики и тестовый клиент протокола.
- **D-02:** Параллелизм: фиксированный пул потоков (стартово ~16) принимает запросы; long-poll `claim` (до 25 с) занимает поток из пула; при переполнении сервер отвечает 503. Число потоков — настройка.
- **D-03:** TLS снаружи (Tailscale Serve или Caddy). `cs2-server` слушает HTTP на настраиваемом bind-адресе (по умолчанию loopback). Детали развёртывания — фаза 9, здесь только настройка адреса.
- **D-04:** `Arc<Mutex<rusqlite::Connection>>` (как в `crates/store`), `rusqlite` строго 0.32.1 (иначе конфликт `links = "sqlite3"`). Атомарная выдача задачи — одна транзакция под мьютексом. Миграции вручную через `user_version`.
- **D-05:** Автомат состояний и аренды — ядро без HTTP: чистые функции над хранилищем с внедрённым трейтом `Clock` (виртуальные часы в тестах). HTTP — тонкая обёртка. Тесты протокола (чанки, докачка, обрывы) идут через реальный HTTP на localhost.

### Claude's Discretion
- Структура модулей `crates/server`, имена таблиц и колонок, точный формат ошибок API.
- Числа аренды (heartbeat 10 с, TTL 45 с, long-poll 25 с) и лимит выдач — стартовые из роадмапа, подстраиваются тестами.
- Протокол докачки чанков, хранилище файлов, формат токенов, конфиг и `submit` — не обсуждались; решать по исследованию (`research/ARCHITECTURE.md`, `PITFALLS.md`) и критериям успеха роадмапа.

### Deferred Ideas (OUT OF SCOPE)
Не обсуждались в этой сессии (оставлены на усмотрение исследования/планирования): модель аренд и повторных выдач, протокол докачки и хранилище файлов, формат токенов/конфига/`submit`.

(Phase boundary, verbatim: "Отдельная программа `cs2-server` на Linux со своей SQLite: пользователи, задачи, раннеры, файлы; автомат состояний задачи с арендами и эпохой; токены раннеров (хеш); передача файлов кусками по 8 МиБ с докачкой и SHA-256; `cs2-server submit`; токены не попадают в журналы (SEC-04). Проверяется без Telegram и без CS2. Требования: SRV-01…SRV-08, SEC-04. Бот, раннер и защита входа — фазы 6–7.")
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SRV-01 | Сервер запускается отдельной программой `cs2-server` на Linux, настройки берёт из файла и переменных окружения | Config section (JSON file + `CS2_SERVER_*` env, `Secret` newtype, `STATE_DIRECTORY` default), `ctrlc` graceful stop, CI Linux build |
| SRV-02 | Сервер хранит пользователей, задачи, раннеров и файлы в своей SQLite с миграциями | Schema sketch (users/runners/jobs/files/job_events), `MIGRATIONS` + `user_version` pattern copied from `crates/store`, executed SQLite spike |
| SRV-03 | Задача проходит состояния receiving → queued → assigned → running → uploading → done / failed / cancelled; переходы проверяются тестом с виртуальными часами | Transition table, pure `decide()` + `Clock` trait, table-driven + randomized sequence tests |
| SRV-04 | Задача выдаётся раннеру атомарно с эпохой аренды; потеря аренды возвращает задачу в очередь, не останавливая запись на раннере | `UPDATE ... RETURNING` claim in `BEGIN IMMEDIATE` (executed), epoch fencing, reversible lease loss (`lease_lost` + re-lease by `hello`/`progress`), idempotent re-delivery of un-started assignment |
| SRV-05 | Раннер входит по токену (на сервере хранится только хеш), токен можно отозвать | `cs2r_` + 32 random bytes hex, `sha256` hash column, `revoked_at`, revoke expires held leases |
| SRV-06 | Демка и результат передаются кусками по 8 МиБ с докачкой и проверкой размера и SHA-256 | HEAD offset + `PUT` chunk + `Range` GET protocol, `.part` per epoch, re-hash on finish; tiny_http transport behaviour measured in spike |
| SRV-07 | Число повторных выдач одной задачи ограничено | `grants` counter checked at claim time, job -> `failed` with `TOO_MANY_GRANTS`; reclaim of same epoch does not count |
| SRV-08 | Команда `cs2-server submit` ставит задачу без Telegram | `submit` ingests file in-process (streaming copy + sha256) directly against the DB; WAL + `BEGIN IMMEDIATE` + `busy_timeout` for the two-process case |
| SEC-04 | Токены бота и раннеров не попадают в журналы и сообщения; тест с подставным токеном | `Secret` type, central `redact()` (exact values + pattern scan), sentinel test scanning log sink, DB file bytes (incl. WAL) and API responses |
</phase_requirements>

## Summary

The stack decision (tiny_http 0.12 on `std::thread`, `Arc<Mutex<Connection>>`, rusqlite 0.32.1) holds up: I read the tiny_http 0.12.0 source and executed a spike (tiny_http 0.12.0 + ureq 3.4.2 + sha2 + rusqlite 0.32.1, edition 2024, rustc 1.99.0). 8 MiB `PUT` with a known `Content-Length` works, keep-alive reuse works, `Expect: 100-continue` works, a client abort mid-body is observable, `Range` responses with a real `Content-Length` work, and a dispatcher-plus-bounded-workers pattern produces a clean 503 on overflow. **The spike passes, so the axum/tokio escape hatch is not triggered.** It also exposed three tiny_http weaknesses the plan must design around: (1) there is no socket read timeout, so a stalled client pins a worker forever; (2) an unread request body is drained inside `Drop` (blocking), so even a 401/409 reject of a `PUT` can pin a worker if the client stalls; (3) an aborted body shows up as a clean EOF (`Ok(0)`), not an error, so handlers must compare bytes read to `Content-Length`. None of these is a blocker for a token-gated, loopback/tailnet-only server, but they define the shape of the HTTP layer (dispatcher thread + bounded workers + 503, reject-on-a-side-thread, byte-count check) and must be written down as accepted risks in Key Decisions.

The load-bearing design is the lease/epoch state machine, and it should be built as a pure `decide(job, event, now_ms)` function plus a thin transactional wrapper, tested with a manual clock. The recommended model: `claim` is one `BEGIN IMMEDIATE` transaction using `UPDATE ... RETURNING` (verified to hand each job to exactly one of four racing connections) that bumps `epoch` and `grants`; lease expiry only moves the job back to `queued` with `lease_lost=1` and the *same* epoch; `hello`/`progress` carrying the current epoch re-attach a `lease_lost` job if nobody claimed it (satisfies "тот же раннер после обрыва связи возвращает её себе"); every mutating call is fenced by `epoch` (409 otherwise); poison protection is a `grants` ceiling checked at claim time. File transfer is HEAD-offset + one-chunk-per-request `PUT`, with the `.part` file size as the only source of truth for the offset, one `.part` per epoch, and the full SHA-256 recomputed at `finish`.

SEC-04 is achievable with three cheap layers: a `Secret` newtype (no `Serialize`, redacted `Debug`), a central `redact()` that every log line and every runner-supplied free-text field passes through (exact registered values plus pattern scanners for `cs2r_<64 hex>`, `Bearer ...`, and the Telegram `\d+:[A-Za-z0-9_-]{30,}` shape), and a test that forces failures and then greps the log sink, the raw bytes of `server.db` and `server.db-wal`, and every API response body for the sentinel strings.

**Primary recommendation:** Build `wire` (serde types only) then `server` as lib + bin `cs2-server` with these seams: `Clock` trait, pure `core::decide`, `Db` = `Arc<Mutex<Connection>>` with `BEGIN IMMEDIATE` writes, a dispatcher thread feeding a bounded worker pool with 503 overflow, 8 MiB chunk PUT/Range GET, `Secret` + `redact()`; test the core with a manual clock and the transport over real loopback HTTP using `ureq` with `http_status_as_error(false)`.

## Project Constraints (from CLAUDE.md)

Extracted actionable directives from `./.claude/CLAUDE.md` (treated as locked):
- **Tech stack:** Rust cargo workspace, `std::thread` without tokio, SQLite via rusqlite; "не менять стек, встраиваться в существующие крейты". All crates use edition 2024 (verbatim from `crates/store/Cargo.toml`: `edition = "2024"`).
- **Concurrency:** one recording at a time (runner-side; server does not change this).
- **Compatibility:** `cs2-cli` must keep building and `crates/cli/tests/pipeline.rs` must stay green. Phase 5 adds only new crates and two workspace `members`; no existing crate may gain a dependency.
- **Language:** user-facing strings in Russian; identifiers in English; doc comments in Russian (new `//!`/`///` comments and error messages in Russian). Test names are sentences (`job_lifecycle`, `rejects_garbage` style).
- **Errors:** `Result<T, String>` (as in `store`/`pipeline`) or local alias; no `anyhow`/`thiserror`; `?` by default; no `unwrap()` outside tests/invariants.
- **Logging:** no logging crate in the workspace; CLI uses `println!`/`eprintln!`; hand-written arg parsing; CLI errors printed as `ошибка: {e}` and `ExitCode`.
- **Style:** `cargo fmt` before commit; no clippy `allow`s beyond existing; prefer an options struct over many arguments; `pub` only at crate root API, internals private.
- **Migrations pattern:** `MIGRATIONS: &[&str]` + `PRAGMA user_version`, "Новая версия — только новым шагом в конце списка и только добавлением колонок или таблиц."
- **CI:** `.github/workflows/ci.yml` runs `cargo test --release --workspace` on **ubuntu-latest and windows-latest**; therefore all server tests must pass on both OSes (no unix-only test code; unix-only hardening must be `cfg(unix)`).
- **Dependency rule from phase criteria:** `cs2-server` must not pull `host`, `pipeline`, `demo` (and by extension `store`, `cs2`, `positions`, `geometry`, `app`).
- **GSD workflow:** edits happen through GSD commands (this research writes only RESEARCH.md).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| TLS termination | Reverse proxy / Tailscale Serve (outside process) | — | D-03: `cs2-server` speaks plain HTTP on loopback; never enable tiny_http `ssl-*` features |
| Request timeouts / idle-connection reaping | Reverse proxy | HTTP edge (bounded pool + 503) | tiny_http has no socket timeouts (see Pitfall 2); proxy is the only real timeout owner |
| Routing, auth, header/body parsing, status mapping | HTTP edge (`server::http`/`api`) | — | Thin wrapper; no business rules |
| Job state transitions, lease/epoch/fencing, grant ceiling | Core (`server::core`) | DB | D-05: pure decision function over a row + event + `now`; DB layer applies it in one tx |
| Atomic claim / queue ordering | DB (single `BEGIN IMMEDIATE` tx) | Core (eligibility rules) | Atomicity must be a DB property, not a code convention |
| Lease expiry sweep, startup grace | Core (called by timer thread and lazily by `claim`) | HTTP edge (timer thread) | Core is clock-driven so tests need no threads |
| Token issue / hash / lookup / revoke | Core (`server::auth`) | CLI (admin commands) | No admin HTTP surface; operator uses CLI on the server |
| Byte transfer (HEAD/PUT/Range GET), `.part` handling, final hash | Blob store (`server::blobs`) | HTTP edge | Filesystem is the offset truth; never hold the DB mutex during file IO |
| Demo ingest for `submit` | Blob store + Core | CLI | Streaming copy + sha256, then `receiving -> queued` in one tx |
| Secret handling / redaction | Cross-cutting (`server::secret`, `server::log`) | every tier | One choke point for all text leaving the process |
| Config loading | CLI/bootstrap (`server::config`) | — | file < env < flags; secrets typed as `Secret` |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `tiny_http` | 0.12.0 | Blocking HTTP/1.1 server | Locked by D-01. Source read + spike executed. Depends only on `ascii`, `chunked_transfer`, `httpdate`, `log` (no TLS features enabled) |
| `rusqlite` | 0.32.1, feature `bundled` | Server DB | Locked by D-04. Bundled SQLite is 3.46.0 (executed `select sqlite_version()`): supports `UPDATE ... RETURNING`, WAL, `STRICT` |
| `sha2` | 0.10.x (lock: 0.10.9) | Streaming SHA-256 of files, token hashing | Already in workspace; do not bump to 0.11 |
| `getrandom` | 0.4.3 (already in `Cargo.lock`) | 32-byte runner tokens via `getrandom::fill(&mut [u8])` | Verified signature `pub fn fill(dest: &mut [u8]) -> Result<(), Error>` in the 0.4.3 source; OS CSPRNG, no `rand` |
| `serde` / `serde_json` | 1.x | `wire` DTOs, config file, JSON bodies | Workspace standard |
| `ctrlc` | 3.5.2, feature `termination` | SIGINT/SIGTERM -> graceful stop (PID 1 in containers ignores SIGTERM otherwise) | Compiles on Windows (verified in spike) and unix; NEW to `Cargo.lock` |
| `chrono` | 0.4 (`clock`, no default features) | UTC timestamp in log lines | Already in workspace |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `ureq` | 3.x (lock: 3.4.2) | **dev-dependency only** in `server`: protocol test client over loopback | Tests; use `Agent::config_builder().http_status_as_error(false)` so 409/416 bodies are readable |
| `libc` | 0.2.190 (already in lock) | OPTIONAL `cfg(unix)` only: `SO_KEEPALIVE` on the listener | Only if the TCP keepalive hardening in Pitfall 2 is adopted; otherwise skip |
| `tempfile` | 3.27.0 (already in lock, transitive) | dev-dependency: per-test data dirs | Tests; or a 10-line helper using `std::env::temp_dir()` + pid + counter |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| tiny_http | axum 0.8 + tokio (confined to `crates/server`) | Only if spike fails (it did not) or a later trigger fires: stalled-client DoS becomes a real concern, more than one runner, push channel needed. Document as the exit in Key Decisions |
| JSON config file | `toml` crate | `toml` is tiny and friendlier for hand-edited config, but STACK.md says "Do not add"; keep `serde_json` and a documented example file. Planner may revisit (low cost either way) |
| `subtle` constant-time compare | lookup by `sha256(token)` in an indexed column | The attacker controls only the preimage; the compared value is a hash of it, so there is nothing to time-attack. No `subtle` needed. (PITFALLS P8 says "constant time"; STACK says unnecessary; hash-lookup satisfies both) |
| Incremental SHA state across chunks | re-hash the whole `.part` at `finish` | Incremental state cannot survive a server restart without serialising the hasher; re-hash costs seconds per GB and is restart-proof |

**Installation (workspace edits only; nothing to install on the dev box):**
```toml
# Cargo.toml (root): add to members
#   "crates/wire",
#   "crates/server",

# crates/wire/Cargo.toml
[package]
name = "wire"
version = "0.1.0"
edition = "2024"
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# crates/server/Cargo.toml
[package]
name = "server"
version = "0.1.0"
edition = "2024"
[[bin]]
name = "cs2-server"
path = "src/main.rs"
[dependencies]
wire = { path = "../wire" }
tiny_http = "0.12"
rusqlite = { version = "0.32", features = ["bundled"] }
sha2 = "0.10"
getrandom = "0.4"
ctrlc = { version = "3.5", features = ["termination"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", default-features = false, features = ["clock"] }
[dev-dependencies]
ureq = "3"
tempfile = "3"
```

**Version verification:** `cargo build` of a scratch crate with `tiny_http 0.12`, `ureq 3`, `sha2 0.10`, `getrandom 0.4`, `rusqlite 0.32.1 (bundled)`, `ctrlc 3.5 (termination)`, `serde`, `serde_json` on edition 2024 / rustc 1.99.0 succeeded; `cargo tree -i libsqlite3-sys` shows a single `libsqlite3-sys v0.30.1` <- `rusqlite v0.32.1`, so no `links = "sqlite3"` conflict. `cargo search` confirms `tiny_http = "0.12.0"` and `ctrlc = "3.5.2"` are the current published versions. [VERIFIED: local scratch build + cargo search, 2026-10-11]

## Package Legitimacy Audit

Run via `gsd-tools query package-legitimacy check --ecosystem crates ...` (all `OK`, no postinstall, repos present):

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| tiny_http | crates.io | since 2015-05 | ~1.2M/wk | github.com/tiny-http/tiny-http | OK | Approved (locked by D-01; stagnant but see Pitfalls) |
| ctrlc | crates.io | since 2015-07 | ~1.8M/wk | github.com/Detegr/rust-ctrlc | OK | Approved |
| getrandom | crates.io | since 2019-01 | ~49.8M/wk | github.com/rust-random/getrandom | OK | Approved (already in lock) |
| sha2 | crates.io | since 2016-05 | ~23.0M/wk | github.com/RustCrypto/hashes | OK | Approved (already in workspace) |
| libc | crates.io | since 2015-01 | ~31.4M/wk | github.com/rust-lang/libc | OK | Approved (already in lock; optional) |
| rusqlite | crates.io | since 2014-11 | ~3.3M/wk | github.com/rusqlite/rusqlite | OK | Approved (workspace) |
| serde / serde_json / ureq | crates.io | — | — | — | OK (first tool output page covered tiny_http..rusqlite; serde/serde_json/ureq are existing workspace deps) | Approved |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none. `ctrlc` is the only package NEW to `Cargo.lock` (on unix it pulls `nix`); it came from the v1.1 STACK research and passed the gate, so no `checkpoint:human-verify` is required.

## Architecture Patterns

### System Architecture Diagram

```
 operator (shell on server)                         runner (Windows, phase 6) / tests
        │                                                         │  HTTPS via Tailscale Serve/Caddy
        ▼                                                         ▼  (plain HTTP on loopback to us)
 ┌─ cs2-server (bin) ───────────────────────────────────────────────────────────────────────────┐
 │  cli.rs: serve | submit | runner add/list/revoke | job list/show/cancel | config show        │
 │        │ (direct calls, no admin HTTP)                                                       │
 │        │                       ┌───────────────── http layer ─────────────────────────────┐   │
 │        │                       │ tiny_http::Server (own accept thread + unbounded conn    │   │
 │        │                       │   threads that only parse headers)                       │   │
 │        │                       │        │ recv_timeout(500ms) loop                        │   │
 │        │                       │        ▼                                                 │   │
 │        │                       │ dispatcher thread: busy < N ? ──yes──► channel ──► N     │   │
 │        │                       │        │ no                              worker threads  │   │
 │        │                       │        └─► 503 + Retry-After (side thread)    │          │   │
 │        │                       │            router(method, path) ◄────────────┘          │   │
 │        │                       │            auth(Bearer → sha256 → runners row)           │   │
 │        │                       └─────────────┬────────────────────────────────────────────┘   │
 │        ▼                                     ▼                                                │
 │  core.rs (pure): decide(job, Event, now_ms) → Decision | Reject   ◄── Clock (System / Manual) │
 │        │  apply in ONE `BEGIN IMMEDIATE` tx: claim · hello(reattach) · progress · release ·   │
 │        │  finish · cancel · sweep(expire leases) · startup_grace                              │
 │        ▼                                                                                      │
 │  db.rs: Arc<Mutex<Connection>> (WAL, busy_timeout, foreign_keys) + MIGRATIONS/user_version    │
 │        ▲  (never held across file IO or hashing)                                              │
 │  blobs.rs: <data>/blobs/incoming/<job_id>/demo.dem(.part)                                     │
 │            <data>/blobs/results/<job_id>/<epoch>.part → highlights.mp4                        │
 │            offset = metadata(.part).len(); per-(job,epoch) upload guard; sha256 at finish     │
 │  log.rs + secret.rs: every line / every runner-supplied text → redact() → sink                │
 └───────────────────────────────────────────────────────────────────────────────────────────────┘
 timer thread: sweep() every 5 s · ctrlc handler: shutdown flag (dispatcher + long-poll loop exit)
```

### Recommended Project Structure
```
crates/
├── wire/                 # serde-only DTOs; no workspace deps (Linux+Windows)
│   └── src/lib.rs        # PROTOCOL_VERSION, CHUNK_SIZE, JobState, CancelMode, Assignment, Progress*, Finish*, Failure, ApiError, header/path consts
└── server/
    ├── Cargo.toml        # [[bin]] cs2-server
    ├── src/
    │   ├── lib.rs        # //! docs; re-exports Ctx, serve(), run_cli()
    │   ├── main.rs       # thin: run_cli(std::env::args())
    │   ├── config.rs     # Config (file+env+flags), Secret fields, validation
    │   ├── secret.rs     # Secret newtype, redact(), Redactor
    │   ├── log.rs        # LogSink trait, StderrSink, MemorySink (tests), logln
    │   ├── clock.rs      # Clock trait, SystemClock, ManualClock
    │   ├── db.rs         # Db handle, open(), MIGRATIONS, with()/write() helpers
    │   ├── core.rs       # JobRow, Event, decide(), Core{db,clock,cfg} transactional API
    │   ├── auth.rs       # issue_token(), hash_token(), authenticate(), revoke
    │   ├── blobs.rs      # layout, ingest(), upload guard, append_chunk(), finalize(), range reader
    │   ├── http.rs       # dispatcher + worker pool + shutdown
    │   ├── api.rs        # router + handlers (hello, claim, progress, demo, result, finish, release)
    │   └── cli.rs        # arg parsing: serve | submit | runner | job | config
    └── tests/
        ├── core_states.rs      # virtual-clock transition table + randomized sequence invariants
        ├── protocol_http.rs    # real loopback HTTP: claim race, chunks, resume, mismatch, 503
        ├── secrets.rs          # SEC-04 sentinel test
        ├── deps.rs             # asserts crates/server/Cargo.toml has no path deps except ../wire
        └── common/mod.rs       # test server harness + ureq protocol client
```

### Pattern 1: Pure decision function + transactional wrapper (D-05)
**What:** `fn decide(job: &JobRow, ev: &Event, now_ms: i64, cfg: &Limits) -> Result<Decision, Reject>` returns the new column values / effects (e.g. `DiscardPart`, `NotifyWake`); the wrapper opens `BEGIN IMMEDIATE`, loads the row, calls `decide`, writes, commits. `claim` is the one exception that selects *and* updates in SQL (see Pattern 2) but still uses the same `Limits`.
**When to use:** every transition. Table-driven unit tests call `decide` directly with a `ManualClock`-derived `now_ms`; no threads, no HTTP.
**Clock:** `trait Clock: Send + Sync { fn now_ms(&self) -> i64 }` (unix epoch ms, persisted values are absolute). `ManualClock(AtomicI64)` with `advance(Duration)`. Long-poll waiting is real time (Condvar) in the HTTP layer; the core `claim` is non-blocking, so virtual time never has to coordinate with sleeping.

### Pattern 2: Atomic claim with epoch (SRV-04) — verified SQL shape
Executed in the spike: a single `UPDATE ... WHERE id=(SELECT ... LIMIT 1) RETURNING` hands out each queued job once; second claim on an empty queue returns `QueryReturnedNoRows`; four threads on four separate connections to the same WAL file, each using `transaction_with_behavior(Immediate)`, claimed ids `[1,2,3,4,5,6]` exactly once each. The server uses one connection under a mutex (so the race is already serialised in-process), but `submit` is a second *process* writing the same file, so `Immediate` + `busy_timeout` stays mandatory.

Claim algorithm (inside one IMMEDIATE tx, runner already authenticated):
1. `expire_leases(now)` (lazy sweep, so tests need no sweeper thread).
2. **Idempotent re-delivery:** if this runner already holds a job in state `assigned` (granted but never progressed) → return the same Assignment (same epoch, `lease_until` refreshed). Covers the "claim response lost in transit" case (PITFALLS P3.3) without a `claim_request_id`.
3. If the runner holds a job in `running`/`uploading` -> 409 `already_holding` (runner violated "claim only when idle").
4. Pick oldest `queued` job (`ORDER BY id`); skip & fail any whose `grants >= max_grants` (`TOO_MANY_GRANTS`); skip jobs with `lease_lost=1` whose `runner_id` is a *different* runner only if you want strictness (single runner: do not special-case).
5. `UPDATE jobs SET state='assigned', epoch=epoch+1, grants=grants+1, runner_id=?, lease_until=?now+ttl, lease_lost=0 ... RETURNING ...`.

### Pattern 3: Reversible lease loss (the "own job back" requirement)
- Expiry: `assigned|running|uploading` and `lease_until < now` -> `queued`, `lease_lost=1`, **epoch unchanged**, `grants` unchanged. If `cancel_requested` != none: -> `cancelled` instead (user intent wins). Never auto-fail because the runner is silent.
- Re-attach: `hello{holding:[{job_id,epoch}]}` or `progress{epoch,...}` from the runner whose id equals `runner_id` on a job with `state='queued' AND lease_lost=1 AND epoch=<presented>` -> back to `assigned`/`running`/`uploading` (chosen from the reported phase), `lease_lost=0`, new `lease_until`, **no new epoch, no new grant**. If another claim already bumped the epoch, the presented epoch mismatches -> `409 stale_epoch` (runner treats it as abandon).
- Startup grace: at process start, `UPDATE jobs SET lease_until = MAX(lease_until, now + startup_grace) WHERE state IN ('assigned','running','uploading')`. Leases are absolute timestamps so a server restart does not requeue everything while the runner is mid-recording (PITFALLS P3.4).
- Revoke: `runner revoke` sets `revoked_at` and `lease_until = now` for that runner's leased jobs (next sweep requeues; the revoked token cannot re-attach).

### Pattern 4: Transition table (SRV-03)
States, verbatim from REQUIREMENTS.md SRV-03: `receiving → queued → assigned → running → uploading → done / failed / cancelled`.

| From | Event | To | Notes |
|------|-------|----|-------|
| receiving | ingest_done | queued | file hashed, `files` row inserted, same tx |
| receiving | ingest_failed / startup recovery of orphaned receiving | failed (`RECEIVE_FAILED`) | at process start any `receiving` row with no live ingest is failed and its `.part` removed |
| receiving, queued | cancel | cancelled | delete blobs (effect, after commit) |
| queued | claim | assigned | epoch+1, grants+1, lease set |
| queued (`lease_lost=1`) | hello/progress with current epoch from owning runner | assigned/running/uploading | re-attach, no epoch bump |
| queued | claim when `grants>=max` | failed (`TOO_MANY_GRANTS`) | SRV-07 |
| assigned | first progress (phase beyond fetch) | running | |
| assigned, running | first result chunk / progress phase=upload | uploading | |
| assigned, running, uploading | lease expiry | queued (`lease_lost=1`) or cancelled if cancel requested | |
| assigned, running, uploading | release (runner gives job back) | queued | counts as a grant already spent; optional endpoint |
| assigned, running, uploading | cancel | same state, `cancel_requested=soft|force` | reported in next progress reply |
| assigned, running, uploading | finish Cancelled | cancelled | |
| assigned, running, uploading | finish Failed | failed | stores code/class/state/hint/message/tail/elapsed (redacted, length-capped) |
| uploading | finish Done (size+sha verified) | done | rename `.part` -> `highlights.mp4`, insert `files` row |
| done, failed, cancelled | anything | — | sticky; late `finish`/upload gets 409 and late bytes are deleted. Repeat of the *same* finish (same runner, epoch, outcome) returns 200 (idempotent) |

Every mutating runner call carries `epoch` (JSON field or `X-Epoch` header on byte endpoints) and is rejected with `409 {"error":"stale_epoch"}` unless `job.runner_id == caller && job.epoch == epoch`.

### Pattern 5: Resumable chunk protocol (SRV-06)
```
HEAD /v1/jobs/{id}/result        X-Epoch: e          → 200, Upload-Offset: <len of <epoch>.part or 0>
PUT  /v1/jobs/{id}/result        X-Epoch: e, Upload-Offset: N, Content-Length: L (1 ≤ L ≤ 8 MiB)
                                 → 204 + Upload-Offset: N+L
                                 → 409 + Upload-Offset: <actual> + JSON {"error":"offset_mismatch","offset":actual}   (body NOT read)
                                 → 411 (no Content-Length / chunked), 413 (L > CHUNK_SIZE or N+L > max_result_bytes)
POST /v1/jobs/{id}/finish        {epoch, outcome: Done{size, sha256} | Failed{...} | Cancelled}
                                 → 200 ; size ≠ part length → 409 offset_mismatch ; sha mismatch → 422 sha256_mismatch + .part deleted
HEAD /v1/jobs/{id}/demo          X-Epoch: e          → 200, Content-Length: size, ETag: "<sha256>", Accept-Ranges: bytes
GET  /v1/jobs/{id}/demo          Range: bytes=N-[M]  → 206, Content-Range: bytes N-M'/size, Content-Length (≤ 8 MiB per response), ETag
                                 → 416 + Content-Range: bytes */size when N ≥ size
```
Rules:
- The **`.part` length on disk is the offset** (no DB column) so server crashes lose nothing. Path is `results/<job_id>/<epoch>.part`; a new epoch uses a new file, so a stale uploader can never corrupt the new attempt's data, and the stale one is already fenced by 409.
- **Partial chunks are kept.** If the connection dies mid-chunk, bytes already read are in the file; the next `HEAD` reports the true offset and the runner sends the remainder. (Spike: server saw `read=2097152` of `body_length=4194304` with no error.)
- **One writer per (job, epoch):** an in-process `Mutex<HashSet<(i64,i64)>>` guard; a second concurrent PUT gets `409 {"error":"upload_busy"}` + `Retry-After: 1`. Covers a retry overlapping an old stalled request.
- Offset arithmetic with `checked_add`; reject `N > actual` and `N < actual` alike (409), never seek-and-overwrite.
- `finish(Done)`: tx1 (verify epoch/state, read part path) -> **outside the DB mutex** `metadata.len()` check, streaming sha256 (1 MiB buffer), `sync_all`, `rename` -> tx2 compare-and-set (`WHERE id=? AND epoch=? AND state='uploading'`) + insert `files` row. If tx2's CAS fails (cancelled meanwhile) delete the file.
- Extend the lease on every received chunk (upload progress is liveness; PITFALLS P3/P4).
- Download chunking: respond to any `Range` with at most `CHUNK_SIZE` bytes and a truthful `Content-Range`; the (phase 6) runner loops. Use `with_chunked_threshold(usize::MAX)` so a real `Content-Length` is sent (see Pitfall 5).

### Pattern 6: Runner-facing JSON protocol (phase 5 surface)
| Endpoint | Request -> Response |
|----------|--------------------|
| `POST /v1/runner/hello` | `{protocol, name, app_version, status, holding:[{job_id,epoch}]}` -> `{server_version, server_time_ms, poll_secs, heartbeat_secs, chunk_size, directives:[{job_id, action: keep|abandon|cancel}]}`; protocol mismatch -> `426` with Russian message |
| `POST /v1/runner/claim?wait=25` | `{status}` -> `200 Assignment{job_id, epoch, lease_until_ms, demo:{size,sha256,orig_name}\|null, faceit_link\|null, player:{steam64\|null,name}, requested_by, options:{}}` or `204` after the long-poll deadline |
| `POST /v1/jobs/{id}/progress` | `{epoch, seq, phase, take?, log[], status}` -> `{lease_until_ms, cancel: none\|soft\|force}`; doubles as heartbeat; stale `seq` is ignored but still extends the lease |
| `POST /v1/jobs/{id}/release` (optional now, cheap) | `{epoch, reason}` -> job back to `queued` |
| byte endpoints / finish | as Pattern 5 |

`#[serde(default)]` on every optional field; enums get a `#[serde(other)] Unknown` arm; `PROTOCOL_VERSION: u32 = 1`. Errors always `{"error":"<ascii_code>","message":"<русский текст>"}` with `Content-Type: application/json`.

### Pattern 7: Bounded worker pool with 503 (D-02) — verified in spike
`tiny_http` queues parsed requests in an unbounded internal queue and spawns its own connection threads, so **overflow cannot be detected by workers calling `recv()` themselves**. Use one dispatcher thread that `recv_timeout(500 ms)`s (also polls the shutdown flag), keeps an `AtomicUsize busy`, and either sends the request to a worker channel or answers `503 + Retry-After: 2`. Do the 503 `respond` on a short-lived side thread, not on the dispatcher: dropping a request drains its unread body (Pitfall 3) and must never block the dispatcher. Default 16 workers (configurable). Each long-poll `claim` occupies a worker for up to 25 s, which is fine for one runner.

### Pattern 8: Config (SRV-01)
Precedence: built-in defaults < JSON file < env < CLI flags. File: `--config PATH` or `CS2_SERVER_CONFIG`; deny unknown keys (`#[serde(deny_unknown_fields)]`). Suggested keys/env: `bind` (`127.0.0.1:8080`), `data_dir` (default `$STATE_DIRECTORY` if systemd sets it, else `./data`), `threads` (16), `lease_ttl_secs` (45), `heartbeat_secs` (10), `longpoll_secs` (25), `max_grants` (3), `startup_grace_secs` (90), `max_demo_bytes`, `max_result_bytes` (2000 MiB, Telegram local limit), `telegram_bot_token` (optional `Secret`, unused until phase 7 but registered with the redactor now). Secrets may come from env (`TELEGRAM_BOT_TOKEN`) to keep them out of the file. On unix warn at startup when the config file mode has group/other bits (`mode & 0o077 != 0`). Warn (do not refuse) when `bind` is not loopback: "HTTP без TLS".
Config validation errors must never echo secret values (print the key name only).

### Anti-Patterns to Avoid
- **Reading the body of a rejected request is not needed, but *dropping* it is not free** (Pitfall 3): reject cheaply and early, but design for the drain.
- **Holding the `Mutex<Connection>` during file IO / hashing / `thread::sleep`:** serialises the whole server behind a 2 GB hash. Lock only around SQL.
- **`lock().unwrap()` on the connection mutex:** a handler panic poisons it and every later request fails. Use `lock().unwrap_or_else(|e| e.into_inner())` (a dropped rusqlite `Transaction` rolls back) and wrap each handler in `catch_unwind`.
- **Deriving `Debug` on structs holding tokens**, logging `{:?}` of a config, or logging `rq.url()` with its query string.
- **Using the runner-supplied file name or any user text in a path:** paths are built from integer ids only; `orig_name` is display data (sanitised, capped).
- **Reusing job ids:** use `INTEGER PRIMARY KEY AUTOINCREMENT` so a deleted job's id (and its blob directory) is never reused.
- **Server depending on `store`/`pipeline`/`host`/`demo`/`cs2`:** enforced by `tests/deps.rs`.
- **Treating lease expiry as failure** (Anti-Pattern 4 in ARCHITECTURE.md) and **comparing runner clocks** (only the server clock decides).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SHA-256 | own hash | `sha2::Sha256` streaming (`update` per 1 MiB buffer) | correctness, hardware acceleration |
| Random tokens | `SystemTime`-seeded RNG | `getrandom::fill` on 32 bytes | OS CSPRNG |
| Password hashing for tokens | argon2/bcrypt | plain `sha256(token)` | tokens are 256-bit random, not passwords |
| HTTP parsing | custom socket server | `tiny_http` | request smuggling history (RUSTSEC-2020-0031, patched >= 0.8.0; we use 0.12) |
| Atomic dequeue | `SELECT` then `UPDATE` | single `UPDATE ... RETURNING` inside `BEGIN IMMEDIATE` | read-then-write races across the `submit` process |
| Schema migration | a framework | the existing `MIGRATIONS: &[&str]` + `user_version` loop | already proven in `crates/store/src/lib.rs`; copy the pattern, do not depend on `store` |
| Signal handling | raw `libc::signal` | `ctrlc` with `termination` | portable, async-signal-safe handling done for you |
| Multipart / tus | tus crate, multipart | 40-line HEAD-offset + PUT protocol | exactly one client; tus adds nothing here |

**Key insight:** the only things worth writing by hand are the three small, product-specific pieces: the decision table (`decide`), the offset-checked append, and `redact()`. Everything that is generic (hashing, randomness, HTTP framing, SQL atomicity) already has a boring correct implementation in the stack.

## Common Pitfalls

### Pitfall 1: An aborted PUT looks like a clean EOF
**What goes wrong:** `rq.as_reader().read()` returns `Ok(0)` when the client closes mid-body; there is no error. A handler that treats EOF as "done" will acknowledge a truncated chunk and the offset arithmetic goes wrong. **Evidence:** spike case 5 logged `PUT body_length=Some(4194304) read=2097152 err=None`. [VERIFIED: local spike]
**How to avoid:** after the copy loop compare `written == body_length`; if short, keep the bytes already appended (offset = file length), skip the success response (the peer is gone; `respond` errors for closed peers are ignored by tiny_http), log at info level. On Linux a hard reset may surface as `Err(ConnectionReset)` instead; tests must accept either outcome as "aborted".
**Warning signs:** a test that aborts mid-chunk and then sees `Upload-Offset` equal to the full chunk size.

### Pitfall 2: No socket timeouts: a stalled client pins a worker forever
**What goes wrong:** tiny_http never sets read/write timeouts on accepted sockets, and `RefinedTcpStream`/`Connection` expose no handle. Spike case 8: headers + 1000 of 1,000,000 body bytes, then silence -> worker stayed busy for the whole observation and freed only when the client closed. [VERIFIED: local spike + source read: `grep timeout src/connection.rs` finds none]
**Do NOT try:** `SO_RCVTIMEO` on the listening socket. A web search surfaced that on Linux a receive timeout on a *blocking listening socket* makes `accept()` return `EAGAIN`; tiny_http's accept loop then pushes the error and `break`s (source: `Err(e) => { log::error!(...); inside_messages.push(e.into()); break; }`), i.e. the server would stop accepting. Inheritance of the option by accepted sockets is also unspecified by POSIX. [CITED: man7.org accept(2) / FreeBSD bug 277118 / Austin Group issue 1337, via web search; inheritance claim ASSUMED]
**Accepted mitigations (all three):**
1. Bounded pool + 503 so the process stays responsive and recovers when peers close. 2. Reverse proxy timeouts in front (Caddy `read_body`/`read_header`, documented in phase 9). 3. Optional `cfg(unix)` `SO_KEEPALIVE` (+ idle/interval/count) on the *listening* socket via `libc::setsockopt` to reap half-open peers; Linux copies socket flags to accepted sockets [ASSUMED] — verify in the Linux CI run or skip.
**Record as a Key Decision:** tiny_http confirmed with this documented limitation; the trigger to move to axum is "stalled/half-open clients become a problem in practice".
**Constraint to remember:** tiny_http reads the *next* request's header on the same socket while your worker is still handling the current one, so any future socket read timeout must be larger than the longest handler (long-poll 25 s, finish re-hash).

### Pitfall 3: Unread request bodies are drained in `Drop` (blocking)
**What goes wrong:** `EqualReader::drop` reads and discards the remaining body. A handler that answers `409`/`401`/`503` without consuming the body blocks (inside `respond`/drop) until the client has sent or closed. Spike case 6 (with `Expect: 100-continue`, no body sent): `respond+drop took 1.2009103s`, worker busy until the client closed. Case 7 (no Expect, client sends the body anyway): drain took 942.5 µs and the keep-alive connection stayed usable. [VERIFIED: local spike + `src/util/equal_reader.rs`]
**How to avoid:** ureq 3 does not send `Expect: 100-continue` unless asked (config has `timeout_await_100` "Only used if ... we sent the Expect header"), curl does for large bodies but falls back after 1 s. Put authentication and offset checks *before* `as_reader()` (so no `100 Continue` is ever sent for a rejected request), keep rejected-body sizes bounded by checking `Content-Length` first, and never answer from the dispatcher thread. Short-lived side thread for 503. A test must cover "reject with large Content-Length and the body actually sent" and assert the connection is reusable. The stalled-reject case is covered by Pitfall 2's mitigations.

### Pitfall 4: tiny_http buffers bodies <= 1024 bytes eagerly, streams larger ones
**What goes wrong:** `content_length <= 1024 && !expects_continue` is read fully at parse time (a stalled small body therefore blocks tiny_http's own connection thread, not a worker); bigger ones stream. Not a bug, but JSON endpoints (small) and byte endpoints (large) take different paths; test both. For JSON bodies always read through `.take(MAX_JSON + 1)` and 413 on overflow. [VERIFIED: `src/request.rs` `} else if content_length <= 1024 && !expects_continue {`]

### Pitfall 5: Responses over 32 KiB silently become chunked
**What goes wrong:** with a known length >= the threshold tiny_http uses `Transfer-Encoding: chunked` (default threshold `unwrap_or(32768)`), so the `Content-Length` header is absent and `add_header("Content-Length")` merely sets the data length. **Fix:** `Response::new(...).with_chunked_threshold(usize::MAX)` for Range responses. Spike case 3 confirmed `content-length=Some("100000")` and no `transfer-encoding`. The `Connection` response header is silently dropped by `add_header` (forbidden list: `Connection`, `Trailer`, `Transfer-Encoding`, `Upgrade`), so you cannot force-close a connection. [VERIFIED: `src/response.rs` + spike]

### Pitfall 6: ureq defaults hide error bodies and cap bodies at 10 MB
**What goes wrong (test client and phase 6 runner):** `http_status_as_error` defaults to true so a 409 becomes `Error::StatusCode` and the `Upload-Offset`/JSON is lost; `Body::read_to_vec()`/`read_to_string()`/`read_json()` have a default 10 MB limit (`const MAX_BODY_SIZE: u64 = 10 * 1024 * 1024;`), close to an 8 MiB chunk plus overhead. **Fix:** `Agent::config_builder().http_status_as_error(false)`, stream big bodies to a file with `body_mut().as_reader()` or raise the limit via `with_config().limit(...)`. [VERIFIED: ureq-3.4.2 source]

### Pitfall 7: Startup recovery and orphans
`receiving` jobs left by a crash during ingest, `.part` files with no job, `assigned/running/uploading` jobs whose runner is gone. Handle all three at startup in one `recover()` (before the listener opens): fail orphaned `receiving`, extend leases by the grace period, delete `.part` files belonging to terminal jobs.

### Pitfall 8: Timestamps and clocks
Persist unix ms integers (`i64`), never local-time strings or `Instant`; leases use the server clock only; runner-reported time is diagnostic. Virtual-clock tests drive `ManualClock`; never `sleep` in core tests.

### Pitfall 9: SQLite specifics
`foreign_keys` and `busy_timeout` are per-connection: set `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;` on every connection the server or `submit` opens. Use `Immediate` transactions for every write. Backups later via `VACUUM INTO`, not file copy (phase 9). Refuse to start when `user_version` > known schema version (downgrade).

### Pitfall 10: The SEC-04 test passing vacuously
Make the test prove that the sentinel *would* appear if redaction were off: first assert the sentinel IS present in the raw runner request/err text you fed in, then assert absent from log sink, DB bytes and responses. Include forced failures that echo input (runner `Failed.message` containing its own token; invalid JSON containing the token; `Authorization: Bearer <sentinel>` of a revoked/unknown runner; handler panic message; config validation error).

## Code Examples

Design sketches; state strings are the verbatim SRV-03 set, SQL and dispatcher shape are the parts executed in the spike. Treat names as suggestions.

### Dispatcher + bounded workers + 503 (executed)
```rust
// Source: executed spike (tiny_http 0.12.0, std only). Server is Sync; share via Arc.
let busy = Arc::new(AtomicUsize::new(0));
let (tx, rx) = std::sync::mpsc::channel::<tiny_http::Request>();
let rx = Arc::new(Mutex::new(rx));
for _ in 0..workers {
    let (rx, busy) = (rx.clone(), busy.clone());
    std::thread::spawn(move || loop {
        let rq = { rx.lock().unwrap_or_else(|e| e.into_inner()).recv() };
        let Ok(rq) = rq else { return };
        handle(rq);                              // catch_unwind inside
        busy.fetch_sub(1, Ordering::SeqCst);
    });
}
std::thread::spawn(move || loop {                // dispatcher
    if shutdown.load(Ordering::Relaxed) { return; }
    match server.recv_timeout(Duration::from_millis(500)) {
        Ok(Some(rq)) => {
            if busy.fetch_add(1, Ordering::SeqCst) >= workers {
                busy.fetch_sub(1, Ordering::SeqCst);
                std::thread::spawn(move || {     // never block the dispatcher on body drain
                    let h = Header::from_bytes(&b"Retry-After"[..], &b"2"[..]).unwrap();
                    let _ = rq.respond(Response::empty(503).with_header(h));
                });
            } else { let _ = tx.send(rq); }
        }
        Ok(None) => {}
        Err(_) => return,
    }
});
```

### Streaming chunk append with short-read detection
```rust
// Source: pattern from executed spike case 1/5; names are design.
fn append_chunk(rq: &mut tiny_http::Request, part: &Path, offset: u64, len: u64) -> io::Result<u64> {
    let mut f = OpenOptions::new().create(true).append(true).open(part)?;
    if f.metadata()?.len() != offset { return Err(io::Error::other("offset")); } // re-check under guard
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0u64;
    loop {
        let n = rq.as_reader().read(&mut buf)?;        // Ok(0) also on client abort
        if n == 0 { break; }
        f.write_all(&buf[..n])?;
        got += n as u64;
    }
    Ok(got)                                              // caller: got == len ? 204 : aborted (keep bytes)
}
```

### Range response with a real Content-Length
```rust
// Source: executed spike case 3 (206 + Content-Range + Content-Length, no chunked)
let r = Response::new(StatusCode(206),
        vec![Header::from_bytes(&b"Content-Range"[..], format!("bytes {a}-{b}/{size}").as_bytes()).unwrap()],
        file.take(len), Some(len as usize), None)
    .with_chunked_threshold(usize::MAX);
```

### Atomic claim (SQL executed; extra predicates are design)
```rust
// Source: executed spike (sqlite 3.46.0); `max_grants` filter and runner/lease columns are design.
let row = tx.query_row(
 "UPDATE jobs SET state='assigned', epoch=epoch+1, grants=grants+1,
                  runner_id=?1, lease_until=?2, lease_lost=0, updated_at=?3
  WHERE id=(SELECT id FROM jobs WHERE state='queued' AND grants < ?4 ORDER BY id LIMIT 1)
  RETURNING id, epoch", params![runner_id, now + ttl_ms, now, max_grants], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
```

### Token issue / hash
```rust
// Source: getrandom 0.4.3 `fill(&mut [u8])` verified; format `cs2r_` + hex is design.
let mut b = [0u8; 32];
getrandom::fill(&mut b).map_err(|e| e.to_string())?;
let token = format!("cs2r_{}", hex(&b));               // shown once by `runner add`
let hash = hex(&Sha256::digest(token.as_bytes()));     // only this is stored (UNIQUE column)
// auth: strip "Bearer ", hash, SELECT ... WHERE token_hash=?1 AND revoked_at IS NULL
```
A fixed `cs2r_` prefix lets the redactor find runner tokens by pattern in text it has never seen (e.g. echoed by a runner).

### Migrations (pattern copied from crates/store)
Verbatim existing pattern: `const MIGRATIONS: &[&str] = &[` ... and `tx.execute_batch(&format!("PRAGMA user_version = {}", k + 1)).map_err(err)?;`. Server version: step 1 creates `users`, `runners`, `jobs`, `files`, `job_events` + indexes (`jobs(state,id)`, `runners(token_hash)`); later changes only append steps. Each step runs in its own transaction.

### Schema sketch (names are discretion)
```
users(id PK AUTOINCREMENT, tg_id INTEGER UNIQUE NULL, name TEXT, role TEXT CHECK(role IN('admin','member')), player_steam TEXT, player_name TEXT, created_at)
runners(id PK, name TEXT UNIQUE, token_hash TEXT UNIQUE NOT NULL, created_at, revoked_at NULL, last_seen_at NULL, version TEXT, status TEXT, pause_reason TEXT)
jobs(id PK AUTOINCREMENT, user_id FK, state TEXT CHECK(state IN ('receiving','queued','assigned','running','uploading','done','failed','cancelled')),
     cancel_requested TEXT CHECK(cancel_requested IN('none','soft','force')) DEFAULT 'none',
     epoch INTEGER DEFAULT 0, grants INTEGER DEFAULT 0, runner_id FK NULL, lease_until INTEGER NULL, lease_lost INTEGER DEFAULT 0, last_seq INTEGER DEFAULT 0,
     source_kind TEXT, orig_name TEXT, faceit_link TEXT NULL, player_steam TEXT NULL, player_name TEXT,
     phase TEXT, take_n INTEGER, take_m INTEGER,
     fail_code TEXT, fail_class TEXT, fail_state TEXT, fail_hint TEXT, fail_message TEXT, fail_tail TEXT, fail_elapsed_ms INTEGER,
     created_at, updated_at, finished_at)
files(id PK, job_id FK, kind TEXT CHECK(kind IN('demo','result')), rel_path TEXT, size INTEGER, sha256 TEXT, created_at, deleted_at NULL)
job_events(job_id FK, seq, ts, kind, text)   -- optional; cap ~200 rows/job, text redacted
```
Paths in `files.rel_path` are relative to `data_dir` and built from integer ids only.

### Sentinel (SEC-04) test skeleton
```rust
// 1) start server in-process with MemorySink, config.telegram_bot_token = Secret("123456789:AAEsentinelSENTINELsentinelSENTINEL01"), issue runner token T
// 2) force failures: unknown/revoked token, malformed Authorization, bad JSON containing T, Failed{message: format!("boom {T}")},
//    stale epoch, oversized chunk, truncated PUT, handler panic (test-only route or injected fault), invalid config containing the bot token
// 3) collect: sink lines, fs::read(server.db) + fs::read(server.db-wal), every response body/headers seen by the client
// 4) assert!(!blob.contains(T)) for each; plus assert the sentinel WAS in the inputs (anti-vacuity)
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `SELECT` then `UPDATE` for dequeue | `UPDATE ... WHERE id=(subselect) RETURNING` (SQLite >= 3.35; bundled 3.46.0 here) | SQLite 3.35 (2021) | one statement, one tx, no race |
| Time-limited signed URLs / JWT for runner auth | opaque 256-bit bearer + stored hash | project decision (STACK) | no clock-skew failures (PITFALLS P22) |
| tus/multipart for resumable upload | HEAD offset + sequential PUT of <= 8 MiB | project decision | no extra deps; fits Cloudflare/proxy body limits |

**Deprecated/outdated:** `tiny_http` is stagnant (0.12.0 released 2022-10-06; request-smuggling advisory RUSTSEC-2020-0031 patched in >= 0.8.0). Acceptable because it never faces the internet; keep it behind a proxy and re-evaluate if the exposure model changes. [CITED: .planning/research/STACK.md]

## Runtime State Inventory

Not applicable (greenfield crates; no rename/refactor/migration of existing runtime state). The only on-disk/runtime artefacts the phase creates are new: `server.db` (+`-wal`/`-shm`), `blobs/`, the config file. Existing `%LOCALAPPDATA%\cs2cinema\cs2cinema.db` and `store` are untouched.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Linux copies `SO_KEEPALIVE`/keepalive tuning from the listening socket to accepted sockets (optional hardening) | Pitfall 2 | Hardening is a no-op; mitigations 1 and 2 still hold. Verify in Linux CI or drop |
| A2 | tiny_http transport behaves on Linux as measured on Windows (abort = EOF vs `ConnectionReset`, drain behaviour) | Pitfalls 1-3 | A Linux-only test failure; tests are written to accept either abort signal; first Linux run is CI |
| A3 | SHA-256 over a 2 GB `.part` completes within a few seconds on the server (hardware SHA) so `finish` fits inside a request | Pattern 5 | Long `finish` blocks one worker; if slow, move hashing to a background step and poll (`202`) |
| A4 | `max_grants = 3` default is a sensible poison ceiling | Config | Too low fails jobs after transient lease blips; configurable, tune in phase 6/9 |
| A5 | Operator-only admin via CLI on the server (no admin HTTP endpoints) is acceptable | Pattern, Security | If the author wants remote admin, phase 7 bot commands cover it (they call the same core) |
| A6 | `cs2-server submit` runs as a second process against the same DB while `serve` is running (not via HTTP) | Pattern 2/8 | If submit must run remotely, add an authenticated demo-upload endpoint reusing the chunk protocol |
| A7 | Heartbeat 10 s / TTL 45 s / long-poll 25 s are adequate on the real link | Pattern 3 | Tuned by tests and phase 6/9 drills (CONTEXT already marks them as starting values) |

## Open Questions

1. **Should `release` count toward `max_grants`?**
   - Known: RUN-04 (phase 6) returns environment-class failures to the queue; poison protection wants a hard ceiling.
   - Unclear: whether environment releases should be exempt.
   - Recommendation: implement `release` now with an explicit `counts: bool` in the wire type defaulting to `true`; phase 6 decides.
2. **Remote demo ingest (HTTP) in phase 5?**
   - Known: `submit` is local; the bot (phase 7) moves files from the Bot API volume locally.
   - Recommendation: no HTTP demo upload now; the blob layer is reusable if needed later.
3. **`toml` instead of JSON for the config file?**
   - Recommendation: JSON now (STACK says no extra deps); ship `cs2-server.example.json`; revisit in phase 9 docs.
4. **Persist `job_events` now?**
   - Recommendation: include the table and a capped writer only if progress `log[]` is part of the phase 5 wire types (it is in the sketch); otherwise defer to phase 6 with no loss, since migrations only append.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain (cargo/rustc) | build + tests | yes | cargo 1.99.0 / rustc 1.99.0 | — |
| crates.io network | fetching new crates | yes (scratch build fetched `ctrlc`, `tiny_http` deps) | — | — |
| Linux host / WSL distro | verify Linux-only behaviour (A1, A2) | no (`wsl -l` reports no distros; no docker) | — | GitHub Actions `ubuntu-latest` job in existing `ci.yml` is the first Linux execution; keep Linux-only code `cfg(unix)` and small |
| curl | manual protocol poking | yes | 8.21.0 (Schannel) | — |
| sqlite3 CLI | ad-hoc DB inspection | no | — | use a `cs2-server job show` command or rusqlite in tests |
| node | GSD tooling only | yes at `C:\Program Files\nodejs` (not on bash PATH by default) | — | — |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** Linux runtime verification -> CI ubuntu; tests must therefore be OS-independent and fast.

## Security Domain

`security_enforcement` is enabled (config: `"security_enforcement": true`, `"security_asvs_level": 1`).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | 256-bit random bearer token (`getrandom`), only `sha256` stored, revoke via `revoked_at`; no default/shared credentials |
| V3 Session Management | no | stateless bearer; no sessions or cookies |
| V4 Access Control | yes | runner may only act on jobs where `runner_id == caller && epoch == presented`; admin operations exist only as local CLI |
| V5 Input Validation | yes | hand-validated headers (`Range`, `Upload-Offset`, `Content-Length`, `X-Epoch`), JSON via serde with size cap and `deny_unknown_fields` on config, `checked_add` for offsets, ids from integers only in paths |
| V6 Cryptography | yes | `sha2` + `getrandom`; never hand-rolled |
| V7 Error Handling and Logging | yes (SEC-04) | `Secret` type, central `redact()`, no query strings or headers in logs, sentinel test |
| V12 Files and Resources | yes | size caps (`max_demo_bytes`, `max_result_bytes`, chunk <= 8 MiB), no user text in paths, atomic rename, `.part` per epoch |
| V13 API and Web Service | yes | versioned `/v1`, uniform JSON errors, 426 on protocol mismatch |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Token disclosure via logs/errors/DB | Information disclosure | `Secret`, `redact()` on every sink, hash-only storage, sentinel test (SEC-04) |
| Stale or superseded runner writes | Tampering | epoch fencing -> 409; `.part` per epoch; terminal states sticky |
| Slowloris / stalled body pinning workers | DoS | bounded pool + 503, proxy timeouts, optional keepalive (Pitfall 2) — residual risk accepted for loopback/tailnet exposure |
| Oversized body / chunk | DoS | `Content-Length` required and capped before reading; JSON `take()` cap |
| Path traversal via file names | Tampering | paths from integer ids; `orig_name` display-only, sanitised and length-capped |
| Request smuggling (tiny_http history) | Tampering | tiny_http >= 0.12 (advisory patched >= 0.8.0), never internet-facing, proxy in front |
| Token brute force | Spoofing | 256-bit entropy; log fingerprint (first 8 hex of sha256) of failed attempts, never the token |
| Public plaintext HTTP | Information disclosure | default bind loopback; warn on non-loopback bind; TLS outside (D-03) |
| Poison job crash loop | DoS | `grants` ceiling -> `failed` (`TOO_MANY_GRANTS`) |
| Config file readable by others | Information disclosure | startup warning when `mode & 0o077 != 0` (unix) |

## Sources

### Primary (HIGH confidence)
- tiny_http 0.12.0 source in `~/.cargo/registry/src` (read this session): `src/lib.rs` (accept loop, TaskPool, `recv`/`recv_timeout`/`unblock`, `Drop for Server`), `src/request.rs` (Expect handling, 1024-byte eager read, `as_reader` sends 100-Continue, `Drop` -> 500), `src/util/equal_reader.rs` (drain on drop), `src/util/task_pool.rs` (`MIN_THREADS: usize = 4`, unbounded growth), `src/response.rs` (forbidden headers, chunked threshold 32768, `with_chunked_threshold`), `src/client.rs` (417/400/408 handling, next-header read while handling)
- Executed spikes (scratch crate in the session scratchpad, rustc 1.99.0, Windows 11): transport cases 1-9 and SQLite/getrandom cases (results quoted above)
- ureq 3.4.2 source: `MAX_BODY_SIZE = 10 * 1024 * 1024`, `timeout_await_100` doc
- getrandom 0.4.3 source (`fill` signature), ctrlc 3.5.2 source (`set_handler`, `termination` feature)
- Project files: `.planning/phases/05-yadro-servera-i-protokol/05-CONTEXT.md`, `.planning/REQUIREMENTS.md`, `.planning/ROADMAP.md` Phase 5, `.planning/research/ARCHITECTURE.md`, `STACK.md`, `PITFALLS.md`, `crates/store/src/lib.rs` (migration pattern), `crates/store/Cargo.toml`, `.github/workflows/ci.yml`, root `Cargo.toml`, `Cargo.lock`
- `gsd-tools query package-legitimacy check --ecosystem crates ...` (all OK)

### Secondary (MEDIUM confidence)
- [accept(2) man page](https://man7.org/linux/man-pages/./man2/accept.2.html), [FreeBSD bug 277118](https://bugs.freebsd.org/bugzilla/show_bug.cgi?id=277118), [Austin Group issue 1337](https://austingroupbugs.net/view.php?id=1337) via web search: `SO_RCVTIMEO` on a blocking listener makes `accept` return `EAGAIN`; option inheritance is implementation-defined
- RustSec RUSTSEC-2020-0031 as cited in STACK.md

### Tertiary (LOW confidence)
- Linux keepalive inheritance (A1) and Linux abort semantics (A2): training knowledge, to be confirmed by the first ubuntu CI run

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — versions verified by build, `cargo tree`, `cargo search`, legitimacy gate
- Architecture (state machine, claim, chunk protocol): HIGH for mechanics (executed), MEDIUM for numeric defaults
- tiny_http behaviour: HIGH (source + execution on Windows); MEDIUM for Linux specifics
- Pitfalls: HIGH for 1-6 (observed), MEDIUM for 7-10 (design experience)

**Research date:** 2026-10-11
**Valid until:** 2026-11-10 for stack facts (tiny_http is frozen; rusqlite/sha2 pins are deliberate); design recommendations remain valid until the lease/epoch semantics change.

*Validation Architecture section omitted: `workflow.nyquist_validation` is `false` in `.planning/config.json`. Test strategy is described inline (Patterns 1/4, Pitfall 10, project structure `tests/`).*
