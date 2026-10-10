# Walking Skeleton — cs2cinema: новый путь записи

**Phase:** 1
**Generated:** 2026-10-10

Это не скелет нового приложения: cs2cinema уже есть и записывает. Скелет фазы 1 — самый тонкий сквозной срез **нового пути записи**: фейковый `Host` → машина состояний `pipeline::record` → типизированный сбой `RecordFailure` → строка задачи в SQLite (`fail_*`) → карточка задачи и `record.log`. Весь срез проверяется `cargo test --release --workspace` без CS2, на Windows и на Linux. На машине автора тот же путь идёт через `WinHost` до `highlights.mp4`.

## Capability Proven End-to-End

Задача с записью проходит машину состояний `pipeline::record` поверх трейта `Host` до записанных дублей или до именованного сбоя с кодом, состоянием, сообщением по-русски, подсказкой, прошедшим временем и хвостом консоли. Сбой виден в строке задачи и в карточке, а `record.log` лежит в папке задачи при любом исходе. Доказывает тест `typed_failure_reaches_the_job_row` (01-07), он проходит на фейковом хосте без CS2.

## Architectural Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Граница с ОС | Новый листовой крейт `crates/host`: трейты `Host` + `Session`, `FakeHost` (виртуальные часы), `UnsupportedHost` (не-Windows), `WinHost` (`cfg(windows)`); `windows-sys` 0.61 только здесь (D-01) | Только этот крейт трогает Win32, сокеты и дочерние процессы; `pipeline` и тесты работают без ОС |
| Владение процессами | Windows Job Object с `KILL_ON_JOB_CLOSE`, PID игры из job, живость ожиданием на handle; запасной путь: принять `cs2.exe` по PID со строгой проверкой (D-02) | Завершаются только свои процессы; ошибка проверки означает «неизвестно», а не «жива» (TMO-02) |
| Консоль | netcon (основной, `-netconport`, команды только сюда) и хвост `console.log` (`-condebug`, запасной); один канал `std::sync::mpsc` за `Session::poll` (D-04) | Маркеры и `AFXERROR` приходят и без порта; без новых крейтов шины |
| Ожидания | Чистая машина `Machine::step(now, Input) -> Vec<Action>` + драйвер с шагом ≤100 мс; время — только `Host::now()`; дедлайны — таблица `Deadlines` × множитель «запас по времени» (D-09) | Одна точка решений о времени; тест на 150-секундный дедлайн идёт микросекунды |
| Маркеры | `CS2C <nonce> READY / SCRIPT / TAKE_START n / TAKE_END n / ALL_DONE`, nonce — 8 hex на план; парсер принимает только целую строку (D-11) | Эхо команд, трассировка `mirv_cmd` и старый `console.log` не путаются с метками |
| Модель сбоя | `RecordFailure { code, state, message, hint, elapsed, console_tail, launcher, game }`, коды `SCREAMING_SNAKE` (D-05, D-08); на границе `app::jobs` превращается в строки | `store` не зависит от `pipeline`; карточка и будущий автоповтор читают код без разбора текста |
| Хранение | SQLite (`rusqlite`), миграции через `PRAGMA user_version`; колонки `fail_*` (+ `fail_elapsed_ms`, `phase` — по решению в 01-03) | База пользователя переживает обновление; изменения только добавляют колонки |
| Транспорт в UI | `Progress::event(&Event)` с телом по умолчанию; `jobs.phase` — текущее состояние для лестницы; `jobs.stage` остаётся этапом конвейера | UI не разбирает строки журнала; полоска прогресса этапов не ломается |
| Тесты | `FakeHost` + `Scenario` (виртуальные часы, удаление и задержка маркеров), интеграционные тесты `crates/pipeline/tests/record.rs`; Windows-тесты `Job`/`WinHost` на стенд-процессах; CI — ubuntu + windows | Критерий успеха 5 фазы: машина записи проверяется без CS2 |
| Расположение | `crates/host/src/{lib,fake,unsupported,runner}.rs`, `crates/host/src/win/{mod,job,process,watch,netcon,console_log}.rs`, `crates/pipeline/src/record/{mod,machine,markers,deadlines,failure,log}.rs` | Политика в `pipeline`, адаптер ОС в `host` |

## Stack Touched in Phase 1

- [ ] Каркас: новый член workspace `crates/host` со своими тестами; стенд спайков `crates/host/examples/spikes.rs` (01-01)
- [ ] Точка входа нового пути: `pipeline::record(&dyn Host, &RecordRequest, &dyn Progress)` (01-04), вызывается из `app::jobs::run` (01-07)
- [ ] База данных: запись `Store::fail_job` / `set_phase` / `set_out_dir` и чтение `Job.fail_*` (01-03, 01-07)
- [ ] UI: блок ошибки и лестница состояний в карточке задачи, настройка «Запас по времени» (01-07)
- [ ] Локальный запуск всего пути: `cargo test --release --workspace` (без CS2) и `cargo run --release -p app` с настоящей задачей FACEIT или `.dem` (на машине автора)

## Out of Scope (Deferred to Later Slices)

- Предпроверка окружения, отказ при чужом `cs2.exe`, проверка Steam и места на диске (фаза 2, PRE-*)
- Поэтапное завершение игры, отмена за 3 с / 10 с в любом состоянии, уборка cfg и копии демки (фаза 2, TMO-03/04/06/07)
- Модальное окно HLAE и `AFXERROR` как мгновенный сбой (фаза 2, LCH-05); в фазе 1 `AFXERROR` лишь остаётся в хвосте и меняет код сбоя по дедлайну на `HLAE_ERROR`
- `ffprobe` дублей, `clips/manifest.json`, `highlights.mp4` через временное имя (фаза 3, TAK-02..06)
- Кнопка «Открыть папку», повтор с этапа записи, статус «прервано» после падения (фаза 3, RET-*, OBS-02)
- Перевод `cs2-cli` на общий код, удаление `crates/app/src/launch.rs` целиком (фаза 4, TST-03)

## Subsequent Slice Plan

- Phase 2: запись не стартует в неисправном окружении, отменяется за секунды, завершает только свои `cs2.exe`/`HLAE.exe` (новые состояния машины: предпроверка, отмена с поэтапной уборкой)
- Phase 3: дубли проверяются `ffprobe` и манифестом, «Повторить» стартует с записи, после падения приложения задача «прервано» (новое состояние «проверено»)
- Phase 4: приложение и `cs2-cli` пишут одним кодом из `pipeline`, очередь из 5 матчей FACEIT проходит без присмотра дважды
