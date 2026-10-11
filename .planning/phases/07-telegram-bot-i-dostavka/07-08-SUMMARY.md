---
phase: 07-telegram-bot-i-dostavka
plan: 08
subsystem: server
tags: [telegram, bot, status, throttling, cancel, retry, queue]
status: complete

requires:
  - phase: 07-telegram-bot-i-dostavka
    plan: 03
    provides: Core::jobs_ahead, runner_online, resubmit, active_counts
  - phase: 07-telegram-bot-i-dostavka
    plan: 06
    provides: ingest::admit (лимиты D-03)
  - phase: 07-telegram-bot-i-dostavka
    plan: 10
    provides: поток bot-notify крутит notify::tick без изменений
provides:
  - "notify::status_text: этап по-русски, дубль k из n, место в очереди, отмена запрошена, строка «Раннер сейчас не в сети»"
  - "notify::tick: правка сообщения статуса только при изменении текста и now >= next_edit_ms; 429 / 403 / not modified / message not found обрабатываются по чатам"
  - "notify::status_reply, queue_reply: ответы /status и /queue только по задачам пользователя"
  - "/cancel [N] и /retry [N] в commands.rs; меню и справка дополнены"
  - "store: status_candidates, note_status, defer_edit, set_status_message, mute_chat, claim_final / unclaim_final, insert_tg_job (NewTgJob), user_job_ids, working_count"
affects: [07-09, 07-11]

actuals:
  tokens: 16000
  tasks: 3
  commits: 3
plan_head_before: 2acf4f2972233e6e65d6744a9c36c290c02aaf1f
plan_head_after: 48cbd9cb8118be7942571008665d1d905010c10f
commits: 3

tech-stack:
  added: []
  patterns:
    - "одно сообщение статуса на задачу: текст строит один код (status_text) и для первой правки при постановке, и для тика, поэтому частота считается с первой правки"
    - "право сказать «отменена» занимает UPDATE ... WHERE final_reported_ms IS NULL: команда и тик не могут прислать сообщение дважды"
    - "ошибка правки классифицируется в одном месте (on_error): 403 глушит чат целиком, 429 откладывает на retry_after, прочее на обычный интервал"

key-files:
  created:
    - crates/server/tests/bot_status.rs
  modified:
    - crates/server/src/bot/notify.rs
    - crates/server/src/bot/commands.rs
    - crates/server/src/bot/store.rs
    - crates/server/src/bot/ingest.rs

key-decisions:
  - "Первое сообщение после постановки задачи (ранее текст собирал ingest::queue_row) идёт через notify::push_now: тот же текст, что у тика, и эта правка занимает слот status_edit_ms"
  - "Заглушаются все строки tg_jobs чата (UPDATE ... WHERE chat_id), а не одна задача: заблокированный бот недоступен для всех задач этого чата"
  - "/cancel в чате владельца занимает final_reported_ms сам и отвечает «Задача #N отменена.»; админ, отменяющий чужую задачу, не занимает: владельцу скажет тик"
  - "Несуществующий номер отвечает так же, как чужой («Это не ваша задача.»): наличие чужих задач не раскрывается"
  - "/retry доступен только владельцу, в том числе админу: повтор занимает лимиты D-03 владельца"
  - "status_text не принимает &TgJob (в плане был в сигнатуре): текст зависит только от задачи и состояния раннера"

requirements-completed: []
---

# Phase 7 Plan 08: ход задачи в чате и управление ею Summary

Друг видит одну строку статуса, которая правится не чаще раза в 10 секунд (этап, «дубль k из n», место в очереди, «раннер не в сети»), спрашивает `/status` и `/queue`, отменяет задачу через `/cancel` и повторяет упавшую через `/retry` без повторной загрузки демки.

## Accomplishments

- **Сообщение статуса.** `notify::status_text` собирает «Задача #N (игрок X): запись, дубль 3 из 5» для идущей и «Задача #N в очереди (игрок X), перед вами K.» для ожидающей (при пустой очереди «очередь пуста», как в подтверждении 07-06). Этапы по-русски из `jobs.phase`, пустой или незнакомый этап заменяется «выдана раннеру» или «в работе». Запрошенная отмена добавляет строку «Отмена запрошена — жду остановки раннера.», офлайн-раннер — «Раннер сейчас не в сети — задача подождёт его.».
- **Частота.** Тик правит только изменившийся текст и только при `now >= next_edit_ms`; после правки `next_edit_ms = now + status_edit_ms`. Время только из часов ядра. Первая правка при постановке задачи тоже записывает текст и срок, поэтому окно 10 секунд считается с неё.
- **Ошибки правки по чатам.** `429` откладывает правку этого чата на `retry_after`; `message is not modified` считается успехом (текст запоминается); `message to edit not found` и отсутствие номера сообщения создают новое сообщение статуса и сохраняют его номер; `403` ставит `muted = 1` всем строкам чата, и правки с отчётами в него прекращаются. Ошибка одного чата не прерывает цикл по остальным; текст ошибок в журнале проходит `Redactor`.
- **`/status`.** Активные задачи пользователя: состояние, этап, дубль, «перед вами K» (по `jobs_ahead`, включая 0), до трёх недавно завершённых с подсказкой `/retry N`, строка про офлайн-раннер при активных задачах; без задач — «У вас нет активных задач.».
- **`/queue`.** «В очереди X, в работе Y.» по общей очереди, места только своих задач («#N — впереди K» или «#N — в работе»), офлайн-строка.
- **`/cancel [N]`.** Без номера — самая свежая активная задача. В очереди — отмена сразу; у раннера — флаг (мягкий, при повторе жёсткий; `force` передаётся явно), ответ «Отменяю задачу #N — раннер остановит запись». Когда раннер завершает задачу как `Cancelled` (или возвращает её через `release` при запрошенной отмене), тик шлёт «Задача #N отменена.» ровно один раз и правит сообщение статуса на «Задача #N отменена».
- **`/retry [N]`.** Без номера — последняя упавшая или отменённая задача. `admit` (лимиты D-03) до `Core::resubmit`, `getFile` не вызывается; новое сообщение статуса «Задача #M — повтор задачи #N, …» и строка `tg_jobs`. Готовая задача отвечает подсказкой `/resend N`, идущая — «ещё идёт». FACEIT-задача повторяется с той же ссылкой.

## Tests

`crates/server/tests/bot_status.rs` (8): `status_message_is_edited_at_most_every_ten_seconds_with_stage_and_take`, `status_and_queue_show_places_and_an_offline_runner`, `offline_runner_is_shown_in_the_status_message`, `edit_errors_are_handled_per_chat`, `cancel_stops_a_queued_job_and_flags_a_running_one`, `cancel_then_release_is_cancelled_not_requeued`, `retry_requeues_from_the_stored_demo_without_get_file`, `retry_respects_limits_and_ownership`.

Зелёные: `cargo test --release -p server` целиком (все наборы), `cargo test --release -p cli --test pipeline`. `cargo clippy --release -p server --all-targets` в изменённых файлах предупреждений не даёт. Форматирование — `rustfmt` по файлам `notify.rs`, `commands.rs`, `store.rs`, `bot_status.rs` (`ingest.rs` не форматировался: в нём уже были расхождения вне этого плана).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Первое сообщение после постановки не учитывалось в частоте правок**
- **Found during:** Task 1 (проектирование теста)
- **Issue:** `ingest::queue_row` правил подтверждение своим текстом и не записывал ни текст, ни `next_edit_ms`; тик сразу после этого сделал бы вторую правку (и с другим текстом из-за офлайн-строки).
- **Fix:** `queue_row` зовёт `notify::push_now`, который строит текст тем же `status_text`, правит сообщение и записывает текст со сроком; если номера сообщения нет, тик пришлёт его новым.
- **Files modified:** crates/server/src/bot/ingest.rs, crates/server/src/bot/notify.rs
- **Commit:** 60b90fa

**2. Сигнатура `status_text`.** В плане `status_text(bot, job, tg)`; параметр `tg: &TgJob` ничего не давал, поэтому сигнатура `status_text(bot, job)`. Критерий `pub fn status_text(` выполнен.

**3. Коммиты по слоям, а не по заданиям.** Три задачи правят общие `notify.rs` и `commands.rs`, поэтому коммиты: реализация статуса (`store.rs`, `notify.rs`, `ingest.rs`), команды (`commands.rs`), тесты (`bot_status.rs`). Тесты писались по `<behavior>` и прошли после правки двух предположений самих тестов (кодом не менялось): `release` отвечает 204, а молчание раннера дольше окна «в сети» само по себе меняет текст (появляется офлайн-строка) — тест теперь проверяет и это.

**4. Мелкие дополнения.** `/status` показывает до трёх недавних завершённых задач (иначе после сбоя не видно номера для `/retry`); `ACTIVE_STATES` в `ingest.rs` стал `pub(super)` и используется в `store.rs` и `commands.rs`; `ahead_phrase` — `pub(super)`.

## Known Limitations

- **Окно в `/retry`.** Между `Core::resubmit` и вставкой строки `tg_jobs` задача уже в очереди; если процесс упадёт именно в этот момент, она выполнится без сообщений в чат (ролик не будет отправлен, пока строка не появится). Команды разбирает один поток опроса, поэтому гонки команд нет.
- **Слепые зоны статуса.** После `done`, `failed` сообщение статуса не правится: готовый ролик приходит отдельным сообщением (07-02), сбой и причину сообщает план 07-09.
- **Сообщение «отменена» при сомнении не дублируется.** Если `send_message` оборвался после начала отправки, отметка `final_reported_ms` остаётся (возможно, сообщение дошло); при явной неудаче без отправки отметка снимается, и тик повторит.
- **Уведомление автору о падении раннера** остаётся BOT-F1 (отложено): офлайн виден только в ответах и в сообщении статуса.

## Known Stubs

Нет. `/resend N`, на которую ссылается ответ про готовую задачу, появится в плане 07-09.

## Threat Flags

Новых поверхностей сверх `<threat_model>` нет. Закрыты: T-07-32 (владелец проверяется в `/cancel` и `/retry`, админ только отменяет; `/status` и `/queue` видят свои задачи; тест `retry_respects_limits_and_ownership`), T-07-33 (правка не чаще `status_edit_ms` и только при изменении текста; 429 и 403 по чатам; тест `edit_errors_are_handled_per_chat`), T-07-34 (`admit` перед `resubmit`; лимит проверяется тестом), T-07-SC (новых пакетов нет).

## Self-Check: PASSED

- FOUND: `pub fn status_text(`, `pub fn status_reply(`, `pub fn queue_reply(`, `next_edit_ms`, `status_edit_ms`, `is_forbidden()` в crates/server/src/bot/notify.rs
- FOUND: ветки `cancel` и `retry`, вызовы `resubmit(` и `admit(` в crates/server/src/bot/commands.rs
- FOUND: crates/server/tests/bot_status.rs, 8 тестов `#[test]` (требовалось не меньше семи)
- FOUND: коммиты 60b90fa, 9dbeed2, 48cbd9c в истории main
