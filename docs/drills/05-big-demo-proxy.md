# 05-big-demo-proxy: демка в 500 МБ через прокси

Учение проводит автор в плане 09-10. Правила, роли, формат `RESULT.md` и правило кодировки — в `docs/drills/README.md`. Общие приёмы (`ctl`, сборщики, перенос файлов в сырую папку) — в `docs/drills/00-stand.md`, раздел «Общие приёмы».

## Цель

Критерий 4 фазы 9 (OPS-04), пятый пункт: демка порядка 500 МБ доходит от друга через бот и сервер до раннера через выбранный прокси (Tailscale Serve или Caddy) целой, а память сервера не растёт вместе с размером файла. Проверяются передача кусками с докачкой, сверка SHA-256 на обеих сторонах, таймауты прокси (больше long-poll 25 секунд), память процесса `cs2-server`. На малых файлах это проверяют `transfer_http` и `remote::e2e`; 500 МБ — только вручную.

Файл может быть настоящей большой демкой либо файлом с сигнатурой `PBDEMS2\0` и случайной добивкой. В последнем случае транспорт пройдёт, а предварительная проверка закончится понятной фразой «демка повреждена или это не демка CS2» (код `BAD_DEMO`) — это тоже допустимый исход учения: проверяется транспорт, а не разбор.

## Предусловия

1. Стенд из `docs/drills/00-stand.md` готов, прокси выбранного варианта (Tailscale Serve или Caddy) работает, раннер ходит на сервер через него. Других учений на стенде нет.
2. Раннер — приложение `cs2cinema.exe` в режиме раннера, запущено и «на связи». Рабочая папка `%LOCALAPPDATA%\cs2cinema\runner` (в командах ниже `$dir`).
3. Свободное место: на хосте не менее 1,5 ГБ (приём демки идёт через рабочую папку bot-api и папку `blobs/incoming`), на машине раннера не менее 1 ГБ.
4. Сырая папка `%LOCALAPPDATA%\cs2cinema\drills\raw\05-big-demo-proxy` и папка улик `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/05-big-demo-proxy` не существуют (правило повтора README); на хосте нет `~/drill/05-big-demo-proxy` и `~/raw/05-big-demo-proxy`.
5. Файл образцов секретов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` лежит вне репозитория.

## Шаги

### Подготовка

1. Файл демки. Настоящая большая демка (около 500 МБ) подходит как есть. Синтетический файл — вне репозитория, на машине `друга` (или автора, если `друг` — второй аккаунт на той же машине):

```powershell
$big = "$env:LOCALAPPDATA\cs2cinema\drills\big.dem"
$fs = [IO.File]::Create($big)
$sig = [byte[]]([Text.Encoding]::ASCII.GetBytes("PBDEMS2") + 0)
$fs.Write($sig, 0, $sig.Length)
$buf = New-Object byte[] (1MB)
$rnd = New-Object Random
1..500 | ForEach-Object { $rnd.NextBytes($buf); $fs.Write($buf, 0, $buf.Length) }
$fs.Close()
```

2. Хост: `date '+%Y-%m-%d %H:%M:%S'` — `<начало>` для сборщика; `mkdir -p ~/drill/05-big-demo-proxy`; `ctl` определена как в `docs/drills/00-stand.md`.
3. Хост: наблюдение за памятью службы, 5 секунд на замер, сорок минут (в `tmux` или через `nohup`); значения в КиБ:

```bash
nohup bash -c 'for i in $(seq 1 480); do echo "$(date +%T) $(ps -o rss= -p "$(systemctl show -p MainPID --value cs2-server)")"; sleep 5; done' > ~/drill/05-big-demo-proxy/rss.txt 2>&1 &
```

4. Windows, из корня репозитория: `$raw = "$env:LOCALAPPDATA\cs2cinema\drills\raw\05-big-demo-proxy"`, `$dir = "$env:LOCALAPPDATA\cs2cinema\runner"`, `New-Item -ItemType Directory -Force $raw | Out-Null`. Наблюдение за скачиванием у раннера (оставить работать, закончит само): оно ждёт появления готового `remote\srv<N>.dem` (скачивание идёт в `.dem.part`, переименование — признак конца), пишет время в `timings.txt` и считает SHA-256, пока файл не удалён раннером после задачи. `<N>` известен после отправки файла боту (`ctl job list`); подставить и запустить сразу после шага «Отправка»:

```powershell
$f = "$dir\remote\srv<N>.dem"
while (-not (Test-Path $f)) { Start-Sleep 1 }
"скачивание у раннера закончено: {0}" -f (Get-Date -Format s) | Out-File -Encoding utf8 -Append "$raw\timings.txt"
Get-FileHash -Algorithm SHA256 $f | Out-File -Encoding utf8 "$raw\sha-runner.txt"
```

### Отправка и ход задачи

1. Записать в `timings.txt` время отправки (часы Windows): `"отправлено боту: {0}" -f (Get-Date -Format s) | Out-File -Encoding utf8 -Append "$raw\timings.txt"` — и сразу отправить файл боту от имени `друга` (файл до 2000 МБ локальный bot-api принимает). Время сообщения бота «Принял файл… Скачиваю…» и время «Задача #… в очереди» взять из Telegram и дописать в `timings.txt` руками (`принят ботом: ЧЧ:ММ:СС`, `в очереди: ЧЧ:ММ:СС`). Часы хоста и Windows могут расходиться: записать расхождение в `notes.txt`.
2. Номер задачи — `<N>` (`ctl job list`). После того как демка принята сервером (состояние `queued` или дальше), на хосте: `sudo sha256sum /var/lib/cs2-server/blobs/incoming/<N>/demo | tee ~/drill/05-big-demo-proxy/sha-server.txt` (хеш считается, пока файл лежит; если папка уже убрана — вывод покажет это, и в этом случае хеш берётся из `ctl job show <N>`: строка `sha256:` у демки).
3. Раннер забирает задачу и скачивает демку через прокси. Если связь рвётся — раннер докачивает; ничего не нажимать. Исход: ролик в чате (для настоящей демки) или фраза «демка повреждена или это не демка CS2» (для синтетической). Номер задачи, время и исход записать в `notes.txt`.
4. Прокси (после конца задачи): вариант А — `tailscale serve status 2>&1 | sed -E 's/[a-z0-9-]+\.ts\.net/<узел>.ts.net/g' | tee ~/drill/05-big-demo-proxy/proxy.txt`; вариант Б — `sudo journalctl -u caddy --since "<начало>" -o short-iso --no-pager | sed -E 's/[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/<ip>/g' | tee ~/drill/05-big-demo-proxy/proxy.txt` (адреса заменяются на месте, чтобы чужие IP не попали в улики).
5. Остановить наблюдение за памятью, если оно ещё идёт: `pkill -f 'seq 1 480'`. Максимум памяти: `sort -k2 -n ~/drill/05-big-demo-proxy/rss.txt | tail -n 1`. Порог — 200 МБ, то есть 204800 КиБ.

### Завершение

1. Состояние раннера «после» (из корня репозитория): `powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp"`, затем `Move-Item "$raw\_tmp\*" $raw` и `Remove-Item "$raw\_tmp"`.
2. Хост: `sudo bash docs/drills/collect-server.sh ~/raw/05-big-demo-proxy "<начало>" <N>`. Скопировать `~/raw/05-big-demo-proxy/*` и `~/drill/05-big-demo-proxy/*` в `$raw` (приём из `docs/drills/00-stand.md`).
3. Синтетический файл удалить: `Remove-Item "$env:LOCALAPPDATA\cs2cinema\drills\big.dem"`.
4. Руками (UTF-8): `chat.txt` — тексты бота и друга по порядку по ролям, без Telegram-ID и имён; `notes.txt` (расхождение часов, обрывы, что видел автор); `verdict.txt` — одна строка `pass` или `fail` и почему.

## Ожидаемый исход

1. Передача дошла: демка принята сервером и скачана раннером; если связь рвалась, раннер докачал (в `runner.log` продолжение скачивания, а не повтор с нуля).
2. Хеши равны: SHA-256 в `sha-server.txt` равен SHA-256 в `sha-runner.txt` (и строке `sha256:` демки в `job-show-<N>.txt`); регистр букв не важен.
3. Память сервера не растёт с размером файла: максимум второго столбца `rss.txt` меньше 204800 КиБ (200 МБ).
4. Исход понятен: ролик в чате у `друга` (настоящая демка) или фраза «демка повреждена или это не демка CS2» (синтетическая); во втором случае задача в состоянии `failed` с кодом `BAD_DEMO` в `job-show-<N>.txt` (код видит автор, друг его не видит).
5. В `timings.txt` есть время отправки, приёма ботом, постановки в очередь и конца скачивания у раннера; общая длительность передачи записана в `RESULT.md`.
6. В `proxy.txt` нет ошибок прокси (для Caddy — нет `error` по маршруту `/v1/runner/…` и обрывов по таймауту).

## Чего быть не должно

- Расхождения SHA-256 у сервера и у раннера (повреждённая передача).
- Памяти `cs2-server` порядка размера файла (рост до сотен мегабайт и больше).
- Обрыва по таймауту прокси без докачки, зависшей задачи без движения, для синтетического файла — `failed` с кодом, отличным от `BAD_DEMO`; для настоящей демки — `failed` вообще.
- Заполненного диска хоста или машины раннера.
- Токена раннера или бота в журналах, в `proxy.txt`, в `notes.txt`; имён и Telegram-ID друзей в уликах; IP-адресов в `proxy.txt`; самой демки (файла `.dem`) в папке улик.

## Улики

Файл из списка существует и непуст. Сборщики: `docs/drills/collect-server.sh` (хост), `docs/drills/collect-runner.ps1` (машина раннера).

- `rss.txt` — память службы каждые 5 секунд (КиБ)
- `sha-server.txt`, `sha-runner.txt` — SHA-256 демки на сервере и у раннера
- `timings.txt` — времена отправки, приёма, очереди и конца скачивания
- `proxy.txt` — состояние или журнал прокси без адресов
- `server-journal.txt` — журнал `cs2-server` с метками времени (сборщик хоста)
- `runner.log` — журнал раннера «после»
- `job-show-<N>.txt` — задача (сборщик хоста)
- `chat.txt`, `notes.txt`, `verdict.txt` — написаны руками
- `RESULT.md`, `token-scan.txt` — исполнитель

## Проверка

Команды исполнителя на машине автора (Git Bash, из корня репозитория):

```bash
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/05-big-demo-proxy" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/05-big-demo-proxy
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/05-big-demo-proxy --require-pass
```

Между ними исполнитель сверяет вердикт автора с фактами улик и пишет `RESULT.md` по формату README (`drill: 05-big-demo-proxy`, `result:` по уликам: `fail` при расхождении с вердиктом, при разных хешах или памяти от 204800 КиБ; `## Исход` — один абзац с размером файла, временем передачи, максимумом памяти и исходом; `## Улики` — список выше): суммы из `sha-server.txt` и `sha-runner.txt` равны, максимум `rss.txt` ниже порога (`sort -k2 -n rss.txt | tail -n 1`).

Правило приёмки: учение пройдено только когда `bash docs/drills/evidence.sh check … --require-pass` для папки `05-big-demo-proxy` проходит. Код 1 при `import` — найден секрет: папку улик не коммитят, удаляют целиком, токен отзывают, учение повторяют. `result: fail` с полными уликами — записанный провал для `/gsd-plan-phase 9 --gaps`.
