---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 09
subsystem: app
tags: [rust, app, queue, policy, auto-retry, notification, tray]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "FailClass/classify и requeue_auto/auto_retries (04-02), Host::notify/FakeHost::sequence (04-03), поток очереди app::queue (04-08)"
provides:
  - "app::queue: QueuePolicy (чистая), Finished, Verdict, PauseReason::Repeated, AUTO_RETRY_PAUSE, AUTO_RETRY_BUDGET, Summary, summary_text, pause_text/pause_notice, QueueView.hold, QueueConfig.retry_pause"
  - "queue::start(store, host, runner, ui, paused, cfg): очередь знает общий хост и шлёт уведомления в трей"
  - "Msg::QueueSummary, JobsState.summary, App::dismiss_summary, подсказка значка трея, Reporter::with_receiver (cfg(test))"
  - "jobs_page: отметка «авто-повтор через N с», баннер причины паузы с веткой Repeated, баннер итога очереди с «Скрыть»; ui::plural"
affects: [04-10, 04-11, 04-12]

actuals:
  tokens: 17900
  tasks: 3
  commits: 3

plan_head_before: b1ba2597bb7eceb40f683fe86efc135910000e01
plan_head_after: 84efa3e849c9ea70beade7448b345295661cb760
commits: 3

coverage:
  - deliverable: "Сбой класса «запуск» повторяется один раз автоматически после паузы, с разделителем «авто-повтор», строкой журнала и счётчиком (DIF-04)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#launch_failure_is_retried_once_after_the_pause_and_logged_as_auto"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#second_launch_failure_is_not_retried"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#auto_retry_runs_through_the_real_resume_path"
        status: pass
  - deliverable: "Смерть игры посреди дубля и класс «прочее» не повторяются автоматически (T-04-21)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#policy_retries_a_launch_failure_once_and_leaves_the_rest"
        status: pass
  - deliverable: "Два одинаковых сбоя «окружение» подряд ставят очередь на паузу с объяснением, остальные задачи ждут «Продолжить» (DIF-03)"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#environment_streak_pauses_the_queue_and_keeps_the_rest_queued"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#the_streak_is_broken_by_another_code_a_success_a_cancel_or_another_class"
        status: pass
      - kind: test
        ref: "crates/app/src/jobs_page.rs#pause_banner_explains_a_repeated_environment_failure"
        status: pass
  - deliverable: "Итог «N готово / M упало · первая причина» уходит в трей один раз и виден в окне и в подсказке значка; отменённые не считаются (OBS-04)"
    human_judgment: true
    rationale: "Вызов Host::notify, сообщение окну и тексты проверены тестами на FakeHost; видимость тоста и кириллицы в нём на машине автора спайком 04-03 не подтверждена (подтверждено только «уведомление показывается»), живая проверка со скрытым окном — учение D6 плана 04-11. Основной путь доставки итога — баннер и подсказка значка"
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#queue_end_notifies_once_with_done_and_failed"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#cancelled_job_is_not_counted_and_the_summary_does_not_wait_for_resume"
        status: pass
      - kind: test
        ref: "crates/app/src/queue.rs#auto_retried_job_that_succeeds_counts_as_one_done"
        status: pass
  - deliverable: "Пауза по повторяющемуся сбою тоже сообщается уведомлением в трее"
    human_judgment: false
    verification:
      - kind: test
        ref: "crates/app/src/queue.rs#repeated_failure_pause_is_notified"
        status: pass
  - deliverable: "Workspace тестируется и собирается, cs2-cli проходит crates/cli/tests/pipeline.rs"
    human_judgment: false
    verification:
      - kind: command
        ref: "cargo test --release --workspace"
        status: pass
      - kind: command
        ref: "cargo check --release --workspace; cargo build --release -p cli"
        status: pass

key-files:
  created: []
  modified:
    - crates/app/src/queue.rs
    - crates/app/src/app.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/ui.rs

key-decisions:
  - "Политика очереди — чистый QueuePolicy без часов и базы; поток очереди (Worker) применяет вердикт: Next, AutoRetry (requeue_auto + hold на AUTO_RETRY_PAUSE) или Pause"
  - "Код сбоя берётся из результата исполнителя до requeue_*, потому что они стирают fail_* в базе"
  - "DC-5b включён: сбой «запуск» с израсходованным авто-повтором считается в серию наравне с «окружением»"
  - "Итог очереди объявляется и при паузе, если ждущих задач нет: пауза без ждущих задач баннером не показывается, итог не должен ждать «Продолжить»; уведомление о паузе в этом случае заменяется итогом"

patterns-established:
  - "Тест очереди с настоящим исполнителем: FakeHost::sequence + дерево HLAE/CS2 во временной папке + record.json с невалидным дублем"
  - "Тексты тоста строит queue, а не окно: pause_text для баннера, pause_notice (без подсказки) для тоста, summary_text для тоста, баннера и подсказки значка"

requirements-completed: [DIF-04, DIF-03, OBS-04, RET-05]
---

# Phase 4 Plan 09: политика очереди без присмотра Summary

**Очередь сама повторяет сбой запуска один раз после паузы, встаёт на паузу после двух одинаковых сбоев окружения и сообщает итог «N готово / M упало» в трей и в окно.**

## Выполнено

1. **Task 1 (tracer), `1fb26b7`.** `QueuePolicy`/`Finished`/`Verdict`; в потоке очереди сбой класса «запуск» с `auto_retries < AUTO_RETRY_BUDGET` ставится `requeue_auto`, очередь держится `retry_pause` (20 с, в тестах 50 мс) через `QueueView.hold`; разделитель попытки `StartKind::Auto` выдаётся по `resume && auto_retries > 0`, поэтому переживает перезапуск приложения. В журнале строка «авто-повтор: сбой КОД на этапе «…» (класс «запуск»); пауза N с, затем запись заново по record.json». На ждущей карточке отметка «авто-повтор через N с» с перерисовкой раз в секунду. Интеграционный тест прогоняет настоящий `jobs::runner` на `FakeHost::sequence`: первая попытка «Повторить» падает `CONSOLE_NOT_OPEN`, вторая («авто-повтор») записывает оба дубля и склеивает.
2. **Task 2, `cf6a674`.** Серия итоговых сбоев (код + класс): два подряд для «окружения» (и для «запуска» без бюджета, DC-5b) дают `PauseReason::Repeated { code, class, jobs, message, hint }`; сообщение чистится `log::clean` и режется до 200 символов с «…». `queue::pause_text` и баннер объясняют код, класс, число задач, последнее сообщение и подсказку. `plural` вынесен из `jobs_page` в `ui`.
3. **Task 3, `84efa3e`.** `Summary`/`summary_text`; очередь, опустев после хотя бы одной законченной задачи, один раз вызывает `host.notify("CS2 Cinema", ..)` и шлёт `Msg::QueueSummary`; пауза по повторяющемуся сбою уведомляет «CS2 Cinema — очередь на паузе». В окне баннер итога с «Скрыть» и подсказка значка трея.

## Проверки

- `cargo test --release -p app --bin cs2cinema`: 41 тест, 0 упало (5 прогонов `queue` подряд без нестабильности).
- `cargo test --release --workspace`: 426 пройдено, 0 упало (включая `crates/cli/tests/pipeline.rs`).
- `cargo check --release --workspace` и `cargo build --release -p cli` без ошибок. Полная `cargo build --release --workspace` не прошла по причине окружения, не кода: `target\release\cs2cinema.exe` занят запущенным приложением автора («Отказано в доступе», os error 5), поэтому для `app` использован `cargo check`.

## Deviations from Plan

### Auto-fixed / принятые решения

**1. [Rule 2 - полнота] Итог при паузе без ждущих задач**
- **Найдено при:** Task 3.
- **Проблема:** по плану итог объявляется, когда «нет паузы». Но баннер паузы показывается только при ждущих задачах, а пауза после отмены последней задачи остаётся до следующей постановки: итог не пришёл бы, а позже слился бы со следующей партией.
- **Решение:** итог объявляется и тогда, когда очередь на паузе, но ждущих задач нет; уведомление о паузе `Repeated` в этом случае не шлётся (его заменяет итог). Покрыто тестами `cancelled_job_is_not_counted_and_the_summary_does_not_wait_for_resume` и `repeated_pause_without_waiting_jobs_is_replaced_by_the_summary`.
- **Коммит:** `84efa3e`.

**2. [Rule 2 - предел] Длина подсказки значка**
- План: 120 символов текста итога. Windows держит в подсказке до 127 символов, поэтому укорачивается вся строка вместе с префиксом «CS2 Cinema — » до 120 символов.

**3. Уведомление о паузе без подсказки**
- Тост режется хостом по длине; подсказка кода (до 170 символов) вытеснила бы суть, а «Исправьте причину» всё равно обрезалась бы. Тост получает `pause_notice` (код, класс, число задач, последнее сообщение), баннер в окне — полный `pause_text` с подсказкой. Это сужение «текстом баннера паузы» из плана.

**4. Тест интеграции — копия фикстуры из `crates/pipeline/tests/job.rs`**
- Помощники интеграционных тестов `pipeline` недоступны из `app`, поэтому дерево HLAE/CS2, `record.json` и манифест воспроизведены в `queue.rs` (около 70 строк тестового кода).

Иных отклонений нет.

## Спайк 04-03 и видимость тоста

Из 04-03-SUMMARY: подтверждено только «уведомление показывается» (A2). Видимость кириллицы и длительность тоста (A3), а также работа очереди при окне в трее (A1) не проверены. Поэтому доставка итога не зависит от тоста: тот же текст идёт в баннер над списком задач, в строку состояния и в подсказку значка трея. `notify` возвращает `false` — в окно уходит «уведомление в трее не показано — итог в окне». Проверка на машине автора остаётся за учением D6 (04-11).

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей. Текст сбоя, попадающий в баннер, уведомление и подсказку значка, проходит `clean` (управляющие символы, секреты, путь профиля) и предел 120/200 символов, затем `notify_text` хоста (T-04-22). Авто-повтор ограничен `AUTO_RETRY_BUDGET = 1`, переживает перезапуск через `auto_retries` в базе и оставляет «авто-повтор» в журнале (T-04-21, T-04-23).

## Для следующих планов

- 04-10/04-12 (приёмка): различать авто-повтор и ручное «Повторить» по `journal::attempts` (`StartKind::Auto`) и `jobs.auto_retries`.
- 04-11 (учения): D4 (чужой CS2), D5 (нет ffmpeg) должны привести к паузе `Repeated` после второй задачи; D6 — тост при скрытом окне, кириллица, длительность.

## Self-Check: PASSED

- `crates/app/src/queue.rs`, `app.rs`, `jobs_page.rs`, `ui.rs`: найдены в рабочем дереве.
- Коммиты `1fb26b7`, `cf6a674`, `84efa3e` лежат в истории `main`.
