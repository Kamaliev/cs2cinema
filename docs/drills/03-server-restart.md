# 03-server-restart: перезапуск сервера посреди задачи

Учение проводит автор в плане 09-09. Правила, роли, формат `RESULT.md` и правило кодировки — в `docs/drills/README.md`. Общие приёмы (`ctl`, сборщики, перенос файлов в сырую папку) — в `docs/drills/00-stand.md`, раздел «Общие приёмы».

## Цель

Критерий 4 фазы 9 (OPS-04), третий пункт: сервер перезапускается (штатно и аварийно) посреди задачи, а задача всё равно доходит до одного ролика. Проверяются механизмы, которые на фейках (`lease_http`, `transfer_http`, `remote::e2e`) уже зелёные, на настоящем systemd и сети: восстановление сервера при старте (продление аренд на время `startup_grace_secs`, строка журнала «восстановление: …»), повтор запросов раннером с паузой до 30 секунд, докачка загрузки, рестарт службы после падения (`Restart=on-failure`, пять секунд).

Две части:

- штатный перезапуск `sudo systemctl restart cs2-server` во время **записи**;
- аварийный `kill -9` главного процесса службы во время **загрузки** ролика.

## Предусловия

1. Стенд из `docs/drills/00-stand.md` готов, живая цепочка пройдена. Других учений на стенде нет (одна запись за раз).
2. Раннер — приложение `cs2cinema.exe` в режиме раннера (повседневный вид; рабочая папка `%LOCALAPPDATA%\cs2cinema\runner`), запущено и «на связи».
3. Для части 2 ролик должен быть достаточно большим, чтобы загрузка шла не меньше нескольких секунд (результат нескольких дублей; кусок результата — 8 МиБ).
4. Сырая папка `%LOCALAPPDATA%\cs2cinema\drills\raw\03-server-restart` и папка улик `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/03-server-restart` не существуют (правило повтора README); на хосте нет `~/drill/03-server-restart` и `~/raw/03-server-restart`.
5. Файл образцов секретов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` лежит вне репозитория.

## Шаги

### Подготовка

1. Хост: `date '+%Y-%m-%d %H:%M:%S'` — `<начало>` для сборщика; `mkdir -p ~/drill/03-server-restart`; `ctl` определена как в `docs/drills/00-stand.md`. Убедиться, что служба поднимается сама: `systemctl show -p Restart,RestartUSec cs2-server` показывает `Restart=on-failure`, `RestartUSec=5s`.
2. Windows, из корня репозитория: `$raw = "$env:LOCALAPPDATA\cs2cinema\drills\raw\03-server-restart"` и `New-Item -ItemType Directory -Force $raw | Out-Null`.

### Часть 1. `systemctl restart` во время записи

1. `друг` присылает демку; номер задачи — `<N1>` (`ctl job list`).
2. Дождаться этапа «Запись»: повторять `ctl job show <N1>`, пока не появится `этап: …` записи и `дубль: k из n`. Снять состояние «до»: `{ date; ctl job show <N1>; } 2>&1 | tee ~/drill/03-server-restart/job-show-<N1>-before.txt` (запомнить `эпоха: E, выдач: G`).
3. Перезапустить службу: `date '+%H:%M:%S'; sudo systemctl restart cs2-server` (время — в `notes.txt`). Сразу посмотреть журнал: `sudo journalctl -u cs2-server --since "2 minutes ago" -o short-iso --no-pager | tail -n 30`.
4. Ничего не нажимать. Раннер повторяет запросы с паузой до 30 секунд, находит сервер и продолжает запись. Дождаться ролика в чате у `друга`.
5. Состояние «после»: `{ date; ctl job show <N1>; } 2>&1 | tee ~/drill/03-server-restart/job-show-<N1>-after.txt`.

### Часть 2. `kill -9` во время загрузки

1. `друг` присылает вторую демку; номер задачи — `<N2>`.
2. Дождаться конца записи и начала загрузки: на хосте `sudo ls -l /var/lib/cs2-server/blobs/results/<N2>/` показывает растущий файл `<эпоха>.part`. Снять состояние «до»: `{ date; ctl job show <N2>; } 2>&1 | tee ~/drill/03-server-restart/job-show-<N2>-before.txt`.
3. Убить главный процесс службы: `date '+%H:%M:%S'; sudo kill -9 "$(systemctl show -p MainPID --value cs2-server)"` (время — в `notes.txt`). Через пять секунд systemd поднимет службу сам (`Restart=on-failure`); проверить: `systemctl is-active cs2-server`.
4. Ничего не нажимать. Загрузка докачивается с места обрыва, задача завершается. Дождаться ролика в чате у `друга`.
5. Состояние «после»: `{ date; ctl job show <N2>; } 2>&1 | tee ~/drill/03-server-restart/job-show-<N2>-after.txt`.

### Завершение

1. Состояние раннера «после» (из корня репозитория): `powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp"`, затем `Move-Item "$raw\_tmp\*" $raw` и `Remove-Item "$raw\_tmp"` (имена `runner.log`, `runner_state.json`, `listing.txt`, `processes.txt`).
2. Хост: `sudo bash docs/drills/collect-server.sh ~/raw/03-server-restart "<начало>" <N1> <N2>`. Он собирает `server-journal.txt` с метками времени (`journalctl -o short-iso`), `job-list.txt`, `job-show-<N>.txt`, `results-<N>.txt`. Скопировать `~/raw/03-server-restart/*` и `~/drill/03-server-restart/*` в `$raw` (приём из `docs/drills/00-stand.md`).
3. Руками (UTF-8): `chat.txt` — тексты бота и друга по порядку по ролям, без Telegram-ID и имён; `notes.txt` (времена перезапуска и убийства, что видел автор); `verdict.txt` — одна строка `pass` или `fail` и почему.

## Ожидаемый исход

Для каждой из двух частей:

1. В `server-journal.txt` после момента перезапуска (или убийства и возврата службы) есть строка «восстановление: упавших приёмов 0, продлённых аренд 1»: сервер при старте продлил аренду идущей задачи и не уронил ни одного приёма.
2. Раннер повторяет запросы с паузой до 30 секунд и продолжает: в `runner.log` видны повторы запросов (сбои сети) и затем «на связи с сервером …»; задача не бросается и не начинается заново.
3. Выдач не прибавилось: в `job-show-<N>-after.txt` те же `эпоха: E, выдач: G`, что в `job-show-<N>-before.txt`; состояние `done`.
4. Часть 2: загрузка докачана, а не начата заново; строка `результат: … МиБ, sha256 …` в `job-show-<N2>-after.txt` совпадает по размеру и SHA-256 с файлом в `results-<N2>.txt` (там `sha256sum` принятого ролика, регистр букв не важен).
5. В чате у `друга` по одному ролику на задачу, статусы по-русски, без локальных путей и кодов сбоя; `job-show-<N>-after.txt` содержит `доставка в Telegram: sent`.
6. Часть 2: после `kill -9` служба поднялась сама примерно через пять секунд (метки времени в `server-journal.txt`: остановка и новый запуск).

## Чего быть не должно

- Задачи в состоянии `failed` из-за перезапуска сервера.
- Повторной записи (ролик записан заново) или роста числа выдач.
- Второго ролика в чате на ту же задачу.
- Потерянного результата: задача `done`, а ролика в чате нет, либо ролик есть, а задача висит.
- Службы, которая не поднялась сама после `kill -9`, либо поднялась с ошибкой базы.
- Токена раннера или бота в журналах, в `notes.txt`, в `chat.txt`; имён и Telegram-ID друзей в уликах; копий базы (`*.db`) в папке улик.

## Улики

Файл из списка существует и непуст. Сборщики: `docs/drills/collect-server.sh` (хост), `docs/drills/collect-runner.ps1` (машина раннера).

- `server-journal.txt` — журнал `cs2-server` с метками времени за всё учение (сборщик хоста)
- `runner.log` — журнал раннера «после»
- `job-show-<N1>-before.txt`, `job-show-<N1>-after.txt`, `job-show-<N2>-before.txt`, `job-show-<N2>-after.txt` — задачи до и после; `job-show-<N1>.txt`, `job-show-<N2>.txt` — то же от сборщика хоста
- `results-<N2>.txt` — папка результата и SHA-256 принятого ролика (сборщик хоста)
- `chat.txt`, `notes.txt`, `verdict.txt` — написаны руками
- `RESULT.md`, `token-scan.txt` — исполнитель

## Проверка

Команды исполнителя на машине автора (Git Bash, из корня репозитория):

```bash
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/03-server-restart" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/03-server-restart
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/03-server-restart --require-pass
```

Между ними исполнитель сверяет вердикт автора с фактами улик и пишет `RESULT.md` по формату README (`drill: 03-server-restart`, `result:` по уликам: `fail` при расхождении с вердиктом; `## Исход` — по абзацу на каждую часть; `## Улики` — список выше): в `server-journal.txt` есть «восстановление: упавших приёмов 0, продлённых аренд 1» (`grep -c "восстановление:" server-journal.txt` не меньше 2: по строке на штатный перезапуск и на возврат службы после `kill -9`; `<начало>` взято в начале учения, поэтому старт при установке стенда в журнал не попадает), выдачи и эпоха в `-before` и `-after` равны, в `chat.txt` один ролик на задачу.

Правило приёмки: учение пройдено только когда `bash docs/drills/evidence.sh check … --require-pass` для папки `03-server-restart` проходит. Код 1 при `import` — найден секрет: папку улик не коммитят, удаляют целиком, токен отзывают, учение повторяют. `result: fail` с полными уликами — записанный провал для `/gsd-plan-phase 9 --gaps`.
