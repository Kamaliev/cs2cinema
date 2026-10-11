# Phase 7: Telegram-бот и доставка - Research

**Researched:** 2026-10-11
**Domain:** синхронный Telegram-бот на `std::thread` поверх локального `telegram-bot-api --local`, встроенный в `cs2-server` (Rust, ureq 3, rusqlite 0.32)
**Confidence:** HIGH по коду репозитория и по поведению `telegram-bot-api` (прочитан исходник), MEDIUM по поведению в эксплуатации (права, GC файлов, метаданные видео — это закрывает спайк)

## Summary

Бот не нужно делать отдельным сервисом. Его правильное место — модуль `server::bot` (+ `server::tg` для клиента Bot API) внутри крейта `server`, в том же процессе, что и HTTP для раннеров. Бот зовёт `Core` напрямую (`submit`, `cancel`, `job`, `jobs`, `users`), а не ходит в HTTP раннера. Единственная новая зависимость — `ureq` (уже в воркспейсе, сейчас только как dev-зависимость сервера). Граница фазы 5 (`crates/server/tests/deps.rs`) проверяет только path-зависимости и имена крейтов воркспейса, поэтому `ureq` её не нарушает. Отдельный крейт `bot` создал бы цикл с бинарём `cs2-server` или потребовал бы третьего бинаря — не нужно.

Главные находки, которых нет в CONTEXT.md и которые меняют план:

1. **Раннер сегодня отвергает ссылки FACEIT.** `PipelineExecutor::run` возвращает `UNSUPPORTED_SOURCE`, если `task.faceit.is_some()` и демки нет (`crates/runner/src/exec.rs:214-216`). А `Core::assignment_of` падает «у задачи #N в очереди нет демки» на задаче без файла (`core.rs`, функция `assignment_of`), и `begin_receive` жёстко пишет `source_kind='file'`. Значит BOT-05 («скачивание делает раннер, ключ у автора») требует трёх изменений вне бота: `Core::submit_faceit`, выдача `DemoInfo{size:0}` для faceit-задач, и скачивание в раннере через `faceit::Client` с `JobConfig.faceit_key` автора.
2. **`faceit::parse_match_id` для строгой проверки не годится**: он режет строку по `/ ? # пробел` и берёт любой кусок, похожий на id (подходит `https://evil.example/<id>` и «голый» id). Для D-01 нужен отдельный строгий разбор. Он должен жить в `wire` (чистая функция без зависимостей), потому что `server` не может зависеть от `faceit` (список в `deps.rs`), а раннер перепроверяет ссылку тем же кодом.
3. **Токен бота попадает в локальные пути `telegram-bot-api`.** Подпапка бота — это сам токен (`bot_token_with_dc_`), а при невозможности двоеточия в имени файла двоеточие заменяется на `~`. `Redactor` ловит форму `цифры:секрет`, но не форму `цифры~секрет`. Нужно добавить `~`-вариант токена в точные секреты и никогда не логировать `file_path`.
4. **`getFile` в локальном режиме блокируется до конца скачивания** (`downloadFile` с ответом по завершению). Значит запрос к 2 ГБ-файлу висит минуты, и его нельзя делать в потоке опроса `getUpdates`.
5. **`uncertain` — состояние доставки, а не задачи.** В `JobState` его нет (восемь состояний, `Done` — конечное). Доставку и все данные чата нужно хранить в новой миграции `server.db` (шаг 2, только добавление таблиц и колонок).
6. **CR-01 всё ещё открыт** (`http.rs` не проверяет `body_length` до диспетчеризации). WR-01 (`release` игнорирует отмену) прямо бьёт по `/cancel` из BOT-08. Рекомендую закрыть CR-01 и WR-01 отдельным ранним планом этой фазы (см. «Открытые CR-01/WR-01»).

**Primary recommendation:** модуль `server::tg` (трейт `Tg` + `HttpTg` на ureq + `FakeTelegram`) и `server::bot` (шаговый автомат `Bot::step`, потоки только вызывают `step`), long polling `getUpdates` с `allowed_updates=["message"]`, смещение и входящие сообщения в SQLite (exactly-once через `UNIQUE(chat_id,message_id)`), ролик отправлять `sendVideo` по `file:///…` (метод и метаданные подтверждает спайк), итог отправки хранить в таблице доставок с состояниями `sending → sent | uncertain | failed`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Ссылки FACEIT (BOT-05)**
- **D-01:** Ссылки FACEIT принимаются только по строгому шаблону `https://www.faceit.com/.../room/<id>`. Сервер хранит только `match_id` и сам по ссылке никуда не ходит (нет SSRF). Демку скачивает раннер ключом FACEIT автора; ключ на сервер не попадает. Ссылка, не прошедшая шаблон, отклоняется с объяснением. Это подтверждено автором на обсуждении (заметка роадмапа про риск SSRF).

**Выбор игрока (BOT-03, BOT-10)**
- **D-02:** `/player <ник или SteamID64>` запоминается на пользователя. Подпись к файлу или ссылке переопределяет его на эту задачу. Без сохранённого ника и без подписи бот просит задать игрока и демку в очередь не ставит. Игрок фиксируется в задаче при постановке. Одна демка может дать несколько задач (по игрокам), дубли определяются по хешу (BOT-10).

**Лимиты и очередь (BOT-11)**
- **D-03:** Стартовые значения, настраиваются конфигом сервера: файл до 2 ГБ; 2 активные задачи на друга, 5 у админа; в общей очереди не больше 10 активных. Сверх лимита бот объясняет причину и задачу не ставит.

**Статус и сбои (BOT-06, BOT-07, BOT-09)**
- **D-04:** Одно сообщение статуса правится не чаще раза в ~10 с: этап и «дубль k из n». Друг получает при сбое короткую фразу по-русски без локальных путей. Админ получает отдельное сообщение: код, этап, подсказка, число попыток. Готовый ролик приходит отдельным новым сообщением. При неясном итоге отправки задача получает `uncertain`, `/resend` отправляет ролик ещё раз.

**Уже принято раньше (не переобсуждать)**
- Политика при удалении пользователя во время задачи: дать задаче закончиться, затем блокировать (рекомендация исследования, роадмап).
- Фаза начинается со спайка `telegram-bot-api --local` (общий том и права, `getFile` с абсолютным путём, `sendVideo` по `file://` на 0,5–1,5 ГБ, `sendVideo` против `sendDocument`, `logOut` из облачного API) и с `FakeTelegram`.
- Бот проверяет белый список до `getFile`; незнакомцу показывается только его Telegram-ID. Токен бота не попадает в журналы и сообщения.
- Стек: Rust, `std::thread`, без tokio; `server` не зависит от `pipeline`/`runner`/`host` (граница из фазы 5, тест `crates/server/tests/deps.rs`).
- Приём чужих демок включается, а фаза закрывается только вместе с защитой входа на раннере (фаза 6, выполнена).

### Claude's Discretion
- Где живёт бот (отдельный крейт или модуль `server`), библиотека/способ вызова Bot API, формат команд `/adduser` `/removeuser` `/users` и кто считается админом (из конфига), формат хранения подписей/ников, тексты сообщений, интервалы опроса `getUpdates`.
- `sendVideo` против `sendDocument` и подпись к ролику решаются по результатам спайка.
- Проверка магических байтов и размера демки до постановки (BOT-04), формат ответа при дубликате.

### Deferred Ideas (OUT OF SCOPE)
- `/pause` и `/resume` для администратора, уведомление «раннер офлайн» автору (BOT-F1).
- Выбор игрока кнопками из списка игроков матча (BOT-F2).
- Грубая оценка времени по истории задач (BOT-F3).
- Закрыть CR-01, WR-01, WR-02 сервера (перенесено в Phase 9 по решению автора; для фазы 7 обязательно до открытия сервера наружу, иначе незнакомец может уронить процесс).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| BOT-01 | Только белый список; незнакомцу его Telegram-ID; проверка до `getFile` | `allowed_updates=["message"]`, только `chat.type=="private"`, решение по `from.id` до разбора `document`; ответ незнакомцу с ограничением частоты; тест «`get_file` не вызывался» на `FakeTelegram` |
| BOT-02 | `/adduser`, `/removeuser`, `/users` | Таблица `users` уже есть (`tg_id UNIQUE`, `role`); нужен мягкий `removed_at` (FK `jobs.user_id`); админ — `role='admin'`, заводится только через CLI `user add --admin` |
| BOT-03 | `/start`, `/help`, `/player`; игрок фиксируется в задаче | Колонки `users.player_steam/player_name` уже в миграции 1; `Submit.player` фиксирует значение в задаче при постановке; подпись переопределяет |
| BOT-04 | `.dem`/`.dem.zst`, магические байты и размер, сразу подтвердить приём | `Core::submit` уже вызывает `sniff_demo` и проверку размера; до `getFile` проверяется `document.file_size`; подтверждение уходит до скачивания |
| BOT-05 | Ссылка FACEIT: строгая проверка, скачивает раннер | `wire::faceit` (строгий разбор), `Core::submit_faceit`, `DemoInfo{size:0}`, скачивание в `PipelineExecutor` на ключе автора |
| BOT-06 | Позиция, `/status`, `/queue`, одно сообщение статуса с ограничением частоты; раннер офлайн | `jobs.phase/take_n/take_m` уже в `JobRow`; позиция = число неоконченных задач с меньшим `id`; офлайн по `runners.last_seen_at`; троттлинг на `Clock` |
| BOT-07 | Ролик новым сообщением; `uncertain` и `/resend` | Таблица доставок с записью `sending` ДО вызова; тайм-аут/падение = `uncertain`; `/resend` по `file_id` после `sent`, иначе заново |
| BOT-08 | `/cancel`, `/retry` без повторной загрузки | `Core::cancel(job, force)` есть; `/retry` = новая задача на жёсткой ссылке на демку старой (`Core::resubmit`); нужен WR-01 |
| BOT-09 | Двухуровневый отчёт | Друг: таблица «код → фраза» с guard-тестом по `wire::fail::KNOWN`; админ: `fail_code/fail_state/fail_hint/grants` |
| BOT-10 | Дубли по хешу, несколько игроков на демку | Два слоя: `file_unique_id → sha256` до `getFile`, `sha256 + ключ игрока` после; жёсткая ссылка вместо второй копии на 2 ГБ |
| BOT-11 | Лимиты на пользователя | Счётчики активных задач по `jobs`+входящим; конфиг сервера; проверка ДО `getFile` |
</phase_requirements>

## Project Constraints (from CLAUDE.md)

- Стек не менять: Rust workspace, `std::thread` без tokio, `ureq` 3.x, `rusqlite` 0.32 (feature `bundled`). Встраиваться в существующие крейты.
- `server` зависит только от `wire` внутри воркспейса (`crates/server/tests/deps.rs`); `wire` — только serde/serde_json.
- Только Windows для записи, но `server` собирается и на Linux (CI job `test`, артефакт `cs2-server-linux-x64`).
- Пользовательские строки и doc-комментарии по-русски, идентификаторы по-английски; тесты названы предложениями о поведении.
- Ошибки: `Result<T, String>`; отмена через sentinel; `unwrap/expect` только в тестах.
- Внешние процессы (здесь `telegram-bot-api`) — таймаут и диагностика у каждого обращения.
- CLI `cs2-cli` продолжает собираться и проходить `crates/cli/tests/pipeline.rs` (изменение раннера не должно его ломать).
- Любое изменение репозитория идёт через GSD-команды (исполнитель фазы), а не напрямую.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Опрос `getUpdates`, разбор команд, белый список | Бот-модуль в процессе `cs2-server` | — | Только он знает Telegram-ID; решение «пускать ли» принимается до любых файловых операций |
| Скачивание файла из Telegram | Локальный `telegram-bot-api` (диск) + бот-модуль (читает путь) | Хранилище сервера (`blobs`) | `getFile` кладёт файл на диск bot-api; сервер копирует и считает хеш, дальше от bot-api не зависит |
| Учёт задач, очередь, лимиты, дедуп | `Core` + SQLite сервера | Бот (политика сообщений) | Транзакционная правда о задачах уже в `Core`; бот только читает и вызывает |
| Состояние чата (смещение, входящие, сообщение статуса, доставка) | SQLite сервера (новые таблицы) | — | Нужна атомарность с созданием задачи и переживание перезапуска |
| Скачивание демки по ссылке FACEIT | Раннер на машине автора | Сервер (хранит лишь канонический URL) | Ключ FACEIT не покидает машину автора; сервер не ходит по ссылкам (нет SSRF) |
| Доставка ролика | Бот-модуль → локальный `telegram-bot-api` → Telegram | Хранилище сервера (читает `highlights.mp4`) | `file://` читает процесс bot-api, он же загружает в Telegram |
| Двухуровневые отчёты о сбое | Бот-модуль (шаблоны) | `wire::fail` (коды), `jobs.fail_*` | Тексты зависят от получателя, данные — из строки задачи |
| Очистка рабочей папки bot-api | Фаза 9 (уборщик) | — | Вне фазы 7; здесь только немедленный перенос/копирование и запись в журнал |

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ureq` | 3.4.2 (уже в `Cargo.lock`) | Блокирующий HTTP-клиент для Bot API | Уже выбран проектом; синхронный; есть раздельные таймауты (`timeout_recv_response`, `timeout_global`, …) |
| `serde` / `serde_json` | 1 (уже есть) | JSON запросов и ответов Bot API | Уже зависимости `server`; ответы разбираются в небольшие структуры |
| `rusqlite` | 0.32.1 | Таблицы бота в `server.db` | Не менять; миграции через `user_version` |
| `sha2` | 0.10 | Хеш демки | Уже есть, считается при приёме в `Blobs` |

Версия ureq: `Cargo.lock` содержит `name = "ureq"`, `version = "3.4.2"` [VERIFIED: Cargo.lock]. Признаки ureq 3.4.2: `default = ["rustls", "gzip"]`, `json`, `multipart`, `platform-verifier` и т. д. [VERIFIED: raw.githubusercontent.com/algesten/ureq/3.4.2/Cargo.toml]. Методы таймаутов `timeout_global`, `timeout_per_call`, `timeout_connect`, `timeout_send_body`, `timeout_recv_response`, `timeout_recv_body` [VERIFIED: ureq 3.4.2 src/config.rs].

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tiny_http` | 0.12 (уже зависимость) | Поддельный Bot API в тестах HTTP-клиента | Только в `[dev-dependencies]`-тестах (уже в обычных зависимостях сервера, подходит и для тестов) |
| `ctrlc` | 3.5 (уже есть) | Остановка | Бот останавливается тем же флагом, что и HTTP |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Тонкий собственный клиент на ureq (~10 методов) | `frankenstein` (синхронный, ureq-бэкенд) | Библиотека тянет огромные модели типов и версионную чехарду; нужен полный контроль над редактированием URL с токеном в ошибках, раздельными таймаутами и абсолютными путями `getFile`, плюс шов `Tg` для `FakeTelegram`. Не берём |
| `teloxide`, любые async-клиенты | — | Требуют tokio — запрещено стеком |
| Webhook | `getUpdates` | Webhook в локальном режиме возможен по HTTP на любой порт, но требует приёма входящих, ответов 2xx и защиты; long polling проще и даёт управляемое подтверждение через `offset` |
| Отдельный крейт `bot` | модуль `server::bot` | Отдельный крейт зависит от `server`, значит бинарь `cs2-server` не может его подключить (цикл); нужен третий бинарь. Лишнее |

**Installation:** новых крейтов нет. Перенести `ureq` из `[dev-dependencies]` в `[dependencies]` сервера. Рекомендуется `ureq = { version = "3", default-features = false, features = ["json"] }` для бота: только HTTP на loopback, без TLS-стека. Но из-за унификации признаков в воркспейсе (раннер включает `platform-verifier`) отсутствие TLS не гарантируется, поэтому проверка схемы базового URL в конфиге обязательна (см. «Безопасность»).

**Version verification:** новых пакетов нет; `gsd-tools query package-legitimacy check --ecosystem crates ureq frankenstein` вернул `OK` для обоих (ureq: с 2018, ~5,3 млн скачиваний в неделю, репозиторий `algesten/ureq`; frankenstein: с 2021, ~1,4 тыс. в неделю, `ayrat555/frankenstein`) [VERIFIED: gsd-tools package-legitimacy].

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | Verdict | Disposition |
|---------|----------|-----|-----------|-------------|---------|-------------|
| ureq | crates.io | ~8 лет | ~5,3 млн/нед | github.com/algesten/ureq | OK | Approved (уже в проекте) |
| frankenstein | crates.io | ~5 лет | ~1,4 тыс./нед | github.com/ayrat555/frankenstein | OK | Не используется (рассмотрен как альтернатива) |

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

Новые зависимости не вводятся, поэтому `checkpoint:human-verify` на установку не нужен.

## Architecture Patterns

### System Architecture Diagram

```
Telegram (облако)
     ^ | загрузка/скачивание
     | v
telegram-bot-api --local  (loopback 127.0.0.1:8081, рабочая папка с токеном в имени)
     ^ | HTTP/JSON (getUpdates, sendMessage, editMessageText, getFile, sendVideo file://)
     | v
+------------------------------- процесс cs2-server -------------------------------+
|  поток poller ----> таблица tg_inbox (UNIQUE chat_id,message_id) + смещение (1 txn) |
|        |                         |                                                  |
|        | быстрые команды         | document/ссылка FACEIT                           |
|        v                         v                                                  |
|  Bot::step (чистая логика)   поток ingest (по одному): getFile -> копия в blobs ->  |
|   - белый список (ДО getFile)     Core::submit / submit_faceit / resubmit           |
|   - лимиты, дедуп, команды        |                                                 |
|        |                          v                                                 |
|        |                    Core (jobs, files, users)  <--- HTTP раннера (tiny_http)|
|        v                          ^                                                 |
|  поток notify: опрос jobs ------->+  статус (edit раз в >=10 с), сбой (2 текста),   |
|  поток deliver: sendVideo <-- blobs/results/<id>/highlights.mp4                     |
+------------------------------------------------------------------------------------+
                                     ^
                                     | claim/progress/finish (фаза 5-6)
                              cs2-cli runner (машина автора) -> FACEIT API по match_id
```

### Рекомендуемая структура
```
crates/server/src/
├── tg/                 # клиент Bot API
│   ├── mod.rs          # трейт Tg, типы Update/Message/File, TgError
│   ├── http.rs         # HttpTg на ureq: таймауты, редактирование токена, разбор ответа
│   └── fake.rs         # FakeTelegram (cfg(any(test, feature="testing")) или pub для tests/)
├── bot/
│   ├── mod.rs          # Bot::step, запуск потоков, остановка по флагу
│   ├── commands.rs     # разбор /команд, тексты (русские)
│   ├── store.rs        # tg_state, tg_inbox, tg_jobs/доставки, tg_files (SQL)
│   ├── ingest.rs       # getFile -> копия -> submit
│   ├── notify.rs       # статус и отчёты о сбоях
│   └── deliver.rs      # sendVideo, uncertain, resend
crates/wire/src/faceit.rs   # строгий разбор ссылки комнаты (чистая функция)
crates/server/tests/bot_*.rs
```
`FakeTelegram` нужен интеграционным тестам из `crates/server/tests/`, поэтому он должен быть публичным (как `ManualClock` в `clock.rs`, а не `#[cfg(test)]`).

### Pattern 1: Бот как шаговый автомат (как `FakeHost` + виртуальное время)
**What:** вся логика в `Bot::step(&self, now) -> StepReport` (обработать пачку входящих, один шаг ingest, один тик статуса, один шаг доставки). Потоки только крутят `step`/блокируются на `getUpdates`. Тесты вызывают `step` напрямую с `ManualClock` и `FakeTelegram`, без потоков и сна.
**When to use:** всё, что связано с троттлингом (10 с), повторными попытками, `retry_after`, `uncertain`.
**Example:** (структура, не готовый код)
```rust
// Источник: паттерн проекта (Clock + FakeHost)
pub trait Tg: Send + Sync {
    fn get_updates(&self, offset: i64, timeout_secs: u64) -> Result<Vec<Update>, TgError>;
    fn send_message(&self, chat: i64, text: &str) -> Result<i64, TgError>;          // -> message_id
    fn edit_message_text(&self, chat: i64, msg: i64, text: &str) -> Result<(), TgError>;
    fn get_file(&self, file_id: &str) -> Result<TgFile, TgError>;                   // file_path абсолютный
    fn send_video(&self, chat: i64, path: &str, caption: &str, meta: &VideoMeta) -> Result<SentVideo, TgError>;
    fn send_video_by_id(&self, chat: i64, file_id: &str, caption: &str) -> Result<SentVideo, TgError>;
    fn set_my_commands(&self, cmds: &[(String, String)]) -> Result<(), TgError>;
    fn get_me(&self) -> Result<Me, TgError>;
}
pub enum TgError {
    Api { code: i64, description: String, retry_after: Option<u64> },
    /// запрос мог дойти (отправка тела начата или ответ не получен)
    Timeout { sent: bool },
    Io(String),
}
```

### Pattern 2: Входящие сообщения — «принять, потом обработать» (exactly-once)
Смещение `getUpdates` двигается только после того, как сообщение **надёжно записано**. Для текстовых команд — после обработки, для документа и ссылки — после вставки строки в `tg_inbox`. Вставка и смещение — одна транзакция `BEGIN IMMEDIATE`. `UNIQUE(chat_id, message_id)` делает повтор безвредным (смещение не сдвинулось из-за падения → то же сообщение придёт снова → `INSERT OR IGNORE`). Состояния строки: `pending → downloading → queued(job_id) | rejected(причина)`. При старте `downloading` откатывается в `pending`.

### Pattern 3: Доставка с записью намерения до вызова
```
tg_jobs.delivery:  none → pending → sending → sent
                                      \→ uncertain   (тайм-аут после начала отправки, или процесс упал в sending)
                                      \→ failed      (Telegram ответил ошибкой: 400/403, повтор не поможет или нужен человек)
```
Строка `sending` пишется **до** вызова `send_video`. При старте процесса все `sending` становятся `uncertain`. Успех сохраняет `result_message_id` и `result_file_id` (из `result.video.file_id`). `/resend` для `sent` идёт по `file_id` (мгновенно, без повторной загрузки; для файлов, уже лежащих на серверах Telegram, лимитов нет [CITED: core.telegram.org/bots/api#sending-files]). Для `uncertain/failed` — заново `send_video` с файла, и перед этим бот пишет автору «если ролик уже пришёл, дубль — нормально». Слепого автоповтора после тайм-аута нет.

### Pattern 4: Статус одним сообщением, правка не чаще ~10 с
В `tg_jobs`: `status_message_id`, `status_text_hash`, `last_edit_ms`, `next_edit_ms`. Тик считает новый текст; правит, только если текст изменился **и** `now >= next_edit_ms`. Подтверждение приёма с номером и позицией и есть будущее сообщение статуса (создаётся сразу). Ошибки: `429` с `retry_after` сдвигает `next_edit_ms`; `400 message is not modified` считается успехом; `400 message to edit not found` — создать новое сообщение статуса; `403` — пользователь заблокировал бота, дальше молчим (недоставляемо). Весь текст — без `parse_mode` (ники и названия попадают в текст как есть, нет ошибки «can't parse entities»), длина ≤ 4096.

### Anti-Patterns to Avoid
- **`getFile` в потоке опроса.** Блокируется на скачивание целого файла; все остальные команды (`/cancel`, `/status`) встанут. Скачивание — в отдельном потоке, по одному за раз.
- **Слепой повтор `sendVideo` по тайм-ауту.** Двойная отправка 1,5 ГБ. Вместо этого `uncertain` и `/resend`.
- **Логировать/слать `file_path` из `getFile`.** В нём токен.
- **Доверие расширению `.dem`.** Решает магия (`PBDEMS2\0` или `28 B5 2F FD`), её проверяет `sniff_demo`.
- **`parse_mode=Markdown/HTML` для текста с ником игрока.** Любой `_`, `<`, `&` в нике ломает отправку именно сообщения о сбое.
- **Изменение `jobs.state` ботом.** Состояниями владеет `rules::decide`; бот только вызывает `Core`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Состояния задачи и отмена | Свои переходы в боте | `Core::cancel`, `Core::submit`, `rules::decide` | Эпохи, аренды и `cancel_requested` уже учтены; бот не должен писать в `jobs.state` |
| Проверка магии демки и размера | Своя проверка расширения | `blobs::sniff_demo` + проверка `max_demo_bytes` в `Core::submit` | Уже есть и протестирована |
| Хеширование и копирование 2 ГБ | Свой цикл | `Blobs::ingest_demo` (`copy_hashed`, SHA-256, `fsync`, `rename`) | Лимит байтов в цикле, `.part` и очистка при сбое уже сделаны |
| Маскирование токенов | Свой `replace` | `secret::Redactor` (+ `~`-вариант токена) | Уже стоит на журнале, базе и CLI |
| Корректная разметка текста | Экранирование MarkdownV2 | Простой текст без `parse_mode` | Исключает целый класс ошибок доставки |
| multipart-загрузка 1,5 ГБ | Свой multipart-писатель | `file:///` в поле `video` (локальный режим) | Тело файла вообще не идёт через HTTP-клиент |
| Время в тестах | `thread::sleep` | `Clock`/`ManualClock` из `clock.rs` | Троттлинг в миллисекундах |

**Key insight:** на этой фазе всё дорогое (состояния, хеши, секреты, время) уже есть в `server`; новое — это Telegram-ввод/вывод, таблицы чата и политика текстов. Новый код должен быть тонким слоем над `Core`.

## Подтверждённое поведение telegram-bot-api (читал исходник `tdlib/telegram-bot-api` master)

| Факт | Источник |
|------|----------|
| В локальном режиме `file:/` в поле файла трактуется как локальный путь: `td::begins_with(file_id, "file:/")` → `inputFileLocal(get_local_file_path(остаток))`. Для `file:///abs/path` остаток `//abs/path`, после снятия одного `/` получается `/abs/path`. Остаток проходит `url_decode(..., false)` — `%` нужно кодировать, `+` не меняется. Путь читает процесс `telegram-bot-api`, значит это путь в его пространстве имён (в контейнере — путь контейнера) | [VERIFIED: tdlib/telegram-bot-api Client.cpp `get_input_file`, `get_local_file_path`] |
| `getFile` в локальном режиме запускает `downloadFile(file_id, 1, 0, 0, false)` и отвечает по завершении скачивания; в ответе `file_path` — абсолютный локальный путь (`file->local_->path_`) | [VERIFIED: Client.cpp `do_get_file`, `on_file_download`, `json_store_file`] |
| Рабочая подпапка бота = `working_directory + bot_token_with_dc_ + "/"`; если ФС не принимает `:`, двоеточие заменяется на `~` | [VERIFIED: Client.cpp ~8600-8612, main.cpp `allow_colon_in_filenames_`] |
| Опции: `--local`, `--api-id`, `--api-hash`, `--http-port` (по умолчанию 8081), `--http-ip-address` (по умолчанию любые локальные IPv4), `--http-stat-ip-address`, `--dir`, `--files-dir`, `--temp-dir`, `--log`, `--username`, `--groupname`, `--max-webhook-connections` | [VERIFIED: telegram-bot-api.cpp] |
| Каталог файлов создаётся с `mkpath(files_directory, 0750)`: другой пользователь (процесс `cs2-server`) без общей группы файл не прочитает | [VERIFIED: main.cpp]; влияние на права — проверяет спайк |
| TDLib получает опции `ignore_file_names`, `store_all_files_in_files_directory`, `use_storage_optimizer` → имя файла Telegram в пути не сохраняется (`file_N.ext`), автоочистка хранилища TDLib включена | [VERIFIED: Client.cpp ~9404]; срок автоочистки — [ASSUMED], поэтому файл переносим сразу |
| README: `--local` даёт загрузку до 2000 МБ, скачивание без лимита, загрузку по локальному пути, `getFile` возвращает абсолютный путь | [CITED: github.com/tdlib/telegram-bot-api README] |
| README: перед переходом с облака вызвать `logOut`; про срок, хранение и очистку файлов в README ничего нет | [CITED: README] |
| Bot API: после `logOut` на локальный сервер можно входить сразу, но в облако — нельзя 10 минут. `close` — перед переносом между локальными серверами, возвращает 429 в первые 10 минут после запуска | [CITED: core.telegram.org/bots/api#logout, #close] |
| `getUpdates`: подтверждение — вызов с `offset` больше `update_id`; обновления хранятся не дольше 24 ч; `allowed_updates` без указания использует прошлую настройку; метод не работает при заданном webhook | [CITED: core.telegram.org/bots/api#getupdates] |

## Решения по каждому вопросу фокуса

### 1. `getUpdates` против webhook
Использовать **`getUpdates`** с `timeout=30`, `limit=100`, `allowed_updates=["message"]` (передавать каждый вызов явно). Таймаут чтения ureq на этот вызов — 45 с (`timeout_recv_response`), иначе постоянные ложные ошибки. `allowed_updates=["message"]` сам по себе закрывает `edited_message`, `channel_post`, `callback_query`, `my_chat_member` и пересланные из каналов. Обновления, созданные до смены настройки, могут прийти один раз — поэтому декодер всё равно обрабатывает только поле `message` и игнорирует остальное [CITED: getUpdates].
- 409 (`terminated by other getUpdates request`) — «второй экземпляр»: залогировать и остановить бот-поток с длинной паузой, не крутить цикл.
- Один экземпляр = один процесс `cs2-server serve`; `cs2-server submit` и прочие CLI-процессы бота не запускают.
- При старте `get_me` (самопроверка): `getMe` должен отвечать с локального базового URL; отказ стартовать, если базовый URL не `http://` loopback при `local_mode=true`.

### 2. `getFile`, общий том и права
`file_path` — абсолютный путь **в пространстве имён процесса bot-api**. Нужны два параметра конфига: `bot_api_files_root` (префикс пути bot-api → префикс на хосте сервера для входящих) и `bot_api_results_prefix` (путь к `blobs/results`, как его видит bot-api, для исходящих `file://`). Если оба процесса видят общий том по одному и тому же пути, отображение пустое. Перед чтением: путь после нормализации должен лежать внутри `bot_api_files_root`, быть обычным файлом (не симлинк: `symlink_metadata`), размер должен совпадать с `document.file_size`. Копирует `Core::submit` (`copy_hashed`), затем бот пытается удалить оригинал (ошибка удаления не фатальна, пишется в журнал; окончательную чистку делает фаза 9).
Спайк обязан ответить: читает ли пользователь сервера файлы из каталога bot-api (группа/umask/`--username`), может ли он удалять оттуда, жив ли файл после удаления/переименования, сколько занимает папка после повторного `getFile` того же `file_id`.

### 3. Загрузка ролика: `file://` против multipart
`file:///abs/path` в поле `video`: тело не идёт через клиент, нет multipart-кода. Путь строится только из числового id задачи (`<prefix>/results/<id>/highlights.mp4`), но префикс от оператора, поэтому функция `file_uri(path)` процентно кодирует `%`, пробел, `#`, `?` и не-ASCII, и покрыта тестом. На Windows-стенде (тесты) путь `C:\…` приводится к `file:///C:/…`; реальный сервер — Linux. Multipart как запасной вариант не делать (нарушает «не писать своё»), если спайк не покажет, что общий том невозможен.

### 4. `sendVideo` против `sendDocument` (до ~1,5 ГБ)
Рабочая гипотеза: **`sendVideo`** с `supports_streaming=true`, `duration`, `width`, `height`. Документация: «Telegram clients support MPEG4 videos (other formats may be sent as Document)», лимит в локальном режиме 2000 МБ [CITED: sendVideo; Using a Local Bot API Server]. Ролик раннера проходит `verify_result` (H.264, размеры, faststart; `crates/runner/src/exec.rs`), то есть подходит. Решение подтверждает спайк на 0,5/1,0/1,5 ГБ: воспроизводится ли с превью, как выглядит на мобильном клиенте, нужны ли `width/height/duration` (в `wire::Outcome::Done` их нет; при необходимости добавить опциональные поля с `#[serde(default)]` — аддитивно, версия протокола не меняется). До лимита Telegram оставлять запас: предварительно отказывать при размере результата > 1950 МиБ с понятным текстом. Граница 2000 «МБ» (десятичные/двоичные) в документации не уточнена [ASSUMED].

### 5. `editMessageText` и частота
FAQ: в одном чате не больше ~1 сообщения в секунду (кратковременные всплески допускаются, потом 429); в группе 20 сообщений в минуту; рассылка ~30 сообщений/с [CITED: core.telegram.org/bots/faq]. Отдельный лимит на редактирование не документирован — считать общим ведром. Интервал ~10 с из D-04 с большим запасом. `ResponseParameters.retry_after` — секунды ожидания при flood control [CITED: ResponseParameters]. Ошибка «message is not modified» в документации не описана — известное поведение Bot API [ASSUMED]; обрабатывать как успех по подстроке в `description`.

### 6. Идемпотентность и `uncertain`
Telegram не имеет ключа идемпотентности для `sendVideo`, и бот не может прочитать свои отправленные сообщения. Единственная защита — порядок записей: намерение (`sending`) до вызова; результат (`message_id`, `file_id`) сразу после; восстановление при старте. Тайм-аут на длинной загрузке — главный источник неясности. Разделить таймауты клиента: управляющие вызовы (`sendMessage`, `editMessageText`, `getMe`) — 15–30 с суммарно; `getFile` — длинный (час и более, настраиваемый); `sendVideo` — `timeout_recv_response` порядка 30–60 мин, настраиваемо. Классификация: тайм-аут/обрыв **после** начала запроса = `uncertain`; отказ соединения до отправки или `ok:false` с кодом 4xx = точный итог (`failed` или повтор). Повтор после `429`/сетевой ошибки подключения безопасен только когда запрос не доходил (`Timeout{sent:false}`/refused).

### 7. `logOut` из облачного API
Единственная одноразовая операция при переходе с облака: `POST https://api.telegram.org/bot<TOKEN>/logOut` до первого запуска `telegram-bot-api` с этим токеном. Облачный клиент после этого недоступен 10 минут. Бот сам `logOut` не вызывает (опасно при ошибке конфигурации). Это шаг документации спайка и фазы 9, с отдельным тестовым ботом. **Токен продакшн-бота для экспериментов не использовать**; завести второй бот у BotFather для спайка.

### 8. Безопасность токена
- Токен лежит только в `Secret` (`config.rs: telegram_bot_token`). Базовый URL клиента `http://127.0.0.1:PORT/bot<TOKEN>/METHOD` собирается только внутри `HttpTg`.
- Любая ошибка клиента, уходящая в журнал/БД/чат, проходит `Redactor`. Добавить в `Redactor::new` дополнительный точный секрет — вариант токена с `~` вместо `:` [VERIFIED: Client.cpp; Redactor сегодня маскирует `цифры:секрет` по форме и точное значение, а `~`-форма не совпадает ни с одним из двух правил, `secret.rs` `mask_bot_tokens` ищет только `:`].
- Отправлять в чат и писать в журнал можно только числа и классы (id задачи, размер), не `file_path`, не URL.
- ureq пишет URL через фасад `log` на уровнях debug/trace; в сервере обработчик `log` не ставится — не ставить без редактирования [ASSUMED].
- Токен не передавать аргументом командной строки в `curl` в документации спайка (виден в `ps`): использовать `curl --config` из файла с правами 0600 или `-K -`.

### 9. Белый список до `getFile` (документ, пересылка, правка)
Порядок проверок в `Bot::on_message` (обязателен, покрыт тестами):
1. `allowed_updates=["message"]`; декодер не знает остальных видов.
2. `chat.type == "private"` и `from` присутствует; иначе молча игнорировать (группы и каналы не обслуживаются).
3. `from.id` ищется среди `users` (`tg_id`, `removed_at IS NULL`). Нет → ответ «ваш Telegram-ID: N», не чаще раза за период на одного незнакомца (в памяти, например 1 раз в 10 мин), и **дальше ничего не разбирается**: ни `document`, ни `caption`, ни текст.
4. Только после этого читается `document`/`forward_origin`/текст. Пересылка от whitelisted-пользователя — обычное сообщение (отправитель — он сам). Правки (`edited_message`) не приходят вовсе.
5. Лимиты (BOT-11) и `file_size` проверяются **до** `getFile`.
Тест: в `FakeTelegram` счётчик `get_file_calls == 0` для незнакомца с документом, с пересланным документом и с `edited_message`.

### 10. Строгий разбор ссылки FACEIT
`wire::faceit::room_match_id(input: &str) -> Option<String>` и `canonical_url(id)`. Правила (вариант D-01):
- вся строка обрезается по краям, длина ≤ 300, без пробелов и управляющих символов внутри;
- схема только `https://`; хост ровно `www.faceit.com` или `faceit.com` (без регистра), без `user@`, без порта;
- путь: `/<язык>/<игра>/room/<id>[/<что-то>]` где язык `[a-z]{2}(-[A-Za-z0-9]+)?`, игра `[a-z0-9_-]{1,16}` (`cs2`, `csgo`), хвост (например `scoreboard`) `[A-Za-z0-9_-]{1,32}`, опциональный `/`; запрос `?…` и якорь `#…` отбрасываются до проверки;
- `id` = `<1–3 цифры>-<uuid 8-4-4-4-12 hex>` (повторяет `faceit::is_match_id`), нормализуется в нижний регистр;
- голый id и не-ссылки **отклоняются** (в отличие от `parse_match_id`), с объяснением «пришлите ссылку на комнату матча».
В `jobs.faceit_link` хранится **каноническая** ссылка, построенная из id (`https://www.faceit.com/en/cs2/room/<id>`), а не ввод пользователя. Раннер перед скачиванием снова прогоняет ссылку через `wire::faceit` и передаёт в `faceit::Client` только `match_id`. Обязательные негативные тесты: `https://www.faceit.com.evil.com/…`, `https://evil.com/https://www.faceit.com/en/cs2/room/<id>`, `https://www.faceit.com@evil.com/…`, `https://www.faceit.com:444/…`, `http://…`, `//www.faceit.com/…`, id с пробелом/переводом строки, юникодные «похожие» символы.

### 11. Дедуп по хешу и несколько игроков (BOT-10)
Ключ дубля = `(sha256 демки, ключ игрока)`, где ключ игрока = steam64 или `lowercase(trim(ник))`. Два слоя:
- До `getFile`: таблица `tg_files(file_unique_id PK, size, sha256, first_seen_ms)`. `file_unique_id` «должен быть тем же во времени и для разных ботов» [CITED: Document fields]. Если есть запись, скачивание не нужно: хеш известен, а демка берётся жёсткой ссылкой на файл прежней задачи (если он ещё существует).
- После скачивания: SHA-256 из `Ingested.sha256` — истина. Нашлась активная или `done` задача с тем же ключом → новую не создавать, ответить «уже в очереди #N» (или «уже готово: /resend N»). Упавшая или отменённая — новая задача разрешена (это и есть повтор). Другой игрок → новая задача.
Хранение: вместо второй копии на 2 ГБ делать `std::fs::hard_link` с копированием как запасным путём (нужна небольшая функция в `Blobs`, `ingest_demo_from(existing_job_id)`). Фаза 9 учитывает, что у файлов может быть несколько ссылок. Для FACEIT-задач хеша нет — дубль по `(match_id, ключ игрока)`.

### 12. Где живёт бот и как говорит с ядром
Внутри `server` (см. «Summary»). В `serve()` после `recover()` и до возврата `Running`: если `telegram_bot_token` задан, создаётся `HttpTg`, выполняется `get_me`/`set_my_commands` (с таймаутом), запускаются потоки poller / ingest / notify / deliver с общим флагом остановки; `Running` получает `bot: Option<BotHandle>`, `stop` будит и ждёт их (ограниченно, как для sweeper). Токена нет — бот не стартует, сервер работает как в фазе 5-6 (сохраняется поведение тестов). Состояние задач бот читает опросом `Core::jobs()`/`job(id)` раз в 2–3 с (таблица маленькая, один процесс, `updated_at` растёт при каждом событии); отдельная шина событий не нужна.

Пользователи: `users` уже содержит `tg_id UNIQUE`, `role`, `player_steam`, `player_name`; `add_user`, `list_users`, `find_user` есть, а удаления и поиска по `tg_id` нет. Нужны: `find_user_by_tg`, `remove_user` как **мягкая блокировка** `removed_at` (удалять строку нельзя: `jobs.user_id REFERENCES users(id)`, `PRAGMA foreign_keys=ON`) и `set_player`. Админ — `role='admin'`; первый админ создаётся CLI (`cs2-server user add NAME --tg ID --admin`), `/adduser` создаёт только `member` — повышение прав через чат невозможно. Это упрощает «кто админ» без новой настройки. Политика удаления: сразу снимается доступ к новым командам, идущие задачи доходят до конца и доставляются [ASSUMED: из формулировки «дать задаче закончиться, затем блокировать» — доставка результата удалённому пользователю не оговорена; подтвердить у автора].

### 13. FakeTelegram
`FakeTelegram` реализует `Tg` и:
- принимает сценарий входящих: `push_update(Update)`, `get_updates` отдаёт их по `offset` (так проверяется, что смещение двигается правильно и повторы не создают задач);
- пишет журнал вызовов `calls(): Vec<Call>` (метод, chat, текст/путь, порядок);
- инжектирует ошибки по методу: `fail_next("edit_message_text", TgError::Api{code:429, retry_after:Some(3)})`, `400 message is not modified`, `400 message to edit not found`, `403 blocked`, `Timeout{sent:true}` для `send_video` (при этом фейк **записывает** сообщение доставленным — чтобы тест видел потенциальный дубль), 409;
- хранит «файлы Telegram»: `put_document(file_id, bytes)` кладёт байты во временную папку и `get_file` возвращает **настоящий абсолютный путь**, так что ingest реально читает диск, проверяет магию и хеширует;
- проверяет `send_video(path)`: файл существует и размер совпадает, путь лежит под префиксом результатов.
Дополнительно маленький поддельный HTTP-сервер на `tiny_http` проверяет **настоящий** `HttpTg`: форму запросов (`allowed_updates`, `offset`, `file:///…`), разбор `ok:false` с `parameters.retry_after`, тайм-ауты (сервер молчит дольше таймаута → `Timeout`), редактирование токена в тексте ошибки при подключении к закрытому порту.

## Что нужно изменить в существующем коде (вне самого бота)

| Место | Изменение | Почему |
|-------|-----------|--------|
| `server/src/db.rs` | Миграция 2: таблицы `tg_kv`, `tg_inbox`, `tg_jobs`, `tg_files`; `ALTER TABLE users ADD COLUMN removed_at INTEGER`; индекс `files(sha256)` | Нет места хранить смещение, входящие, сообщение статуса, доставку, дедуп; правило проекта — только добавление |
| `server/src/core.rs` | `submit_faceit`, `resubmit`/`retry`, `find_duplicate(sha, player_key)`, счётчики активных (на пользователя и общий), позиция в очереди, `runner_online(now)` | Сейчас `begin_receive` жёстко `'file'`, дедупа и счётчиков нет |
| `server/src/core.rs` `assignment_of` | Для `source_kind='faceit'` без файла отдавать `DemoInfo{size:0, sha256:"", orig_name}` вместо ошибки | Иначе `claim` падает на ссылочной задаче; раннер уже умеет `demo_size == 0` (`runner/src/lib.rs:174-176`) |
| `server/src/api.rs` | Проверить, что `GET demo` для faceit-задачи даёт чистый 404 (а не 500) | Раннер не должен её запрашивать, но защита нужна |
| `server/src/users.rs` | `find_user_by_tg`, `remove_user` (мягко), `set_player`, `list` с флагом блокировки | BOT-02, BOT-03 |
| `server/src/config.rs` | Новые ключи: `bot_api_url`, `bot_api_files_root`, `bot_api_results_prefix`, лимиты BOT-11, интервалы; файл настроек читается с `deny_unknown_fields` и лимитом 64 КиБ — новые ключи добавить в `FileConfig` и `cs2-server.example.json`, `config show` | Единый способ настройки |
| `server/src/secret.rs` | `Redactor` умеет `~`-вариант токена | Токен в путях bot-api |
| `wire/src/faceit.rs` (новый) | `room_match_id`, `canonical_url` | Общая строгая проверка сервера и раннера |
| `runner/src/exec.rs` | Для `task.faceit` без файла: проверить ссылку `wire::faceit`, скачать демку `faceit::Client::new(cfg.faceit_key)` (`demo_urls(match_id)` → индекс 0 → `download`) в `remote/srv{id}.dem(.zst)`, затем обычный путь `inspect_demo` → `run_job` | Убирает `UNSUPPORTED_SOURCE`; ключ остаётся у автора. `runner` может зависеть от `faceit` (ограничен только `server`); этап `wire::Phase::Fetch` уже есть |
| `server/src/http.rs` | CR-01: ограничение `body_length()` до диспетчеризации и ограниченный слив тела; регресс-тест `Content-Length: 1000000000000000` | См. раздел ниже |
| `server/src/rules.rs` | WR-01: `Event::Release` при `cancel_requested != none` → `Cancelled` | `/cancel` иначе возвращает отменённую задачу в очередь |

## Открытые CR-01 / WR-01 / WR-02

- **CR-01** (аварийное завершение процесса от запроса без токена с огромным `Content-Length`): код проверен, защиты на уровне диспетчера нет — `http.rs` читает `body_length()` только в обработчиках (`api.rs:259`, `api.rs:534`), а `Drop` запроса в `tiny_http` 0.12 выделяет буфер размером объявленной длины [CITED: 05-REVIEW.md CR-01]. Так как бот живёт в том же процессе, падение HTTP-слоя останавливает и бота. **Рекомендация: отдельный ранний план фазы 7 (волна 1, до живого стенда и до открытия порта наружу).** Это небольшая правка в `http.rs` + регресс-тест, независимая от бота. Если нужно сохранить решение автора «в фазу 9», условие минимум — не открывать порт раннера наружу (только loopback/Tailscale) до закрытия, и вписать это условие в план живого стенда.
- **WR-01**: напрямую влияет на BOT-08 (`/cancel`) — включить в тот же ранний план (правка `rules.rs` + тест «отмена выданной задачи + release = Cancelled»).
- **WR-02** (`TOO_MANY_GRANTS` для задач с потерянной арендой): на бота не влияет, оставить в фазе 9.

## Common Pitfalls

### Pitfall 1: `getFile` блокирует опрос
**What goes wrong:** пока скачивается 2 ГБ, бот не отвечает на `/cancel`, а смещение не двигается; Telegram повторяет обновления.
**Why it happens:** локальный режим отвечает на `getFile` только после завершения скачивания [VERIFIED: Client.cpp].
**How to avoid:** отдельный ingest-поток, по одному; приём подтверждается до скачивания; длинный таймаут для `getFile`.
**Warning signs:** `/status` не отвечает во время загрузки.

### Pitfall 2: Токен в пути файла и в ошибке
**What goes wrong:** `ENOENT: /var/lib/telegram-bot-api/123456789~AAE…/documents/file_0.dem` попадает в `job.fail_message` или в чат.
**Why it happens:** подпапка бота названа токеном; `Redactor` не знает форму с `~`.
**How to avoid:** `~`-вариант в `Redactor`; в сообщениях об ошибках ingest — только класс ошибки, не путь; тест с подставным токеном по всем выходам.
**Warning signs:** `grep "bot[0-9]*[:~]" журнал`.

### Pitfall 3: Окно между «получил» и «записал»
**What goes wrong:** смещение сдвинулось, процесс упал до создания задачи — демка потеряна; или не сдвинулось — задача создана дважды.
**How to avoid:** Pattern 2: `tg_inbox` + смещение в одной транзакции, `UNIQUE(chat_id,message_id)`; ingest возобновляется по `pending`.

### Pitfall 4: Гонка лимитов
**What goes wrong:** друг присылает пять демок подряд; на момент проверки активных задач ещё нет (они в процессе скачивания) — лимит не срабатывает.
**How to avoid:** считать `jobs` активных статусов **плюс** строки `tg_inbox` в `pending/downloading` для того же пользователя; проверка и вставка в одной транзакции.

### Pitfall 5: Тихая потеря сообщения о сбое
**What goes wrong:** `403`/`400` на отправку сообщения о сбое останавливает цикл доставки для всех.
**How to avoid:** ошибки по чату изолированы: `403` → пометить получателя недоставляемым и идти дальше; `429` откладывает только этот чат; сообщения админу о недоставке пишутся в журнал.

### Pitfall 6: Проверка размера не по тому
**What goes wrong:** `document.file_size` отсутствует или лжёт.
**How to avoid:** отсутствующий размер = отказ с просьбой прислать файлом; фактический размер перепроверяется `Core::submit` (лимит в цикле копирования) и сравнивается с заявленным.

### Pitfall 7: Двойная копия на 2 ГБ
**What goes wrong:** bot-api хранит файл, сервер копирует, второй игрок той же демки — ещё копия, повтор — ещё одна; диск кончается.
**How to avoid:** жёсткие ссылки при совпадении хеша; удаление оригинала в рабочей папке bot-api сразу после копирования; журнал занятого места; уборка — фаза 9.

### Pitfall 8: `receiving` зависает
**What goes wrong:** `recover()` валит задачи в `receiving` старше 30 минут (`RECEIVING_STALE_MS`).
**How to avoid:** `getFile` и скачивание идут **до** `begin_receive` (задача создаётся, когда файл уже локален), копия и хеш внутри `Core::submit` — минуты, не 30.

## Code Examples

### Формирование `file://` URI (функция для теста)
```rust
// Источник: Client.cpp get_local_file_path — остаток после "file:/" проходит url_decode без '+' → '%' кодировать обязательно
fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    if !path.starts_with('/') { out.push('/'); } // C:/… на Windows-стенде тестов
    for b in path.replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
```

### Тайм-ауты клиента (ureq 3)
```rust
// Источник: ureq 3.4.2 src/config.rs — имена методов проверены
let control = ureq::Agent::config_builder()
    .http_status_as_error(false)               // ok:false разбираем сами
    .timeout_connect(Some(Duration::from_secs(5)))
    .timeout_global(Some(Duration::from_secs(30)))
    .build().into();
let transfer = ureq::Agent::config_builder()   // getFile и sendVideo
    .http_status_as_error(false)
    .timeout_connect(Some(Duration::from_secs(5)))
    .timeout_recv_response(Some(cfg.transfer_timeout)) // 30-60 мин, из конфига
    .build().into();
```

### Разбор ответа Bot API
```rust
// Источник: core.telegram.org/bots/api#making-requests и ResponseParameters
#[derive(Deserialize)]
struct Reply<T> { ok: bool, result: Option<T>, error_code: Option<i64>, description: Option<String>, parameters: Option<Params> }
#[derive(Deserialize)]
struct Params { retry_after: Option<u64> }
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Загрузка файла multipart | `file://` в локальном режиме | — (возможность локального сервера) | Нет потокового тела в клиенте, нет таймаута на тело |
| Лимит бота 50 МБ загрузка / 20 МБ скачивание в облаке | 2000 МБ загрузка, скачивание без лимита в `--local` | — | Именно ради этого поднимается локальный сервер |
| `file_path` как относительный путь для `https://api.telegram.org/file/bot<token>/<path>` | Абсолютный путь на диске bot-api | — | Клиентские библиотеки, ждущие URL, не подходят; свой клиент |

**Deprecated/outdated:** `parse_match_id` как проверка пользовательской ссылки (годится только для разбора доверенного ввода CLI); `ParseMode::Markdown` для текста с пользовательскими данными.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Автоочистка TDLib (`use_storage_optimizer`) может удалить файлы в папке bot-api со временем (срок неизвестен) | Подтверждённое поведение | Низкий: файл переносится сразу после `getFile`, повторные обращения идут к копии сервера |
| A2 | Текст ошибки `message is not modified` / `message to edit not found` определяется по подстроке `description` | Решения §5 | Лишнее сообщение статуса или бесполезный повтор |
| A3 | `sendVideo` без `width/height/duration` может показываться некорректно; метаданные может понадобиться добавить в `Outcome::Done` | Решения §4 | Либо лишняя работа, либо ролик с неверным соотношением сторон |
| A4 | Права: пользователь `cs2-server` может читать (и удалять) файлы из каталога bot-api только при общей группе или `--username/--groupname` | Решения §2 | Приём демок не работает до настройки; закрывает спайк |
| A5 | Граница 2000 МБ — не 2000 МиБ; запас 1950 МиБ безопасен | Решения §4 | Либо ложные отказы, либо отказ Telegram после долгой загрузки |
| A6 | После удаления пользователя результат уже принятых им задач всё же доставляется | Решения §12 | Если автор хочет иное — поменять одну проверку при доставке |
| A7 | ureq пишет URL на уровнях debug/trace через `log`, в сервере обработчик `log` не установлен | Решения §8 | Утечка токена в журнал при будущей установке `env_logger` |
| A8 | `file_unique_id` одинаков для одного и того же файла при повторной пересылке (документация: «supposed to be the same over time») | Решения §11 | Слой дедупа до `getFile` не сработает, но слой по хешу остаётся |
| A9 | FACEIT bo3 даёт несколько демок; берём первую (`demo_index 0`, как `Source::Faceit` в `pipeline`) | Что изменить | Бот ответит роликом по первой карте; выбор карты — вне фазы |
| A10 | `ureq` без TLS-признаков на схеме `https://` возвращает ошибку, а не тихо уходит в открытый текст | Standard Stack | Потенциально токен уйдёт в открытом виде; смягчено проверкой схемы в конфиге |

## Open Questions

1. **Метаданные видео для `sendVideo`**
   - Известно: поля `duration/width/height` необязательны; раннер знает их после `verify_result`, а в `Outcome::Done` их нет.
   - Неясно: нужны ли они для корректного показа в клиентах Telegram на локальном сервере.
   - Рекомендация: решает спайк. Если нужны — добавить опциональные поля в `Outcome::Done` (аддитивно) и читать их ботом.

2. **Кто и как делает спайк**
   - Известно: `telegram-bot-api` и docker на этой машине нет (`command -v` — отсутствуют), WSL установлен; нужны `api_id/api_hash` с my.telegram.org и **второй тестовый бот**.
   - Неясно: на каком хосте автор запускает спайк (Linux-машина будущего сервера или WSL).
   - Рекомендация: первый план — чек-лист и скрипт для автора (`checkpoint:human-verify`), итог записывается в `07-SPIKE.md` до выбора `sendVideo`/`sendDocument` и значений прав. Код фазы можно писать параллельно на `FakeTelegram`.

3. **Доставка удалённому пользователю** — подтвердить A6.

4. **Свободное место перед `getFile`** — проверка свободного места требует системного вызова (нужен крейт вроде `fs4`). Предложение: оставить на фазу 9 (уборщик и предупреждение админу), в фазе 7 ограничиться лимитами BOT-11 и одним ingest за раз.

5. **Удаление оригинала в папке bot-api** — если права не позволяют, остаётся копия на диске до уборщика фазы 9; решается по итогам спайка.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo / rustc | сборка и тесты | да | 1.99.0 | — |
| git | коммиты | да | 2.55.0 | — |
| curl | документация спайка | да | 8.21.0 | — |
| ffmpeg | ролик для спайка (тестовое видео 0,5–1,5 ГБ) | да | 9.0.2 | синтетический файл через ffmpeg `testsrc` |
| node (для gsd-tools) | инструменты GSD | есть, но не в PATH (`/c/Program Files/nodejs/node.exe`) | — | вызывать по полному пути |
| `telegram-bot-api` | спайк, живой стенд | **нет** | — | собрать из исходников или образ docker на Linux-хосте автора; без него код фазы пишется на `FakeTelegram` |
| docker | развёртывание bot-api | **нет** на этой машине | — | WSL / Linux-хост; полное развёртывание — фаза 9 |
| Telegram `api_id`/`api_hash`, тестовый бот | спайк | **не проверялось** (у автора) | — | без них приём реальных файлов невозможен; блокирует только живой стенд |
| Linux-хост сервера | живой стенд | не проверялось | — | сервер собирается и на Windows для тестов |

**Missing dependencies with no fallback:** реальный `telegram-bot-api` + учётные данные Telegram (нужны автору для спайка и для критерия 5; остальной код тестируется без них).
**Missing dependencies with fallback:** docker (WSL/Linux), токены (тестовый бот).

## Validation Architecture

> `workflow.nyquist_validation` в `.planning/config.json` равно `false`, поэтому формальная карта не требуется; раздел оставлен по запросу как подход к тестированию.

### Test Framework
| Property | Value |
|----------|-------|
| Framework | встроенный `cargo test` (как в проекте) |
| Config file | none |
| Quick run command | `cargo test -p server -p wire` |
| Full suite command | `cargo test --workspace` (плюс `cargo test -p cli --test pipeline` для совместимости CLI) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| BOT-01 | Незнакомец: только ID, `get_file` не вызван (документ, пересылка, правка) | integration | `cargo test -p server --test bot_whitelist` | Wave 0 |
| BOT-02 | `/adduser /removeuser /users`, админ только по `role` | integration | `cargo test -p server --test bot_users` | Wave 0 |
| BOT-03 | `/player`, подпись переопределяет, без ника — отказ | unit+integration | `cargo test -p server bot::commands` | Wave 0 |
| BOT-04 | Магия, размер, подтверждение до скачивания | integration | `cargo test -p server --test bot_ingest` | Wave 0 |
| BOT-05 | Строгий разбор ссылки; faceit-задача проходит `claim`; раннер не отвечает `UNSUPPORTED_SOURCE` | unit+integration | `cargo test -p wire faceit` / `-p runner` | Wave 0 |
| BOT-06 | Позиция, статус не чаще 10 с, раннер офлайн | integration (ManualClock) | `cargo test -p server --test bot_status` | Wave 0 |
| BOT-07 | `sending`→`uncertain` при тайм-ауте, `/resend` без слепого повтора | integration | `cargo test -p server --test bot_delivery` | Wave 0 |
| BOT-08 | `/cancel` (с WR-01), `/retry` без повторной загрузки | integration | `cargo test -p server --test bot_cancel_retry` | Wave 0 |
| BOT-09 | Друг: текст без путей по всем `wire::fail::KNOWN`; админ: код/этап/подсказка/попытки | unit+integration | `cargo test -p server bot::notify` | Wave 0 |
| BOT-10 | Дедуп по хешу, два игрока — две задачи | integration | `cargo test -p server --test bot_dedupe` | Wave 0 |
| BOT-11 | Лимиты 2/5/10, гонка пяти одновременных файлов | integration | `cargo test -p server --test bot_limits` | Wave 0 |
| SEC | Подставной токен нигде: журнал, БД, сообщения FakeTelegram, ошибки `HttpTg` (закрытый порт, `~`-форма) | integration | `cargo test -p server --test secrets` (расширить) | существует, дополнить |
| CR-01 | `Content-Length: 10^15` без токена не роняет сервер | integration | `cargo test -p server --test protocol_http` | дополнить |

### Sampling Rate
- **Per task commit:** `cargo test -p server -p wire`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** полный набор зелёный + ручной живой стенд (критерий 5, `human_verify_mode: end-of-phase`)

### Wave 0 Gaps
- [ ] `server/src/tg/{mod,http,fake}.rs` — трейт, клиент, `FakeTelegram`
- [ ] `server/tests/common` — добавить стенд «сервер + бот + FakeTelegram + ManualClock» поверх существующего `TestServer`
- [ ] Поддельный HTTP Bot API на `tiny_http` для тестов `HttpTg`
- [ ] Фейковый раннер для сквозного теста: существующие помощники `tests/common` (claim/progress/finish/upload) уже есть — переиспользовать
- [ ] Спайк `telegram-bot-api` (ручной, автор) — `07-SPIKE.md`

**Живой стенд — только вручную:** бот на локальном `telegram-bot-api` → сервер → `cs2-cli runner` → ролик в чате. Автоматически проверяются все ветки до границы Telegram.

## Security Domain

### Applicable ASVS Categories (уровень 1)

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | да (идентичность = Telegram `from.id`) | Белый список по числовому `from.id`, не по `@username`; только `chat.type=="private"` |
| V3 Session Management | нет | Сессий нет; смещение `getUpdates` — состояние, не сессия |
| V4 Access Control | да | `role='admin'` только из БД и CLI; админ-команды проверяют роль; пользователь видит только свои задачи; чужие `/cancel`/`/resend` запрещены |
| V5 Input Validation | да | Строгий разбор FACEIT (`wire::faceit`), очистка ника и имени файла (`Redactor::clean`, запрет управляющих), длины, магические байты, размер, сверка с `file_size` |
| V6 Cryptography | нет (новой нет) | SHA-256 из `sha2`; токены не хешируются заново |
| V7 Error Handling and Logging | да | Весь выходной текст через `Redactor` (с `~`-вариантом); нет `file_path` в журнале |
| V12 Files and Resources | да | Путь `getFile` внутри `bot_api_files_root`, не симлинк, размер сверен; пути хранилища только из числовых id |
| V13 API and Web Service | да | Базовый URL бота только `http://` loopback при локальном режиме; `getMe` самопроверка; нет webhook |
| V14 Configuration | да | Токен только в `Secret`; файл настроек с правами 0600; токен не в аргументах командной строки |

### Known Threat Patterns for этого стека

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Незнакомец заставляет скачать 2 ГБ (disk fill через `getFile`) | DoS | Белый список и лимиты до `getFile`; один ingest за раз; `allowed_updates=["message"]` |
| Спам незнакомца ответами бота | DoS/Amplification | Один ответ незнакомцу за период; ничего не разбирается |
| SSRF через ссылку | Tampering/Info disclosure | Сервер не ходит по ссылкам; строгий шаблон; только канонический URL; раннер ходит только в FACEIT API по `match_id` |
| Инъекция ника в cfg CS2/ffmpeg/скрипты | Tampering | Очистка ника в боте и на сервере; ранее защищено `renderer::console_safe`; запрет управляющих и bidi-символов (дополнительно проверить U+2028/2029) |
| Утечка токена через ошибки, пути, URL | Info disclosure | `Redactor` + `~`-вариант; не логировать `file_path`; тест с подставным токеном |
| Подмена/выход пути из `getFile` | Tampering | Нормализация и префикс-проверка; отказ для симлинков |
| Двойная отправка 1,5 ГБ | Reliability | `sending`→`uncertain`, `/resend` вручную |
| Падение процесса огромным `Content-Length` (CR-01) | DoS | Лимит `body_length` в диспетчере + ограниченный слив |
| Повышение прав через чат | Elevation | Роль админа только из CLI/БД |
| Повтор обновлений после падения | Tampering/Repudiation | `UNIQUE(chat_id,message_id)`, смещение в транзакции |

## Sources

### Primary (HIGH confidence)
- Репозиторий: `crates/server/src/{lib,core,db,rules,blobs,users,config,secret,http,cli,auth,api,clock}.rs`, `crates/server/tests/{deps.rs,common/mod.rs}`, `crates/wire/src/lib.rs` (прочитаны в этой сессии; пути с номерами строк указаны в тексте)
- `crates/runner/src/{exec,lib}.rs`, `crates/faceit/src/lib.rs`, `crates/pipeline/src/job.rs` — граница FACEIT
- `tdlib/telegram-bot-api` `Client.cpp`, `telegram-bot-api.cpp` (master), README — поведение локального режима
- https://core.telegram.org/bots/api — `getUpdates`, `getFile`, `sendVideo`, «Sending files», `logOut`, `close`, «Using a Local Bot API Server», `ResponseParameters`, `setMyCommands`, `BotCommand`, `Chat`
- https://core.telegram.org/bots/faq — лимиты частоты
- ureq 3.4.2 `Cargo.toml`, `src/config.rs`; `Cargo.lock`
- `gsd-tools query package-legitimacy check` (ureq, frankenstein)

### Secondary (MEDIUM confidence)
- `.planning/research/PITFALLS.md`, `.planning/research/ARCHITECTURE.md` (прежние исследования проекта; часть утверждений теперь подтверждена исходником: токен в путях, `--http-ip-address`)
- `.planning/phases/05-yadro-servera-i-protokol/05-REVIEW.md` — описание CR-01/WR-01/WR-02 (код для CR-01 перепроверен: защиты нет)

### Tertiary (LOW confidence)
- Поведение TDLib GC, права на файлы в общем томе, отображение видео без метаданных — всё в Assumptions Log, закрывается спайком

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — только уже используемые зависимости, версия из `Cargo.lock`
- Architecture: HIGH — по существующему коду `server`; решение «модуль внутри server» следует из `deps.rs` и структуры бинарей
- Telegram-поведение: HIGH для того, что читалось в исходнике bot-api и в документации; MEDIUM для эксплуатации (права, GC, метаданные видео)
- Pitfalls: HIGH — часть найдена чтением кода (FACEIT-задача, `assignment_of`, `~` в токене)

**Research date:** 2026-10-11
**Valid until:** 2026-11-10 (Bot API меняется часто; перед реализацией перечитать changelog Bot API и `getFile`/`sendVideo`)
