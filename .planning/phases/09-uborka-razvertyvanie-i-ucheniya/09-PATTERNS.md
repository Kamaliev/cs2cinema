# Phase 9: Уборка, развёртывание и учения - Pattern Map

**Mapped:** 2026-10-12
**Files analyzed:** 22 (новые и изменяемые)
**Analogs found:** 17 / 22 (5 без аналога: документы и скрипты, брать из RESEARCH.md)

Все названные аналоги проверены как отслеживаемые git-файлы (`crates/...`, `.github/workflows/ci.yml`). Зеркал `.gsd/capabilities` не используется.

## File Classification

| Новый / изменяемый файл | Роль | Поток данных | Ближайший аналог | Качество |
|---|---|---|---|---|
| `crates/server/src/clean.rs` (+ `Core::clean`) | service | batch (DB + fs) | `Core::sweep` в `crates/server/src/core.rs:1191`, `Blobs::discard_parts` `blobs.rs:311` | role-match |
| `crates/server/src/lib.rs` (`spawn_cleaner`) | поток | event-driven (таймер) | `spawn_sweeper` `lib.rs:178-210` | exact |
| `crates/server/src/config.rs` (ключи `retention_*`) | config | transform | ключ `startup_grace_secs` (`config.rs:31,64,120,159,261,343`) | exact |
| `crates/server/src/cli.rs` (`backup`, `clean --dry-run`) | command | request-response | ветки `execute` (`cli.rs:230+`), `show_config` `cli.rs:182` | exact |
| `crates/server/src/core.rs` (правка WR-02, строка ~684) | service | CRUD | сам запрос `poisoned` + `expire_leases` | exact |
| `crates/server/tests/cleanup.rs` | test | batch | `crates/server/tests/core_states.rs`, `core_bot.rs:96` (`manual_server`) | role-match |
| `crates/server/tests/lease_http.rs` (тест WR-02) | test | request-response | `core_states.rs:234` `job_granted_max_times_fails_with_too_many_grants` | exact |
| `crates/server/tests/operator.rs` (тест `backup`) | test | file-I/O | тот же файл: `bin()`, `run()`, `test_dir()` | exact |
| `crates/runner/src/sweep.rs` | service | batch (fs) | `Runner::remove_finished`/`remove_files` `runner/src/lib.rs:782+` | role-match |
| вызов из `Runner::tick` (простой) | service | event-driven | `Runner::tick` `lib.rs:390` (ветка `Tick::Idle`) | role-match |
| `crates/runner/tests/sweep.rs` | test | batch | `crates/runner/tests/flow.rs`, `common/mod.rs` (`TestDir`) | role-match |
| `crates/runner/tests/chain.rs` | test | event-driven e2e | `crates/runner/tests/faceit_flow.rs` + `crates/server/tests/bot_e2e.rs` | exact |
| `crates/runner/tests/common/mod.rs` (`Stand::start_with_bot`) | test-фикстура | request-response | `Stand::start_with` (`mod.rs:44-95`) | exact |
| `.github/workflows/ci.yml` (job `chain`) | config | batch | job `test` в том же файле | exact |
| `docs/deploy/*` (README, compose, юнит, таймер, Caddyfile, json) | документ/config | — | `README.md`, `crates/server/cs2-server.example.json` | нет аналога (частично) |
| `docs/drills/*` (протоколы, `collect-*`, `scan-token.sh`) | документ/скрипт | — | `install.ps1` (стиль скриптов) | нет аналога |
| `.planning/phases/09-.../evidence/NN-*` | данные | — | — | нет аналога |

## Pattern Assignments

### `crates/server/src/lib.rs` — `spawn_cleaner` (поток, таймер)

**Аналог:** `spawn_sweeper`, `crates/server/src/lib.rs:177-210`. Копировать целиком: сигнатуру `(core, log, stop: Arc<AtomicBool>) -> JoinHandle<()>`, ожидание шагами по 200 мс (остановка не ждёт интервала), `match core.clean(..)` с `log.info`/`log.warn`.

```rust
fn spawn_sweeper(core: core::Core, log: log::Log, stop: std::sync::Arc<std::sync::atomic::AtomicBool>)
    -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        'outer: loop {
            // Шаги по 200 мс: остановка не ждёт целых 5 секунд.
            for _ in 0..25 {
                if stop.load(Ordering::SeqCst) { break 'outer; }
                std::thread::sleep(Duration::from_millis(200));
            }
            match core.sweep() { Ok(s) => { /* log.info(...) */ } Err(e) => log.warn(&format!("sweep: {e}")) }
        }
    })
}
```
Различие: интервал минуты (N шагов по 200 мс), хэндл кладётся в `Running` рядом с `sweeper` (`lib.rs:167-173`) и останавливается тем же флагом. Журнал только числами и `#id`, без путей.

### `crates/server/src/clean.rs` и `Core::clean` (batch, двухшаговое удаление)

**Аналог:** `Core::sweep` (`core.rs:1191-1205`): время из `self.clock.now_ms()`, запись в `self.db.write(|tx| ...)`, файловые операции ПОСЛЕ выхода из транзакции.

```rust
pub fn sweep(&self) -> Result<Sweep> {
    let now = self.clock.now_ms();
    let out = self.db.write(|tx| {
        let mut out = expire_leases(tx, now, &self.limits)?;
        out.receive_failed = fail_stale_receiving(tx, now, &self.limits)?;
        Ok::<Sweep, String>(out)
    })?;
    for id in &out.cancelled { let _ = self.blobs.discard_parts(*id); }
    ...
}
```
**Пометка строки до удаления файла:** `files.deleted_at INTEGER` (`db.rs:83`); читатели уже фильтруют `deleted_at IS NULL`:
```rust
// core.rs:1400-1404
"SELECT rel_path, size, sha256 FROM files WHERE job_id = ?1 AND kind = 'result' AND deleted_at IS NULL ORDER BY id DESC LIMIT 1"
```
Шаблон: в `db.write` выбрать строки по правилам таблицы RESEARCH (Pattern 2) и `UPDATE files SET deleted_at = ?now`; затем `fs::remove_file` и пустую папку; ошибка файла не откатывает метку (сирота подберётся следующим проходом).

**Чистка `.part`:** `Blobs::discard_parts` (`blobs.rs:311-327`), обход `read_dir`, `extension() == "part"`, `NotFound => Ok(0)`. Расширить вариантом «по mtime старше `part_idle_ms`» и учётом `Blobs::lock_upload` (`blobs.rs:154`).

**Папка bot-api:** копировать проверку `safe_local_path`, `crates/server/src/bot/ingest.rs:466-500`: `config::is_abs`, `strip_prefix`, запрет компонентов не `Normal`, `symlink_metadata().file_type().is_file()`, `canonicalize` + `starts_with(root)`. Не удалять каталоги, только обычные файлы старше порога.

Конфигурируемость: Retention по образцу `Limits` (`config.rs:120` `startup_grace_ms: self.startup_grace_secs as i64 * 1000`).

### `crates/server/src/config.rs` — ключи `retention_*`

**Аналог:** `startup_grace_secs`: пять мест (`Config` поле `:31`, `Default` `:64`, перевод в `Limits` `:120`, `FileConfig` `Option<u64>` `:159`, слияние файла `:261-263`, env `:343-345` через `env_num(env, "CS2_SERVER_STARTUP_GRACE_SECS")`), плюс `show_config` `cli.rs:195`, список ключей в справке `cli.rs:51`, `ENV_KEYS` в `crates/server/tests/operator.rs:9-33` и `crates/server/cs2-server.example.json`. Файл читается с `deny_unknown_fields`, поэтому каждый новый ключ добавляется во все места сразу.

### `crates/server/src/cli.rs` — `backup`, `clean --dry-run`

**Аналог:** match по `a.positional` в `execute` (`cli.rs:230-262`): `["serve"]`, `["submit", file]` и т. п.; `open_core(&a, env, redactor)?` для не-serve команд; ошибки `CliError::Usage/Run`. `backup {папка}` = `VACUUM INTO` через `core.db()`; не перезаписывать существующее, оставлять N последних. Печать результата `println!`, как у остальных.

### `crates/server/src/core.rs` — WR-02

**Аналог/место правки:** `core.rs:683-691` (запрос `poisoned`). Сейчас `WHERE state = 'queued' AND grants >= ?1 ORDER BY id`. Правка по RESEARCH Pattern 3: `grants >= max AND (lease_lost = 0 OR updated_at <= now - (lease_ttl + startup_grace))`; параметры `now` и окно берутся из `self.limits`. Тест: `core_states.rs:234`.

### `crates/server/tests/cleanup.rs` (batch, ManualClock)

**Аналоги:** `core_bot.rs:88-100` (`use server::clock::ManualClock; fn manual_server() -> (TestServer, ManualClock)` с `T0`), `core_states.rs:3-7` (свой `TestDir`, `Core` напрямую), общий стенд `crates/server/tests/common/mod.rs` (`TestDir`, `TestServer`, `TestOptions`, `result_bytes`, `sha256_hex`, `wait_until`). Паттерн: создать задачу и файлы, `clock.advance(...)`, `std::fs::File::set_modified` для mtime файлов, `core.clean(...)`, проверка файлов и ответов `/retry` и `/resend` на понятный текст без путей. Имена тестов предложениями. `File::set_modified` в репозитории нигде ещё не использован (grep по `crates/server/src` и `runner/src` пуст), это новое применение стандартной библиотеки.

### `crates/server/tests/operator.rs` (тест `backup`)

**Аналог:** этот же файл: `bin()` (`:38`) с очисткой `ENV_KEYS`, `test_dir(tag)` (`:46`), `fake_demo(path, size)` (`:54`), `run(&[&str]) -> (i32, String, String)` (`:64`), `data_arg(d)` (`:159`). Образец теста `submitted_demo_is_queued_and_seen_by_a_new_process` (`:74`). Если добавляется ключ в `ENV_KEYS`, обновить список.

### `crates/runner/src/sweep.rs` (batch, fs) и вызов из `tick`

**Аналог:** `Runner::remove_finished` и `remove_files` (`runner/src/lib.rs:776-800`): удалять только внутри `self.cfg.dir`, не саму `cfg.dir`.
```rust
fn remove_finished(&self, job: &HeldJob) {
    self.remove_files(job);
    if let Some(dir) = &job.out_dir
        && dir.starts_with(&self.cfg.dir)
        && dir != &self.cfg.dir
    {
        let _ = fs::remove_dir_all(dir);
    }
}
```
Новое: обход `cfg.dir/out/srv*` и `cfg.dir/remote/srv*` (имена `srv{id}`, `srv{id}.dem`, `srv{id}.dem.part`, `srv{id}.faceit`), пропуск номера из `self.state.job` (удерживаемая задача), удаление по mtime старше порога. Вызов: в `Runner::tick` (`lib.rs:390`) в ветке простоя (`Tick::Idle`, когда `self.state.job` пусто), чтобы не мешать записи. `drop_job` (`lib.rs:741`) не трогает `out/srv{id}` (по RESEARCH остаётся уликой), поэтому эти папки убирает именно sweep по возрасту. Журналирование: `self.log.line(&format!("..."))`, как в `drop_job`. Ротация `runner.log`: файл открывается в `Log::open(&cfg.dir.join("runner.log"), cfg.token.secret())` (`lib.rs:347`), значения токена заменяются на `***`.

### `crates/runner/tests/chain.rs` и `Stand::start_with_bot`

**Аналог 1 (раннерная сторона, копировать почти дословно):** `crates/runner/tests/faceit_flow.rs:96-190` — `Stand::start()`, `Bot::new(core.clone(), Arc::new(tg.clone()), log, BotSettings::new(&stand.dir.path.join("data"), tg.files_dir()))`, `PipelineExecutor::new(Arc::new(host), job_config(&dir), InspectSettings{..}).with_fetch_fn(..).with_job_fn(..)`, `Runner::new(stand.runner_config(), Arc::new(exec))`, `runner.tick()` → `Tick::Settled { job_id, state: JobState::Done }`, затем цикл `bot.step()` до появления `tg.videos()`.
Константы для копирования: `OK_REPLY`, `GOOD_PROBE` (`faceit_flow.rs:19-22`), помощники `mp4_box`/`mp4`/`probing_host`/`job_config` (`:37-78`).
Импорты:
```rust
use pipeline::host::fake::{FakeHost, Scenario};
use runner::{InspectSettings, PipelineExecutor, Runner, Tick};
use server::bot::{Bot, BotSettings};
use server::tg::fake::FakeTelegram;
use wire::JobState;
```
**Аналог 2 (сообщение от друга):** `crates/server/tests/bot_e2e.rs:16-60`: `tg.put_document("file-1","uniq-1",&bytes)`, `tg.document_update(FRIEND, FRIEND, "file-1","uniq-1","match.dem", len, Some("s1mple"))`, `tg.push_update(upd)`, `server::users::add_user(core.db(), "вася", Some(FRIEND), false, now)`, `common::fake_demo(4096)`.
Выбор: бот в `chain.rs` — либо ручной `bot.step()` (как faceit_flow, детерминированно), либо потоки `serve_with_bot` (как bot_e2e) с `wait_until`. Для CI надёжнее `bot.step()`.
Сценарий сбоя: `BAD_DEMO_REPLY` из `crates/runner/tests/exec.rs`; проверка фразы другу и отчёта админу как в `crates/server/tests/bot_delivery.rs:505-519` (`fail_code == "TOO_MANY_GRANTS"`; друг не видит кода, админ видит).
`Stand::start_with_bot`: расширить `Stand::start_with` (`common/mod.rs:61-86`), который строит `ServerConfig { bind: "127.0.0.1:0", data_dir, threads: 4, longpoll_secs: 1, ..Default::default() }` и вызывает `server::serve(&cfg, clock, Arc::new(MemorySink::default()))`; добавить параметры токена бота и адреса bot-api под `FakeTelegram`, как делает `TestServer::start_with(TestOptions{ tg: Some(tg), clock, .. })` в `server/tests/common/mod.rs`. Ограничение: `crates/server/tests/deps.rs` запрещает серверу зависеть от `runner` (даже dev), поэтому тест лежит в `runner/tests`.

### `.github/workflows/ci.yml` — job `chain`

**Аналог:** job `test` (`ci.yml:12-27`): `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `run: cargo test --release ...`. Добавить новый job `chain` (ubuntu-latest, `timeout-minutes`, `concurrency`) по заготовке из RESEARCH «Ворота CI». Артефакт `cs2-server-linux-x64` уже выгружается в job `test` (`if: runner.os == 'Linux'`, `if-no-files-found: error`).

## Shared Patterns

### Уборка только внутри корня, по `id`
**Источник:** `ingest.rs:466-500` (`safe_local_path`) и `runner/src/lib.rs:782-790` (`starts_with(&self.cfg.dir)` + `!= cfg.dir`).
**Применять к:** `clean.rs`, `runner/src/sweep.rs`.

### Время через `Clock`, не `SystemTime`
**Источник:** `Core::sweep` (`self.clock.now_ms()`), `server::clock::{Clock, ManualClock, SystemClock}`; в тестах `ManualClock::new(T0)` и `clock.advance(Duration)`.
**Применять к:** `Core::clean`, все тесты уборщика.

### Транзакция без файловых операций внутри
**Источник:** `Core::sweep` (шапка `db.rs`: мьютекс базы не держать во время файлов).
**Применять к:** `Core::clean`.

### Ошибки и журнал
`Result<T, String>` (`Core::sweep -> Result<Sweep>`), журнал по-русски, без путей и токенов (`log.info(&format!("... #{id} ..."))`; `Log::new(sink, core.redactor().clone())` в тестах).

### Стенд тестов
`TestDir` с `Drop` (`runner/tests/common/mod.rs:22-41`, `server/tests/common/mod.rs`); `ManualClock` + `Stand::start_manual()`; `FakeHost` с `with_job_fn` для подмены записи; ничего не требует CS2.

### Конфигурация сервера (пять мест + справка + пример)
См. `startup_grace_secs` выше; новые ключи следуют тому же маршруту.

### Миграции
Только добавление в конец `MIGRATIONS` (`db.rs`); новых колонок не нужно (`files.deleted_at`, `jobs.finished_at` уже есть); при необходимости шаг 4 только `CREATE INDEX`.

## No Analog Found

| Файл | Роль | Поток | Причина |
|---|---|---|---|
| `docs/deploy/README.md`, `docker-compose.yml`, `cs2-server.service`, `cs2-server-backup.{service,timer}`, `Caddyfile` | документ/config | — | В репозитории нет Linux-развёртывания; брать заготовки из RESEARCH.md («Code Examples»). Пример JSON настроек: `crates/server/cs2-server.example.json` (дополнить ключами сроков). Стиль русской документации: `README.md` |
| `docs/drills/NN-*.md`, `collect-server.sh`, `collect-runner.ps1`, `scan-token.sh` | документ/скрипт | — | Скриптов учений нет; рецепт `grep -rFa -f patterns.txt` из RESEARCH.md. Стиль PowerShell: `install.ps1` |
| `.planning/phases/09-.../evidence/NN-*` | улики | — | Новая папка; каталоги заранее не создавать |
| Тест `clean` на свободное место / `df` | — | — | Вне OPS-01 (Open Question 6) |
| `File::set_modified` в тестах | — | — | Нет примера в коде; использовать `std::fs::File::options().write(true).open(p)?.set_modified(t)` (std ≥ 1.75) |

## Metadata

**Analog search scope:** `crates/server/{src,tests}`, `crates/runner/{src,tests}`, `crates/host/src/fake.rs`, `.github/workflows/ci.yml`
**Files scanned:** около 25
**Pattern extraction date:** 2026-10-12
