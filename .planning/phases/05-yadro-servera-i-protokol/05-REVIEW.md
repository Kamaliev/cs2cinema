---
phase: 05-yadro-servera-i-protokol
reviewed: 2026-10-11T12:00:00Z
depth: standard
files_reviewed: 28
files_reviewed_list:
  - crates/wire/Cargo.toml
  - crates/wire/src/lib.rs
  - crates/server/Cargo.toml
  - crates/server/cs2-server.example.json
  - crates/server/src/api.rs
  - crates/server/src/auth.rs
  - crates/server/src/blobs.rs
  - crates/server/src/cli.rs
  - crates/server/src/clock.rs
  - crates/server/src/config.rs
  - crates/server/src/core.rs
  - crates/server/src/db.rs
  - crates/server/src/http.rs
  - crates/server/src/lib.rs
  - crates/server/src/log.rs
  - crates/server/src/main.rs
  - crates/server/src/rules.rs
  - crates/server/src/secret.rs
  - crates/server/src/users.rs
  - crates/server/tests/common/mod.rs
  - crates/server/tests/core_states.rs
  - crates/server/tests/deps.rs
  - crates/server/tests/lease_http.rs
  - crates/server/tests/operator.rs
  - crates/server/tests/protocol_http.rs
  - crates/server/tests/secrets.rs
  - crates/server/tests/transfer_http.rs
  - .github/workflows/ci.yml
  - Cargo.toml
findings:
  critical: 1
  warning: 6
  info: 5
  total: 12
status: issues_found
---

# Phase 05: Code Review Report

**Reviewed:** 2026-10-11
**Depth:** standard
**Files Reviewed:** 28 (plus `Cargo.toml`)
**Status:** issues_found

## Summary

The core is well built. Every state change goes through a `BEGIN IMMEDIATE` transaction under one connection mutex. The transition rules live in a pure `decide` function. Fencing by `(runner_id, epoch)` is applied consistently. File paths are built only from `i64` ids, and every epoch used in a path is first checked against the job row, so I found no path traversal. All SQL is parametrized. Tokens are stored as SHA-256 hashes and the redactor sits on every output. `cargo test -p server -p wire` passes (all suites green).

The tests miss one hole, and I confirmed it against the real binary. A single unauthenticated request with a huge `Content-Length` kills the whole `cs2-server` process (CR-01). The other findings are state-machine edge cases, DoS hardening, and config validation.

## Critical Issues

### CR-01: One unauthenticated request with a huge `Content-Length` aborts the server process

**File:** `crates/server/src/http.rs:263-270`, `crates/server/src/api.rs:87-99`, `crates/server/src/api.rs:163`, `crates/server/src/api.rs:259-274`, `crates/server/src/http.rs:316-333`

**Issue:** `tiny_http` 0.12 drains an unread request body in `Drop`. `EqualReader::drop` in `src/util/equal_reader.rs:66-86` runs `let mut buf = vec![0; remaining_to_read];` once per loop iteration, where `remaining_to_read` is the declared `Content-Length`. This server rejects before reading the body, by design: 401 for a missing token, 404 for an unknown route, 413 from `read_json` and `put_result`, and 503 `overloaded`. In each of those cases the drain allocates a buffer as large as the client-declared length. An allocation failure is `handle_alloc_error`, which aborts the process. `catch_unwind` in `http.rs:266` cannot catch it.

I reproduced this against the built binary (`target/debug/cs2-server.exe serve`). I sent `POST /v1/runner/claim HTTP/1.1` with no `Authorization` header and `Content-Length: 1000000000000000`. The client got the 401, then the process died with `memory allocation of 1000000000000000 bytes failed`. A standalone `tiny_http` 0.12 reproduction behaves the same way.

- **Who can trigger it:** anyone who can reach the port, on any path and with no token. Authenticated runners can also trigger it through the 413 path.
- **Effect:** the whole queue goes down, and `recover()` plus the lease grace window only help after a manual restart.
- **Why the tests miss it:** the 413 tests send `CHUNK_SIZE + 1`, so the allocation is only 8 MiB.

**Fix:** put a hard cap on `body_length()` before any handler or `overloaded` can drop the request, and make sure the drain never allocates the declared length. Options:

1. Preferred: vendor or patch `tiny_http` through `[patch.crates-io]`. Change `EqualReader::drop` to a fixed buffer, e.g. `let mut buf = [0u8; 8192]`, and a bounded drain (for example, give up after 1 MiB and close the connection).
2. Interim, in the dispatcher at `http.rs:287`, before `tx.send`:

```rust
const MAX_BODY: usize = 8 * 1024 * 1024 + 4096; // CHUNK_SIZE + запас
Ok(Some(rq)) => {
    if rq.body_length().is_some_and(|n| n > MAX_BODY) {
        busy.fetch_sub(1, Ordering::SeqCst); // если уже увеличен
        // Ответить 413 и НЕ отдавать rq в Drop с огромным остатком:
        let _ = rq.respond(too_large_response()); // + заголовок Connection: close
        // без патча tiny_http остаток всё равно дочитывается в Drop,
        // поэтому до патча единственный безопасный вариант: std::mem::forget(rq)
        // (утечка сокета; ограничить числом таких запросов)
        continue;
    }
    ...
}
```

3. Add a regression test that sends `Content-Length: 1000000000000000` to `/v1/runner/claim` with no token and asserts that the server still answers afterwards.

## Warnings

### WR-01: `release` ignores a requested cancel, so a cancelled job is re-queued and re-issued

**File:** `crates/server/src/rules.rs:190-214` (compare `Event::Expire`, `rules.rs:387-395`)

**Issue:** `Event::Expire` turns a job with `cancel_requested != none` into `Cancelled`. `Event::Release` does not. If the operator cancels a leased job and the runner then calls `release` (for example "environment not ready"), the job goes to `Queued` with `cancel_requested = soft|force` still set. `claim` does not look at that flag, so the job is issued again with a new epoch and a new grant. The cancel is only noticed when the next runner sends its first progress and gets `cancel` back. A grant is wasted, and a job that is about to hit `max_grants` can end in `TOO_MANY_GRANTS` instead of "cancelled". `release_returns_the_job_and_can_refund_the_grant` and the cancel tests do not cover this combination.

**Fix:** in the `Release` branch, treat it like `Expire`:

```rust
if job.cancel_requested != CancelMode::NotRequested {
    j.state = JobState::Cancelled;
    j.finished_at = Some(now_ms);
    j.runner_id = None;
    j.lease_until = None;
    j.lease_lost = false;
    return Ok(Decision { job: j, reattached: false, discard_parts: true });
}
```

Alternatively, `claim_report` could skip jobs with a cancel flag and cancel them.

### WR-02: Poison-job sweep fails queued jobs whose runner can still reattach

**File:** `crates/server/src/core.rs:393-420`

**Issue:** The query `state = 'queued' AND grants >= ?1` also matches jobs with `lease_lost = 1`. That is a job whose lease merely lapsed (a network blip longer than the TTL) and whose runner may reattach with the same `(runner_id, epoch)`. The design in `rules.rs:7-9` says lease expiry is not a failure. But when `grants == max_grants` (the third issue with the default of 3), any `claim` from another runner, or the same one, marks the job `failed` with `TOO_MANY_GRANTS` and the text "ни разу она не дошла до конца — похоже, демка роняет раннер". The live runner then gets `job_terminal` after possibly uploading most of the result.

**Fix:** exclude recoverable jobs from the poison query: `... WHERE state = 'queued' AND grants >= ?1 AND lease_lost = 0`. Jobs with a lost lease then stay claimable by someone else, which bumps the epoch, or reattachable by their owner. Add a test for `grants == max` with a lost lease.

### WR-03: DoS hardening gaps in the HTTP layer (unbounded 503 threads, no I/O timeouts, unbounded drain)

**File:** `crates/server/src/http.rs:288-292`, `crates/server/src/http.rs:263-270`

**Issue:** When all workers are busy, `std::thread::spawn(move || overloaded(rq))` creates one new OS thread per extra request, with no cap. `overloaded` ends in `Drop` of the request, which blocks while it drains the client's body, and the client controls the length and the pace. A few hundred slow connections therefore produce hundreds of blocked threads and FD exhaustion. The same drain blocks a worker for an unauthenticated 401 or 404 with a large `Content-Length` and a stalled client. With 16 workers, 16 such connections stop the service. The module doc admits this and delegates it to a front proxy. The default bind is loopback, but the warning for non-loopback `bind` only says "no TLS" and does not say that a timeout-enforcing proxy is required.

**Fix:**
- Cap concurrent 503 responders, e.g. an `AtomicUsize` limit of 32. Past the limit, close the request without a body.
- Combine with the CR-01 fix so the drain is bounded in size and time.
- Mention the required proxy timeouts in the non-loopback warning in `config.rs:264`.

### WR-04: `authenticate` opens a write transaction for every request, including bad tokens, and logs each rejection

**File:** `crates/server/src/auth.rs:500-532`, `crates/server/src/api.rs:513-522`

**Issue:** The lookup is wrapped in `db.write`, which is `BEGIN IMMEDIATE` under the global connection mutex. It also runs for tokens that do not exist. Any client that sends well-formed `cs2r_<64 hex>` strings serializes against all legitimate traffic on the single connection. In addition, every unknown or revoked token produces a WARN log line, which makes it a log-flood vector. `UPDATE runners SET last_seen_at` also runs on every request, including each 8 MiB chunk's polling.

**Fix:** do a read-only `SELECT` first. Open a write transaction only for a valid, non-revoked runner and only when `last_seen_at` is older than, say, 5 seconds. Rate-limit or aggregate the "token unknown" log lines.

### WR-05: Config limits have no upper bounds, so `i64` overflow can silently break all leases

**File:** `crates/server/src/config.rs:59-63`, `crates/server/src/config.rs:278-304`

**Issue:** `lease_ttl_secs as i64 * 1000` and `startup_grace_secs as i64 * 1000` are unchecked. `validate` bounds only the lower side of `lease_ttl_secs`, `heartbeat_secs` and `startup_grace_secs`. A value above about 9.2e15 wraps negative or overflows. That panics in debug builds and wraps in release, giving an instantly expired lease. `now + lease_ttl_ms` in `rules.rs` and `claim_report` can overflow the same way. `max_demo_mib` and `max_result_mib` are bounded by 4096, so those two are fine. This needs operator action, but the validation was clearly meant to catch operator mistakes.

**Fix:** add upper bounds in `validate`, e.g. `lease_ttl_secs <= 86_400`, `startup_grace_secs <= 86_400`, `heartbeat_secs <= lease_ttl_secs / 2`. Alternatively use `checked_mul` in `Config::limits()` and return an error.

### WR-06: A failed `finish_receive` transaction leaves an orphan demo blob on disk

**File:** `crates/server/src/core.rs:263-285`

**Issue:** `ingest_demo` copies up to `max_demo_bytes` (2 GB by default) to `blobs/incoming/{id}/demo`. The transaction that follows can fail:
- `decide(Received)` returns an error if the job stopped being `receiving` while the copy ran, e.g. `cs2-server job cancel` from another process.
- The `files` insert or the commit fails.

In both cases the function returns `Err` and the file stays. Nothing ever removes `blobs/incoming/{id}`: `recover` only touches results, and `discard_parts` only touches `results/`.

**Fix:** on the `Err` path of that transaction, remove the demo file and its directory (`let _ = std::fs::remove_dir_all(self.blobs.demo_path(job_id).parent()...)`). Also add a startup sweep for `incoming/{id}` that has no `files` row.

## Info

### IN-01: `seq` is cast `u64 -> i64` without a guard

**File:** `crates/server/src/rules.rs:164-166`

**Issue:** `*seq as i64 > job.last_seq` wraps for `seq >= 2^63`. The value becomes negative and the message is treated as stale. A buggy runner that sends `seq = i64::MAX` freezes `phase` and `take` updates for that epoch. Only the runner's own job is affected.

**Fix:** reject `seq > i64::MAX as u64` with `bad_request`, or use `i64::try_from(*seq)`.

### IN-02: A progress with an unknown phase regresses a reattached job to `assigned`

**File:** `crates/server/src/rules.rs:137-143`, `crates/server/src/rules.rs:304-308`

**Issue:** `Phase::Unknown` from a newer runner is stored as the string `"unknown"`. `Phase::parse("unknown")` returns `None`, so a later reattach of a lost lease maps that job to `Assigned` instead of `Running`. This is harmless in practice, since the next progress corrects it, but the two functions disagree.

**Fix:** do not store `phase` when it is `Unknown`, or let `state_for_phase` and the reattach branch share one helper.

### IN-03: Part files from older epochs are never removed while the job is alive

**File:** `crates/server/src/blobs.rs:134-136`, `crates/server/src/core.rs:423-432`

**Issue:** Each epoch gets its own `{epoch}.part`. A new claim bumps the epoch, but the previous epoch's `.part` stays until the job is terminal. At worst that is `max_grants` times `max_result_mib` of dead data per job.

**Fix:** in `claim_report`, after issuing a new epoch, delete `*.part` files other than `{epoch}.part`. Do the file work outside the DB transaction.

### IN-04: `SystemClock` is wall time, so a clock step shifts every lease

**File:** `crates/server/src/clock.rs:16-22`

**Issue:** The `Clock` trait is wall-time milliseconds. A backward NTP step extends every lease, and a forward step expires all of them at once, with mass requeue and grant consumption. A single-host server with NTP is low risk, but the design stores absolute times.

**Fix:** document the assumption. Optionally, have the sweeper log a warning when the clock goes backwards or jumps by more than a minute.

### IN-05: Dead public API and a missing test class

**File:** `crates/server/src/api.rs:608` (`respond_json`), `crates/server/src/core.rs:347`, `crates/server/src/core.rs:635`, `crates/server/src/core.rs:935`

**Issue:** `api::respond_json` has no callers anywhere. `Core::wait_for_work`, and the thin wrappers `Core::claim` and `Core::finish`, are used only in tests. The test suite has no case for oversized or adversarial `Content-Length` values (see CR-01), and none for `cancel_requested` combined with `release` or `lease_lost` combined with `grants == max` (see WR-01 and WR-02).

**Fix:** remove or mark `respond_json` as test-only, and add the regression tests named in CR-01, WR-01 and WR-02.

---

_Reviewed: 2026-10-11_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
