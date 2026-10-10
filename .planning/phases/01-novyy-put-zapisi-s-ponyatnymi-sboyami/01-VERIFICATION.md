---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
verified: 2026-10-10T00:00:00Z
status: human_needed
score: 5/5 must-haves verified
covered_files:
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-01-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-01-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-02-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-02-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-03-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-03-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-04-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-04-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-05-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-05-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-06-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-06-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-07-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-07-SUMMARY.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-08-PLAN.md
  - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-08-SUMMARY.md
  - crates/app/src/jobs.rs
  - crates/app/src/jobs_page.rs
  - crates/app/src/launch.rs
  - crates/app/src/settings.rs
  - crates/host/src/fake.rs
  - crates/host/src/lib.rs
  - crates/host/src/runner.rs
  - crates/host/src/unsupported.rs
  - crates/host/src/win/mod.rs
  - crates/host/src/win/process.rs
  - crates/pipeline/src/lib.rs
  - crates/pipeline/src/record/deadlines.rs
  - crates/pipeline/src/record/failure.rs
  - crates/pipeline/src/record/log.rs
  - crates/pipeline/src/record/machine.rs
  - crates/pipeline/src/record/mod.rs
  - crates/pipeline/tests/record.rs
  - crates/renderer/src/lib.rs
  - crates/store/src/lib.rs
covered_digest: "v3:sha256:c66bb0dc00947d8d90e0131176d77bb0e62152b150978972b82d6ac7344bfc68"
behavior_unverified: 0
overrides_applied: 0
human_verification:
  - test: "Успешная запись на реальной машине: поставить задачу «ролик» (ссылка FACEIT или .dem), Steam запущен, свой CS2 закрыт, HLAE и CS2 указаны в настройках, ffmpeg в PATH (`cargo run --release -p app`)"
    expected: "Появляется `highlights.mp4`. В журнале задачи по порядку: «запись: запуск лаунчера (HLAE)», «HLAE.exe завершился с кодом 0», «запись: ожидание игры (cs2.exe)», «cs2.exe запущена (PID …)», «запись: игра запущена, жду консоль», «запись: консоль открыта, жду загрузку демки», «запись: демка готова, скрипт запущен», «запись: дубль 1 из M» … «записан дубль M/M», «запись: все дубли записаны, закрываю игру», строка «HLAE.exe: код 0 · cs2.exe: …». На лестнице подсвечен текущий шаг, на шаге записи «дубль a/b». В папке задачи есть `record.log` со строками `[netcon]`, `[console.log]`, `[app]` и строкой о числе строк stdout/stderr HLAE. После задачи `cs2.exe` в диспетчере задач нет."
    why_human: "Настоящий WinHost (Job Object, netcon, console.log, ворота демки) проверен только стендом спайков S1-S4 и 4 тестами win_session на подставном лаунчере; весь путь приложение -> WinHost -> HLAE -> CS2 -> highlights.mp4 в плане 01-07 намеренно не запускался (запрет на запуск cs2.exe/HLAE.exe). Реальные маркеры консоли и формат строк CS2 grep и фейк не докажут."
  - test: "Вызванный сбой: во время второй задачи «ролик», когда на лестнице подсвечен «дубль 1/M», закрыть окно CS2 вручную"
    expected: "Карточка: `GAME_EXITED · дубль`, сообщение «Игра закрылась на этапе «дубль» (дубль 1 из M)» со строкой фактов «HLAE.exe: код 0 · cs2.exe: не жива (код …)», подсказка, «прошло N с», путь `record.log`, раскрывающийся блок «Последние строки консоли» (повторы свёрнуты в «×N»); на лестнице «лаунчер → игра → консоль → демка → скрипт» отмечены ✓, «дубль» — ✕; `record.log` содержит блок «итог»; «Повторить» работает."
    why_human: "Реальная смерть процесса cs2.exe, срабатывание handle в WinSession и вид карточки в egui на фейке и по коду не воспроизводятся."
  - test: "Отмена: во время записи нажать «Отменить»"
    expected: "Статус «отменена», блока ошибки нет, `cs2.exe` закрыт, `record.log` содержит итог CANCELLED."
    why_human: "Закрытие настоящего дерева процессов через Job Object при отмене в приложении не запускалось (на фейке проверено: `host.terminated() > 0`)."
  - test: "«Запас по времени» в настройках: поставить ×2.00, сохранить, перезапустить приложение; затем открыть со старым `settings.json` с `load_wait_secs`"
    expected: "Значение сохранилось; в журнале следующей задачи с записью есть «запас по времени ×2.00»; старый файл настроек загружается без ошибки."
    why_human: "Отрисовка DragValue и сохранение через UI не проверяются тестами (тест `old_settings_file_with_load_wait_still_loads` покрывает только разбор JSON)."
  - test: "Склейка ffmpeg через Host::run на реальных дублях"
    expected: "В журнале «склейка: N%» и «склейка заняла N с», `highlights.mp4` собран. Дубли не недописаны: приложение больше не проверяет наличие `video.mp4`/`audio.wav` после записи (это TAK-02..05, фаза 3), а в спайке S1 признак `ffmpeg_in_job` остался `unknown` — если склейка найдёт недописанный клип, это первое место для разбора."
    why_human: "Нужны реальные файлы HLAE; закрытие дубля по `TAKE_END` + `quit` без опроса размера файла на реальной игре не наблюдалось."
  - test: "Подпись неудачи в карточке для сбоев запуска: убрать из пути HLAE `AfxHookSource2.dll` (или запустить с запущенным лаунчером без Steam) и поставить задачу"
    expected: "Задача падает в срок с кодом `LAUNCHER_FAILED` или `GAME_NOT_STARTED` и подсказкой; HLAE.exe и cs2.exe названы раздельно; время на карточке близко к дедлайну (30 с / 20 с)."
    why_human: "Реальные дедлайны запуска зависят от поведения HLAE.exe (окно с ошибкой не завершает процесс); проверено только на виртуальных часах."
---

# Фаза 1: Новый путь записи с понятными сбоями — отчёт верификации

**Цель фазы:** Задача с записью в приложении идёт через новый `pipeline::record` (машина состояний поверх трейта `Host`), а любой сбой старта игры останавливает её в срок с типизированной ошибкой, которую видно в карточке задачи и в `record.log`.
**Проверено:** 2026-10-10
**Статус:** human_needed
**Повторная верификация:** Нет — первичная

## Достижение цели

Код проверен напрямую, а не по SUMMARY: прочитаны `crates/host/src/{lib,unsupported,runner}.rs`, `crates/host/src/win/{mod,process}.rs`, `crates/pipeline/src/record/{mod,machine,failure,deadlines,log}.rs`, `crates/app/src/{jobs,jobs_page,launch,settings}.rs`, `crates/store/src/lib.rs`, `crates/renderer/src/lib.rs`, тесты `crates/pipeline/tests/record.rs`. Тесты перепрогнаны мной: `cargo test --release -p pipeline -p host -p app -p store -p renderer` — зелёные (app 8, host 41 + win_session 4, pipeline 17 + record 24 + transcripts 6, store 6, renderer 13). Оркестратор сообщил, что `cargo build --release --workspace` и `cargo test --release --workspace` зелёные.

### Наблюдаемые истины (критерии успеха ROADMAP)

| # | Истина | Статус | Доказательство |
|---|--------|--------|----------------|
| 1 | Запись из приложения идёт через `pipeline::record`; журнал показывает именованные состояния; M дублей выведены из плана до записи | ✓ VERIFIED | `app/src/jobs.rs:run()` -> `record_request(...)` -> `record_stage(&*host, &req, p)` -> `pipeline::record`. `host = pipeline::host::default_host()` (`jobs.rs`). Старых `launch()`/`kill_game`/`wait_for_clips`/`taskkill`/`tasklist` в `crates/app/src` нет (grep пуст). Машина: `RecordState` с лестницей Launcher -> Game -> Console -> Demo -> Script -> Take -> Closing -> Done (`machine.rs`); каждое состояние выдаёт `Event::State`, `JobReporter::event` пишет его в `jobs.phase` и в журнал задачи (`jobs.rs`). Дубли: `renderer::takes(&timeline,&rcfg)` вызывается в `pipeline::plan_player` (`pipeline/src/lib.rs:251`), `Plan.takes` попадает в `RecordRequest.takes`; первая строка журнала «ожидаю дублей: N — take0000..take000K, в каждом video.mp4 и audio.wav» пишется до `host.launch` (`record/mod.rs:record`). Тест `fake_recording_walks_the_whole_ladder_and_returns_every_take` сверяет точную последовательность состояний и эту строку. Мост до базы: `typed_failure_reaches_the_job_row`. |
| 2 | Нет игры / демки / скрипта / маркеров READY, SCRIPT, TAKE_START, TAKE_END -> сбой в срок; карточка показывает код, состояние, сообщение, подсказку, этап, время и хвост консоли | ✓ VERIFIED | `Machine::limit()` даёт для каждого ожидания свой дедлайн и свой код: `LAUNCHER_FAILED`, `GAME_NOT_STARTED`, `CONSOLE_NOT_OPEN`, `DEMO_NOT_LOADED`, `SCRIPT_NOT_RUN`, `TAKE_TIMEOUT` (`machine.rs`); пропавшая метка — всегда `Finish(Err)`, `ALL_DONE` не заменяет `TAKE_END` (тест `all_done_does_not_stand_in_for_a_lost_last_take_end`). Тесты на фейке: `missing_ready_marker_fails_with_demo_not_loaded_after_the_deadline` (дедлайн 150 с проходит за <2 с реальных), `launcher_that_never_exits_...`, `game_that_never_appears_...`, `console_that_never_opens_...`, `missing_script_marker_...` (`exec` ровно один раз), `missing_first_take_start_...`, `missing_take_end_... naming_the_take`. Поля сбоя `RecordFailure{code,state,message,hint,elapsed,console_tail}` (`failure.rs`) -> `job_failure()` -> `store.fail_job` (миграция user_version=2, колонки `fail_code/fail_state/fail_hint/fail_tail/fail_elapsed_ms/phase`) -> `jobs_page::error_block`: «код · состояние», сообщение, подсказка, «прошло N с», путь `record.log`, сворачиваемый блок «Последние строки консоли»; `ladder` отмечает ✕ на упавшем шаге. Тексты по-русски, подсказка есть у каждого кода кроме `CANCELLED`. |
| 3 | После каждой записи (успех или сбой) в папке задачи есть `record.log` с консолью игры и stdout/stderr HLAE | ✓ VERIFIED | `Journal::open` вызывается первым действием `record()`, до `host.launch`; `RecordLog::open` создаёт папку, открывает на дозапись, каждая строка сразу сбрасывается (`flush`). Закрыты все выходы `record()`: успех (`j.success`), сбой (`j.failure`), ошибка запуска (`launch_failed` + `j.failure`), отмена (`Cancelled` проходит через тот же `j.failure`). Источники `[netcon]`, `[console.log]`, `[hlae-stdout]`, `[hlae-stderr]`, `[app]`, `[host]` (`host::Source::label`); счётчик строк HLAE пишется даже при нуле. Тесты: `record_log_is_written_on_success_and_on_failure_with_source_tags` (две попытки в одну папку, прошлая не стирается), `cancelled_recording_still_leaves_a_complete_record_log`. На Windows `WinHost` читает pipe лаунчера как `Source::HlaeStdout/HlaeStderr` (`win/mod.rs:211-215`). Секреты и профиль пользователя скрываются (`log::redact`). |
| 4 | Журнал и текст ошибки раздельно называют код выхода HLAE.exe и живость cs2.exe; живость — по handle; ошибка проверки — «неизвестно», не «жива» | ✓ VERIFIED | `facts_line(launcher, game)` -> «HLAE.exe: код 0 · cs2.exe: не жива (код 0)» / «…cs2.exe: неизвестно (<ошибка ОС>)» (`failure.rs`); добавляется к сообщению любого сбоя кроме отмены и пишется в журнал (`Journal::failure`, `success`). Живость: `OwnedProc::wait` = `WaitForSingleObject` на handle: `WAIT_TIMEOUT` -> `Alive`, `WAIT_OBJECT_0` -> `Exited(код)`, `WAIT_FAILED`/прочее -> `Unknown(текст)`; `OpenProcess` не открылся -> `Unknown`, никогда `Alive` (`win/process.rs`; тест `liveness_of_a_missing_process_is_unknown_not_alive`). `WinSession::game_liveness` читает этот handle, а не `tasklist`. Тесты на фейке: `unknown_liveness_is_reported_as_unknown_never_alive`, `journal_reports_launcher_code_and_game_liveness_separately`, `game_exit_mid_take_reports_the_dead_game_with_its_code`, `game_that_never_appears_names_the_hlae_code_and_the_missing_game`. |
| 5 | `cargo test --workspace` без CS2 гоняет машину записи на фейковом Host (успех и потеря маркера), а workspace собирается на не-Windows с заглушкой | ✓ VERIFIED (сборка на не-Windows — по чтению cfg, не сборкой) | Фейк `host::fake::FakeHost` — обычный модуль (не `cfg(test)`), виртуальное время; 24 сценария в `pipeline/tests/record.rs`, 17 тестов машины, 6 по транскриптам реальных сбоев (`transcripts.rs`) — все проходят без CS2. Не-Windows: `windows-sys` только в `[target.'cfg(windows)'.dependencies]` крейта `host`; `pub mod win` под `#[cfg(windows)]`; `default_host()` на не-Windows возвращает `UnsupportedHost` (`launch` -> `HostError::Unsupported`, `run` -> `runner::run_bounded` на чистом `std`, `CREATE_NO_WINDOW` под `cfg(windows)`); `host/examples/spikes.rs` имеет `#[cfg(not(windows))] fn main` и всё остальное под `cfg(windows)`; `host/tests/win_session.rs` — `#![cfg(windows)]`; в `pipeline` нет `cfg(windows)` вообще; в `app` все вызовы `launch::no_window`, `tray_icon` и `std::os::windows` стоят под `cfg(windows)` (`app.rs:565-566`, `ui.rs:273-277`, `launch.rs:136-137`, `main.rs:31`). Тест `unsupported_host_refuses_to_launch` проходит и на Windows. **Ограничение:** установлен только `x86_64-pc-windows-msvc`, цель Linux в `rustup` отсутствует, поэтому `cargo check --target x86_64-unknown-linux-gnu` выполнить нельзя; Ubuntu CI не запускался. |

**Счёт:** 5/5 истин подтверждены кодом и тестами на фейке; все пять требуют также реального прогона (см. «Требуется проверка человеком»).

### Проверка обязательных артефактов

| Артефакт | Ожидается | Статус | Детали |
|----------|-----------|--------|--------|
| `crates/host` (lib, fake, runner, unsupported, win/*) | Листовой крейт, трейты `Host`/`Session`, фейк, заглушка, WinHost | ✓ VERIFIED | Входит в `Cargo.toml` workspace; зависимостей на крейты workspace нет; `windows-sys` только под `cfg(windows)`; `unsafe` только в `win/` |
| `crates/pipeline/src/record/{mod,machine,failure,deadlines,markers,log}.rs` | Машина, драйвер, коды, дедлайны, журнал | ✓ VERIFIED | Существенные (50-750 строк), используются `app` и тестами; `pub use` в `pipeline/src/lib.rs` |
| `crates/store/src/lib.rs` | Миграция по `user_version`, `fail_job`, `set_phase`, `set_out_dir` | ✓ VERIFIED | Миграция v1 -> v2 покрыта тестами `version_one_database_gains_elapsed_and_phase` |
| `crates/app/src/jobs.rs`, `jobs_page.rs`, `settings.rs` | Мост, карточка, «Запас по времени» | ✓ VERIFIED | Старый код записи удалён; `time_margin` в настройках, `load_wait_secs` убран, старый файл настроек читается (тест) |
| `crates/renderer/src/lib.rs` | `marker(nonce,…)`, `TakePlan`, `takes()` | ✓ VERIFIED | Один формат маркера для генератора cfg и парсера; тест `markers_carry_the_nonce_and_cover_every_take` |

### Проверка ключевых связей

| Откуда | Куда | Через | Статус | Детали |
|--------|------|-------|--------|--------|
| `jobs::run` | `pipeline::record` | `record_request` + `record_stage` | WIRED | единственный путь записи в приложении |
| `pipeline::record` | `host::Host` | `host.launch`, `Session::poll/send/terminate` | WIRED | `Guard` завершает сессию при любом выходе |
| `Machine` | `Event`/`RecordFailure` | `Action::Emit`/`Finish` | WIRED | драйвер выполняет все действия, включая повторную подачу `SendFailed` |
| `JobReporter::event` | `jobs.phase` | `store.set_phase` | WIRED | `phase` не трогает `stage` |
| `RecordFailure` | строка `jobs` | `job_failure` -> `store.fail_job` | WIRED | тест `typed_failure_reaches_the_job_row` |
| `Job` в базе | карточка | `ladder`, `error_block` | WIRED | показывает все поля; для нетипизированных ошибок — прежняя строка |
| `Settings.time_margin` | `Deadlines::scaled` | `jobs::deadlines(s)` | WIRED | питает и запись, и `ffmpeg_for` |
| склейка | `Host::run` | `run_ffmpeg` с `CmdSpec`/дедлайном | WIRED | `Command::new("ffmpeg")` в `jobs.rs` нет |
| `WinSession::game_liveness` | handle процесса | `OwnedProc::liveness` | WIRED | без `tasklist` |

### Трассировка потока данных (уровень 4)

| Артефакт | Данные | Источник | Реальные данные | Статус |
|----------|--------|----------|-----------------|--------|
| карточка, лестница | `j.phase`, `j.fail_state` | `store.set_phase`/`fail_job` из `record()` | да (путь покрыт тестом моста на `Store`) | FLOWING |
| карточка, ошибка | `fail_code/hint/tail/elapsed` | `RecordFailure` из машины | да | FLOWING |
| ожидаемые дубли | `RecordRequest.takes` | `renderer::takes(timeline)` | да | FLOWING |
| хвост консоли | `Journal.tail` | события сессии (`Source::*`) | да на фейке; на Windows зависит от реальной игры | FLOWING (реальный источник — в человеческой проверке) |

### Поведенческие проверки

| Поведение | Команда | Результат | Статус |
|-----------|---------|-----------|--------|
| Машина записи на фейке (успех и потеря маркеров) | `cargo test --release -p pipeline` | 17 + 24 + 6 passed | ✓ PASS |
| Хост: фейк, заглушка, runner, WinSession-стенд | `cargo test --release -p host` | 41 + 4 passed | ✓ PASS |
| Мост приложения: тип сбоя -> строка `Store`, отмена, склейка, настройки | `cargo test --release -p app` | 8 passed | ✓ PASS |
| Миграция базы | `cargo test --release -p store` | 6 passed | ✓ PASS |
| Весь workspace, сборка release | выполнено оркестратором | зелёные | ✓ PASS (по сообщению оркестратора) |

### Выполнение проб

Шаг 7c: пробы `scripts/*/tests/probe-*.sh` не объявлены ни в одном PLAN/SUMMARY фазы и в репозитории отсутствуют — пропущено.

### Покрытие требований

Сверка ID из frontmatter PLAN с `REQUIREMENTS.md`: объединение `requirements` планов 01-01..01-08 = {TST-01, TMO-02, TAK-01, LCH-03, LCH-02, LCH-01, TMO-01, LCH-04, LCH-06}, то есть ровно 9 ID фазы. В таблице трассировки `REQUIREMENTS.md` на «Phase 1» отображены те же 9 ID; осиротевших требований нет.

| Требование | План(ы) | Описание | Статус | Доказательство |
|------------|---------|----------|--------|----------------|
| LCH-01 | 01-04, 01-07 | Явная машина состояний | ✓ SATISFIED | `RecordState`/`Machine`, тест полной лестницы |
| LCH-02 | 01-03, 01-04, 01-07, 01-08 | Типизированный сбой, виден в карточке | ✓ SATISFIED | `RecordFailure` -> `fail_job` -> `error_block`; реальный вид в egui — человеческая проверка |
| LCH-03 | 01-02, 01-04 | Потерянный маркер — сбой, а не предупреждение | ✓ SATISFIED | 7 тестов потерь маркеров; `ALL_DONE` не засчитывается |
| LCH-04 | 01-05, 01-06, 01-07, 01-08 | Консоль и stdout/stderr HLAE в `record.log` при любом исходе | ✓ SATISFIED (на фейке) | тесты `record.log`; реальные каналы Windows — человеческая проверка |
| LCH-06 | 01-06, 01-07, 01-08 | Раздельные факты HLAE.exe и cs2.exe | ✓ SATISFIED | `facts_line`, тесты журнала |
| TMO-01 | 01-04, 01-06, 01-07 | Именованные дедлайны ожиданий | ✓ SATISFIED для пути записи и склейки | таблица `Deadlines`, `ffmpeg_for` через `Host::run`; поле `ffprobe` пока без потребителя (ffprobe не на пути записи в приложении; потребитель — TAK-04, фаза 3) |
| TMO-02 | 01-01, 01-05, 01-06, 01-08 | Живость по handle; ошибка = «неизвестно» | ✓ SATISFIED | `OwnedProc::wait`, `facts_line`, тесты |
| TAK-01 | 01-02, 01-04, 01-07 | Набор дублей выводится из плана до записи | ✓ SATISFIED | `renderer::takes`, `expected_line` |
| TST-01 | 01-01, 01-04, 01-06 | Хост за трейтом, фейк, заглушка не-Windows | ✓ SATISFIED (сборка не-Windows — по чтению) | см. истину 5 |

### Найденные антипаттерны

| Файл | Строка | Шаблон | Серьёзность | Влияние |
|------|--------|--------|-------------|---------|
| `TBD`/`FIXME`/`XXX` в изменённых `crates/host`, `crates/pipeline`, `crates/app`, `crates/store`, `crates/renderer/src/lib.rs` | — | не найдено | — | Блокирующих маркеров нет |
| `crates/app/src/launch.rs` | 138 | `Command::output()` без дедлайна (`nvenc_check`) | Info | Вызывается до записи для задач с GPU-кодированием. Это территория TMO-03 (фаза 2: «ни один вызов `Command::output()` без дедлайна на пути записи»); к целям фазы 1 не относится |
| `crates/app/src/jobs.rs` | `record_stage`/`RunError` | `clippy::result_large_err` | Info | Зафиксировано исполнителем в SUMMARY 01-07; контракт плана требует `Option<RecordFailure>` |
| `crates/pipeline/src/record/deadlines.rs` | 42 | поле `ffprobe` без потребителя | Info | Заготовка под TAK-04 (фаза 3) |

### Замечания (не блокируют фазу)

- `record.log` создаётся первым действием `record()`. Сбои до входа в запись (`analyze`, `find_player`, `check_paths`, проверка пробелов в пути, `install_files`) `record.log` не создают, и на карточке такая задача показывает прежнюю строку ошибки без пути к `record.log`. Критерий 3 говорит о «задачах записи» и относится к `record()`; расхождения с целью нет, но пользователь должен знать, что для сбоев подготовки лог записи отсутствует.
- Приложение больше не проверяет наличие `video.mp4`/`audio.wav` после записи (старый `wait_for_clips` удалён, замена — TAK-02..05 в фазе 3). Фаза 1 заявляла это явно (SUMMARY 01-07, «Behavior Notes»); вывод о риске недописанных клипов — в человеческой проверке 5.
- Сборка для не-Windows подтверждена только чтением `cfg`: цель Linux не установлена, Ubuntu CI не запускался.

### Требуется проверка человеком

Исполнитель плана 01-07 перечислил эти проверки как «Human verification pending», и они не выполнялись. Они же дают статус `human_needed`.

#### 1. Успешная запись от ссылки до `highlights.mp4`

**Тест:** Steam запущен, свой CS2 закрыт, HLAE и CS2 заданы в настройках, ffmpeg в PATH. `cargo run --release -p app`, поставить задачу «ролик» по ссылке FACEIT или `.dem`.
**Ожидается:** появился `highlights.mp4`; в журнале по порядку «запись: запуск лаунчера (HLAE)», «HLAE.exe завершился с кодом 0», «запись: ожидание игры (cs2.exe)», «cs2.exe запущена (PID …)», «запись: игра запущена, жду консоль», «запись: консоль открыта, жду загрузку демки», «запись: демка готова, скрипт запущен», «запись: дубль 1 из M» … «записан дубль M/M», «запись: все дубли записаны, закрываю игру», «HLAE.exe: код 0 · cs2.exe: …»; на лестнице подсвечен текущий шаг и «дубль a/b»; в папке задачи `record.log` со строками `[netcon]`, `[console.log]`, `[app]` и строкой о числе строк stdout/stderr HLAE; после задачи `cs2.exe` в диспетчере задач нет.
**Почему человек:** настоящий WinHost целиком в приложении не запускался.

#### 2. Вызванный сбой закрытием CS2 посреди дубля

**Тест:** на второй задаче, когда подсвечено «дубль 1/M», закрыть окно CS2.
**Ожидается:** карточка `GAME_EXITED · дубль`, сообщение «Игра закрылась на этапе «дубль» (дубль 1 из M)» со строкой «HLAE.exe: код 0 · cs2.exe: не жива (код …)», подсказка, «прошло N с», путь `record.log`, блок «Последние строки консоли» с моноширинным хвостом (повторы «×N»); на лестнице ✓ до «скрипт», ✕ у «дубль»; в `record.log` блок «итог»; «Повторить» работает.
**Почему человек:** реальная смерть процесса и вид egui.

#### 3. Отмена во время записи

**Тест:** нажать «Отменить» во время записи.
**Ожидается:** «отменена», без блока ошибки, `cs2.exe` закрыт, в `record.log` итог CANCELLED.
**Почему человек:** завершение настоящего Job Object при отмене в приложении.

#### 4. «Запас по времени»

**Тест:** поставить ×2.00, сохранить, перезапустить; открыть приложение со старым `settings.json` с `load_wait_secs`.
**Ожидается:** значение сохранилось, в журнале «запас по времени ×2.00», старый файл загружается.
**Почему человек:** UI-ввод и сохранение; тесты покрывают только разбор JSON и масштаб дедлайнов.

#### 5. Склейка и целостность дублей

**Тест:** на успешной задаче посмотреть журнал склейки; по желанию ×0.5 и длинный ролик.
**Ожидается:** «склейка: N%» и «склейка заняла N с»; клипы не недописаны (приложение дубли после записи не проверяет, `ffmpeg_in_job` в спайке S1 — `unknown`); при слишком малом запасе «ffmpeg не уложился в N с…».
**Почему человек:** нужны настоящие файлы HLAE.

#### 6. Сбой запуска в срок

**Тест:** сломать запуск (например, переименовать `x64\AfxHookSource2.dll` или запускать без Steam) и поставить задачу.
**Ожидается:** сбой `LAUNCHER_FAILED` или `GAME_NOT_STARTED`/`HLAE_ERROR` с подсказкой в пределах дедлайнов (30 с / 20 с, умножаются на «Запас по времени»), HLAE.exe и cs2.exe названы раздельно.
**Почему человек:** реальное поведение HLAE (окно ошибки не завершает процесс) проверено только на виртуальных часах и транскриптах.

### Итог

Расхождений между целью фазы и кодом не найдено: путь записи в приложении действительно идёт через `pipeline::record` на `host::default_host()`, каждое ожидание ограничено своим дедлайном и кодом сбоя, типизированный сбой проходит через `Store` до карточки, `record.log` пишется на всех выходах `record()`, факты о HLAE.exe и cs2.exe раздельны, а «неизвестно» не превращается в «жива». Автоматические проверки зелёные. Пробел один: всё, что доказывается только реальным прогоном CS2/HLAE и видом окна, остаётся человеческой проверкой (шесть пунктов выше), поэтому итоговый статус `human_needed`, а не `passed`. Блокирующих замечаний (BLOCKER) нет.

---

_Проверено: 2026-10-10_
_Проверяющий: Claude (gsd-verifier)_
