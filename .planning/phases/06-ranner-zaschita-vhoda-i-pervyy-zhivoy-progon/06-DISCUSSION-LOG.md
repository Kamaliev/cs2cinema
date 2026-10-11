# Phase 6: Раннер, защита входа и первый живой прогон - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-10-11
**Phase:** 6-Раннер, защита входа и первый живой прогон
**Areas discussed:** Outbox, перезапуск и сбои среды

---

## Перезапуск раннера посреди записи
| Option | Selected |
|---|---|
| Продолжить с проверенных дублей (через `pipeline::job::resume`) | ✓ |
| Всегда с нуля | |
| Вернуть серверу и забыть | |

## Возврат при сбое среды
| Option | Selected |
|---|---|
| Пауза раннера + лимит выдач сервера | ✓ |
| Счётчик возвратов в задаче | |
| Возвращать и ждать оператора | |

## Результат в outbox при окончательном отказе
| Option | Selected |
|---|---|
| Удалить, оставить запись в журнале | ✓ |
| Хранить N дней | |
| Не удалять никогда | |

## Claude's Discretion
Не выбраны для обсуждения: изоляция разбора демки, выбор игрока, живой прогон и проверка видео.
