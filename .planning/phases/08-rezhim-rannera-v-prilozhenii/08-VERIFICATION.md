---
phase: 08-rezhim-rannera-v-prilozhenii
verified: 2026-10-12T14:15:00Z
status: human_needed
score: 4/4 must-haves verified
covered_files:
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-01-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-01-SUMMARY.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-02-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-02-SUMMARY.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-03-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-03-SUMMARY.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-04-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-04-SUMMARY.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-05-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-05-SUMMARY.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-06-PLAN.md"
  - ".planning/phases/08-rezhim-rannera-v-prilozhenii/08-06-SUMMARY.md"
  - "crates/app/src/app.rs"
  - "crates/app/src/jobs_page.rs"
  - "crates/app/src/queue.rs"
  - "crates/app/src/remote/control.rs"
  - "crates/app/src/remote/e2e.rs"
  - "crates/app/src/remote/hub.rs"
  - "crates/app/src/remote/tests.rs"
  - "crates/app/src/remote/token.rs"
  - "crates/app/src/settings.rs"
  - "crates/app/src/settings_page.rs"
  - "crates/runner/src/exec.rs"
  - "crates/runner/src/lib.rs"
  - "crates/runner/tests/gate.rs"
  - "crates/runner/tests/observer.rs"
  - "crates/store/src/lib.rs"
covered_digest: "v3:sha256:810a53207cbddc9152197cd97e98a258e711d203559124055eeb8e3a303a993f"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Раздел «Задачи с сервера» в настройках (план 08-05, <human-check>): запустить отладочную сборку с LOCALAPPDATA на временную папку и CS2CINEMA_PAGE=settings"
    expected: "Раздел на месте; при пустом адресе строка «Выключено: адрес сервера не задан» и нет кнопки «Переподключить»; после адреса и «Сохранить» строка становится «Токен раннера не задан…»; после ввода токена и «Сохранить токен» поле пустое, токен нигде на экране не виден; icacls на файл токена показывает единственную запись — текущего пользователя"
    why_human: "Вид окна, скрытый ввод и реальные права NTFS (icacls без повышения прав, допущение A1) тестами без окна и без Windows ACL не проверить; исполнитель этот шаг не выполнял"
  - test: "Живой стенд: бот Telegram, сервер, приложение на машине автора с настоящими CS2 и HLAE"
    expected: "Задача друга доходит до готового ролика в чате; перезапуск приложения посреди записи возобновляет задачу сама"
    why_human: "Настоящие CS2/HLAE и Telegram автоматически не воспроизводятся; автор отложил живой стенд в фазу 9 (07-UAT, коммит 01e2ffd). Это отложенная проверка, а не пробел"
---

# Фаза 8: Режим раннера в приложении — отчёт о проверке

**Цель фазы:** Приложение на машине автора работает раннером: удалённая задача с сервера идёт через существующую очередь как обычная карточка, не мешает локальным задачам и сама продолжается после перезапуска, а без настроенного сервера приложение работает как раньше.
**Проверено:** 2026-10-12
**Статус:** human_needed (все автоматически проверяемые критерии выполнены, остались два ручных пункта, заранее объявленные как отложенные)
**Повторная проверка:** Нет — первичная проверка

## Достижение цели

### Наблюдаемые истины (критерии успеха ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | В настройках задаются адрес и токен, виден статус подключения; удалённая задача — обычная карточка с журналом, этапами, отменой и «Повторить»; результат уходит на сервер | ✓ VERIFIED | `Settings.runner_server` (`settings.rs:34`, по умолчанию пусто, старые файлы настроек читаются); раздел «Задачи с сервера» в `settings_page.rs:190-250` (адрес, `password(true)` для токена, «Сохранить токен», «Переподключить», `RemoteStatus::text()`); `RemoteExecutor::open` создаёт или усыновляет строку `jobs` через `Store::adopt_remote`, `queue_runner` исполняет её настоящим `PipelineExecutor` в потоке очереди, `CardProgress` пересылает этапы, журнал и артефакты. Тесты (зелёные, запуск мной): `server_job_becomes_a_card_and_its_video_reaches_the_server`, `address_and_token_make_the_app_take_a_server_job`, `server_cancel_stops_the_running_card_without_pausing_the_queue`, `author_cancel_of_a_running_card_pauses_the_queue_and_cancels_on_the_server`, `author_cancel_of_a_waiting_card_reaches_the_server`, `retry_of_a_closed_server_card_runs_locally_and_uploads_nothing`, `server_outcome_is_written_into_the_card_journal`. Внешний вид окна — см. раздел ручной проверки |
| 2 | Удалённая задача берётся только при пустой, свободной и не стоящей на паузе очереди; локальная задача, поставленная во время удалённой, ждёт; одновременно не больше одной записи | ✓ VERIFIED | `control::admission` закрывает ворота при `view.running`, `view.paused`, `view.hold` (ожидание авто-повтора), ждущей локальной задаче (`store.next_queued()` без удержанных карточек), открытой карточке сервера и незаданных папках HLAE/CS2; `Gate` в `runner/src/lib.rs` не вызывает `claim` за закрытыми воротами и держит раннер на связи через `hello` («занят: причина»), после `claim` ворота проверяются повторно с `release(counts: false)`. Запись идёт только в единственном потоке очереди (поток раннера лишь создаёт карточку и ждёт итога по каналу). Тесты: `admission_closes_for_every_busy_queue_state`, `local_job_closes_the_gate_and_the_server_job_waits`, `e2e::one_recording_at_a_time_and_local_jobs_wait_their_turn` (счётчик `Recordings.current() == 1` всё время, B ждёт в `Queued` пока пишется карточка), `e2e::paused_queue_keeps_server_jobs_out`, `crates/runner/tests/gate.rs` (3 passed) |
| 3 | Закрыли или убили приложение посреди удалённой задачи — после запуска задача возобновляется сама, результат доходит до сервера | ✓ VERIFIED | `App::new` вызывает `recover_after_crash`, `Store::reset_running` переводит карточки сервера (`running`/`queued`, `external_ref IS NOT NULL`) в `interrupted` с пометкой «продолжится сама» и **без** паузы очереди (`Reset.interrupted_remote` отдельно от `interrupted`); раннер читает `runner_state.json`, шлёт `hello` с `holding`, получает `Keep`, и `adopt_remote(resume=true)` возвращает ту же строку в очередь с прежней папкой. Тесты: `interrupted_server_job_resumes_after_restart_without_a_pause` (проверяет отсутствие паузы очереди и `JobState::Done` на сервере), `e2e::killed_during_recording_the_app_finishes_the_server_job_by_itself`, `e2e::killed_during_upload_the_app_uploads_without_recording_again`, `e2e::closed_app_with_a_waiting_server_card_resumes_it` — все зелёные |
| 4 | Без адреса сервера приложение ведёт локальные задачи как в v1.0; `crates/cli/tests/pipeline.rs` и тесты очереди остаются зелёными | ✓ VERIFIED | `RemoteRunner::start` при пустом адресе не создаёт поток и не делает запросов (`RemoteStatus::Off`). `queue::start` сохранён как обёртка над `start_with_hook(.., None)`; diff `queue.rs` относительно начала фазы удаляет только строку-комментарий и три заменённые строки, прежние тесты очереди не правились; `opens_result` не открывает плеер только для карточек сервера. Тесты: `empty_address_starts_no_runner_and_the_queue_works_as_before`, `e2e::without_an_address_the_app_has_no_runner_and_behaves_like_v1`, `cargo test -p app --bin cs2cinema queue` (31 passed), `cargo test -p cli --test pipeline` (2 passed) |

**Счёт:** 4/4 истин проверено (0 «присутствует, поведение не проверено»; все поведенческие истины подтверждены сквозными тестами на настоящем `server::serve` и `FakeHost`)

### Требования (REQUIREMENTS.md)

| Требование | Планы | Описание | Статус | Доказательство |
|-----------|-------|----------|--------|----------------|
| APP-01 | 08-01…08-06 | Адрес сервера и токен в настройках; удалённая задача — обычная карточка очереди | ✓ SATISFIED | истина 1; раздел настроек и карточка (`jobs_page.rs`: `remote_card_buttons`, `server_heading`, `waiting_reason`, `server_note`) покрыты тестами без окна; вид окна — ручной пункт 1 |
| APP-02 | 08-02, 08-04, 08-06 | Удалённая задача берётся только когда локальная очередь свободна и не на паузе | ✓ SATISFIED | истина 2 |
| APP-03 | 08-03, 08-04, 08-06 | После перезапуска прерванная удалённая задача возобновляется сама | ✓ SATISFIED | истина 3 |
| APP-04 | 08-01, 08-04, 08-06 | Без сервера приложение работает как раньше; `pipeline.rs` зелёный | ✓ SATISFIED | истина 4 |

Все четыре ID объявлены в `requirements:` frontmatter планов (08-01: APP-01/04; 08-02: APP-02/01; 08-03: APP-03/01; 08-04: все четыре; 08-05: APP-01; 08-06: APP-02/03/04/01) и присутствуют в REQUIREMENTS.md (стр. 47-50, таблица трассировки: Phase 8). Осиротевших требований фазы нет. Чекбоксы в REQUIREMENTS.md уже отмечены `[x]`, поля `requirements-completed` в сводках 08-01…08-04 пусты по решению оркестратора (отметка после проверки) — расхождение процедурное, не функциональное.

### Требуемые артефакты

| Артефакт | Назначение | Статус | Детали |
|----------|-----------|--------|--------|
| `crates/app/src/remote/hub.rs` | Хаб карточек, `RemoteExecutor`, `queue_runner`, `CardProgress` | ✓ VERIFIED | 359 строк, подключён в `App::new` |
| `crates/app/src/remote/control.rs` | `RemoteRunner`, `admission`, `RemoteStatus`, наблюдатель | ✓ VERIFIED | 1014 строк, `apply`/`tick` вызываются из `App::new` и каждого кадра (`app.rs:252, 829`) |
| `crates/app/src/remote/token.rs` | Файл токена с суженным ACL | ✓ VERIFIED | пустой файл → `icacls` → значение; при отказе файла нет (`failed_acl_leaves_no_token_file`) |
| `crates/app/src/queue.rs` | `SettleHook`, `Advice`, `start_with_hook` | ✓ VERIFIED | `start` сохранён, хук слышит только окончательное решение |
| `crates/store/src/lib.rs` | Миграция 6 (`external_ref`), `adopt_remote`, `reset_running`, `cancel_if_queued`, `set_card_info` | ✓ VERIFIED | тесты хранилища зелёные по сводке и входят в общий прогон оркестратора |
| `crates/runner/src/lib.rs`, `exec.rs` | `Gate`/`Admission`, `Observer`/`RunnerEvent`, `Task.epoch`, `Artifact::Match` | ✓ VERIFIED | `gate.rs` (3 passed), `observer.rs` (5 passed) |
| `crates/app/src/settings_page.rs`, `jobs_page.rs` | Раздел настроек и карточка сервера | ✓ VERIFIED (структура) | чистые функции кнопок и текстов покрыты тестами; отрисовка — ручной пункт |
| `crates/app/src/remote/e2e.rs`, `testkit.rs` | Сквозной стенд приложения | ✓ VERIFIED | семь сквозных тестов, все зелёные |

### Проверка ключевых связей

| Откуда | Куда | Через | Статус |
|--------|------|-------|--------|
| `App::new` | очередь | `queue::start_with_hook(.., queue_runner(jobs::runner, hub, app_factory), .., Some(hub))` | ✓ WIRED |
| `App::new` | раннер | `RemoteRunner::new(RemoteDeps{..}).apply(&s.runner_server)`, `remote.tick()` каждый кадр | ✓ WIRED |
| `RemoteExecutor::run` | таблица `jobs` | `Store::adopt_remote` под замком хаба | ✓ WIRED |
| поток очереди | итог раннеру | `RemoteHub: SettleHook::on_settled` → `HubMsg::Settled` | ✓ WIRED |
| ворота (`gate`) | вид очереди и база | `queue.view()`, `store.next_queued()`, `hub.has_slots()`, настройки | ✓ WIRED |
| наблюдатель | окно и журнал карточки | `Shared.status`, `hub.set_held_ref`, `journal()`, `repaint` | ✓ WIRED |
| `crates/server` | приложение | только `[dev-dependencies]` (`server`, `wire`); diff `crates/server` за фазу пуст | ✓ WIRED (ограничение соблюдено) |

### Трассировка потока данных (уровень 4)

| Артефакт | Данные | Источник | Реальные данные | Статус |
|----------|--------|----------|-----------------|--------|
| Карточка сервера | задача, демка, ник | `runner::Task` из `claim` настоящего сервера | да (стенд на `server::serve`) | ✓ FLOWING |
| Подпись карточки | карта и игрок | `Artifact::Match` из дочерней предпроверки → `set_card_info` (с очисткой текста друга) | да | ✓ FLOWING |
| Статус в окне | `RemoteStatus` | события `Observer` потока раннера | да | ✓ FLOWING |
| Итог на сервере | ролик | `PipelineExecutor` → загрузка кусками с докачкой | да (сервер получает ролик в тестах) | ✓ FLOWING |

### Поведенческие проверки

| Поведение | Команда | Результат | Статус |
|-----------|---------|-----------|--------|
| Режим раннера, токен, хаб, e2e | `cargo test -p app --bin cs2cinema remote::` | 36 passed, 0 failed | ✓ PASS |
| Прежняя очередь | `cargo test -p app --bin cs2cinema queue` | 31 passed, 0 failed | ✓ PASS |
| CLI не сломан | `cargo test -p cli --test pipeline` | 2 passed, 0 failed | ✓ PASS |
| Ворота и наблюдатель раннера | `cargo test -p runner --test gate --test observer` | 3 + 5 passed | ✓ PASS |
| Весь workspace (прогон оркестратора на HEAD 6686507) | `cargo test --workspace` | 938 passed, 1 ignored | ✓ PASS |

### Исполнение проб

Проб (`scripts/*/tests/probe-*.sh`) фаза не объявляет. Шаг пропущен.

### Антипаттерны

| Файл | Строка | Шаблон | Серьёзность | Влияние |
|------|--------|--------|-------------|---------|
| `crates/app/src/remote/mod.rs` | 7 | устаревший комментарий «хаб подключается к окну и настройкам следующими планами» при `#![allow(dead_code)]` | ℹ️ Info | Модуль давно подключён; комментарий и `allow` можно убрать позже, на цель не влияет |

Маркеров `TBD`/`FIXME`/`XXX`/`TODO`/`unsafe` в файлах фазы не найдено. Незафиксированное изменение `crates/store/src/journal.rs` (перенос строки в тесте под rustfmt) к фазе функционально не относится.

Дополнительно, по 08-SECURITY.md (31/31 угроз закрыто, `threats_open: 0`) и 08-UI-REVIEW.md (17/24, блокеров нет) — как справочный контекст; на вердикт они не влияют.

### Требуется проверка человеком

#### 1. Раздел «Задачи с сервера» в окне (из `<human-check>` плана 08-05)

**Тест:** собрать `cargo build -p app -p cli`, запустить `target/debug/cs2cinema.exe` с `LOCALAPPDATA` на временную папку и `CS2CINEMA_PAGE=settings`. Проверить: раздел на месте; при пустом адресе «Выключено: адрес сервера не задан» и нет «Переподключить»; после адреса и «Сохранить» — «Токен раннера не задан…»; после токена и «Сохранить токен» поле пустое, токен нигде не виден; `icacls "<папка>\cs2cinema\runner\token"` показывает одну запись — текущего пользователя.
**Ожидается:** как перечислено выше.
**Почему человек:** внешний вид окна и реальные права NTFS без повышения привилегий (допущение A1) тестами без окна не проверяются; исполнитель этот шаг не выполнял (записан в 08-05-SUMMARY.md как отложенный).

#### 2. Живой стенд (отложен автором в фазу 9)

**Тест:** бот → сервер → приложение с настоящими CS2/HLAE; затем закрыть приложение посреди записи и запустить снова.
**Ожидается:** ролик приходит в чат; прерванная задача возобновляется сама.
**Почему человек:** настоящие CS2/HLAE и Telegram автоматически не воспроизводятся; решение автора (07-UAT, коммит 01e2ffd). Это не пробел фазы 8.

### Резюме по пробелам

Пробелов нет. Все четыре критерия успеха ROADMAP и все четыре требования APP-01…APP-04 подтверждены кодом и повторно запущенными тестами: удалённая задача проходит через ту же очередь как строка `jobs` с `external_ref`, ворота допуска закрыты по виду очереди, запись остаётся единственной в потоке очереди, прерванная карточка усыновляется заново без паузы, а без адреса поток раннера не создаётся и прежние тесты очереди и `cli` зелёные. Статус `human_needed` вызван только двумя заранее объявленными ручными пунктами (вид окна и права на файл токена; живой стенд — фаза 9).

---

_Проверено: 2026-10-12_
_Верификатор: Claude (gsd-verifier)_
