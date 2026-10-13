# 00-stand: стенд перед учениями, спайки bot-api и живая цепочка

Этот протокол проводит автор в плане 09-08. Он даёт три папки улик: `00-deploy`, `00-spikes`, `00-live-chain`. Правила, роли, формат `RESULT.md` и правило кодировки — в `docs/drills/README.md`; здесь они не повторяются, но действуют целиком.

## Цель

Развернуть `cs2-server` и локальный `telegram-bot-api` на чистой Linux-машине строго по `docs/deploy/README.md`, доказать уликами, что развёртывание переживает перезагрузку, снаружи открыты только задуманные порты, копии базы снимаются и восстанавливаются, а затем пройти отложенные живые проверки:

- `00-deploy`: критерий 2 фазы 9 (OPS-02) — развёртывание по документу, проверка хоста до и после перезагрузки, скан портов снаружи, сверка бинарника, копия и восстановление базы;
- `00-spikes`: 07-UAT тесты 2 и 3 — `sendVideo` против `sendDocument` на синтетических роликах 0,5 / 1,0 / 1,5 ГБ, права и пути общего тома bot-api, нужен ли `botapi-janitor`, растёт ли рабочая папка bot-api, виден ли токен в журнале bot-api (решения `bot_send_as` и метаданных видео);
- `00-live-chain`: 07-UAT тест 1 и 08-UAT тест 5 — демка друга через бота, сервер и приложение в режиме раннера с настоящими CS2 и HLAE доходит до ролика в чате; приложение, закрытое посреди записи и запущенное снова, продолжает задачу само.

Поиск токена по всем уликам, журналам, базам и чату (07-UAT тест 5) — отдельное учение `99-token-scan` плана 09-10. Здесь поиск идёт при каждом `import`.

## Предусловия

1. В `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/09-STAND.md` есть раздел «## Сроки хранения» со строками `сроки: …` и `учение 08: compressed` или `учение 08: real` (решение автора из плана 09-08, Task 1). Если автор назвал свои сроки, они заданы в `/etc/cs2-server/cs2-server.json` до первого запуска службы (раздел 5 `docs/deploy/README.md`).
2. Машины: чистый Linux-хост с systemd и Docker (доступ по SSH, `sudo`); машина раннера на Windows 11 с CS2 и HLAE, приложение `cs2cinema.exe` и `cs2-cli.exe` собраны исполнителем (`cargo build --release -p cli -p app`). Раннер и хост в одной сети доступа выбранного варианта (Tailscale Serve или Caddy). Вариант «репетиция в WSL» эти учения не закрывает.
3. Бинарник сервера — артефакт `cs2-server-linux-x64` зелёного прогона CI (ссылка в `evidence/00-ci/run-chain.txt`) или собранный на хосте `cargo build --release -p server`. Исполнитель сообщает автору SHA-256 артефакта до паузы.
4. Файл образцов секретов `%LOCALAPPDATA%\cs2cinema\drills\patterns.txt` лежит вне репозитория (формат — раздел «Файл образцов» README). В нём: токен тестового бота (отдельный, для этих учений; после них его отзывают в `@BotFather`), токен раннера (после `runner add`), ключ FACEIT, `api_hash`. Токены в командную строку и в историю оболочки не попадают никогда.
5. Роли в Telegram: `автор` (админ), `друг` (второй аккаунт или настоящий друг; доступ выдаёт `/adduser`), `незнакомец` (третий аккаунт без доступа, нужен учению 07). В командах, заметках и чате — только роли, без имён и Telegram-ID.
6. Скрипты на месте: `docs/drills/collect-server.sh` и файлы `docs/deploy/*` скопированы на хост (`scp -r docs/deploy docs/drills/collect-server.sh <хост>:`); `bash docs/drills/selftest.sh` на машине автора печатает `selftest: OK`.
7. Сырые папки чистые (правило повтора README): на Windows нет `%LOCALAPPDATA%\cs2cinema\drills\raw\00-deploy`, `…\00-spikes`, `…\00-live-chain`; на хосте нет `~/drill/00-*` и `~/raw/00-*`; в репозитории нет `.planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-deploy`, `…/00-spikes`, `…/00-live-chain`.
8. Учения на этом стенде идут по одному: пока идёт запись, другого учения не начинают (CS2 пишет один экземпляр).

## Шаги

### Общие приёмы (для этого и остальных протоколов)

На хосте, в начале каждого сеанса:

```bash
ctl() { sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json "$@"; }
```

Файлы, которые автор снимает руками на хосте, идут в `~/drill/<учение>/` (по одному файлу на команду, `| tee`); сборщик `docs/drills/collect-server.sh` пишет в **другую** папку `~/raw/<учение>/` (он отказывается писать в непустую). Обе папки потом копируются в одну сырую папку на Windows.

Сборщик на хосте (запускать в конце учения; `<начало>` — время начала учения вида `2026-10-20 10:00:00`, снятое командой `date '+%Y-%m-%d %H:%M:%S'` на хосте в первом шаге; `<N>` — номера задач учения):

```bash
sudo bash docs/drills/collect-server.sh ~/raw/<учение> "<начало>" <N> [<N> …]
```

На Windows, из корня репозитория, сырая папка и сбор состояния раннера (для «до» и «после» два запуска; папка `-Out` должна быть пустой, поэтому временная подпапка, из которой файлы переносятся плоско в сырую папку):

```powershell
$raw = "$env:LOCALAPPDATA\cs2cinema\drills\raw\<учение>"
New-Item -ItemType Directory -Force $raw | Out-Null
# «до» (файлы получают суффикс -before):
powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp"
Get-ChildItem "$raw\_tmp" | ForEach-Object { Move-Item $_.FullName (Join-Path $raw ($_.BaseName + '-before' + $_.Extension)) }
Remove-Item "$raw\_tmp"
# «после» (файлы остаются с обычными именами: runner.log, runner_state.json, listing.txt, processes.txt):
powershell -NoProfile -File docs\drills\collect-runner.ps1 -Out "$raw\_tmp"
Move-Item "$raw\_tmp\*" $raw
Remove-Item "$raw\_tmp"
```

Если раннер — `cs2-cli runner --dir <папка>`, к `collect-runner.ps1` добавляется `-Dir <папка>`. Копирование с хоста в ту же сырую папку:

```powershell
scp -r "<хост>:raw/<учение>/*" "<хост>:drill/<учение>/*" $raw\
```

В сырую папку автор кладёт ещё три файла, написанные руками (UTF-8): `chat.txt` — тексты бота и друга по порядку, строки вида `друг: …` и `бот: …`, без Telegram-ID и имён; `notes.txt` — всё странное и все отклонения от протокола (время и что случилось); `verdict.txt` — одна строка: `pass` или `fail` и почему.

### Часть A. Развёртывание (`00-deploy`)

Сырая папка: `…\raw\00-deploy`. Ничего не чинить «на лету»: отклонение от документа записывается в `notes.txt`, исправление документа делает исполнитель в плане 09-08.

1. Хост: `date '+%Y-%m-%d %H:%M:%S'` — время начала запомнить; `mkdir -p ~/drill/00-deploy`.
2. Развернуть всё строго по `docs/deploy/README.md` выбранного варианта, разделы 1–12 по порядку (пользователь и папки, бинарник, бот, `telegram-bot-api` в compose, настройки, служба, пользователи и раннер, доступ раннера, межсетевой экран, копии, проверка, перезагрузка). Пользователей завести ролями: `ctl user add автор --tg <Telegram-ID автора> --admin`, `ctl runner add мой-пк`; друга добавить командой бота `/adduser` от автора. Токен раннера, который печатает `runner add`, положить в файл токена на машине раннера; на экран и в заметки он не копируется.
3. Бинарник: на хосте `{ echo "# хост"; sha256sum /usr/local/bin/cs2-server; } | tee ~/drill/00-deploy/sha256.txt`; на Windows после копирования в сырую папку дописать сумму артефакта: `"# артефакт CI" | Out-File -Encoding utf8 -Append "$raw\sha256.txt"`, затем `Get-FileHash -Algorithm SHA256 <файл cs2-server-linux-x64> | Out-File -Encoding utf8 -Append "$raw\sha256.txt"` (две суммы должны совпасть буква в букву).
4. Проверка хоста до перезагрузки: `sudo bash docs/deploy/check-host.sh --access tailscale 2>&1 | tee ~/drill/00-deploy/check-host-before-reboot.txt` (`--access caddy` для варианта Б; файл скопирован на хост вместе с `docs/deploy`, запуск из его папки). Строк `ПРОБЛЕМА:` быть не должно.
5. Скан снаружи, **с другой машины** (не с хоста): `nmap -Pn -p- <публичный адрес хоста> | Out-File -Encoding utf8 "$raw\port-scan.txt"`; если `nmap` нет — `Test-NetConnection <адрес> -Port 8080` и `-Port 8081` (в обоих `TcpTestSucceeded : False`), вывод так же через `Out-File -Encoding utf8`. Открытыми могут быть только 22 (и 80, 443 для варианта Caddy).
6. Проверка с машины раннера: `curl.exe -s -o NUL -w "%{http_code}" -X POST https://<адрес из раздела 8 документа>/v1/runner/hello | Out-File -Encoding utf8 "$raw\hello-401.txt"` — ожидается `401`.
7. Копия и восстановление базы: `{ ctl backup /var/backups/cs2-server; systemctl list-timers cs2-server-backup.timer --no-pager; sudo ls -l /var/backups/cs2-server; } 2>&1 | tee ~/drill/00-deploy/backup-restore.txt`. Затем восстановление **на копии папки данных**, служба не останавливается и боевая база не трогается:

```bash
sudo install -d -m 0700 -o cs2-server -g cs2-server /var/tmp/restore-test
LAST=$(sudo ls -1t /var/backups/cs2-server/server-*.db | head -n 1)
sudo -u cs2-server cp "$LAST" /var/tmp/restore-test/server.db
sudo -u cs2-server sh -c 'sed "s#/var/lib/cs2-server#/var/tmp/restore-test#" /etc/cs2-server/cs2-server.json > /var/tmp/restore-test/cs2-server.json'
{ echo "# job list на восстановленной копии"; sudo -u cs2-server cs2-server --config /var/tmp/restore-test/cs2-server.json job list; } 2>&1 | tee -a ~/drill/00-deploy/backup-restore.txt
sudo rm -rf /var/tmp/restore-test
```

   Копию по таймеру подтверждает строка `NEXT`/`LAST` в `list-timers`; если стенд работает меньше суток, один раз запустить тот же юнит вручную: `sudo systemctl start cs2-server-backup.service` и снова `ls -l /var/backups/cs2-server` (в `notes.txt` записать, что копию сняли вручную тем же юнитом).
8. Сроки хранения в силе: `ctl config show 2>&1 | tee ~/drill/00-deploy/config-show.txt` (значения `retention_*` и `clean_interval_secs` должны совпасть с разделом «Сроки хранения» `09-STAND.md`). Токенов в выводе нет: они лежат в окружении службы.
9. Перезагрузка: `sudo systemctl reboot`; дождаться хоста; затем `sudo bash docs/deploy/check-host.sh --access tailscale 2>&1 | tee ~/drill/00-deploy/check-host-after-reboot.txt`, для варианта А ещё `tailscale serve status 2>&1 | tee ~/drill/00-deploy/serve-status.txt` (для варианта Б — `systemctl status caddy --no-pager 2>&1 | tee ~/drill/00-deploy/caddy-status.txt`). Строк `ПРОБЛЕМА:` быть не должно, `cs2-server` и контейнеры подняты сами.
10. Сбор: на хосте `sudo bash docs/drills/collect-server.sh ~/raw/00-deploy "<начало>"`; на Windows скопировать обе папки хоста в `$raw` (приём из «Общих приёмов»), положить `notes.txt` и `verdict.txt`.

### Часть B. Спайки bot-api (`00-spikes`)

Сырая папка: `…\raw\00-spikes`. На хосте `mkdir -p ~/drill/00-spikes`; время начала запомнить (`date`). Запросы к Bot API только через `curl -K` с файлом настроек: токен в командной строке запрещён.

**Файлы настроек (права 0600, удаляются после спайка).** Дважды создать файл так: `sudo install -m 0600 /dev/null /root/spike-video.cfg`, затем `sudo nano /root/spike-video.cfg` и вписать (токен и номер чата — только внутри файла; чат — переписка автора с тестовым ботом):

```text
url = "http://127.0.0.1:8081/bot<ТОКЕН ТЕСТОВОГО БОТА>/sendVideo"
form = "chat_id=<номер чата автора с тестовым ботом>"
silent
output = "/root/spike-resp.json"
write-out = "%{http_code} %{time_total}\n"
```

Второй файл — `/root/spike-doc.cfg` такой же, но `…/sendDocument`. Файл `/root/spike-file.cfg` (без секретов) собирается перед каждым запросом: `printf 'form = "%s=file://%s"\n' video "$F" | sudo tee /root/spike-file.cfg >/dev/null` (для документа вместо `video` — `document`). Запрос: `sudo curl -K /root/spike-video.cfg -K /root/spike-file.cfg`. Ответ целиком лежит в `/root/spike-resp.json` и в улики не идёт (в нём номер чата и имена): из него берут только `ok` и размеры видео командой `sudo grep -oE '"(ok|width|height|duration)":[a-z0-9]+' /root/spike-resp.json`.

1. Вид пути и права (A2, A3). Пока стенд пуст, автор отправляет боту небольшой файл `.dem` (или настоящую демку — это же начало части C), сервер принимает его; затем на хосте:

```bash
sudo find /var/lib/telegram-bot-api -type f -printf '%u %g %m %s %p\n' \
  | sed 's#/var/lib/telegram-bot-api/[^/]*/#/var/lib/telegram-bot-api/<bot>/#' | tee ~/drill/00-spikes/botapi-tree.txt
```

   В имени папки под рабочей папкой bot-api стоит токен бота, поэтому в вывод и в улики она попадает только заменённой на `<bot>`. Из вывода в `SPIKE.md` идёт вид абсолютного пути файла (`/var/lib/telegram-bot-api/<bot>/documents/…`), владелец, группа и права. Затем: может ли сервер удалять эти файлы — `F=$(sudo find /var/lib/telegram-bot-api -type f -path '*/documents/*' | head -n 1); { sudo -u cs2-server test -r "$F" && echo "читает: да" || echo "читает: нет"; sudo -u cs2-server test -w "$(dirname "$F")" && echo "удаляет: да" || echo "удаляет: нет"; } | tee ~/drill/00-spikes/server-access.txt`.
2. Удаляет ли сервер исходник после приёма. Перед отправкой файла боту и после того, как задача стала `queued`: `{ date; sudo du -sb /var/lib/telegram-bot-api; } | tee -a ~/drill/00-spikes/disk-documents.txt` — два замера и `find … -path '*/documents/*'` после приёма (в `SPIKE.md`: исходник остался или исчез).
3. Нужен ли `botapi-janitor` (A3). Принудительно состарить файл в контейнере и дождаться прохода уборщика (до 10 минут):

```bash
sudo docker compose -f /opt/cs2-bot-api/docker-compose.yml exec -T botapi-janitor sh -c \
  'for d in /var/lib/telegram-bot-api/*/documents; do touch "$d/janitor-test.bin" && touch -d "7 hours ago" "$d/janitor-test.bin"; done' 2>&1 | tee ~/drill/00-spikes/janitor-test.txt
sleep 660; sudo find /var/lib/telegram-bot-api -name janitor-test.bin | sed 's#/var/lib/telegram-bot-api/[^/]*/#/var/lib/telegram-bot-api/<bot>/#' | tee -a ~/drill/00-spikes/janitor-test.txt
```

   Пустой второй вывод — `botapi-janitor` убрал файл. Вместе с `server-access.txt` (шаг 1) это решает, нужен ли контейнер-уборщик или хватает потока уборки сервера.
4. Токен в журнале bot-api (A8), при `TELEGRAM_VERBOSITY` 1: `sudo docker compose -f /opt/cs2-bot-api/docker-compose.yml logs --no-color telegram-bot-api | grep -cE '/bot[0-9]+:' | tee ~/drill/00-spikes/token-in-botapi-log.txt` — печатается только число (ожидается `0`); значения не печатаются и не копируются. Если число не ноль — записать в `notes.txt`, значение не показывать: `evidence.sh import` остановится кодом 1 по правилу README.
5. Синтетические ролики (на хосте с `ffmpeg`: `sudo apt install ffmpeg`; либо на машине автора с последующим `scp`). Папка для спайка внутри тома, который виден контейнеру только на чтение: `sudo -u cs2-server mkdir -p /var/lib/cs2-server/blobs/results/spike`. Три файла по 0,5 / 1,0 / 1,5 ГБ (`-t` равно 200 / 400 / 600 секундам, 20 Мбит/с):

```bash
for t in 200 400 600; do
  sudo -u cs2-server ffmpeg -f lavfi -i testsrc2=size=1920x1080:rate=60 -t $t -c:v libx264 -b:v 20M -pix_fmt yuv420p -movflags +faststart /var/lib/cs2-server/blobs/results/spike/s$t.mp4
done
```

   Пока файлы лежат в `results/spike`, поток уборки сервера их не считает (в базе их нет); после спайка папка удаляется (шаг 8).
6. Замеры A12 и времени. Для каждого размера: `{ date; sudo du -sb /var/lib/telegram-bot-api; } | tee -a ~/drill/00-spikes/disk-documents.txt` до и после отправки (растёт ли рабочая папка при `sendVideo` по `file://`), затем запросы по списку, результат каждого — одна строка в `~/drill/00-spikes/spike-times.txt` (размер, метод, `код время_секунд`, `ok`/ширина/высота/длительность из ответа):
   - `sendVideo` без метаданных, каждый из трёх файлов;
   - `sendDocument`, каждый из трёх файлов;
   - только если превью или соотношение сторон `sendVideo` оказались неверными: `sendVideo` с полями `form = "width=1920"`, `form = "height=1080"`, `form = "duration=<секунд>"` (дописать в `spike-file.cfg` строки через `tee -a`).
7. Наблюдения в Telegram (руками, только автор): для каждой отправки — пришло ли сообщение, есть ли превью, верное ли соотношение сторон (не растянуто, не квадрат), воспроизводится ли, на телефоне и в Telegram Desktop. Это единственная часть спайка, которую видит только человек.
8. Решение и уборка. Автор пишет `SPIKE.md` (UTF-8, роли вместо имён) с разделами: `A2` — вид пути и раскладка папки bot-api; `A3` — владелец и права, удаляет ли сервер исходник, нужен ли `botapi-janitor`; `A8` — токен в журнале bot-api (число); `A12` — растёт ли рабочая папка; `sendVideo против sendDocument` — таблица «размер / метод / секунды / превью / соотношение сторон»; решение `bot_send_as` (`video` или `document`) и нужны ли `width`, `height`, `duration`. Затем: `sudo shred -u /root/spike-video.cfg /root/spike-doc.cfg /root/spike-file.cfg /root/spike-resp.json; sudo rm -rf /var/lib/cs2-server/blobs/results/spike`.
9. Сбор: `sudo bash docs/drills/collect-server.sh ~/raw/00-spikes "<начало>"` (журнал bot-api в `botapi-logs.txt` нужен для поиска токена); копирование в `…\raw\00-spikes`; `notes.txt`, `verdict.txt`, `SPIKE.md` туда же.

### Часть C. Живая цепочка (`00-live-chain`)

Сырая папка: `…\raw\00-live-chain`. Раннер — приложение `cs2cinema.exe` в режиме раннера (рабочая папка `%LOCALAPPDATA%\cs2cinema\runner`; адрес сервера из `09-STAND.md`, токен раннера из `runner add` вносится в настройки приложения). На хосте `mkdir -p ~/drill/00-live-chain`.

1. Хост: `date '+%Y-%m-%d %H:%M:%S'` — время начала запомнить. Приложение запущено, раннер «на связи» (в `ctl runner list` виден).
2. `друг` присылает боту настоящую демку CS2 (файл `.dem` или `.dem.zst`; можно ссылку FACEIT доигранного матча). Ожидание: бот сразу подтверждает приём и называет место в очереди; статус обновляется не чаще раза в 10 секунд. Номер задачи — `<N>` (`ctl job list`).
3. Раннер забрал задачу и дошёл до этапа «Запись» (дубль k из n). **Посреди записи** автор закрывает приложение: закрыть окно и выбрать «Выход» в меню значка в трее, затем убедиться, что процесса нет: `Get-Process cs2cinema -ErrorAction SilentlyContinue` ничего не печатает. Сразу выполнить сбор «до» (приём «до» из «Общих приёмов»).
4. Через 10–20 секунд запустить `cs2cinema.exe` снова. Дальше ничего не нажимать: задача должна продолжиться сама.
5. Дождаться ролика в чате у `друга`. Проверить его воспроизведение на телефоне и в Telegram Desktop; результат записать в `notes.txt`.
6. Сбор «после» на Windows (приём «после») и на хосте `sudo bash docs/drills/collect-server.sh ~/raw/00-live-chain "<начало>" <N>`; `ctl job show <N>` в `job-show-<N>.txt` собирается сборщиком. `chat.txt` — тексты по порядку по ролям; `notes.txt`; `verdict.txt`.

## Ожидаемый исход

- `00-deploy`: оба `check-host-*.txt` состоят из строк `OK: …`; снаружи открыты только 22 (и 80, 443 при Caddy), порты 8080 и 8081 закрыты; суммы бинарника на хосте и артефакта совпадают; в `backup-restore.txt` есть свежая копия и `job list` на восстановленной копии отвечает без ошибки; `config-show.txt` показывает сроки из `09-STAND.md`; после перезагрузки служба и контейнеры подняты сами.
- `00-spikes`: на каждый вопрос из `SPIKE.md` есть ответ с файлом улик; рабочая папка bot-api после `sendVideo` по `file://` не выросла на размер ролика (или рост назван числом); число строк с токеном в журнале bot-api — `0`; решение `bot_send_as` и метаданных видео записано.
- `00-live-chain`: бот подтвердил приём сразу; после закрытия и запуска приложения задача продолжилась этим же раннером без вмешательства; в чате один ролик, он воспроизводится; `job-show-<N>.txt` содержит строку `доставка в Telegram: sent`; сообщения друга по-русски, без локальных путей.

## Чего быть не должно

- Токена бота, токена раннера, ключа FACEIT или `api_hash` в командной строке, в заметках, в `SPIKE.md`, в журналах и уликах (поиск при `import` не должен найти ни одного совпадения); имён и Telegram-ID друзей в уликах.
- Открытого снаружи порта 8080 или 8081, строк `ПРОБЛЕМА:` в любом `check-host-*.txt`.
- Двух роликов в чате на одну задачу, второй записи после перезапуска приложения, задачи в `failed`, потерянного результата, зависшей задачи без движения.
- Папки `results/spike` или файлов `/root/spike-*` после спайка; копий базы (`*.db`) и экспорта чата в папке улик.

## Улики

Список для `RESULT.md`; файл из списка должен существовать и быть непустым. Сборщики: `docs/drills/collect-server.sh` (хост), `docs/drills/collect-runner.ps1` (машина раннера); остальное снимается командами шагов.

`00-deploy`:
- `check-host-before-reboot.txt`, `check-host-after-reboot.txt` — результат `docs/deploy/check-host.sh` до и после перезагрузки
- `port-scan.txt` — скан портов снаружи; `hello-401.txt` — ответ `hello` с машины раннера
- `sha256.txt` — суммы бинарника на хосте и артефакта CI
- `backup-restore.txt` — копия, таймер, `job list` на восстановленной копии
- `config-show.txt` — сроки хранения в силе; `serve-status.txt` или `caddy-status.txt` — доступ раннера
- `services.txt`, `server-journal.txt` — состояние служб и журнал (сборщик хоста); `notes.txt`, `verdict.txt`

`00-spikes`:
- `SPIKE.md` — ответы A2, A3, A8, A12, сравнение `sendVideo` и `sendDocument`, решение `bot_send_as`
- `botapi-tree.txt`, `server-access.txt`, `disk-documents.txt` — раскладка, права, размеры рабочей папки
- `janitor-test.txt`, `token-in-botapi-log.txt`, `spike-times.txt` — проверка уборщика, число строк с токеном, замеры
- `botapi-logs.txt` — журнал bot-api (сборщик хоста); `notes.txt`, `verdict.txt`

`00-live-chain`:
- `job-show-<N>.txt`, `server-journal.txt` — задача и журнал сервера (сборщик хоста)
- `runner.log`, `runner_state.json`, `runner_state-before.json`, `processes.txt`, `listing.txt` — журнал и состояние раннера «после» и «до» закрытия
- `chat.txt`, `notes.txt`, `verdict.txt`

## Проверка

Команды исполнителя на машине автора (Git Bash, из корня репозитория). Для каждой из трёх папок: сначала импорт сырой папки в улики, потом `RESULT.md` (формат README, `result:` по уликам, а не по `verdict.txt`: при расхождении писать `fail` и называть расхождение), потом проверка.

```bash
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/00-deploy" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-deploy
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-deploy --require-pass
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/00-spikes" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-spikes
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-spikes --require-pass
bash docs/drills/evidence.sh import "$LOCALAPPDATA/cs2cinema/drills/raw/00-live-chain" .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-live-chain
bash docs/drills/evidence.sh check .planning/phases/09-uborka-razvertyvanie-i-ucheniya/evidence/00-live-chain --require-pass
```

Сверка фактов перед `result: pass`: в `check-host-after-reboot.txt` нет `ПРОБЛЕМА:`; в `port-scan.txt` нет 8080 и 8081 среди открытых; две суммы в `sha256.txt` равны; в `job-show-<N>.txt` есть `доставка в Telegram: sent`; в `token-in-botapi-log.txt` число `0`; в `SPIKE.md` есть строка `bot_send_as`.

Правило приёмки: учение пройдено только когда для его папки проходит `bash docs/drills/evidence.sh check … --require-pass`. Код 1 при `import` — найден секрет: папку улик не коммитят, удаляют целиком, источник устраняют, токен отзывают, учение повторяют (правило повтора README). `result: fail` с полными уликами — записанный провал для `/gsd-plan-phase 9 --gaps`, а не пройденное учение.
