#!/usr/bin/env bash
# Поиск секретов (токен бота, токен раннера, ключ FACEIT) по папкам и файлам улик.
#
# Использование:  bash docs/drills/scan-token.sh <файл-образцов> <путь>...
#
# Правила:
#  - секреты берутся ТОЛЬКО из файла образцов (одна строка — один секрет, строки с # и пустые пропускаются),
#    не из аргументов: аргументы видны в списке процессов и в истории shell;
#  - файл образцов внутри рабочего дерева git отвергается (код 2): его легко закоммитить в публичный репозиторий;
#  - к каждому образцу с ':' добавляются форма с '~' вместо первого ':' (так bot-api и журналы маскируют адрес)
#    и секретная часть после ':' (если она не короче 16 знаков);
#  - ищем и в текстах, и в двоичных файлах (grep -a), по подстроке, без регулярных выражений;
#  - печатаются ТОЛЬКО имена файлов и числа совпавших строк, значения — никогда.
#
# Коды: 0 — чисто, 1 — найдено, 2 — ошибка запуска (или файл образцов внутри репозитория).
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "использование: bash docs/drills/scan-token.sh <файл-образцов> <путь>..." >&2
  exit 2
fi

patterns=$1
shift

if [ ! -f "$patterns" ]; then
  echo "scan-token: нет файла образцов" >&2
  exit 2
fi

# Файл образцов не должен лежать внутри репозитория.
pdir=$(cd "$(dirname "$patterns")" && pwd)
if git -C "$pdir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "scan-token: файл образцов внутри репозитория: перенесите его (например, в %LOCALAPPDATA%\\cs2cinema\\drills)" >&2
  exit 2
fi

for p in "$@"; do
  if [ ! -e "$p" ]; then
    echo "scan-token: нет пути для поиска: $p" >&2
    exit 2
  fi
done

# Развёрнутые образцы — во временный файл, видимый только владельцу; удаляется при любом выходе.
expanded=$(mktemp)
chmod 600 "$expanded"
trap 'rm -f "$expanded"' EXIT

while IFS= read -r line || [ -n "$line" ]; do
  line=${line%$'\r'}
  [ -z "$line" ] && continue
  case $line in
    \#*) continue ;;
  esac
  printf '%s\n' "$line" >>"$expanded"
  case $line in
    *:*)
      head=${line%%:*}
      tail=${line#*:}
      # форма токена бота с '~' вместо ':'
      printf '%s~%s\n' "$head" "$tail" >>"$expanded"
      # секретная часть отдельно, если она достаточно длинная, чтобы не давать ложных совпадений
      if [ "${#tail}" -ge 16 ]; then
        printf '%s\n' "$tail" >>"$expanded"
      fi
      ;;
  esac
done <"$patterns"

if [ ! -s "$expanded" ]; then
  echo "scan-token: в файле образцов нет ни одного секрета" >&2
  exit 2
fi

# grep -c печатает «путь:число» для каждого файла; значения совпавших строк не печатаются никогда.
rc=0
counts=$(grep -rFac -H -f "$expanded" -- "$@" 2>/dev/null) || rc=$?

total=0
while IFS= read -r entry; do
  [ -z "$entry" ] && continue
  # разбор по последнему ':' — в путях Windows есть 'C:'
  file=${entry%:*}
  n=${entry##*:}
  case $n in
    '' | *[!0-9]*) continue ;;
  esac
  if [ "$n" -gt 0 ]; then
    printf '%s: %s\n' "$file" "$n"
    total=$((total + n))
  fi
done <<EOF
$counts
EOF

printf 'совпадений: %s\n' "$total"

if [ "$total" -gt 0 ]; then
  exit 1
fi
if [ "$rc" -ge 2 ]; then
  echo "scan-token: поиск закончился с ошибкой (нечитаемый файл?)" >&2
  exit 2
fi
exit 0
