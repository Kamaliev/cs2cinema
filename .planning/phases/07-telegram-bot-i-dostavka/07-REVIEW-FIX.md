---
phase: 07-telegram-bot-i-dostavka
fixed_at: 2026-10-12T00:00:00Z
review_path: .planning/phases/07-telegram-bot-i-dostavka/07-REVIEW.md
iteration: 1
findings_in_scope: 12
fixed: 12
skipped: 0
status: all_fixed
---

# Phase 7: Code Review Fix Report

**Fixed at:** 2026-10-12
**Source review:** `.planning/phases/07-telegram-bot-i-dostavka/07-REVIEW.md`
**Iteration:** 1

**Summary:**
- Findings in scope: 12 (критических 2, предупреждений 10; Info не трогались)
- Fixed: 12
- Skipped: 0

**Проверка:** все гейты запускались в основной рабочей копии (`workflow.use_worktrees = false`, изолированного
worktree не было). `cargo test --workspace --release`: 876 тестов прошли, 0 упали, 1 проигнорирован (как и раньше),
включая `crates/cli/tests/pipeline.rs`. Каждое исправление проверялось отдельным регрессионным тестом на
`FakeTelegram` и настоящем сервере с базой. Правки в логике (CR-02, WR-01, WR-03, WR-05, WR-07, WR-08, WR-09, WR-10)
помечены «требуют проверки человеком»: тесты покрывают описанные сценарии, но не все сочетания состояний.

## Fixed Issues

### CR-01: Проверка пути из `getFile` ломает приём файлов на Windows-сервере с bot-api в другой системе

**Статус:** fixed
**Commit:** c1910fc
**Files modified:** `crates/server/src/config.rs`, `crates/server/src/bot/ingest.rs`
**Applied fix:** `config::is_abs` стала `pub(crate)`, `safe_local_path` вызывает её вместо `Path::is_absolute()`, так что
путь вида `/var/lib/telegram-bot-api/...` принимается на Windows так же, как его принимает проверка настройки. Добавлены
тесты: Windows-путь с unix-корнем принимается (`#[cfg(windows)]`) и чужой корень отвергается; относительный путь и
`..` по-прежнему отвергаются.

### CR-02: `/retry` может потерять доставку ролика: задача ставится раньше, чем создаётся `tg_jobs`

**Статус:** fixed: requires human verification
**Commit:** a7082dc
**Files modified:** `crates/server/src/bot/commands.rs`, `crates/server/tests/bot_status.rs`
**Applied fix:** после `resubmit` сразу, до любых вызовов Telegram, создаётся строка `tg_jobs` с пустым `status_message_id`
(`next_edit_ms` отодвинут, чтобы тик не слал статус параллельно с подтверждением). Подтверждение отправляется потом,
номер сообщения дописывается через `set_status_message`; если сообщение не ушло, статус пришлёт тик. Если `insert_tg_job`
не удался, новая задача снимается (`cancel(.., true)`), а другу уходит честный ответ. Тест: подтверждение теряется
(`fail_next("send_message")`), строка `tg_jobs` существует, тик потом присылает статус. Оговорка: порядок «вставка раньше
вызова Telegram» тест проверяет по итоговому состоянию, а не по моменту вызова (в `FakeTelegram` нет хука на время вызова).

### WR-01: Дубли ищутся по всем пользователям, а `/resend` разрешён только владельцу

**Статус:** fixed: requires human verification
**Commit:** 1d8dfa3
**Files modified:** `crates/server/src/core.rs`, `crates/server/src/bot/ingest.rs`, `crates/server/tests/bot_ingest.rs`
**Applied fix:** новый `Core::find_duplicate_for(key, user_id)`: дубль для пользователя это его собственная задача
(активная или готовая) либо чужая ещё идущая; чужой готовый ролик дублем не считается, и второй друг получает свою
задачу. Все четыре места приёма и `other_duplicate` переведены на него. `duplicate_text` для чужой активной задачи
отвечает «уже в очереди — дождитесь результата» без номера и без `/resend`. `find_duplicate` оставлен как был.

### WR-02: После перезапуска «неясная» доставка остаётся молчаливой

**Статус:** fixed
**Commit:** 7657f75
**Files modified:** `crates/server/src/bot/store.rs`, `crates/server/src/bot/deliver.rs`, `crates/server/src/bot/mod.rs`, `crates/server/tests/bot_flow.rs`
**Applied fix:** `store::recover` возвращает номера задач, чья отправка стала `uncertain`; `Bot::recover` рассылает им то же
сообщение «Не уверен, что ролик дошёл… /resend N» (`deliver::tell_uncertain_after_restart`, отметка `final_reported_ms`
защищает от дубля). Текст вынесен в `uncertain_text`. Тест перезапуска теперь проверяет ровно одно такое сообщение.
Админу, как и в обычном пути `fail_delivery`, об `uncertain` не сообщается.

### WR-03: `/retry` не проверяет дубли и повторную постановку

**Статус:** fixed: requires human verification
**Commit:** 24117f8
**Files modified:** `crates/server/src/bot/commands.rs`, `crates/server/src/bot/ingest.rs`, `crates/server/tests/bot_status.rs`
**Applied fix:** перед `resubmit` строится `DedupKey` по хешу демки или ссылке FACEIT и игроку, и `find_duplicate_for`
отвечает «уже в очереди: задача #N» (или `/resend`, если есть готовая своя). Это же решает «`/retry` без номера дважды»:
второй вызов видит идущий повтор. `ingest::duplicate_text` стал `pub(super)`. Тест: второй `/retry` и `/retry 1` не
создают третью задачу.

### WR-04: Ник из подписи и после ссылки не проходит `valid_player`

**Статус:** fixed
**Commit:** a230805
**Files modified:** `crates/server/src/bot/ingest.rs`, `crates/server/tests/bot_ingest.rs`
**Applied fix:** `choose_player` возвращает `Result<String, String>`: непустая подпись или остаток после ссылки проходит
`commands::valid_player` (длина до 64, без управляющих и bidi-символов), при ошибке друг получает «Не принял ник из
подписи: <причина>», молчаливое обрезание убрано. Тест: подпись 70 символов, подпись с переводом строки и ссылка с
U+202E отклоняются, во входящие ничего не попадает, `getFile` не вызывается.

### WR-05: Любая ошибка `getFile` отвергает входящее навсегда

**Статус:** fixed: requires human verification
**Commit:** 5da8a2f
**Files modified:** `crates/server/src/db.rs`, `crates/server/src/bot/store.rs`, `crates/server/src/bot/ingest.rs`, `crates/server/tests/bot_ingest.rs`
**Applied fix:** третий шаг миграции `tg_inbox`: колонки `attempts` и `next_try_ms`. `claim_pending` берёт только строки
`next_try_ms <= now`. Временные сбои `getFile` (`Timeout`, `Io`, `429`, `5xx`) откладывают строку (`defer_inbox`: 30 с, 2 мин,
8 мин или `retry_after`, потолок 10 мин) до 4 попыток, после чего она отвергается; `400/403/404` отвергаются сразу. Тест:
`Io` откладывает и принимает после срока, `400` отвергает сразу.

### WR-06: Скачанный файл остаётся в папке bot-api при отказах после `getFile`

**Статус:** fixed
**Commit:** 8b6794b
**Files modified:** `crates/server/src/bot/ingest.rs`, `crates/server/tests/bot_ingest.rs`
**Applied fix:** `OriginalGuard` (удаление файла в `Drop`) создаётся сразу после успешной проверки пути, поэтому оригинал
уходит при любом выходе (отказ `submit`, ошибка записи, `?`). При несовпадении размера `safe_local_path` сам удаляет файл,
но только после проверки, что он внутри корня (проверка корня переставлена перед проверкой размера). Тест
`size_mismatch_is_refused` проверяет, что файл убран.

### WR-07: Любая ошибка `?` в `ingest_once` навсегда оставляет строку в `downloading`

**Статус:** fixed: requires human verification
**Commit:** 9bf466a
**Files modified:** `crates/server/src/bot/ingest.rs`, `crates/server/src/bot/store.rs`, `crates/server/tests/bot_ingest.rs`
**Applied fix:** тело вынесено в `ingest_row`; при `Err` `ingest_once` вызывает `release_after_error`: строка, всё ещё
`downloading`, возвращается в `pending` с отсрочкой (или отвергается после 4 попыток). Ошибка наружу идёт прежняя.
`defer_inbox` срабатывает только для `downloading`. В `queue_row` `mark_queued` вызывается первым, `remember_file` после и
с `warn` (в ветке дубля он тоже не фатален). Тесты: сбой кэша файла (триггер на `tg_files`) не оставляет задачу без
`tg_jobs`; сбой `mark_queued` (триггер) возвращает строку в `pending`, повтор привязывается к уже созданной задаче.

### WR-08: `/resend` по `file_id` синхронно блокирует поток опроса на срок передачи

**Статус:** fixed: requires human verification
**Commit:** 6dc06c0
**Files modified:** `crates/server/src/bot/deliver.rs`, `crates/server/tests/bot_delivery.rs`
**Applied fix:** `resend_by_id` и `ByIdOutcome` удалены. `/resend` доставленного ролика только возвращает доставку в `pending`
(ответ «Отправляю ролик задачи #N ещё раз.»). `deliver_once` в потоке доставки, если известен `result_file_id`, сначала шлёт
по `InputFile::Id`, а при `400` от Telegram переходит к отправке с файла в том же вызове. Тест: после опроса без доставки
`send_video` не вызван, затем уходят вызовы по `file_id` и (после `400`) с файла.

### WR-09: Отчёт админам занимается до отправки и теряется целиком при сбое

**Статус:** fixed: requires human verification
**Commit:** 24045ac
**Files modified:** `crates/server/src/bot/store.rs`, `crates/server/src/bot/notify.rs`, `crates/server/tests/bot_delivery.rs`
**Applied fix:** новый `store::unclaim_admin_report`. Отметка снимается при ошибке `list_users` и когда ни одно сообщение
точно не ушло (сеть, `429`); `maybe_delivered` и `403` считаются «улаженными» и повтор не вызывают. Если подходящих админов
нет, в журнал пишется `warn`, отметка остаётся (повтор ничего не изменит и забил бы журнал каждым тиком). Тест: оба
сообщения (другу и админу) не дошли, следующий тик присылает отчёт админу ровно один раз.

### WR-10: Ошибка записи итога доставки оставляет `sending` без повтора и без отчёта

**Статус:** fixed: requires human verification
**Commit:** a015228
**Files modified:** `crates/server/src/bot/deliver.rs`, `crates/server/tests/bot_delivery.rs`
**Applied fix:** итог отправки пишется через `record_sent`/`record_failure` с тремя попытками (`with_retries`, пауза 100 мс);
`deliver_once` после вызова Telegram больше не возвращает `Err`. Если запись так и не удалась, `outcome_lost` отдельной записью
ставит `uncertain` и говорит другу про `/resend`. Тест: триггер блокирует запись `sent`, доставка становится `uncertain`,
друг получает сообщение, повторной отправки нет, `step()` не падает.

## Skipped Issues

None — все находки в области исправления применены.

## Дополнительно

- Отдельный коммит 8b372f1 (`style(07)`) содержит только `rustfmt` для затронутых файлов `crates/server`.
- Схема базы сервера выросла до версии 3 (WR-05, добавлением колонок); существующие базы мигрируют сами.
- Info-находки (IN-01…IN-06) по заданию не трогались.

---

_Fixed: 2026-10-12_
_Fixer: Claude (gsd-code-fixer)_
_Iteration: 1_
