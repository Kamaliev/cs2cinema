# Phase 6: Раннер, защита входа и первый живой прогон - Context

**Gathered:** 2026-10-11
**Status:** Ready for planning

<domain>
## Phase Boundary

`cs2-cli runner` на машине автора сам подключается к `cs2-server`, берёт задачу, ведёт её через неизменный `pipeline::job::run_job` и надёжно доставляет `highlights.mp4` на сервер. Существующий код безопасно обрабатывает чужие демки и ники (SEC-01…03) и объясняет отказы своими кодами до запуска CS2 (RUN-05). Фаза заканчивается первым живым прогоном через настоящий CS2. Требования: RUN-01…06, SEC-01…03.

</domain>

<decisions>
## Implementation Decisions

### Перезапуск раннера посреди задачи (RUN-02)
- **D-01:** Раннер хранит в файле состояния (запись атомарная) `job_id`, `epoch`, папку задачи и фазу. После перезапуска он делает `hello`, берёт ту же задачу по эпохе (на сервере это обратимая потеря аренды из фазы 5) и вызывает `pipeline::job::resume`: проверенные дубли не переснимаются. Если `resume` невозможен (`RETRY_NO_DATA`), задача начинается заново. — **Reversibility:** costly — формат файла состояния и связка с `hello`/эпохой станут контрактом раннера, который позже использует приложение (фаза 8).

### Сбои класса «окружение» (RUN-04)
- **D-02:** Сбой класса `Environment` (и `Launch`, как их классифицирует `pipeline::record::failure::classify`) раннер возвращает серверу через `release`, а не через `finish(failed)`. Бесконечный цикл ограничивают две вещи: раннер сам встаёт на паузу при двух одинаковых сбоях подряд (как `QueuePolicy` в приложении), а сервер считает выдачи и после `max_grants` отмечает задачу `TOO_MANY_GRANTS` (фаза 5). Новых счётчиков и правок протокола на сервере нет.

### Результат в outbox (RUN-02)
- **D-03:** Если сервер окончательно не принимает результат (задача отменена, эпоха устарела, `422 sha256_mismatch` после повторной загрузки с нуля), файл удаляется из outbox, в журнале раннера остаётся запись с причиной. Временные ошибки сети докачиваются бесконечно, пока сервер не подтвердит размер и SHA-256. — **Reversibility:** reversible — политику хранения можно сменить настройкой.

### Уже принято раньше (из research/ARCHITECTURE.md и фазы 5, не переобсуждать)
- Раннер только исходящий (pull): `claim` long-poll, `progress` как heartbeat, `finish`. Входящий порт не нужен.
- Одна задача и одна запись за раз; загрузка результата последовательная, без перекрытия со следующей записью.
- Раннер не служба Windows: CS2 нужна интерактивная сессия.
- `wire` держит свои enum и строки кодов сбоев; `server` не зависит от `pipeline`.
- `X-Epoch` обязателен на `HEAD`/`PUT` результата и `HEAD`/`GET` демки; смещение берётся только из `Upload-Offset`; после `422 sha256_mismatch` загрузка с нуля (контракт фазы 5).

### Claude's Discretion
Зоны не обсуждались, решать по исследованию, критериям успеха и коду:
- **Изоляция разбора (SEC-02):** дочерний `cs2-cli` в Job Object с лимитом памяти и времени, лимиты размера файла и распаковки zstd, различие `BAD_DEMO` и `DEMO_TOO_LARGE`. Нужен короткий спайк на Windows (см. Notes в ROADMAP).
- **Выбор игрока (SEC-03, RUN-05):** точное или единственное совпадение, регистр и пробелы, SteamID64, список до 10 очищенных кандидатов, `NO_HIGHLIGHTS`; всё до запуска CS2.
- **Очистка текста (SEC-01):** санитайзер ников и карты, `drawtext` из файла, тесты на враждебных никах.
- **Живой прогон и ffprobe (RUN-06):** допуски H.264, yuv420p, faststart; как проводить прогон и что делать с видео, не прошедшим проверку (своё code).
- Расположение каталога раннера (`%LOCALAPPDATA%\cs2cinema\runner`), интервалы heartbeat и backoff, структура крейта `runner`.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Проект и требования
- `.planning/ROADMAP.md` § Phase 6 — цель, критерии успеха, заметки про спайк изоляции
- `.planning/REQUIREMENTS.md` — RUN-01…06, SEC-01…03
- `.planning/research/ARCHITECTURE.md` — D1–D8, раннер, `Tee`, `Link`, `Loop`, состояния задачи
- `.planning/research/PITFALLS.md` — риски протокола и безопасности
- `.planning/phases/05-yadro-servera-i-protokol/05-CONTEXT.md` и `05-*-SUMMARY.md` — протокол сервера, эпоха, аренда, чанки
- `.planning/phases/05-yadro-servera-i-protokol/05-REVIEW.md` — CR-01, WR-01, WR-02 (дефекты сервера, влияют на раннер)

### Код
- `crates/pipeline/src/job.rs` — `run_job`, `resume`, `JobSpec`, `Outcome`
- `crates/pipeline/src/record/failure.rs` — `FailCode`, `FailClass`, `classify`
- `crates/app/src/queue.rs` — `QueuePolicy` (пауза при повторяющихся сбоях)
- `crates/cli/src/record.rs`, `crates/cli/src/main.rs` — точка входа CLI, `ConsoleProgress`
- `crates/wire/src/lib.rs`, `crates/server/src/api.rs` — протокол раннера

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `pipeline::job::resume`: повтор по сохранённым проверенным дублям — основа D-01.
- `QueuePolicy` в `app::queue`: логика «два одинаковых сбоя подряд = пауза» — образец для D-02.
- `host::Host` (Job Object, `Host::run` с дедлайном): основа изоляции разбора демки.

### Established Patterns
- `Progress`-трейт и `check(p)?`: отмена с сервера подмешивается в `cancelled()`/`force_cancelled()` обёрткой (`Tee`).
- Ошибки `Result<T, String>`, пользовательские строки по-русски, `std::thread` без tokio.

### Integration Points
- Новый крейт `runner` (зависит от `wire`, `pipeline`, `host`, `ureq`, `sha2`); `cs2-cli` получает команду `runner`.
- `server` не должен получить зависимость от `pipeline`.

</code_context>

<specifics>
## Specific Ideas

Живой прогон (критерий 1) заодно закрывает неформально долг v1.0 по живой проверке записи.

</specifics>

<deferred>
## Deferred Ideas

- Дефекты сервера из ревью фазы 5 (CR-01 — падение от огромного `Content-Length`, WR-01, WR-02) — отдельный gap-план фазы 5 до подключения раннера по сети; в эту фазу не входят.

</deferred>

---

*Phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon*
*Context gathered: 2026-10-11*
