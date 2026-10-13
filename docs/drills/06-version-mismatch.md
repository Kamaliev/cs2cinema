# 06-version-mismatch: раннер другой версии протокола

Учение проводит автор в плане 09-09. Правила, роли, формат `RESULT.md` и правило кодировки — в `docs/drills/README.md`. Общие приёмы (`ctl`, сборщики, перенос файлов в сырую папку) — в `docs/drills/00-stand.md`, раздел «Общие приёмы».

## Цель

Критерий 4 фазы 9 (OPS-04), шестой пункт: раннер, собранный с другим номером протокола (`PROTOCOL_VERSION = 2` вместо `1` в `crates/wire/src/lib.rs`), не забирает задачи, а останавливается с понятной фразой; сервер отвечает 426 `protocol_mismatch`, задача остаётся в очереди и не получает выдач; затем обычный раннер доводит её до конца. Ответ 426 и ветка остановки раннера на фейках проверены (`lease_http`); учение проверяет их на настоящем сервере.

Сверяется только номер протокола. Версия пакета (`app_version`) пишется в журнал сервера и в `runners.version`, но не сверяется; усиление проверки вне требований.

## Предусловия

1. Стенд из `docs/drills/00-stand.md` готов, живая цепочка пройдена. Других учений на стенде нет.
2. Раннер с `PROTOCOL_VERSION = 2` собирает **исполнитель** в плане 09-09 в отдельном рабочем дереве вне репозитория: готовый файл лежит в `%LOCALAPPDATA%\cs2cinema\drills\proto2\cs2-cli.exe`. Основная ветка и этот протокол его не меняют: в основном дереве `PROTOCOL_VERSION: u32 = 1`.
3. Файл токена раннера (одна строка, права только владельца, вне репозитория; токен в командную строку не передаётся) и обычный раннер: приложение `cs2cinema.exe` в режиме раннера. На время части 1 приложение закрыто.
4. Сырая папка `%LOCALAPPDATA%\cs2cinema\drills\raw\06-version-mismatch` и папка улик `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/06-version-mismatch` не существуют (правило повтора README); на хосте нет `~/drill/06-version-mismatch` и `~/raw/06-version-mismatch`.
5. Файл образцов секретов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` лежит вне репозитория.

## Шаги

### Подготовка

1. Хост: `date '+%Y-%m-%d %H:%M:%S'` — `<начало>` для сборщика; `mkdir -p ~/drill/06-version-mismatch`; `ctl` определена как в `docs/drills/00-stand.md`.
2. Windows, из корня репозитория:

```powershell
$raw = "$env:LOCALAPPDATA\cs2cinema\drills\raw\06-version-mismatch"
$exe = "$env:LOCALAPPDATA\cs2cinema\drills\proto2\cs2-cli.exe"
$dir2 = "$env:LOCALAPPDATA\cs2cinema\drills\runner-proto2"
New-Item -ItemType Directory -Force $raw | Out-Null
Test-Path $exe
```

   `Test-Path` обязан напечатать `True`. Приложение `cs2cinema.exe` закрыто: `Get-Process cs2cinema -ErrorAction SilentlyContinue` ничего не печатает.

### Часть 1. Раннер другой версии

1. `друг` присылает демку; номер задачи — `<N>`. Раннера нет, задача ждёт в очереди. Снять состояние «до»: `{ date; ctl job show <N>; } 2>&1 | tee ~/drill/06-version-mismatch/job-show-<N>-before.txt` (ожидается `queued` и `выдач: 0`).
2. Запустить раннер другой версии **со своей рабочей папкой** и выводом в файл. Пути без пробелов; запуск через `cmd /c`, чтобы вывод остался в UTF-8 (перенаправление `>` в PowerShell 5.1 дало бы UTF-16, где поиск токена ничего не найдёт, а `evidence.sh check` такой файл отвергает):

```powershell
cmd /c "$exe runner --server https://<адрес из 09-STAND.md> --token-file <файл токена> --dir $dir2 > $raw\console-proto2.txt 2>&1"
```

   Раннер здоровается с сервером, получает 426 и останавливается сам. Если он остался работать — это провал: остановить `Ctrl+C` и записать в `notes.txt`.
3. Снять `console-proto2.txt` в уликах: файл содержит фразу про версию протокола. Состояние задачи после неудачной попытки: `{ date; ctl job show <N>; } 2>&1 | tee ~/drill/06-version-mismatch/job-show-<N>-mid.txt`. В `/status` у `друга` записать, что показано (задача в очереди; строка «Раннер сейчас не в сети — задача подождёт его.» может появиться или нет: сервер считает раннер «на связи» по любому запросу с токеном; записать, как было).
4. Скопировать журнал раннера другой версии в сырую папку и удалить его папку: `Copy-Item "$dir2\runner.log" "$raw\runner-proto2.log"` (если файла нет — записать в `notes.txt`), затем `Remove-Item -Recurse -Force $dir2`.

### Часть 2. Обычный раннер доводит задачу до конца

1. Запустить `cs2cinema.exe` в режиме раннера. Дальше ничего не нажимать: задача `<N>` берётся в работу и доходит до ролика у `друга`.
2. Состояние «после»: `{ date; ctl job show <N>; } 2>&1 | tee ~/drill/06-version-mismatch/job-show-<N>-after.txt`.

### Завершение

1. Состояние обычного раннера (из корня репозитория): `powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp"`, затем `Move-Item "$raw\_tmp\*" $raw` и `Remove-Item "$raw\_tmp"` (имена `runner.log`, `runner_state.json`, `listing.txt`, `processes.txt`).
2. Хост: `sudo bash docs/drills/collect-server.sh ~/raw/06-version-mismatch "<начало>" <N>`. Скопировать `~/raw/06-version-mismatch/*` и `~/drill/06-version-mismatch/*` в `$raw` (приём из `docs/drills/00-stand.md`).
3. Руками (UTF-8): `chat.txt` — тексты бота и друга по порядку по ролям, без Telegram-ID и имён; `notes.txt` (в том числе показал ли `/status` строку про офлайн-раннер); `verdict.txt` — одна строка `pass` или `fail` и почему.

## Ожидаемый исход

1. `console-proto2.txt` содержит «версия протокола сервера не совпадает с раннером»; раннер другой версии остановился сам, не взяв ни одной задачи.
2. В `server-journal.txt` есть запрос `hello` раннера с ответом 426 (строка вида `POST /v1/runner/hello → 426 …`) и ни одной выдачи задачи этому раннеру.
3. Задача `<N>` осталась в очереди: в `job-show-<N>-before.txt` и `job-show-<N>-mid.txt` состояние `queued`, `выдач: 0`; `друг` в `/status` видит задачу в очереди (ожидание), а не сбой.
4. Затем обычный раннер доводит задачу до `done`: `job-show-<N>-after.txt` — `done`, `доставка в Telegram: sent`, в чате один ролик.
5. `друг` не видит ни номеров протокола, ни фраз про версию, ни локальных путей.

## Чего быть не должно

- Выдачи задачи раннеру другой версии, роста числа выдач, перехода задачи в `failed`.
- Работы раннера другой версии после 426 (повторов запросов без остановки).
- Лишних файлов раннера другой версии на диске после учения (папка `runner-proto2` удалена).
- Изменения версии протокола в основном дереве репозитория (`git status --porcelain -- crates/wire` пуст).
- Токена раннера или бота в `console-proto2.txt`, в журналах и `notes.txt`; имён и Telegram-ID друзей в уликах; UTF-16 в `console-proto2.txt`.

## Улики

Файл из списка существует и непуст. Сборщики: `docs/drills/collect-server.sh` (хост), `docs/drills/collect-runner.ps1` (машина раннера).

- `console-proto2.txt` — вывод раннера другой версии (UTF-8, через `cmd /c`)
- `runner-proto2.log` — журнал раннера другой версии; `runner.log` — журнал обычного раннера
- `server-journal.txt` — журнал `cs2-server` с метками времени, в нём ответ 426 (сборщик хоста)
- `job-show-<N>-before.txt`, `job-show-<N>-mid.txt`, `job-show-<N>-after.txt` — задача до, после неудачной попытки и после ролика; `job-show-<N>.txt` — то же от сборщика хоста
- `chat.txt`, `notes.txt`, `verdict.txt` — написаны руками
- `RESULT.md`, `token-scan.txt` — исполнитель

## Проверка

Команды исполнителя на машине автора (Git Bash, из корня репозитория):

```bash
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/06-version-mismatch" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/06-version-mismatch
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/06-version-mismatch --require-pass
```

Между ними исполнитель сверяет вердикт автора с фактами улик и пишет `RESULT.md` по формату README (`drill: 06-version-mismatch`, `result:` по уликам: `fail` при расхождении с вердиктом; `## Исход` — один абзац; `## Улики` — список выше): `grep -c "версия протокола" console-proto2.txt` не ноль; `grep -c "hello → 426" server-journal.txt` не ноль; в `job-show-<N>-before.txt` `queued` и `выдач: 0`; в `job-show-<N>-after.txt` `done`; в основном дереве `git status --porcelain -- crates/wire` пуст, а `PROTOCOL_VERSION: u32 = 1` на месте.

Правило приёмки: учение пройдено только когда `bash docs/drills/evidence.sh check … --require-pass` для папки `06-version-mismatch` проходит. Код 1 при `import` — найден секрет: папку улик не коммитят, удаляют целиком, токен отзывают, учение повторяют. `result: fail` с полными уликами — записанный провал для `/gsd-plan-phase 9 --gaps`.
