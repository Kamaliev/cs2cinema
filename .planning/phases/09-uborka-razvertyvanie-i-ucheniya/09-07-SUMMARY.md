---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 07
subsystem: drills-protocols
tags: [drills, protocols, ops, docs, evidence]

requires:
  - phase: 09-uborka-razvertyvanie-i-ucheniya
    provides: "План 09-06: evidence.sh import/check, collect-server.sh, collect-runner.ps1, scan-token.sh, образец протокола из семи разделов в docs/drills/README.md; план 09-05: docs/deploy/README.md и check-host.sh"
provides:
  - "docs/drills/00-stand.md: развёртывание по документу, спайки bot-api (sendVideo против sendDocument, права и пути общего тома, botapi-janitor, токен в журнале), живая цепочка; три папки улик 00-deploy, 00-spikes, 00-live-chain"
  - "docs/drills/01-kill-runner.md: четыре прогона (приложение и cs2-cli, паузы 10 и 60 с), наблюдение за числом процессов cs2, WR-01/WR-02 как возможная причина провала варианта приложения"
  - "docs/drills/02-upload-cut.md: --upload-chunk-limit 2 и обрыв сети, сверка SHA-256 раннера и сервера"
  - "docs/drills/03-server-restart.md: systemctl restart во время записи и kill -9 во время загрузки, строка «восстановление: …», выдачи не растут"
  - "docs/drills/04-cancel-offline.md: /cancel в очереди и после убийства раннера, Abandon при возврате, что остаётся на диске раннера"
  - "docs/drills/05-big-demo-proxy.md: демка 500 МБ (настоящая или PBDEMS2 с добивкой), rss.txt, SHA-256 на обеих сторонах, журнал прокси без адресов"
  - "docs/drills/06-version-mismatch.md: раннер PROTOCOL_VERSION=2, запуск через cmd /c в UTF-8, ответ 426"
  - "docs/drills/07-bot-live.md: девять шагов сценария 07-UAT теста 4 (незнакомец, FACEIT, /status, /queue, сбой, /retry, /cancel, /resend)"
  - "docs/drills/08-day-soak.md: сутки работы в вариантах compressed и real, критерий возврата места на диске"
affects: [09-08, 09-09, 09-10 проведение учений]

actuals:
  tokens: 18000
  tasks: 3
  commits: 3
plan_head_before: deb81afb44498b2ec001f84de8c0cc91007337b9
plan_head_after: f87951e3c14d4edc7d5207d95382d7606a3d0cdf

tech-stack:
  added: []
  patterns:
    - "Протокол учения из семи разделов образца README; раздел «Проверка» — команды evidence.sh import и evidence.sh check --require-pass для своей папки улик"
    - "Файлы, снятые руками на хосте, идут в ~/drill/<учение>/, сборщик collect-server.sh — в ~/raw/<учение>/ (он отказывается писать в непустую); обе папки копируются в одну сырую папку на Windows"
    - "Состояние раннера «до» и «после» собирается одним и тем же collect-runner.ps1 через временную подпапку и перенос плоско в сырую папку (суффикс -before для «до»)"

key-files:
  created:
    - docs/drills/00-stand.md
    - docs/drills/01-kill-runner.md
    - docs/drills/02-upload-cut.md
    - docs/drills/03-server-restart.md
    - docs/drills/04-cancel-offline.md
    - docs/drills/05-big-demo-proxy.md
    - docs/drills/06-version-mismatch.md
    - docs/drills/07-bot-live.md
    - docs/drills/08-day-soak.md
  modified: []

key-decisions:
  - "Учение 08 описано в двух вариантах; compressed засчитывается за «после дня работы» только при записанном решении автора в 09-STAND.md, а в плане задач успешная идёт последней, чтобы «26 ч после последнего доставленного ролика» было позже всех остальных событий"
  - "01-kill-runner разложено на четыре прогона (А/10, А/60, Б/10, Б/60) в одной папке улик: имена файлов разведены суффиксами -before-<метка>, а число процессов cs2 пишет отдельное наблюдение cs2-count.txt, потому что processes.txt сборщика — снимок, а не история"
  - "Для 04, 06 и 08 плоские имена runner.log, listing.txt, console-proto2.txt оставлены в корне сырой папки: эти файлы читают проверки плана 09-10 и 09-09"
  - "В протоколе 00 папка рабочей части bot-api под токеном выводится только через sed с заменой на <bot>; число строк с токеном в журнале bot-api берётся grep -c (значения не печатаются)"

patterns-established:
  - "Роли «автор», «друг», «незнакомец» вместо имён и Telegram-ID в командах, заметках, chat.txt; ответ бота незнакомцу содержит его собственный ID, в chat.txt он заменяется на <ID>"

requirements-completed: []

coverage:
  - id: D1
    description: "Девять протоколов docs/drills/NN-имя.md с семью разделами образца README и командами evidence.sh import и evidence.sh check --require-pass для своей папки улик"
    requirement: OPS-04
    verification:
      - kind: other
        ref: "grep по заголовкам и по evidence.sh check.*--require-pass для всех девяти файлов: all-protocols-ok"
        status: pass
    human_judgment: false
  - id: D2
    description: "Каждый скрипт, названный в протоколе полным путём (docs/drills/…, docs/deploy/…), существует"
    requirement: OPS-04
    verification:
      - kind: other
        ref: "grep -oE docs/(drills|deploy)/…\\.(sh|ps1) и test -f для каждого: refs-ok, all-protocols-ok"
        status: pass
    human_judgment: false
  - id: D3
    description: "Инструменты 09-06 не сломаны: bash docs/drills/selftest.sh печатает selftest: OK"
    requirement: OPS-04
    verification:
      - kind: integration
        ref: "docs/drills/selftest.sh"
        status: pass
    human_judgment: false
  - id: D4
    description: "Протоколы исполнимы на живом стенде: команды хоста (ctl backup, docker compose exec, journalctl), PowerShell-приёмы (Collect-Runner, cmd /c), curl -K, наблюдение rss"
    requirement: OPS-04
    verification:
      - kind: other
        ref: "первый прогон — планы 09-08, 09-09, 09-10; на этой машине нет хоста, Docker, CS2"
        status: unrun
    human_judgment: true
    rationale: "Команды не запускались: нет Linux-хоста с systemd и Docker, нет CS2 и HLAE. Исполнимость проверит первое живое учение; протоколы не принимаются за «пройденное» до тех пор, пока evidence.sh check --require-pass для их папки не пройдёт"

duration: 13min
completed: 2026-10-13
status: complete
---

# Phase 9 Plan 07: Протоколы учений Summary

**Девять протоколов живых учений в `docs/drills/` по образцу README: стенд со спайками bot-api, убийство раннера, обрыв загрузки, перезапуск сервера, отмена при офлайн-раннере, демка 500 МБ через прокси, несовпадение версий, живой бот и сутки работы в двух вариантах; в каждом раздел «Проверка» с `evidence.sh import` и `evidence.sh check --require-pass`**

## Performance

- **Duration:** около 13 мин
- **Completed:** 2026-10-13T16:27Z
- **Tasks:** 3 (трассер и две обычные)
- **Files modified:** 9 (все созданы)

## Accomplishments

- `00-stand.md`: три папки улик (`00-deploy`, `00-spikes`, `00-live-chain`). Развёртывание строго по `docs/deploy/README.md`, `docs/deploy/check-host.sh` до и после `systemctl reboot`, скан портов снаружи, сверка SHA-256 бинарника, копия базы и `job list` на восстановленной копии папки данных. Спайки: запросы к Bot API только через `curl -K` с файлом 0600, пути под токеном выводятся с заменой на `<bot>`, проверка `botapi-janitor` принудительно состаренным файлом, число строк с токеном в журнале bot-api, синтетические ролики 0,5 / 1,0 / 1,5 ГБ. Живая цепочка с закрытием приложения посреди записи.
- `08-day-soak.md`: оба варианта (`compressed` со сжатыми сроками и обратной настройкой, `real` с наблюдением не раньше 26 ч), три задачи (упавшая, отменённая, успешная последней), критерий «после − до не больше 1 МиБ» (в `real` плюс демка упавшей задачи), ожидание «в `listing.txt` нет ни дублей, ни метки `srv<N>.accepted` задач `done`».
- `01`–`03`: убийство раннера в четырёх прогонах, обрыв загрузки двумя способами, перезапуск сервера штатный и аварийный; факты проверяются по `job show` (эпоха и число выдач до и после), журналу сервера («аренда задачи #N истекла», «восстановление: …») и числу процессов `cs2`.
- `04`–`07`: отмена в очереди и после убийства раннера, большая демка с `rss.txt` и сверкой SHA-256, раннер `PROTOCOL_VERSION = 2` через `cmd /c`, сценарий бота из 07-UAT теста 4.
- Все ссылки на скрипты — полным путём от корня репозитория и существуют; в документах нет значений токенов, имён и Telegram-ID, секреты в командную строку нигде не передаются.

## Task Commits

1. **Task 1 (трассер): стенд и сутки работы** — `ec00ff2` (docs)
2. **Task 2: убийство раннера, обрыв загрузки, перезапуск сервера** — `d3d336c` (docs)
3. **Task 3: отмена при офлайн-раннере, большая демка, версии, живой бот** — `f87951e` (docs)

**Plan metadata:** коммит `docs(09-07)` с этим SUMMARY, STATE.md и ROADMAP.md.

## Verification

- Первая команда `<verify>` (семь заголовков, `evidence.sh check.*--require-pass`, существование всех названных скриптов) для всех девяти протоколов — `all-protocols-ok`; для Task 1 и Task 2 по отдельности — `protocols-ok`, `refs-ok`.
- Критерии приёмки Task 1: `00-stand.md` содержит `sendVideo`, `sendDocument`, `botapi-janitor`, `systemctl reboot`, `curl -K`, `docs/deploy/check-host.sh`; `08-day-soak.md` содержит `compressed`, `real`, `clean_interval_secs`, «26 ч», `.accepted`, «Сроки хранения» и `09-STAND.md`.
- Критерии приёмки Task 2: `Stop-Process`, `TOO_MANY_GRANTS`, `processes.txt`, `WR-01`; `--upload-chunk-limit`, `sha-runner.txt`, `Out-File -Encoding utf8`; `systemctl restart cs2-server`, `kill -9`.
- Критерии приёмки Task 3: `/cancel`, «сервер велел бросить задачу»; `PBDEMS2`, `rss.txt`; `PROTOCOL_VERSION`, `cmd /c`; `/retry`, `/resend`, «незнакомец».
- `bash docs/drills/selftest.sh` — `selftest: OK`.
- Фразы, на которые опираются ожидания, сверены с кодом: «аренда задачи #… истекла — задача снова в очереди / задача отменена» (`crates/server/src/lib.rs`), «восстановление: упавших приёмов …, продлённых аренд …», «сервер велел бросить задачу» (`crates/runner/src/lib.rs`), «версия протокола сервера не совпадает с раннером», `POST … → 426` (журнал запросов `crates/server/src/api.rs`), «доставка в Telegram: …» и «эпоха: …, выдач: …» (`crates/server/src/cli.rs`), фразы бота (`crates/server/src/bot/`).

## Decisions Made

См. `key-decisions` во frontmatter.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical] Наблюдение за числом процессов cs2 в протоколе 01**
- **Found during:** Task 2
- **Issue:** план требует доказать «два процесса cs2 одновременно не было» по `processes.txt`, но `collect-runner.ps1` пишет только снимок процессов на момент сбора; одним снимком двойную запись не поймать
- **Fix:** протокол 01 добавляет второе окно PowerShell с циклом, пишущим число процессов `cs2` каждые 5 секунд в `cs2-count.txt` (`Out-File -Encoding utf8 -Append`); `processes.txt` остаётся снимком «после»
- **Files modified:** `docs/drills/01-kill-runner.md`
- **Committed in:** `d3d336c`

**2. [Rule 3 - Blocking] Раздельные папки для рук и сборщика на хосте**
- **Found during:** Task 1
- **Issue:** `collect-server.sh` отказывается писать в непустую папку, а часть улик (`check-host-*.txt`, `backup-restore.txt`, `rss.txt`, `part-before.txt`) снимается руками до сборщика
- **Fix:** вручную снятые файлы идут в `~/drill/<учение>/`, сборщик пишет в `~/raw/<учение>/`; в «Общих приёмах» `00-stand.md` описано копирование обеих папок в одну сырую папку
- **Files modified:** `docs/drills/00-stand.md` и ссылки в остальных протоколах
- **Committed in:** `ec00ff2`

**3. [Rule 2 - Missing critical] Маскировка секретов и ID в уликах, которых план не назвал**
- **Found during:** Task 1 и Task 3
- **Issue:** рабочая папка bot-api названа токеном бота, ответ бота незнакомцу содержит его Telegram-ID, журнал Caddy и `serve status` содержат адреса
- **Fix:** вывод `find` по рабочей папке bot-api идёт через `sed` с заменой на `<bot>`; в `chat.txt` протокола 07 ID заменяется на `<ID>`, ник и ссылка FACEIT — на заглушки; журнал Caddy и `serve status` проходят `sed` с заменой IP и имени узла
- **Files modified:** `docs/drills/00-stand.md`, `docs/drills/05-big-demo-proxy.md`, `docs/drills/07-bot-live.md`
- **Committed in:** `ec00ff2`, `f87951e`

**4. [Rule 1 - Bug] Срок 26 ч привязан к самому позднему событию**
- **Found during:** Task 1
- **Issue:** «26 ч после последнего доставленного ролика» не покрывает отмену, если она случилась позже ролика, и демку отменённой задачи (срок 24 ч) ещё можно было бы застать
- **Fix:** в протоколе 08 задачи идут в порядке «упавшая, отменённая, успешная последней»
- **Files modified:** `docs/drills/08-day-soak.md`
- **Committed in:** `ec00ff2`

---

**Total deviations:** 4 auto-fixed (1 bug, 2 missing critical, 1 blocking)
**Impact on plan:** набор протоколов соответствует плану и числу файлов; правки только добавляют защиту от утечки и проверяемость фактов.

## Issues Encountered

- Ни одна команда протоколов не запускалась: на этой машине нет Linux-хоста с systemd и Docker, нет CS2 и HLAE. Первые вопросы к стенду (флаги для 09-08): формат вывода `docker compose logs --since` в `collect-server.sh`, права `sudo -u cs2-server` на `/var/tmp` для `backup`, поведение `touch -d` в контейнере `botapi-janitor` (uid 101, `network_mode: none`), совпадение `sed`-маскировки с реальным видом папки под токеном.
- Протокол 02 предполагает, что раннер сам не пишет SHA-256 в журнал; сверка идёт по `Get-FileHash` до продолжения и `sha256sum` сборщика. Если в 09-09 станет видно, что `runner.log` печатает сумму, сверку можно упростить.
- Протокол 06 просит `cmd /c` без кавычек вокруг путей (пути стенда без пробелов); при пробелах в `%LOCALAPPDATA%` команду придётся переписать.

## Known Stubs

None. Заглушек и TODO в созданных файлах нет.

## Threat Flags

Новых сетевых поверхностей нет: документы. Реестр плана: T-09-35 (запросы к Bot API только через `curl -K`, роли вместо имён, `chat.txt` без ID; поиск токена — `evidence.sh import`) и T-09-36 (вывод PowerShell только через `| Out-File -Encoding utf8` или `cmd /c … > файл`; `evidence.sh check` отвергает UTF-16) отражены в протоколах 00, 02, 05, 06, 07.

## User Setup Required

None для этого плана. Файл образцов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` и остальные действия у машин — в планах 09-08, 09-09, 09-10 по этим протоколам.

## Next Phase Readiness

- План 09-08 проводит `00-stand.md` (стенд, спайки, живая цепочка); 09-09 — `01`, `02`, `03`, `06`; 09-10 — `04`, `05`, `07`, `08` и итоговый поиск токена. Имена сырых и итоговых папок улик в протоколах совпадают с ожиданиями этих планов (`runner.log`, `listing.txt`, `server-journal.txt`, `console-proto2.txt` в корне папки).
- OPS-04 этим планом не закрыт: его закрывают только живые учения, чьи папки проходят `evidence.sh check --require-pass`.

## Self-Check: PASSED

- Файлы на диске: `docs/drills/00-stand.md`, `01-kill-runner.md`, `02-upload-cut.md`, `03-server-restart.md`, `04-cancel-offline.md`, `05-big-demo-proxy.md`, `06-version-mismatch.md`, `07-bot-live.md`, `08-day-soak.md`.
- Коммиты в ветке: `ec00ff2`, `d3d336c`, `f87951e`; `commits: 3` измерено `git rev-list --count` от `plan_head_before`.
- `bash docs/drills/selftest.sh` — `selftest: OK`.
- Rust-файлы, `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `crates/store/src/journal.rs` не затронуты.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-13*
