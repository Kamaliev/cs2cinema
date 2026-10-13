---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 04
subsystem: chain-ci
tags: [chain, e2e, bot, server, runner, fake-host, ci, github-actions, secrets-scan, rust]

requires:
  - phase: 09-uborka-razvertyvanie-i-ucheniya
    provides: "09-01: ci.yml с concurrency и шагом Dependency guard, улики 00-ci; 09-02: доставка бота и Core, через которые идёт цепочка"
  - phase: 05-yadro-servera-i-protokol
    provides: "server::serve, Core, Stand::start в runner/tests/common, FakeTelegram, Bot::step"
provides:
  - "crates/runner/tests/chain.rs: два сквозных теста цепочки фейковый Telegram -> бот -> настоящий сервер -> раннер на FakeHost -> чат"
  - "ci.yml: отдельный job chain (ubuntu) с timeout-minutes: 30, шагами Dependency guard и Chain test"
  - "Улики evidence/00-ci/: раздел «перед push 09-04» в prepush-scan.txt, run-chain.txt (зелёный прогон 38425807557), строка в RESULT.md"
affects: [09-10 учения (00-ci), публикация версии 1.1]

actuals:
  tokens: 14000
  tasks: 3
  commits: 5
plan_head_before: 249bbfc077b57cbbe8f3fe00e782eeb2df4b1ca2
plan_head_after: 207718d4b36b5a0b0bc23b5916762bc353c0415d

tech-stack:
  added: []
  patterns:
    - "Сквозной тест как один интеграционный файл в runner/tests: сервер там уже dev-зависимость, а server/tests/deps.rs запрещает обратную связь server -> runner"
    - "Бот ведётся детерминированными шагами bot.step(), а не потоками serve_with_bot: без ожиданий по времени в CI"

key-files:
  created:
    - crates/runner/tests/chain.rs
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/run-chain.txt
  modified:
    - .github/workflows/ci.yml
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/prepush-scan.txt
    - .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-ci/RESULT.md

key-decisions:
  - "Тест лежит в crates/runner/tests/chain.rs, а не в server: зависимость server -> runner запрещена охраной deps.rs даже для dev-зависимостей"
  - "Подменены только Telegram (FakeTelegram) и внешние процессы (FakeHost); сервер и бот настоящие"
  - "Класс секретов exact-values для коммитов 09-02…09-04 снова остался без проверки точных значений (файла образцов нет); автор подтвердил публикацию ответом «запушено»"

patterns-established:
  - "Красный job chain считается дефектом, а не поводом для перезапуска; перезапуск допустим только для известного флейка win_session на windows-latest"

requirements-completed: [OPS-03]

coverage:
  - id: D1
    description: "Демка друга проходит весь путь: сообщение в чат, бот, настоящий сервер, раннер на FakeHost, ролик возвращается в чат ровно один раз; задача done, доставка sent, папка дублей и демка раннера удалены"
    requirement: OPS-03
    verification:
      - kind: integration
        ref: "crates/runner/tests/chain.rs#demo_from_a_friend_goes_through_the_server_and_the_runner_and_comes_back_as_a_video"
        status: pass
    human_judgment: false
  - id: D2
    description: "Сломанная демка (BAD_DEMO) заканчивается фразой другу по-русски без кода и путей и отчётом администратору с кодом; CS2 не запускался"
    requirement: OPS-03
    verification:
      - kind: integration
        ref: "crates/runner/tests/chain.rs#broken_demo_ends_with_a_clear_message_to_the_friend_and_a_report_to_the_admin"
        status: pass
    human_judgment: false
  - id: D3
    description: "CI гоняет цепочку и охрану зависимостей отдельным job chain (ubuntu); первый прогон после публикации зелёный по трём job"
    requirement: OPS-03
    verification:
      - kind: other
        ref: "GitHub Actions run 38425807557 (head_sha 3f68875), evidence/00-ci/run-chain.txt"
        status: pass
      - kind: integration
        ref: "cargo test --release -p server --test deps"
        status: pass
    human_judgment: false
  - id: D4
    description: "Поиск секретов по коммитам origin/main..HEAD перед публикацией 09-02…09-04, семь классов; пункты «НУЖНО ПРОВЕРИТЬ АВТОРУ»"
    requirement: OPS-03
    verification: []
    human_judgment: true
    rationale: "Пометки «НУЖНО ПРОВЕРИТЬ АВТОРУ» и класс exact-values снимает только автор; автор опубликовал коммиты ответом «запушено»"

duration: не измерена (работа в два захода с паузой на решение автора)
completed: 2026-10-13
status: complete
---

# Phase 9 Plan 04: Сквозная цепочка в CI Summary

**Демка друга проходит в одном тесте весь путь от сообщения в чат через настоящие бот и сервер и раннер на FakeHost до ролика в чате, сломанная демка заканчивается понятной фразой, а job chain (ubuntu) в CI зелёный с первого прогона.**

## Performance

- **Duration:** не измерена (два захода: до паузы на решение автора и после неё)
- **Completed:** 2026-10-13
- **Tasks:** 3 (Task 3 — точка решения автора с публикацией)
- **Files modified:** 5 (без учёта SUMMARY, STATE и ROADMAP)

## Accomplishments

- `crates/runner/tests/chain.rs` (300 строк, два теста): `FakeTelegram` присылает документ друга, `Bot::step()` создаёт задачу, `Runner::tick()` с `PipelineExecutor` на `FakeHost` доводит её до `Done`, дальнейшие шаги бота доставляют ролик. Итог проверяется по всем сторонам: ролик в чате друга ровно один, доставка `sent`, `highlights.mp4` на сервере, у раннера нет `out/srv{id}` и `remote/srv{id}.dem`, в текстах друга нет `blobs`, `out/` и локальных путей.
- Сценарий сбоя: предпроверка отвергает демку (`BAD_DEMO`), задача `failed`, друг получает фразу «демка повреждена или это не демка CS2» без кода, администратор получает отчёт с кодом, `host.launches() == 0`.
- `ci.yml`: job `chain` (`name: chain (ubuntu)`, `timeout-minutes: 30`) с шагами `Dependency guard` и `Chain test` (`cargo test --release -p runner --test chain -- --nocapture`). Подставные токены в вывод не попадают.
- Перед публикацией коммиты `origin/main..HEAD` (17 штук) проверены по семи классам секретов, раздел «перед push 09-04» в `prepush-scan.txt` написан без значений.
- Автор опубликовал `main` (`origin/main` равен `3f68875`); прогон 38425807557 завершился `success` по всем трём job: `test (ubuntu-latest)`, `test (windows-latest)`, `chain (ubuntu)` (2 мин 29 с); артефакт `cs2-server-linux-x64` (3191762 байт) выгружен. Перезапусков не потребовалось.

## Task Commits

1. **Task 1 (tracer): демка друга возвращается к нему роликом через сервер и раннер** — `37b1ea1` (test)
2. **Task 2: сломанная демка и отдельный job chain в CI**
   - `1e45631` (test) — второй тест: `BAD_DEMO`, фраза другу, отчёт администратору
   - `da28768` (ci) — job `chain (ubuntu)`
3. **Task 3: поиск секретов, решение автора, улика прогона**
   - `3f68875` (docs) — раздел «перед push 09-04» в `prepush-scan.txt`
   - `207718d` (docs) — `run-chain.txt` и строка в `RESULT.md`

**Plan metadata:** коммит `docs(09-04): complete …` (SUMMARY, STATE, ROADMAP).

## Files Created/Modified

- `crates/runner/tests/chain.rs` — два сквозных теста и локальные помощники (копии из `faceit_flow.rs` и `exec.rs`, файл самодостаточен)
- `.github/workflows/ci.yml` — job `chain (ubuntu)`
- `evidence/00-ci/prepush-scan.txt` — раздел «перед push 09-04»
- `evidence/00-ci/run-chain.txt` — прогон 38425807557: три job success, артефакт сервера
- `evidence/00-ci/RESULT.md` — строка про `run-chain.txt`

## Decisions Made

- Тест живёт в `runner`, потому что `server/tests/deps.rs` запрещает серверу зависеть от `runner`; `cargo test --release -p server --test deps` остаётся зелёным.
- Бот ведётся шагами `bot.step()` (как в `faceit_flow.rs`): тест детерминирован, порт `127.0.0.1:0` и свои временные папки делают параллельный запуск двух тестов безопасным.
- Монтаж тест не проверяет (`FakeHost` не умеет разбор демки, запись и ffmpeg): его охраняет `crates/cli/tests/pipeline.rs`.
- Класс `exact-values` для новых коммитов остался непроверенным (файла образцов `drills\patterns.txt` нет); автор подтвердил публикацию так же, как в 09-01.

## Deviations from Plan

None - plan executed exactly as written. Красных прогонов и автоисправлений не было.

## Issues Encountered

- На точке Task 3 пришлось ждать окончания прогона после push автора (около 6 минут); прогон завершён с `status: completed`, отменённых и оборванных прогонов не было, поэтому улика берётся с первой попытки.
- Исполнитель не создал файл-журнал начальной точки плана (`gsd-plan-head-before-09-04`) в первом заходе, поэтому `plan_head_before` взят как родитель первого коммита плана (`37b1ea1^` = `249bbfc`); измеренное число коммитов — 5 (`git rev-list --count 249bbfc..207718d`).
- Флейк `win_session` на `windows-latest` в этом прогоне не проявился.
- `git push` делал только автор; исполнитель его не выполнял.

## Threat Model Dispositions

- **T-09-13 (подставные токены в выводе тестов):** смягчено. Тесты используют подставные строки стенда и не печатают их; шаг CI идёт с `--nocapture`, и прогон не оставил токенов в тексте чата и журнале бота (проверки в самом тесте на тексты друга).
- **T-09-14 (зависимость server -> runner):** смягчено. Сквозной тест вынесен в `runner`, охрана `deps.rs` зелёная локально и в CI (шаг `Dependency guard` на всех трёх job).
- **T-09-37 (публикация коммитов 09-02…09-04):** смягчено. Перед паузой выполнен поиск по семи классам секретов (контроль выражений, без значений); найдены только подставные токены в тестах `cleanup.rs` и `deploy_docs.rs` и null-значение в `docs/deploy/cs2-server.json`; пометки «НУЖНО ПРОВЕРИТЬ АВТОРУ» снимает автор. Остаточный риск: класс `exact-values` не проверен, решение автора.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Сквозная цепочка охраняется в CI: регрессия на стыке бот, сервер, раннер ловится отдельным job `chain (ubuntu)`.
- Для публикации версии 1.1 остаются за автором пункты «НУЖНО ПРОВЕРИТЬ АВТОРУ» из `prepush-scan.txt`.

## Self-Check: PASSED

- Файлы на месте: `crates/runner/tests/chain.rs`, `.github/workflows/ci.yml`, `prepush-scan.txt`, `run-chain.txt`, `RESULT.md`.
- Коммиты 37b1ea1, 1e45631, da28768, 3f68875, 207718d присутствуют в истории `main`; измеренное число коммитов плана — 5.
- Прогон 38425807557: `completed`, `success`, `head_sha` = 3f68875, три job success, артефакт `cs2-server-linux-x64` есть.
- Локально на Windows зелёные `cargo test --release -p runner --test chain` (2 теста) и `cargo test --release -p server --test deps` (2 теста).

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-13*
