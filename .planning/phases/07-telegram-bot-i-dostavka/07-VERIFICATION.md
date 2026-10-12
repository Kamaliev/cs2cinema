---
phase: 07-telegram-bot-i-dostavka
verified: 2026-10-12T12:00:00Z
status: passed
score: 4/5 must-haves verified
covered_files:
  - .planning/phases/07-telegram-bot-i-dostavka/07-01-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-01-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-02-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-02-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-03-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-03-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-04-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-04-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-05-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-05-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-06-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-06-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-07-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-07-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-08-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-08-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-09-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-09-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-10-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-10-SUMMARY.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-11-PLAN.md
  - .planning/phases/07-telegram-bot-i-dostavka/07-11-SUMMARY.md
  - Cargo.toml
  - crates/cli/src/runner_cmd.rs
  - crates/pipeline/src/record/failure.rs
  - crates/runner/Cargo.toml
  - crates/runner/src/exec.rs
  - crates/runner/src/lib.rs
  - crates/runner/tests/exec.rs
  - crates/runner/tests/faceit_flow.rs
  - crates/server/Cargo.toml
  - crates/server/cs2-server.example.json
  - crates/server/src/blobs.rs
  - crates/server/src/bot/commands.rs
  - crates/server/src/bot/deliver.rs
  - crates/server/src/bot/dispatch.rs
  - crates/server/src/bot/ingest.rs
  - crates/server/src/bot/mod.rs
  - crates/server/src/bot/notify.rs
  - crates/server/src/bot/store.rs
  - crates/server/src/cli.rs
  - crates/server/src/config.rs
  - crates/server/src/core.rs
  - crates/server/src/db.rs
  - crates/server/src/http.rs
  - crates/server/src/lib.rs
  - crates/server/src/rules.rs
  - crates/server/src/secret.rs
  - crates/server/src/tg/fake.rs
  - crates/server/src/tg/http.rs
  - crates/server/src/tg/mod.rs
  - crates/server/src/users.rs
  - crates/server/tests/bot_access.rs
  - crates/server/tests/bot_delivery.rs
  - crates/server/tests/bot_e2e.rs
  - crates/server/tests/bot_faceit.rs
  - crates/server/tests/bot_flow.rs
  - crates/server/tests/bot_ingest.rs
  - crates/server/tests/bot_status.rs
  - crates/server/tests/common/mod.rs
  - crates/server/tests/core_bot.rs
  - crates/server/tests/lease_http.rs
  - crates/server/tests/operator.rs
  - crates/server/tests/protocol_http.rs
  - crates/server/tests/secrets.rs
  - crates/server/tests/tg_http.rs
  - crates/wire/src/faceit.rs
  - crates/wire/src/lib.rs
covered_digest: "v3:sha256:483ecb909030a88e15c0ec4527875af53b571463c521020edfd9680743247631"
behavior_unverified: 0
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 4/5
  gaps_closed: []
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "Живая цепочка: демка друга через бота на локальном `telegram-bot-api --local` → `cs2-server serve` → `cs2-cli runner` с настоящими CS2 и HLAE → ролик в чате друга"
    expected: "Подтверждение с местом в очереди сразу; правки статуса не чаще раза в 10 с с этапом и «дубль k из n»; ролик приходит новым сообщением, воспроизводится на телефоне и в Telegram Desktop; `job show` показывает «доставка в Telegram: sent»"
    why_human: "Нужны настоящий Telegram, локальный bot-api, CS2/HLAE и автор за машиной; в тестах Telegram подменён FakeTelegram и поддельным HTTP Bot API. План 07-11 пропущен по решению автора."
  - test: "Спайк: `sendVideo` по `file://` на 0,5 / 1,0 / 1,5 ГБ против `sendDocument`; нужны ли `width/height/duration`"
    expected: "Ролик показывается как видео с превью и правильным соотношением сторон без метаданных (допущение A3); иначе значение `bot_send_as` по умолчанию меняется на `document` или в протокол раннера добавляются метаданные видео"
    why_human: "Поведение настоящего Telegram на больших файлах и в клиентах автоматически не проверить. Сейчас умолчание `bot_send_as = video` и `supports_streaming: true` выбраны без замера."
  - test: "Спайк: права и пути общего тома (`bot_api_files_root`, `bot_api_files_root_local`, `bot_api_results_prefix`), `getFile` с абсолютным путём, удаление исходника после копирования, `logOut` тестового бота"
    expected: "Сервер читает и удаляет файл в рабочей папке bot-api; путь `getFile` проходит проверку `files_root`; рабочая папка bot-api не растёт после приёма"
    why_human: "Проверка пути и удаления в тестах идёт на подставной папке (`file_outside_the_bot_api_root_is_refused`, `original_is_removed_after_copy`); права настоящего bot-api (другой пользователь, WSL/Docker) не проверялись."
  - test: "Учения вживую: незнакомец (только ID, файл не скачивается), ссылка FACEIT от друга, `/cancel`, `/retry`, `/status`, `/queue`, сбой задачи (друг получает фразу, админ — код, этап, подсказку, число выдач)"
    expected: "Каждое учение даёт ожидаемый текст по-русски без локальных путей; улики сохранены текстом"
    why_human: "Тесты проверяют те же сценарии на FakeTelegram и FakeHost; вживую не проводились."
  - test: "Поиск токена бота (формы `:` и `~`), токена раннера и ключа FACEIT в настоящих журналах сервера и раннера, `job show` и сообщениях чата после живого прогона"
    expected: "Ни одного совпадения"
    why_human: "Тесты `bot_token_in_tilde_form_is_masked_everywhere` и родственные покрывают подставные секреты; настоящие журналы стенда не просматривались."
---

# Фаза 7: Telegram-бот и доставка — отчёт о проверке

**Цель фазы:** Автор и друзья из белого списка отправляют боту демку файлом (до 2 ГБ через локальный `telegram-bot-api`) или ссылку FACEIT, видят место в очереди и ход записи и получают готовый ролик новым сообщением или понятную причину сбоя, а незнакомцы не могут занять ни диск, ни очередь.
**Проверено:** 2026-10-12 (повторно, после исправлений код-ревью)
**Статус:** human_needed
**Повторная проверка:** да — после исправлений код-ревью (`07-REVIEW-FIX.md`, 12 из 12: CR-01, CR-02, WR-01…WR-10; коммиты c1910fc…e08b5af); прежних gaps не было, регрессий нет

## Итог

Всё, что можно доказать кодом и тестами на фейках, подтверждено: `cargo test --workspace --release` выполнен в этой проверке заново на коде после исправлений ревью, код выхода 0, 68 строк `test result: ok`, 876 пройдено (было 864: +12 регрессионных тестов исправлений), 0 упало, 1 игнорируемый (`host`, окно на рабочем столе, ручной запуск). `crates/cli/tests/pipeline.rs` зелёный. Бот-тесты: `bot_access` 12, `bot_ingest` 21, `bot_status` 10, `bot_delivery` 14, `bot_faceit` 7, `bot_flow` 11, `bot_e2e` 5, `core_bot` 9, `tg_http` 7, `secrets` 7, плюс `runner/tests/faceit_flow.rs`. Критерий 5 (живой стенд) и живые части критериев 2–4 не доказаны: план 07-11 пропущен по решению автора (07-11-SUMMARY.md: `status: skipped-by-author`; `07-SPIKE.md`, `07-LIVE-STAND.md`, `evidence/` не созданы). Они не засчитаны как пройденные и вынесены в `human_verification`. Невыполненных по коду пунктов (gaps) нет. Исправления код-ревью (миграция схемы базы сервера до версии 3 — колонки `tg_inbox.attempts` и `tg_inbox.next_try_ms`, новый порядок `/retry`, доставка по `file_id`, повтор отчёта админам, запись итога доставки) прочитаны в коде и проверены запуском тестов; отдельный раздел ниже. Долг оформлен так же, как 06-11 в фазе 6.

## Достижение цели

### Наблюдаемые истины (критерии успеха из ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | Незнакомец получает только свой ID, `getFile` не вызывается ни для документа, ни для пересланного/отредактированного; `/adduser`, `/removeuser`, `/users` | ✓ VERIFIED | `dispatch::decide`: нет `message` → игнор; не личный чат или бот → игнор; `find_user_by_tg` + `is_active` до любого разбора документа и текста; незнакомцу один ответ с ID, не чаще `stranger_reply_ms`. `getUpdates` запрашивает `allowed_updates: ["message"]` (`tg/http.rs:323`). Тесты: `stranger_document_is_never_fetched`, `stranger_gets_only_the_id_and_nothing_is_fetched` (`get_file_calls() == 0`), `edited_channel_and_group_updates_are_ignored` (edited_message, channel_post, my_chat_member, callback_query, группа: 0 сообщений, 0 `getFile`, смещение сдвинуто), `stranger_reply_is_rate_limited`, `forwarded_document_from_a_friend_is_his_own_demo`, `member_cannot_run_admin_commands`, `adduser_never_grants_admin`, `admin_adds_and_removes_a_friend_from_the_chat`, `removed_friend_job_still_finishes_and_is_delivered`. |
| 2 | `/player` (ник/SteamID64); `.dem`/`.dem.zst` или ссылка FACEIT; подтверждение и место в очереди сразу; игрок в задаче; отказ по магии/лимиту с объяснением; дубль по хешу; несколько игроков на одной демке; лимит активных задач; ссылку скачивает раннер, ключ не на сервере | ✓ VERIFIED (код и тесты) | `ingest::precheck`: размер и имя до `getFile`; `known_file` по `file_unique_id`; `admit` в той же транзакции, что и вставка (2/5/10, считает и ещё не скачанные строки `tg_inbox`); проверка пути внутри `files_root`; магия — `sniff_demo` в `Core::submit`; ссылка — строгий `wire::faceit::room_match_id`, на сервере хранится канонический URL, сервер по нему не ходит. Тесты: `oversized_or_sizeless_or_non_demo_name_is_refused_before_get_file`, `garbage_bytes_with_dem_name_are_refused_after_download_without_a_job`, `same_demo_for_another_player_becomes_a_second_job_without_a_second_download`, `same_bytes_from_another_upload_are_found_by_hash`, `friend_limit_counts_pending_downloads`, `admin_limit_is_five_and_queue_limit_is_ten`, `ack_shows_the_place_in_queue_before_the_download`, `player_is_saved_and_fixed_in_the_job`, `caption_overrides_the_saved_player_for_one_job`, `player_with_control_or_bidi_characters_is_refused`, `file_outside_the_bot_api_root_is_refused`, `dot_dot_path_is_refused`, `symlinked_file_is_refused`, `size_mismatch_is_refused`; FACEIT: `strict_link_is_queued_without_get_file`, `non_room_links_are_refused_with_an_explanation`, `faceit_link_from_a_friend_is_downloaded_by_the_runner_and_delivered` (ключ только у раннера из `FACEIT_API_KEY`, флага нет), `crates/server/tests/deps.rs` (`server` не тянет `faceit` и Windows-крейты). |
| 3 | Одно сообщение статуса не чаще интервала, этап и «дубль k из n»; `/status` и `/queue` с позицией; офлайн-раннер называется; `/cancel` останавливает; `/retry` по сохранённой демке без повторной загрузки | ✓ VERIFIED (код и тесты) | `status_message_is_edited_at_most_every_ten_seconds_with_stage_and_take` (виртуальные часы), `status_and_queue_show_places_and_an_offline_runner`, `offline_runner_is_shown_in_the_status_message`, `edit_errors_are_handled_per_chat`, `cancel_stops_a_queued_job_and_flags_a_running_one`, `cancel_then_release_is_cancelled_not_requeued` (WR-01 закрыт в 07-01), `retry_requeues_from_the_stored_demo_without_get_file`, `retry_respects_limits_and_ownership`, `resubmit_reuses_the_demo_by_a_hard_link`. Поведенческие истины (троттлинг, отмена, повтор) подтверждены запуском тестов, а не наличием символов. |
| 4 | Ролик новым сообщением и воспроизводится в Telegram; неясный итог → `uncertain`, `/resend` без слепого повтора; сбой: другу фраза без путей, админу код, этап, подсказка, число попыток | ✓ VERIFIED кодом и тестами; воспроизведение в клиентах — human_needed | `deliver_once`: `pending` → `sending` (запись до вызова Telegram) → `sent`/`uncertain`/`failed`; после перезапуска `sending` → `uncertain`; `sendVideo` с `supports_streaming: true`. Тесты: `friend_demo_becomes_a_job_and_the_video_comes_back_as_a_new_message`, `timeout_after_upload_marks_uncertain_and_resend_sends_once_more`, `sending_delivery_becomes_uncertain_after_restart_without_a_resend`, `sent_video_is_resent_by_file_id`, `rate_limit_and_unsent_errors_retry_without_duplicates` (429), `forbidden_marks_undeliverable` (403), `oversized_result_is_refused_before_sending` (1950 МиБ), `document_mode_sends_a_document`, `other_users_cannot_resend`, `failure_sends_a_friend_phrase_and_an_admin_report`, `server_side_failures_are_reported_too`, `admin_owner_gets_both_messages_and_a_muted_friend_only_the_admin_report`, `failure_report_reaches_friend_and_admin_through_the_running_server`. Фраза другу (`failure_text_for_friend`) — таблица фраз по коду и `/retry N`, без путей; админу (`failure_text_for_admin`) — код, состояние, фаза, подсказка, число выдач раннеру, сообщение раннера через `Redactor::clean`. Воспроизведение роликов 0,5–1,5 ГБ без `width/height/duration` не проверено (спайк). |
| 5 | Демка друга проходит всю цепочку на реальном стенде; токен бота не в журналах и сообщениях | ? UNCERTAIN (human_needed) | Не выполнялось (07-11 пропущен автором). Заменители на фейках: `running_server_with_the_bot_turns_a_friend_demo_into_a_delivered_video` и `serve_starts_the_bot_against_a_local_bot_api_and_stops_quickly` (настоящий `serve`, `HttpTg` против поддельного Bot API на loopback, быстрая остановка), `bot_failing_getme_stops_serve_with_a_clear_error`, `non_loopback_or_cloud_url_is_refused`, `closed_port_error_has_no_token_in_any_form`, `bot_token_never_leaves_config_errors_or_config_show`, `bot_token_in_tilde_form_is_masked_everywhere`. |

**Счёт:** 4/5 истин подтверждены; истина 5 (живой стенд) — человеку. У истины 4 есть живая часть в `human_verification`.

### Исправления код-ревью (повторная проверка)

Сверено с кодом на HEAD, а не с `07-REVIEW-FIX.md`; поведенческие пункты подтверждены зелёными регрессионными тестами в запуске выше.

| Находка | Что в коде | Тест | Статус |
|---------|-----------|------|--------|
| CR-01 путь `getFile` вида `/var/lib/…` на Windows | `ingest::safe_local_path` зовёт `config::is_abs` (`pub(crate)`, `is_absolute() || has_root()`) | `unix_style_path_from_foreign_bot_api_is_accepted_on_windows`, `relative_path_and_parent_components_are_rejected` | ✓ VERIFIED |
| CR-02 `/retry`: `tg_jobs` раньше вызова Telegram | `commands::retry_cmd`: `insert_tg_job` (`status_message_id: None`) до `bot.say`; при сбое вставки задача снимается `cancel(.., true)`; номер сообщения дописывается `set_status_message` | `retry_records_the_delivery_row_even_when_the_confirmation_is_lost` (порядок проверен чтением кода, тест — по итоговому состоянию) | ✓ VERIFIED |
| WR-01 дубли по всем пользователям | `Core::find_duplicate_for`: своя задача или чужая не `Done` | тесты `bot_ingest`/`bot_status` | ✓ VERIFIED |
| WR-02 тишина после перезапуска | `store::recover` возвращает номера, `Bot::recover` → `deliver::tell_uncertain_after_restart` | `sending_delivery_becomes_uncertain_after_restart_without_a_resend` | ✓ VERIFIED |
| WR-03 `/retry` и дубли | `DedupKey` по хешу или ссылке + `find_duplicate_for` до `resubmit` | `second_retry_of_the_same_job_does_not_queue_a_copy` | ✓ VERIFIED |
| WR-04 ник из подписи | `choose_player` → `Result`, `valid_player` | `bad_caption_nick_is_refused_with_a_reason_not_truncated` | ✓ VERIFIED |
| WR-05 временный сбой `getFile` | миграция 3, `claim_pending` по `next_try_ms`, `defer_or_reject`/`backoff_ms`, предел попыток | `transient_get_file_errors_are_retried_later_permanent_ones_are_not` | ✓ VERIFIED |
| WR-06 файл остаётся в bot-api | `OriginalGuard` (удаление в `Drop`), удаление при несовпадении размера внутри корня | `size_mismatch_is_refused` | ✓ VERIFIED |
| WR-07 строка застревает в `downloading` | `ingest_once` → `release_after_error`; `queue_row`: `mark_queued` раньше `remember_file` | тесты с триггерами в `bot_ingest` | ✓ VERIFIED |
| WR-08 `/resend` блокирует опрос | `deliver_once` шлёт по `result_file_id` в потоке доставки, `400` → отправка с файла | `resend_of_a_sent_video_is_left_to_the_delivery_thread` | ✓ VERIFIED |
| WR-09 отчёт админам теряется | `store::unclaim_admin_report` при ошибке или если ничего точно не ушло | `admin_report_is_retried_when_the_send_definitely_failed` | ✓ VERIFIED |
| WR-10 потеря записи итога | `with_retries` (3 попытки), `outcome_lost` → `uncertain` + сообщение другу | `lost_outcome_write_turns_into_uncertain_and_a_message` | ✓ VERIFIED |

Миграция схемы 3 добавляет только колонки с `DEFAULT`, существующие базы поднимаются сами (шаг в `MIGRATIONS` в `db.rs`, тесты базы зелёные). Info-находки IN-01…IN-06 по заданию не трогались и на цель фазы не влияют.

### Отложенные пункты (Step 9b)

Не переносились: живой стенд и учения фазы 7 в ROADMAP не закреплены за фазой 8 или 9 (учения фазы 9 — другие: убийство раннера, обрыв загрузки, перезапуск сервера и т. п.). Остаются долгом ручной проверки фазы 7.

### Требования (REQUIREMENTS.md)

| Требование | Статус | Подтверждение |
|------------|--------|---------------|
| BOT-01 | ✓ SATISFIED на фейках | `decide` до `getFile`; `stranger_*`, `edited_channel_and_group_updates_are_ignored`; живое — human_needed |
| BOT-02 | ✓ SATISFIED на фейках | `admin_adds_and_removes_a_friend_from_the_chat`, `member_cannot_run_admin_commands`, `adduser_never_grants_admin` |
| BOT-03 | ✓ SATISFIED на фейках | `start_and_help_list_the_commands`, `player_is_saved_and_fixed_in_the_job`, `caption_overrides_the_saved_player_for_one_job`, `player_with_control_or_bidi_characters_is_refused` |
| BOT-04 | ✓ SATISFIED на фейках | размер и имя до `getFile`, магия после копирования, подтверждение сразу; `getFile` на настоящем bot-api не проверен |
| BOT-05 | ✓ SATISFIED (отмечено в 07-07) | `faceit_flow.rs`, `bot_faceit.rs`; ключ только у раннера |
| BOT-06 | ✓ SATISFIED на фейках | `bot_status.rs` (троттлинг 10 с, офлайн-раннер) |
| BOT-07 | ✓ SATISFIED на фейках; воспроизведение — human_needed | `bot_delivery.rs`, `sending_delivery_becomes_uncertain_after_restart_without_a_resend` |
| BOT-08 | ✓ SATISFIED на фейках | `cancel_*`, `retry_*`, WR-01 закрыт в 07-01 |
| BOT-09 | ✓ SATISFIED на фейках | `failure_sends_a_friend_phrase_and_an_admin_report` и два соседних |
| BOT-10 | ✓ SATISFIED на фейках | `same_demo_for_another_player_becomes_a_second_job_without_a_second_download`, `same_bytes_from_another_upload_are_found_by_hash`, `done_duplicate_points_to_resend`, `failed_duplicate_is_not_a_duplicate` |
| BOT-11 | ✓ SATISFIED на фейках | `friend_limit_counts_pending_downloads`, `admin_limit_is_five_and_queue_limit_is_ten`, `oversized_or_sizeless_or_non_demo_name_is_refused_before_get_file` |

Все 11 идентификаторов фазы учтены; осиротевших требований нет. Чекбоксы в REQUIREMENTS.md этой проверкой не менялись (отмечен только BOT-05).

### Ключевые связи

| От | К | Через | Статус |
|----|---|-------|--------|
| `cs2-server serve` (`cli.rs:412`, `lib.rs:86`) | `HttpTg` → `serve_with_bot` → `Bot::new` (`lib.rs:135`) | `bot_api_url` и `TELEGRAM_BOT_TOKEN` | WIRED |
| `dispatch::decide` | `ingest::precheck` / `commands::handle` | белый список до разбора | WIRED |
| `ingest_once` | `Tg::get_file` → проверка пути → `Core::submit` | только после `accept` | WIRED |
| `deliver_once` | `send_video` / `send_document` | `sending` пишется до вызова | WIRED |
| раннер `ensure_faceit_demo` | `faceit::Client` | только `match_id`, ключ из окружения | WIRED |
| бот | `jobs.state` | не пишет сам, только через `Core` | WIRED |

### Анти-паттерны

В файлах `crates/`, изменённых фазой, `TBD`/`FIXME`/`XXX`/`TODO`/`HACK`/`todo!`/`unimplemented!` не найдено. Заглушек в проверенных путях нет.

### Проверки поведения

| Проверка | Команда | Результат | Статус |
|----------|---------|-----------|--------|
| Весь workspace | `cargo test --workspace --release` | код выхода 0, 68 × `test result: ok`, 864 passed, 0 failed, 1 ignored (ручной тест окна в `host`) | ✓ PASS |
| `crates/cli/tests/pipeline.rs` | входит в запуск выше | зелёный | ✓ PASS |
| Живой стенд и спайк | — | не выполнялись (07-11 пропущен) | ? SKIP → human_needed |

### Предупреждения и остаточные риски

- Код-ревью фазы 7 проведено (`07-REVIEW.md`: 2 критических, 10 предупреждений, 6 info), все 12 находок в области исправлены (`07-REVIEW-FIX.md`). Повторное ревью после исправлений не запускалось; Info-находки открыты. Тесты проверяют сценарии исправлений на `FakeTelegram` и настоящем сервере с базой, но не все сочетания состояний (например, порядок CR-02 «вставка раньше вызова Telegram» тест видит только по итоговому состоянию).
- WR-03 фазы 5 (таймауты медленных клиентов) остаётся для фазы 9: порт `cs2-server` слушает только loopback или Tailscale.
- Умолчание `bot_send_as = video` без метаданных видео (A3) не проверено на настоящем Telegram; при провале спайка — смена умолчания или расширение протокола (`/gsd-plan-phase 07 --gaps`).
- Допущение: «число попыток» в отчёте админа — число выдач раннеру (`grants`).

### Резюме

Нереализованных по коду требований нет (в том числе после исправлений ревью): BOT-01…BOT-11 имеют реализацию, проводку и тесты на фейках и настоящем `server::serve`. Статус `human_needed` отражает только то, что спайк и живой стенд (07-11, критерий 5) автор пропустил осознанно; эти пункты остаются долгом ручной проверки и не считаются выполненными.

---

_Verified: 2026-10-12 (повторно)_
_Verifier: Claude (gsd-verifier)_

## Решение автора (2026-10-12)

Автор отложил живой стенд 07-11 («давай следующую фазу») и выбрал вариант 1: закрыть фазу 7 с отложенным долгом. Пять пунктов `human_verification` записаны в `## Deferred Follow-Ups` файла `07-UAT.md` и проходятся в фазе 9 вместе с учениями. Статус passed выставлен по этому решению, а не потому, что живая часть критериев 2–5 доказана.
