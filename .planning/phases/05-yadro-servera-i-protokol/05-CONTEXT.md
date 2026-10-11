# Phase 5: Ядро сервера и протокол - Context

**Gathered:** 2026-10-11
**Status:** Ready for planning

<domain>
## Phase Boundary

Отдельная программа `cs2-server` на Linux со своей SQLite: пользователи, задачи, раннеры, файлы; автомат состояний задачи с арендами и эпохой; токены раннеров (хеш); передача файлов кусками по 8 МиБ с докачкой и SHA-256; `cs2-server submit`; токены не попадают в журналы (SEC-04). Проверяется без Telegram и без CS2. Требования: SRV-01…SRV-08, SEC-04. Бот, раннер и защита входа — фазы 6–7.

</domain>

<decisions>
## Implementation Decisions

### HTTP-слой
- **D-01:** HTTP-сервер — `tiny_http` 0.12, синхронно на `std::thread`, без tokio. Спайк в начале фазы проверяет `PUT` по 8 МиБ, `Expect: 100-continue` и обрывы. Если спайк провален — запасной выход `axum` 0.8 + `tokio` только внутри `crates/server`, решение записать в Key Decisions. — **Reversibility:** costly — смена HTTP-стека трогает все обработчики и тестовый клиент протокола.
- **D-02:** Параллелизм: фиксированный пул потоков (стартово ~16) принимает запросы; long-poll `claim` (до 25 с) занимает поток из пула; при переполнении сервер отвечает 503. Число потоков — настройка.
- **D-03:** TLS снаружи (Tailscale Serve или Caddy). `cs2-server` слушает HTTP на настраиваемом bind-адресе (по умолчанию loopback). Детали развёртывания — фаза 9, здесь только настройка адреса.

### Доступ к БД
- **D-04:** `Arc<Mutex<rusqlite::Connection>>` (как в `crates/store`), `rusqlite` строго 0.32.1 (иначе конфликт `links = "sqlite3"`). Атомарная выдача задачи — одна транзакция под мьютексом. Миграции вручную через `user_version`.

### Тестируемость
- **D-05:** Автомат состояний и аренды — ядро без HTTP: чистые функции над хранилищем с внедрённым трейтом `Clock` (виртуальные часы в тестах). HTTP — тонкая обёртка. Тесты протокола (чанки, докачка, обрывы) идут через реальный HTTP на localhost.

### Claude's Discretion
- Структура модулей `crates/server`, имена таблиц и колонок, точный формат ошибок API.
- Числа аренды (heartbeat 10 с, TTL 45 с, long-poll 25 с) и лимит выдач — стартовые из роадмапа, подстраиваются тестами.
- Протокол докачки чанков, хранилище файлов, формат токенов, конфиг и `submit` — не обсуждались; решать по исследованию (`research/ARCHITECTURE.md`, `PITFALLS.md`) и критериям успеха роадмапа.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Планирование
- `.planning/ROADMAP.md` §Phase 5 — цель, критерии успеха, заметки (спайк tiny_http, числа аренды)
- `.planning/REQUIREMENTS.md` — SRV-01…SRV-08, SEC-04
- `.planning/PROJECT.md` — ограничения и Key Decisions v1.1

### Исследование v1.1
- `.planning/research/SUMMARY.md` — итоги и три критичных дефекта существующего кода
- `.planning/research/STACK.md` — версии крейтов (tiny_http, frankenstein, sha2, getrandom, ctrlc)
- `.planning/research/ARCHITECTURE.md` — крейты `wire`, `server`, `runner`, протокол аренд/эпох
- `.planning/research/PITFALLS.md` — ловушки (токены в логах, ядовитые демки, аренды)
- `.planning/research/FEATURES.md` — ожидаемое поведение очереди

### Код-образцы
- `crates/store/src/lib.rs` — шаблон `Arc<Mutex<Connection>>`, миграции, `Result<T, String>`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/store`: паттерн Store и миграций через `user_version`; не зависеть от него, повторить подход.
- `crates/host/src/fake.rs`: образец виртуального времени (`FakeHost`) для стиля `Clock`.

### Established Patterns
- `std::thread`, ошибки `Result<T, String>`, русские пользовательские строки, имена тестов-предложений.
- Сервер не зависит от `pipeline`, `store`, `host`, `demo` (критерий успеха 1); коды сбоев `FailCode` передаются строкой.

### Integration Points
- Новые крейты `wire` (serde-типы протокола) и `server` (бинарник `cs2-server`) в воркспейсе; `runner` — фаза 6.

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

Не обсуждались в этой сессии (оставлены на усмотрение исследования/планирования): модель аренд и повторных выдач, протокол докачки и хранилище файлов, формат токенов/конфига/`submit`.

</deferred>

---

*Phase: 5-Ядро сервера и протокол*
*Context gathered: 2026-10-11*
