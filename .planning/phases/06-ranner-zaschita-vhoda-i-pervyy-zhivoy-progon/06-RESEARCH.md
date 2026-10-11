# Phase 6: Раннер, защита входа и первый живой прогон - Research

**Researched:** 2026-10-11
**Domain:** Rust workspace (std::thread, ureq, Windows Job Object): pull-раннер к `cs2-server`, защита от враждебных демок и ников, строгий выбор игрока, проверка результата ffprobe
**Confidence:** HIGH по коду и протоколу (прочитан в этой сессии), HIGH по спайку Job Object (запущен на машине автора), MEDIUM по числовым лимитам (нет реальной демки для калибровки)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Перезапуск раннера посреди задачи (RUN-02)**
- **D-01:** Раннер хранит в файле состояния (запись атомарная) `job_id`, `epoch`, папку задачи и фазу. После перезапуска он делает `hello`, берёт ту же задачу по эпохе (на сервере это обратимая потеря аренды из фазы 5) и вызывает `pipeline::job::resume`: проверенные дубли не переснимаются. Если `resume` невозможен (`RETRY_NO_DATA`), задача начинается заново. — **Reversibility:** costly — формат файла состояния и связка с `hello`/эпохой станут контрактом раннера, который позже использует приложение (фаза 8).

**Сбои класса «окружение» (RUN-04)**
- **D-02:** Сбой класса `Environment` (и `Launch`, как их классифицирует `pipeline::record::failure::classify`) раннер возвращает серверу через `release`, а не через `finish(failed)`. Бесконечный цикл ограничивают две вещи: раннер сам встаёт на паузу при двух одинаковых сбоях подряд (как `QueuePolicy` в приложении), а сервер считает выдачи и после `max_grants` отмечает задачу `TOO_MANY_GRANTS` (фаза 5). Новых счётчиков и правок протокола на сервере нет.

**Результат в outbox (RUN-02)**
- **D-03:** Если сервер окончательно не принимает результат (задача отменена, эпоха устарела, `422 sha256_mismatch` после повторной загрузки с нуля), файл удаляется из outbox, в журнале раннера остаётся запись с причиной. Временные ошибки сети докачиваются бесконечно, пока сервер не подтвердит размер и SHA-256. — **Reversibility:** reversible — политику хранения можно сменить настройкой.

**Уже принято раньше (из research/ARCHITECTURE.md и фазы 5, не переобсуждать)**
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

### Deferred Ideas (OUT OF SCOPE)
- Дефекты сервера из ревью фазы 5 (CR-01 — падение от огромного `Content-Length`, WR-01, WR-02) — отдельный gap-план фазы 5 до подключения раннера по сети; в эту фазу не входят.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| RUN-01 | `cs2-cli runner` подключается сам, держит heartbeat, берёт задачу, скачивает демку, выполняет `run_job`, загружает результат | Протокол `wire` прочитан целиком (раздел «Протокол сервера»); цикл Idle→Fetch→Preflight→Execute→Verify→Upload→Report; `Link` на `ureq` 3.4.2; heartbeat-поток |
| RUN-02 | Результат в outbox до подтверждения размера и SHA-256; докачка после перезапуска | `runner_state.json` (одна запись: раннер не берёт новое, пока outbox не пуст); `HEAD`→`Upload-Offset`→`PUT` по 8 МиБ→`finish(Done)`; правила D-01/D-03; ловушка `seq` после перезапуска |
| RUN-03 | Этап и «дубль k из n» уходят на сервер; отмена доходит за секунды | `Tee: Progress` → `ProgressRequest{phase,take}`; ответ `ProgressReply.cancel` → `cancelled()`/`force_cancelled()`; heartbeat ≤5 с; мягкая отмена закрывает игру через `quit` (`shutdown` в `record/mod.rs`) |
| RUN-04 | Сбои «окружение» возвращают задачу в очередь | `classify` → `release`; `PausePolicy`; ловушка: `precheck` считает «не демка» сбоем окружения → preflight обязан идти ДО `run_job`; WR-01 сервера (release при запрошенной отмене) |
| RUN-05 | Коды `PLAYER_NOT_FOUND`, `PLAYER_AMBIGUOUS`, `NO_HIGHLIGHTS`, `BAD_DEMO`, `DEMO_TOO_LARGE` | Новые `FailCode` + `classify` (исчерпывающий `match` заставит решить класс); типизированные ошибки из `resolve_player`/`plan_player`/`demo::read_demo_limited`; `wire::fail` |
| RUN-06 | Перед загрузкой ffprobe: H.264, yuv420p, faststart | Подтверждено запуском: текущие аргументы кодера НЕ дают faststart; нужен `-movflags +faststart` в `renderer::Encoder::args`; проверка боксов MP4 (moov раньше mdat) + `ffprobe` JSON |
| SEC-01 | Текст из демки очищается, не попадает в cfg; `drawtext` из файла; тесты | Найдены две точки инъекции в `renderer::render` (комментарии `//` с `t.map` и `shot.title`); санитайзер в `Match::build`; `drawtext textfile=`; таблица враждебных фикстур |
| SEC-02 | Лимит размера и распаковки, магия, разбор в дочернем процессе с лимитом памяти | Спайк: Job Object `ProcessMemoryLimit` самоприсвоением в дочернем `cs2-cli inspect`; потоковый счётчик zstd; проверка `snap::decompress_len` |
| SEC-03 | `find_player`: точное или единственное совпадение; `PLAYER_AMBIGUOUS` со списком до 10 | Новая `resolve_player` + `find_player` как её `.ok()`; порядок ярусов; кандидаты «имя (SteamID64)» |
</phase_requirements>

## Summary

Фаза состоит из двух независимых блоков, которые сходятся в живом прогоне. **Блок A — защита входа** (SEC-01…03, RUN-05, RUN-06): правки в существующих крейтах `cs2`, `renderer`, `demo`, `host`, `pipeline`, `wire`, `cli`. **Блок B — раннер** (RUN-01…04): новый крейт `runner` поверх готового протокола фазы 5, команда `cs2-cli runner`. Новых внешних зависимостей нет: `ureq` 3.4.2, `sha2` 0.10.9, `serde`/`serde_json`, `ctrlc` 3.5.2, `zstd` 0.14, `windows-sys` 0.61 уже в `Cargo.lock`.

Главные находки, меняющие план: (1) в `renderer::render` демо-текст попадает в `//`-комментарии cfg (`t.map`, `shot.title`) — перевод строки превращает хвост в живую команду консоли CS2; (2) текущая склейка не пишет `-movflags +faststart`, поэтому проверка RUN-06 провалила бы КАЖДЫЙ ролик (проверено ffmpeg 9.0.2: у ролика без флага `mdat` идёт раньше `moov`); (3) `precheck` классифицирует «файл не похож на демку» как `PRECHECK_FAILED` (класс «окружение») — без предварительного разбора в дочернем процессе мусорный файл друга зациклит `release` и поставит раннер на паузу (DoS очереди); (4) `snap::raw::Decoder::decompress_vec` выделяет заявленный размер сразу, а `demo::decompress` использует `zstd::decode_all` без потолка; (5) `seq` в `ProgressRequest` обязан расти и после перезапуска раннера в той же эпохе, иначе сервер перестаёт обновлять этап и дубль.

**Primary recommendation:** сначала блок A тремя–четырьмя параллельными планами (санитайзер+renderer / лимиты демки+`host::limit_self_memory`+`cs2-cli inspect` / `resolve_player`+новые `FailCode`+`wire::fail` / проверка результата+faststart), затем крейт `runner` (Link, transfer, state, Tee, Executor, PausePolicy) с интеграционными тестами на `server::serve`, затем один план «живой прогон» как human checkpoint.

## Project Constraints (from CLAUDE.md)

- Стек не менять: Rust workspace, `std::thread` без tokio, `ureq` блокирующий, `rusqlite` не нужен раннеру; встраиваться в существующие крейты.
- Платформа Windows; **все процессы, сокеты и Win32 — только через крейт `host`** (`Host`, `WinHost`, `FakeHost`). Поэтому лимит памяти дочернего процесса оформляется функцией в `host`, а не `windows-sys` в `cli`/`runner`.
- Одна задача записи за раз.
- `cs2-cli` обязан собираться и проходить `crates/cli/tests/pipeline.rs`; `cargo test --workspace` зелёный на ubuntu и windows (`ci.yml` гоняет оба).
- Пользовательские строки, доки и комментарии — по-русски, идентификаторы по-английски; ошибки `Result<T, String>`; отмена через `pipeline::check(p)?` и `Progress`.
- Тесты называются предложениями; тестовые данные — маленькие приватные `fn`; edition 2024; `cargo fmt` перед коммитом; clippy локально.
- Долгие шаги отчитываются через `Progress::stage`/`log`.
- Ключи и токены не попадают в `Debug`/журнал (образец: `JobConfig::fmt`).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Приём задачи, аренда, heartbeat, `release`/`finish` | `runner` (клиент `Link`) | `server` (уже готов) | Раннер только исходящий; сервер хранит истину о намерении |
| Скачивание демки / загрузка результата | `runner::transfer` | `server::blobs` | Потоково, с докачкой; хеш считает раннер, сверяет сервер |
| Лимит размера и распаковки демки | `demo` (`read_demo_limited`) | `runner` (проверка `Assignment.demo.size` до скачивания) | Единственное место, где читается и распаковывается файл |
| Изоляция разбора (память, время) | `cs2-cli inspect` в Job Object (`host::limit_self_memory`) | `pipeline::inspect` (родитель, `Host::run` с дедлайном) | Падение/OOM/зависание убивают только дочерний процесс |
| Очистка текста из демки | `cs2::Match::build` (граница доверия) | `renderer` (комментарии без демо-текста, `drawtext textfile`) | Два слоя: чистые данные + нулевая поверхность в cfg |
| Выбор игрока и коды отказов | `pipeline::resolve_player` + новые `FailCode` | `wire::fail` (строки кодов для бота) | Один источник правды; `run_job` и `inspect` зовут одну функцию |
| Проверка результата (H.264/yuv420p/faststart) | `pipeline::result` (ffprobe через `Host::run` + разбор боксов) | `renderer` (флаг `+faststart`) | Проверку используют и раннер, и в будущем приложение |
| Класс сбоя → release/finish, пауза | `runner::PausePolicy` | `pipeline::record::failure::classify` | Политика раннера; классификация общая |
| Состояние/outbox | `runner::state` (`runner_state.json`) | — | Раннер — источник правды о диске и итоге (D-01, D-03) |
| Живой прогон | человек за машиной автора | — | Нужны настоящие CS2/HLAE и демка |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| ureq | 3.4.2 (в `Cargo.lock`, features `json`, `platform-verifier`) | HTTP-клиент раннера | Уже в проекте (`faceit`, `app`); `&[u8]` реализует `AsSendBody` — тело с настоящим `Content-Length`; есть `timeout_send_body`, `timeout_recv_response`, `http_status_as_error(false)` [VERIFIED: ureq-3.4.2/src/send_body.rs:293 `impl AsSendBody for &[u8]`, config.rs:726-821 `timeout_*`] |
| sha2 | 0.10.9 | SHA-256 демки и результата | Уже в `Cargo.lock`; тот же хеш у сервера |
| serde / serde_json | 1.x | Состояние раннера, разбор ответов, вывод `inspect` | Уже везде |
| ctrlc | 3.5.2 (feature `termination`) | Корректная остановка раннера (Ctrl+C) | Уже в `Cargo.lock` через `server`; на Windows закрывает консоль |
| zstd | 0.14.0 | Потоковый ограниченный разбор `.dem.zst` | Уже в `demo`; `zstd::stream::Decoder` + `Read::take` проверены спайком |
| windows-sys | 0.61.2 (уже в `host`, feature `Win32_System_JobObjects`, `Win32_System_Threading`) | `host::limit_self_memory` | Только внутри `host`; новых features не нужно |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| chrono | 0.4 (есть) | Метки времени в `runner.log` | Только форматирование |
| server (path, **dev-dependency** раннера) | workspace | `server::serve` для интеграционных тестов | Только `[dev-dependencies]`; охрана `crates/server/tests/deps.rs` смотрит манифесты `server` и `wire`, но список `WORKSPACE_CRATES` стоит дополнить именем `runner` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Самоограничение дочернего процесса (`host::limit_self_memory` в начале `cs2-cli inspect`) | Родитель создаёт Job и присваивает дочерний процесс после `spawn` + «go» по stdin | Родительский вариант требует нового метода `Host` и piped stdin (у `run_bounded` stdin = null); самоограничение работает с неизменным `Host::run`, проверяется `FakeHost` (скрипт вывода). Оба подтверждены спайком |
| Новое поле `CmdSpec.memory_limit` | `Host::run_limited` | Поле ломает 17 литералов `CmdSpec`; выбранный вариант не трогает их вовсе |
| Крейт `unicode-general-category` для санитайзера | Явные диапазоны + `char::is_control`/`is_whitespace` | Новая зависимость ради 20 диапазонов не оправдана |
| `axum`/tokio для раннера | — | Запрещено ограничениями проекта |

**Installation:** новых внешних пакетов нет. Крейт `runner` в `[workspace].members`, зависимости: `wire`, `pipeline`, `ureq`, `sha2`, `serde`, `serde_json`, `ctrlc`, `chrono`; `[dev-dependencies]`: `server`.

**Version verification:** версии взяты из `Cargo.lock` (ureq 3.4.2, sha2 0.10.9, ctrlc 3.5.2, zstd 0.14.0, windows-sys 0.61.2) [VERIFIED: Cargo.lock].

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| ureq | crates.io | 8 лет (2018) | 5.3M/нед | github.com/algesten/ureq | OK | Approved (уже в проекте) |
| sha2 | crates.io | 10 лет | 23M/нед | github.com/RustCrypto/hashes | OK | Approved (уже в проекте) |
| ctrlc | crates.io | 11 лет | 1.8M/нед | github.com/Detegr/rust-ctrlc | OK | Approved (уже в `server`) |
| serde / serde_json | crates.io | 11 / 10 лет | 27M/нед | github.com/serde-rs | OK | Approved |
| zstd | crates.io | 10 лет | 7.2M/нед | — | OK | Approved (уже в `demo`) |
| snap | crates.io | 10 лет | 2.0M/нед | — | OK | Approved (уже в `demo`) |
| windows-sys | crates.io | 5 лет | 35M/нед | — | OK | Approved (уже в `host`) |

[VERIFIED: `gsd-tools query package-legitimacy check --ecosystem crates …` — все `OK`]. **Packages removed ([SLOP]):** none. **Flagged ([SUS]):** none. Все пакеты уже находятся в `Cargo.lock`, `checkpoint:human-verify` не нужен.

## Протокол сервера (прочитан в этой сессии, контракт для раннера)

| Что | Факт | Источник |
|-----|------|----------|
| Версия протокола | `pub const PROTOCOL_VERSION: u32 = 1;` | [VERIFIED: crates/wire/src/lib.rs:11] |
| Размер куска | `pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;` | [VERIFIED: crates/wire/src/lib.rs:14] |
| Адреса | `CLAIM = "/v1/runner/claim"`, `HELLO = "/v1/runner/hello"`, `job(id, action)` = `/v1/jobs/{id}/{action}`; действия `progress`, `release`, `finish`, `result` (HEAD/PUT), `demo` (HEAD/GET) | [VERIFIED: wire/src/lib.rs:446-454; server/src/api.rs:149-157] |
| Заголовки | `EPOCH = "X-Epoch"`, `UPLOAD_OFFSET = "Upload-Offset"`, `RETRY_AFTER = "Retry-After"` | [VERIFIED: wire/src/lib.rs:457-461] |
| Этапы `Phase` | `fetch, download, parse, positions, geometry, plan, record, assemble, upload` (+`Unknown`); **`precheck` в протоколе нет** | [VERIFIED: wire/src/lib.rs:205-217] |
| `claim` | `POST /v1/runner/claim?wait=N`, тело `ClaimRequest{status}`; 200 `Assignment{job_id, epoch, lease_until_ms, demo{size,sha256,orig_name}, faceit_link, player{steam64,name}, requested_by, options}` или 204 | [VERIFIED: api.rs:444-472, wire:182-199] |
| `progress` | тело `ProgressRequest{epoch, seq, phase, take{n,m}, status}` → `ProgressReply{lease_until_ms, cancel: none|soft|force}`; 409 `stale_epoch`/`job_terminal` | [VERIFIED: wire:262-283] |
| `release` | `ReleaseRequest{epoch, reason, counts}`, `counts` по умолчанию `true`; «Когда так делать, решает раннер фазы 6» | [VERIFIED: wire:289-300] |
| `finish` | `Outcome::Done{size,sha256}` / `Failed(Failure{code,class,state,hint,message,tail,elapsed_ms})` / `Cancelled`; повтор идемпотентен | [VERIFIED: wire:304-346; core.rs:640-]|
| Результат | `HEAD` → `Upload-Offset`; `PUT` с `X-Epoch`, `Upload-Offset`, `Content-Length` ≤ 8 МиБ (иначе 411/413), 204 + новый `Upload-Offset`; 409 `offset_mismatch` (смещение в теле и заголовке), 409 `upload_busy` + `Retry-After: 1` | [VERIFIED: api.rs:231-331] |
| Демка | `HEAD` (размер из `Content-Length`, `ETag` = sha256), `GET` + `Range: bytes=N-` → 206, не больше `CHUNK_SIZE` за ответ | [VERIFIED: api.rs:333-408] |
| Версия | `hello` с чужим `protocol` → 426 `protocol_mismatch` | [VERIFIED: api.rs:416-425] |
| `seq` | «Номер сообщения раннера считается заново в каждой эпохе», не больше последнего принятого — молча «не свежий» (этап и дубль не обновляются) | [VERIFIED: core.rs:422; rules.rs:164-170] |
| Неверный SHA-256 | `finalize_result` **удаляет `.part`** → следующий `HEAD` даёт 0 | [VERIFIED: blobs.rs:259-262 `std::fs::remove_file(&part)`] |
| `release` + отмена | `Event::Release` не смотрит `cancel_requested` (WR-01 ревью): отменённая задача уходит в очередь | [VERIFIED: 05-REVIEW.md WR-01; rules.rs:190-214] |

## Architecture Patterns

### System Architecture Diagram

```
 cs2-server (Linux или localhost)                       Windows-машина автора (один процесс cs2-cli runner)
 ┌──────────────────────────┐   исходящие HTTP    ┌────────────────────────────────────────────────────┐
 │ claim (long-poll 25 с) ◄─┼────────────────────┤ Loop (главный поток)                               │
 │ progress = heartbeat   ◄─┼──── Uplink thread ──┤  Idle ─claim─► Persist(Claimed) ─► Fetch demo      │
 │ release / finish       ◄─┼────────────────────┤    │ GET Range 8 МиБ, sha256 ── ok ──►             │
 │ HEAD/PUT result        ◄─┼────────────────────┤    ▼                                               │
 │ HEAD/GET demo          ──┼───────────────────►│  Preflight: Host::run(cs2-cli inspect …)           │
 └──────────────────────────┘                    │    │  дочерний процесс, Job Object (память), дедлайн │
        ▲ ответ progress: cancel none|soft|force  │    │  JSON: ok | {code, message, ≤10 кандидатов}    │
        └──────────── Tee: Progress ◄──── флаги ──┤    ▼ ok                                            │
                                                  │  pipeline::job::run_job(WinHost, JobSpec, Tee)     │
                                                  │    precheck → analyze → plan → record → assemble   │
                                                  │    │ Err(RunError{fail})  ──► classify              │
                                                  │    │   Environment|Launch ─► release (counts?)      │
                                                  │    │   Other / CANCELLED   ─► finish(Failed/Cancelled)│
                                                  │    ▼ Ok(Outcome)                                    │
                                                  │  pipeline::result::verify (ffprobe + moov/mdat)    │
                                                  │    │ негоден ─► finish(Failed RESULT_INVALID)       │
                                                  │    ▼ годен                                          │
                                                  │  Outbox: state.json{stage=Finished,result,sha}     │
                                                  │  Upload: HEAD offset → PUT 8 МиБ… → finish(Done)   │
                                                  │    └ 422 (после 2-го раза), 409 stale/terminal ─►   │
                                                  │        удалить из outbox + запись в runner.log     │
                                                  └────────────────────────────────────────────────────┘
 Перезапуск: state.json → kill_orphans + gamedir::recover → hello(holding) → Keep ─► resume / upload
```

### Recommended Project Structure
```
crates/
├── wire/        # + fail::{PLAYER_NOT_FOUND,…,KNOWN}; (опционально) Phase::Precheck
├── runner/      # НОВЫЙ: lib.rs, link.rs, transfer.rs, state.rs, uplink.rs, tee.rs,
│   │            #        exec.rs (Executor + PipelineExecutor), preflight.rs, policy.rs, backoff.rs
│   └── tests/   # интеграция с server::serve (dev-dep), без CS2
├── host/        # + limit_self_memory (win/sandbox.rs, shim в lib.rs)
├── demo/        # + read_demo_limited, DemoLimits, DemoError; проверка snap::decompress_len
├── cs2/         # + text.rs (clean_text), Match::build чистит имена и карту
├── renderer/    # комментарии без демо-текста; drawtext textfile; Encoder +faststart
├── pipeline/    # + resolve_player, inspect.rs (родитель+ядро), result.rs, новые FailCode
└── cli/         # + подкоманды `runner` и `inspect` (скрытая)
```

### Pattern 1: Preflight в дочернем процессе — ДО `run_job`
**What:** раннер запускает `cs2-cli inspect <demo> --player … --max-… --mem-mib …` через `Host::run` (дедлайн + отмена). Дочерний процесс первым делом `host::limit_self_memory(bytes)`, ставит `SetErrorMode` и сторожевой поток времени, затем: `read_demo_limited` → `demo::parse` → `Match::from_parsed` → `resolve_player` → `for_player`/`utility_for` → (если `flybys`) `positions::track`. Печатает ровно одну JSON-строку. Код выхода 0 = «ответ напечатан» (и `ok`, и типизированный отказ); любой другой код / отсутствие JSON / таймаут = `BAD_DEMO` («разбор не завершился: не уложился в память или время»). Родитель **повторно очищает** строки ответа (дочерний процесс недоверен) и режет число кандидатов до 10.
**When to use:** каждая задача, до `run_job`. Мусорный файл с неверной магией отсекается здесь же, а не в `precheck` (там он стал бы «окружением» — см. ловушку 1).
**Почему так, а не «весь run_job в дочернем»:** `run_job` неизменен; двойной разбор — осознанная цена v1.1 (оптимизация — отдельная задача).

### Pattern 2: Очистка на границе доверия + нулевая поверхность в cfg
**What:** `Match::build` пропускает `name` игроков и `map` через `cs2::text::clean_text`; `renderer::render` вообще не пишет демо-текст в cfg (комментарий шота — только номер и `takeNNNN`). Подписи `drawtext` — файлами `cap_NNN.txt` рядом с `assemble.ffgraph`. Тест: враждебные строки через `render` → каждая строка каждого `*.cfg` начинается с известного префикса.
**Example:** см. «Code Examples».

### Pattern 3: Единый источник выбора игрока
`pipeline::resolve_player(&Match, steam: Option<u64>, nick: &str) -> Result<PlayerId, PlayerError>`; `find_player` = `resolve_player(..).ok()` (приложение `app.rs:547` получает «None вместо наугад»). `run_job` и `inspect` вызывают одну функцию, `PlayerError` → `RunError{fail: Some(RecordFailure{code, state: RecordState::Precheck, …})}` (так уже сделан `retry_no_data`).

### Pattern 4: Outbox как единственная запись состояния
`runner_state.json` (запись: temp → `sync_all` → `rename`; `std::fs::rename` на Windows заменяет существующий файл — проверено запуском rustc 1.99) хранит ровно одну задачу `{version, server_job_id, epoch, stage, demo_path, demo_sha256, out_dir, local_job_id, attempt, result{path,size,sha256}, pending_outcome, sha_retries, last_good}`. Пока запись не пуста — раннер не делает `claim` (одна задача, загрузка без перекрытия). Состояние пишется: сразу после ответа `claim`; после скачивания; при `Artifact::OutDir`; перед первой отправкой `finish` (в том числе `Failed`/`Cancelled` — терминальные факты тоже надёжны, не только `Done`).

### Pattern 5: `Tee` как `Progress`
`Tee { inner: &dyn Progress, shared: Arc<Shared> }`: `stage(s)` → `shared.phase = map(s)`; `event(State{Take, take:Some((n,m))})`/`TakeDone` → `shared.take`; `cancelled()` = `inner.cancelled() || shared.cancel>=Soft`; `force_cancelled()` = `shared.cancel==Force`; `artifact(OutDir)` → запись в state. Heartbeat-поток читает `Shared`, шлёт `ProgressRequest` каждые `min(heartbeat_secs, 5)` с и сразу (не чаще раза в 2 с) при смене этапа/дубля, применяет `ProgressReply.cancel`. `Progress: Sync` — поэтому только атомики и `Mutex`.

### Anti-Patterns to Avoid
- **`run_job` на потоке без `catch_unwind` и без изоляции разбора:** паника в `positions::track` (`source2-demo` на враждебных данных) убьёт раннер, аренда истечёт, задача вернётся и убьёт снова.
- **Отмена как `finish(Cancelled)` при остановке раннера Ctrl+C:** остановка — не решение пользователя; запись состояния остаётся (`Executing`) и задача возобновится. Различать флаги: серверная отмена и локальное завершение.
- **Брать из `Assignment.options` пути или настройки:** раннер — граница доверия; опции игнорируются (в v1.1 пусты).
- **Имя файла из `orig_name`:** демка пишется как `remote/srv<job_id>.dem` (`console_name` + номер), оригинальное имя нигде в путях.
- **Глобальный таймаут на передаче:** только таймаут на кусок (8 МиБ).
- **Каждая ошибка сети = сбой задачи:** сеть — только backoff.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTTP | свой клиент / сокеты | `ureq` 3.4.2 | Уже в проекте, TLS системным хранилищем |
| SHA-256 | свой хеш | `sha2` | Совпадает с сервером |
| Лимит памяти процесса | `catch_unwind`/потоки/самосчёт | Job Object `JOB_OBJECT_LIMIT_PROCESS_MEMORY` через `host` | OOM и stack overflow не ловятся в процессе; спайк подтвердил быструю смерть дочернего процесса |
| Ограничение распаковки | `decode_all` + проверка потом | `zstd::stream::Decoder` + `Read::take(limit+1)` (+ потоковый счёт) | Бомба 128 КБ → 4 ГиБ; счёт 1 ГиБ без накопления занял ~44 мс |
| Остановка по Ctrl+C | свой `SetConsoleCtrlHandler` | `ctrlc` (feature `termination`) | Уже в `server` |
| Запуск ffprobe/ffmpeg с дедлайном | `std::process::Command` | `Host::run` | Дедлайн, отмена, `FakeHost` |
| JSON-разбор вывода ffprobe | регэкспы | `serde_json` с `#[serde(default)]` (как `verify::parse_probe`) | Недоверенный текст |
| Одна копия раннера | PID-файл с эвристиками | Windows: `OpenOptions::share_mode(0)` на `runner.lock`, держать открытым | Снимается ОС при падении |
| Единица очистки Unicode | зависимость от таблиц категорий | ~20 явных диапазонов + `is_control`/`is_whitespace` | Нужно «вырезать опасное», а не классифицировать весь Unicode |

**Исключение (нужно писать самим):** разбор верхнеуровневых боксов MP4 для faststart (25 строк, проверено на `plain.mp4`/`fast.mp4`, см. Code Examples) — `ffprobe` не сообщает порядок боксов в JSON.

**Key insight:** вся надёжность фазы держится на том, что недоверенное исполняется только в процессе, который можно убить, а недоверенный текст не попадает ни в один интерпретатор (cfg, filtergraph) — фильтры вторичны.

## Результаты спайка изоляции разбора (запущен на машине автора, Windows 11, rustc 1.99)

Стенд: `scratchpad/spike` (родитель + дочерний бинарь + генератор zstd-бомбы). Родитель создавал Job Object с `KILL_ON_JOB_CLOSE`, дочерний ждал «go» по stdin.

| Опыт | Результат |
|------|-----------|
| Лимит процесса 256 МиБ, аллокация 100 МиБ | выход 0, 20 мс |
| Лимит 256 МиБ, 600 МиБ кусками по 1 МиБ | выход `0xC0000409`, 100 мс; stderr `memory allocation of 1048576 bytes failed` |
| Лимит 256 МиБ, один `vec![1u8; 3000 МиБ]` | выход `0xC0000409`, 81 мс |
| `JOB_OBJECT_LIMIT_JOB_MEMORY` 256 МиБ, 600 МиБ | выход `0xC0000409`, 81 мс (оба варианта лимита работают) |
| Без лимита, 600 МиБ | выход 0 (контроль) |
| Зависший дочерний, дедлайн родителя 10 с | `Child::kill`, TIMEOUT 10.0 с |
| Бомба zstd 4 ГиБ нулей = 131 097 байт файл; сбор в `Vec` с потолком 2000 МиБ при лимите процесса 1 ГиБ | `Err(OutOfMemory)` → паника, 243 мс, процесс умер быстро |
| Та же бомба, потоковый счёт до потолка 1 ГиБ при лимите 256 МиБ | «streamed 1073741825 bytes in 44ms; over limit = true», выход 0 |
| **Самоприсвоение**: дочерний сам делает `CreateJobObjectW`+`SetInformationJobObject`+`AssignProcessToJobObject(GetCurrentProcess())` в начале `main` (лимит 256 МиБ, `ActiveProcessLimit=1`) | работает: 100 МиБ — выход 0; 600 МиБ — `memory allocation … failed`, abort; попытка породить `cmd` внутри — `os error 1816` (ограничение числа процессов действует) |
| `std::fs::rename` поверх существующего файла и при открытом читателе | `Ok(())` в обоих случаях |

[VERIFIED: запуск `spike.exe`, `selfjob.exe`, `renametest.exe` в этой сессии]. Выводы: (1) `Host::run` без изменений годится для запуска дочернего процесса, если он сам себя ограничивает; (2) окно гонки «spawn→assign» исчезает; (3) OOM-смерть быстрая и без диалога на этой машине (WER-диалог на других конфигурациях — `[ASSUMED]`, поэтому дочерний вызывает `SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX)`); (4) явный потолок распаковки даёт аккуратный код `DEMO_TOO_LARGE`, лимит памяти — страховка → `BAD_DEMO`.

**Рекомендуемые стартовые числа** (`[ASSUMED]`, откалибровать на живом прогоне — `inspect` печатает `peak_mem_bytes` из `PeakProcessMemoryUsed`): файл ≤ 2000 МиБ (как `max_demo_bytes` сервера), распакованная демка ≤ 3 ГиБ, память дочернего процесса 6 ГиБ (на машине автора 32 ГБ ОЗУ), дедлайн разбора 15 мин. Дополнительно: `snap::raw::decompress_len(payload)` ≤ 128 МиБ в `Command::decode_payload` — иначе одна команда с заявленными 4 ГиБ выделяет их сразу (`decompress_vec` делает `vec![0; decompress_len(input)?]`) [VERIFIED: snap-1.1.2/src/decompress.rs:105-110].

## Common Pitfalls

### Pitfall 1: Мусорный файл превращается в «сбой окружения» и ставит раннер на паузу
**What goes wrong:** `precheck::demo_item` для файла без магии возвращает `Item::fail("файл не похож на демку CS2")` → `PRECHECK_FAILED` → `classify` = `Environment` → по D-02 `release` → задача снова выдаётся → две подряд = пауза раннера для всех друзей.
**Why:** `precheck` считает демку частью окружения.
**How to avoid:** preflight (проверка магии, лимитов, разбор) идёт ДО `run_job`; задачи с кодами `BAD_DEMO`/`DEMO_TOO_LARGE`/`PLAYER_*`/`NO_HIGHLIGHTS` завершаются `finish(Failed)` и **никогда** не считаются в серию паузы. Тест: файл с неверной магией → `finish(Failed, BAD_DEMO)`, `release` не вызывался, `PausePolicy` не сдвинулся.
**Warning signs:** `PRECHECK_FAILED` с текстом про демку; пауза после одного плохого файла.
[VERIFIED: pipeline/src/precheck/mod.rs:511-541, failure.rs:531-547 `FailCode::PrecheckFailed | … => FailClass::Environment`]

### Pitfall 2: `seq` после перезапуска раннера в той же эпохе
**What goes wrong:** `seq` стартует с 1, у сервера `last_seq` уже 200 → все progress «не свежие»: этап и «дубль k из n» замёрзли, аренда при этом продлевается — не заметно.
**How to avoid:** `seq = max(prev + 1, unix_ms())` (`u64`, ниже `i64::MAX`, иначе сервер приводит к `i64`: IN-01 ревью). `last_seq` сбрасывается только при новой выдаче [VERIFIED: core.rs:422].

### Pitfall 3: `release` отменённой задачи (WR-01 сервера)
**What goes wrong:** `release` ставит задачу в очередь, не глядя на `cancel_requested` → задача выдаётся снова, тратит выдачу.
**How to avoid (на стороне раннера, независимо от починки сервера):** перед `release` проверить `Shared.cancel`; если отмена запрошена — `finish(Cancelled)`, не `release`.

### Pitfall 4: «Проверенные дубли не переснимаются» — только если проверены ВСЕ
**What goes wrong:** D-01 формулирует «проверенные дубли не переснимаются», но `resume` при любом непроверенном дубле пишет **все заново** (HLAE нумерует дубли «наибольший + 1»): в журнале «записываю все заново». Тест RUN-02 «убить посреди записи» ожидает полную перезапись, а «убить после записи/во время склейки/загрузки» — сразу склейка или загрузка.
[VERIFIED: job.rs:663-694, цитата: `"дубли не все проверены: {not_validated} — записываю все заново (новые дубли HLAE нумерует подряд)"`]

### Pitfall 5: Остаток папки игры и осиротевший `cs2.exe` после падения раннера
**How to avoid:** при старте, если в state есть `out_dir`: `pipeline::gamedir::kill_orphans(host, &out_dir)` → `pipeline::gamedir::recover(cs2_dir, &out_dir)` (порядок обязателен — живой `cs2.exe` держит копию демки; образец `app::jobs::recover_after_crash`) [VERIFIED: app/src/jobs.rs:240-250].

### Pitfall 6: Классификация новых кодов
`classify` — исчерпывающий `match` без `_`: новые `FailCode` не скомпилируются, пока класс не выбран. Все пять новых кодов + `RESULT_INVALID` + `JOB_ERROR` → `FailClass::Other`. `FailCode::hint()` тоже исчерпывающий. Тест-страж: `FailCode::ALL` (завести) ⊆ `wire::fail::KNOWN` (защита от дрейфа протокола, Pitfall 19 исследования).

### Pitfall 7: Таймауты `ureq` на длинных запросах
`claim` держится до 25 с: `timeout_recv_response ≥ poll_secs + 15`; куски: глобальный таймаут на один запрос (PUT 8 МиБ ≈ 180 с, GET ≈ 120 с), не на файл. `http_status_as_error(false)` — иначе 409/422 станут `Err` и потеряют тело с `offset`. `max_redirects(0)`: заголовок с токеном не должен уходить по перенаправлению.

### Pitfall 8: `upload_busy`, `offset_mismatch`, `422`
`409 upload_busy` — `Retry-After: 1`, повторить. `409 offset_mismatch` — новое смещение из `ApiError.offset`/`Upload-Offset`, продолжить с него (файл на диске, перемотка допустима). `422 sha256_mismatch` на `finish` — сервер уже удалил `.part`: **один** повтор с нуля, счётчик `sha_retries` лежит в state; второй 422 = окончательный отказ (D-03). Локально перед загрузкой пересчитать SHA-256 и сверить с записанным в state: расхождение = `RESULT_INVALID`.

### Pitfall 9: Подавление дубля `Done`/`Cancelled` и гонка отмены с готовым результатом
Если отмена пришла во время загрузки — прервать между кусками, `finish(Cancelled)`. Если `finish(Done)` ушёл при уже запрошенной отмене, сервер принимает `Done` (в `decide` для `Done` флаг отмены не проверяется) — это допустимо, не ловушка раннера.

### Pitfall 10: Раннер и приложение одновременно
Раннер и очередь приложения не знают друг о друге (интеграция — фаза 8). Две записи = `FOREIGN_GAME_RUNNING` у одной из сторон. Документировать «не запускать приложение с очередью во время раннера»; замок `runner.lock` защищает только от двух раннеров.

### Pitfall 11: Подписи с `%`, `'`, `:` и управляющими символами в `drawtext`
`text()` экранирует `'`, `\`, `:`, но не управляющие символы; `expansion=none` отключает `%{…}`. Переход на `textfile=` убирает всю грамматику фильтра: проверено ffmpeg 9.0.2 — файл `Глазами o'Neil:7\4 %{pid} ',a=b[x];y` нарисован буквально, `-/filter_complex` читает граф из файла с `cwd = out_dir` (`run_ffmpeg`: `cwd: Some(dir.to_path_buf())`). Файл с `\n` рисуется двумя строками — санитайзер обязан убрать переводы строки до записи. Файлы пишутся без завершающего перевода строки.

### Pitfall 12: Источник реальной демки для живого прогона
На диске автора нет ни одной `.dem`/`.dem.zst` (`out\demos` пуст, LFS-демки не вынуты; репозиторий не содержит демок). `cs2-server submit` принимает локальный файл, поэтому демку нужно получить заранее (скачивание FACEIT через `cs2-cli <ссылка>` кладёт файл в `./demos`). Включить в план как предусловие живого прогона.

## Code Examples

### Лимит памяти самого процесса (в `host`, проверено спайком `selfjob.exe`)
```rust
// Source: spike scratchpad/spike/src/bin/selfjob.rs (запущен), API: windows-sys 0.61
pub fn limit_self_memory(bytes: u64) -> Result<(), HostError> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() { return Err(HostError::JobAssign(format!("CreateJobObjectW: {}", last_error()))); }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = bytes as usize;
        // SetInformationJobObject(job, JobObjectExtendedLimitInformation, …) и
        // AssignProcessToJobObject(job, GetCurrentProcess()); handle НЕ закрывать до конца процесса
    }
    Ok(())
}
```
Вне Windows — `Err(HostError::Unsupported)`; `inspect` на не-Windows пишет предупреждение и идёт дальше (тесты потолка распаковки работают и на Linux).

### Ограниченная распаковка (проверено спайком)
```rust
// Source: spike child.rs "bombstream"; zstd 0.14 Decoder + Read::take
let dec = zstd::stream::Decoder::new(File::open(path)?)?;
let n = std::io::copy(&mut dec.take(limit + 1), &mut std::io::sink())?; // счёт без накопления
let too_large = n > limit;                                              // → DemoError::TooLarge
```
Для самого чтения: `Decoder::new(BufReader<File>)` + `.take(limit + 1)` + `read_to_end` в `Vec`, `n > limit` → `TooLarge`; plain `.dem` — `metadata().len()` против `max_unpacked`.

### Грамматика cfg: что менять в `renderer::render`
```rust
// БЫЛО (crates/renderer/src/lib.rs:170 и :207):
//   writeln!(s, "// Сгенерировано cs2-cli. Карта: {}. Шотов: {}.", t.map, t.shots.len());
//   writeln!(s, "// Шот {} -> take{i:04}: {} (score {:.0}, {:?})", k + 1, shot.title, shot.score, shot.tier);
// СТАЛО: демо-текст в комментарии не попадает вовсе
let _ = writeln!(s, "// Шот {} -> take{i:04} (score {:.0}, {:?})", k + 1, shot.score, shot.tier);
```
Тест-таблица враждебных строк (имена и карта): `"a\nquit"`, `"x\r\nexec evil"`, `"; quit"`, `"\" ; disconnect"`, `"%{pid}"`, `"'"`, `"\\"`, 4000 символов, `"\u{202E}"`, `"\u{2028}exec x"`, эмодзи, `"\0"`. Ассерт: каждая строка каждого `cfg` файла начинается с `//`, `echo CS2C`, `mirv_`, `cl_drawhud`, `r_show_build_info`, `hideconsole`, `engine_no_focus_sleep`, `exec cs2c_`, `demo_gototick`, `demo_resume`. Прогон в двух вариантах: (1) враждебные строки прямо в `Timeline` (renderer безопасен сам по себе); (2) через `Match::build` (граница доверия).

### `drawtext` из файла (проверено ffmpeg 9.0.2)
```
drawtext=fontfile='C\:/Windows/Fonts/arialbd.ttf':textfile='cap_001.txt':expansion=none:fontsize=30:fontcolor=white:x=10:y=10
```
`caption()` возвращает такую строку и кладёт `("cap_001.txt", text)` в `Output.files`; счётчик файлов ведёт `assembly`.

### Faststart (проверено на `plain.mp4` → `false`, `fast.mp4` → `true`)
```rust
// Source: spike scratchpad/spike/src/bin/faststart.rs (запущен)
fn is_faststart(path: &Path) -> Result<bool, String> { /* читаем заголовки боксов: size32, type;
   size==1 → 64-бит, size==0 → до конца; ftyp → ok; `moov` раньше `mdat` → true; `mdat` раньше `moov` → false */ }
```
Аргументы ffprobe для результата: `-v error -print_format json -show_entries stream=codec_type,codec_name,pix_fmt,width,height,duration:format=format_name,duration -i <файл>`. Допуски: видеопоток `codec_name == "h264"` и `pix_fmt == "yuv420p"`; есть аудиопоток (склейка пишет AAC); `format_name` содержит `mp4`; `duration > 0`; размер файла ≥ 1 байта и ≤ лимита сервера; faststart. Любое нарушение — `RESULT_INVALID` (класс «прочее»), файл остаётся уликой, не загружается.

### Протокол: кусок результата (набросок, не копировать вслепую)
```rust
// PUT: тело — &[u8] (Content-Length ставится автоматически), http_status_as_error(false)
agent.put(&url).header("Authorization", bearer).header("X-Epoch", epoch).header("Upload-Offset", off)
     .send(&buf[..n])
```
Токен только в `Authorization`; `Debug` типа токена печатает `***`; сообщения ошибок `ureq` не печатать без очистки.

### Порядок ярусов `resolve_player` (SEC-03)
1. Если `steam != 0` и найден ровно один `xuid` — он. Несколько записей с одним `xuid` — см. Open Question 4.
2. Имена после `clean_text`, сравнение: точное → без регистра и с схлопнутыми пробелами → подстрока без регистра. На каждом ярусе: 1 совпадение → игрок, >1 → `PlayerAmbiguous`, 0 → следующий ярус.
3. Ничего — `PlayerNotFound`. Кандидаты: до 10, в формате «имя (SteamID64)» (SteamID64 — безопасные цифры, различает одинаковые очищенные имена). Текст — в `Failure.message`, подсказка — «укажите SteamID64 игрока».

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `find_player`: подстрока, первое совпадение | `resolve_player`: точное/единственное, иначе типизированный отказ | эта фаза | Нет «записи не того игрока» |
| `drawtext=text='…'` со своими экранированиями | `textfile=` + очищенный текст | эта фаза | Нет грамматики фильтра от демо-текста |
| `highlights.mp4` без faststart | `-movflags +faststart` в обоих `Encoder::args` | эта фаза | Telegram показывает видео (фаза 7) |
| `read_demo`: `decode_all` без потолка | `read_demo_limited` | эта фаза | Бомба → `DEMO_TOO_LARGE` |
| Разбор в процессе приложения | Разбор в дочернем `cs2-cli inspect` | эта фаза | OOM/зависание/панику переживает раннер |

**Deprecated/outdated:** предположение ROADMAP/CONTEXT, что `run_job` может пройти без изменений в `pipeline` — API и порядок шагов остаются прежними, но внутри меняются `find_player`→`resolve_player`, `plan_player` (типизированный `NO_HIGHLIGHTS`) и `analyze` (типизированные `BAD_DEMO`).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Стартовые лимиты: файл 2000 МиБ, распаковка 3 ГиБ, память дочернего 6 ГиБ, дедлайн разбора 15 мин | Результаты спайка | Легитимная длинная демка получит `DEMO_TOO_LARGE`/`BAD_DEMO`; калибровать по `peak_mem_bytes` на живом прогоне |
| A2 | `source2-demo` 0.5.9 на враждебных данных может паниковать/зависать | Pattern 1 | Если устойчив — двойной разбор лишний, но вреда нет |
| A3 | WER-диалог при `0xC0000409` на других машинах блокирует; `SetErrorMode` его подавляет | Спайк | Дочерний зависнет на диалоге до дедлайна родителя — потеря минут, не безопасности |
| A4 | В реальных демках одинаковый `xuid` в двух записях `userinfo` бывает (переподключение) | resolve_player | Если не бывает — ярус 1 однозначен; если бывает — нужно правило |
| A5 | Heartbeat каждые ≤5 с даёт отмену «за секунды» | Pattern 5 | Если сервер/лог не выдерживает частоты — вернуть к `heartbeat_secs` (10 с) |
| A6 | `counts=false` для класса `Environment`, `true` для `Launch` — лучшая политика выдач | Open Questions 1 | Расходится с буквой D-02 («сервер считает выдачи»); нужна подтверждение автора |
| A7 | Результат лучше оставлять на месте (`out_dir/highlights.mp4`), а не переносить в отдельный каталог outbox | Pattern 4 | Если нужен изолированный outbox — `rename` на том же томе, меняет только пути в state |
| A8 | `ProgressRequest.status` — достаточное место для «предпроверка/проверка демки», `Phase::Precheck` можно не заводить | Open Questions 3 | Бот фазы 7 будет показывать этап неточно |

## Open Questions

1. **Политика `counts` при `release` (нужно решение автора).**
   - Знаем: D-02 говорит, что сервер считает выдачи и после `max_grants` (3) ставит `TOO_MANY_GRANTS`; `ReleaseRequest.counts` по умолчанию `true`. Пауза после двух одинаковых сбоев + возобновление = третья выдача; если окружение ещё сломано, задача друга падает с «демка роняет раннер» — вопреки духу RUN-04.
   - Рекомендация: `counts=false` для чистого `Environment` (`PRECHECK_FAILED`, `FOREIGN_GAME_RUNNING`, `PREV_GAME_STUCK`, `HLAE_ERROR`, `LAUNCHER_FAILED`), `counts=true` для `Launch` (`GAME_NOT_STARTED`, `CONSOLE_NOT_OPEN`, `DEMO_NOT_LOADED`, ранний `GAME_EXITED` — их может вызывать «ядовитая» демка, этот счётчик её и останавливает). Меняет только значение поля, протокол и сервер не трогаются. Если автор хочет букву D-02 — `counts=true` везде.
2. **Калибровка лимитов.** Реальной демки нет; `inspect` печатает `peak_mem_bytes`, значения A1 подтвердить на первом живом прогоне и вынести в `RunnerConfig`.
3. **Этап «предпроверка» в протоколе.** В `wire::Phase` его нет. Варианты: слать `Phase::Parse` со `status="предпроверка…"` (без правок протокола) или добавить `Phase::Precheck` (аддитивно; `Unknown` у старых читателей; `Phase::ALL` растёт до 10; тест `wire` обновить).
4. **Одинаковый `xuid` в нескольких записях игроков.** Проверить на живой демке (в `inspect` вывести число дублей). Пока: если все записи с этим `xuid` имеют одно очищенное имя — это один игрок (берём запись с наибольшим числом убийств+гранат), иначе `PLAYER_AMBIGUOUS`.
5. **Минимальная длина запроса для яруса «подстрока».** SEC-03 буквально: «точное или единственное совпадение». Минимум 3 символа сделал бы поведение ещё безопаснее, но добавляет правило, которого в требованиях нет; решать автору (по умолчанию — буквально по SEC-03).
6. **Мягкая или жёсткая отмена по умолчанию.** `cs2-server job cancel` ставит мягкую; мягкая закрывает игру через `quit` и ждёт `quit_wait` (секунды), жёсткая — сразу. «За секунды» (критерий 2) выполнено обоими путями; значение `quit_wait` смотреть в `Deadlines` на живом прогоне.
7. **FACEIT-ссылка в раннере.** `Assignment.faceit_link` уже в протоколе, `submit` её не создаёт, BOT-05 — фаза 7. Поддержка (`DemoSource::Faceit(parse_match_id)` + ключ автора) — три строки, но без теста вживую; предлагается включить только распознавание и отказ `UNSUPPORTED_SOURCE`, пока бот не готов.
8. **Дефекты сервера из ревью фазы 5 (CR-01, WR-01, WR-02).** Вне фазы, но влияют: CR-01 безопасен, пока сервер слушает loopback (live-run); WR-01 обходится проверкой отмены перед `release` (Pitfall 3); WR-02 — риск потери задачи при третьей выдаче после обрыва связи (редко).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust / cargo | сборка | ✓ | 1.99.0 | — |
| ffmpeg / ffprobe | склейка, RUN-06, тесты drawtext | ✓ | 9.0.2 (gyan full build) | — |
| NVIDIA GPU (NVENC) | кодирование | ✓ | RTX 3060 Ti | x264 на процессоре (автоматически в `run_job`) |
| HLAE | запись | ✓ | `C:\Users\kamal\AppData\Local\hlae` (из `settings.json`) | — |
| CS2 (Steam) | запись | ✓ | `…\Counter-Strike Global Offensive` | — |
| Git LFS | демки | ✓ | 3.7.1, но демок на диске нет | скачать через FACEIT (ключ в `settings.json`) |
| ОЗУ машины | калибровка лимитов | ✓ | ≈32 ГБ | — |
| Node (для GSD tooling) | gsd-tools | ✓ | 24.20 (`/c/Program Files/nodejs`) | — |
| Реальная `.dem` / `.dem.zst` | живой прогон | ✗ | — | скачать матч FACEIT командой `cs2-cli`; без неё критерий 1 невыполним |
| Python | — | ✗ | — | не нужен |

**Missing with no fallback:** демка для живого прогона — получить до плана «живой прогон».
**Missing with fallback:** нет.

## Security Domain

`security_enforcement` включён (`security_asvs_level: 1`).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes (раннер → сервер) | Bearer-токен раннера, хранится в env/файле, не в argv; `Token` с редактируемым `Debug`; 401 → остановка с понятным текстом |
| V3 Session Management | нет | — |
| V4 Access Control | yes | `X-Epoch`/эпоха в теле; раннер бросает задачу при `stale_epoch`/`job_terminal`; сервер ничего не присылает в пути и настройки (`options` игнорируются) |
| V5 Input Validation | **yes (ядро фазы)** | `cs2::text::clean_text` на границе; `textfile` для `drawtext`; нет демо-текста в cfg; разбор SteamID64; лимиты; `console_name` для путей |
| V6 Cryptography | yes | SHA-256 только `sha2`; ничего своего |
| V7 Error Handling and Logging | yes | `Failure.message/tail` очищаются (`pipeline::record::log::clean`, `redact`), без локальных путей и токенов (SEC-04 на стороне раннера) |
| V12 Files and Resources | **yes** | потолок размера файла и распаковки, магические байты, дочерний процесс с лимитом памяти и времени, имя файла по номеру задачи |
| V13 API and Web Service | yes | TLS системным хранилищем (`platform-verifier`), `max_redirects(0)`, идемпотентный `finish`, таймауты на куски |
| V14 Configuration | yes | токен не в журнале; `runner_state.json`/`runner.log` в профиле пользователя |

### Known Threat Patterns for Rust-раннер + демки от третьих лиц

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Ник/карта с `\n`, `;`, кавычками → команда в cfg CS2 | Tampering / Elevation | Санитайзер в `Match::build` + нулевая поверхность в `renderer` + тест грамматики cfg |
| Ник в `drawtext` ломает граф ffmpeg | Tampering | `textfile=`, `expansion=none` |
| zstd-бомба, огромный `snap`-размер | DoS | `read_demo_limited`, `decompress_len` ≤ 128 МиБ, Job Object `ProcessMemoryLimit` |
| Битая демка: паника/зависание парсера | DoS | Дочерний процесс + дедлайн + `catch_unwind` вокруг `run_job` |
| «Ядовитая» демка роняет игру, задача зацикливается | DoS | `Launch` → `release counts=true` → `TOO_MANY_GRANTS` (сервер) |
| Мусорный файл ставит раннер на паузу | DoS | Preflight до `precheck`; контентные коды не идут в серию паузы |
| Выбор не того игрока | Integrity | `resolve_player` строго; `PLAYER_AMBIGUOUS` до запуска CS2 |
| Подмена/усечение результата | Integrity | размер + SHA-256, пересчёт перед загрузкой, `finish` проверяет сервер |
| Устаревший раннер пишет в чужую выдачу | Tampering | `X-Epoch`, `stale_epoch` → бросить |
| Утечка токена в журнал/`argv`/`Debug` | Info disclosure | `Token` newtype, env/файл, тест с подставным токеном по `runner.log` и `Debug` |
| Остаточный риск: CS2 сам открывает чужую демку | — | Вне фазы; фиксируется в Assumptions как принятый риск автора |

## Подход к тестам (без Nyquist-раздела: `workflow.nyquist_validation: false`)

- **Быстрые модульные (Windows и Linux):** `cs2::text` (таблица враждебных строк, обычные имена не меняются); `renderer` грамматика cfg и `textfile`; `demo::read_demo_limited` (бомба генерируется в тесте `zstd::Encoder` — 4 МиБ нулей с потолком 1 МиБ, не 4 ГиБ); `resolve_player` (ярусы, регистр, пробелы, SteamID, >10 кандидатов); `FailCode::ALL` ⊆ `wire::fail::KNOWN`; `PausePolicy`; `is_faststart` на синтетических байтах; разбор вывода ffprobe для `result`; `state.json` (атомарная запись, повреждённый файл не роняет раннер).
- **Мутационный тест демки (`cargo test`):** валидная синтетическая демка из `crates/cli/tests/pipeline.rs::synthetic_demo` → усечение в ~50 смещениях и ~200 перевёрнутых байтов → `demo::parse` + `Match::from_parsed` в `catch_unwind` с потоковым таймаутом: ни паники, ни зависания. Честное ограничение: `positions::track`/`source2-demo` на синтетике не проверяется — это закрывает дочерний процесс.
- **Интеграционные раннера (`server::serve` в процессе, `Executor`-заглушка):** claim → progress (этап, «дубль k из n») → finish; отмена с сервера доходит до `Tee` за < 2×heartbeat; обрыв загрузки (флаг остановки между кусками) → новый запуск докачивает с `Upload-Offset`; `422` дважды → запись удалена; `stale_epoch` → бросить; `release` для `Environment`; контентный сбой → `finish(Failed)` без `release`; `seq` растёт через «перезапуск».
- **Windows-only:** `host::limit_self_memory` (дочерний тест-бинарь или `cs2-cli inspect` на большой аллокации), `runner.lock`.
- **Живой прогон (human checkpoint, отдельный план):** сценарий: поднять `cs2-server serve` (loopback `127.0.0.1:8080`) → `runner add live` → `cs2-server submit <демка> --player wytom` → `cs2-cli runner --server … --hlae … --cs2 …` → наблюдать `cs2-server job show`; проверить `highlights.mp4` на сервере (размер, SHA-256); затем учения: `job cancel` посреди записи (CS2 закрылся за секунды), убийство раннера после записи и посреди загрузки (докачка), второй запуск с чужим `cs2.exe` (задача вернулась в очередь), мусорный `.dem` и демка с игроком-двойником (коды до запуска CS2). Улики сохранить в папке фазы.

### Wave 0 Gaps
- [ ] `crates/runner/` (крейт, `Cargo.toml`, члены workspace) — нет
- [ ] `FailCode::ALL`, `wire::fail::KNOWN` — нет
- [ ] Фикстуры враждебных строк (общий модуль данных для `cs2`, `renderer`, `runner`) — нет
- [ ] `server::serve`-стенд для тестов раннера (взять из `crates/server/tests/common/mod.rs`; он тестовый, не библиотечный — либо скопировать минимум, либо вынести в `server::testing`)

## Рекомендуемая нарезка на планы (для планировщика)

| Волна | План | Требования | Основные файлы | Пересечения |
|-------|------|-----------|----------------|-------------|
| 1 | Текстовая безопасность | SEC-01 | `cs2/src/{lib,text}.rs`, `renderer/src/lib.rs` (комментарии, `caption`→`textfile`), тесты | `renderer/lib.rs` — общий с планом «результат»: вести последовательно или одним планом |
| 1 | Лимиты и изоляция разбора | SEC-02, часть RUN-05 | `demo/src/{lib,command}.rs`, `host/src/{lib,win/sandbox}.rs`, `pipeline/src/inspect.rs`, `cli/src/{main,inspect}.rs` | `pipeline/src/lib.rs` — общий с планом «игрок»: разные модули |
| 1 | Выбор игрока и коды | SEC-03, RUN-05 | `pipeline/src/{lib,job}.rs`, `record/failure.rs`, `wire/src/lib.rs` | `job.rs` |
| 1 | Проверка результата | RUN-06 | `renderer/src/lib.rs` (`+faststart`), `pipeline/src/result.rs` | см. выше |
| 2 | Крейт `runner`: Link, transfer, state, Tee, policy, executor, loop | RUN-01…04 | `crates/runner/**`, `cli/src/runner.rs`, `Cargo.toml` | зависит от всех планов волны 1 (коды, `inspect`, `result`) |
| 3 | Живой прогон и учения | критерии 1–3 | `.planning/phases/06-…/` улики | human checkpoint |

## Sources

### Primary (HIGH confidence)
- Код (прочитан в этой сессии): `crates/wire/src/lib.rs`, `crates/server/src/{api,core,rules,blobs,lib,cli}.rs`, `crates/pipeline/src/{lib,job,assemble}.rs`, `crates/pipeline/src/record/{failure,mod,verify}.rs`, `crates/pipeline/src/precheck/mod.rs`, `crates/renderer/src/lib.rs`, `crates/demo/src/{lib,parse,command}.rs`, `crates/cs2/src/lib.rs`, `crates/host/src/{lib,runner}.rs`, `crates/host/src/win/job.rs`, `crates/cli/src/{main,record}.rs`, `crates/app/src/{queue,jobs}.rs`, `crates/server/tests/deps.rs`
- `.planning/phases/05-yadro-servera-i-protokol/05-REVIEW.md`, `05-VERIFICATION.md`, `05-CONTEXT.md`
- `.planning/research/ARCHITECTURE.md`, `PITFALLS.md` (Pitfall 1, 2, 4, 13–15, 18, 19, 24, 26, 39)
- Исходники зависимостей в `~/.cargo/registry`: `ureq-3.4.2` (`send_body.rs`, `config.rs`), `snap-1.1.2` (`decompress.rs`)
- Запущено в этой сессии: спайк Job Object/zstd/rename (`scratchpad/spike`), ffmpeg 9.0.2 (faststart, `drawtext textfile`), `gsd-tools query package-legitimacy check`

### Secondary (MEDIUM confidence)
- Сопоставление `FailClass`/`QueuePolicy` с желаемым поведением раннера (логика прочитана, поведение раннера ещё не запускалось)

### Tertiary (LOW confidence)
- Числовые лимиты A1, робастность `source2-demo` A2, WER-диалог A3, дубли `xuid` A4

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — всё уже в `Cargo.lock`, проверено `package-legitimacy`
- Architecture: HIGH — протокол и точки интеграции прочитаны в коде; спайк подтвердил механизм изоляции
- Pitfalls: HIGH для 1–6, 11 (подтверждены кодом или запуском), MEDIUM для 7–10, 12
- Числа лимитов: LOW до живого прогона

**Research date:** 2026-10-11
**Valid until:** ~30 дней (код стабилен; протокол фазы 5 зафиксирован, но gap-план фазы 5 может поменять поведение `release`)
