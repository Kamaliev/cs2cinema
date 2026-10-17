# Развёртывание cs2-server на Linux-хосте

Этот документ ведёт от чистой Linux-машины к работающей цепочке «друг в Telegram, сервер, раннер на Windows».
Все файлы, о которых идёт речь, лежат рядом: `docker-compose.yml`, `cs2-server.service`,
`cs2-server-backup.service`, `cs2-server-backup.timer`, `cs2-server.json`, `check-host.sh`.
Заготовки проверяются тестом `crates/server/tests/deploy_docs.rs`, поэтому не расходятся с кодом сервера.

## Что получится

```text
 друг --(демка или ссылка)--> Telegram --> telegram-bot-api (Docker, 127.0.0.1:8081)
                                               |
 +------------------- Linux-хост ---------------v------------------------------------+
 | cs2-server (systemd, 127.0.0.1:8080)                                              |
 |   бот: приём демок, статусы, отправка роликов (sendVideo по file://)               |
 |   HTTP для раннеров: claim, progress, загрузка результата кусками                  |
 |   поток уборки: сроки хранения демок, роликов, недозагруженных кусков, папки bot-api |
 |   server.db (WAL) <-- резервная копия раз в сутки (таймер systemd)                 |
 +-----------------------------------^-----------------------------------------------+
                                     | HTTPS: Tailscale Serve (основной вариант) или Caddy с доменом
 +----------------- Windows, машина автора -------------------------------------------+
 | cs2cinema.exe (режим раннера) или cs2-cli runner: забирает задачу, пишет ролик в CS2 |
 +------------------------------------------------------------------------------------+
```

Снаружи открыт только SSH (и порты 80 и 443 при варианте с Caddy). Сервер и bot-api слушают только loopback.
Раннер не отправляет токен по открытому `http://` на внешний адрес, поэтому доступ раннера всегда идёт по HTTPS.

## Требования к хосту

- x86-64 Linux с systemd, Docker и плагином `docker compose` (версия 2).
- Место на диске: до 2 ГБ на каждую задачу в очереди (демка и ролик). Сроки хранения и уборка — раздел 15.
- Бинарник `cs2-server` берётся из артефакта CI и собран на `ubuntu-latest`. Если хост старше, при запуске
  появится «version GLIBC_2.xx not found». Проверьте заранее: `ldd --version` на хосте не должна быть старее версии
  на образе сборки CI. Если старее, соберите на самом хосте: `cargo build --release -p server`
  (понадобятся Rust и компилятор C), готовый файл — `target/release/cs2-server`.
- Установленный Tailscale (вариант А раздела 8) или домен с DNS-записью на хост (вариант Б).

Служебные команды оператора выполняются на хосте так (без токена: он есть только у службы):

```bash
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json <команда>
```

## 1. Пользователь и папки

```bash
sudo useradd --system --home-dir /var/lib/cs2-server --shell /usr/sbin/nologin cs2-server
sudo install -d -m 0750 -o root -g cs2-server /etc/cs2-server
sudo install -d -m 0700 -o cs2-server -g cs2-server /var/backups/cs2-server
sudo install -d -m 0755 /opt/cs2-bot-api
```

Пользователь обычный системный, а не динамический: каталог данных должен быть читаем контейнером bot-api
(uid 101), чтобы тот отправлял ролики. Каталог `/var/lib/cs2-server` создаёт сама служба (`StateDirectory`).
Рабочая папка bot-api `/var/lib/telegram-bot-api` создаётся при первом запуске контейнера (раздел 4).

## 2. Бинарник

1. Возьмите артефакт `cs2-server-linux-x64` из зелёного прогона CI на GitHub (вкладка Actions, нужный коммит).
2. Запишите контрольную сумму: `sha256sum cs2-server-linux-x64` на машине, где скачали, и снова на хосте после
   копирования (`scp`). Суммы должны совпасть буква в букву, иначе файл подменён или повреждён.
3. Установите:

```bash
sudo install -m 0755 cs2-server-linux-x64 /usr/local/bin/cs2-server
cs2-server --help | head -n 5
```

При обновлении предыдущий файл сохраняется как `/usr/local/bin/cs2-server.prev` (раздел 13).

## 3. Бот Telegram

1. В `@BotFather` создайте бота командой `/newbot` и получите токен. Токен — секрет: не пересылайте его, не вставляйте
   в чат, не набирайте в командной строке.
2. На `my.telegram.org` откройте «API development tools» и получите `api_id` и `api_hash` (нужны только bot-api).
3. Локальный `telegram-bot-api` не работает с ботом, который числится на облачном сервере Telegram.
   Один раз выйдите ботом из облачного Bot API. Токен при этом не попадает в командную строку:

```bash
sudo install -m 0600 /dev/null /root/logout.cfg
sudo nano /root/logout.cfg      # одна строка:  url = "https://api.telegram.org/bot<ТОКЕН>/logOut"
sudo curl -K /root/logout.cfg   # ответ: {"ok":true,"result":true}
sudo shred -u /root/logout.cfg
```

После выхода бот примерно десять минут не принимает вход обратно в облако; это нормально.

## 4. telegram-bot-api

```bash
sudo cp docker-compose.yml /opt/cs2-bot-api/docker-compose.yml
sudo install -m 0600 /dev/null /opt/cs2-bot-api/.env
sudo nano /opt/cs2-bot-api/.env   # две строки: TELEGRAM_API_ID=...  и  TELEGRAM_API_HASH=...
cd /opt/cs2-bot-api && sudo docker compose up -d
```

Проверка: `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8081/` печатает любой HTTP-код, кроме `000`.

Закрепите образы дайджестом, чтобы обновление образа не прилетало незаметно:

```bash
sudo docker inspect --format '{{index .RepoDigests 0}}' aiogram/telegram-bot-api:latest
sudo docker inspect --format '{{index .RepoDigests 0}}' alpine:3.20
```

Подставьте полученные строки вида `имя@sha256:...` в поле `image:` обеих служб и выполните `docker compose up -d`.

Если контейнер не стартует и в журнале (`docker compose logs telegram-bot-api`) жалоба на рабочую или временную
папку, добавьте в `environment:` переменные `TELEGRAM_WORK_DIR: /var/lib/telegram-bot-api` и
`TELEGRAM_TEMP_DIR: /tmp/telegram-bot-api`.

Порт опубликован только на `127.0.0.1`: Docker пишет собственные правила межсетевого экрана в обход `ufw`, и
запись вида `8081:8081` открыла бы bot-api в интернет (раздел 9).

## 5. Настройки сервера

```bash
sudo install -m 0600 -o cs2-server -g cs2-server cs2-server.json /etc/cs2-server/cs2-server.json
sudo install -m 0600 /dev/null /etc/cs2-server/env
sudo nano /etc/cs2-server/env
```

В `/etc/cs2-server/env` две строки (права 0600, владелец root:root):

```text
TELEGRAM_BOT_TOKEN=<токен от BotFather>
CS2_SERVER_BOT_API_URL=http://127.0.0.1:8081
```

Файл настроек `/etc/cs2-server/cs2-server.json` секретов и адреса bot-api не содержит. Поэтому служебные команды
(`job show`, `backup`, `clean`, `user add`, `runner add`) работают без токена: загрузчик настроек требует токен
только вместе с адресом bot-api. Адрес bot-api вынесен в окружение службы по той же причине. Права файла настроек
0600: при более широких загрузчик печатает предупреждение.

Экран настроек ролика (`/settings`, Telegram Mini App) открывается только по https: чтобы в чате появилась кнопка,
задайте `public_url` (в файле настроек или `CS2_SERVER_PUBLIC_URL=https://cs2.example.com`) — публичный адрес, который
прокси (вариант Б раздела 8) отдаёт на порт сервера. Нужны адреса `/app/settings` и `/app/api/settings`; вход по подписи
Telegram, токен раннера для них не нужен. Без `public_url` `/settings` покажет текущие значения текстом.

Остальные ключи (`bot_send_as`, лимиты очереди, сроки хранения) описаны в `config show` и в образце
`crates/server/cs2-server.example.json`.

## 6. Служба

```bash
sudo cp cs2-server.service /etc/systemd/system/cs2-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now cs2-server
sudo journalctl -u cs2-server -f
```

В журнале должны появиться строки «слушает http://127.0.0.1:8080» и «бот @… запущен через локальный Bot API».
Если после перезагрузки сервер стартует раньше контейнера bot-api, он завершается, а systemd запускает его снова
каждые 5 секунд без ограничения числа попыток.

## 7. Пользователи и раннер

```bash
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json user add автор --tg <ваш Telegram-ID> --admin
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json runner add мой-пк
```

Вторая команда печатает токен раннера. Он показывается один раз, на сервере хранится только его хеш. Токен
положите на машину раннера в файл токена приложения cs2cinema или в файл для `--token-file` у `cs2-cli runner`.
В командной строке токен не принимается. Остальных друзей добавляет админ командой бота `/adduser`.

Раннер можно подключить и без консоли сервера. В приложении cs2cinema (Настройки → «Задачи с сервера», адрес
сервера уже сохранён) нажмите «Подключить через Telegram»: приложение само придумает токен, сохранит его у себя и
покажет шестизначный код. Отправьте боту `/runner код` (только админ; `/runner код имя` — назвать раннера иначе),
и приложение выйдет на связь само. Серверу при этом уходит только хеш токена (`POST /v1/runner/pair`, без
токена в заголовке); код действует 10 минут. Список раннеров — `/runners`, отзыв токена — `/runner revoke имя`.

## 8. Доступ раннера

Раннер ходит на сервер по HTTPS. Открытый `http://` он принимает только для localhost и частных сетей;
адреса Tailscale в частные не входят (см. `plain_http_allowed` в `crates/runner/src/link.rs`). Поэтому адрес вида
«открытый http на адрес узла в Tailscale» раннер отвергнет с ошибкой «токен нельзя слать по открытому http:// на
внешний адрес». Ослаблять эту проверку не нужно: нужен HTTPS.

### Вариант А: Tailscale Serve (основной)

Публичных портов, кроме SSH, не появляется. Сервер доступен только участникам вашего tailnet.

1. Установите Tailscale на хост и войдите: `sudo tailscale up`.
2. В админ-консоли Tailscale (вкладка DNS) включите MagicDNS и HTTPS-сертификаты.
3. Опубликуйте сервер внутри tailnet:

```bash
sudo tailscale serve --bg --https=443 http://127.0.0.1:8080
sudo tailscale serve status
```

4. Адрес для раннера: `https://<узел>.<tailnet>.ts.net` (точное имя показывает `tailscale serve status`).
5. Правило экрана: `sudo ufw allow in on tailscale0 to any port 443 proto tcp`.
6. Машина раннера тоже должна быть в этом tailnet.

### Вариант Б: Caddy с доменом (запасной)

1. Заведите DNS-запись типа A с доменом на публичный адрес хоста. Установите Caddy по инструкции дистрибутива.
2. `/etc/caddy/Caddyfile`:

```text
{
	servers {
		timeouts {
			read_header 10s
			read_body 5m
			write 5m
			idle 2m
		}
	}
}

<ваш-домен> {
	request_body {
		max_size 10MB
	}
	reverse_proxy 127.0.0.1:8080 {
		flush_interval -1
	}
}
```

3. Откройте только веб-порты: `sudo ufw allow 80,443/tcp`. Затем `sudo systemctl reload caddy`.
4. Адрес для раннера: `https://<ваш-домен>`. Сертификат Caddy получает сам.

Предел тела запроса 10 МБ с запасом покрывает кусок результата (8 МиБ) и служебные запросы.

### Вариант В: всё в Docker

Сервер и bot-api можно поднять одним `docker compose up -d --build` из исходников репозитория, без бинарника из CI
и без systemd: нужны только `git`, Docker с Compose, домен и уже работающий на хосте Traefik (compose подключает сервер
к его docker-сети и описывает маршрут labels; свой прокси не поднимает). Это отдельный путь вместо разделов 1, 2, 4-6 и 10
(одновременно с ними не использовать). Файлы и порядок действий — в `docker/README.md`.

## 9. Межсетевой экран

```bash
sudo ufw default deny incoming
sudo ufw allow OpenSSH
# затем правила выбранного варианта раздела 8
sudo ufw enable
sudo ufw status verbose
```

Порт сервера (8080) и порт bot-api (8081) в правилах не появляются никогда: оба слушают только loopback.
Docker обходит `ufw` для опубликованных портов, поэтому защита bot-api держится не на `ufw`, а на привязке
`127.0.0.1:8081:8081` в `docker-compose.yml`. Проверка — раздел 11.

## 10. Резервные копии и восстановление

```bash
sudo cp cs2-server-backup.service cs2-server-backup.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now cs2-server-backup.timer
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json backup /var/backups/cs2-server
```

Таймер запускает копию раз в сутки, хранятся 14 последних (`--keep N` меняет число). Копия снимается
`VACUUM INTO`, она целая и безопасна, пока сервер работает. Файл базы не копируйте вручную обычным копированием:
при записи в режиме WAL такая копия может оказаться порванной. Папка копий 0700, файлы 0600: в них хеши токенов
раннеров и Telegram-ID.

Восстановление:

```bash
sudo systemctl stop cs2-server
sudo cp /var/backups/cs2-server/server-ГГГГММДД-ЧЧММСС-ммм.db /var/lib/cs2-server/server.db
sudo rm -f /var/lib/cs2-server/server.db-wal /var/lib/cs2-server/server.db-shm
sudo chown cs2-server:cs2-server /var/lib/cs2-server/server.db
sudo systemctl start cs2-server
```

Демки и ролики в копию не входят: демки воспроизводимы, а ролики уходят в Telegram и убираются по срокам.

## 11. Проверка развёртывания

На хосте:

```bash
sudo bash check-host.sh --access tailscale     # или --access caddy
sudo ss -tlnp
```

Скрипт печатает «OK: …» или «ПРОБЛЕМА: …» и возвращает 0 только когда проблем нет. Он проверяет службы, контейнеры,
слушающие порты, ответ сервера (401 без токена), свежесть копии и права файлов секретов.

Снаружи, с другой машины, должны быть закрыты порты 8080 и 8081:

```bash
nmap -Pn -p- <публичный адрес хоста>
```

В списке открытых портов только 22 (и 80 с 443 при Caddy). Из PowerShell на Windows:

```powershell
Test-NetConnection <публичный адрес> -Port 8080
Test-NetConnection <публичный адрес> -Port 8081
```

В обоих случаях ожидается `TcpTestSucceeded : False`.

С машины раннера:

```powershell
curl.exe -s -o NUL -w "%{http_code}" -X POST https://<адрес из раздела 8>/v1/runner/hello
```

Ответ `401` означает: сервер достижим по HTTPS и требует токен.

## 12. Перезагрузка

Развёртывание считается готовым, когда оно переживает перезагрузку:

```bash
sudo systemctl reboot
```

После возвращения хоста выполните `sudo bash check-host.sh --access tailscale` (или `caddy`) и, для варианта А,
`tailscale serve status`. Контейнеры стартуют сами (`restart: unless-stopped`), сервер поднимается после
них с повторными попытками.

## 13. Обновление и откат

```bash
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json backup /var/backups/cs2-server
sudo cp /usr/local/bin/cs2-server /usr/local/bin/cs2-server.prev
sudo install -m 0755 cs2-server-linux-x64 /usr/local/bin/cs2-server
sudo systemctl restart cs2-server
sudo journalctl -u cs2-server -n 30
```

Перед заменой снимите копию базы и сверьте контрольную сумму нового файла (раздел 2). Откат:

```bash
sudo install -m 0755 /usr/local/bin/cs2-server.prev /usr/local/bin/cs2-server
sudo systemctl restart cs2-server
```

Схема базы только дополняется. База новой версии не открывается старым бинарником («база версии N новее этой
сборки»): при откате за границу миграции восстановите копию, снятую перед обновлением (раздел 10).

## 14. Медленные клиенты (WR-03 фазы 5)

Сервер держит пул обработчиков, и клиент, который открыл соединение и молчит, занимает обработчик. Решение
документом, без правки кода:

- сервер слушает только `127.0.0.1:8080`; из интернета до него не дойти;
- при Tailscale Serve к нему ходят только участники tailnet (вы сами);
- при Caddy действуют таймауты `read_header 10s`, `read_body 5m`, `write 5m`, `idle 2m` и предел тела
  запроса 10 МБ (кусок результата — 8 МиБ), см. Caddyfile в разделе 8.

Поэтому открывать порт сервера наружу напрямую нельзя: ни прокси, ни таймаутов перед ним не будет.

## 15. Место на диске и сроки хранения

Сервер сам убирает файлы раз в `clean_interval_secs`. Сроки меняются в `/etc/cs2-server/cs2-server.json`
(или переменными `CS2_SERVER_RETENTION_...` в `/etc/cs2-server/env`) и применяются после `systemctl restart cs2-server`.

| Ключ | По умолчанию | Что убирается |
|------|--------------|---------------|
| `retention_demo_hours` | 24 | демка готовой или отменённой задачи, часов после окончания |
| `retention_failed_demo_hours` | 72 | демка упавшей задачи (запас на `/retry`) |
| `retention_result_hours` | 24 | ролик, доставленный в Telegram |
| `retention_unsent_result_hours` | 168 | ролик, который не ушёл в Telegram или пришёл не из бота |
| `retention_part_hours` | 6 | недозагруженные куски `.part` и остатки прерванной уборки |
| `retention_botapi_hours` | 6 | файлы в `documents` и `videos` рабочей папки bot-api |
| `clean_interval_secs` | 600 | период прохода уборщика, секунд |

Ролик, у которого доставка ещё не закончилась, не удаляется никогда, как и демка задачи в очереди.

Посмотреть, что убрал бы проход, ничего не удаляя, и запустить его вручную:

```bash
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json clean --dry-run
sudo -u cs2-server cs2-server --config /etc/cs2-server/cs2-server.json clean
```

Служебная команда не знает адреса bot-api (он лежит в окружении службы), поэтому рабочую папку bot-api она не
считает: ту чистят поток уборки сервера и контейнер `botapi-janitor`. Занятое место: `df -h /var/lib/cs2-server /var/lib/telegram-bot-api`
(скрипт `check-host.sh` печатает то же в конце).

## 16. Секреты: где лежат и чего не делать

| Секрет | Где живёт | Права |
|--------|-----------|-------|
| токен бота, адрес bot-api | `/etc/cs2-server/env` | 0600 root:root |
| `TELEGRAM_API_ID`, `TELEGRAM_API_HASH` | `/opt/cs2-bot-api/.env` | 0600 |
| токен раннера | на машине раннера, в файле токена | только владелец |
| копии базы (хеши токенов, Telegram-ID) | `/var/backups/cs2-server` | папка 0700, файлы 0600 |

Чего не делать:

- не набирать токен бота, токен раннера и `api_hash` в командной строке: аргументы видны в списке процессов и
  попадают в историю оболочки (для `curl` служит ключ `-K` с файлом, раздел 3);
- не слать токены в чат и не коммитить файлы с ними в репозиторий;
- не повышать `TELEGRAM_VERBOSITY` выше 1: при подробном журнале bot-api в нём появляются адреса запросов с
  токеном бота. Если токен всё же попал в журнал (`docker compose logs telegram-bot-api`), перевыпустите его
  в BotFather;
- не открывать порты 8080 и 8081 наружу ни напрямую, ни через переадресацию.
