# Phase 6: Раннер, защита входа и первый живой прогон - Pattern Map

**Mapped:** 2026-10-11
**Files analyzed:** 24 (новые и изменяемые)
**Analogs found:** 22 / 24
Все аналоги проверены как git-tracked исходники (`crates/...`). Пути-зеркала `.gsd/capabilities` не используются.
Номера строк приблизительны (файлы читались на момент 2026-10-11); перед правкой перечитывать нужный диапазон.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/runner/Cargo.toml` (+ `[workspace].members`) | config | - | `crates/server/Cargo.toml`, `crates/server/tests/deps.rs` | role-match |
| `crates/runner/src/lib.rs` (Loop) | service | event-driven (claim -> run -> report) | `crates/app/src/queue.rs` (поток очереди) | role-match |
| `crates/runner/src/link.rs` (HTTP-клиент) | service | request-response | `crates/faceit/src/lib.rs` (ureq), `crates/wire/src/lib.rs` (типы) | role-match |
| `crates/runner/src/transfer.rs` | service | file-I/O, streaming (chunk/Range) | `crates/server/src/api.rs` (серверная сторона), `crates/server/tests/transfer_http.rs` | role-match |
| `crates/runner/src/state.rs` (`runner_state.json`) | utility | file-I/O | `crates/pipeline/src/record/` `RecordJson`/`Manifest::load` (JSON на диске) | partial |
| `crates/runner/src/tee.rs` | provider | event-driven | `trait Progress` в `crates/pipeline/src/lib.rs`; `cli::record::ConsoleProgress`; `app::jobs::JobReporter` | exact (по трейту) |
| `crates/runner/src/policy.rs` (`PausePolicy`) | utility | transform | `QueuePolicy` в `crates/app/src/queue.rs:195` | exact |
| `crates/runner/src/exec.rs` (Executor) | service | request-response | `Executor` в `crates/app/src/jobs.rs` (~252) | exact |
| `crates/runner/src/preflight.rs` | service | request-response (дочерний процесс) | `pipeline::assemble` (`Host::run` + дедлайн) | role-match |
| `crates/runner/src/uplink.rs`, `backoff.rs` | utility | event-driven | нет | no analog |
| `crates/runner/tests/*.rs` | test | request-response | `crates/server/tests/transfer_http.rs`, `common/mod.rs` | exact |
| `crates/wire/src/lib.rs` (`fail::*`, `KNOWN`) | model | - | `pub mod fail` в `wire/src/lib.rs:146` | exact |
| `crates/pipeline/src/record/failure.rs` (новые `FailCode`) | model | transform | собственные `FailCode`/`classify` | exact |
| `crates/pipeline/src/lib.rs` (`resolve_player`, `plan_player`) | service | transform | `find_player` (lib.rs:233) | exact |
| `crates/pipeline/src/inspect.rs` | service | request-response | `retry_no_data` в `job.rs:448`; `assemble.rs` | role-match |
| `crates/pipeline/src/result.rs` (ffprobe + faststart) | utility | transform | `crates/pipeline/src/record/verify.rs` (`parse_probe`) | exact |
| `crates/demo/src/lib.rs` (`read_demo_limited`, `DemoError`) | utility | file-I/O | `read_demo`/`decompress` (lib.rs:20-40) | exact |
| `crates/host/src/win/` `limit_self_memory` + shim | utility | - | `Job::new` в `host/src/win/job.rs:44` | exact |
| `crates/cs2/src/text.rs` + `Match::build` | utility | transform | `Match::build` (`cs2/src/lib.rs:117`) | role-match |
| `crates/renderer/src/lib.rs` (комментарии, `drawtext textfile`, `+faststart`) | service | transform | собственные `text`/`caption`/`Encoder::args` | exact |
| `crates/cli/src/main.rs`, `crates/cli/src/runner.rs`, `inspect` | controller | request-response | `record.rs` (`record_cmd`, `ConsoleProgress`) | exact |

## Pattern Assignments

### `crates/runner/src/policy.rs` (utility, transform)

**Analog:** `crates/app/src/queue.rs` `QueuePolicy` (стр. 195-260). Чистая логика без часов и ввода-вывода, серия `(код, класс, n)`; две подряд = пауза.

```rust
pub struct QueuePolicy {
    streak: Option<(String, FailClass, u32)>,
}
impl QueuePolicy {
    pub fn after(&mut self, auto_retries_done: u32, f: &Finished) -> Verdict {
        match f {
            Finished::Done => { self.streak = None; Verdict::Next }
            Finished::Failed { code: Some(code), state: Some(state), message, hint, .. } => {
                let class = classify(*code, *state);
                match class {
                    FailClass::Environment | FailClass::Launch => self.count(code.as_str(), class, message, hint),
                    FailClass::Other => { self.streak = None; Verdict::Next }
                }
            }
            ...
fn count(&mut self, code: &str, class: FailClass, ...) -> Verdict {
    let n = match &self.streak {
        Some((c, k, n)) if c == code && *k == class => n + 1,
        _ => 1,
    };
```
Импорты: `pipeline::{FailCode, RecordState, record::failure::{FailClass, classify}}`. Отличие раннера (D-02): Environment/Launch -> `release`, а не `finish(Failed)`; `BAD_DEMO`/`DEMO_TOO_LARGE`/`PLAYER_*`/`NO_HIGHLIGHTS` (класс Other) серию не двигают. Тесты называть предложениями; чистые тесты без сети.

---

### `crates/runner/src/exec.rs` + `tee.rs` (service/provider)

**Analog:** `app::jobs` (Executor с `run_job` на общем хосте, `recover_after_crash`) и `trait Progress` (`pipeline/src/lib.rs:72-95`).

```rust
pub trait Progress: Sync {
    fn stage(&self, s: Stage);
    fn log(&self, msg: &str);
    fn cancelled(&self) -> bool { false }
    fn force_cancelled(&self) -> bool { false }
    fn event(&self, e: &record::Event) { self.log(&e.to_string()) }
    fn artifact(&self, _a: &Artifact) {}
}
```
`Tee` держит только атомики/`Mutex` (трейт `Sync`): `cancelled() = inner.cancelled() || shared.cancel >= Soft`, `force_cancelled()` = `Force`, `artifact(OutDir)` -> запись в state. Отмену проверять до `release` (Pitfall 3).

Восстановление после падения (образец `crates/app/src/jobs.rs:240-250`, порядок обязателен):
```rust
let mut notes = pipeline::gamedir::kill_orphans(host, &dir);
notes.extend(pipeline::gamedir::recover(Path::new(cs2_dir), &dir));
```
Запуск: `run_job(&*pipeline::host::default_host(), &spec, &progress)` как в `crates/cli/src/record.rs:265`; результат `Err(RunError{fail: Some(RecordFailure{code,state,..}), ..})`, отмена по `e.message == pipeline::CANCELLED` (`print_failure`, `record.rs:56-67`). Паники `run_job` перехватывать `catch_unwind(AssertUnwindSafe(..))` как в `queue.rs` (импорты `std::panic::{AssertUnwindSafe, catch_unwind}`).

Resume (D-01): `JobSpec { resume: true, out_dir: Some(..), .. }`; `resume` (`job.rs:638`) при `RETRY_NO_DATA` -> начать заново через `retry_no_data(..)`-код. Pitfall 4: непроверенные дубли переснимаются все.

---

### `crates/runner/src/link.rs` (service, request-response)

**Analog:** `crates/faceit/src/lib.rs` (блокирующий `ureq`, приватный `type Result<T> = ...`), типы и константы из `crates/wire/src/lib.rs` (`PROTOCOL_VERSION`, `CHUNK_SIZE`, `CLAIM`, `HELLO`, `job(id, action)`, заголовки `EPOCH`, `UPLOAD_OFFSET`, `RETRY_AFTER`). Конфигурация агента по Pitfall 7: `http_status_as_error(false)`, `max_redirects(0)`, `timeout_recv_response >= poll_secs + 15`, токен только в `Authorization`, `Debug` токена печатает `***` (образец: `JobConfig::fmt` в `pipeline`). Ошибки `Result<T, String>` по-русски.

---

### `crates/runner/src/transfer.rs` (service, streaming)

**Analog (контракт):** серверные обработчики `crates/server/src/api.rs:231-408` (HEAD/PUT результата, HEAD/GET демки) и тесты `crates/server/tests/transfer_http.rs`. Правила: `HEAD` -> `Upload-Offset`; `PUT` по `CHUNK_SIZE` с `X-Epoch`, `Upload-Offset`; `409 offset_mismatch` -> брать смещение из ответа; `409 upload_busy` -> `Retry-After: 1`; `422 sha256_mismatch` -> один повтор с нуля, второй = окончательный отказ (D-03); `GET` демки с `Range: bytes=N-`, SHA-256 считать потоково (`sha2`). Имя файла демки: `remote/srv<job_id>.dem`, не `orig_name`.

---

### `crates/runner/src/state.rs` (utility, file-I/O)

**Analog:** JSON на диске задачи (`RecordJson::load`, `Manifest::load` в `crates/pipeline/src/job.rs`/`manifest.rs`; serde-derive `Serialize, Deserialize`, версия в файле). Запись атомарная: temp -> `sync_all` -> `std::fs::rename` (на Windows заменяет файл, проверено спайком). Один объект `{version, server_job_id, epoch, stage, demo_path, demo_sha256, out_dir, local_job_id, attempt, result, pending_outcome, sha_retries, last_good}`. Каталог `%LOCALAPPDATA%\cs2cinema\runner`; в приложении путь данных берётся через `data_dir()` в `crates/app/src/settings.rs` (для раннера свой код, `app` не зависит). `seq = max(prev+1, unix_ms())` (Pitfall 2).

---

### `crates/runner/tests/*.rs` (test)

**Analog:** `crates/server/tests/transfer_http.rs` + `common/mod.rs` (`TestServer::start_with`, `submit`, `add_runner`, `fake_demo`, `ManualClock`). Раннер берёт `server` как `[dev-dependencies]` (только для тестов, интеграция с `server::serve`, `crates/server/src/lib.rs:68`).

```rust
mod common; // у раннера свой tests/common
fn clocked() -> TestServer {
    TestServer::start_with(TestOptions { clock: Some(ManualClock::new(T0)), ..Default::default() })
}
```
Дополнить `WORKSPACE_CRATES` в `crates/server/tests/deps.rs` (стр. 7-21) именем `"runner"` (он проверяет, что `server` зависит только от `wire`):
```rust
const WORKSPACE_CRATES: &[&str] = &["host","pipeline","demo","store","cs2","positions","geometry","app","cli","renderer","director","highlights","faceit"];
```
Сам `server/Cargo.toml` менять нельзя: `server` не получает `pipeline`.

---

### `crates/wire/src/lib.rs` (model)

**Analog:** `pub mod fail` (стр. 146-151).
```rust
pub mod fail {
    pub const TOO_MANY_GRANTS: &str = "TOO_MANY_GRANTS";
    pub const RECEIVE_FAILED: &str = "RECEIVE_FAILED";
}
```
Добавить `PLAYER_NOT_FOUND`, `PLAYER_AMBIGUOUS`, `NO_HIGHLIGHTS`, `BAD_DEMO`, `DEMO_TOO_LARGE`, `RESULT_INVALID` (+ `JOB_ERROR` при необходимости) и `KNOWN: &[&str]`. Тест-страж в `runner` или `pipeline`: все `FailCode::as_str()` из нового `FailCode::ALL` входят в `wire::fail::KNOWN`. Если добавляется `Phase::Precheck`, обновить `Phase::ALL` и тест `wire` (решение Open Question 3).

---

### `crates/pipeline/src/record/failure.rs` (model)

**Analog:** сам файл. `enum FailCode` (стр. 16-36), `as_str` (`=> "GAME_NOT_STARTED"` и т. д.), исчерпывающие `hint()` и `classify` (стр. ~531-547, без ветки `_`). Новые коды идут в ветку `FailClass::Other`:
```rust
FailCode::ScriptNotRun | FailCode::TakeTimeout | FailCode::TakeInvalid
    | FailCode::RetryNoData | FailCode::Cancelled => FailClass::Other,
```
Тесты в стиле `codes_have_stable_names_and_hints`.

---

### `crates/pipeline/src/lib.rs` (`resolve_player`, `find_player`)

**Analog:** текущая `find_player` (стр. 233-247): SteamID -> точное имя -> подстрока `.to_lowercase()`, `.find(..)` (первое совпадение: это и надо заменить).
```rust
pub fn find_player(m: &Match, steam_id: Option<u64>, nick: &str) -> Option<PlayerId> {
    if let Some(p) = steam_id.filter(|&s| s != 0).and_then(|s| m.players.iter().find(|p| p.xuid == s)) { return Some(p.id); }
    let want = nick.trim().to_lowercase();
    ...
```
Новое: `resolve_player(&Match, Option<u64>, &str) -> Result<PlayerId, PlayerError>` (ярусы: xuid -> точное -> без регистра/пробелов -> подстрока; 1 = игрок, >1 = `PlayerAmbiguous`, 0 = дальше), `find_player = resolve_player(..).ok()`. Вызов `app.rs:547` не ломается. `PlayerError` -> `RunError { fail: Some(RecordFailure::new(code, RecordState::Precheck, msg, Duration::ZERO)) }` по образцу `retry_no_data`:
```rust
pub fn retry_no_data(reason: &str) -> RunError {
    let f = RecordFailure::new(FailCode::RetryNoData, RecordState::Precheck, failure::msg_retry_no_data(reason), Duration::ZERO);
    RunError { message: f.message.clone(), fail: Some(f) }
}
```
Сообщения по-русски, кандидаты «имя (SteamID64)», не более 10.

---

### `crates/pipeline/src/result.rs` (utility, transform)

**Analog:** `crates/pipeline/src/record/verify.rs` `parse_probe` (стр. 84-121): `#[derive(Deserialize, Default)] #[serde(default)]` для недоверенного JSON, возврат `Result<_, String>`, обрезка вывода `one_line(&head)`. ffprobe запускать через `Host::run` с дедлайном (как `pipeline::assemble`/`record::verify`), аргументы по RESEARCH (stream: codec_name, pix_fmt). Разбор боксов MP4 (`moov` раньше `mdat`) пишется самим (нет аналога).

---

### `crates/demo/src/lib.rs` (`read_demo_limited`)

**Analog:** `read_demo`/`decompress` (стр. 20-40): определение формата по сигнатуре `[0x28,0xB5,0x2F,0xFD]` (zstd) и `[0x1F,0x8B]` (gzip). Заменить `zstd::stream::decode_all(contents.as_slice())` на `zstd::stream::Decoder` + `Read::take(limit + 1)`; `n > limit` -> `DemoError::TooLarge`. Магия `PBDEMS2\0` (`data.starts_with(b"PBDEMS2\0")`, см. `crates/positions/tests/real_demo.rs`) -> `BadDemo`. `read_demo` оставить (обратная совместимость). Проверка `snap::raw::decompress_len` <= 128 МиБ в `command.rs` `Command::decode_payload`.

---

### `crates/host/` `limit_self_memory` (utility)

**Analog:** `Job::new` в `crates/host/src/win/job.rs:44-76` (`CreateJobObjectW`, `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` через `std::mem::zeroed()`, `SetInformationJobObject`, `// SAFETY:` комментарии, `HostError::JobAssign(format!("...: {}", last_error()))`).
```rust
let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
SetInformationJobObject(handle, JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)
```
Добавить флаги `JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, `ProcessMemoryLimit`, `AssignProcessToJobObject(job, GetCurrentProcess())`, handle не закрывать. Вне Windows заглушка в `unsupported.rs` (`HostError::Unsupported`). Все Win32 только в `host`.

---

### `crates/cs2/src/text.rs` + `Match::build` (utility)

**Analog:** `Match::build` в `crates/cs2/src/lib.rs:117` (граница доверия: имена игроков и `map` проходят `clean_text` один раз). Правила: убрать управляющие, переводы строки, bidi/`\u{2028}`, обрезать по длине; ~20 явных диапазонов + `char::is_control/is_whitespace` (без новой зависимости). Тесты на таблицу враждебных строк из RESEARCH.

---

### `crates/renderer/src/lib.rs` (service, transform)

**Analog:** сам файл. Три правки:
1. Комментарии cfg (стр. ~170 и ~207): убрать `t.map` и `shot.title` из `writeln!` (`"// Шот {} -> take{i:04} (score {:.0}, {:?})"`).
2. `caption()` (стр. 482) сейчас: `drawtext=fontfile='..':text='{}':expansion=none:{style}` с `text()` (стр. 468). Заменить на `textfile='cap_NNN.txt'` и класть `("cap_001.txt", text)` в `Output.files` (механизм как для остальных файлов `Output::write_to`).
3. `Encoder::args` (стр. 67): добавить `-movflags +faststart` в обе ветки перед `highlights.mp4`:
```rust
Encoder::X264 => "-c:v libx264 -crf 18 -preset medium -pix_fmt yuv420p -c:a aac -b:a 192k highlights.mp4",
```
Тесты в `renderer` называть предложениями; проверить, что `crates/cli/tests/pipeline.rs` остаётся зелёным (не ломать формат `plan.json`/cfg).

---

### `crates/cli/src/main.rs`, `runner.rs`, `inspect` (controller)

**Analog:** диспетчер в `main.rs:128-145`:
```rust
match env::args().nth(1).as_deref() {
    Some("assemble") => return assemble_cmd(),
    Some("record") => return record::record_cmd(),
    Some("retry") => return record::retry_cmd(),
    Some("acceptance") => { return match env::args().nth(2).as_deref() { Some("check") => acceptance::check_cmd(), ... } }
    _ => {}
}
```
Добавить `Some("runner") => return runner::runner_cmd()`, скрытый `Some("inspect") => return inspect::inspect_cmd()` (в начале: `host::limit_self_memory`, `SetErrorMode`, сторожевой поток времени; одна JSON-строка в stdout). Ошибки: `eprintln!("ошибка: {e}")`, `ExitCode`. Ctrl+C через `ctrlc` (feature `termination`, уже в `server`). Консольный `Progress`: `ConsoleProgress` (`record.rs:44-53`). USAGE дополнить по-русски.

## Shared Patterns

### Результат и ошибки
**Source:** `crates/pipeline/src/lib.rs` (`type Result<T> = std::result::Result<T, String>`, `CANCELLED`, `check(p)?`); `RunError { message, fail: Option<RecordFailure> }`.
**Apply to:** `runner`, `inspect`, `result`. Отмена сервером не должна превращаться в `finish(Failed)`: `e.message == CANCELLED`. Остановка раннера Ctrl+C не равна `finish(Cancelled)`.

### Классификация сбоя
**Source:** `pipeline::record::failure::classify(code, state) -> FailClass` (исчерпывающий `match`).
**Apply to:** `PausePolicy`, ветка `release` против `finish(Failed)`.

### Граница с ОС
**Source:** `host::Host` (`run(&CmdSpec, &cancel, &mut on_line)`, `CmdSpec { program, args, cwd, deadline }`), `host::fake::FakeHost` в тестах.
**Apply to:** preflight (`cs2-cli inspect`), ffprobe, любые процессы. В `runner`/`cli` нет `windows-sys` и `std::process::Command` для долгих задач.

### Недоверенный JSON
**Source:** `parse_probe` (`verify.rs:96`) `#[serde(default)]`, обрезка ответа до 120 символов в сообщении.
**Apply to:** разбор вывода `inspect` (родитель очищает строки повторно), ответов сервера.

### Секреты
**Source:** `JobConfig::fmt` (ручной `Debug`, ключ не печатается).
**Apply to:** тип токена раннера, `runner.log`.

### Язык и стиль
Идентификаторы по-английски, доки/сообщения по-русски, `//!` в каждом `lib.rs`, тесты-предложения, edition 2024, `std::thread` без tokio.

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `crates/runner/src/uplink.rs` (heartbeat-поток) | service | event-driven | Фоновых потоков с общим состоянием и ответом сервера в `Progress` нет; ближайшее: потоки `queue.rs` + `Msg` каналы (`mpsc`) |
| `crates/runner/src/backoff.rs` | utility | transform | Нет готового backoff; писать чистую функцию с тестами |
| Разбор боксов MP4 (`is_faststart`) | utility | transform | Нет в кодовой базе; брать 25-строчный набросок из RESEARCH |
| Замок единственной копии раннера (`runner.lock`, `share_mode(0)`) | utility | file-I/O | Нет аналога (Windows-only `OpenOptions` ext) |

## Metadata

**Analog search scope:** `crates/{pipeline,app,cli,wire,server,host,demo,cs2,renderer}`
**Files scanned:** ~25 (чтение выборочно: диапазоны, не целиком)
**Pattern extraction date:** 2026-10-11
