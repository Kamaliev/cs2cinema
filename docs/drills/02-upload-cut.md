# 02-upload-cut: обрыв загрузки ролика

Учение проводит автор в плане 09-09. Правила, роли, формат `RESULT.md` и правило кодировки — в `docs/drills/README.md`. Общие приёмы (`ctl`, сборщики, перенос файлов в сырую папку) — в `docs/drills/00-stand.md`, раздел «Общие приёмы».

## Цель

Критерий 4 фазы 9 (OPS-04), второй пункт: загрузка готового ролика на сервер обрывается, а раннер докачивает его с места обрыва по `HEAD` и длине недозагруженного куска `.part`, а не начинает заново и не записывает ролик повторно; SHA-256 принятого ролика сходится с SHA-256 у раннера. Два способа обрыва:

- «флаг»: скрытый флаг учений `--upload-chunk-limit 2` у `cs2-cli runner` останавливает раннер после двух кусков (на сервере остаётся `<эпоха>.part` длиной 16 МиБ), следующий запуск без флага докачивает;
- «сеть»: посреди загрузки на машине раннера отключается сеть на 30 секунд (`tailscale down` или выключение сети), затем включается.

## Предусловия

1. Стенд из `docs/drills/00-stand.md` готов, живая цепочка пройдена. Других учений на стенде нет (одна запись за раз).
2. Бинарник `target\release\cs2-cli.exe` собран исполнителем (`cargo build --release -p cli`). Раннер — `cs2-cli runner` (приложение флага `--upload-chunk-limit` не имеет). Нужны: файл токена раннера (одна строка, права только владельца, вне репозитория; токен в командную строку не передаётся), отдельная рабочая папка `%LOCALAPPDATA%\cs2cinema\drills\runner-cut`. Приложение `cs2cinema.exe` на время учения закрыто.
3. Демка даёт ролик больше 16 МиБ (результат нескольких дублей; сервер принимает ролик кусками по 8 МиБ, два куска — 16 МиБ). Для варианта «сеть» ролик должен быть достаточно большим, чтобы загрузка шла не меньше минуты.
4. Для варианта «сеть» на хосте доступен `tmux` или `nohup`: если SSH к хосту идёт через тот же Tailscale, что отключается на машине раннера, наблюдение на хосте должно пережить обрыв сеанса.
5. Сырая папка `%LOCALAPPDATA%\cs2cinema\drills\raw\02-upload-cut` и папка улик `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/02-upload-cut` не существуют (правило повтора README); на хосте нет `~/drill/02-upload-cut` и `~/raw/02-upload-cut`.
6. Файл образцов секретов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` лежит вне репозитория.

## Шаги

### Подготовка

1. Хост: `date '+%Y-%m-%d %H:%M:%S'` — `<начало>` для сборщика; `mkdir -p ~/drill/02-upload-cut`; `ctl` определена как в `docs/drills/00-stand.md`.
2. Windows, из корня репозитория:

```powershell
$raw = "$env:LOCALAPPDATA\cs2cinema\drills\raw\02-upload-cut"
$dir = "$env:LOCALAPPDATA\cs2cinema\drills\runner-cut"
New-Item -ItemType Directory -Force $raw | Out-Null
```

Команда раннера (в обеих частях та же, меняется только флаг): `target\release\cs2-cli.exe runner --server https://<адрес из 09-STAND.md> --token-file <файл токена> --dir $dir` (при необходимости `--hlae <папка>`, `--cs2 <папка>`).

### Часть 1. Вариант «флаг»

1. Запустить раннер **с флагом**: команда раннера и `--upload-chunk-limit 2`.
2. `друг` присылает демку; номер задачи — `<N>` (`ctl job list`). Раннер записывает ролик, загружает два куска и останавливается (в `runner.log` видно, что остановка по лимиту кусков, задача осталась в состоянии для продолжения). Дождаться остановки процесса.
3. Хост, до продолжения: `sudo ls -l /var/lib/cs2-server/blobs/results/<N>/ | tee ~/drill/02-upload-cut/part-before.txt` — должен быть файл `<эпоха>.part` длиной 16777216 байт (16 МиБ), готового `highlights.mp4` ещё нет. Состояние задачи: `{ date; ctl job show <N>; } 2>&1 | tee ~/drill/02-upload-cut/job-show-<N>-before.txt` (запомнить `эпоха: E, выдач: G`).
4. Windows, **до продолжения** (после принятия ролика раннер его удаляет): SHA-256 ролика у раннера. Папка задачи называется `out\srv<N>_<герой>`:

```powershell
Get-ChildItem "$dir\out" -Recurse -Filter highlights.mp4 | Where-Object { $_.FullName -match "srv<N>_" } |
  Get-FileHash -Algorithm SHA256 | Out-File -Encoding utf8 "$raw\sha-runner.txt"
```

   Если подходящих файлов нет или их несколько — записать в `notes.txt` и не продолжать: учение повторяют с чистого листа.
5. Запустить раннер **без флага** (та же команда, тот же `--dir`). Он докачивает ролик с места обрыва. Дождаться ролика в чате у `друга`.
6. Хост: `sha256sum` принятого ролика снимает сборщик (`results-<N>.txt`); состояние после: `{ date; ctl job show <N>; } 2>&1 | tee ~/drill/02-upload-cut/job-show-<N>-after.txt`.

### Часть 2. Вариант «сеть»

1. Раннер запущен **без флага** той же командой (`--dir $dir`). `друг` присылает вторую демку; номер задачи — `<M>`.
2. На хосте до начала загрузки запустить наблюдение за ростом `.part` (в `tmux` или через `nohup`, чтобы оно пережило обрыв сеанса): `nohup bash -c 'for i in $(seq 1 600); do date +%T; sudo ls -l /var/lib/cs2-server/blobs/results/<M>/ 2>&1; sleep 2; done' > ~/drill/02-upload-cut/part-growth.txt 2>&1 &`
3. Когда запись закончилась и в `part-growth.txt` пошёл рост `.part` (идёт загрузка): на машине раннера отключить сеть — `tailscale down` (при варианте Caddy — выключить сетевое подключение Windows). Время отключения записать в `notes.txt`.
4. Пока сеть отключена (30 секунд; ролик у раннера ещё на месте, после принятия он будет удалён), снять SHA-256 ролика, как в части 1, шаг 4, но в тот же файл с пометкой: `"# задача <M>" | Out-File -Encoding utf8 -Append "$raw\sha-runner.txt"`, затем тот же конвейер `Get-ChildItem … | Get-FileHash -Algorithm SHA256 | Out-File -Encoding utf8 -Append "$raw\sha-runner.txt"`. Через 30 секунд включить сеть (`tailscale up` или подключение Windows) и записать время в `notes.txt`.
5. Дождаться ролика в чате у `друга`. Состояние: `{ date; ctl job show <M>; } 2>&1 | tee ~/drill/02-upload-cut/job-show-<M>-after.txt`.

### Завершение

1. Остановить раннер (`Ctrl+C` в его окне). Состояние раннера «после» (из корня репозитория; `-Dir` указывает на `runner-cut`): `powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp" -Dir $dir`, затем `Move-Item "$raw\_tmp\*" $raw` и `Remove-Item "$raw\_tmp"` (имена `runner.log`, `runner_state.json`, `listing.txt`, `processes.txt`).
2. Хост: `sudo bash docs/drills/collect-server.sh ~/raw/02-upload-cut "<начало>" <N> <M>` (журнал `server-journal.txt`, `job-show-<N>.txt`, `results-<N>.txt` с SHA-256 ролика). Скопировать `~/raw/02-upload-cut/*` и `~/drill/02-upload-cut/*` в `$raw` (приём из `docs/drills/00-stand.md`).
3. Руками (UTF-8): `chat.txt` — тексты бота и друга по порядку по ролям, без Telegram-ID и имён; `notes.txt`; `verdict.txt` — одна строка `pass` или `fail` и почему.

## Ожидаемый исход

1. Вариант «флаг»: `part-before.txt` показывает `<эпоха>.part` длиной 16777216 байт; после запуска без флага загрузка продолжена с места обрыва, а не начата заново; ролик принят; SHA-256 из `sha-runner.txt` равен SHA-256 из `results-<N>.txt` (регистр букв не важен); в `job-show-<N>-after.txt` те же `эпоха: E, выдач: G`, что в `job-show-<N>-before.txt`, состояние `done`, `доставка в Telegram: sent`.
2. Вариант «сеть»: загрузка после включения сети продолжена, не начата заново: в `part-growth.txt` длина `.part` не падает до нуля и не уменьшается (после паузы она снова растёт с достигнутого значения); ролик один; `job-show-<M>-after.txt` — `done`, `sent`; SHA-256 из `sha-runner.txt` (раздел задачи `<M>`) равен SHA-256 из `results-<M>.txt`.
3. Запись не повторялась: после остановки и возобновления `runner.log` показывает продолжение загрузки и подтверждение, а не новый проход по дублям; число выдач не выросло.
4. В чате у `друга` по одному ролику на задачу, статусы по-русски, без локальных путей.

## Чего быть не должно

- Повторной записи (ролик записан второй раз), роста числа выдач или смены эпохи из-за обрыва.
- Двух роликов в чате на одну задачу.
- Отказа сервера из-за эпохи (ответ «эпоха устарела» в `runner.log` или журнале сервера), задачи в `failed`, потерянного результата.
- Расхождения SHA-256 у раннера и на сервере.
- Токена раннера или бота в журналах, в `notes.txt`, в `chat.txt`; имён и Telegram-ID друзей в уликах; ролика (файла `.mp4`) в папке улик.

## Улики

Файл из списка существует и непуст. Сборщики: `docs/drills/collect-server.sh` (хост), `docs/drills/collect-runner.ps1` (машина раннера).

- `runner.log` — журнал раннера в папке `runner-cut` за обе части
- `server-journal.txt` — журнал `cs2-server` с метками времени (сборщик хоста)
- `part-before.txt` — `ls -l` папки результата с недозагруженным `.part` до продолжения
- `part-growth.txt` — рост `.part` во время варианта «сеть»
- `sha-runner.txt` — SHA-256 ролика у раннера до продолжения (обе задачи, во второй пометка `# задача <M>`)
- `results-<N>.txt`, `results-<M>.txt` — папка результата и SHA-256 принятого ролика (сборщик хоста)
- `job-show-<N>-before.txt`, `job-show-<N>-after.txt`, `job-show-<M>-after.txt`, `job-show-<N>.txt`, `job-show-<M>.txt` — состояние задач
- `chat.txt`, `notes.txt`, `verdict.txt` — написаны руками
- `RESULT.md`, `token-scan.txt` — исполнитель

## Проверка

Команды исполнителя на машине автора (Git Bash, из корня репозитория):

```bash
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/02-upload-cut" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/02-upload-cut
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/02-upload-cut --require-pass
```

Между ними исполнитель сверяет суммы (`sha-runner.txt` против `results-<N>.txt` и `results-<M>.txt`), длину `.part` в `part-before.txt`, выдачи и эпоху в `job-show-*`, наличие одного ролика в `chat.txt`, и пишет `RESULT.md` по формату README (`drill: 02-upload-cut`, `result:` по уликам: `fail` при расхождении с вердиктом автора; `## Исход` — по абзацу на каждый вариант; `## Улики` — список выше).

Правило приёмки: учение пройдено только когда `bash docs/drills/evidence.sh check … --require-pass` для папки `02-upload-cut` проходит. Код 1 при `import` — найден секрет: папку улик не коммитят, удаляют целиком, токен отзывают, учение повторяют. `result: fail` с полными уликами — записанный провал для `/gsd-plan-phase 9 --gaps`.
