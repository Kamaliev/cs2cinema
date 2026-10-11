---
status: complete
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
source: [06-VERIFICATION.md]
started: 2026-10-11T17:20:22Z
updated: 2026-10-11T18:05:27.057Z
---

## Current Test

[testing complete]

## Tests

<!-- Результаты 1-6 — со слов автора ("pass"); улик evidence/ нет, 06-11 живьём не выполнялся. -->

### 1. Живой прогон настоящей демки
expected: ролик на сервере, размер и SHA-256 подтверждены
result: pass

### 2. Отмена с сервера посреди записи
expected: CS2 закрывается не позже 20 с после job cancel
result: pass

### 3. Убийство раннера посреди загрузки
expected: после перезапуска результат догружается до подтверждённых размера и SHA-256
result: pass

### 4. Чужой CS2
expected: задача возвращается в queued, две подряд ставят раннер на паузу
result: pass

### 5. Враждебные входы на реальном разборе
expected: мусорная демка даёт BAD_DEMO, неоднозначный ник PLAYER_AMBIGUOUS, всё до запуска CS2
result: pass

### 6. Калибровка лимитов
expected: по строке журнала «пик памяти разбора» лимиты и дедлайны проверены
result: pass

## Summary

total: 6
passed: 6
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps
