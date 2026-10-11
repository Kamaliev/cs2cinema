---
status: testing
phase: 05-yadro-servera-i-protokol
source: [05-VERIFICATION.md]
started: 2026-10-11T11:51:02Z
updated: 2026-10-11T11:51:02Z
---

## Current Test

number: 1
name: Запуск CI на ubuntu-latest для коммитов фазы 05
expected: |
  job `test (ubuntu-latest)`: `cargo test --release --workspace` и `cargo build --release -p server` зелёные, приложен артефакт `cs2-server-linux-x64`
awaiting: user response

## Tests

### 1. Запуск CI на ubuntu-latest для коммитов фазы 05
expected: `cargo test --release --workspace` и `cargo build --release -p server` зелёные, артефакт `cs2-server-linux-x64` приложен (SRV-01; ветка `#[cfg(unix)]` проверки прав файла настроек, config.rs:200)
result: [pending]

## Summary

total: 1
passed: 0
issues: 0
pending: 1
skipped: 0
blocked: 0

## Gaps
