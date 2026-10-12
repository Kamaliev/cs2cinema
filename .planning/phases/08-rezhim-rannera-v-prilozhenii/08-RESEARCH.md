# Phase 8: Режим раннера в приложении - Research

**Researched:** 2026-10-12
**Domain:** встраивание крейта `runner` (протокол раннера, outbox, эпоха) в очередь egui-приложения `crates/app` (Rust, `std::thread`, SQLite, без tokio)
**Confidence:** HIGH по коду (всё прочитано в этой сессии), MEDIUM по поведению на живом стенде (живой стенд отложен в фазу 9)

<user_constraints>
## User Constraints (CONTEXT.md для фазы 8 нет)

`08-CONTEXT.md` не создавался (`/gsd-discuss-phase` не запускался). Ограничениями считаются цель, критерии успеха и требования фазы из `ROADMAP.md` и `REQUIREMENTS.md`, плюс решения фазы 6, на которые фаза 8 прямо опирается.

### Locked Decisions (из ROADMAP.md, Phase 8, дословно)
- Goal: «Приложение на машине автора работает раннером: удалённая задача с сервера идёт через существующую очередь как обычная карточка, не мешает локальным задачам и сама продолжается после перезапуска, а без настроенного сервера приложение работает как раньше.»
- Критерий 1: «В настройках приложения автор вводит адрес сервера и токен и видит состояние подключения. Удалённая задача появляется в очереди обычной карточкой с журналом, этапами, отменой и «Повторить», а её результат уходит на сервер.»
- Критерий 2: «Удалённая задача берётся только тогда, когда локальная очередь пуста, ничего не записывается и очередь не на паузе. Локальная задача, поставленная во время удалённой, ждёт своей очереди, и в любой момент идёт не больше одной записи.»
- Критерий 3: «Если закрыть или убить приложение посреди удалённой задачи и запустить снова, прерванная задача возобновляется сама, без действий автора, и её результат доходит до сервера.»
- Критерий 4: «Без адреса сервера приложение ведёт локальные задачи как в v1.0 (очередь, авто-повтор, пауза, «Повторить»); `crates/cli/tests/pipeline.rs` и существующие тесты очереди `app::queue` остаются зелёными.»
- Notes: «наибольший риск регрессий этапа: перед хуком `on_settled` в `queue.rs` прочитать существующие тесты очереди. Удалённая задача — строка `store` с `external_ref` (миграция 6), прогресс уходит на сервер обёрткой над `Progress`.»
- Из фазы 6 (06-CONTEXT D-01, D-02, D-03): состояние раннера в `runner_state.json` (атомарная запись) с `job_id`, `epoch`, папкой и этапом; после перезапуска `hello` с `holding` и `pipeline::job::resume`; сбой класса Environment/Launch раннер возвращает через `release`; приложение реализует `Executor` поверх своей очереди (`crates/runner/src/exec.rs`, шапка модуля).

### Claude's Discretion
Всё остальное: форма хука в очереди, формат `external_ref`, где хранить токен, как показывать состояние, судьба «Повторить» у закрытой на сервере задачи (см. Open Questions).

### Deferred Ideas (OUT OF SCOPE)
- Уборка файлов раннера, развёртывание, учения (фаза 9: OPS-01…04).
- Живой стенд (бот → сервер → приложение) отложен в фазу 9 (07-UAT, коммит 01e2ffd).
- Дефекты и расширения бота (BOT-F1 «раннер офлайн» и т.п.).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| APP-01 | В настройках задаются адрес сервера и токен; удалённая задача — обычная карточка очереди | Раздел «Карточка» (миграция 6, `external_ref`, `RemoteExecutor`), раздел «Настройки, токен, состояние» |
| APP-02 | Удалённая задача берётся только когда локальная очередь свободна и не на паузе | Раздел «Допуск (gate)»: ворота в `runner` до `claim` и после него, один поток очереди |
| APP-03 | После перезапуска прерванная удалённая задача возобновляется сама | Раздел «Возобновление»: `runner_state.json`, `hello`/`Keep`, усыновление карточки по `external_ref` |
| APP-04 | Приложение без сервера работает как раньше, `cli/tests/pipeline.rs` зелёный | Раздел «Без сервера» и «Тесты»: ноль потоков и ноль сетевых вызовов при пустом адресе, `queue::start` сохраняет сигнатуру |
</phase_requirements>

## Project Constraints (from CLAUDE.md)

- Стек не менять: Rust workspace, egui/eframe, `std::thread` без tokio, SQLite через rusqlite; встраиваться в существующие крейты.
- Только Windows для записи; одна задача записи за раз.
- Внешние процессы (HLAE, CS2): любой детектор с таймаутом и диагностикой.
- `cs2-cli` должен собираться и проходить `crates/cli/tests/pipeline.rs`.
- Пользовательские строки, doc-комментарии по-русски; идентификаторы по-английски.
- Ошибки `Result<T, String>`, отмена через `pipeline::CANCELLED`, прогресс только через `Progress`; `check(p)?` на границах этапов.
- Публичный API через корень крейта, приватное по умолчанию; миграции схемы store только добавлением в конец `MIGRATIONS`.
- Правки репозитория — только через GSD-команды (этот файл — артефакт планирования).

## Summary

Крейт `runner` уже содержит всё тяжёлое: исходящий цикл `claim → демка → исполнитель → результат → finish`, outbox `runner_state.json`, `hello` с `holding`, пульс прогресса, `release`, отмену сервера, паузу после двух одинаковых неисправностей. Шов один: трейт `Executor::run(&Task, &dyn Progress)` — в шапке `exec.rs` прямо сказано «Приложение фазы 8 реализует `Executor` поверх своей очереди». Поэтому фаза 8 — это не второй раннер, а три приращения: (1) `RemoteExecutor` в приложении превращает `Task` в карточку очереди и ждёт её итога, (2) в `runner` добавляются ворота допуска перед `claim` и наблюдатель состояния для окна, (3) миграция 6 в `store` и небольшой хук в `queue.rs`.

Запись остаётся строго в потоке очереди: поток раннера только создаёт (или усыновляет) карточку в `store`, пересылает в `Tee` события, которые поток очереди присылает по каналу, и отдаёт итог раннеру. Благодаря этому «не больше одной записи» обеспечивается тем, что очередь и так одна, а не новым замком. Безопасность чужих демок сохраняется, потому что карточку исполняет тот же `PipelineExecutor` (дочерняя предпроверка, `run_job` под `catch_unwind`, `verify_result`), только вызванный из потока очереди; для этого рядом с `cs2cinema.exe` должен лежать `cs2-cli.exe` (в релизном zip он есть).

**Primary recommendation:** `RemoteExecutor` (поток раннера) ↔ `RemoteHub` (канал на карточку) ↔ ветка в `jobs::runner` (поток очереди) с вызовом настоящего `PipelineExecutor`; ворота допуска и наблюдатель добавить в `runner`; `queue::start` не менять (хук подключается отдельной функцией), токен хранить отдельным файлом, а не в `settings.json`.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Протокол, outbox, эпоха, пульс, release | `runner` (крейт) | `wire` | Уже сделано и покрыто тестами фазы 6; приложение не дублирует |
| Допуск «брать ли задачу» | `runner` (ворота-замыкание) | `app` (решение) | Решение читает вид очереди и базу — это знание приложения; проверка перед `claim` — место раннера |
| Карточка удалённой задачи | `store` (строка с `external_ref`) | `app::remote` | Очередь, журнал, UI уже живут на строках `jobs` |
| Исполнение (запись) | `app::queue` (единственный поток) | `runner::PipelineExecutor` | Одна запись за раз обеспечивается одним потоком очереди |
| Прогресс, отмена сервера | `runner::Tee` | канал `RemoteHub` | `Tee` ждёт `&dyn Progress` в потоке раннера; из потока очереди события идут каналом |
| Адрес, токен, состояние | `app` (настройки, страница) | `runner::Token` | Токен не в `Settings`, значение не печатается |
| Классификация сбоя (release/finish) | `runner::policy` | `app::queue::QueuePolicy` | Для удалённой карточки решает раннер; очередь отвечает за авто-повтор Launch и паузу |

## Standard Stack

Новых внешних пакетов нет. Все зависимости — путевые крейты воркспейса, которые уже собираются.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `runner` (path) | 0.1.0 | Раннер: `Runner`, `Config`, `Executor`, `Task`, `PipelineExecutor`, `Token`, `Control`, `RunnerState` | Единственный раннер проекта; `app` получает зависимость `runner = { path = "../runner" }` [VERIFIED: crates/runner/Cargo.toml, crates/app/Cargo.toml] |
| `store` (path) | — | Миграция 6, `external_ref` | Единственная база приложения [VERIFIED: crates/store/src/lib.rs:266-285] |
| `wire` (path) | — | Строки протокола (`Phase`, `Assignment`) | Приходит транзитивно через `runner`; в `app` напрямую нужен только `wire::faceit::room_match_id` для метки карточки |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `server` (path, dev-dependency `app`) | — | Живой `server::serve` в e2e-тесте приложения | Только в `[dev-dependencies]` `app`; охрана `crates/server/tests/deps.rs` проверяет манифест сервера, а не приложения [VERIFIED: crates/server/tests/deps.rs:1-30] |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Исполнять карточку в потоке очереди, поток раннера ждёт | Исполнять `run_job` прямо в потоке раннера и держать общий замок записи с очередью | Замок обходит `QueuePolicy`, пересоздаёт «Повторить»/авто-повтор/паузу и нарушает критерий «обычная карточка»; не брать |
| Приложение запускает `cs2-cli runner` дочерним процессом | — | Карточек в очереди приложения не будет; не соответствует цели |
| Токен в `settings.json` | отдельный файл токена | `Settings` выводится как есть и сравнивается `draft != s`; отдельный файл проще не утечь |

**Installation:** внешних пакетов нет; `crates/app/Cargo.toml`: `runner = { path = "../runner" }`, в `[dev-dependencies]` — `server = { path = "../server" }`.

**Version verification:** внешних пакетов не добавляется, `npm view`/`cargo search` не требовались. `rustc 1.99.0` [VERIFIED: rustc --version]; `std::fs::File::try_lock` уже используется в `crates/cli/src/runner_cmd.rs` (`lock_dir`).

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| — | — | — | — | — | — | Новых внешних пакетов нет; `runner`, `wire`, `server` — путевые крейты этого воркспейса |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```text
 [cs2-server]  <--HTTP (исходящие)-->  поток раннера (app::remote)
   Telegram->задача                     Runner::tick():
                                          hello(holding) -> Keep/Cancel/Abandon
                                          gate() ? --закрыто--> hello-пинг "занят" (раз в 30 с), НЕ claim
                                          claim -> Assignment -> gate() ещё раз
                                              закрыто -> release(counts=false), в outbox не пишем
                                          outbox runner_state.json (stage Claimed->Fetched->Executing->Finished)
                                          скачать демку -> RemoteExecutor::run(Task, Tee)
                                                |                                  ^
                          (1) hub.open(task):  |                                  | (4) Settled(result)
                              создать/усыновить|карточку (store, external_ref)     |
                              (2) цикл: recv_timeout(200мс) ---- события ---->  Tee.stage/log/event/artifact
                                  tee.cancelled()? -> token.cancel()/force()  (отмена сервера)
                                                |                                  |
                                                v                                  |
                         store.jobs (queued, external_ref="srv:{id}:{epoch}")      |
                                                |                                  |
                                   поток очереди (queue::start) -- один на приложение
                                     next_queued -> start_job -> run_one
                                       runner-замыкание: external_ref есть и в hub есть запись?
                                         да  -> PipelineExecutor::run(Task, ТееПрогресс{JobReporter + канал})
                                         нет -> обычный jobs::job_spec / run_job (локальная или ручной «Повторить»)
                                       QueuePolicy.after(...) -> Next / AutoRetry / Pause
                                       on_settled(job, finished, result) --- только при Verdict != AutoRetry ---> hub.settle
                                                |
   Runner: disposition (Done->загрузка кусками / Fail->finish / Release->release / Cancel->finish(Cancelled))
```

### Recommended Project Structure
```
crates/runner/src/
├── lib.rs          # + Config.admission, Admission, Tick::Gated, Runner::run_observed, RunnerEvent
crates/store/src/lib.rs   # + миграция 6 (external_ref), add_remote_job/adopt, job_by_external_ref, Reset.interrupted_remote
crates/app/src/
├── remote.rs       # новый: RemoteHub, RemoteExecutor, RemoteStatus, запуск/остановка потока раннера, файл токена
├── queue.rs        # + необязательный хук on_settled (start_with_hook); start() не меняется
├── jobs.rs         # runner(): ветка для карточек с external_ref
├── settings.rs     # + runner_server, runner_name (serde default); токен — не здесь
└── settings_page.rs / jobs_page.rs / app.rs   # раздел «Удалённые задачи», бейдж, кнопки карточки
```

### Pattern 1: карточка = строка `jobs` с `external_ref`
**What:** удалённая задача живёт в той же таблице. `external_ref = "srv:{job_id}:{epoch}"` (пара «задача, выдача» уникальна; повторная выдача того же job_id после `release` даёт новую эпоху и новую карточку). Уникальный частичный индекс `WHERE external_ref IS NOT NULL`.
**When to use:** создание в `RemoteExecutor::run`; `Executor::run` вызывается и при первой выдаче, и после перезапуска (`Task.resume = true`), поэтому всегда `adopt_or_create(ref)`.
**Правило усыновления:** найдена карточка с этим `external_ref` → `requeue` с `resume = task.resume` в любом статусе (`interrupted`, `running`, даже `done`, если процесс умер между итогом исполнителя и записью `Finished` в outbox: тогда `job::resume` увидит все проверенные дубли и сразу склеит). Не найдена → `add_remote_job`.
**Что пишем в строку:** `kind = Record`; `source` = `JobSource::Faceit(match_id)` для ссылки FACEIT (id берётся `wire::faceit::room_match_id`) или `JobSource::File(task.demo)`; `hero_name`/`hero_steam` из `Task`; `label = "сервер #{id} · {requested_by}"` — текст `requested_by` и ник приходят от внешнего пользователя, перед записью пропускать через `pipeline::record::log::clean`/`one_line`; `map` неизвестна до разбора — заглушка «—».
**Example (предлагаемая форма):**
```rust
// Предлагаемая сигнатура, в коде пока не существует.
impl Store {
    /// Карточка удалённой задачи: найти по `external_ref` и поставить в очередь заново или создать.
    pub fn adopt_remote(&self, n: &NewJob, external_ref: &str, resume: bool, out_dir: Option<&Path>) -> Result<i64>;
    pub fn job_by_external_ref(&self, r: &str) -> Result<Option<Job>>;
}
```

### Pattern 2: `RemoteExecutor` — поток раннера только проксирует
**What:** `run(&Task, &dyn Progress)` (1) создаёт канал и регистрирует запись в `RemoteHub` (id карточки → `Task`, отправитель, признак источника отмены), (2) усыновляет карточку, (3) крутит цикл `recv_timeout(200 мс)`: события `Stage/Log/Event/Artifact` пересылает в `p`; если `p.cancelled()`/`p.force_cancelled()` — нажимает `CancelToken` карточки (через `Queue::view().running` так же, как `App::cancel_job`) или, если карточка ещё `queued`, сразу `store.cancel_job` и возвращает `Err(CANCELLED)`; (4) получив `Settled(result)`, возвращает его раннеру.
**Почему канал:** `&dyn Progress` в потоке раннера не `'static` и в поток очереди не передаётся; события идут по `mpsc`, как уже принято для `Msg` в `app.rs`. `Progress::artifact(OutDir)` обязательно пересылается: именно по нему `Tee` пишет папку задачи в outbox (иначе после убийства `resume` не найдёт `record.json`) [VERIFIED: crates/runner/src/tee.rs:Tee::artifact — `(self.persist_out_dir)(p)`].

### Pattern 3: ветка в замыкании очереди
`jobs::runner(...)` получает `Option<Arc<RemoteHub>>`. Если у `Job` есть `external_ref` и в хабе есть живая запись: собрать `Task` (демка, ссылка FACEIT, герой из записи хаба; `resume = task.resume || job.resume`, `out_dir = job.out_dir.or(task.out_dir)`, `attempt = rep.attempt`), создать `PipelineExecutor::new(host, cfg, InspectSettings{exe: cs2-cli.exe рядом, limits: default})` заново на каждый запуск из `settings()` (приложение читает настройки на каждую задачу) с `out_root = <каталог раннера>/out` и вызвать `exec.run(&task, &tee_progress)`. `tee_progress` — `Progress`, который пишет и в `JobReporter` (журнал, этап в карточке, окно), и в канал хаба. Если записи в хабе нет (старая закрытая карточка, ручной «Повторить» после перезапуска) — обычный путь `job_spec`/`run_job` с `resume`.
**Итог:** `on_settled` вызывается из `Worker::run_one` ровно один раз после решения политики, только если `verdict != AutoRetry`; он отправляет `Settled(result)` в канал. Тем самым авто-повтор Launch (одна попытка `resume` из `record.json`, пауза 20 с) происходит внутри одной выдачи, не отдавая серверу промежуточный сбой.

### Pattern 4: ворота допуска (`Admission`) в `runner`
**What:** `Config.admission: Option<Arc<dyn Fn() -> Admission + Send + Sync>>`, `Admission::{Open, Closed(String)}`. В `Runner::tick`, ветка «нет удерживаемой задачи и нет паузы»: если закрыто — не вызывать `link.claim`, а выполнить `pause_ping`-подобный `hello` со `status = "занят: {причина}"` раз в 30 с (сервер считает раннер «онлайн» по `last_seen_at` в окне `longpoll_secs*1000 + lease_ttl_ms` ≈ 70 с, `Core::runner_online`; без пинга бот решит, что раннера нет) и вернуть `Tick::Gated(причина)`. После успешного `claim` проверить ворота ещё раз: закрылись (автор успел поставить локальную задачу) — `link.release(job_id, ReleaseRequest{epoch, reason, counts:false})` и обычный `release_cooldown`; в outbox ничего не писать. `counts:false` для этого и предусмотрен в `wire::ReleaseRequest` [VERIFIED: crates/wire/src/lib.rs:365-374 «`false` — раннер возвращает задачу не по её вине, и выдача не засчитывается»].
**Решение ворот** (в приложении, чистая функция над видом): `Open` ⇔ `view.running.is_none()` ∧ `view.paused.is_none()` ∧ `view.hold.is_none()` ∧ нет локальных задач в `queued` (`store.next_queued()` пуст; пока карточек удалённых в очереди нет, потому что их создаёт только исполнитель после `claim`) ∧ очередь/хаб не держат открытую удалённую карточку.
**`poll_secs`:** приложение ставит `Config.poll_secs = Some(5)` (поле «замена `poll_secs` сервера») — окно гонки между воротами и постановкой короткое, остановка потока быстрая; сервер при 5 с просто чаще видит запросы.

### Pattern 5: наблюдатель состояния
`Runner::run_observed(&mut self, &dyn Fn(RunnerEvent))`, `run()` = `run_observed` с пустым наблюдателем (тесты `crates/runner/tests/*` не трогаются). События: `Connected{server_version}`, `Gated(причина)`, `Paused(PauseReason)`, `Working{job_id}`, `NetError(текст)` (с ближайшим повтором), `Fatal(текст)` для `TOKEN_REJECTED`/`PROTOCOL_MISMATCH`. Приложение складывает последнее в `Arc<Mutex<RemoteStatus>>` и вызывает `ctx.request_repaint()`; окно читает значение каждый кадр (как `queue.view()`), новых вариантов `Msg` не нужно.

### Anti-Patterns to Avoid
- **Будить очередь через `Queue::resume()`.** Он снимает паузу очереди [VERIFIED: crates/app/src/queue.rs:`pub fn resume` — «Снять паузу…»]; хаб должен ждать следующего такта (500 мс) или получить отдельную команду `Cmd::Wake`.
- **Второй путь записи вне очереди** (замок между потоками): ломает авто-повтор, паузу, итоговый тост и «Повторить».
- **Прокидывать `&dyn Progress` в поток очереди через `unsafe`/`transmute`.** Только канал.
- **`hello` без `holding` как «проверка соединения».** Для задачи, которую раннер держит, сервер ответит директивой, и состояние потеряется; кнопка «Проверить» должна перезапускать цикл раннера, а не слать отдельный `hello`.
- **Менять сигнатуру `queue::start`.** Тесты очереди вызывают её напрямую, а `cfg()` собирает `QueueConfig` литералом из трёх полей; новое поле сломает компиляцию всех ~25 тестов.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Outbox и возобновление | свой файл состояния в приложении | `runner::RunnerState`/`runner_state.json` | Атомарная запись, версия формата, `pending_release`, отложенные битые файлы |
| Эпоха, аренда, пульс, отмена сервера | свой heartbeat | `Runner` + `Uplink` + `Tee` | Фаза 5/6, тесты с виртуальными часами |
| Защита от враждебной демки | вызов `run_job` из приложения напрямую | `PipelineExecutor` (дочерний `cs2-cli inspect` в Job Object, `catch_unwind`, `verify_result`) | SEC-01…03 и RUN-05/06 из фазы 6 |
| Редактирование токена в журналах | `String` | `runner::Token` (`Debug` = `Token(***)`, `Display` нет) + замена значения в журнале раннера | Уже сделано, проверено SEC-тестами |
| Единственная копия раннера | свой замок | `runner.lock` в каталоге раннера (`File::try_lock`, как в `runner_cmd.rs`) | Тот же файл разделяют приложение и `cs2-cli runner`: они не запустятся вместе |
| Блокировка конкурентной записи | свой семафор | единственный поток очереди | Уже гарантирует одну запись |

**Key insight:** вся новая логика — склейка двух готовых конечных автоматов (очередь с `QueuePolicy` и раннер с `PausePolicy`); любая попытка переписать часть автомата даёт расхождение политик.

## Ответы на вопросы задачи

### 1. Как удалённая задача становится карточкой (store/JobSource)
Миграция 6: `ALTER TABLE jobs ADD COLUMN external_ref TEXT;` + `CREATE UNIQUE INDEX IF NOT EXISTS jobs_external_ref ON jobs(external_ref) WHERE external_ref IS NOT NULL;` (шаги только добавляют, как требует комментарий над `MIGRATIONS`). `Job` получает `external_ref: Option<String>`; `Store::row` читает его по имени. `JobSource` не расширять: хватает `File`/`Faceit`. Схема сейчас на версии 5: «5 — счётчик авто-повторов (`auto_retries`, DIF-04) и партия приёмки (`batch`, ACC-01).» [VERIFIED: crates/store/src/lib.rs:266-272]. Два теста `store` содержат литерал `5`: `version_four_database_gains_auto_retries_and_batch` (`assert_eq!(user_version(&s), 5)` ×2, строки ~936 и ~942) — заменить на `schema_version() as i64` и добавить тест v5→v6 [VERIFIED: crates/store/src/lib.rs:920-945]. `cs2-cli acceptance` открывает базу только на чтение и отвергает базу старше своей версии (`open_read_only`) — собирать `cli` и `app` одной сборкой [VERIFIED: crates/store/src/lib.rs:304-314].

### 2. Как гарантируется «только при пустой очереди, не на паузе, не больше одной записи»
- Ворота до `claim` и повторная проверка после него (Pattern 4). Сервер видит раннер онлайн по `hello`-пингу.
- Одна запись: потоков очереди один; удалённая карточка исполняется в нём же; поток раннера записи не запускает.
- Локальная задача, поставленная во время удалённой, получает больший `id` и ждёт: `next_queued` берёт самую старую queued-строку (`ORDER BY id LIMIT 1`) [VERIFIED: crates/store/src/lib.rs:407-409].
- Окно гонки «ворота открыты → `claim` вернул задачу → автор поставил локальную» закрывается повторной проверкой и `release(counts:false)`; если локальная всё же вставилась между повторной проверкой и вставкой карточки, она получит меньший `id` и пойдёт первой, а удалённая подождёт на живой аренде (пульс идёт из потока раннера). Это допустимо по критерию 2 (одна запись), но риск R3 ниже.
- После `Tick::Settled`/загрузки результата поток раннера продолжает `upload` в `drive`, не берёт новых задач; загрузка идёт параллельно с записью следующей локальной задачи — это сеть, не CS2.

### 3. Возобновление после убийства
Цепочка уже работает в `runner`: `Runner::new` загружает `runner_state.json` (битый файл откладывается в `runner_state.bad-*.json`, старт с пустого) → первый `tick`: `recover_executor` (`kill_orphans` + `gamedir::recover` по `out_dir`), `hello` с `holding[(job_id, epoch)]` → директива `Keep` (продолжать), `Cancel` (итог `Cancelled`) или `Abandon`/неизвестна (файлы убираются, задача отброшена) → `drive` с сохранённого этапа; для `Executing` `execute` передаёт `resumed_dir` и `Task.resume = true`; `PipelineExecutor` пропускает предпроверку демки и вызывает `run_job` с `resume`, а при `RETRY_NO_DATA` начинает заново [VERIFIED: crates/runner/src/lib.rs:tick/hello/execute, crates/runner/src/exec.rs:Executor for PipelineExecutor]. Приложению остаётся:
1. `recover_after_crash` при старте уже убивает осиротевший `cs2.exe` и чистит папку игры по `interrupted_jobs()`; `reset_running` превращает все `running` в `interrupted`. Для карточек с `external_ref` этот сброс не должен ни считаться в `Reset.interrupted` (иначе очередь встанет на паузу `Interrupted` и «без действий автора» не выйдет), ни показываться как «прервана, нажмите Повторить». Вариант: `Reset.interrupted_remote` отдельным счётчиком, а `PauseReason::Interrupted` ставить только при `interrupted > 0` локальных [VERIFIED: crates/app/src/app.rs:`(reset.interrupted > 0).then_some(PauseReason::Interrupted)`].
2. Запуск потока раннера после очереди и после `recover_after_crash`; `RemoteExecutor::run` усыновляет карточку (Pattern 1).
3. `Executor::recover` в приложении делегирует в те же `pipeline::gamedir::kill_orphans`/`recover` (идемпотентно) или ничего не делает, но тогда нужен тест, что порядок «процесс завершён раньше уборки» сохранён.
Эпоха/аренда: пока аренда на сервере жива, `hello` с `holding` даёт `Keep`; если TTL (45 с) истёк, а задачу никто не забрал, тот же раннер получает её обратно без остановки записи (критерий 3 фазы 5); если её уже выдали заново или закрыли — `Abandon`, `drop_job`, карточка остаётся `done/failed/interrupted` в приложении с пометкой в журнале.
**Что не покрыто `runner`:** убийство на этапах `Claimed`/`Fetched` (папки задачи ещё нет): `Task.resume = false`, карточка могла уже существовать в `queued` — усыновление начинает её заново, дублей нет.

### 4. Как показывать состояние подключения и где хранить адрес и токен
- **Адрес и имя:** `Settings.runner_server: String`, `runner_name: String` (по умолчанию `COMPUTERNAME`), оба с `#[serde(default)]`. Старый `settings.json` грузится (по образцу `settings_without_afxerror_ignore_still_load`) [VERIFIED: crates/app/src/settings.rs:tests]. Пустой `runner_server` ⇒ раннер не запускается вообще. Правило `Link::new` о `https://` и частной сети (`plain_http_allowed`) срабатывает при старте: ошибка адреса показывается как состояние «Адрес не принят: …», а не паникой.
- **Токен:** отдельный файл `<каталог раннера>\token` (каталог по умолчанию `%LOCALAPPDATA%\cs2cinema\runner`, как у `cs2-cli runner` — `default_dir`). Не в `Settings`: `Settings` — `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`, пишется целиком в `settings.json` и сравнивается `draft != s` [VERIFIED: crates/app/src/settings.rs:5-7]. На странице — поле `password(true)` (как `faceit_key`) с черновиком токена в `App`, «Сохранить» пишет файл и перезапускает поток раннера; значение читается обратно только как `runner::Token`; проверка символов — как в `parse_runner` («печатные ASCII без пробелов»), иначе вечные повторы `Authorization`.
- **Права файла.** NTFS-ACL наследуется от профиля. На машине автора на `%LOCALAPPDATA%\cs2cinema` стоит полный доступ у SYSTEM, Администраторов, `PC\kamal` и ещё у локальной учётки `pc\kto-t` [VERIFIED: `icacls "$LOCALAPPDATA/cs2cinema"` в этой сессии]. Значит «лежит в профиле» не равно «читает только автор». Рекомендация: писать файл токена с отключённым наследованием и единственной записью для текущего пользователя (через `icacls /inheritance:r /grant:r "%USERNAME%:F"` вызовом через `Host::run` с дедлайном, либо `windows-sys` Security API, уже в зависимостях `host`), а на не-Windows (`cfg(unix)`, тесты CI) — `OpenOptionsExt::mode(0o600)`. DPAPI (`CryptProtectData`) — более сильный вариант, но добавляет feature `windows-sys` и формат файла; решение оставить автору (Open Question 3). Это `[ASSUMED]` в части того, что `icacls /inheritance:r` корректно отработает без повышения прав для файла в профиле.
- **Токен не в логах:** журнал раннера уже заменяет значение на `***`; в приложении запрещено форматировать `Config` (его `Debug` печатает `***`) и тексты ошибок `LinkError` можно показывать (заголовок `Authorization` в них не попадает); тест SEC-типа: подставной токен после принудительных сбоев (401, обрыв, мусор в ответе) не находится ни в `runner.log`, ни в `jobs.log`/`error` БД, ни в `RemoteStatus`-тексте, ни в `settings.json`.
- **Состояние (`RemoteStatus`)**, строка и цвет на странице «Настройки» (раздел «Удалённые задачи») и бейдж/строка в боковой панели: «Выключено (адрес не задан)», «Подключаюсь…», «На связи · сервер {версия} · ждёт задачи», «Занят: идёт локальная задача / очередь на паузе» (причина ворот), «Выполняет задачу #{id}», «Нет связи: {текст}; повтор через {n} с», «Токен не принят — введите новый», «Версия протокола не совпала: {текст}», «Пауза раннера: {код}…», «Раннер уже запущен в другом процессе (cs2-cli runner)», «База недоступна — удалённые задачи отключены», «Не найден cs2-cli.exe рядом с приложением». Кнопки: «Переподключить» (перезапуск потока), «Продолжить» при паузе раннера (`Control::resume`).

### 5. Как приложение остаётся прежним без адреса
`App::new`: если `runner_server.trim().is_empty()` — `RemoteHub`/поток не создаются, ветка в `jobs::runner` видит `None`, хук очереди не подключается, сетевых вызовов ноль; `queue::start` вызывается как сейчас. Тест: счётчик «поток раннера запущен» = 0, `queue::start` вызван с прежними аргументами (ни одно существующее значение по умолчанию не меняется), `Settings::default()` содержит пустой `runner_server`. Миграция 6 только добавляет колонку; старые данные и `cs2-cli` без сервера не затрагиваются.

## Common Pitfalls

### Pitfall 1: отмена по команде сервера ставит локальную очередь на паузу
**What goes wrong:** сервер отменил задачу → карточка получает `Err(CANCELLED)` → `QueuePolicy::after` возвращает `Pause(Cancelled)` [VERIFIED: crates/app/src/queue.rs:`Finished::Cancelled => … Verdict::Pause(PauseReason::Cancelled)`] → локальные задачи встали, хотя автор ничего не отменял.
**How to avoid:** хаб помечает происхождение отмены (`Server`/`Local`); хук `on_settled` возвращает `suppress_pause`, `run_one` не применяет `Pause(Cancelled)` для отмены сервера. Отмена автором кнопкой — прежнее поведение (пауза), раннер ответит `finish(Cancelled)`.
**Warning signs:** тест «отмена сервера не ставит паузу» красный.

### Pitfall 2: два автомата считают сбои
**What goes wrong:** `QueuePolicy` (две одинаковые неисправности подряд ⇒ пауза очереди) и `PausePolicy` раннера (то же ⇒ пауза раннера) срабатывают независимо; пауза очереди снимается «Продолжить», пауза раннера — нет.
**How to avoid:** «Продолжить» в окне вызывает и `queue.resume()`, и `Control::resume()`; в статусе раннера есть собственная кнопка на случай расхождения. Не отключать ни один из автоматов для удалённой карточки.

### Pitfall 3: `std::process::exit` без остановки
**What goes wrong:** пункт «Выход» в трее и самообновление зовут `exit(0)` [VERIFIED: crates/app/src/main.rs, app.rs `Msg::Updated`]. Для удалённой задачи это равносильно убийству: безопасно (возобновится), но самообновление посреди удержания лучше отложить до конца задачи.

### Pitfall 4: останов раннера кнопкой = «отмена» для очереди
**What goes wrong:** `Control::stop()` делает `Tee::cancelled()` истинным, карточка отменяется, очередь встаёт на паузу.
**How to avoid:** запретить выключение/смену адреса, пока удалённая задача в статусе «выполняется» (подсказка «дождитесь конца задачи или закройте приложение»), в остальных состояниях останов безопасен. Тогда путь `LocalStop` в хабе не нужен.

### Pitfall 5: ролик пропадает с диска после подтверждения сервером
**What goes wrong:** `Runner::remove_finished` удаляет результат и папку задачи (только внутри каталога раннера), карточка «готово» ссылается на отсутствующий файл [VERIFIED: crates/runner/src/lib.rs:remove_finished]. Превью/«Открыть» не сработают.
**How to avoid:** считать это штатным (диск не копится, OPS-01 в фазе 9), в карточке писать «отправлен на сервер, локальная копия удалена»; `JobsState.sizes` и `request_thumbs` уже проверяют `is_file()`.

### Pitfall 6: тихий пропуск предпроверки
**What goes wrong:** если исполнять карточку обычным `job_spec`, чужая демка не пройдёт дочерний `inspect`, `verify_result` и ограничения размеров (SEC-02, RUN-06).
**How to avoid:** удалённая карточка всегда идёт через `PipelineExecutor`; отсутствие `cs2-cli.exe` рядом с `current_exe()` делает удалённые задачи недоступными (состояние «Не найден cs2-cli.exe»), но не переключает на небезопасный путь.

### Pitfall 7: ложное «ключ FACEIT — переменная окружения»
`ensure_faceit_demo` пишет «у раннера не задан ключ FACEIT (переменная FACEIT_API_KEY)» [VERIFIED: crates/runner/src/exec.rs]. В приложении ключ берётся из `Settings.faceit_key` (`job_config`). Поправить текст так, чтобы он подходил обоим (`ключ FACEIT не задан`), и не ломать тест `missing_key_is_faceit_fetch_failed` (проверять по коду `FACEIT_FETCH_FAILED`).

### Pitfall 8: аренда держится, а карточка не стартует
Очередь на паузе (например, `Repeated`) или идёт долгая локальная задача, поставленная до возобновления: поток раннера шлёт пульсы, сервер считает задачу «в работе». Рекомендация: в `RemoteStatus`/карточке показывать «ждёт очередь приложения»; жёсткий потолок ожидания (например, 10 мин ⇒ `release(counts:false)`) вынести в Open Question 4.

## Code Examples

### Хук очереди без изменения сигнатуры `start`
```rust
// Предлагаемо. queue.rs: старый вход остаётся, тесты не трогаются.
pub trait SettleHook: Send + Sync {
    /// Вызывается один раз после решения политики, если оно не AutoRetry.
    fn on_settled(&self, job: &Job, finished: &Finished, result: &Result<JobOutcome, RunError>) -> Advice;
}
pub struct Advice { pub suppress_pause: bool }

pub fn start(...) -> Queue { start_with_hook(..., None) }          // прежняя сигнатура
pub fn start_with_hook(..., hook: Option<Arc<dyn SettleHook>>) -> Queue { /* тело прежнего start */ }
```
Источник контрактов: `Worker::run_one` и `QueuePolicy::after` [VERIFIED: crates/app/src/queue.rs].

### Ворота и наблюдатель в `runner`
```rust
// Предлагаемо. crates/runner/src/lib.rs
pub enum Admission { Open, Closed(String) }
pub struct Config { /* ... */ pub admission: Option<Arc<dyn Fn() -> Admission + Send + Sync>> }
pub enum RunnerEvent { Connected { server_version: String }, Gated(String), Paused(PauseReason), Working { job_id: i64 }, NetError(String), Fatal(String) }
impl Runner { pub fn run_observed(&mut self, observer: &dyn Fn(RunnerEvent)) -> Result<(), String>; }
```
Новое поле `Config` нужно добавить во все литералы (`crates/cli/src/runner_cmd.rs`, `crates/runner/tests/common/mod.rs`) значением `None` — перечень мест проверить `grep "Config {"` перед правкой.

### Усыновление карточки
```rust
// Предлагаемо, RemoteExecutor::run
let r = format!("srv:{}:{}", task.job_id, epoch);   // epoch нужен Executor: см. Open Question 5
let id = store.adopt_remote(&new_job_from(task), &r, task.resume, task.out_dir.as_deref())?;
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `tasklist`/`taskkill`, `launch.rs` | `host::Host`, Job Object, удержание по дескриптору | фаза 4 v1.0 | Приложение не ищет процессы по имени; чужой `cs2.exe` не трогается |
| Раннер только как `cs2-cli runner` | Тот же `runner` внутри приложения через `Executor` | фаза 8 | Один раннер, два хозяина; `runner.lock` не даёт запустить их одновременно |

**Deprecated/outdated:** текст справки `USAGE_RUNNER` «Не запускайте очередь приложения одновременно с раннером» после фазы 8 нужно заменить пояснением: в приложении раннер встроен, отдельный `cs2-cli runner` на той же машине запускать не нужно [VERIFIED: crates/cli/src/runner_cmd.rs:USAGE_RUNNER].

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `icacls /inheritance:r /grant:r` без повышения прав сузит доступ к файлу токена в профиле | Ответ 4 | Токен останется читаемым `pc\kto-t`; запасной путь — DPAPI |
| A2 | Любой аутентифицированный запрос (в том числе `hello`) обновляет `last_seen_at`, поэтому пинг раз в 30 с держит раннер «онлайн» (окно ≈ 70 с) | Pattern 4 | Бот сообщит «раннера нет» при закрытых воротах; проверить тестом на сервере (по grep видны `UPDATE runners SET last_seen_at` в `auth.rs:182` и `core.rs:759`; тела функций целиком не читались; `runner_online` — `core.rs:596-608` читалась) |
| A3 | `poll_secs = Some(5)` допустим для сервера (нет нижней границы) | Pattern 4 | Сервер может отвергать частые `claim`; проверить на `Stand` |
| A4 | Релизный zip кладёт `cs2-cli.exe` рядом с `cs2cinema.exe` | Pitfall 6 | Без файла удалённые задачи недоступны; `release.yml:43` по grep копирует `cs2-cli.exe` в каталог пакета; что рядом лежит `cs2cinema.exe`, файл целиком не читался |
| A5 | Дедлайн ожидания карточки в очереди не нужен для первой версии | Pitfall 8 | Задача может «зависнуть» на живой аренде при долгой паузе очереди |

## Open Questions

1. **Что значит «Повторить» у удалённой карточки, чья выдача уже закрыта на сервере?**
   - Known: до закрытия внутри одной выдачи работает авто-повтор Launch (один раз, `resume`); после `finish`/`release` аренды нет, демка раннера удалена (`remove_demo`/`remove_finished`), результат уже на сервере либо задача возвращена в очередь сервера.
   - Unclear: критерий 1 требует «Повторить» на карточке.
   - Recommendation: показывать «Повторить» (локальное продолжение по `record.json`, без загрузки на сервер; в журнале строка «результат не отправляется: задача на сервере закрыта») и скрывать «Заново» (демки нет); пока карточка удержана раннером (есть запись в хабе или статус `interrupted` ждёт усыновления), обе кнопки скрыты. Подтвердить с автором.
2. **Где в UI жить состоянию?** Рекомендация: раздел «Удалённые задачи» на странице настроек плюс компактная строка в боковой панели.
3. **Токен: файл с ограниченным ACL или DPAPI?** Рекомендация: файл + `icacls`; DPAPI — если автор хочет защиту от других локальных учёток.
4. **Нужен ли потолок ожидания удержанной задачи в очереди приложения?**
5. **Нужна ли `Executor` эпоха?** `Task` её не содержит [VERIFIED: crates/runner/src/exec.rs `pub struct Task`]. Для уникального `external_ref` либо добавить `epoch: i64` в `Task` (правки тестовых заглушек в `crates/runner/tests/common/mod.rs`), либо использовать `srv:{job_id}` и усыновлять любую карточку, не закрытую в этой же выдаче. Рекомендация: добавить `epoch` в `Task`.
6. **Метка и карта карточки:** какую подпись показывать, пока карта неизвестна (рекомендация: «—»).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | сборка, тесты | ✓ | rustc 1.99.0 | — |
| `cs2-cli.exe` рядом с `cs2cinema.exe` | дочерняя предпроверка демки | в релизном пакете (A4) | — | без него удалённые задачи отключены с понятным состоянием |
| `ffmpeg`/`ffprobe`, HLAE, CS2 | запись и проверка ролика | как в v1.0 (`precheck`) | — | `PRECHECK_FAILED`-ветка существующей предпроверки |
| Живой `cs2-server` | живой прогон | не проверялся (стенд отложен в фазу 9) | — | `server::serve` в процессе теста |

**Missing dependencies with no fallback:** нет (живой стенд — задача фазы 9).

## Validation Architecture

`workflow.nyquist_validation` в `.planning/config.json` равен `false` [VERIFIED: чтение config.json], поэтому формальная карта Nyquist не нужна; ниже перечень тестов, без которых критерии не проверяемы.

| Req | Поведение | Тип | Где | Команда |
|-----|-----------|-----|-----|---------|
| APP-04 | существующие тесты очереди и конвейера | регрессия | `app::queue` (≈25 тестов), `cli/tests/pipeline.rs` | `cargo test -p app queue` ; `cargo test -p cli --test pipeline` |
| APP-04 | без адреса: ноль потоков и сетевых вызовов; старый `settings.json` грузится | unit | `app::settings`, `app::remote` | `cargo test -p app remote` |
| APP-01 | миграция 6: v5→v6, уникальность `external_ref`, повторное открытие, два обновлённых теста с литералом `5` | unit | `store` | `cargo test -p store` |
| APP-01 | `adopt_remote`: создание, усыновление в любом статусе, метка очищена от управляющих символов | unit | `store`/`app::remote` | `cargo test -p store adopt` |
| APP-02 | ворота закрыты при `running`, `paused`, `hold`, локальной queued — `claim` не вызывается, `hello`-пинг идёт | integration | `crates/runner/tests/gate.rs` на `Stand` | `cargo test -p runner --test gate` |
| APP-02 | ворота закрылись после `claim` ⇒ `release(counts:false)`, число выдач не выросло | integration | то же | то же |
| APP-02 | локальная задача, поставленная во время удалённой, ждёт; максимум одновременных запусков = 1 | unit | `app::queue` со счётчиком в заглушке | `cargo test -p app queue::remote` |
| APP-01 | удалённая карточка: этапы и журнал попадают в `Tee`, отмена сервера доходит до `CancelToken` и не ставит паузу; Launch-сбой повторяется внутри выдачи, `on_settled` один раз | unit/integration | `app::queue`, `app::remote` | то же |
| APP-01 | e2e: `server::serve` + `FakeHost` + заглушка `job_fn`: демка → карточка → `done` → результат на сервере | integration | `app/tests/remote_e2e.rs` | `cargo test -p app --test remote_e2e` |
| APP-03 | «убийство»: `runner_state.json` на этапе `Executing`, карточка `interrupted` в store ⇒ после старта `requeue` с `resume`, результат доходит, пауза `Interrupted` не ставится | integration | `app/tests/remote_e2e.rs` | то же |
| SEC-04 | подставной токен после сбоев (401, обрыв, мусор) не найден в `runner.log`, БД, `RemoteStatus`, `settings.json`, `Debug` | integration | `app/tests/remote_secrets.rs` | `cargo test -p app --test remote_secrets` |

### Wave 0 Gaps
- [ ] тестовый стенд сервера в `app` (скопировать минимум из `crates/runner/tests/common/mod.rs`: `TestDir`, `Stand::start`, `submit`, `runner_config`) — общий крейт не выделять в этой фазе
- [ ] `Task.epoch` и обновление заглушек `ScriptedExecutor`
- [ ] `Config.admission`, `Tick::Gated`, `run_observed` в `runner` с тестами в `crates/runner/tests/gate.rs`

Частота: после каждой задачи `cargo test -p <затронутый крейт>`; перед закрытием фазы `cargo test --workspace`.

## Security Domain

`security_enforcement` в конфиге не отключён ⇒ включён.

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | да (токен раннера) | Bearer-токен только в заголовке, перенаправления запрещены (`Link`), `http://` только для localhost и частной сети |
| V3 Session Management | нет (сессий нет, эпоха — защита выдачи) | эпоха и аренда сервера |
| V4 Access Control | да | сервер выдаёт только то, что принадлежит токену; приложение не принимает входящих запросов |
| V5 Input Validation | да | ник, `requested_by`, ссылка FACEIT из задачи — недоверенный текст: `one_line`/`clean` перед записью в БД/журнал; ссылка только через `wire::faceit::room_match_id`; пути строятся из номера задачи |
| V6 Cryptography | да (хранение токена) | не изобретать: ACL файла или DPAPI (решение автора) |
| V7 Error handling/logging | да | `Token` без `Display`, замена значения в журнале, тест SEC-04 в приложении |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Токен в `settings.json`/логе/`Debug` | Information Disclosure | отдельный файл, `runner::Token`, тест поиска подставного токена |
| Чужая демка исполняется приложением | Elevation/DoS | карточка идёт через `PipelineExecutor` (дочерний `inspect` в Job Object, потолки размера и времени) |
| Враждебный ник/метка в UI и журнале | Tampering/Spoofing | очистка управляющих символов, длина ограничена |
| Токен отправлен на чужой адрес через редирект | Information Disclosure | `Link` запрещает перенаправления; адрес валидируется при сохранении |
| Две копии раннера пишут одновременно | DoS | `runner.lock`, одна очередь |

## Риски фазы (кратко)

- **R1 (высокий): регрессия очереди.** Тесты `app::queue` строят `QueueConfig` литералом, `start(...)` вызывают напрямую; хук подключать только новой функцией, `Finished`/`Verdict`/`QueuePolicy` не менять, а `suppress_pause` применять в `run_one` только когда хук вернул совет (для локальных карточек хук не вызывается).
- **R2 (высокий): авто-повтор и итог раннеру.** `on_settled` вызывать после политики и только для `verdict != AutoRetry`, иначе серверу уйдёт промежуточный сбой и выдача сгорит.
- **R3 (средний): гонка ворот.** Закрыта повторной проверкой и `release(counts:false)`, остаточное окно (локальная вставилась после повторной проверки) безопасно по числу записей.
- **R4 (средний): `reset_running` на старте** пометит удалённую карточку `interrupted` и выставит `fail_code = 'INTERRUPTED'` [VERIFIED: crates/store/src/lib.rs:reset_running]; усыновление через `requeue_with` сбрасывает `fail_*`.
- **R5 (средний): `cs2-cli.exe` не найден.** Нужно состояние и тест, а не молчаливый запуск.
- **R6 (средний): двойная пауза** (Pitfall 2).
- **R7 (низкий): `Queue::resume()` как «разбудить».**
- **R8 (низкий): токен виден другим локальным учёткам** на машине автора (факт `icacls`).
- **R9 (низкий): ожидание удержанной задачи** (Pitfall 8).

## Sources

### Primary (HIGH confidence — прочитано в этой сессии)
- `.planning/ROADMAP.md` (Phase 8), `.planning/REQUIREMENTS.md` (APP-01…04), `.planning/phases/06-*/06-CONTEXT.md`, `.planning/phases/07-*/07-CONTEXT.md`, сводки 07-07
- `crates/runner/src/{lib,exec,state,policy,link,tee}.rs`, `crates/runner/Cargo.toml`, `crates/runner/tests/{flow.rs,common/mod.rs}`
- `crates/cli/src/runner_cmd.rs` (аргументы, `runner.lock`, `default_dir`, сборка `Config`)
- `crates/app/src/{queue,jobs,app,settings,settings_page,jobs_page,main}.rs`, `crates/app/Cargo.toml`
- `crates/store/src/lib.rs` (схема, `MIGRATIONS`, `next_queued`, `reset_running`, `requeue_*`, `open_read_only`)
- `crates/pipeline/src/{lib,job,inspect}.rs`, `crates/wire/src/lib.rs` (`ReleaseRequest`, `HelloRequest`, `Phase`, `paths`), `crates/server/src/{core,auth}.rs` (`runner_online`, `last_seen_at`), `crates/server/tests/deps.rs`
- `icacls` на `%LOCALAPPDATA%\cs2cinema` (факт о правах на этой машине)

### Secondary / Tertiary
- Нет: внешние источники не использовались, все выводы из кода репозитория.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — новых пакетов нет, зависимости читались.
- Architecture: HIGH по границам потоков и шву `Executor`; MEDIUM по остаточным деталям (формат `external_ref`, `epoch` в `Task`, наблюдатель).
- Pitfalls: HIGH для найденных в коде (пауза при отмене, литералы `5`, `Queue::resume`), MEDIUM для поведения на живом сервере (A2, A3).

**Research date:** 2026-10-12
**Valid until:** 30 дней или до изменения `queue.rs`/`runner` другой фазой (фаза 9 их не меняет по плану).
