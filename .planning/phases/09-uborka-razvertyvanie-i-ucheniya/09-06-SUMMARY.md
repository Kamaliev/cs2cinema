---
phase: 09-uborka-razvertyvanie-i-ucheniya
plan: 06
subsystem: drills-tooling
tags: [drills, evidence, bash, powershell, secrets-scan, ops]

requires:
  - phase: 09-uborka-razvertyvanie-i-ucheniya
    provides: "План 09-05: пути хоста (/etc/cs2-server, /var/lib/cs2-server, /opt/cs2-bot-api), служебная команда оператора job list/show, backup {папка} --keep N"
provides:
  - "docs/drills/scan-token.sh: поиск секретов из файла образцов (формы ':' и '~', секретная часть) в текстах и двоичных файлах, печать только имён и чисел"
  - "docs/drills/evidence.sh import/check [--require-pass]: отбор текстовых улик из сырой папки, token-scan.txt, проверка RESULT.md, непустоты, баз, чата и кодировки"
  - "docs/drills/selftest.sh: самопроверка на подставном токене, пункты (а)-(е) плюс добавленные"
  - "docs/drills/collect-server.sh и collect-runner.ps1: сбор улик на Linux-хосте и на машине раннера в сырую папку"
  - "docs/drills/README.md: роли, две папки, файл образцов, формат RESULT.md, правило кодировки, образец протокола из семи разделов, таблица учений, правила приёмки и повтора"
affects: [09-07 протоколы учений, 09-08, 09-09, 09-10 проведение учений и итоговый поиск токена]

actuals:
  tokens: 10029
  tasks: 2
  commits: 2
plan_head_before: 36c23fce25db02c143122c83a046bdd9f34f6c9c
plan_head_after: 5e594662b717a40818d42f6581028a903f24c1a4

tech-stack:
  added: []
  patterns:
    - "Две папки на учение: сырая вне репозитория (базы, экспорт чата) и улики в папке фазы (только текст); поиск секретов идёт по обеим"
    - "Секреты только из файла образцов вне репозитория, значения никогда не печатаются; самопроверка собирает весь вывод и ищет в нём подставной токен"
    - "Учение засчитывается только когда evidence.sh check --require-pass проходит; result: fail с полными уликами — записанный провал"

key-files:
  created:
    - docs/drills/README.md
    - docs/drills/scan-token.sh
    - docs/drills/evidence.sh
    - docs/drills/selftest.sh
    - docs/drills/collect-server.sh
    - docs/drills/collect-runner.ps1
  modified:
    - .gitattributes

key-decisions:
  - "check повторно ищет секреты по папке улик, если файл образцов под рукой: token-scan.txt от import мог устареть, если после него в папку добавили файл"
  - "collect-server.sh снимает копию базы во временную папку cs2-server в /var/tmp и переносит в raw-db/: папка улик остаётся 0700 root, а backup --keep 1 не стирает суточные копии в /var/backups/cs2-server"
  - "collect-runner.ps1 сохранён с меткой UTF-8 (BOM): без неё Windows PowerShell 5.1 читает скрипт как ANSI и портит русские строки"
  - "Состояние раннера «до» и «после» собирается двумя запусками в разные подпапки сырой папки (import копирует рекурсивно), отдельного параметра для этого нет"

patterns-established:
  - "Скрипты docs/drills/*.sh хранятся с LF (.gitattributes), как docs/deploy"

requirements-completed: [OPS-04]

coverage:
  - id: D1
    description: "scan-token.sh находит подставной токен в тексте, в форме с '~', в двоичном файле, по секретной части и по второму образцу; чистая папка даёт код 0; файл образцов внутри репозитория даёт код 2; несуществующий путь даёт код 2"
    requirement: OPS-04
    verification:
      - kind: integration
        ref: "docs/drills/selftest.sh (пункты а, б, в)"
        status: pass
    human_judgment: false
  - id: D2
    description: "evidence.sh import копирует только текстовые файлы, пропускает raw-db, chat-export, базы, token и файлы больше 5 МиБ, пишет import-log.txt и token-scan.txt; код 1 при токене в сырой папке; код 2 без файла образцов"
    requirement: OPS-04
    verification:
      - kind: integration
        ref: "docs/drills/selftest.sh (пункт г)"
        status: pass
    human_judgment: false
  - id: D3
    description: "evidence.sh check отвергает папку без RESULT.md, с пустым или отсутствующим файлом из списка улик, с несовпадающим drill:, с базой, в UTF-16, с токеном, добавленным после import; --require-pass отвергает result: fail"
    requirement: OPS-04
    verification:
      - kind: integration
        ref: "docs/drills/selftest.sh (пункт д)"
        status: pass
    human_judgment: false
  - id: D4
    description: "Значение подставного токена, его форма с '~', секретная часть и второй образец не попадают ни в вывод скриптов, ни в папку улик, ни в token-scan.txt и import-log.txt"
    requirement: OPS-04
    verification:
      - kind: integration
        ref: "docs/drills/selftest.sh (пункт е)"
        status: pass
    human_judgment: false
  - id: D5
    description: "collect-runner.ps1 разбирается без ошибок, собирает runner.log, runner_state.json, listing.txt, processes.txt в UTF-8, не копирует token и не пишет в непустую папку"
    requirement: OPS-04
    verification:
      - kind: other
        ref: "ParseFile разбор и ручной прогон на синтетической рабочей папке раннера, затем evidence.sh import (файл token не попал, секрет не найден)"
        status: pass
    human_judgment: false
  - id: D6
    description: "collect-server.sh на настоящем Linux-хосте собирает журналы, состояние служб, job show, результат с SHA-256, диск и копию базы"
    requirement: OPS-04
    verification:
      - kind: other
        ref: "bash -n; проверка аргументов и отказа без sudo на этой машине"
        status: pass
    human_judgment: true
    rationale: "Скрипт не запускался на хосте: нет Linux-машины с systemd, Docker и cs2-server. Первый живой запуск — учение плана 09-08; формат вывода journalctl, docker compose logs --since и sudo -u cs2-server проверит оно"

duration: 25min
completed: 2026-10-13
status: complete
---

# Phase 9 Plan 06: Инструменты учений Summary

**Путь улик от сырой папки до проверенной папки фазы: `scan-token.sh` ищет секреты по файлу образцов вне репозитория, `evidence.sh import/check` отбирает текст и проверяет полноту, `selftest.sh` доказывает всё на подставном токене, а `collect-server.sh` и `collect-runner.ps1` собирают улики на обеих машинах; правила учений и образец протокола записаны в README**

## Performance

- **Duration:** около 25 мин
- **Completed:** 2026-10-13T16:12Z
- **Tasks:** 2 (трассер и обычный)
- **Files modified:** 7 (6 созданы)

## Accomplishments

- `scan-token.sh`: образцы только из файла (строки с `#` и пустые пропускаются, CRLF снимается), к каждому образцу с `:` добавляются форма с `~` и секретная часть длиннее 16 знаков; `grep -rFac -H -f` по текстам и двоичным файлам; печатаются «путь: число» и «совпадений: N», значения — никогда; код 0/1/2, файл образцов внутри рабочего дерева git даёт 2.
- `evidence.sh import`: рекурсивный отбор `.txt .log .md .json .csv` и `runner.log.1`, пропуск `raw-db/`, `chat-export/`, баз, `token` и файлов больше 5 МиБ с причинами в `import-log.txt`, поиск по сырой папке и по уликам, итог в `token-scan.txt`. `check`: `RESULT.md` (`drill:`, `result:`, `date:`, `## Исход`, `## Улики`), непустота файлов из списка, отсутствие баз и экспорта чата, `совпадений: 0`, UTF-16 отвергается, `--require-pass`.
- `selftest.sh` выдаёт `selftest: OK` за 15 секунд, проверяя пункты (а)-(е) плана и ещё четыре случая: секретная часть и второй образец, форма с `~` при короткой секретной части, несуществующий путь, токен, добавленный в улики после `import`.
- Самопроверка проверена мутациями на копиях скриптов: без формы с `~` (падает на коротком образце), без порога секретной части, без отказа от UTF-16, без лимита 5 МиБ и с печатью значений в выводе — каждая поломка роняет `selftest.sh`.
- `collect-server.sh` и `collect-runner.ps1`: сбор по плану; `/etc/cs2-server/env` и `.env` не читаются, файл `token` раннера не копируется, обе утилиты отказываются писать в непустую папку (правило повтора).
- README: роли, две папки и что можно в репозиторий, файл образцов, формат `RESULT.md`, правило кодировки (`Out-File -Encoding utf8`), семь разделов образца протокола, таблица учений с планами 09-08/09-09/09-10 и вопросами из 07-UAT и 08-UAT, правило приёмки `--require-pass`, правило повтора, репетиция на фейках, что делать при провале.

## Task Commits

1. **Task 1 (трассер): путь улик — отбор, поиск токена, проверка, самопроверка на синтетике** — `3d53f9a` (feat)
2. **Task 2: сбор улик на хосте и на машине раннера** — `5e59466` (feat)

**Plan metadata:** коммит `docs(09-06)` с этим SUMMARY, STATE.md и ROADMAP.md.

## Verification

- `bash docs/drills/selftest.sh` — `selftest: OK`, код 0 (запущен после каждой правки и перед обоими коммитами).
- `bash -n` для `scan-token.sh`, `evidence.sh`, `selftest.sh`, `collect-server.sh` — без ошибок; `Parser::ParseFile` для `collect-runner.ps1` — без ошибок.
- Проверка заголовков README (`--require-pass`, `Out-File -Encoding utf8`, семь заголовков образца, `09-08`, `09-09`, `09-10`) — `readme-ok`.
- Критерии приёмки по `grep -c`: `grep -rFac` и `rev-parse --is-inside-work-tree` в `scan-token.sh`; `journalctl -u cs2-server`, `docker compose`, `backup`, `sha256sum` в `collect-server.sh`, строки `cat /etc/cs2-server/env` нет; `runner_state.json`, `listing.txt`, `processes.txt`, `AppDb`, `Out-File -Encoding utf8` в `collect-runner.ps1`.
- Ручной прогон `collect-runner.ps1` на синтетической рабочей папке раннера (с файлом `token`, `out\` и `remote\`), затем `evidence.sh import` с образцом, равным содержимому `token`: файл `token` не попал в сборку, секретов не найдено, повторный запуск в непустую папку отвергнут.

## Decisions Made

См. `key-decisions` во frontmatter.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Правило LF для скриптов учений**
- **Found during:** Task 1 (создание `.sh`-файлов на Windows с `core.autocrlf`)
- **Issue:** без правила в `.gitattributes` рабочая копия получила бы CRLF, а `bash` на Linux-хосте (`collect-server.sh`) CRLF не принимает; тот же случай уже закрывала фаза 09-05 для `docs/deploy`
- **Fix:** строка `docs/drills/*.sh text eol=lf` в `.gitattributes`
- **Files modified:** `.gitattributes`
- **Committed in:** `3d53f9a`

**2. [Rule 2 - Missing critical] Повторный поиск секретов в `check` и четыре дополнительных случая самопроверки**
- **Found during:** Task 1 (мутационная проверка `selftest.sh`)
- **Issue:** первая мутация (удаление формы с `~`) не роняла самопроверку: форму находила секретная часть. Кроме того, `token-scan.txt` от `import` не охраняет файлы, добавленные в папку улик позже
- **Fix:** в `selftest.sh` добавлен образец с короткой секретной частью; `check` повторно ищет секреты в уликах, если файл образцов доступен; добавлены случаи «токен добавлен после import», «drill: не совпадает с папкой», «нет файла образцов», «несуществующий путь»
- **Files modified:** `docs/drills/evidence.sh`, `docs/drills/selftest.sh`
- **Committed in:** `3d53f9a`

**3. [Rule 1 - Bug] Копия базы в `collect-server.sh` через временную папку**
- **Found during:** Task 2 (проектирование по плану: «`raw-db/` — каталог во владении `cs2-server`»)
- **Issue:** `raw-db/` во владении `cs2-server` внутри папки 0700 root недоступна пользователю службы (нет права входа в родителя), а `backup --keep 1` в общей папке копий стёр бы суточные копии
- **Fix:** копия снимается служебной командой в `mktemp -d /var/tmp/cs2drill.XXXXXX` (владелец `cs2-server`, 0700), затем переносится в `raw-db/` папки улик; временная папка удаляется
- **Files modified:** `docs/drills/collect-server.sh`
- **Committed in:** `5e59466`

---

**Total deviations:** 3 auto-fixed (1 blocking, 1 missing critical, 1 bug)
**Impact on plan:** набор скриптов и README соответствуют плану; правки только добавляют охрану или чинят пути, которые план не мог проверить без хоста.

## Issues Encountered

- Метка UTF-8 в `collect-runner.ps1`: Write не пишет BOM, а Windows PowerShell 5.1 читает такой скрипт как ANSI. Метка добавлена после записи; разбор и прогон на синтетике проходят, русские строки в файлах улик читаются как UTF-8 (с меткой).
- В консоли PowerShell кириллица в сообщениях скрипта показывается в кодировке OEM; файлы на диске в UTF-8.
- `collect-server.sh` не запускался на Linux: нет хоста с systemd и Docker. Первый живой запуск — учение в плане 09-08 (флаг для 09-08: формат `docker compose logs --since`, права `sudo -u cs2-server` на `/var/tmp`, имя `job show`).

## Known Stubs

None. Заглушек и TODO в созданных файлах нет.

## Threat Flags

Новых сетевых поверхностей нет. Реестр плана: T-09-23 (секреты только из файла образцов, внутри репозитория отвергается; пункт (в) самопроверки), T-09-24 (отсев баз, чата, `token` и больших файлов; пункты (г), (д)), T-09-25 (в вывод идут только числа, UTF-16 отвергается; пункты (д), (е), мутационная проверка печати значений).

## User Setup Required

None для этого плана. Файл `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` автор создаёт перед первым `import` (план 09-08), формат описан в `docs/drills/README.md`.

## Next Phase Readiness

- План 09-07 пишет протоколы `00-stand.md`…`08-day-soak.md` по образцу из семи разделов и по командам `evidence.sh` из README.
- Правило приёмки и правило повтора записаны; `evidence.sh check --require-pass` — единственный критерий «учение пройдено».
- Допущения, оставшиеся для живых учений: поведение `collect-server.sh` на хосте; `evidence.sh` и `scan-token.sh` рассчитаны на GNU grep и bash 4+ (Git Bash, Ubuntu).

## Self-Check: PASSED

- Файлы на диске: `docs/drills/README.md`, `scan-token.sh`, `evidence.sh`, `selftest.sh`, `collect-server.sh`, `collect-runner.ps1`.
- Коммиты в ветке: `3d53f9a`, `5e59466`; `commits: 2` измерено `git rev-list --count` от `plan_head_before`.
- `bash docs/drills/selftest.sh` — `selftest: OK`.
- Rust-файлы, `.planning/config.json`, `.planning/state.json`, `.planning/milestone.lock`, `crates/store/src/journal.rs` не затронуты.

---
*Phase: 09-uborka-razvertyvanie-i-ucheniya*
*Completed: 2026-10-13*
