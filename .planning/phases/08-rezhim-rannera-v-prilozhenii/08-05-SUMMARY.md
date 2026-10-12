---
phase: 08-rezhim-rannera-v-prilozhenii
plan: 05
subsystem: app-ui
tags: [runner, app, settings, jobs-page, egui, token, store]

requires:
  - phase: 08-rezhim-rannera-v-prilozhenii
    provides: "08-04: RemoteRunner (apply/restart/save_token/clear_token/resume_paused/pending/busy/has_token), RemoteStatus (text/short/level); 08-03: RemoteHub::is_held; 08-01: CardProgress, RemoteExecutor"
provides:
  - "settings_page: раздел «Задачи с сервера», remote_buttons/RemoteButtons, remote_section"
  - "App.token_draft; App::resume_queue снимает и паузу очереди, и паузу раннера; строка состояния раннера в боковой панели"
  - "jobs_page: remote_card_buttons, server_heading, waiting_reason, server_note; метка «с сервера»"
  - "Store::set_card_info; CardProgress пишет карту и игрока из Artifact::Match в карточку"
affects: [08-06]

actuals:
  tokens: 45000
  tasks: 2
  commits: 3
plan_head_before: fd50e3e195db35eda9b1a77297cb0e79b555b2f2
plan_head_after: 0b1eaa2ce7fc7f7f8b8bc14b885809981d3b912e

tech-stack:
  added: []
  patterns:
    - "Действия страницы настроек, которым нужен &mut app.remote, откладываются флагами после закрытия секции (как run_precheck)"
    - "Правила кнопок карточки — чистые функции (remote_card_buttons, server_note, waiting_reason), проверяются без окна"

key-files:
  created: []
  modified:
    - crates/app/src/settings_page.rs
    - crates/app/src/app.rs
    - crates/app/src/jobs_page.rs
    - crates/app/src/jobs.rs
    - crates/app/src/remote/hub.rs
    - crates/app/src/remote/tests.rs
    - crates/store/src/lib.rs

key-decisions:
  - "Заголовок «Задача с сервера #N» строится из числа в external_ref, а не из label: текст от друга в него не попадает"
  - "waiting_reason различает авто-повтор самой карточки («ждёт авто-повтора») и чужой («очередь ждёт авто-повтора другой задачи»): очередь не берёт задач до конца паузы авто-повтора, и молчать об этом нечестно"
  - "Кнопка «Отменить» остаётся и у удержанной карточки: отмена идёт на сервер; убраны только «Повторить», «Заново», «Открыть папку» и «Удалить»"

requirements-completed: [APP-01]

coverage:
  - id: D1
    description: "Раздел «Задачи с сервера» в настройках: адрес сохраняется общей кнопкой и сразу применяется, токен вводится скрытым полем и стирается после попытки сохранения, состояние подключения цветом, «Переподключить»/«Продолжить» по состоянию"
    requirement: "APP-01"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema remote_buttons_follow_the_status"
        status: pass
    human_judgment: true
    rationale: "Вид раздела, скрытый ввод токена, смена строки состояния после «Сохранить токен» и права файла токена (icacls) проверяются глазами и на живой машине — <human-check> задачи 1, выполняется в конце фазы"
  - id: D2
    description: "Строка состояния раннера в боковой панели при заданном адресе; «Продолжить» в очереди снимает и паузу очереди, и паузу раннера"
    requirement: "APP-01"
    verification:
      - kind: other
        ref: "cargo build --release -p app (grep: resume_paused() в App::resume_queue)"
        status: pass
    human_judgment: true
    rationale: "Строка в боковой панели видна только в окне; <human-check> задачи 1"
  - id: D3
    description: "Карточка сервера подписана картой и игроком из предпроверки или номером; рядом метка «с сервера»"
    requirement: "APP-01"
    verification:
      - kind: integration
        ref: "cargo test --release -p app --bin cs2cinema card_learns_map_and_player_from_the_preflight"
        status: pass
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema server_card_without_map_and_player_is_titled_by_its_number"
        status: pass
      - kind: unit
        ref: "cargo test --release -p store set_card_info_keeps_known_values_when_new_ones_are_empty"
        status: pass
    human_judgment: false
  - id: D4
    description: "У карточки сервера нет «Заново»; удержанная раннером карточка без «Повторить», «Открыть папку» и «Удалить» (D-01, T-08-21)"
    requirement: "APP-01"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema remote_card_offers_retry_without_redo_and_nothing_while_held"
        status: pass
    human_judgment: false
  - id: D5
    description: "Ждущая карточка называет причину ожидания, прерванная и удержанная обещает продолжение, готовая — что ролик отправлен и копия удалена (D-04, Pitfall 5)"
    requirement: "APP-01"
    verification:
      - kind: unit
        ref: "cargo test --release -p app --bin cs2cinema server_note_explains_waiting_and_the_deleted_copy"
        status: pass
    human_judgment: false

duration: 25min
completed: 2026-10-12
status: complete
---

# Phase 8 Plan 05: Окно показывает режим раннера Summary

**Раздел «Задачи с сервера» в настройках (адрес, токен скрытым вводом, цветное состояние, «Переподключить»/«Продолжить»), короткая строка раннера в боковой панели и карточка сервера с подписью картой и игроком, меткой «с сервера», причиной ожидания и без опасных кнопок.**

## Performance

- **Duration:** ~25 мин
- **Completed:** 2026-10-12
- **Tasks:** 2 (tracer + auto/tdd)
- **Files:** 7 изменены, новых нет
- **Commits:** 3 (`95fab03`, `2494422`, `0b1eaa2`)

## Accomplishments

- **Настройки.** Раздел «Задачи с сервера» после «Записи»: поле адреса (общая кнопка «Сохранить» применяет его к раннеру только при изменении), поле токена с `password(true)` и кнопки «Сохранить токен»/«Удалить токен» (через `RemoteRunner::save_token/clear_token`), надпись «токен сохранён/не задан», строка `RemoteStatus::text()` цветом по `level()`, «Переподключить» (неактивна с подсказкой, пока идёт задача сервера), «Продолжить» при паузе раннера, пометка «адрес применится, когда закончится задача с сервера», серое пояснение про `cs2-cli runner`. Черновик токена стирается `mem::take` при любом исходе сохранения; ошибка сохранения идёт в строку состояния окна без значения.
- **Приложение.** `App.token_draft`; `resume_queue` зовёт `queue.resume()` и `remote.resume_paused()` (Pitfall 2); в боковой панели при заданном адресе — `status().short()` цветом по уровню.
- **Карточка сервера.** `set_card_info` в `store` (пустое значение прежнее не затирает); `CardProgress::artifact` на `Artifact::Match` пишет очищенные `clean_text(.., NAME_MAX)` карту и игрока в строку карточки; `JobReporter::id()`. В `job_card` для карточки с `external_ref`: заголовок `server_heading` либо карта и игрок без пустых частей, метка `с сервера`, серая строка `server_note`, кнопки `remote_card_buttons` (удержанная — никаких), «Удалить» скрыта у удержанной, «Открыть» у готовой — только если файл ролика ещё есть. Локальные карточки не изменились.

## Task Commits

1. **Task 1 (tracer): раздел настроек и строка в боковой панели** - `95fab03` (feat)
2. **Task 2 RED: тесты карточки сервера и заглушки** - `2494422` (test)
3. **Task 2 GREEN: подпись, кнопки, причины ожидания, карта и игрок** - `0b1eaa2` (feat)

Тракер проверен до расширения: `remote_buttons_follow_the_status` и `cargo build --release -p app` прошли, затем выполнена задача 2.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `JobReporter.id` приватное поле, нужное хабу**
- **Found during:** Task 2
- **Fix:** добавлен `pub(crate) fn id(&self)` в `crates/app/src/jobs.rs` (файл не был в списке `files_modified`, правка из трёх строк)
- **Commit:** `0b1eaa2`

**2. [Уточнение контракта] `waiting_reason` для чужого авто-повтора**
- План называет четыре текста; для случая, когда авто-повтор держит другая задача, добавлен тот же по смыслу, но честный вариант «очередь ждёт авто-повтора другой задачи». Для карточки, которую держит сам авто-повтор, текст «ждёт авто-повтора», как в плане. Тест проверяет четыре различных текста и отдельно чужой повтор.

**Total deviations:** 1 auto-fixed (Rule 3), 1 уточнение. **Impact:** без влияния на поведение, заявленное планом.

## Known Stubs

Нет.

## Human verification deferred to end of phase

Режим `end-of-phase`: проверка не выполнялась, останавливаться на ней не требуется; пункт собирает проверка фазы в UAT. Дословно из `<human-check>` задачи 1:

> Собрать отладочную сборку (`cargo build -p app -p cli`), запустить `target/debug/cs2cinema.exe` с `LOCALAPPDATA`, указывающим на временную папку, и `CS2CINEMA_PAGE=settings` (рецепт снимков окна из памяти проекта). Проверить: раздел «Задачи с сервера» на месте; при пустом адресе строка «Выключено: адрес сервера не задан» и нет кнопки «Переподключить»; после ввода адреса и «Сохранить» строка меняется на «Токен раннера не задан…»; после ввода токена и «Сохранить токен» поле пустое, надпись «токен сохранён»; токен нигде на экране не виден; `icacls "<временная папка>\cs2cinema\runner\token"` показывает единственную запись — текущего пользователя (A1).

Дополнительно стоит взглянуть глазами (не из плана): карточка с выдачей сервера на странице «Задачи» — метка «с сервера», строка причины ожидания, отсутствие «Заново», строка в боковой панели.

## Verification Results

- `cargo test --release -p app --bin cs2cinema` — 80 passed, 0 failed
- `cargo test --release -p store` — 32 passed (в том числе `set_card_info_keeps_known_values_when_new_ones_are_empty`)
- `cargo test --release -p cli --test pipeline` — 2 passed
- `cargo build --release -p app -p cli` — без ошибок
- `cargo test --workspace --release` — 931 passed, 0 failed (флака `win_session` не проявилась)

## Threat Flags

Нет новой поверхности: токен остаётся только в файле с суженным доступом (08-04), новых сетевых путей и схем нет. T-08-19 (поле `password(true)`, черновик стирается, токен в окно не читается), T-08-20 (`clean_text` перед записью, подпись по числу), T-08-21 (нет «Удалить»/«Повторить»/«Заново» у удержанной карточки) закрыты кодом и тестами; T-08-19 дополнительно — `<human-check>`.

## Next Phase Readiness

Окно показывает режим раннера целиком; остаётся план 08-06 и проверка фазы с отложенным `<human-check>`.

## Self-Check: PASSED

- Файлы из `key-files.modified` на месте, коммиты `95fab03`, `2494422`, `0b1eaa2` достижимы из HEAD.
