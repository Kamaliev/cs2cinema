---
status: testing
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
source: [01-VERIFICATION.md]
started: 2026-10-10T12:18:58Z
updated: 2026-10-10T12:18:58Z
---

## Current Test

number: 1
name: Удачная запись от демки до highlights.mp4
expected: |
  Журнал показывает состояния по порядку (лаунчер → игра → консоль → демка готова → скрипт → дубль N из M → закрытие), в лестнице подсвечен «дубль a/b», в папке задачи есть record.log со строками [netcon], [console.log], [app], строка фактов «HLAE.exe: … · cs2.exe: …», после задачи cs2.exe не остаётся.
awaiting: user response

## Tests

### 1. Удачная запись до highlights.mp4
expected: журнал с состояниями по порядку, «дубль a/b» в лестнице, record.log с [netcon]/[console.log]/[app], cs2.exe закрыт после задачи
result: [pending]

### 2. Вызванный сбой: закрыть окно CS2 посреди дубля
expected: карточка с блоком ошибки GAME_EXITED · дубль, строка «cs2.exe: не жива (код …)», хвост консоли, record.log, кнопка «Повторить» работает
result: [pending]

### 3. Отмена во время записи
expected: статус «отменена», блока ошибки нет, cs2.exe закрыт, в record.log есть CANCELLED
result: [pending]

### 4. «Запас по времени»
expected: значение ×2.00 переживает перезапуск приложения; старый settings.json с load_wait_secs открывается без ошибки; в журнале есть «запас по времени ×2.00»
result: [pending]

### 5. Склейка ffmpeg на настоящих дублях
expected: в журнале «склейка: N%» и «склейка заняла N с»; клипы дописаны (приложение больше не ждёт video.mp4/audio.wav, см. WR-01 и WR-05 в 01-REVIEW.md)
result: [pending]

### 6. Сбой запуска в срок (например, испортить AfxHookSource2.dll)
expected: показан код и подсказка, HLAE.exe и cs2.exe названы раздельно, задача падает по дедлайну, а не висит
result: [pending]

## Summary

total: 6
passed: 0
issues: 0
pending: 6
skipped: 0
blocked: 0

## Gaps
