---
status: testing
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
source: [06-VERIFICATION.md]
started: 2026-10-11T17:20:22Z
updated: 2026-10-11T17:20:22Z
---

## Current Test

number: 1
name: Живой прогон настоящей демки до highlights.mp4 на сервере
expected: |
  cs2-server submit → cs2-cli runner → настоящие CS2/HLAE → ролик на сервере, размер и SHA-256 совпадают с роликом раннера
awaiting: user response

## Tests

### 1. Живой прогон настоящей демки
expected: ролик на сервере, размер и SHA-256 подтверждены
result: [pending]

### 2. Отмена с сервера посреди записи
expected: CS2 закрывается не позже 20 с после job cancel
result: [pending]

### 3. Убийство раннера посреди загрузки
expected: после перезапуска результат догружается до подтверждённых размера и SHA-256
result: [pending]

### 4. Чужой CS2
expected: задача возвращается в queued, две подряд ставят раннер на паузу
result: [pending]

### 5. Враждебные входы на реальном разборе
expected: мусорная демка даёт BAD_DEMO, неоднозначный ник PLAYER_AMBIGUOUS, всё до запуска CS2
result: [pending]

### 6. Калибровка лимитов
expected: по строке журнала «пик памяти разбора» лимиты и дедлайны проверены
result: [pending]

## Summary

total: 6
passed: 0
issues: 0
pending: 6
skipped: 0
blocked: 0

## Gaps
