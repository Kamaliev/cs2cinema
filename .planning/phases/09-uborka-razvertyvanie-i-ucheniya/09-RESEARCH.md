# Phase 9: Уборка, развёртывание и учения - Research

**Researched:** 2026-10-12
**Domain:** эксплуатация цепочки бот → `cs2-server` → раннер: уборщик файлов (Rust, `std::thread`, SQLite, виртуальные часы), развёртывание на Linux (systemd, Docker Compose, Tailscale/Caddy, резервные копии), сквозной тест в CI, учения с уликами
**Confidence:** HIGH по коду (всё прочитано в этой сессии), MEDIUM по поведению `telegram-bot-api` в контейнере (права на файлы и раскладка папок не проверены, спайк 07-11 пропущен), LOW по тому, как поведёт себя код на Linux в CI (на Linux серверные и раннерные крейты ни разу не запускались)

<user_constraints>
## User Constraints (CONTEXT.md для фазы 9 нет)

`09-CONTEXT.md` не создавался (`/gsd-discuss-phase` не запускался). Ограничениями считаются цель, критерии успеха, «Inherited from Phase 5» и «Notes» фазы 9 из `ROADMAP.md` и требования OPS-01…OPS-04 из `REQUIREMENTS.md`, как и в `08-RESEARCH.md`.

### Locked Decisions (из ROADMAP.md, Phase 9, дословно)
- Goal: «Цепочка бот → сервер → раннер работает днями без ручного присмотра: диски не заполняются, сервер разворачивается по документу на чистой машине, сквозной тест защищает цепочку в CI, а учения на обрывы и перезапуски заканчиваются продолжением работы или понятным сообщением.»
- Критерий 1: «После дня работы занятое место возвращается к исходному: уборщик удаляет просроченные демки, результаты, `.part` и файлы рабочей папки `telegram-bot-api` на сервере, а на раннере — сырые дубли задач, результат которых сервер подтвердил.»
- Критерий 2: «По документации развёртывания автор поднимает на чистой Linux-машине `telegram-bot-api` (compose), `cs2-server` под systemd, доступ к API раннера через Tailscale или Caddy и резервные копии БД; снаружи открыт только задуманный порт, а сервер переживает перезагрузку машины.»
- Критерий 3: «CI собирает `cs2-server` на Linux и прогоняет сквозной тест: фейковый Telegram, сервер в процессе и раннер на `FakeHost` проводят демку от сообщения в чат до отправленного ролика, а сценарий сбоя — до понятного сообщения о сбое.»
- Критерий 4: «Учения проведены с уликами: убийство раннера посреди записи, обрыв загрузки, перезапуск сервера посреди задачи, отмена при офлайн-раннере, демка на 500 МБ через прокси, несовпадение версий раннера и сервера. Каждое заканчивается продолжением задачи или понятным сообщением, без двойной записи и потерянного результата; после учений поиск подставного токена по журналам, БД и сообщениям чата ничего не находит.»
- Inherited from Phase 5 (принято автором, вариант B): до входа в фазу 9 закрыть CR-01, WR-01, WR-02 и прогнать CI на ubuntu-latest (job test, артефакт `cs2-server-linux-x64`).
- Notes: «учения требуют реального стенда и автора за машиной. В v1.0 ручные учения остались непроведёнными, поэтому здесь улики каждого учения (журналы сервера и раннера, состояние задачи) сохраняются в папке фазы до её закрытия.»
- Mode: mvp. Depends on: фазы 7 и 8.

### Claude's Discretion
Всё остальное: сроки хранения, форма уборщика и его место в потоках сервера и раннера, имена новых ключей настроек, раскладка `docs/`, выбор Tailscale или Caddy как основного, формат протокола учений и папка улик, разбиение на планы.

### Deferred Ideas (OUT OF SCOPE)
- Новые возможности бота и раннера (BOT-F1 «раннер офлайн» и т. п.), мультиплеерная очередь, веб-панель.
- Защита HTTP-слоя сервера от медленных клиентов (WR-03 фазы 5) как отдельная доработка кода: в этой фазе закрывается документом (прокси со своими таймаутами), а не правкой `http.rs`, если автор не решит иначе.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| OPS-01 | Уборщик чистит демки, результаты, `.part`, рабочую папку bot-api и сырые дубли на раннере | Разделы «Уборщик сервера» и «Уборщик раннера»: правила по состояниям, двухшаговое удаление через `files.deleted_at`, проверка по возрасту для папки bot-api, виртуальные часы в тестах |
| OPS-02 | Документация развёртывания: compose для `telegram-bot-api`, systemd-юнит, Tailscale или Caddy, резервные копии | Раздел «Развёртывание»: готовые заготовки compose, юнита, таймера резервных копий, вывод про `http://100.x` (раннер его не принимает), ловушки `DynamicUser` и Docker/ufw |
| OPS-03 | Сквозной тест с фейковым Telegram и `FakeHost`; сборка сервера на Linux в CI | Раздел «Сквозной тест и CI»: `crates/runner/tests/chain.rs`, что `FakeHost` умеет и не умеет, что в CI уже есть и чего нет (CI на Linux с серверным кодом ни разу не запускался) |
| OPS-04 | Учения: убить раннер посреди записи, обрыв загрузки, перезапуск сервера, отмена при офлайн-раннере, 500 МБ через прокси, несовпадение версий | Раздел «Учения»: механизмы, якоря в коде, улики, рецепт поиска токена, разделение «подготовка автоматом / проведение человеком» |
</phase_requirements>

## Project Constraints (from .claude/CLAUDE.md)

- Стек не менять: Rust workspace, `std::thread` без tokio, SQLite через rusqlite 0.32, встраиваться в существующие крейты (`server`, `runner`, `wire`, `app`, `cli`).
- Платформы: сервер — Linux (собирается и тестируется также на Windows в CI), раннер и запись — только Windows. Одна запись за раз.
- Внешние процессы (HLAE, CS2) имеют недокументированные маркеры: любой детектор с таймаутом и диагностикой.
- `cs2-cli` собирается, `crates/cli/tests/pipeline.rs` остаётся зелёным.
- Пользовательские строки, `//!`-доки и журналы по-русски; идентификаторы по-английски; ошибки `Result<T, String>`; прогресс только через `Progress`/`Log`; имена тестов — предложения.
- Миграции схемы базы только добавлением в конец списка (`crates/server/src/db.rs`, `MIGRATIONS`; сейчас три шага).
- Правки репозитория только через GSD-команды (этот файл — артефакт планирования).

## Summary

Три из четырёх требований упираются не в «написать уборщик», а в четыре находки, которых нет в ROADMAP. **Первая:** на сервере вообще нет сроков хранения. Колонка `files.deleted_at` есть в схеме (`db.rs`), запросы `result_by_job`/`demo_by_job` её уже учитывают (`core.rs:1402`, `core.rs:1412`), но не пишет в неё никто; `Core::sweep` (`core.rs:1191`) занимается только арендами и зависшим приёмом, а `discard_parts` вызывается лишь при окончательных состояниях и при `recover`. Бот и ядро уже написаны так, как будто демки может «больше не быть» (`submit_from`: «демки задачи #N больше нет — пришлите файл заново»; `deliver_once` без файла: «Ролик задачи #N не найден на сервере»), поэтому уборщик встраивается без переделки: помечает `deleted_at` в транзакции, потом удаляет файл. **Вторая:** на раннере половина OPS-01 уже сделана в фазе 6 (`Runner::remove_finished`, `lib.rs:782`, вызывается при `TransferEnd::Done`, `lib.rs:1251`: после принятого сервером результата удаляются демка, ролик и папка `out/srv{id}`), но папка дублей **не** удаляется при `drop_job` (`lib.rs:741`), при отмене и при сбое («остаётся уликой»), а упавший между «сервер принял» и «удалили» раннер оставляет папку навсегда. Значит, на раннере нужен не «уборщик подтверждённых», а уборщик осиротевших `out/srv*` и `remote/srv*` по возрасту и по номеру удерживаемой задачи. **Третья:** код раннера (`Link::new`, `link.rs:154`) отказывается слать токен по `http://100.x.y.z` (адрес Tailscale из диапазона 100.64/10 не «частный» для `Ipv4Addr::is_private`, см. `link.rs:129-148`), поэтому документ развёртывания обязан строиться на `tailscale serve` (HTTPS с настоящим сертификатом, имя `*.ts.net`) или на Caddy с доменом, а не на «адрес Tailscale и порт 8080». **Четвёртая:** локальная ветка `main` на 365 коммитов впереди `origin/main` (последний CI-запуск на GitHub — 2026-10-09, коммит `6f4c6b1`, до всей версии 1.1), то есть `cs2-server`, `runner`, `wire` и бот на Linux в CI **не запускались ни разу**; «прогнать CI на ubuntu-latest» из «Inherited» — это ещё не сделанная работа с риском красного результата, а не формальность.

Из унаследованных дефектов: CR-01 и WR-01 закрыты в 07-01 (проверено по коду), WR-02 **всё ещё открыт** (`core.rs:684` — `WHERE state = 'queued' AND grants >= ?1` без учёта `lease_lost`), причём предложенная в 05-REVIEW правка `AND lease_lost = 0` сама оставляет дыру: задача с потерянной арендой и `grants == max_grants` не выдаётся никому (`core.rs:716`: `grants < ?4`) и не падает, то есть зависает в `queued` навсегда, если её раннер не вернётся. Нужна правка с окном на возврат. WR-03 фазы 5 (медленные клиенты) остаётся и закрывается документом: сервер за прокси с таймаутами, наружу не слушает.

Учения нельзя «выполнить» агентом: нужен настоящий Linux-хост, настоящие CS2/HLAE и автор у машины. Прошлые «pass» нельзя считать уликами: `06-UAT.md` прямо пишет «со слов автора, улик evidence/ нет», а `08-UAT.md` тест 5 (живой стенд) стоит `result: pass` с примечанием «автор отложил живой стенд в фазу 9»; пять проверок из `07-UAT.md` (живая цепочка, спайки `sendVideo` и общего тома, учения бота, поиск токенов) отложены в фазу 9 явно. План фазы должен отделить автоматическую подготовку (код уборщика, сквозной тест, документы, заготовки compose/юнитов, скрипты сбора улик и поиска токена, чек-листы) от точек, где ждёт человек (`checkpoint:human-action` для push и развёртывания, `checkpoint:human-verify` для учений).

**Primary recommendation:** (1) сначала закрыть WR-02 (с окном возврата) и добиться зелёного CI на ubuntu — это входные ворота; (2) уборщик сервера как чистая функция над базой и файлами с внедрёнными часами в `Core::clean`, запускаемая отдельным потоком раз в несколько минут, плюс уборщик осиротевших папок в `runner::Runner` (в `tick` при простое); (3) сквозной тест в `crates/runner/tests/chain.rs` (сервер не может зависеть от раннера, `crates/server/tests/deps.rs`); (4) документы и заготовки в `docs/deploy/` с Tailscale Serve как основным вариантом и Caddy как запасным; (5) учения — шесть протоколов с папкой улик `.planning/phases/09-…/evidence/NN-имя/` и одним скриптом поиска токена по всем уликам; проводит автор.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Сроки хранения демок и результатов | `server` (`Core::clean`, база) | `bot` (`tg_jobs.delivery` как условие) | Только сервер знает, что файл нужен для `/retry`, `/resend` и доставки |
| Чистка `*.part` и `demo.part` | `server` (`blobs`) | — | `.part` создаёт только `Blobs` (`blobs.rs:135-137`, `blobs.rs:331`) |
| Чистка рабочей папки `telegram-bot-api` | `server` (по возрасту файлов) | запасной вариант в compose (контейнер-уборщик под uid 101) | Сервер уже читает и удаляет оригинал после копирования (`ingest.rs` `OriginalGuard`); остаток убирается по возрасту. Права на удаление не проверены (спайк) |
| Чистка сырых дублей раннера | `runner` (`Runner`, папки `out/`, `remote/` рабочей папки) | `app` (запускает тот же `Runner`) | `remove_finished` и `drop_job` уже живут в `runner`; приложение лишь выставляет `dir = data_dir()/runner` (`app/src/remote/control.rs:174`) |
| Резервные копии БД | `server` (подкоманда `backup` через `VACUUM INTO`) | systemd timer | Копировать файл WAL-базы нельзя (`05-RESEARCH.md:380`); подкоманда не требует `sqlite3` на хосте |
| Доступ раннера к API | прокси (Tailscale Serve или Caddy) | `server` слушает только loopback | `Link` не отдаёт токен по открытому `http://` на внешний адрес; D-03 фазы 5 |
| Запуск и перезапуск сервера | systemd (`Restart=on-failure`) | Docker (`restart: unless-stopped`) для bot-api | `serve` падает, если `getMe` не отвечает (`lib.rs:~121`), systemd перезапускает до готовности bot-api |
| Сквозной тест | `runner/tests` (зависит от `server` как dev-зависимость) | `server::tg::fake::FakeTelegram`, `pipeline::host::fake::FakeHost` | `crates/server/tests/deps.rs` запрещает серверу зависеть от `runner` |
| Учения и улики | человек + скрипты сбора | `cs2-server job show`, `runner.log`, `runner_state.json`, `journalctl` | Реальный CS2, HLAE, Telegram и Linux-хост в CI недоступны |

## Standard Stack

Новых cargo-пакетов фаза не добавляет (всё делается на `std`, `rusqlite`, `chrono` и путевых крейтах). Внешние программы развёртывания перечислены отдельно: это не пакеты реестра, но их имена и флаги проверены ниже.

### Core
| Library / Tool | Version | Purpose | Why Standard |
|----------------|---------|---------|--------------|
| `rusqlite` | 0.32.1 (bundled SQLite, `libsqlite3-sys` 0.30.1) | `VACUUM INTO` для резервной копии; запросы уборщика | Уже в `server`; `VACUUM INTO` — обычный SQL, отдельная feature `backup` не нужна [VERIFIED: Cargo.lock — `rusqlite 0.32.1`, `libsqlite3-sys 0.30.1`; версия самого SQLite в 0.30.1 — [ASSUMED] 3.46, для `VACUUM INTO` нужно ≥ 3.27] |
| `server::clock::Clock` / `ManualClock` | путевой | Виртуальное время уборщика в тестах | Уже используется во всех тестах сервера (`bot_e2e.rs`, `common/mod.rs`) |
| `pipeline::host::fake::FakeHost` | путевой | Раннер без CS2 в сквозном тесте | Уже используется (`runner/tests/exec.rs`, `faceit_flow.rs`) |
| `server::tg::fake::FakeTelegram` | путевой | Подставной Telegram | Есть `put_document`, `document_update`, `text_update`, `texts_to`, `videos`, `get_file_calls`, `files_dir` (`tg/fake.rs:83-231`) |
| `aiogram/telegram-bot-api` (образ Docker) | тег `latest` (других тегов страница не называет) | Локальный Bot API для файлов до 2 ГБ | Образ собирает `tdlib/telegram-bot-api`; работает от пользователя `telegram-bot-api` (uid/gid 101) [CITED: github.com/aiogram/telegram-bot-api, Dockerfile] |
| systemd | системный | Юнит `cs2-server`, таймер резервных копий | `cs2-server` читает `STATE_DIRECTORY` (`config.rs:224`), пишет журнал в stderr для journald (`log.rs:14`) |
| Tailscale Serve | клиент ≥ 1.52 | HTTPS в пределах tailnet к `127.0.0.1:8080` | Единственный вариант, при котором `Link` принимает адрес без правки кода [CITED: tailscale.com/kb/1242/tailscale-serve] |

### Supporting
| Library / Tool | Version | Purpose | When to Use |
|----------------|---------|---------|-------------|
| Caddy 2 | текущая стабильная | HTTPS с публичным сертификатом перед `127.0.0.1:8080` | Если раннеру нельзя в tailnet или нужен публичный адрес; снаружи открыты 80 и 443 [CITED: caddyserver.com/docs/quick-starts/reverse-proxy] |
| `ufw` / `nftables` | системные | Закрыть всё, кроме SSH (и 80/443 для Caddy) | Всегда; но Docker обходит `ufw` для опубликованных портов — публиковать только `127.0.0.1:8081:8081` [ASSUMED: общеизвестное поведение Docker, проверить спайком] |
| `journalctl` | systemd | Выгрузка журнала сервера в улики | Учения |
| `grep -rFa -f файл` | GNU grep (Git Bash тоже) | Поиск подставного токена по текстам и двоичным файлам (БД) без токена в командной строке | Учения, скрипт `scan-token` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Tailscale Serve (основной) | Caddy + домен | Caddy открывает 80/443 в интернет и требует домен и DNS; зато раннер может быть вне tailnet. Оба дают `https://` с системным доверием, которое использует `Link` (`PlatformVerifier`) |
| `tailscale serve`/Caddy | Разрешить в `plain_http_allowed` адреса `100.64.0.0/10` и править `Link` | Правка кода раннера и ослабление проверки; WireGuard шифрует канал, но решение меняет модель угроз (SEC) — не брать без решения автора |
| `cs2-server backup` (встроенная подкоманда) | `sqlite3 server.db ".backup"` по таймеру | Нужен пакет `sqlite3` на хосте и версия клиента не ниже версии базы; встроенная подкоманда тестируется в CI |
| Уборщик в процессе сервера (поток) | `systemd timer` + `find -mtime -delete` | `find` не знает состояний задач и `/resend`; для папки bot-api, где прав может не хватать, `find` под uid 101 в контейнере-спутнике — запасной вариант |
| Отдельная служба Docker для сервера | `cs2-server` под systemd | Решение roadmap: systemd |

**Installation:** на Linux-хосте `apt install docker.io docker-compose-v2 ufw` (или эквивалент дистрибутива) [ASSUMED: имена пакетов зависят от дистрибутива], Tailscale по официальной инструкции. Cargo-пакеты не добавляются.

**Version verification:** `cargo search` не требовался — новых cargo-пакетов нет. Данные об образе взяты со страницы `github.com/aiogram/telegram-bot-api` (тег `latest`, порты 8081 и 8082, пользователь uid 101); закрепление образа по дайджесту — рекомендация, не проверка.

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| — | — | — | — | — | — | Новых cargo-пакетов нет; `server`, `runner`, `wire`, `pipeline` — путевые крейты воркспейса. Образ `aiogram/telegram-bot-api` — не пакет реестра cargo, его происхождение (Docker-сборка `tdlib/telegram-bot-api`) подтверждено страницей репозитория, но дайджест образа планировщик закрепляет на этапе развёртывания |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```text
 друг ──(демка/ссылка)──> Telegram ──> telegram-bot-api (docker, 127.0.0.1:8081, --local)
                                           │ getFile: абсолютный путь в /var/lib/telegram-bot-api/<токен>/documents/file_N.dem
                                           v
 ┌─────────────── Linux-хост ────────────────────────────────────────────────────────────┐
 │ cs2-server (systemd, 127.0.0.1:8080)                                                  │
 │   поток bot-poll/ingest ── копия в blobs/incoming/<id>/demo(.part) ── удаляет оригинал │
 │   HTTP для раннеров ── claim/hello/progress/PUT результата кусками ── blobs/results/<id>/<эпоха>.part │
 │   поток sweep (5 с): аренды, зависший приём                                           │
 │   НОВОЕ: поток clean (раз в N мин): сроки по состояниям, .part, сироты, папка bot-api  │
 │   server.db (WAL)  <── НОВОЕ: cs2-server backup (VACUUM INTO) по systemd timer         │
 │   bot-deliver ── sendVideo file:///…/blobs/results/<id>/highlights.mp4 (bot-api читает) │
 └──────────────▲────────────────────────────────────────────────────────────────────────┘
                │ tailscale serve --https=443 → http://127.0.0.1:8080   (или Caddy :443)
                │ HTTPS, токен раннера в заголовке
 ┌──────────────┴──── Windows, машина автора ────────────────────────────────────────────┐
 │ cs2cinema.exe (режим раннера) или cs2-cli runner                                      │
 │   Runner::tick: hello(holding) → claim → скачать демку → Executor (HLAE+CS2) → PUT → finish │
 │   runner_state.json (outbox), runner.log, remote/srv<id>.dem, out/srv<id>/…           │
 │   НОВОЕ: уборка сирот out/srv*, remote/srv* по возрасту и по номеру удерживаемой задачи │
 └───────────────────────────────────────────────────────────────────────────────────────┘
 CI (GitHub, ubuntu-latest + windows-latest): cargo test --workspace; НОВОЕ: отдельный job «chain» с runner/tests/chain.rs
```

### Recommended Project Structure
```
crates/server/src/clean.rs        # НОВОЕ: правила уборки (чистая часть) + Core::clean; тесты на ManualClock
crates/server/src/config.rs       # новые ключи retention_* (5 мест: Config, Default, FileConfig, env, validate, config show, example json)
crates/server/src/lib.rs          # spawn_cleaner рядом со spawn_sweeper (lib.rs:178)
crates/server/src/cli.rs          # подкоманды `backup {папка}` и `clean [--dry-run]`
crates/server/tests/cleanup.rs    # НОВОЕ: уборщик на TestServer + ManualClock
crates/runner/src/sweep.rs        # НОВОЕ: уборка сирот в рабочей папке раннера + ротация runner.log
crates/runner/tests/sweep.rs      # НОВОЕ
crates/runner/tests/chain.rs      # НОВОЕ: сквозной тест (успех и сбой) на сервере с ботом и Runner на FakeHost
crates/runner/tests/common/mod.rs # Stand::start_with_bot(tg)
docs/deploy/README.md             # НОВОЕ: порядок развёртывания, проверки «снаружи только порт», перезагрузка, откат
docs/deploy/docker-compose.yml, cs2-server.service, cs2-server-backup.{service,timer}, Caddyfile, cs2-server.json (образец)
docs/drills/…                     # НОВОЕ: протоколы шести учений, скрипты сбора улик и поиска токена
.planning/phases/09-…/evidence/NN-имя/   # улики по каждому учению (только текст)
.github/workflows/ci.yml          # job chain (ubuntu), timeout-minutes, concurrency
```

### Pattern 1: Двухшаговое удаление через `files.deleted_at`
**What:** уборщик сначала в транзакции `BEGIN IMMEDIATE` ставит `deleted_at = now` у строки `files` (после этого `demo_by_job`/`result_by_job` её не видят, `submit_from`/`/retry` честно отвечают «демки больше нет»), потом вне транзакции удаляет файл (`std::fs::remove_file`) и пустую папку задачи. Сбой между шагами оставляет файл-сироту без живой строки — его подбирает проход «сироты» (каталоги `blobs/incoming/<id>` и `blobs/results/<id>`, у которых нет живой строки `files`).
**When to use:** любое удаление файла, на который ссылается база. Мьютекс базы не держать во время файловых операций (правило `db.rs` шапка).
**Example:**
```rust
// Source: набросок по crates/server/src/core.rs (Core::sweep, 1191) и blobs.rs (discard_parts, 311)
pub struct Retention { pub demo_ms: i64, pub failed_demo_ms: i64, pub result_ms: i64, pub unsent_result_ms: i64,
                       pub part_idle_ms: i64, pub botapi_ms: i64 }
impl Core {
    /// Один проход уборки; `now` берётся из `Clock`, поэтому тест двигает часы, а не ждёт.
    pub fn clean(&self, r: &Retention, botapi_root: Option<&Path>) -> Result<Cleaned> { /* см. таблицу правил */ }
}
```

### Pattern 2: Правила уборки по состояниям (рекомендуемая таблица)
Значения состояний: `jobs.state` — `'receiving','queued','assigned','running','uploading','done','failed','cancelled'` [VERIFIED: crates/server/src/db.rs, CHECK(state IN ('receiving','queued','assigned','running','uploading','done','failed','cancelled'))]; `tg_jobs.delivery` — `'none','pending','sending','sent','uncertain','failed'` [VERIFIED: crates/server/src/db.rs, CHECK(delivery IN ('none','pending','sending','sent','uncertain','failed'))].

| Что | Удалять, когда | Никогда, пока |
|-----|----------------|---------------|
| Демка `blobs/incoming/<id>/demo` | задача `done`/`cancelled` и `finished_at <= now − demo_ms`; `failed` — через `failed_demo_ms` (для `/retry`) | задача не окончательная (`receiving…uploading`) |
| Результат `blobs/results/<id>/highlights.mp4` | `done` и (доставлен: `tg_jobs.delivery = 'sent'` и `result_file_id IS NOT NULL` и `updated_at <= now − result_ms`) либо (нет строки `tg_jobs`, задача из `cs2-server submit`, и `finished_at <= now − unsent_result_ms`) либо (`delivery = 'failed'` и `finished_at <= now − unsent_result_ms`) | `delivery IN ('pending','sending','uncertain')` — `/resend` из файла ещё может понадобиться (`deliver.rs:67-92`) |
| `*.part` результата | задача окончательная (`discard_parts`); либо эпоха ≠ `jobs.epoch` и mtime старше `part_idle_ms`; либо любая `.part` старше 24 ч у неактивной загрузки | идёт загрузка этой пары `(задача, эпоха)` (`Blobs::lock_upload`, `blobs.rs:154`) |
| `incoming/<id>/demo.part` | mtime старше `RECEIVING_STALE_MS` (`rules.rs`) и задача не `receiving` | задача `receiving` и моложе порога |
| Сирота: папка `incoming/<id>` или `results/<id>` без живой строки `files` и без активной задачи | старше `part_idle_ms` | задача с этим `id` активна |
| Файл в рабочей папке bot-api (`bot_api_files_root_local`) | обычный файл (не ссылка), mtime старше `botapi_ms`, лежит в подпапке `documents/` (при необходимости `videos/`, `temp/`) | моложе `max(2 × bot_transfer_timeout_mins, 120 мин)` — приём большой демки может идти минутами |

Удалять каталоги внутри рабочей папки bot-api не нужно: процесс bot-api ждёт свои подпапки [ASSUMED: раскладка `<dir>/<токен>/documents/…` и поведение при удалении подпапок не проверены].

### Pattern 3: Окно возврата для WR-02
**What:** `claim_report` (`core.rs:684`) отбрасывает задачи с `grants >= max_grants`. Исправление по 05-REVIEW (`AND lease_lost = 0`) оставляет задачу с потерянной арендой на последней выдаче в `queued` навсегда (выбор следующей задачи требует `grants < max`, `core.rs:716`). Нужно: «ядовитыми» считать `grants >= max AND (lease_lost = 0 OR updated_at <= now − (lease_ttl + startup_grace))`. Пока окно не вышло, задача ждёт возврата своего раннера (`hello` → `Keep`); потом падает как `TOO_MANY_GRANTS` с тем же текстом.
**Test:** `lease_http`-стиль на `ManualClock`: `grants == max`, аренда истекла → чужой `claim` не роняет задачу; тот же раннер возвращает её через `hello`; без возврата после окна — `failed/TOO_MANY_GRANTS`.

### Pattern 4: Сквозной тест как один интеграционный файл в `runner/tests`
**What:** `crates/runner/tests/chain.rs`: `serve_with_bot` (потоки бота настоящие, часы `ManualClock` для правок статуса), `FakeTelegram`, `Runner` с `PipelineExecutor` на `FakeHost`; демка приходит «сообщением» (`put_document` + `document_update`), раннер доводит задачу, ролик появляется в `tg.videos()`; второй тест — сбой предпроверки (`BAD_DEMO_REPLY` из `exec.rs`) и фраза другу плюс отчёт админу.
**Why here:** `crates/server/tests/deps.rs` (его `WORKSPACE_CRATES` содержит `runner`) запрещает серверу зависеть от `runner`, в том числе в dev-зависимостях; а `runner` уже имеет `server` как dev-зависимость (`crates/runner/Cargo.toml`).
**Existing pieces to reuse:** `Stand` (`runner/tests/common/mod.rs:44`), цепочка FACEIT `faceit_flow.rs` (`faceit_link_from_a_friend_is_downloaded_by_the_runner_and_delivered`, шаги бота вручную `bot.step()`), бот-потоки без раннера `server/tests/bot_e2e.rs:16` и `:230` (раннер там сыгран сырыми HTTP-вызовами).
**What `FakeHost` cannot do:** настоящий разбор демки (дочерний `cs2-cli` подменён ответом `OK_REPLY`), запись CS2, ffmpeg. Ролик подставляет `with_job_fn` (MP4 из боксов `ftyp/moov/mdat`), `ffprobe` отвечает `GOOD_PROBE`. Это проверка транспорта, статуса, отмены и доставки, а не монтажа; монтаж проверяет `crates/cli/tests/pipeline.rs`.

### Anti-Patterns to Avoid
- **Удалять файл, а потом метить строку:** окно, в котором `/retry` видит строку и падает на `link_demo`; метить первым.
- **Один `DELETE` по возрасту без состояний:** `done` с `delivery = 'uncertain'` теряет единственную копию; без `result_file_id` у `sent` — тоже.
- **Чистить папку bot-api по шаблону вне корня:** корень `bot_api_files_root_local` задаётся админом; перед удалением канонизировать путь, проверить `starts_with(корень)` и `symlink_metadata().is_file()` (как `safe_local_path`, `ingest.rs:466`).
- **Считать место свободным по числу ссылок:** у демки бывает несколько жёстких ссылок (`link_demo`, `blobs.rs:353`); уборщик удаляет путь задачи, а не содержимое.
- **Токен в аргументах скрипта:** скрипт поиска берёт образцы из файла (`grep -f`), а не из командной строки (она видна в списке процессов и в журнале shell).
- **`DynamicUser=yes` для `cs2-server`:** `/var/lib/cs2-server` тогда ссылка на `/var/lib/private/…` с правами 0700; контейнер bot-api (uid 101) не прочитает `blobs/results` для `file://` [ASSUMED: поведение systemd `DynamicUser` + `StateDirectory`, проверить на хосте]. Использовать обычного системного пользователя `cs2-server`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Резервная копия работающей WAL-базы | `cp server.db` | `VACUUM INTO 'файл'` (подкоманда `cs2-server backup`) | Копия файла без `-wal` даёт неконсистентную базу; решение зафиксировано в `05-RESEARCH.md:380` |
| Виртуальное время в тестах уборщика | `thread::sleep` на сутки | `ManualClock` + `File::set_modified` для mtime | В репозитории так устроены все тесты сроков |
| Определение «чужой/свой» путь в папке bot-api | свой разбор путей | `safe_local_path`-подобная проверка (канонизация, `starts_with`, не ссылка) | Уже отлажено в `ingest.rs:466-500` |
| Поиск токена в БД и журналах | самописный сканер | `grep -rFa -f образцы папка_улик` | Находит и в двоичной БД (`-a`), не печатает токен |
| TLS для раннера | свой TLS в `cs2-server` | Tailscale Serve или Caddy | Решение D-03 фазы 5 |
| Перезапуск и автозапуск | свой supervisor | systemd `Restart=on-failure`, `docker restart: unless-stopped` | Стандарт |
| Ограничение размера журнала Docker | cron с `truncate` | `logging: json-file` с `max-size`/`max-file` в compose | Встроено в Docker |

**Key insight:** в этой фазе почти всё «умное» уже есть в системе (двухфазные состояния, `deleted_at`, `OriginalGuard`, `remove_finished`, `--upload-chunk-limit`, `Core::recover`); не хватает только планового обхода, сроков и документов. Любая новая логика состояний — повод остановиться и найти существующий механизм.

## Runtime State Inventory

Фаза не переименовывает и не мигрирует, но уборка и развёртывание меняют живое состояние, поэтому ответы по категориям:

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | Сервер: `server.db` (WAL; таблицы `users`, `runners`, `jobs`, `files`, `tg_*`), версия схемы 3 (`db.rs`, три шага в `MIGRATIONS`). Раннер: `runner_state.json` с `job_id`, `epoch`, этапом, `out_dir`. База приложения `cs2cinema.db` хранит `out_dir` карточки сервера | Новых миграций не требуется (колонки `files.deleted_at` и `jobs.finished_at` уже есть). Если сроки потребуют индексов — добавить шагом 4 только `CREATE INDEX` |
| Live service config | `telegram-bot-api`: рабочая папка с подпапкой на токен (вид `<id>~<секрет>` или `<id>:<секрет>`), binlog, база; Tailscale Serve (постоянная конфигурация с `--bg`); `crontab`/timer отсутствуют | Задокументировать; `tailscale serve status` в проверке после перезагрузки |
| OS-registered state | Нет: ни юнитов, ни планировщика ещё не регистрировалось | Юнит `cs2-server.service` и таймер резервных копий создаются документом; проверка `systemctl is-enabled` |
| Secrets/env vars | `TELEGRAM_BOT_TOKEN`, токен раннера (`runner add`, хранится хешем), `FACEIT_API_KEY` (только у раннера), `TELEGRAM_API_ID/HASH` для bot-api | Хранить в `EnvironmentFile` с правами 0600 и в `.env` compose с правами 0600; учения ищут подставной токен |
| Build artifacts | Артефакт CI `cs2-server-linux-x64` собирается на `ubuntu-latest`; нужная версия glibc зависит от образа | Указать в документе минимальный glibc или собирать на целевом хосте |

## Common Pitfalls

### Pitfall 1: CI на Linux ни разу не запускался с кодом версии 1.1
**What goes wrong:** первый push 365 коммитов даёт красный job `test (ubuntu-latest)`: тесты `app` (модуль `remote`), `runner`, `server` и `pipeline` могли опираться на Windows.
**Why it happens:** `origin/main` отстаёт (`git rev-list --count origin/main..HEAD` = 365); последние запуски на GitHub — 2026-10-09 (коммиты `d74fd6f`, `6f4c6b1`, оба success, workflow без шага сборки сервера) [VERIFIED: api.github.com/repos/Kamaliev/cs2cinema/actions/runs]. Есть единственные явные `cfg(windows)` в `crates/server/src/bot/ingest.rs:958`, `crates/server/tests/bot_ingest.rs:455`, `crates/app/src/remote/token.rs:87,107,173,194` — остальное проверено только на Windows.
**How to avoid:** первым планом фазы поставить `checkpoint:human-action` «push в ветку и просмотр результата job `test (ubuntu-latest)`» (push — действие автора). Чинить красное до остальной работы. Локальной проверки нет: на машине нет цели Linux (`rustup target list --installed` показывает только `x86_64-pc-windows-msvc`) и нет дистрибутива WSL.
**Warning signs:** тесты с путями `\\`, `cmd`, `C:\`, права `icacls`, `File::try_lock`.

### Pitfall 2: раннер не примет `http://100.x.y.z`
**What goes wrong:** автор настраивает `--server http://100.101.102.103:8080` по привычке Tailscale; `Link::new` отвечает «токен нельзя слать по открытому http:// на внешний адрес: используйте https://» (`link.rs:161-166`).
**Why it happens:** `plain_http_allowed` (`link.rs:129-148`) пропускает `localhost`, loopback, RFC1918 и link-local; адреса 100.64.0.0/10 не входят.
**How to avoid:** `tailscale serve --bg --https=443 localhost:8080` (синтаксис из документации Tailscale; точная форма `--bg 8080` на странице не приведена) и адрес раннера `https://<узел>.<tailnet>.ts.net`; в tailnet должны быть включены MagicDNS и HTTPS-сертификаты [ASSUMED: условия включения проверяются в админ-консоли Tailscale]. Либо Caddy с доменом.

### Pitfall 3: `serve` падает, пока bot-api не поднялся
**What goes wrong:** после перезагрузки `cs2-server` стартует раньше контейнера, `getMe` не отвечает, процесс завершается («бот не запустился: локальный telegram-bot-api не отвечает», `lib.rs`), systemd бросает попытки.
**How to avoid:** `Restart=on-failure`, `RestartSec=5`, `StartLimitIntervalSec=0`, `After=docker.service`, `Wants=docker.service`; проверка перезагрузкой в документе.

### Pitfall 4: права на файлы bot-api
**What goes wrong:** сервер читает входящий файл, но не удаляет (`OriginalGuard` молча теряет ошибку), рабочая папка растёт; либо bot-api не читает `blobs/results/<id>/highlights.mp4` по `file://` и ролики не отправляются.
**Why it happens:** контейнер работает как uid/gid 101 (`telegram-bot-api`), сервер — как другой пользователь; права на создаваемые bot-api подпапки неизвестны.
**How to avoid:** спайк на живом стенде (из `07-UAT.md`, тест 3) раньше документа: создать файл через бота, проверить `ls -l`, попытаться удалить как `cs2-server`. Если не выходит — запасной вариант в compose (контейнер-уборщик под тем же uid) или общая группа с `setgid`. Результаты читаются bot-api: `blobs/results` смонтировать только на чтение, проверить `UMask=0022` у сервера.

### Pitfall 5: Docker обходит `ufw`
**What goes wrong:** `ports: "8081:8081"` открывает bot-api в интернет, хотя `ufw` его «закрыл».
**How to avoid:** `127.0.0.1:8081:8081`; проверка «снаружи открыт только задуманный порт» — `ss -tlnp` на хосте и `nmap -Pn -p- <публичный адрес>` с другой машины [ASSUMED: поведение Docker/iptables общеизвестно, закрепить проверкой].

### Pitfall 6: артефакт CI не запускается на хосте из-за glibc
**What goes wrong:** `cs2-server-linux-x64` собран на `ubuntu-latest` (по умолчанию новейший LTS), а на хосте старый Debian/Ubuntu: «version GLIBC_2.xx not found».
**How to avoid:** в документе указать минимальный дистрибутив либо собирать на целевом хосте (`cargo build --release -p server`), либо собирать артефакт на более старом образе (`ubuntu-22.04`, [ASSUMED]: доступность метки в октябре 2026 не проверена).

### Pitfall 7: уборка раньше `/retry` и `/resend`
**What goes wrong:** друг через три дня жмёт `/resend` или `/retry`; ролика или демки уже нет.
**How to avoid:** сроки конфигурируются, значения по умолчанию называют автор; сообщения при отсутствии файла уже есть (`submit_from`, `deliver_once`), но тест на каждое должен подтверждать понятный текст без путей.

### Pitfall 8: «улики» без улик
**What goes wrong:** учение пройдено «со слов» (как `06-UAT.md`).
**How to avoid:** протокол учения заканчивается шагом «сохранить» и проверкой скриптом, что файлы улик существуют и непусты; закрытие фазы без папки `evidence/NN-…` невозможно (проверка в плане верификации).

### Pitfall 9: токен бота в журнале bot-api
**What goes wrong:** процесс `telegram-bot-api` пишет в свой журнал адреса запросов `/bot<токен>/…`; поиск токена по «журналам» находит его там [ASSUMED: зависит от уровня подробности, `TELEGRAM_VERBOSITY`].
**How to avoid:** в поиск входит `docker compose logs telegram-bot-api`; если найдено — понизить `TELEGRAM_VERBOSITY` и/или направить журнал в файл с правами 0600 (`TELEGRAM_LOG_FILE`).

## Code Examples

### Минимальный `docker-compose.yml` для bot-api (заготовка, не проверена на живом хосте)
```yaml
# Source: https://github.com/aiogram/telegram-bot-api (переменные окружения и uid 101); пути и права — решение спайка
services:
  telegram-bot-api:
    image: aiogram/telegram-bot-api:latest   # закрепить дайджестом после первого успешного запуска
    restart: unless-stopped
    env_file: .env                           # TELEGRAM_API_ID, TELEGRAM_API_HASH; права 0600
    environment:
      TELEGRAM_LOCAL: "1"
      TELEGRAM_VERBOSITY: "1"
    ports:
      - "127.0.0.1:8081:8081"                # только loopback, иначе Docker обойдёт ufw
    volumes:
      - /var/lib/telegram-bot-api:/var/lib/telegram-bot-api
      - /var/lib/cs2-server/blobs/results:/var/lib/cs2-server/blobs/results:ro   # file:// для sendVideo
    logging:
      driver: json-file
      options: { max-size: "10m", max-file: "3" }
```
Путь `/var/lib/telegram-bot-api` совпадает с рабочей папкой образа [CITED: github.com/aiogram/telegram-bot-api]; одинаковые пути на хосте и в контейнере делают отображение пустым: `bot_api_files_root = /var/lib/telegram-bot-api`, `bot_api_files_root_local` не нужен, `bot_api_results_prefix` по умолчанию равен `data_dir/blobs/results` (`bot/mod.rs:84-95`).

### Заготовка юнита `cs2-server.service`
```ini
# Source: заготовка; параметры StateDirectory/EnvironmentFile — стандартные средства systemd [ASSUMED до проверки на хосте]
[Unit]
Description=cs2-server (очередь монтажа)
After=network-online.target docker.service
Wants=network-online.target docker.service
StartLimitIntervalSec=0

[Service]
User=cs2-server
Group=cs2-server
StateDirectory=cs2-server
StateDirectoryMode=0755
UMask=0022
EnvironmentFile=/etc/cs2-server/env           # TELEGRAM_BOT_TOKEN=…, права 0600
ExecStart=/usr/local/bin/cs2-server serve --config /etc/cs2-server/cs2-server.json
Restart=on-failure
RestartSec=5
TimeoutStopSec=15
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=/var/lib/telegram-bot-api

[Install]
WantedBy=multi-user.target
```
`ctrlc` с feature `termination` обрабатывает SIGTERM (`crates/server/Cargo.toml`: `ctrlc = { version = "3.5", features = ["termination"] }`), остановка сервера с ботом укладывается в 5 с (тест `serve_starts_the_bot_against_a_local_bot_api_and_stops_quickly`, `bot_e2e.rs:144`).

### Резервная копия
```bash
# Source: заготовка; VACUUM INTO — SQL SQLite ≥ 3.27, подкоманда cs2-server backup — НОВАЯ работа этой фазы
cs2-server backup /var/backups/cs2-server        # создаёт server-YYYYmmdd-HHMM.db, не перезаписывает, оставляет N последних
# восстановление: systemctl stop cs2-server; cp копия /var/lib/cs2-server/server.db; rm -f server.db-wal server.db-shm; systemctl start cs2-server
```
Блобы (демки, результаты) в копию не входят: они воспроизводимы или отдаются уборщиком по срокам. В копию входят `server.db`, `/etc/cs2-server/*`, но не токен в открытом виде в публичное место.

### Ворота CI для сквозного теста
```yaml
# Source: расширение .github/workflows/ci.yml (текущий файл прочитан целиком в этой сессии)
  chain:
    name: chain (ubuntu)
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --release -p server --test deps
      - run: cargo test --release -p runner --test chain -- --nocapture
```
Что в `ci.yml` уже есть: матрица `os: [ubuntu-latest, windows-latest]`, шаги `cargo test --release --workspace`, `cargo build --release -p cli`, `cargo build --release -p server`, выгрузка `cs2-server-linux-x64` (только Linux, `if-no-files-found: error`) и `cs2-cli-windows-x64` [VERIFIED: .github/workflows/ci.yml]. Чего нет: отдельного именованного сквозного job, `timeout-minutes`, `concurrency`, проверки охраны зависимостей отдельной строкой.

### Рецепт поиска подставного токена (улики одной папкой)
```bash
# Source: заготовка. Образцы в файле (токен не попадает в историю команд): целиком, часть до ':' не нужна, но нужны обе формы ':' и '~'
# patterns.txt: ТОКЕН (с двоеточием), ТОКЕН с '~' вместо ':', секретная часть после ':'
grep -rFa -f patterns.txt evidence/NN-имя/ && echo "НАЙДЕНО (провал)" || echo "чисто"
# что должно лежать в evidence/NN-имя/: server-journal.txt (journalctl -u cs2-server -o short-iso),
# botapi-compose-logs.txt, runner.log, runner_state.json (до/после), job-show.txt (cs2-server job show N),
# db-dump.sql (sqlite3 .dump или копия server.db для grep -a), chat-export/ (экспорт Telegram Desktop, JSON)
```
Тесты `bot_token_in_tilde_form_is_masked_everywhere` и `the_check_itself_catches_a_leak` (`crates/server/tests/secrets.rs:598`, `:477`) покрывают подставные секреты в коде; живые журналы стенда эта проверка не заменяет.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Копировать файл SQLite для бэкапа | `VACUUM INTO` / Online Backup API | SQLite 3.27 (2019) | Консистентная копия без остановки сервера |
| `docker-compose.yml` с ключом `version:` | Compose Specification (ключ игнорируется) | Docker Compose v2 | В заготовках ключ не нужен |
| Tailscale `serve` в режиме переднего плана | `--bg` и постоянная конфигурация, CLI изменён в клиенте 1.52 | Tailscale 1.52 | Команды из старых заметок могут не работать [CITED: tailscale.com/kb/1242/tailscale-serve] |

**Deprecated/outdated:** облачный Bot API для файлов > 50 МБ: нужен локальный сервер (`--local`, загрузка до 2000 МБ, выдача абсолютного `file_path`) [CITED: github.com/tdlib/telegram-bot-api, README].

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Сроки хранения по умолчанию: демка `done`/`cancelled` 24 ч, `failed` 72 ч, результат 24 ч после `sent`, не доставленный/без Telegram 7 суток, `.part` 6 ч бездействия, bot-api 6 ч | Pattern 2 | Критерий «после дня работы место возвращается» не сойдётся или друг не успеет `/retry`; нужно подтверждение автора |
| A2 | Раскладка bot-api: `<dir>/<токен>/documents/file_N.ext`; уборщику достаточно `documents/` (+`videos/`, `temp/`) | Pattern 2, Pitfall 4 | Удалим не то или не найдём файлы; решает спайк 07-UAT тест 3 |
| A3 | Файлы bot-api создаются с правами, при которых пользователь сервера может удалять | Pitfall 4 | Уборщик сервера бессилен; нужен запасной контейнер-уборщик |
| A4 | Docker обходит `ufw` для опубликованных портов | Pitfall 5 | Лишний открытый порт; закрывается публикацией на 127.0.0.1 и проверкой `nmap` |
| A5 | `DynamicUser` + `StateDirectory` делает каталог недоступным контейнеру | Anti-Patterns | Лишнее ограничение документа; безвредно |
| A6 | В tailnet нужно включить MagicDNS и HTTPS-сертификаты для `tailscale serve --https` | Pitfall 2 | Шаг документа пропущен, `serve` не выдаёт сертификат |
| A7 | `ubuntu-22.04` как метка раннера доступна, `ubuntu-latest` даёт слишком новый glibc | Pitfall 6 | Артефакт не запускается на старом хосте; запасной путь — сборка на хосте |
| A8 | Поведение `telegram-bot-api` при уровне подробности > 1: адреса с токеном попадают в журнал | Pitfall 9 | Поиск токена найдёт его в журнале bot-api |
| A9 | `libsqlite3-sys` 0.30.1 несёт SQLite ≥ 3.27 | Standard Stack | `VACUUM INTO` не сработает; тест подкоманды `backup` обнаружит сразу |
| A10 | Образ `aiogram/telegram-bot-api` задаёт `TELEGRAM_WORK_DIR=/var/lib/telegram-bot-api`, `TELEGRAM_TEMP_DIR=/tmp/telegram-bot-api` по умолчанию (энтрипойнт требует обе переменные) | Code Examples | Контейнер не стартует; потребуется указать их в compose |
| A11 | У автора есть чистая Linux-машина (VPS или домашний сервер) | Summary | Критерии 2 и 4 не проверить; в репозитории хост нигде не назван |
| A12 | Загрузка `sendVideo` по `file://` не копирует файл в рабочую папку bot-api | Pattern 2 | Рабочая папка bot-api растёт за счёт копий роликов; спайк 07-UAT тест 2/3 |

## Open Questions

1. **Какая Linux-машина и кто её готовит?**
   - What we know: сервер «на чистой Linux-машине», WSL без дистрибутива, Docker/Tailscale/Caddy на этой машине нет.
   - What's unclear: адрес, дистрибутив, есть ли доменное имя (для Caddy), есть ли Tailscale-аккаунт.
   - Recommendation: первый `checkpoint:human-action` фазы — автор называет хост и вариант доступа; если хоста нет, предварительный прогон документа в WSL2 Ubuntu с systemd (`wsl --install -d Ubuntu`) даёт частичную репетицию (без проверки открытых портов извне).

2. **Сроки хранения (A1).** Автор подтверждает или меняет значения; они выносятся в настройки сервера и в `config show`.

3. **Права на папку bot-api (A3).** Решается живым спайком до окончательного текста документа; в плане предусмотреть две ветки (серверный уборщик / контейнер-уборщик).

4. **Что делать с WR-03 фазы 5 (медленные клиенты, `http.rs`).** Предлагается закрыть документом (прокси с таймаутами, сервер на loopback); правка кода вне требований фазы. Нужен явный выбор автора.

5. **Живой стенд из 07-UAT и 08-UAT тест 5.** Считать «pass» без улик недостоверным и повторить в фазе 9 с уликами (рекомендуется), либо принять со слов автора.

6. **Проверка свободного места до `getFile`** (Open Question 4 из `07-RESEARCH.md`): требует системного вызова (нового крейта) — вне OPS-01; предлагается отложить, заменить на мониторинг `df` в документе.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Cargo/Rust | сборка и тесты | ✓ | rustc 1.99.0 | — |
| Цель Linux в rustup | локальная проверка сборки сервера | ✗ | только `x86_64-pc-windows-msvc` | проверка только в CI |
| WSL с дистрибутивом | репетиция документа | ✗ | команда `wsl -l -v` сообщает, что дистрибутивов нет | `wsl --install -d Ubuntu` (действие автора) или настоящий хост |
| Docker | репетиция compose | ✗ | `which docker` пуст | настоящий хост |
| Caddy, Tailscale, sqlite3 (CLI) | развёртывание и проверка | ✗ | `which` пуст | настоящий хост; подкоманда `backup` не требует `sqlite3` |
| `gh` (GitHub CLI) | чтение результатов CI | ✗ | не установлен | публичный `api.github.com/repos/Kamaliev/cs2cinema/actions/runs` (работает с curl) |
| ffmpeg | тесты `pipeline` | ✓ | в PATH (WinGet, 9.0.2) | — |
| Git Bash `grep -rFa -f` | скрипт поиска токена на Windows | ✓ | GNU grep Git Bash [ASSUMED] | PowerShell `Select-String -SimpleMatch` |

**Missing dependencies with no fallback:**
- Настоящий Linux-хост с Docker и (Tailscale или домен для Caddy) — блокирует критерии 2 и 4, т. е. все `checkpoint:human-*` плана.

**Missing dependencies with fallback:**
- Локальная сборка под Linux — заменяется job `test (ubuntu-latest)` после push.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | встроенный `cargo test` (интеграционные тесты в `crates/*/tests/*.rs`, модульные в `mod tests`) |
| Config file | нет; `Cargo.toml` воркспейса, `[patch.crates-io] tiny_http = { path = "vendor/tiny_http" }` |
| Quick run command | `cargo test --release -p server --test cleanup` / `cargo test --release -p runner --test sweep` |
| Full suite command | `cargo test --release --workspace` (в 07-VERIFICATION: 68 строк `test result: ok`, 876 пройдено) |

Запускать по пакетам, не весь воркспейс на каждый коммит. Известный нестабильный тест `crates/host/tests/win_session.rs::stand_in_launcher_game_and_console_flow_through_one_session` падает по таймауту 5 с при параллельном полном запуске и проходит отдельно: перезапустить один раз, прежде чем считать ворота красными [VERIFIED: память проекта gsd-execute-phase-quirks].

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| OPS-01 | Демка, результат, `.part`, сироты удаляются по срокам; активные и недоставленные не трогаются | integration (ManualClock) | `cargo test --release -p server --test cleanup` | ❌ Wave 0 |
| OPS-01 | Папка bot-api: удаляются только старые обычные файлы внутри корня, ссылки и чужие пути нетронуты | integration | `cargo test --release -p server --test cleanup bot_api` | ❌ Wave 0 |
| OPS-01 | После удаления `/retry` и `/resend` дают понятный текст без путей | integration | `cargo test --release -p server --test cleanup retry_and_resend` | ❌ Wave 0 |
| OPS-01 | Раннер: сироты `out/srv*`, `remote/srv*` убираются, удерживаемая задача нетронута, `runner.log` ротируется | integration | `cargo test --release -p runner --test sweep` | ❌ Wave 0 |
| OPS-01 | Подтверждённый результат по-прежнему освобождает диск | integration | `cargo test --release -p runner --test flow` | ✅ |
| OPS-01 | Настройки сроков: валидация, `config show`, пример JSON | unit | `cargo test --release -p server config` | ✅ (дополнить) |
| inherited | WR-02: потерянная аренда на последней выдаче не роняет задачу в окне возврата и роняет после него | integration | `cargo test --release -p server --test lease_http wr02` | ❌ Wave 0 |
| OPS-02 | Подкоманда `backup`: консистентная копия, отказ перезаписи, отбор N последних | integration | `cargo test --release -p server --test operator backup` | ❌ Wave 0 |
| OPS-02 | Пример JSON разбирается, ключи известны | unit | `cargo test --release -p server config` | ✅ |
| OPS-02 | Проверка документа на чистой машине | manual | чек-лист `docs/deploy/README.md` | ❌ Wave 0 (человек) |
| OPS-03 | Демка сообщением → ролик в чате (успех) | integration | `cargo test --release -p runner --test chain` | ❌ Wave 0 |
| OPS-03 | Сбой предпроверки → фраза другу и отчёт админу, без путей | integration | `cargo test --release -p runner --test chain` | ❌ Wave 0 |
| OPS-03 | Сервер собирается на Linux, охрана зависимостей | CI | job `test (ubuntu-latest)` + `cargo test --release -p server --test deps` | ✅ (первый запуск не был) |
| OPS-04 | Репетиция учений на фейках: убийство раннера, обрыв загрузки, перезапуск сервера, отмена при офлайн-раннере, несовпадение версий | integration | `cargo test --release -p app --bin cs2cinema remote::e2e`, `cargo test --release -p runner --test outbox`, `-p server --test lease_http` | ✅ |
| OPS-04 | Живые учения с уликами | manual | `docs/drills/*` + `evidence/` | ❌ Wave 0 (человек) |
| regression | CLI и очередь приложения без изменений | integration | `cargo test --release -p cli --test pipeline`, `cargo test --release -p app --bin cs2cinema queue` | ✅ |

### Sampling Rate
- **Per task commit:** `cargo test --release -p <изменённый крейт>` (server, runner) плюс профильный файл тестов.
- **Per wave merge:** `cargo test --release -p server -p runner -p wire` и `cargo test --release -p cli --test pipeline`.
- **Phase gate:** `cargo test --release --workspace` зелёный локально на Windows и job `test (ubuntu-latest)` зелёный в CI, затем `/gsd-verify-work`.

### Wave 0 Gaps
- [ ] `crates/server/tests/cleanup.rs` — OPS-01 (сервер)
- [ ] `crates/runner/tests/sweep.rs` — OPS-01 (раннер)
- [ ] `crates/runner/tests/chain.rs` и `Stand::start_with_bot` в `crates/runner/tests/common/mod.rs` — OPS-03
- [ ] регрессионный тест WR-02 в `crates/server/tests/lease_http.rs` (или `core_states.rs`)
- [ ] тест подкоманды `backup` в `crates/server/tests/operator.rs`
- [ ] `docs/drills/` — протоколы, скрипты сбора улик и поиска токена; `evidence/` — пустые каталоги не создавать заранее

## Security Domain

`security_enforcement` явно не отключён в `.planning/config.json` по информации оркестратора; раздел включён.

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | да | Токены раннеров хранятся хешем (`runners.token_hash`), сравнение уже реализовано в `auth.rs`; в фазе — только выдача и ротация по документу (`runner add/revoke`) |
| V3 Session Management | нет | Сессий нет, только токен раннера |
| V4 Access Control | да | Белый список пользователей бота (фаза 7); уборщик не меняет права |
| V5 Input Validation | да | Пути уборщика строятся из целых `id` (`blobs.rs` шапка); папка bot-api — канонизация, `starts_with`, `symlink_metadata` |
| V6 Cryptography | да (транспорт) | TLS снаружи (Tailscale/Caddy); токены раннера не слать по `http://` вне частной сети (`link.rs`) |
| V7 Error Handling and Logging | да (SEC-04) | `Redactor`, журнал в stderr/journald; учения ищут токен по журналам, БД, чату |
| V12 Files and Resources | да | Ограничение удаления корнем; права 0600 на `.env`/`env`/конфиг; `NoNewPrivileges`, `ProtectSystem=strict` |
| V14 Configuration | да | Файл настроек читается с `deny_unknown_fields` и лимитом 64 КиБ; новые ключи добавлять во все пять мест |

### Known Threat Patterns for {Rust + systemd + Docker + Tailscale/Caddy}

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Удаление не того файла уборщиком (обход через `..`/симлинк) | Tampering | Канонизация, `starts_with(корень)`, `symlink_metadata().is_file()`, работа только по `id` из базы |
| Утечка токена бота через журнал bot-api или `docker inspect` | Information Disclosure | `.env` с правами 0600, `TELEGRAM_VERBOSITY` низкий, поиск токена в учениях |
| Публичный порт bot-api из-за Docker | Elevation / Info | Публикация на `127.0.0.1`, проверка `nmap`/`ss` |
| Токен раннера открытым текстом через внешний `http://` | Information Disclosure | `Link` отвергает; только `https://` на внешние адреса |
| Медленные клиенты занимают пул потоков (WR-03 фазы 5) | Denial of Service | Сервер за прокси с таймаутами, слушает loopback; Tailscale Serve ограничивает круг клиентов tailnet |
| Резервная копия с хешами токенов и данными пользователей | Information Disclosure | Права 0600/0700 на каталог копий, не публиковать |
| Подмена бинарника при обновлении | Tampering | Артефакт CI с известного коммита, проверка SHA-256 при копировании на хост (по аналогии с `install.ps1`) |

## Учения (OPS-04): механизмы, улики, разделение работы

Общее: подготовка (скрипты, чек-листы, шаблон улик) выполняется агентом; проведение — только автором. Каждый протокол в `docs/drills/NN-имя.md` содержит: цель; предусловия; шаги с командами; ожидаемый исход («продолжение» или «понятное сообщение»); чего быть не должно (две одновременные записи, два ролика в чате, потерянный результат); список улик и команду их сбора; проверку скриптом (непустые файлы, поиск токена). Исход фиксируется строкой в `evidence/NN-имя/RESULT.md` (pass/fail и один абзац).

| # | Учение | Что должно сработать (якоря в коде) | Как вызвать | Улики | Автоматическая репетиция уже есть |
|---|--------|-------------------------------------|-------------|-------|-------------------------------------|
| 1 | Убить раннер посреди записи | `runner_state.json` (`state.rs:17`, этапы `Claimed/Fetched/Executing/Finished/Reporting`, `state.rs:26-36`); `hello` с `holding` и ответ `Keep/Cancel/Abandon` (`runner/lib.rs:673-740`); восстановление игры (`recover_executor`, `gamedir::kill_orphans/recover`); продление аренд при старте сервера (`Core::recover`, `core.rs:802`); возврат потерянной аренды тем же раннером | Диспетчер задач: завершить `cs2cinema.exe`/`cs2-cli.exe` во время этапа «запись»; запустить снова; вариант с паузой дольше аренды (45 с) | журнал сервера (строки «аренда истекла», «раннер … на связи»), `runner.log`, `runner_state.json` до/после, `cs2-server job show N` (число выдач, эпоха), чат (один ролик) | `remote::e2e::killed_during_recording_the_app_finishes_the_server_job_by_itself`, `…killed_during_upload…` (app), `runner/tests/outbox.rs`, `heartbeat.rs`, `lease_http` |
| 2 | Обрыв загрузки | докачка по `HEAD` и длине `.part` (`blobs.rs:144`, `api.rs` `head_result`); эпоха; `cs2-cli runner --upload-chunk-limit N` (`runner_cmd.rs:173`, `Config.upload_chunk_limit`, `runner/lib.rs:116`) | Запустить раннер с флагом, затем без него; либо `tailscale down` на машине раннера посреди `PUT` | журнал сервера, `runner.log`, длина `.part` до/после, SHA-256 результата на сервере и локально | `runner/tests/outbox.rs`, `transfer_http.rs` |
| 3 | Перезапуск сервера посреди задачи | `Core::recover` продлевает аренды до `now + startup_grace` (90 с) и роняет зависшие приёмы; раннер повторяет с паузой до 30 с (`runner/lib.rs`, ветка `NetError`); `systemd Restart` | `systemctl restart cs2-server` во время записи и во время загрузки результата; вариант `kill -9` | `journalctl` с метками времени, строка «восстановление: упавших приёмов …», `job show` (выдачи не выросли), `runner.log` | `lease_http`, `transfer_http`, `app` `remote::e2e` |
| 4 | Отмена при офлайн-раннере | `Event::Cancel` (флаг для выданной, сразу для `queued`), `Event::Expire` при запрошенной отмене → `Cancelled` (`rules.rs:391-400`), бот `/cancel` | Закрыть раннер; друг шлёт `/cancel` по задаче `queued` и по `running`; через 45–50 с проверка; затем запуск раннера (`Abandon`) | сообщения чата, `job show` (cancelled), `runner.log` («сервер велел бросить задачу»), содержимое `out/` на раннере после (**здесь проявится дыра: `drop_job` не удаляет `out_dir`**) | `core_states::cancel_requested_job_with_an_expired_lease_becomes_cancelled` (1083), `bot_status::cancel_*` |
| 5 | Демка 500 МБ через прокси | передача кусками по `wire::CHUNK_SIZE`, SHA-256, докачка; `max_demo_mib = 2000` по умолчанию; `bot_transfer_timeout_mins = 60`; прокси без буферизации и с таймаутами больше long-poll (25 с) | Отправить боту файл ≈ 500 МБ (реальная большая демка, либо `.dem` с магией `PBDEMS2\0` и добивкой — транспорт пройдёт, разбор закончится `BAD_DEMO`, тоже понятный исход); раннер идёт через Tailscale Serve/Caddy | время передачи, SHA-256 на обеих сторонах, пиковая память сервера (`ps`), строки прокси | `transfer_http` на малых файлах; 500 МБ — только вручную |
| 6 | Несовпадение версий | `wire::PROTOCOL_VERSION: u32 = 1` (`wire/lib.rs:13`); сервер отвечает 426 `protocol_mismatch` (`api.rs:416-423`); раннер: «версия протокола сервера не совпадает с раннером» и остановка, задача остаётся (`runner/lib.rs:48`, `:614-620`, `:686-690`); проверка `lease_http.rs:377` | Собрать раннер из временной правки `PROTOCOL_VERSION = 2` (одноразовая сборка в отдельной копии репозитория, не в основной ветке) или старой версии | текст ошибки в окне/консоли, `runner.log`, `job show` (задача `queued`, выдач 0), бот показывает «раннер офлайн» | `lease_http` (426), `runner` ветка fatal |

Замечание о версиях: сравнивается только номер протокола; `app_version` (версия пакета) пишется в журнал и в `runners.version`, но не сверяется. Усиление проверки — вне требований.

Поиск токена после учений: образцы — подставной токен бота в обеих формах (`:` и `~`) и секретная часть, токен раннера, `FACEIT_API_KEY`; места — журнал сервера (`journalctl`), журнал bot-api (`docker compose logs`), `runner.log`, `runner_state.json`, копия `server.db` (`grep -a`), `cs2cinema.db` приложения, экспорт чата Telegram. Скрипт печатает только число совпадений по файлам.

Разбиение работы: агент готовит `docs/drills/` (протоколы, `collect-server.sh`, `collect-runner.ps1`, `scan-token.sh`), папку-шаблон улик и проверяющий скрипт; автор проводит учения, а в плане стоят `checkpoint:human-verify` для каждого учения с условием «папка улик существует, проверка скриптом зелёная». Живую часть `07-UAT.md` (тесты 1–5: цепочка, спайк `sendVideo` против `sendDocument`, спайк общего тома, учения бота, поиск токенов) включить в тот же стенд.

## Унаследованные входы (состояние на HEAD)

| Пункт | Состояние | Доказательство |
|-------|-----------|----------------|
| CR-01 (огромный `Content-Length` без токена) | закрыт | `vendor/tiny_http` 0.12.0 с правкой `EqualReader::drop`, подключён `[patch.crates-io]` (Cargo.toml воркспейса); `07-01-SUMMARY.md`: «CR-01 закрыт: огромный Content-Length отвергается 413 в диспетчере, дочитывание тела в tiny_http ограничено» |
| WR-01 (`release` игнорирует отмену) | закрыт | `rules.rs:190-224`: при `cancel_requested != NotRequested` задача становится `Cancelled`; комментарий «(WR-01)»; тест `cancel_then_release_is_cancelled_not_requeued` |
| WR-02 (`TOO_MANY_GRANTS` вместо потерянной аренды) | **открыт** | `core.rs:684`: `WHERE state = 'queued' AND grants >= ?1 ORDER BY id` без `lease_lost`; тестов нет (`grep` по `WR-02`/`lease_lost` в тестах не находит регресс на `grants == max`) |
| WR-03 фазы 5 (медленные клиенты) | открыт, вне требований | `07-VERIFICATION.md`: «остаётся для фазы 9»; закрывается документом |
| CI на ubuntu-latest | **не выполнялся с кодом 1.1** | `ci.yml` содержит матрицу и шаги серверной сборки и артефакта; на GitHub последние запуски — 2026-10-09 |

## Рекомендуемое разбиение на планы (для планировщика)

| Волна | План | Содержание | Тип |
|-------|------|-----------|-----|
| 1 | 09-01 | Вход: WR-02 с окном возврата и регрессионным тестом; первый push и прогон `test (ubuntu-latest)`, разбор красного | код + `checkpoint:human-action` (push) |
| 1 | 09-02 | Уборщик сервера: ключи сроков (5 мест в `config.rs`, `cs2-server.example.json`, `config show`), `Core::clean`, поток `spawn_cleaner`, `cs2-server clean --dry-run`, `tests/cleanup.rs` на `ManualClock` | код |
| 1 | 09-03 | Уборщик раннера: `runner::sweep` (сироты `out/srv*`, `remote/srv*`, ротация `runner.log`), вызов из `tick` при простое, `tests/sweep.rs`, проверка в режиме приложения | код |
| 2 | 09-04 | Сквозной тест `runner/tests/chain.rs` (успех и сбой), `Stand::start_with_bot`, job `chain` в `ci.yml` с `timeout-minutes` и `concurrency` | код + CI |
| 2 | 09-05 | Развёртывание: `cs2-server backup`, `docs/deploy/*` (compose, юнит, таймер, Tailscale Serve, Caddy), проверка «снаружи только задуманный порт», перезагрузка | код + документ |
| 3 | 09-06 | Набор для учений: протоколы `docs/drills/`, `collect-*`, `scan-token.sh`, шаблон `evidence/`, чек-лист живой части `07-UAT` | документ + скрипты |
| 4 | 09-07 | Стенд на Linux-хосте: развёртывание по документу, спайки bot-api (права, `sendVideo`/`sendDocument`, `logOut`), решение A3 | `checkpoint:human-action` |
| 4 | 09-08 | Шесть учений с уликами, поиск токена, закрытие | `checkpoint:human-verify` на каждое учение |

Порядок важен: 09-01 стоит первым, потому что красный CI на Linux может менять остальные планы; 09-07 стоит до окончательной правки документа по правам bot-api.

## Sources

### Primary (HIGH confidence)
- Код репозитория, прочитанный в этой сессии: `crates/server/src/{lib,core,blobs,db,config,cli,log,rules,api}.rs`, `crates/server/src/bot/{deliver,ingest,commands,mod}.rs`, `crates/server/tests/{bot_e2e,common/mod,deps}.rs`, `crates/runner/src/{lib,link,state}.rs`, `crates/runner/tests/{common/mod,exec,faceit_flow,flow}.rs`, `crates/app/src/remote/{control,e2e}.rs`, `.github/workflows/{ci,release}.yml`, `Cargo.toml`
- `.planning/phases/05-…/05-REVIEW.md`, `05-VERIFICATION.md`, `06-…/06-VERIFICATION.md`, `06-UAT.md`, `07-…/07-VERIFICATION.md`, `07-UAT.md`, `07-RESEARCH.md`, `07-01-SUMMARY.md`, `08-…/08-VERIFICATION.md`, `08-UAT.md`, `08-RESEARCH.md`
- api.github.com/repos/Kamaliev/cs2cinema/actions/runs — история CI

### Secondary (MEDIUM confidence)
- https://github.com/aiogram/telegram-bot-api — образ, переменные окружения, uid 101, порты 8081/8082 (README, Dockerfile, энтрипойнт)
- https://github.com/tdlib/telegram-bot-api — режим `--local` (загрузка без ограничения размера, отправка до 2000 МБ, абсолютный `file_path`)
- https://core.telegram.org/bots/api#using-a-local-bot-api-server — то же, из официальной документации
- https://tailscale.com/kb/1242/tailscale-serve — `tailscale serve`, `--bg`, `status`, `reset`, отличие от Funnel
- https://caddyserver.com/docs/quick-starts/reverse-proxy — `reverse_proxy`, автоматический HTTPS, порты 80/443

### Tertiary (LOW confidence)
- Поведение Docker/ufw, права на файлы в контейнере, раскладка `documents/` в bot-api, `DynamicUser`+`StateDirectory`, метка `ubuntu-22.04` — из общих знаний, помечены `[ASSUMED]`; веб-поиск по глубине настроек bot-api и образам GitHub-раннера не удался (лимит сессии), требуют проверки живым стендом

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — новых cargo-пакетов нет, всё построено на прочитанном коде
- Architecture (уборщик, runner, сквозной тест): HIGH — механизмы и якоря прочитаны; MEDIUM по тому, взлетит ли первый запуск на Linux
- Развёртывание: MEDIUM — compose и юнит собраны по документации образа и systemd, не запускались
- Pitfalls: MEDIUM — часть из общих знаний (Docker/ufw, права)
- Учения: HIGH по механизмам, живые исходы неизвестны

**Research date:** 2026-10-12
**Valid until:** 2026-11-12 для кода и CI; документация образа и Tailscale — перепроверить при развёртывании (быстро меняются)
