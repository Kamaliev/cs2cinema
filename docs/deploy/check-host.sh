#!/usr/bin/env bash
# Проверка хоста после развёртывания и после перезагрузки (README, разделы 11 и 12).
#
# Запуск: sudo bash check-host.sh --access tailscale|caddy
# Каждая проверка печатает «OK: …» или «ПРОБЛЕМА: …». Код выхода 0 — проблем нет, 1 — есть хотя бы одна.
# Содержимое файлов секретов не печатается: проверяются только права, владелец и наличие токена в файле настроек.
set -u

USAGE="Использование: sudo bash check-host.sh --access tailscale|caddy"

ACCESS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --access)
      ACCESS="${2:-}"
      shift 2 || shift
      ;;
    -h | --help)
      echo "$USAGE"
      exit 0
      ;;
    *)
      echo "неизвестный аргумент: $1" >&2
      echo "$USAGE" >&2
      exit 2
      ;;
  esac
done
case "$ACCESS" in
  tailscale | caddy) ;;
  *)
    echo "$USAGE" >&2
    exit 2
    ;;
esac

PROBLEMS=0
ok() { echo "OK: $*"; }
bad() {
  echo "ПРОБЛЕМА: $*"
  PROBLEMS=$((PROBLEMS + 1))
}

SERVER_URL="http://127.0.0.1:8080"
BOTAPI_URL="http://127.0.0.1:8081"
BACKUP_DIR="/var/backups/cs2-server"
COMPOSE_FILE="/opt/cs2-bot-api/docker-compose.yml"

# --- службы systemd ---
if systemctl is-enabled cs2-server >/dev/null 2>&1; then
  ok "cs2-server включён в автозапуск"
else
  bad "cs2-server не включён в автозапуск (systemctl enable cs2-server)"
fi
if systemctl is-active cs2-server >/dev/null 2>&1; then
  ok "cs2-server запущен"
else
  bad "cs2-server не запущен (journalctl -u cs2-server -n 50)"
fi
if systemctl is-enabled cs2-server-backup.timer >/dev/null 2>&1; then
  ok "таймер резервных копий включён"
else
  bad "таймер cs2-server-backup.timer не включён"
fi

# --- контейнеры bot-api ---
if command -v docker >/dev/null 2>&1 && [ -f "$COMPOSE_FILE" ]; then
  RUNNING=$(docker compose -f "$COMPOSE_FILE" ps --status running --services 2>/dev/null)
  for svc in telegram-bot-api botapi-janitor; do
    if printf '%s\n' "$RUNNING" | grep -qx "$svc"; then
      ok "контейнер $svc запущен"
    else
      bad "контейнер $svc не запущен (docker compose -f $COMPOSE_FILE ps)"
    fi
  done
else
  bad "нет docker или файла $COMPOSE_FILE"
fi

# --- слушающие порты: снаружи открыто только задуманное ---
ALLOWED_PORTS="22"
if [ "$ACCESS" = "caddy" ]; then
  ALLOWED_PORTS="22 80 443"
fi
LISTEN=$(ss -tlnp 2>/dev/null | awk 'NR>1 {print $4}')
EXPOSED=0
while IFS= read -r addr; do
  [ -z "$addr" ] && continue
  port="${addr##*:}"
  host="${addr%:*}"
  case "$host" in
    127.* | "[::1]" | "::1") continue ;; # loopback: снаружи не виден
    100.* | "[fd7a:"*) continue ;;       # адрес Tailscale: доступен только участникам tailnet
  esac
  case " $ALLOWED_PORTS " in
    *" $port "*) ;;
    *)
      bad "порт $port слушает на всех интерфейсах ($addr); разрешены только: $ALLOWED_PORTS"
      EXPOSED=1
      ;;
  esac
done <<EOF_LISTEN
$LISTEN
EOF_LISTEN
if [ "$EXPOSED" = 0 ]; then
  ok "снаружи слушают только порты: $ALLOWED_PORTS"
fi
for want in "127.0.0.1:8080" "127.0.0.1:8081"; do
  if printf '%s\n' "$LISTEN" | grep -qx "$want"; then
    ok "слушает $want"
  else
    bad "нет слушателя на $want (ss -tlnp)"
  fi
done

# --- ответы служб ---
CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST "$SERVER_URL/v1/runner/hello" 2>/dev/null)
if [ "$CODE" = "401" ]; then
  ok "сервер отвечает и требует токен (/v1/runner/hello -> 401)"
else
  bad "сервер на $SERVER_URL ответил кодом ${CODE:-нет ответа}, ожидалось 401"
fi
CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$BOTAPI_URL/" 2>/dev/null)
if [ -n "$CODE" ] && [ "$CODE" != "000" ]; then
  ok "telegram-bot-api отвечает (код $CODE)"
else
  bad "telegram-bot-api на $BOTAPI_URL не отвечает"
fi

# --- резервные копии ---
NEWEST=$(ls -1t "$BACKUP_DIR"/server-*.db 2>/dev/null | head -n 1)
if [ -z "$NEWEST" ]; then
  bad "в $BACKUP_DIR нет копий (systemctl start cs2-server-backup.service)"
else
  AGE=$(($(date +%s) - $(stat -c %Y "$NEWEST")))
  if [ "$AGE" -lt $((26 * 3600)) ]; then
    ok "свежая копия: $(basename "$NEWEST"), возраст $((AGE / 3600)) ч"
  else
    bad "самая новая копия старше 26 часов: $(basename "$NEWEST")"
  fi
fi

# --- права файлов секретов ---
for f in /etc/cs2-server/env /opt/cs2-bot-api/.env; do
  if [ ! -f "$f" ]; then
    bad "нет файла $f"
    continue
  fi
  mode=$(stat -c %a "$f")
  owner=$(stat -c %U:%G "$f")
  if [ "$mode" = "600" ] && [ "$owner" = "root:root" ]; then
    ok "$f: права 600, владелец root:root"
  else
    bad "$f: права $mode, владелец $owner (нужно 600 root:root)"
  fi
done
if grep -Eq '"telegram_bot_token"[[:space:]]*:[[:space:]]*"' /etc/cs2-server/cs2-server.json 2>/dev/null; then
  bad "токен бота лежит в /etc/cs2-server/cs2-server.json: ему место только в /etc/cs2-server/env"
else
  ok "в файле настроек токена бота нет"
fi

# --- доступ раннера ---
if [ "$ACCESS" = "tailscale" ]; then
  if [ -n "$(tailscale serve status 2>/dev/null)" ]; then
    ok "tailscale serve настроен"
  else
    bad "tailscale serve status пуст (README, раздел 8, вариант А)"
  fi
else
  if systemctl is-active caddy >/dev/null 2>&1; then
    ok "caddy запущен"
  else
    bad "caddy не запущен (README, раздел 8, вариант Б)"
  fi
fi

# --- место на диске ---
echo "--- место на диске ---"
df -h /var/lib/cs2-server /var/lib/telegram-bot-api 2>/dev/null

if [ "$PROBLEMS" -gt 0 ]; then
  echo "Итог: проблем $PROBLEMS"
  exit 1
fi
echo "Итог: всё в порядке"
exit 0
