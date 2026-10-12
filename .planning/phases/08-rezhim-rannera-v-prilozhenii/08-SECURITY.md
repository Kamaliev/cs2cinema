---
phase: "08"
slug: rezhim-rannera-v-prilozhenii
status: verified
threats_open: 0
asvs_level: 1
created: "2026-10-12"
---

# Фаза 8 — безопасность

Аудит по коду и тестам (ASVS L1, `block_on: high`), код на коммите `b7a63d6` (HEAD после плана 08-06; незакоммиченная правка форматирования в `crates/store/src/journal.rs` не учитывалась): 31/31 записей реестра закрыты (25 mitigate подтверждены строками кода и запущенными тестами, 6 accept — довод подтверждён диффом манифестов). Открытых угроз нет ни на пороге блокировки, ни ниже него. Реестр: 25 нумерованных угроз T-08-01…25 и 6 строк T-08-SC (по одной на план 08-01…08-06). Живой стенд (настоящие CS2, HLAE, Telegram) отложен автором в фазу 9; ручная проверка окна настроек и прав файла токена (`<human-check>` плана 08-05) остаётся пунктом UAT конца фазы, для L1 код и тесты T-08-14 и T-08-19 присутствуют.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| сервер → раннер в приложении (`Assignment`, `hello`-директивы, отмена в пульсе) | номер задачи, эпоха, ник игрока, ссылка FACEIT; сервер решает судьбу выдачи (`Keep`, `Cancel`, `Abandon`) | ник друга из Telegram, причины от сервера |
| раннер → сервер (`hello`, `claim`, `release`) | исходящие запросы с Bearer-токеном на адрес из настроек | токен раннера |
| демка друга → запись в CS2 на машине автора | враждебные байты демки исполняются конвейером приложения | карта и имя игрока из демки |
| поток раннера ↔ поток очереди | итог и события каналом; запись только в потоке очереди | состояние выдачи |
| раннер → наблюдатель (окно приложения) | тексты ошибок связи и причины попадают в окно и журнал карточки | строки событий без токена |
| база приложения после падения | строки `running`/`queued` удалённых карточек без живого исполнителя | статусы карточек |
| автор → карточка и окно (кнопки, поле токена) | действия автора над чужой задачей; секрет вводится в окно и уходит в файл | токен, команды отмены |
| другие учётные записи Windows ↔ профиль автора | на `%LOCALAPPDATA%\cs2cinema` унаследован доступ другой локальной учётки | файл `token` |
| `cs2-cli runner` ↔ приложение на одной машине | два возможных хозяина одной папки раннера | замок `runner.lock` |
| следы работы раннера ↔ всё, что может прочесть или переслать автор | журналы, база, состояние, окно, `Debug` | токен не должен оказаться ни в одном следе |

---

## Threat Register

| Threat ID | Category | Component | Severity | Disposition | Mitigation | Status |
|-----------|----------|-----------|----------|-------------|------------|--------|
| T-08-01 | Elevation of privilege / Denial of service | удалённая карточка исполнена обычным `run_job` в обход дочерней предпроверки | high | mitigate | ветка с записью в хабе только через `factory()` → `PipelineExecutor` (`hub.rs:344-352`, `control.rs:186-194`); `inspect_demo` при `!resume`, `catch_unwind`, `verify` (`runner/src/exec.rs:354-378`); `local` только при `resume` (`hub.rs:334-340`); тесты `server_job_becomes_a_card_and_its_video_reaches_the_server` (`inspect_runs == 1`), `closed_server_card_cannot_start_over` | closed |
| T-08-02 | Denial of service | две записи CS2 одновременно (раннер и очередь) | high | mitigate | единственный `Runner::new` с `RemoteExecutor` (`control.rs:556`); `RemoteExecutor::run` только ждёт канала (`hub.rs:240-281`); тест `remote::e2e::one_recording_at_a_time_and_local_jobs_wait_their_turn` (`max_concurrent == 1`) | closed |
| T-08-03 | Tampering | ник игрока от друга в базе, журнале и окне | medium | mitigate | `clean_text(&task.hero_name, NAME_MAX)` перед `adopt_remote` (`hub.rs:168`); подпись из номера (`remote/mod.rs:31-33`); ник в аргументах предпроверки очищен (`inspect.rs:113`); тест `card_reference_and_label_are_built_from_numbers_only` | closed |
| T-08-04 | Tampering / Repudiation | повторный вызов исполнителя для той же выдачи создаёт вторую карточку | medium | mitigate | `external_ref = srv:{id}:{epoch}`; уникальный частичный индекс `jobs_external_ref` (`store/src/lib.rs:293-294`); `adopt_remote` в одной транзакции (`:599-626`); тест `adopt_remote_creates_one_card_per_grant_and_requeues_it_in_any_status` | closed |
| T-08-05 | Repudiation | промежуточный сбой запуска уходит серверу до авто-повтора | medium | mitigate | `on_settled` после `policy.after` и только при `verdict != AutoRetry` (`queue.rs:434-444`); тесты `hook_hears_the_final_verdict_once_after_an_auto_retry`, `launch_failure_is_retried_inside_the_same_grant` (`grants == 1`) | closed |
| T-08-06 | Information disclosure | значение токена в тексте `RunnerEvent` в окне | high | mitigate | `emit` → `scrub` (`***`, `one_line`) для всех строковых полей (`runner/src/lib.rs:437-487`); `Config` Debug печатает только признаки (`:155-173`); тесты `run_reports_gate_and_net_errors_without_the_token`, `config_debug_shows_flags_only` | closed |
| T-08-07 | Denial of service (доступность) | закрытые ворота: сервер считает раннер офлайн | medium | mitigate | `gate_ping` шлёт `hello` «занят: …» сразу и каждые 30 с (`PAUSE_HELLO_EVERY`, `lib.rs:50,499-516`) при окне сервера 70 с (`server/src/core.rs:596-597`); тест `closed_gate_takes_no_job_and_keeps_the_runner_online` | closed |
| T-08-08 | Denial of service | гонка ворот сжигает выдачи до `TOO_MANY_GRANTS` | medium | mitigate | повторная проверка ворот после `claim`, `hand_back` → `ReleaseRequest { counts: false }` (`lib.rs:411-418,521-543`); тест `gate_closing_after_claim_returns_the_job_without_counting_the_grant` (`grants == 0`) | closed |
| T-08-09 | Tampering | карта и имя игрока из демки в журнале и карточке | low | mitigate | `clean_text(.., NAME_MAX)` в `preflight` (`inspect.rs:334-339`) и при записи в карточку (`hub.rs:313-316`); тест `preflight_reports_the_map` | closed |
| T-08-10 | Denial of service | отмена с сервера ставит очередь автора на паузу | medium | mitigate | `Advice { suppress_pause }` только при `server_cancel && Cancelled` (`hub.rs:146`), снимается только `Pause(Cancelled)` (`queue.rs:440-442`); тесты `server_cancel_stops_the_running_card_without_pausing_the_queue`, `author_cancel_of_a_running_card_pauses_the_queue_and_cancels_on_the_server`, `hook_lifts_only_the_cancel_pause` | closed |
| T-08-11 | Tampering / Denial of service | после падения очередь берёт удалённую карточку раньше раннера | high | mitigate | `reset_running` переводит удалённые `running`/`queued` в `interrupted` (`store/src/lib.rs:669-672`) из `recover_after_crash` до старта очереди (`app.rs:219-226`); в очередь возвращает только `adopt_remote` под замком `slots` (`hub.rs:172-188`); тест `interrupted_server_job_resumes_after_restart_without_a_pause` (`calls.count() == 1`) | closed |
| T-08-12 | Repudiation | «Повторить» закрытой выдачи загружает старый ролик поверх чужого итога | medium | mitigate | без записи в хабе карточка идёт в `local` без отправки (`hub.rs:334-338`), путей отправки вне `RemoteExecutor` нет; тест `retry_of_a_closed_server_card_runs_locally_and_uploads_nothing` | closed |
| T-08-13 | Repudiation | отмена или удаление ждущей карточки не доходит до сервера | medium | mitigate | `ROW_STEP` = 1 с (`hub.rs:33`), строка карточки читается в цикле (`:274-279`), `on_row_closed` шлёт `CANCELLED` (`:219-236`); тест `author_cancel_of_a_waiting_card_reaches_the_server` | closed |
| T-08-14 | Information disclosure | файл `token` читается другой локальной учётной записью | medium | mitigate | `write_private`: пустой файл → `icacls /inheritance:r /grant:r USERNAME:F` через `Host::run` с дедлайном 15 с (не-Windows `0o600`) → значение; при отказе файл удаляется (`token.rs:71-118`); тесты `saved_token_loads_back_and_the_acl_is_narrowed_first`, `failed_acl_leaves_no_token_file`, `invalid_token_is_not_written`; живой `icacls` — UAT конца фазы | closed |
| T-08-15 | Information disclosure | токен в `settings.json`, `Debug`, журналах, тексте состояния | high | mitigate | в `settings.rs` нет `token` (grep), только `runner_server`; `runner::Token` Debug = `Token(***)`, Display нет (`link.rs:31-55`); тексты `BadToken` без значения (`token.rs:28-53`); тесты `token_file_value_is_trimmed_and_checked`, `issued_token_is_found_only_in_its_file` | closed |
| T-08-16 | Information disclosure / Spoofing | токен уходит на чужой адрес | medium | mitigate | `Link::new` пускает `http://` только на localhost, loopback, частные и link-local адреса (`link.rs:131-166`), `max_redirects(0)` (`:170`); отказ → `BadAddress` до запросов (`control.rs:556-559`); тест `refusals_before_start_name_their_reason` | closed |
| T-08-17 | Denial of service | `cs2-cli runner` и приложение держат одну папку и пишут CS2 одновременно | high | mitigate | `runner.lock` + `File::try_lock` (`control.rs:445-454`), `WouldBlock` → `LockBusy` (`:537-541`); тот же файл у `cs2-cli runner` (`runner_cmd.rs:84-90,216-231`); тест `second_runner_in_the_same_folder_is_refused`. Остаток вне формулировки: `cs2-cli runner --dir <другая>` обходит замок, об этом предупреждают `USAGE_RUNNER` и настройки | closed |
| T-08-18 | Denial of service / Repudiation | смена адреса или токена посреди задачи отменяет задачу друга | medium | mitigate | при `is_running() && busy()` адрес откладывается в `pending`, `tick` применяет после закрытия карточки (`control.rs:294-315`); тот же путь у `save_token`/`clear_token`/`restart` (`:348-365`); тест `address_change_waits_for_the_server_card` | closed |
| T-08-19 | Information disclosure | токен на экране или в черновике окна | medium | mitigate | `TextEdit::singleline(..).password(true)` (`settings_page.rs:209`); черновик стирается `mem::take` при любом исходе (`:247-254`); окну доступен только `has_token` (`token.rs:56-58`); тест `remote_buttons_follow_the_status`; вид окна — UAT конца фазы | closed |
| T-08-20 | Tampering / Spoofing | ник и карта из задачи друга искажают окно | low | mitigate | `clean_text` в `hub.rs:168,313-316` и `inspect.rs:334-338` вырезает управляющие и bidi-символы (`cs2/src/text.rs:11-37`); `server_heading` из числа (`jobs_page.rs:164-167`); тесты `server_card_without_map_and_player_is_titled_by_its_number`, `card_learns_map_and_player_from_the_preflight`, `clean_text_strips_control_bidi_and_invisible_chars` | closed |
| T-08-21 | Denial of service / Repudiation | автор удаляет или запускает «Заново» удержанную карточку | medium | mitigate | `remote_card_buttons`: при `held` нет `retry`/`redo`/`open_dir`, у карточки сервера `redo: false` всегда (`jobs_page.rs:155-160`), «Удалить» скрыта при `held` (`:398`); тест `remote_card_offers_retry_without_redo_and_nothing_while_held` | closed |
| T-08-22 | Information disclosure | токен в `runner.log`, `runner_state.json`, базе, состояниях, `settings.json`, `Debug` после сбоев | high | mitigate | тест `remote::e2e::issued_token_is_found_only_in_its_file` (отказ задачи, успех, недоступный адрес, отзыв токена; сырые байты всех файлов папки и базы, `jobs()`, тексты и Debug состояний, `Settings`, `runner::Config`, `host.runs()`; поиск не холостой) | closed |
| T-08-23 | Denial of service | две записи CS2 одновременно в смешанной очереди | high | mitigate | тесты `one_recording_at_a_time_and_local_jobs_wait_their_turn` (`max_concurrent == 1`), `paused_queue_keeps_server_jobs_out` | closed |
| T-08-24 | Repudiation | задача друга теряется или пишется дважды после убийства приложения | medium | mitigate | тесты `killed_during_recording_the_app_finishes_the_server_job_by_itself`, `killed_during_upload_the_app_uploads_without_recording_again`, `closed_app_with_a_waiting_server_card_resumes_it` (`calls.count() == 1`) | closed |
| T-08-25 | Denial of service | регрессия v1.0 без адреса сервера | medium | mitigate | тесты `without_an_address_the_app_has_no_runner_and_behaves_like_v1`, `empty_address_starts_no_runner_and_the_queue_works_as_before`; `cargo test -p app queue` (31), `cargo test --release -p cli --test pipeline` (2), полный прогон воркспейса (938) | closed |
| T-08-SC (08-01) | Tampering | зависимости | low | accept | внешних пакетов нет: `runner` в `[dependencies]`, `server` и `wire` в `[dev-dependencies]` приложения — путевые крейты воркспейса; манифест сервера не менялся (`--test deps`) | closed |
| T-08-SC (08-02) | Tampering | зависимости | low | accept | новых пакетов нет (`git diff 01e2ffd..3b58d7e -- Cargo.toml crates/*/Cargo.toml`) | closed |
| T-08-SC (08-03) | Tampering | зависимости | low | accept | новых пакетов нет (`git diff 3b58d7e..HEAD -- Cargo.toml crates/*/Cargo.toml` пуст) | closed |
| T-08-SC (08-04) | Tampering | зависимости | low | accept | новых пакетов нет; `icacls` — системная утилита Windows через `Host::run` с `ACL_DEADLINE` 15 с (`token.rs:61,90-96`) | closed |
| T-08-SC (08-05) | Tampering | зависимости | low | accept | новых пакетов нет | closed |
| T-08-SC (08-06) | Tampering | зависимости | low | accept | новых пакетов нет; `Cargo.lock` за фазу: +3 строки (`runner`, `server`, `wire` в зависимостях `app`), новых `name`/`source` нет | closed |

*Status: open · closed · open — below high threshold (non-blocking)*
*Severity: critical > high > medium > low — only open threats at or above workflow.security_block_on count toward threats_open*
*Disposition: mitigate (implementation required) · accept (documented risk) · transfer (third-party)*

---

## Accepted Risks Log

| Risk ID | Threat Ref | Rationale | Accepted By | Date |
|---------|------------|-----------|-------------|------|
| R-08-01 | T-08-SC (08-01…08-06) | новых внешних зависимостей фаза не добавила; `runner`, `server`, `wire` — путевые крейты воркспейса, `icacls` — системная утилита с дедлайном | автор (решение при планировании) | 2026-10-12 |

---

## Наблюдения вне реестра

Не новые угрозы, на `threats_open` не влияют; переданы в проверку фазы.

1. **`Dropped.reason` от сервера** попадает в журнал карточки строкой «сервер снял задачу: {reason}» (`control.rs:514`) после `scrub` (токен, `one_line`), но без `clean_text`: bidi-символы не вырезаются. Источник — собственный сервер автора; в T-08-20 эта строка не входит.
2. **Непроверенная гипотеза о гонке.** Если удалённую карточку, которую раннер не держит (`held_ref = None`, раннер в `NoToken`/`LockBusy`), запустить «Повторить» локально, а раннер в это время поднимется и усыновит ту же выдачу, `adopt_remote` перепишет статус идущей строки на `queued` (`store/src/lib.rs:608-611`). Две записи одновременно невозможны (поток очереди один), отправлен будет ролик той же выдачи. Тестом не воспроизводилось.
3. **Отложенные проверки.** `<human-check>` плана 08-05 (вид раздела настроек, живой `icacls` на файле токена) — пункт UAT конца фазы.

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-10-12 | 31 | 31 | 0 | gsd-security-auditor (opus), код на `b7a63d6`; тесты `app` (87), `runner` (84), `store` (32), `pipeline inspect` (11), `cs2 text` (3), `cli pipeline` (2), `server deps` (2) запущены аудитором |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer)
- [x] Accepted risks documented in Accepted Risks Log
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-10-12
