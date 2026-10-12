---
status: complete
phase: 07-telegram-bot-i-dostavka
source: [07-VERIFICATION.md]
started: 2026-10-12T00:00:00Z
updated: 2026-10-12T00:00:00Z
---

## Current Test

[testing complete]

## Tests

### 1. Живая цепочка: демка друга через бота до ролика в чате
expected: подтверждение с местом в очереди сразу; статус не чаще раза в 10 с; ролик новым сообщением, воспроизводится на телефоне и в Telegram Desktop; job show: доставка sent
result: skipped
reason: "Deferred follow-up: нужен стенд с telegram-bot-api --local, CS2/HLAE и автором за машиной (решение автора, 07-11 пропущен; пройти в фазе 9 вместе с учениями)"

### 2. Спайк: sendVideo по file:// на 0,5/1,0/1,5 ГБ против sendDocument; width/height/duration
expected: видео с превью и верным соотношением сторон без метаданных, иначе bot_send_as=document или метаданные в протоколе
result: skipped
reason: "Deferred follow-up: умолчание bot_send_as=video выбрано без замера (решение автора, 07-11 пропущен; пройти в фазе 9 вместе с учениями)"

### 3. Спайк: права и пути общего тома bot-api, getFile с абсолютным путём, удаление исходника, logOut
expected: сервер читает и удаляет файл в рабочей папке bot-api; рабочая папка не растёт
result: skipped
reason: "Deferred follow-up: права настоящего bot-api (другой пользователь, WSL/Docker) не проверялись (решение автора, 07-11 пропущен; пройти в фазе 9 вместе с учениями)"

### 4. Учения вживую: незнакомец, FACEIT от друга, /cancel, /retry, /status, /queue, сбой задачи
expected: ожидаемые тексты по-русски без локальных путей; улики сохранены
result: skipped
reason: "Deferred follow-up: в тестах проверено только на FakeTelegram и FakeHost (решение автора, 07-11 пропущен; пройти в фазе 9 вместе с учениями)"

### 5. Поиск токенов бота, раннера и ключа FACEIT в настоящих журналах, job show и чате
expected: ни одного совпадения
result: skipped
reason: "Deferred follow-up: настоящие журналы стенда не просматривались (решение автора, 07-11 пропущен; пройти в фазе 9 вместе с учениями)"

## Summary

total: 5
passed: 0
issues: 0
pending: 0
skipped: 5
blocked: 0

## Gaps

## Deferred Follow-Ups

- test: 1
  idea: "Живая цепочка бот → сервер → раннер → ролик в чате на реальном стенде"
  deferred_at: 2026-10-12
- test: 2
  idea: "Спайк sendVideo против sendDocument на 0,5–1,5 ГБ, нужны ли width/height/duration"
  deferred_at: 2026-10-12
- test: 3
  idea: "Спайк общего тома telegram-bot-api: права, пути, удаление исходника"
  deferred_at: 2026-10-12
- test: 4
  idea: "Живые учения бота (незнакомец, FACEIT, /cancel, /retry, отчёт о сбое)"
  deferred_at: 2026-10-12
- test: 5
  idea: "Поиск подставного токена по настоящим журналам и сообщениям после живого прогона"
  deferred_at: 2026-10-12
