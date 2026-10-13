#!/usr/bin/env bash
# Сбор улик учения на Linux-хосте в «сырую» папку.
#
#   sudo bash collect-server.sh <папка> "<с-какого-времени>" [номер-задачи...]
#
# Пример:  sudo bash collect-server.sh ~/raw/01-kill-runner "2026-10-20 10:00:00" 12 13
#
# Что кладёт в папку (права 0700):
#   server-journal.txt  журнал cs2-server (journalctl -u cs2-server)
#   botapi-logs.txt     журнал контейнеров bot-api (docker compose logs)
#   services.txt        состояние службы, таймера копий и контейнеров
#   job-list.txt        список задач (служебная команда cs2-server job list)
#   job-show-N.txt      подробности задач (job show N)
#   results-N.txt       содержимое папки результата задачи и SHA-256 ролика
#   disk.txt            размер папок blobs и bot-api, df -h
#   raw-db/             копия базы сервера (только для поиска токена, в репозиторий не идёт)
#
# Файлы с секретами (/etc/cs2-server/env, .env) скрипт не читает и не копирует.
# Ошибка одной команды не останавливает сбор: код возврата записывается в её файл.
set -u

CONFIG=/etc/cs2-server/cs2-server.json
COMPOSE=/opt/cs2-bot-api/docker-compose.yml
BLOBS=/var/lib/cs2-server/blobs
BOTAPI_DIR=/var/lib/telegram-bot-api

if [ "$#" -lt 2 ]; then
  echo 'использование: sudo bash collect-server.sh <папка> "<с-какого-времени>" [номер-задачи...]' >&2
  exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
  echo "collect-server: запустите через sudo (нужны журнал службы, docker и папки службы)" >&2
  exit 2
fi

OUT=${1%/}
SINCE=$2
shift 2
JOBS=("$@")
for n in "${JOBS[@]+"${JOBS[@]}"}"; do
  case $n in
    '' | *[!0-9]*)
      echo "collect-server: номер задачи должен быть числом: $n" >&2
      exit 2
      ;;
  esac
done

# Правило повтора: в непустую папку не собираем, чтобы прогоны не смешивались.
if [ -d "$OUT" ] && [ -n "$(ls -A "$OUT" 2>/dev/null)" ]; then
  echo "collect-server: папка не пуста; удалите её и соберите заново: $OUT" >&2
  exit 2
fi
install -d -m 0700 "$OUT"

ctl() { sudo -u cs2-server cs2-server --config "$CONFIG" "$@"; }

# grab <файл> <команда...>: вывод команды (и ошибки) в файл, в конце — код возврата.
grab() {
  local file=$1
  shift
  {
    echo "# $*"
    "$@"
    echo "# код возврата: $?"
  } >"$OUT/$file" 2>&1
}

grab server-journal.txt journalctl -u cs2-server --since "$SINCE" -o short-iso --no-pager
grab botapi-logs.txt docker compose -f "$COMPOSE" logs --since "$SINCE" --no-color

{
  echo "# systemctl status cs2-server"
  systemctl status cs2-server --no-pager 2>&1
  echo "# systemctl list-timers cs2-server-backup.timer"
  systemctl list-timers cs2-server-backup.timer --no-pager 2>&1
  echo "# docker compose ps"
  docker compose -f "$COMPOSE" ps 2>&1
} >"$OUT/services.txt" 2>&1

grab job-list.txt ctl job list
for n in "${JOBS[@]+"${JOBS[@]}"}"; do
  grab "job-show-$n.txt" ctl job show "$n"
  {
    echo "# ls -l $BLOBS/results/$n"
    ls -l "$BLOBS/results/$n" 2>&1
    if [ -f "$BLOBS/results/$n/highlights.mp4" ]; then
      echo "# sha256sum highlights.mp4"
      sha256sum "$BLOBS/results/$n/highlights.mp4" 2>&1
    else
      echo "# ролика highlights.mp4 нет"
    fi
  } >"$OUT/results-$n.txt" 2>&1
done

{
  echo "# du -sb $BLOBS $BOTAPI_DIR"
  du -sb "$BLOBS" "$BOTAPI_DIR" 2>&1
  echo "# df -h"
  df -h 2>&1
} >"$OUT/disk.txt" 2>&1

# Копия базы: служебная команда backup (VACUUM INTO, безопасна при работающем сервере). Пишет от имени cs2-server,
# поэтому во временную папку этого пользователя; потом переносим в сырую папку. Копии из /var/backups не трогаем:
# --keep 1 стёр бы там суточные копии.
STAGE=$(mktemp -d /var/tmp/cs2drill.XXXXXX)
chown cs2-server:cs2-server "$STAGE"
chmod 0700 "$STAGE"
install -d -m 0700 "$OUT/raw-db"
if ctl backup "$STAGE" --keep 1 >"$OUT/backup-log.txt" 2>&1; then
  mv "$STAGE"/* "$OUT/raw-db/" 2>>"$OUT/backup-log.txt"
else
  echo "# backup не выполнен, копии базы нет" >>"$OUT/backup-log.txt"
fi
rm -rf "$STAGE"

# Папку отдаём пользователю, который вызвал sudo, чтобы её можно было скопировать без root.
if [ -n "${SUDO_USER:-}" ]; then
  chown -R "$SUDO_USER" "$OUT"
fi

echo "collect-server: собрано в $OUT"
echo "Дальше: скопируйте папку на машину автора, например:"
echo "  scp -r <хост>:$OUT \"\$LOCALAPPDATA/cs2cinema/drills/raw/<NN-имя>\""
echo "(сырая папка лежит вне репозитория; в репозиторий её отбирает evidence.sh import)"
