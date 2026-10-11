# Фаза 7: Telegram-бот и доставка — карта паттернов

**Составлено:** 2026-10-11
**Файлов проанализировано:** 24 (новых и изменяемых)
**Аналогов найдено:** 22 / 24 (все указанные пути отслеживаются git; зеркал `.gsd/capabilities` нет)

Все пути и номера строк проверены чтением. Номера строк — на состояние `main` при составлении.

## Классификация файлов

| Новый / изменяемый файл | Роль | Поток данных | Ближайший аналог | Качество |
|---|---|---|---|---|
| `crates/server/src/tg/mod.rs` (трейт `Tg`, `TgError`, типы Update/File) | service (шов) | request-response | `crates/host/src/lib.rs` (трейт `Host`) + `crates/server/src/clock.rs` (`Clock`) | роль |
| `crates/server/src/tg/http.rs` (`HttpTg` на ureq) | service | request-response | `crates/faceit/src/lib.rs` (`Client`, ureq) + `crates/server/tests/common/mod.rs` (`send`) | роль |
| `crates/server/src/tg/fake.rs` (`FakeTelegram`) | test-double (публичный) | event-driven (сценарий + журнал вызовов) | `crates/host/src/fake.rs` (`FakeHost`, `Scenario`) + `ManualClock` в `clock.rs` | точное |
| `crates/server/src/bot/mod.rs` (`Bot::step`, потоки) | service | event-driven | `spawn_sweeper` в `crates/server/src/lib.rs:121-151` | роль |
| `crates/server/src/bot/commands.rs` | utility (разбор команд, тексты) | transform | `crates/server/src/cli.rs` | роль |
| `crates/server/src/bot/store.rs` (SQL таблиц бота) | model | CRUD | `crates/server/src/users.rs` | точное |
| `crates/server/src/bot/ingest.rs` | service | file-I/O | `Core::submit` (`core.rs:309-325`) | роль |
| `crates/server/src/bot/notify.rs` | service | event-driven | `Core::sanitize_failure` (`core.rs:~610`) + `wire::fail` | частичное |
| `crates/server/src/bot/deliver.rs` | service | file-I/O + request-response | `Core::finish_report` (`core.rs:644`) для порядка «намерение, затем результат» | частичное |
| `crates/server/src/db.rs` (миграция 2) | migration | CRUD | сам `db.rs` `MIGRATIONS` (`db.rs:24-87`) | точное |
| `crates/server/src/core.rs` (расширения) | service | CRUD | `begin_receive` / `submit` / `cancel` / `assignment_of` | точное |
| `crates/server/src/users.rs` (`find_user_by_tg`, `remove_user`, `set_player`) | model | CRUD | сам `users.rs` | точное |
| `crates/server/src/config.rs` (ключи бота) | config | transform | сам `config.rs` (`Config`, `FileConfig`) | точное |
| `crates/server/cs2-server.example.json` | config | — | сам файл | точное |
| `crates/server/src/secret.rs` (`~`-вариант токена) | utility | transform | `Redactor::new` + `mask_bot_tokens` (`secret.rs:58-150`) | точное |
| `crates/server/src/http.rs` (CR-01: `body_length`) | middleware | request-response | `overloaded` + диспетчер (`http.rs:114-162`) | точное |
| `crates/server/src/rules.rs` (WR-01: `Release` при отмене) | model (автомат) | CRUD | `Event::Release` (`rules.rs:190-217`) | точное |
| `crates/server/src/lib.rs` (`serve`, `Running`) | config/wiring | event-driven | сам `lib.rs:56-118` | точное |
| `crates/wire/src/faceit.rs` (строгий разбор ссылки) | utility | transform | `faceit::parse_match_id` / `is_match_id` (`faceit/src/lib.rs:21-39`) | роль (нужен строже) |
| `crates/runner/src/exec.rs` (скачивание FACEIT) | service | file-I/O | `PipelineExecutor::run` и `unsupported_source` (`exec.rs:206-241`) | точное |
| `crates/runner/Cargo.toml` | config | — | сам файл (нет зависимости `faceit`) | точное |
| `crates/server/Cargo.toml` (`ureq` из dev в deps) | config | — | сам файл | точное |
| `crates/server/tests/bot_*.rs` (whitelist, ingest, status, delivery, cancel_retry, dedupe, limits, users) | test | request-response | `crates/server/tests/core_states.rs` (ManualClock) + `tests/common/mod.rs` (`TestServer`) | точное |
| `crates/server/tests/deps.rs`, `tests/secrets.rs`, `tests/protocol_http.rs` (дополнить) | test | — | они же | точное |

Без аналога: поддельный HTTP Bot API на `tiny_http` (см. раздел «Без аналога»).

## Назначения паттернов

### `crates/server/src/tg/fake.rs` — `FakeTelegram` (публичный, не `cfg(test)`)

**Аналог:** `crates/host/src/fake.rs` (`FakeHost`) и `crates/server/src/clock.rs` (`ManualClock`).

Что копировать:

1. Модульная шапка `//!`: фейк обычный модуль, не `cfg(test)`, потому что им пользуются интеграционные тесты (`host/src/fake.rs:1-11`). Для `FakeTelegram` это обязательно: тесты лежат в `crates/server/tests/`.
2. Сценарий билдером с цепочкой методов, возвращающих `Self` (`host/src/fake.rs:138-410`), например `pub fn drop_marker(mut self, m: FakeMarker) -> Self`. Для бота: `fail_next(method, TgError)`, `push_update`, `put_document(file_id, bytes)`.
3. Журнал вызовов геттером-копией (`host/src/fake.rs:508-540`): `runs()`, `sent()`, `notifications()`. Для бота: `calls() -> Vec<Call>`, счётчик `get_file_calls()`.
4. Внутреннее состояние под `Arc<Mutex<..>>`, клоны делят его (как `ManualClock`):

```rust
// crates/server/src/clock.rs:29-48
#[derive(Clone)]
pub struct ManualClock {
    ms: Arc<AtomicI64>,
}
impl ManualClock {
    pub fn new(start_ms: i64) -> Self { Self { ms: Arc::new(AtomicI64::new(start_ms)) } }
    pub fn advance(&self, d: Duration) { self.ms.fetch_add(d.as_millis() as i64, Ordering::SeqCst); }
}
```

```rust
// crates/host/src/fake.rs:11-14 (импорты и стиль)
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
```

Особенность `FakeTelegram`: `put_document` кладёт байты в настоящую временную папку, `get_file` возвращает настоящий абсолютный путь (ingest реально читает диск). Временная папка с удалением в `Drop` — копия `TestDir` из `tests/common/mod.rs:16-37`:

```rust
static COUNTER: AtomicUsize = AtomicUsize::new(0);
pub struct TestDir { pub path: PathBuf }
impl TestDir {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("cs2srv-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}
impl Drop for TestDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); } }
```

Мьютекс: брать как `host`/`Db` — с восстановлением отравления (`db.rs:142-144`):

```rust
self.conn.lock().unwrap_or_else(|e| e.into_inner())
```

### `crates/server/src/tg/mod.rs` — трейт `Tg`

**Аналог:** `crates/server/src/clock.rs:9-12` (маленький трейт `Send + Sync`) и `host::Host`.

```rust
// clock.rs:9-12
pub trait Clock: Send + Sync {
    /// Текущее время, unix-миллисекунды.
    fn now_ms(&self) -> i64;
}
```

Копировать форму: `pub trait Tg: Send + Sync`, русская doc-строка у каждого метода с неочевидным контрактом, внедрение как `Arc<dyn Tg>` (так же, как `Arc<dyn Clock>` в `serve`, `lib.rs:57-61`). Ошибка — собственный enum `TgError` (RESEARCH, Pattern 1: `Api{code, description, retry_after}`, `Timeout{sent}`, `Io`); проектный `Result<T> = Result<T, String>` (`lib.rs:21`) для самих методов не годится, потому что нужна классификация `uncertain`. Конвертация в `String` через `Redactor::redact` на границе с журналом/чатом.

### `crates/server/src/tg/http.rs` — `HttpTg`

**Аналог:** `crates/faceit/src/lib.rs:100-125` (ureq 3, `Agent`, `send_json`, ручная проверка статуса) и `tests/common/mod.rs` (`send`).

```rust
// faceit/src/lib.rs:108-114 — стиль вызова ureq 3
let mut resp = self
    .agent
    .post(DOWNLOAD_API)
    .header("Authorization", &format!("Bearer {}", self.key))
    .send_json(serde_json::json!({ "resource_url": resource_url }))?;
check(resp.status().as_u16(), "ссылка на скачивание ...")?;
let json: serde_json::Value = resp.body_mut().read_json()?;
```

В Bot API статусом ошибки считать нельзя (`http_status_as_error(false)`), `ok:false` разбираем сами. Два `Agent`: управляющий (`timeout_global` 30 с) и передающий (`getFile`, `sendVideo`, длинный `timeout_recv_response`), коды см. RESEARCH «Тайм-ауты клиента (ureq 3)» и «Разбор ответа Bot API». Токен собирается в URL только внутри `HttpTg`, берётся через `Secret::expose` (`secret.rs:28-30`); любая ошибка наружу идёт через `Redactor::redact`. Не логировать `file_path`.

`ureq` перенести из `[dev-dependencies]` в `[dependencies]` в `crates/server/Cargo.toml` (сейчас строка 22: `ureq = { version = "3", features = ["json"] }`). Охрана `tests/deps.rs` ловит только path-зависимости и имена из `WORKSPACE_CRATES` (`deps.rs:6-21`), `ureq` её не нарушает.

### `crates/server/src/bot/mod.rs` — шаговый автомат и потоки

**Аналог:** `spawn_sweeper` (`crates/server/src/lib.rs:121-151`) и `Running::stop` (`lib.rs:44-52`).

```rust
// lib.rs:121-151 (суть)
fn spawn_sweeper(core: core::Core, log: log::Log, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        'outer: loop {
            // Шаги по 200 мс: остановка не ждёт целых 5 секунд.
            for _ in 0..25 {
                if stop.load(Ordering::SeqCst) { break 'outer; }
                std::thread::sleep(Duration::from_millis(200));
            }
            match core.sweep() { Ok(s) => { /* log.info(..) */ } Err(e) => log.warn(&format!("sweep: {e}")) }
        }
    })
}
```

Копировать: поток крутит только `bot.step()`, останавливается по общему флагу `shutdown` (тот же `Arc<AtomicBool>`, что у HTTP, `lib.rs:90`), шаги сна короткие. Ошибки только в `log.warn`, наружу не паникуют. `Running` расширяется полем `bot: Option<BotHandle>` и `stop` ждёт потоки так же, как `sweeper` (`h.join()`). Бот стартует только при заданном `cfg.telegram_bot_token` (без токена поведение и тесты фазы 5-6 не меняются). Время — только `core.clock().now_ms()` (`core.rs:228`), никогда `Instant::now()` в логике троттлинга.

Бот зовёт `Core` напрямую (`core.rs`), состояние задач не пишет сам.

### `crates/server/src/bot/store.rs` и миграция 2 — `crates/server/src/db.rs`

**Аналог:** `MIGRATIONS` в `db.rs:21-87`, `users.rs`.

Правило из `db.rs:21-23`: «Новая версия — только новым шагом в конце списка и только добавлением колонок или таблиц», элемент k поднимает схему до версии k+1.

```rust
// db.rs:24 и 85-87 — как добавить шаг: второй элемент массива
const MIGRATIONS: &[&str] = &["
CREATE TABLE users ( ... );
...
CREATE INDEX files_job ON files(job_id);
",
// Новый шаг 2 — отдельным элементом:
"
CREATE TABLE tg_kv (...);
CREATE TABLE tg_inbox (..., UNIQUE(chat_id, message_id));
CREATE TABLE tg_jobs (...);
CREATE TABLE tg_files (file_unique_id TEXT PRIMARY KEY, ...);
ALTER TABLE users ADD COLUMN removed_at INTEGER;
CREATE INDEX files_sha256 ON files(sha256);
"];
```

Стиль колонок: `INTEGER PRIMARY KEY AUTOINCREMENT`, `CHECK(col IN (...))` для перечислений, метки времени `INTEGER NOT NULL` в unix-мс (`db.rs:25-87`). `PRAGMA foreign_keys=ON` включён (`db.rs:110`): таблицы бота со ссылкой на `jobs(id)` и `users(id)`, пользователей не удалять (мягкая блокировка `removed_at`).

Тесты схемы: расширить перечень таблиц в `fresh_database_migrates_to_the_current_version` (`db.rs:189-205`, сейчас `["users","runners","jobs","files"]`) и оставить `reopening_changes_nothing` / `newer_database_is_refused` как есть (они используют `schema_version()`, число шагов подхватывается само).

Доступ к SQL в `bot/store.rs` — по образцу `users.rs`:

```rust
// users.rs:28-63 — запись: транзакция через Db::write, проверки внутри, db_err на всё
db.write(|tx| {
    let taken: Option<i64> = tx
        .query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get(0))
        .optional()
        .map_err(db_err)?;
    if taken.is_some() { return Err(format!("пользователь «{name}» уже есть")); }
    tx.execute("INSERT INTO users(name, tg_id, role, created_at) VALUES (?1, ?2, ?3, ?4)",
               params![name, tg_id, role, now_ms]).map_err(db_err)?;
    Ok(User { id: tx.last_insert_rowid(), .. })
})
// users.rs:65-85 — чтение: Db::read, prepare + query_map, константа с перечнем колонок
const USER_COLUMNS: &str = "id, name, tg_id, role, created_at";
```

Для Pattern 2 (exactly-once): смещение `getUpdates` и вставка `INSERT OR IGNORE INTO tg_inbox` в одной `db.write` (это `BEGIN IMMEDIATE`, `db.rs:156-167`). Мьютекс базы не держать во время `getFile` и копирования (правило `db.rs:3-4`).

Новые функции `users.rs`: `find_user_by_tg` по образцу `find_user` (`users.rs:75-85`, с `.optional()`), `remove_user` — `UPDATE users SET removed_at = ?`, `set_player` — `UPDATE users SET player_steam=?, player_name=?` (колонки уже в миграции 1). Структуру `User` и `USER_COLUMNS` расширить добавлением (не ломая `user_from_row`, `users.rs:16-26`). Тест в стиле `users_are_added_listed_and_found` (`users.rs:91-106`): русские сообщения об ошибках, `Db::in_memory()`.

### `crates/server/src/core.rs` — расширения ядра

**Аналоги в том же файле:**

- `begin_receive` (`core.rs:241-261`) жёстко пишет `'file'`; `submit_faceit` строить как её копию без файла:

```rust
// core.rs:241-260
let player = self.redactor.clean(s.player.trim(), 64);
if player.is_empty() { return Err("не указан игрок".to_owned()); }
let steam = is_steam64(&player).then(|| player.clone());
let orig = self.redactor.clean(&s.orig_name, 120);
let by = self.redactor.clean(&s.requested_by, 64);
self.db.write(|tx| {
    tx.execute(
        "INSERT INTO jobs(user_id, requested_by, state, source_kind, orig_name, player_steam, player_name, created_at, updated_at)
         VALUES (?1, ?2, 'receiving', 'file', ?3, ?4, ?5, ?6, ?6)",
        params![s.user_id, by, orig, steam, player, now],
    ).map_err(db_err)?;
    Ok(tx.last_insert_rowid())
})
```

  Для `submit_faceit`: `state='queued'`, `source_kind='faceit'`, `faceit_link` = каноническая ссылка из `wire::faceit::canonical_url`. Задача создаётся сразу в `queued`, поэтому `RECEIVING_STALE_MS` (`core.rs:1041`) её не затронет.
- `submit` (`core.rs:309-325`): `sniff_demo`, проверка `max_demo_bytes`, затем `begin_receive` + `finish_receive`. `ingest.rs` бота вызывает `core.submit(&path, &Submit{ user_id: Some(..), ..})` после того, как `getFile` положил файл на диск, и до него не создаёт задачу (Pitfall 8).
- `cancel(job_id, force)` (`core.rs:593-613`) вызывается ботом как есть; бот не пишет `jobs.state` (anti-pattern из RESEARCH). Для `/cancel` нужен WR-01 (ниже).
- `assignment_of` (`core.rs:946-964`): сейчас `demo_by_job(c, id)?.ok_or_else(|| format!("у задачи #{id} в очереди нет демки"))?`. Заменить на ветку для `job.source_kind == "faceit"`: `DemoInfo { size: 0, sha256: String::new(), orig_name }`. Раннер уже умеет `demo_size == 0` (`runner/src/lib.rs:174-176`).
- `Submit` (`core.rs:107-112`) расширять только добавлением полей: `#[derive(Default)]` позволяет тестам писать `..Default::default()`.
- Новые методы (`find_duplicate`, счётчики активных, позиция в очереди, `runner_online(now)`, `resubmit`): читающие — через `self.db.read(|c| ..)`, пишущие — через `self.db.write`, часы только `self.clock.now_ms()`. Ошибки — `Result<_, String>` на русском. Для отказов с кодом протокола использовать `Reject::new(code, message)` (`core.rs:59-70`), если ошибка уходит в API.

### `crates/server/src/rules.rs` — WR-01

**Аналог:** `Event::Release` (`rules.rs:190-217`). Сейчас при `Release` задача безусловно возвращается в `Queued` (`j.state = JobState::Queued`, строки ~205). Добавить: если `job.cancel_requested != CancelMode::None`, результат `Cancelled` (по образцу того, как это сделано для соседних событий `Finish`/`Cancel`, смотреть ветку `Event::Cancel` в том же файле). Тест — рядом с существующим, `rules.rs:590` строит `Event::Release { runner_id, epoch, counts }`; взять его как шаблон теста «отмена выданной задачи + release = Cancelled».

### `crates/server/src/http.rs` — CR-01 (`body_length`)

**Аналог:** диспетчер и `overloaded` (`http.rs:114-162`, `166-185`). Правило шапки модуля, п. 2: «отказ (401, 413, 503) отправляется до `as_reader`», потому что недочитанное тело дочитывается в `Drop`. В диспетчере (до `tx.send(rq)`) проверить `rq.body_length()` (в обработчиках так читается: `api.rs:259`, `match rq.body_length() { None => return Err(Reject::new(codes::LENGTH_REQUIRED, ..)), ..}`), при превышении потолка ответить 413 из отдельного потока, как `overloaded`:

```rust
// http.rs:152-155 — образец «отказ из отдельного потока, тело не читаем»
} else if busy.fetch_add(1, Ordering::SeqCst) >= workers {
    busy.fetch_sub(1, Ordering::SeqCst);
    std::thread::spawn(move || overloaded(rq));
```

Ответ по образцу `overloaded` (`http.rs:166-185`): `ApiError { error: wire::codes::TOO_LARGE.to_owned(), message: "...", offset: None }`, `Response::from_string(body).with_status_code(413)`, заголовок `Content-Type: application/json`. Тест — в `tests/protocol_http.rs` на сыром TCP (хелперы `read_reply`, `RawReply` из `tests/common/mod.rs`, ~строка 560): отправить `Content-Length: 1000000000000000` без токена и проверить, что сервер жив (следующий запрос отвечает).

### `crates/server/src/config.rs` и `cs2-server.example.json` — ключи бота

**Аналог:** сам `config.rs`. Добавлять ключ в четыре места, как `telegram_bot_token`:

1. поле `Config` (`config.rs:20-33`) и значение в `Default` (`config.rs:36-52`);
2. поле `FileConfig` (`config.rs:92-106`, `#[serde(deny_unknown_fields, default)]`, все `Option`);
3. слияние уровней в `load` (около `config.rs:199-248`, как `file_token` → `c.telegram_bot_token`);
4. `cs2-server.example.json` и вывод `config show`.

Новые ключи: `bot_api_url`, `bot_api_files_root`, `bot_api_results_prefix`, лимиты BOT-11 (`user_max_active` 2, `admin_max_active` 5, `queue_max_active` 10), интервалы статуса. Единицы в имени ключа (`_secs`, `_mib`), как у `lease_ttl_secs`, `max_demo_mib`. Секреты — только `Secret`; `Debug` печатает `***` (`secret.rs:34-38`). Проверку схемы `bot_api_url` (только `http://` loopback) делать при загрузке конфига и тестировать там же, где тесты `config.rs` (`c.telegram_bot_token.is_some()`, строка ~422).

### `crates/server/src/secret.rs` — `Redactor` с `~`-вариантом

**Аналог:** `Redactor::new` (`secret.rs:58-68`) и `mask_bot_tokens` (`129-150`, ищет только `:`).

```rust
// secret.rs:58-68
pub fn new(secrets: &[&Secret]) -> Self {
    let exact = secrets.iter().map(|s| s.expose())
        .filter(|v| v.chars().count() >= MIN_SECRET_LEN)
        .map(str::to_owned).collect();
    Self { exact: Arc::new(exact) }
}
```

Добавить в `exact` для каждого секрета также `value.replace(':', "~")`, если значение содержит `:` (дубликаты не вредят). Тест — в `tests/secrets.rs`: подставной токен `123456789:AAE...` не должен проявляться ни в журнале, ни в базе, ни в сообщениях `FakeTelegram`, ни в ошибке `HttpTg` при закрытом порту, ни в форме `123456789~AAE...`. Стенд уже умеет `TestOptions.bot_token` (`tests/common/mod.rs:44-45`).

### `crates/wire/src/faceit.rs` — строгий разбор ссылки

**Аналог (форма проверки id):** `faceit/src/lib.rs:21-39`.

```rust
// faceit/src/lib.rs:32-39 — is_match_id: повторить правила, но НЕ parse_match_id (режет по / ? # пробел и берёт любой кусок)
fn is_match_id(s: &str) -> bool {
    let Some((prefix, uuid)) = s.split_once('-') else { return false };
    let groups: Vec<&str> = uuid.split('-').collect();
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_digit())
        && groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}
```

Ограничения: `wire` имеет только `serde`/`serde_json` (`tests/deps.rs:73-80` — страж `wire_has_no_workspace_dependencies`), поэтому никаких `regex`/`url`, только разбор строк. Экспортировать модуль в `wire/src/lib.rs` (`pub mod faceit;`) рядом с `pub mod fail` (`lib.rs:147`), `codes` (496), `paths` (515). Функции `room_match_id(&str) -> Option<String>` и `canonical_url(&str) -> String`. Правила строгого разбора и обязательные негативные тесты перечислены в RESEARCH §10. Тесты — в `#[cfg(test)] mod tests` того же файла, названия-предложения.

### `crates/runner/src/exec.rs` — скачивание FACEIT на раннере

**Аналог:** `PipelineExecutor::run` и `unsupported_source` (`exec.rs:197-217`).

```rust
// exec.rs:213-217 — ветка, которую заменяет скачивание
fn run(&self, task: &Task, p: &dyn Progress) -> Result<Outcome, RunError> {
    if task.faceit.is_some() && task.demo.as_os_str().is_empty() {
        return Err(unsupported_source());
    }
    if !task.resume { self.inspect_demo(task, p)?; }
    let outcome = match self.call_job(task, task.resume, p) { ... }?;
    self.verify(outcome, p)
}
```

Заменить `return Err(unsupported_source())` на: перепроверить ссылку `wire::faceit::room_match_id(link)` (при отказе — тот же `RunError` с `FailCode`, по образцу `unsupported_source`, `exec.rs:197-208`: `RecordFailure::new(FailCode::..., RecordState::Precheck, msg, Duration::ZERO)` затем `RunError { message: f.message.clone(), fail: Some(f) }`), скачать через `faceit::Client::new(cfg.faceit_key)`, `demo_urls(match_id)` (индекс 0), `download(url, dir)` (`faceit/src/lib.rs:90`, `103`), положить в `remote/srv{id}.dem(.zst)`, затем обычный путь `inspect_demo` → `call_job`. Прогресс — через `p.stage(...)` / `p.log(..)` и `pipeline::check`, как весь `run`. Тест — в стиле `runner/src/lib.rs:1129-1155` (`fn assignment(faceit: Option<&str>, size: u64) -> Assignment`, тест `faceit_link_without_a_demo_file_is_not_downloaded`): его ожидание придётся обновить. В `runner/Cargo.toml` (сейчас `wire`, `pipeline`, `ureq`...) добавить `faceit = { path = "../faceit" }`; ограничение «не зависит от faceit» относится только к `server` (`deps.rs:6-21`). Это затрагивает `crates/cli/tests/pipeline.rs`: проверить, что он по-прежнему проходит.

### Тесты бота — `crates/server/tests/bot_*.rs`

**Аналоги:** `tests/core_states.rs:36-100` (Core + `ManualClock`, без HTTP) и `tests/common/mod.rs:78-179` (`TestServer`).

Для логики троттлинга/лимитов/дедупа/доставки (`Bot::step` напрямую, без потоков, без сна):

```rust
// core_states.rs:42-67
fn core_with_clock(limits: Limits) -> (Core, ManualClock, TestDir) {
    let dir = TestDir::new();
    let clock = ManualClock::new(T0);
    let core = Core::open(CoreOptions {
        data_dir: dir.0.join("data"), limits, clock: Arc::new(clock.clone()), redactor: Redactor::default(),
    }).unwrap();
    (core, clock, dir)
}
fn submit(core: &Core, dir: &TestDir, player: &str) -> i64 {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let src = dir.0.join(format!("in-{n}.dem"));
    let mut bytes = b"PBDEMS2\0".to_vec();
    bytes.resize(2048, 9);
    std::fs::write(&src, bytes).unwrap();
    let s = Submit { player: player.to_owned(), requested_by: "оператор".to_owned(), orig_name: "m.dem".to_owned(), ..Default::default() };
    core.submit(&src, &s).unwrap().0
}
fn secs(clock: &ManualClock, n: u64) { /* clock.advance(Duration::from_secs(n)) */ }
```

Для сквозных тестов «сервер + бот + FakeTelegram + раннер»: `TestServer::start_with(TestOptions { clock: Some(ManualClock::new(..)), bot_token: Some(..), .. })` (`common/mod.rs:91-116`). Расширить `TestOptions` (добавлением поля, `Default` обновить) и `Config` стенда полями бота, а `FakeTelegram` передавать в `serve` через новый параметр или `serve_with_bot` (не ломая сигнатуру `serve`, которой пользуется весь набор тестов). Фейковый раннер уже есть: `add_runner`, `claim`, `progress`, `release`, `submit` (`common/mod.rs:139-200`) и `fake_demo(size)` (`common/mod.rs:302-310`, магия `PBDEMS2\0`).

Названия тестов — предложения о поведении на английском в snake_case (`submitted_demo_is_claimed_over_http_once`, `expired_lease_requeues_without_a_new_epoch_or_grant`), комментарии и сообщения assert — по-русски. Обязательный тест белого списка: `get_file_calls == 0` для незнакомца с документом, с пересланным документом.

Guard по кодам сбоев (BOT-09): тест по `wire::fail::KNOWN` (`wire/src/lib.rs:188-213`) — для каждого кода есть русская фраза друга без `/`, `\`, `:\`; образец итерации — `for code in fail::KNOWN` (`wire/src/lib.rs:555`).

## Общие паттерны

### Время

**Источник:** `crates/server/src/clock.rs`. Применять ко всему коду бота. Только `Arc<dyn Clock>` из ядра, `now_ms()`, троттлинг статуса `next_edit_ms` в базе. `std::time::Instant` и `thread::sleep` в логике — запрещены (ломают тесты на виртуальном времени).

### Ошибки и русские сообщения

**Источник:** `crates/server/src/lib.rs:21`, `db.rs:17-19`, `users.rs`.

```rust
pub type Result<T> = std::result::Result<T, String>;
pub fn db_err(e: rusqlite::Error) -> String { format!("база данных: {e}") }
```

`unwrap`/`expect` — только в тестах. Пользовательские тексты (в том числе ответы бота) по-русски, идентификаторы по-английски.

### Любой исходящий текст проходит `Redactor`

**Источник:** `crates/server/src/secret.rs:70-94`, `core.rs` (`self.redactor.clean(..)` на каждом тексте раннера и оператора до записи). Применять к: текстам в чат, `log.*`, ошибкам `HttpTg`, нику игрока и имени файла из Telegram. `clean(s, max_chars)` ещё убирает управляющие символы и режет длину (`secret.rs:82-94`). Текст без `parse_mode`, длина не больше 4096. Не логировать `file_path` из `getFile`.

### Журнал

**Источник:** `crates/server/src/log.rs`, использование `lib.rs:80-110`: `log.info(&format!(..))`, `log.warn(..)`; `Log` сам маскирует строки. Тесты читают `MemorySink` (`TestServer.sink`).

### Состояния задач и транзакции

Бот не пишет `jobs.state`; только `Core::*` (автомат — `rules::decide`). Запись в базу только `Db::write` (`BEGIN IMMEDIATE`), чтение `Db::read`; тяжёлое (копирование, хеш 2 ГБ) вне мьютекса (`db.rs:3-4`, `core.rs:263-308` `finish_receive`).

### Зависимости

`crates/server/tests/deps.rs` — сервер не зависит от крейтов воркспейса, кроме `wire`; `wire` — только `serde`/`serde_json`. Оба требования сохраняются: `tg`/`bot` лежат внутри `server`, `wire::faceit` — чистые функции без новых зависимостей.

## Без аналога

| Файл | Роль | Поток данных | Причина |
|---|---|---|---|
| Поддельный HTTP Bot API на `tiny_http` (тесты `HttpTg`) | test-double | request-response | В репозитории нет локального поддельного HTTP-сервера; ближайшее — `http::start` с `Handler` (`http.rs:67-80`) и `TcpListener::bind("127.0.0.1:0")`. Взять `tiny_http::Server::http("127.0.0.1:0")` из зависимостей `server` и отвечать JSON по `Request::url()`, пользуясь RESEARCH «Разбор ответа Bot API». Тайм-аут проверять сервером, который молчит дольше таймаута клиента |
| Чтение только `uncertain`-доставки (`sending → sent / uncertain / failed`) | service | event-driven | Аналога порядка «запись намерения до вызова» нет; ближайшее — `finish_report` в `core.rs:644-807` (идемпотентный итог). Использовать схему Pattern 3 из RESEARCH |
| `sendVideo` по `file:///` и `file_uri()` | utility | transform | Нет; готовая функция в RESEARCH «Формирование `file://` URI» с тестом на `%`, пробел, `#`, `?`, не-ASCII |
| Спайк `telegram-bot-api` | — | — | Ручной, автор; результат в `07-SPIKE.md` |

## Метаданные

**Область поиска аналогов:** `crates/server/src`, `crates/server/tests`, `crates/wire/src`, `crates/host/src/fake.rs`, `crates/runner/src`, `crates/faceit/src`.
**Прочитано файлов:** 18 (`db.rs`, `users.rs`, `secret.rs`, `clock.rs`, `config.rs` частично, `core.rs` по фрагментам, `lib.rs`, `http.rs`, `rules.rs` фрагмент, `tests/common/mod.rs`, `tests/core_states.rs` фрагмент, `tests/deps.rs`, `wire/src/lib.rs` фрагменты, `host/src/fake.rs` фрагменты, `runner/src/exec.rs`, `faceit/src/lib.rs`, `server/Cargo.toml`, `runner/Cargo.toml`).
**Все пути проверены как отслеживаемые** (`git ls-files crates/server crates/wire crates/host crates/runner crates/faceit`).
**Дата извлечения:** 2026-10-11
