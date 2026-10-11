# Phase 5: Ядро сервера и протокол - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-10-11
**Phase:** 5-Ядро сервера и протокол
**Areas discussed:** HTTP-слой и доступ к БД

---

## HTTP-слой и доступ к БД

| Вопрос | Выбрано |
|--------|---------|
| HTTP-стек | tiny_http (альтернативы: axum+tokio сразу; tiny_http со спайком) |
| Доступ к SQLite | Arc<Mutex<Connection>> (альтернатива: поток-актор) |
| Тестируемость | Ядро без HTTP + трейт Clock (альтернатива: только через HTTP) |
| Параллелизм long-poll | Пул потоков с лимитом, 503 при переполнении |
| TLS | Снаружи (Tailscale Serve/Caddy), сервер по HTTP |

## Claude's Discretion

Аренды/выдачи, протокол докачки, хранилище, токены, конфиг, submit.

## Deferred Ideas

Не обсуждённые области (аренды, файлы, токены/submit) — оставлены исследованию и планированию.
