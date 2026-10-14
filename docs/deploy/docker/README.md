# Вариант В: всё в Docker

Один `.env` с токеном бота, одна команда `docker compose up -d --build`: из исходников репозитория собирается
`cs2-server`, рядом поднимаются локальный `telegram-bot-api` и его уборщик. HTTPS для раннера даёт Traefik, который
уже работает на хосте: compose не поднимает свой прокси, а подключает сервер к внешней docker-сети Traefik и описывает
маршрут labels. Основной документ развёртывания (`../README.md`, сервер под systemd) этим вариантом не заменяется:
берите один из двух. Оба сразу на одном хосте не работают: у них один и тот же порт `127.0.0.1:8080`.

Файлы рядом: `Dockerfile`, `docker-compose.yml`, `cs2-server.json`, `.env.example`.
В корне репозитория лежит `.dockerignore` (без него в контекст сборки попали бы `target`, демки и `.git`).
Файлы проверяет тест `crates/server/tests/deploy_docs.rs`.

## Что получится

```text
 друг --(демка или ссылка)--> Telegram --> telegram-bot-api (Docker)
                                                |  127.0.0.1:8081 (общая сеть с cs2-server)
 +--------------------- Linux-хост, docker compose --------v----------------------------+
 | cs2-server (uid 1500): бот, HTTP для раннеров :8080, поток уборки, server.db          |
 | botapi-janitor: стирает старые файлы рабочей папки bot-api                            |
 +--------------------------------------^------------------------------------------------+
                                        | внешняя docker-сеть Traefik, cs2-server:8080
 +--------------------------------------+------------------------------------------------+
 | Traefik автора (свой стек): 80/443 наружу, HTTPS, маршрут по labels                   |
 +--------------------------------------^------------------------------------------------+
                                        | HTTPS, https://<ваш-домен>
 +------------- Windows, машина автора ------------+
 | cs2cinema.exe (режим раннера) или cs2-cli runner |
 +--------------------------------------------------+
```

Наружу открыты только 22 (SSH), 80 и 443 (Traefik). Сервер (8080) и bot-api (8081) опубликованы лишь на `127.0.0.1` хоста.

## Что нужно на хосте

- x86-64 Linux, `git`, Docker и плагин Compose версии 2.20 или новее (`docker compose version`).
- Работающий Traefik (v2 или v3) с провайдером Docker, точкой входа HTTPS и certresolver; он один держит 80 и 443.
  Compose подключается к его docker-сети: узнайте её имя (`docker network ls`), имя точки входа и certresolver из
  его настроек. Без Traefik бот работает, но раннер к серверу не подключится (раздел «Проба без домена»).
- DNS-запись типа A: ваш домен указывает на публичный адрес хоста, иначе Traefik не получит сертификат.
- Бот от `@BotFather`, `api_id` и `api_hash` с `my.telegram.org` («API development tools»), как в разделе 3 основного документа.
- Rust, ffmpeg и прочее на хосте не нужны: сборка идёт внутри Docker.

## Быстрый старт

```bash
# 1. Исходники. GIT_LFS_SKIP_SMUDGE: демки в репозитории лежат в Git LFS, для сборки сервера они не нужны.
sudo GIT_LFS_SKIP_SMUDGE=1 git clone https://github.com/Kamaliev/cs2cinema.git /opt/cs2cinema
cd /opt/cs2cinema/docs/deploy/docker

# 2. Настройки: токены Telegram, домен и три значения из вашего Traefik.
sudo cp .env.example .env
sudo chmod 600 .env
sudo nano .env        # TELEGRAM_BOT_TOKEN, TELEGRAM_API_ID, TELEGRAM_API_HASH, CS2_DOMAIN,
                      # TRAEFIK_NETWORK, TRAEFIK_ENTRYPOINT (пусто = websecure), TRAEFIK_CERTRESOLVER

# 3. Один раз: бот выходит из облачного Bot API (локальный telegram-bot-api не работает с ботом,
#    который числится на сервере Telegram). Токен идёт через stdin curl, а не через командную строку.
sudo sh -c 'set -a; . ./.env; set +a; printf "url = \"https://api.telegram.org/bot%s/logOut\"\n" "$TELEGRAM_BOT_TOKEN" | curl -sS -K -'
#    ответ: {"ok":true,"result":true}; после выхода бот примерно десять минут не принимает вход обратно в облако

# 4. Сборка и запуск. Первая сборка занимает 5-10 минут (компилируется Rust), следующие быстрее.
sudo docker compose up -d --build

# 5. Проверка.
sudo docker compose ps
sudo docker compose logs -f cs2-server
```

В журнале `cs2-server` должны появиться строки «cs2-server … слушает http://0.0.0.0:8080» и «бот @… запущен через
локальный Bot API». Первая служба (`prepare`) выполняется один раз и завершается со статусом `Exited (0)`: она готовит
папки на хосте (`/var/lib/cs2-server`, `/var/lib/telegram-bot-api`, `/var/backups/cs2-server`) и отдаёт их пользователю
сервера. `docker compose ps` показывает её завершённой, это нормально.

Если пропущено значение в `.env`, Compose останавливается сразу с понятным сообщением («укажите TELEGRAM_BOT_TOKEN в файле .env»).
Если сети с именем из `TRAEFIK_NETWORK` нет, Compose тоже останавливается («network … declared as external, but could not be found»):
сеть создаёт стек Traefik, здесь её не создают.
Если ваш пользователь в группе `docker`, `sudo` перед `docker` не нужен.

## Пользователи и раннер

Служебные команды выполняются внутри контейнера. Флаг `-T` (без псевдотерминала) обязателен: `runner add`
печатает токен раннера один раз, и его не должно ловить ничто, кроме вашего экрана.

```bash
cd /opt/cs2cinema/docs/deploy/docker
sudo docker compose exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json user add автор --tg <ваш Telegram-ID> --admin
sudo docker compose exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json runner add мой-пк
```

Токен раннера положите на машину раннера в файл токена приложения cs2cinema (или в файл для `--token-file` у
`cs2-cli runner`); в командной строке он не принимается. Адрес сервера для раннера: `https://<ваш-домен>`.
Остальных друзей добавляет админ командой бота `/adduser`.

Полезно определить на время сеанса (так же называется функция в `docs/drills/00-stand.md`):

```bash
ctl() { docker compose -f /opt/cs2cinema/docs/deploy/docker/docker-compose.yml exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json "$@"; }
ctl config show
ctl job list
```

`config show` печатает токен как `***`.

## Проверка развёртывания

```bash
sudo docker compose ps                                  # cs2-server, telegram-bot-api (healthy), botapi-janitor
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8080/v1/runner/hello   # 401: сервер отвечает, нужен токен
sudo ss -tlnp | grep -E ':(80|443|8080|8081)\b'         # 80 и 443 у Traefik; 8080 и 8081 только на 127.0.0.1
sudo docker network inspect <TRAEFIK_NETWORK> --format '{{range .Containers}}{{.Name}} {{end}}'   # есть telegram-bot-api
```

Маршрут виден в панели Traefik (если она включена) как `cs2-server@docker`; в журнале Traefik не должно быть строк
про `cs2-server` с `error`.

Снаружи, с другой машины, должны быть закрыты 8080 и 8081 (раздел 11 основного документа: `nmap -Pn -p- <адрес>`,
`Test-NetConnection <адрес> -Port 8080`). С машины раннера:

```powershell
curl.exe -s -o NUL -w "%{http_code}" -X POST https://<ваш-домен>/v1/runner/hello
```

Ответ `401` значит: сервер достижим по HTTPS и требует токен. Ответ `404` значит, что Traefik не нашёл маршрут
(раздел «Если что-то не так»).

Скрипт `../check-host.sh` рассчитан на сервер под systemd и здесь неприменим как есть (он проверяет `systemctl is-active cs2-server`
и таймер копий); вместо него — команды выше.

## Как это устроено

- **Сеть.** Сервер принимает адрес bot-api только вида `http://127.0.0.1:порт` (проверка в `check_base_url`: это сделано,
  чтобы токен бота не уходил по сети). Поэтому контейнер `cs2-server` работает с `network_mode: "service:telegram-bot-api"`,
  то есть делит сеть с bot-api, и для него bot-api — это `127.0.0.1:8081`. Следствия: публикация портов обеих служб
  записана у `telegram-bot-api`, и labels для Traefik тоже у него: провайдер Docker берёт адрес контейнера-владельца сети,
  а порт `8080` в этой сети слушает `cs2-server` (`loadbalancer.server.port: 8080`). Контейнер подключён к двум сетям,
  поэтому `traefik.docker.network` называет, из какой брать адрес. В сети Traefik у него псевдоним `cs2-server`: если ваш
  Traefik настроен файлом, а не labels, адрес сервера для него `http://cs2-server:8080`.
  Сервер слушает `0.0.0.0:8080` внутри этой сети; на хосте порт виден только на `127.0.0.1:8080`.
- **Что делает прокси.** Маршрут `Host(<CS2_DOMAIN>)` на точке входа `TRAEFIK_ENTRYPOINT` с TLS от `TRAEFIK_CERTRESOLVER`
  и промежуточный слой `buffering` с пределом тела запроса 10 МБ (кусок результата — 8 МиБ, служебные запросы меньше);
  запрос и ответ до 10 МБ держатся в памяти, на диск не пишутся (ответы сервера не больше 8 МиБ: демка отдаётся кусками).
  Таймауты у Traefik общие на точку входа (`respondingTimeouts` в его статической настройке), по маршруту их не задать:
  `readTimeout` должен быть не меньше 5 минут (у Traefik v3 по умолчанию 60 секунд; кусок 8 МиБ по медленному каналу
  может не уложиться, раннер тогда повторит кусок с нового смещения), `idleTimeout` больше long-poll (25 секунд).
- **Пользователь.** Процесс сервера идёт под uid/gid 1500 (пользователь `cs2-server` в образе). Файлы данных на хосте
  принадлежат этому числу. Контейнер bot-api работает под uid 101 и читает папку результатов: каталоги 0755, файлы 0644 (umask
  контейнера 0022). Чтобы сервер читал файлы, которые кладёт bot-api, ему добавлена группа 101 (`group_add`).
- **Те же пути.** `/var/lib/cs2-server` и `/var/lib/telegram-bot-api` на хосте и в контейнерах называются одинаково, поэтому
  `bot_api_files_root` из `cs2-server.json` и пути `file://` для `sendVideo` совпадают во всех трёх местах.
- **Секреты.** `.env` читает только Compose; значения передаются в `environment:` тем службам, которым они нужны
  (токен бота — только `cs2-server`, `api_id` и `api_hash` — только bot-api). В файле настроек секретов нет, поэтому его
  разрешено хранить в git.
- **Защита контейнера.** Для `cs2-server` включены `no-new-privileges`, `cap_drop: ALL`, корневая файловая система только для чтения
  (`read_only`) с записью в смонтированные папки и в `/tmp` (tmpfs). Если после обновления сервер жалуется на запись
  в незнакомое место, строку `read_only: true` можно временно убрать и сообщить, что именно он писал.
- **Журналы.** Хранятся в Docker (`json-file`): `cs2-server` — до 5 файлов по 100 МБ, остальные — 3 по 10 МБ.

## Настройки и сроки хранения

`cs2-server.json` (рядом с compose) монтируется в контейнер на чтение. В нём те же ключи, что в
`../cs2-server.json` (сроки хранения по умолчанию, решение «A1» в паспорте стенда). Менять можно здесь или переменными
`CS2_SERVER_RETENTION_...` в `environment:` службы. Применить:

```bash
sudo docker compose up -d --force-recreate cs2-server
```

Именно пересоздание, а не `restart`: многие редакторы сохраняют файл новым файлом, и контейнер с отдельным файлом-монтированием
продолжит видеть старый.

Права файла настроек. Сервер при чтении файла предупреждает («доступен группе или всем: выполните chmod 600»), если права шире 0600.
В файле нет секретов, поэтому обычные 0644 безопасны, а предупреждение в журнале можно игнорировать. Если предупреждение мешает:
`sudo chown 1500:1500 cs2-server.json && sudo chmod 600 cs2-server.json` (после этого редактировать файл придётся через `sudo`).
Нельзя оставить 0600 за другим владельцем: процесс под uid 1500 тогда не сможет прочитать файл («не удалось открыть (PermissionDenied)»).

## Резервные копии

Папка копий на хосте: `/var/backups/cs2-server` (создаётся службой `prepare`, права 0700, файлы 0600, владелец uid 1500).
Разовая копия:

```bash
cd /opt/cs2cinema/docs/deploy/docker
sudo docker compose exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json backup /var/backups/cs2-server --keep 7
```

Раз в сутки вместо таймера systemd (файл `/etc/cron.d/cs2-server-backup`, права 0644, владелец root):

```text
17 4 * * * root cd /opt/cs2cinema/docs/deploy/docker && docker compose exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json backup /var/backups/cs2-server --keep 7 >> /var/log/cs2-server-backup.log 2>&1
```

Копия снимается `VACUUM INTO` и безопасна, пока сервер работает. Вручную базу не копируйте.
Восстановление:

```bash
cd /opt/cs2cinema/docs/deploy/docker
sudo docker compose stop cs2-server
sudo cp /var/backups/cs2-server/server-ГГГГММДД-ЧЧММСС-ммм.db /var/lib/cs2-server/server.db
sudo rm -f /var/lib/cs2-server/server.db-wal /var/lib/cs2-server/server.db-shm
sudo chown 1500:1500 /var/lib/cs2-server/server.db
sudo docker compose start cs2-server
```

## Повседневная работа

| Что | Команда (из `/opt/cs2cinema/docs/deploy/docker`) |
|-----|--------------------------------------------------|
| состояние | `sudo docker compose ps` |
| журнал сервера (вместо `journalctl -u cs2-server`) | `sudo docker compose logs -t cs2-server` |
| журнал вживую, последние 100 строк | `sudo docker compose logs -f -t --tail 100 cs2-server` |
| журнал bot-api | `sudo docker compose logs -t telegram-bot-api` |
| журнал прокси | в стеке Traefik: `docker logs -t <контейнер traefik>` (журнал доступа — если включён у вас) |
| перезапуск сервера | `sudo docker compose restart cs2-server` |
| остановить всё (данные остаются) | `sudo docker compose stop` |
| снова запустить | `sudo docker compose up -d` |
| обновление | `git pull && sudo docker compose up -d --build` |
| место на диске | `df -h /var/lib/cs2-server /var/lib/telegram-bot-api` |

Перед обновлением снимите копию базы (раздел «Резервные копии»). Схема базы только дополняется, поэтому откат за границу
миграции требует восстановления копии (как в разделе 13 основного документа). Откат кода: `git checkout <прежний коммит>`
и `sudo docker compose up -d --build`.

Автозапуск после перезагрузки: у всех служб `restart: unless-stopped`, нужна включённая служба Docker
(`sudo systemctl enable docker`). Если сервер стартует раньше готового bot-api, он завершается с ошибкой, и Docker запускает
его снова; кроме того, `depends_on` ждёт, пока bot-api не станет `healthy`.

## Межсетевой экран

```bash
sudo ufw default deny incoming
sudo ufw allow OpenSSH
sudo ufw allow 80,443/tcp
sudo ufw enable
```

Порты 80 и 443 держит Traefik из своего стека; этот compose наружу не публикует ничего.
Docker обходит `ufw` для опубликованных портов (он пишет собственные правила). Поэтому защита держится не на `ufw`, а на
привязке `127.0.0.1:...` в `docker-compose.yml`: все порты здесь опубликованы только на loopback.
Не добавляйте в compose записи вида `8080:8080`: сервер оказался бы в интернете без прокси и таймаутов (раздел 14 основного документа).

## Учения (docs/drills) в этом варианте

Протоколы учений написаны под systemd. Соответствие:

| В протоколе | Здесь |
|-------------|-------|
| `sudo -u cs2-server cs2-server --config … <команда>`, `ctl` | `docker compose exec -T cs2-server cs2-server --config /etc/cs2-server/cs2-server.json <команда>` (функция `ctl` выше) |
| `journalctl -u cs2-server -o short-iso` | `docker compose logs -t --no-color --since "<начало>" cs2-server` (метки времени `-t` обязательны) |
| `sudo systemctl restart cs2-server` (учение 03, часть 1) | `docker compose restart cs2-server` |
| `sudo kill -9 "$(systemctl show -p MainPID --value cs2-server)"` (учение 03, часть 2) | `docker kill -s KILL cs2-server` |
| «служба поднимется через пять секунд» (`Restart=on-failure`, `RestartSec=5`) | Docker перезапускает убитый контейнер почти сразу; в `notes.txt` запишите фактическое время по меткам журнала |
| `systemctl show -p Restart cs2-server` | `docker inspect --format '{{.HostConfig.RestartPolicy.Name}}' cs2-server` (ожидается `unless-stopped`) |
| `ps -o rss= -p "$(systemctl show -p MainPID --value cs2-server)"` (учение 05) | `docker stats --no-stream cs2-server`; в улики запишите, каким способом снято |
| `sudo ls -l /var/lib/cs2-server/blobs/results/<N>/` | то же: папка на хосте та же |
| `journalctl` для Caddy (учение 05) | журнал Traefik из его стека: `docker logs -t --since "<начало>" <контейнер traefik>`; адреса заменить, как в протоколе |

`docs/drills/collect-server.sh` в этом варианте не работает (он читает `journalctl`, `systemctl` и пути вида `/opt/cs2-bot-api`).
Улики хоста снимайте руками в `~/raw/<учение>/`:

```bash
cd /opt/cs2cinema/docs/deploy/docker
sudo docker compose logs -t --no-color --since "<начало>" cs2-server > ~/raw/<учение>/server-journal.txt
sudo docker compose logs -t --no-color --since "<начало>" telegram-bot-api > ~/raw/<учение>/botapi-logs.txt
sudo docker compose ps > ~/raw/<учение>/services.txt
ctl job list > ~/raw/<учение>/job-list.txt
ctl job show <N> > ~/raw/<учение>/job-show-<N>.txt
```

Для учения 08 (сутки или три часа) размер журнала контейнера выбран с запасом (100 МБ × 5 файлов); если журнал вырос больше,
увеличьте `max-size` у `cs2-server` в `docker-compose.yml` и запишите значение в `notes.txt`.

## Проба без домена

Сервер и бот поднимаются и без Traefik, но `.env` всё равно должен быть заполнен целиком (compose проверяет значения
и ищет сеть `TRAEFIK_NETWORK` при любом запуске). Если Traefik ещё нет, создайте пустую сеть с тем же именем
(`sudo docker network create <TRAEFIK_NETWORK>`) и запустите как обычно: labels без Traefik ничего не делают.
Бот работает полностью. Раннер ходит на сервер только по HTTPS, кроме адресов
этого же компьютера и частной сети; порт 8080 опубликован лишь на loopback хоста. Поэтому без домена раннер подключается
через SSH-туннель с машины раннера: `ssh -N -L 8080:127.0.0.1:8080 <пользователь>@<хост>`, адрес сервера для раннера
в этом случае `http://127.0.0.1:8080` (открытый `http://` на localhost раннер принимает). Для настоящей работы нужны Traefik и домен.

## Секреты: чего не делать

- Токен бота, `api_hash` и токен раннера не набирать в командной строке и не слать в чат. `.env` — права 0600, в git не коммитится
  (`docs/deploy/docker/.env` в `.gitignore`).
- Не печатать `docker compose config` и `docker inspect cs2-server` в чат и в улики: они показывают значения переменных,
  включая токен бота. Если токен всё же попал в чат или журнал, перевыпустите его в BotFather и обновите `.env`.
- Не повышать `TELEGRAM_VERBOSITY` выше 1: при подробном журнале bot-api в нём появляются адреса запросов с токеном бота.
- Не открывать 8080 и 8081 наружу ни напрямую, ни переадресацией.

## Если что-то не так

| Симптом | Что смотреть |
|---------|--------------|
| Compose ругается «укажите … в файле .env» | заполнить значение в `.env`; файл должен лежать рядом с `docker-compose.yml` |
| `telegram-bot-api` остаётся `unhealthy`, сервер не стартует | `docker compose logs telegram-bot-api`: чаще всего неверные `api_id`/`api_hash`. Если в журнале всё спокойно, а проверка готовности красная, временно замените в `depends_on` у `cs2-server` `condition: service_healthy` на `service_started`: сервер сам перезапускается, пока bot-api не ответит |
| «бот не запустился: локальный telegram-bot-api не отвечает (getMe)» | не выполнен шаг 3 (выход из облачного Bot API) или не прошло десять минут; либо bot-api ещё не поднялся (сервер перезапустится сам) |
| «файл настроек … не удалось открыть (PermissionDenied)» | файл `cs2-server.json` с правами 0600 у другого владельца: `chmod 644` или `chown 1500:1500` |
| «Permission denied» при записи в `/var/lib/cs2-server` | каталог остался от установки под systemd и принадлежит другому uid: `sudo chown -R 1500:1500 /var/lib/cs2-server` |
| Compose: «network … declared as external, but could not be found» | имя в `TRAEFIK_NETWORK` не совпадает с сетью Traefik (`docker network ls`), либо стек Traefik не запущен |
| с машины раннера `404` вместо `401` | Traefik не видит маршрут: `telegram-bot-api` не в сети Traefik (`docker network inspect`), `traefik.docker.network` не та сеть, неверное имя точки входа, или у Traefik провайдер Docker выключен; журнал Traefik |
| «Gateway Timeout» или `502` с машины раннера | Traefik подключён к другой сети, чем в `traefik.docker.network`; либо `cs2-server` не запущен (`docker compose ps`) |
| сертификат не выдаётся | DNS-запись A домена, порты 80/443 у Traefik, имя `TRAEFIK_CERTRESOLVER` как в его настройке; журнал Traefik |
| раннер жалуется на обрыв загрузки куска | `readTimeout` точки входа Traefik меньше 5 минут («Как это устроено», «Что делает прокси») |
| сервер не читает файл, скачанный bot-api | права файла в `/var/lib/telegram-bot-api/<токен>/documents`: нужен доступ на чтение для группы 101 или всем |
