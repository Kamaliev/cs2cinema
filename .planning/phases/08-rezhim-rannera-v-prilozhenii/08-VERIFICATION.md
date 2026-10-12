---
phase: 08-rezhim-rannera-v-prilozhenii
verified: 2026-10-12T15:00:00Z
status: passed
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
covered_digest: "v3:sha256:32436906389c8d8f6ad7d59f07a1eed56c66b668920c4528adab31bcb108b5c0"
behavior_unverified: 0
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 4/4
  gaps_closed:
    - "Ручной пункт 1: раздел «Задачи с сервера» в настройках, скрытый токен, права (icacls) на файл токена (08-UAT.md, тесты 1 и 2: pass)"
    - "Ручной пункт 2: живой стенд с настоящими CS2/HLAE/Telegram, включая перезапуск посреди записи (08-UAT.md, тест 5: pass)"
  gaps_remaining: []
  regressions: []
---

# Фаза 8: Режим раннера в приложении — отчёт о проверке

**Цель фазы:** Приложение на машине автора работает раннером: удалённая задача с сервера идёт через существующую очередь как обычная карточка, не мешает локальным задачам и сама продолжается после перезапуска, а без настроенного сервера приложение работает как раньше.
**Проверено:** 2026-10-12
**Статус:** passed (все автоматические критерии выполнены повторно; оба ручных пункта прежнего отчёта автор закрыл в UAT, 28/28 pass)
**Повторная проверка:** Да — после UAT; прежний отчёт устарел, потому что исправили frontmatter покрытой сводки. Код в `crates/` с момента первичной проверки (коммит 67b0afa) не менялся (`git diff 67b0afa HEAD -- crates` пуст; в рабочей копии отличается только перенос строки в тесте `crates/store/src/journal.rs` под rustfmt, к фазе функционально не относится).

## Достижение цели

### Наблюдаемые истины (критерии успеха ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | В настройках задаются адрес и токен, виден статус подключения; удалённая задача — обычная карточка с журналом, этапами, отменой и «Повторить»; результат уходит на сервер | ✓ VERIFIED | `Settings.runner_server` (`settings.rs`, по умолчанию пусто); раздел «Задачи с сервера» в `settings_page.rs` (адрес, скрытый ввод токена, «Сохранить токен», «Переподключить», `RemoteStatus::text()`); `RemoteExecutor::open` создаёт или усыновляет строку `jobs` через `Store::adopt_remote`, `queue_runner` исполняет её настоящим `PipelineExecutor` в потоке очереди, `CardProgress` пересылает этапы, журнал и артефакты. Тесты (повторно запущены мной, зелёные): `server_job_becomes_a_card_and_its_video_reaches_the_server`, `address_and_token_make_the_app_take_a_server_job`, `server_cancel_stops_the_running_card_without_pausing_the_queue`, `author_cancel_of_a_running_card_pauses_the_queue_and_cancels_on_the_server`, `retry_of_a_closed_server_card_runs_locally_and_uploads_nothing`. Вид окна, скрытый токен и права файла токена автор подтвердил в UAT (тесты 1, 2, 3) |
| 2 | Удалённая задача берётся только при пустой, свободной и не стоящей на паузе очереди; локальная задача, поставленная во время удалённой, ждёт; одновременно не больше одной записи | ✓ VERIFIED | `control::admission` (`control.rs:197`) закрывает ворота при `view.running`, `view.paused`, `view.hold`, ждущей локальной задаче, открытой карточке сервера и незаданных папках HLAE/CS2; `Gate` в `runner/src/lib.rs` не вызывает `claim` за закрытыми воротами. Запись идёт только в единственном потоке очереди. Тесты: `admission_closes_for_every_busy_queue_state`, `e2e::one_recording_at_a_time_and_local_jobs_wait_their_turn`, `e2e::paused_queue_keeps_server_jobs_out`, `crates/runner/tests/gate.rs` (3 passed) |
| 3 | Закрыли или убили приложение посреди удалённой задачи — после запуска задача возобновляется сама, результат доходит до сервера | ✓ VERIFIED | `App::new` вызывает `recover_after_crash` (`app.rs:219`); `Store::reset_running` переводит карточки сервера в `interrupted` без паузы очереди; раннер читает `runner_state.json`, шлёт `hello` с `holding`, и `adopt_remote(resume=true)` возвращает ту же строку в очередь. Тесты: `interrupted_server_job_resumes_after_restart_without_a_pause`, `e2e::killed_during_recording_the_app_finishes_the_server_job_by_itself`, `e2e::killed_during_upload_the_app_uploads_without_recording_again`, `e2e::closed_app_with_a_waiting_server_card_resumes_it`. Живой стенд с настоящими CS2/HLAE и перезапуском посреди записи автор подтвердил в UAT (тест 5) |
| 4 | Без адреса сервера приложение ведёт локальные задачи как в v1.0; `crates/cli/tests/pipeline.rs` и тесты очереди остаются зелёными | ✓ VERIFIED | `RemoteRunner::start` при пустом адресе не создаёт поток (`RemoteStatus::Off`); `queue::start` сохранён как обёртка над `start_with_hook(.., None)`. Тесты: `empty_address_starts_no_runner_and_the_queue_works_as_before`, `e2e::without_an_address_the_app_has_no_runner_and_behaves_like_v1`; запуск мной: `cargo test -p app --bin cs2cinema queue` — 31 passed; `cargo test -p cli --test pipeline` — 2 passed |

**Счёт:** 4/4 истин проверено (0 «присутствует, поведение не проверено»)

### Требования (REQUIREMENTS.md)

| Требование | Планы | Описание | Статус | Доказательство |
|-----------|-------|----------|--------|----------------|
| APP-01 | 08-01…08-06 | Адрес сервера и токен в настройках; удалённая задача — обычная карточка очереди | ✓ SATISFIED | истина 1; вид окна подтверждён в UAT |
| APP-02 | 08-02, 08-04, 08-06 | Удалённая задача берётся только когда локальная очередь свободна и не на паузе | ✓ SATISFIED | истина 2 |
| APP-03 | 08-03, 08-04, 08-06 | После перезапуска прерванная удалённая задача возобновляется сама | ✓ SATISFIED | истина 3 |
| APP-04 | 08-01, 08-04, 08-06 | Без сервера приложение работает как раньше, `pipeline.rs` зелёный | ✓ SATISFIED | истина 4 |

Все четыре ID объявлены в `requirements:` frontmatter планов (08-01: APP-01/04; 08-02: APP-02/01; 08-03: APP-03/01; 08-04: все четыре; 08-05: APP-01; 08-06: APP-02/03/04/01) и присутствуют в REQUIREMENTS.md (строки 47-50, чекбоксы `[x]`, таблица трассировки: Phase 8 / Complete, строки 122-125). Осиротевших требований нет.

### Требуемые артефакты

| Артефакт | Назначение | Статус | Детали |
|----------|-----------|--------|--------|
| `crates/app/src/remote/hub.rs` | Хаб карточек, `RemoteExecutor`, `queue_runner`, `CardProgress` | ✓ VERIFIED | 359 строк, подключён в `App::new` |
| `crates/app/src/remote/control.rs` | `RemoteRunner`, `admission`, `RemoteStatus`, наблюдатель | ✓ VERIFIED | 1014 строк; `tick()` вызывается каждый кадр (`app.rs:829`) |
| `crates/app/src/remote/token.rs` | Файл токена с суженным ACL | ✓ VERIFIED | при отказе `icacls` файла нет; права подтверждены автором в UAT (тест 2) |
| `crates/app/src/queue.rs` | `SettleHook`, `Advice`, `start_with_hook` | ✓ VERIFIED | `start` сохранён |
| `crates/store/src/lib.rs` | Миграция 6 (`external_ref`), `adopt_remote`, `reset_running`, `cancel_if_queued`, `set_card_info` | ✓ VERIFIED | входит в общий зелёный прогон |
| `crates/runner/src/lib.rs`, `exec.rs` | `Gate`/`Admission`, `Observer`, `Task.epoch`, `Artifact::Match` | ✓ VERIFIED | `gate.rs` (3 passed), `observer.rs` (5 passed) |
| `crates/app/src/settings_page.rs`, `jobs_page.rs` | Раздел настроек и карточка сервера | ✓ VERIFIED | структура покрыта тестами; отрисовка подтверждена в UAT |
| `crates/app/src/remote/e2e.rs`, `testkit.rs` | Сквозной стенд приложения | ✓ VERIFIED | сквозные тесты на настоящем `server::serve` и `FakeHost` зелёные |

### Проверка ключевых связей

| Откуда | Куда | Через | Статус |
|--------|------|-------|--------|
| `App::new` | очередь | `queue::start_with_hook(.., Some(hub))` (`app.rs:226`) | ✓ WIRED |
| `App::new` | раннер | `RemoteRunner::new(RemoteDeps{..})` (`app.rs:240`), `remote.tick()` каждый кадр | ✓ WIRED |
| `RemoteExecutor::run` | таблица `jobs` | `Store::adopt_remote` | ✓ WIRED |
| поток очереди | итог раннеру | `RemoteHub: SettleHook::on_settled` | ✓ WIRED |
| ворота (`gate`) | вид очереди и база | `queue.view()`, `store.next_queued()`, настройки | ✓ WIRED |
| `crates/server` | приложение | только `[dev-dependencies]`; в коде фазы сервер не изменялся | ✓ WIRED (ограничение соблюдено) |

### Трассировка потока данных (уровень 4)

| Артефакт | Данные | Источник | Реальные данные | Статус |
|----------|--------|----------|-----------------|--------|
| Карточка сервера | задача, демка, ник | `runner::Task` из `claim` настоящего сервера | да | ✓ FLOWING |
| Подпись карточки | карта и игрок | `Artifact::Match` → `set_card_info` | да | ✓ FLOWING |
| Статус в окне | `RemoteStatus` | события `Observer` | да | ✓ FLOWING |
| Итог на сервере | ролик | `PipelineExecutor` → загрузка кусками с докачкой | да (в тестах сервер получает ролик; в UAT — на живом стенде) | ✓ FLOWING |

### Поведенческие проверки (запущены мной в этой проверке)

| Поведение | Команда | Результат | Статус |
|-----------|---------|-----------|--------|
| Режим раннера, токен, хаб, e2e | `cargo test -p app --bin cs2cinema remote::` | 36 passed, 0 failed | ✓ PASS |
| Прежняя очередь | `cargo test -p app --bin cs2cinema queue` | 31 passed, 0 failed | ✓ PASS |
| CLI не сломан | `cargo test -p cli --test pipeline` | 2 passed, 0 failed | ✓ PASS |
| Ворота и наблюдатель раннера | `cargo test -p runner --test gate --test observer` | 3 + 5 passed | ✓ PASS |
| Весь workspace (прогон оркестратора на HEAD) | `cargo test --workspace` | 70 наборов ok, 0 failed | ✓ PASS |

### Исполнение проб

Проб (`scripts/*/tests/probe-*.sh`) фаза не объявляет. Шаг пропущен.

### Антипаттерны

| Файл | Строка | Шаблон | Серьёзность | Влияние |
|------|--------|--------|-------------|---------|
| `crates/app/src/remote/mod.rs` | 7 | устаревший комментарий «хаб подключается … следующими планами» при `#![allow(dead_code)]` | ℹ️ Info | модуль давно подключён; на цель не влияет |

Маркеров `TBD`/`FIXME`/`XXX` в `crates/app/src/remote`, `crates/runner/src`, `crates/app/src/queue.rs` не найдено (grep повторён). Контекст, на вердикт не влияющий: 08-SECURITY.md (`threats_open: 0`), 08-REVIEW.md (0 critical, 3 warning, 3 info — рекомендательные, открыты), 08-UI-REVIEW.md (17/24, блокеров нет).

### Требуется проверка человеком

Нет. Оба ручных пункта прежнего отчёта закрыты автором лично в 08-UAT.md (`status: complete`, 28 passed, 0 issues): раздел настроек, скрытый токен и `icacls` (тесты 1, 2) и живой стенд с настоящими CS2/HLAE/Telegram с перезапуском посреди записи (тест 5).

### Резюме по пробелам

Пробелов нет. Все четыре критерия успеха ROADMAP и требования APP-01…APP-04 подтверждены кодом и повторно запущенными тестами; прежние находки остаются в силе, регрессий нет. Статус изменён с `human_needed` на `passed` только потому, что ручные пункты теперь закрыты автором.

---

_Проверено: 2026-10-12_
_Верификатор: Claude (gsd-verifier)_
