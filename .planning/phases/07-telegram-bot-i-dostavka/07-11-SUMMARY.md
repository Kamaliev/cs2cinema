---
phase: 07-telegram-bot-i-dostavka
plan: 11
subsystem: live-stand
tags: [live-stand, spike, skipped, human-verification]
status: skipped-by-author
requirements-completed: []
---

# План 07-11: спайк `telegram-bot-api --local` и живой стенд — ПРОПУЩЕН по решению автора

Автор («давай следующую фазу») решил не проводить спайк и живой стенд и перейти к следующей фазе.

## Что НЕ сделано (ничего из этого не выполнялось и не проверялось)

- Спайк локального `telegram-bot-api --local`: общий том и права (читает ли и может ли удалить сервер файлы bot-api), абсолютный путь `getFile`, жив ли файл после переноса, `sendVideo` по `file://` на 0,5 / 1,0 / 1,5 ГБ, `sendVideo` против `sendDocument` (от этого зависит значение `bot_send_as` по умолчанию, пока оно `video` без замеров), нужны ли `width/height/duration` (допущение A3), `logOut` тестового бота из облака.
- Живая цепочка (критерий 5): демка друга → бот на локальном `telegram-bot-api` → `cs2-server serve` → `cs2-cli runner` с настоящими CS2 и HLAE → ролик в чате, воспроизведение в клиентах Telegram.
- Учения D1…D8 вживую: незнакомец, ссылка FACEIT от друга, `/cancel`, `/retry`, `/status`, `/queue`, отчёт о сбое.
- Проверка улик на токены бота (формы `:` и `~`), токен раннера и ключ FACEIT в настоящих журналах и чате.
- Файлы `07-SPIKE.md`, `07-LIVE-STAND.md`, `evidence/`, `spike/make-videos.sh` не созданы.

## Чем это покрыто вместо живого стенда

Тесты на `FakeTelegram`, поддельном HTTP Bot API (`HttpTg`), `FakeHost` и настоящем `server::serve` (планы 07-01…07-10); `cargo test --workspace --release` зелёный (см. 07-VERIFICATION.md). Живое поведение не подтверждено — пункты выше остаются долгом ручной проверки и входят в `human_verification` отчёта 07-VERIFICATION.md. Условие остаётся в силе: порт `cs2-server` слушает только loopback/Tailscale, пока в фазе 9 не закрыты таймауты медленных клиентов (WR-03).
