#!/usr/bin/env bash
# Улики учений: отбор из «сырой» папки и проверка папки улик.
#
#   bash docs/drills/evidence.sh import <сырое> <улики>
#   bash docs/drills/evidence.sh check  <улики> [--require-pass]
#
# import: копирует из сырой папки (вне репозитория) в папку улик (в папке фазы) ТОЛЬКО текстовые файлы
#         (.txt .log .md .json .csv и runner.log.1), без raw-db/, chat-export/, баз данных, файла token и
#         файлов больше 5 МиБ; пропуски пишет в import-log.txt; затем ищет секреты по сырой папке и по уликам
#         (образцы — из $CS2_DRILL_PATTERNS, иначе из %LOCALAPPDATA%/cs2cinema/drills/patterns.txt) и
#         пишет итог в token-scan.txt. Код 1, если секрет найден где-либо; код 2 — ошибка запуска.
# check:  проверяет RESULT.md, непустоту файлов из списка улик, отсутствие баз и экспорта чата, token-scan.txt
#         с нулём совпадений и кодировку (UTF-16 отвергается). С --require-pass требует result: pass.
#         Код 0 — «улики в порядке», код 1 — строки «ПРОБЛЕМА: …».
#
# Перед повтором учения сырую папку и папку улик удаляют целиком: import не стирает лишние файлы прошлого прогона.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
max_bytes=$((5 * 1024 * 1024))

usage() {
  echo "использование:" >&2
  echo "  bash docs/drills/evidence.sh import <сырое> <улики>" >&2
  echo "  bash docs/drills/evidence.sh check <улики> [--require-pass]" >&2
  exit 2
}

# Файл образцов: переменная окружения, иначе папка данных приложения.
patterns_file() {
  if [ -n "${CS2_DRILL_PATTERNS:-}" ]; then
    printf '%s\n' "$CS2_DRILL_PATTERNS"
    return
  fi
  if [ -n "${LOCALAPPDATA:-}" ]; then
    base=$LOCALAPPDATA
    if command -v cygpath >/dev/null 2>&1; then
      base=$(cygpath -u "$LOCALAPPDATA")
    fi
    printf '%s/cs2cinema/drills/patterns.txt\n' "$base"
  fi
}

cmd_import() {
  [ "$#" -eq 2 ] || usage
  raw=${1%/}
  out=${2%/}
  if [ ! -d "$raw" ]; then
    echo "import: нет сырой папки" >&2
    return 2
  fi
  patterns=$(patterns_file)
  if [ -z "$patterns" ] || [ ! -f "$patterns" ]; then
    echo "import: нет файла образцов (задайте CS2_DRILL_PATTERNS или создайте %LOCALAPPDATA%\\cs2cinema\\drills\\patterns.txt)" >&2
    return 2
  fi

  mkdir -p "$out"
  log="$out/import-log.txt"
  : >"$log"
  copied=0
  skipped=0

  while IFS= read -r -d '' file; do
    rel=${file#"$raw"/}
    name=${rel##*/}
    lower=$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')
    reason=""
    case "/$rel/" in
      */raw-db/* | */chat-export/*) reason="каталог не идёт в репозиторий" ;;
    esac
    if [ -z "$reason" ]; then
      case $lower in
        *.db | *.db-wal | *.db-shm | *.sqlite*) reason="база данных" ;;
        token) reason="файл токена" ;;
      esac
    fi
    if [ -z "$reason" ]; then
      case $lower in
        *.txt | *.log | *.md | *.json | *.csv | runner.log.1) ;;
        *) reason="расширение не из списка текстовых" ;;
      esac
    fi
    if [ -z "$reason" ]; then
      size=$(wc -c <"$file" | tr -d ' ')
      if [ "$size" -gt "$max_bytes" ]; then
        reason="больше 5 МиБ"
      fi
    fi
    if [ -n "$reason" ]; then
      printf 'пропущен: %s (%s)\n' "$rel" "$reason" >>"$log"
      skipped=$((skipped + 1))
      continue
    fi
    mkdir -p "$out/$(dirname "$rel")"
    cp "$file" "$out/$rel"
    copied=$((copied + 1))
  done < <(find "$raw" -type f -print0 | sort -z)

  printf 'скопировано: %s, пропущено: %s\n' "$copied" "$skipped" >>"$log"

  # Поиск секретов: сначала по сырой папке (там базы и экспорт чата), затем по отобранным уликам.
  # Файлы token-scan.txt и import-log.txt пересоздаются, поэтому их не ищем.
  scan_raw=$(bash "$here/scan-token.sh" "$patterns" "$raw" 2>&1)
  rc_raw=$?
  scan_ev=$(bash "$here/scan-token.sh" "$patterns" "$out" 2>&1)
  rc_ev=$?

  count_of() { printf '%s\n' "$1" | sed -n 's/^совпадений: \([0-9][0-9]*\)$/\1/p' | tail -n 1; }
  n_raw=$(count_of "$scan_raw")
  n_ev=$(count_of "$scan_ev")
  n_raw=${n_raw:-0}
  n_ev=${n_ev:-0}
  total=$((n_raw + n_ev))

  {
    echo "[сырая папка]"
    printf '%s\n' "$scan_raw" | grep -v '^совпадений: ' || true
    echo "[улики]"
    printf '%s\n' "$scan_ev" | grep -v '^совпадений: ' || true
    printf 'найдено в сырой папке: %s, в уликах: %s\n' "$n_raw" "$n_ev"
    printf 'совпадений: %s\n' "$total"
  } >"$out/token-scan.txt"

  if [ "$rc_raw" -ge 2 ] || [ "$rc_ev" -ge 2 ]; then
    echo "import: поиск секретов закончился ошибкой, см. $out/token-scan.txt" >&2
    return 2
  fi
  if [ "$total" -gt 0 ]; then
    echo "import: НАЙДЕН СЕКРЕТ ($total), не коммитьте папку улик; файлы — в $out/token-scan.txt" >&2
    return 1
  fi
  echo "import: скопировано $copied, пропущено $skipped, секретов не найдено"
  return 0
}

cmd_check() {
  [ "$#" -ge 1 ] || usage
  dir=${1%/}
  shift
  require_pass=0
  for a in "$@"; do
    case $a in
      --require-pass) require_pass=1 ;;
      *) usage ;;
    esac
  done
  if [ ! -d "$dir" ]; then
    echo "ПРОБЛЕМА: нет папки улик"
    return 1
  fi

  problems=0
  problem() {
    echo "ПРОБЛЕМА: $1"
    problems=$((problems + 1))
  }

  name=$(basename "$dir")
  result="$dir/RESULT.md"

  if [ ! -f "$result" ]; then
    problem "нет RESULT.md"
  else
    body=$(tr -d '\r' <"$result")
    d_name=$(printf '%s\n' "$body" | sed -n 's/^drill: *//p' | head -n 1)
    d_res=$(printf '%s\n' "$body" | sed -n 's/^result: *//p' | head -n 1)
    d_date=$(printf '%s\n' "$body" | sed -n 's/^date: *//p' | head -n 1)
    [ "$d_name" = "$name" ] || problem "в RESULT.md drill: должен быть равен имени папки ($name)"
    case $d_res in
      pass | fail) ;;
      *) problem "в RESULT.md result: должен быть pass или fail" ;;
    esac
    grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}$' <<<"$d_date" || problem "в RESULT.md date: должна быть вида ГГГГ-ММ-ДД"
    if [ "$require_pass" -eq 1 ] && [ "$d_res" != "pass" ]; then
      problem "нужен result: pass, а записано '$d_res' (учение не засчитывается)"
    fi

    outcome=$(printf '%s\n' "$body" | awk '/^## Исход/{f=1; next} /^## /{f=0} f && NF {n++} END{print n+0}')
    [ "$outcome" -gt 0 ] || problem "в RESULT.md пуст раздел «## Исход»"

    listed=$(printf '%s\n' "$body" | awk '/^## Улики/{f=1; next} /^## /{f=0} f' | sed -n 's/^- \([^ ][^ ]*\) — .*$/\1/p')
    if [ -z "$listed" ]; then
      problem "в RESULT.md нет ни одной строки «- <файл> — <что это>» в разделе «## Улики»"
    else
      while IFS= read -r item; do
        [ -z "$item" ] && continue
        item=${item#\`}
        item=${item%\`}
        case $item in
          /* | ../* | */../* | *..) problem "недопустимый путь в списке улик: $item"; continue ;;
        esac
        if [ ! -f "$dir/$item" ]; then
          problem "файла из списка улик нет: $item"
        elif [ ! -s "$dir/$item" ]; then
          problem "файл из списка улик пуст: $item"
        fi
      done <<EOF
$listed
EOF
    fi
  fi

  # Опасное в папке улик: базы, экспорт чата, сырой каталог.
  while IFS= read -r bad; do
    [ -z "$bad" ] && continue
    problem "в уликах не должно быть: ${bad#"$dir"/}"
  done < <(find "$dir" \( -iname '*.db' -o -iname '*.db-*' -o -iname '*.sqlite*' -o -name raw-db -o -name chat-export -o -name token \) 2>/dev/null)

  # Итог поиска секретов.
  scan="$dir/token-scan.txt"
  if [ ! -f "$scan" ]; then
    problem "нет token-scan.txt (улики не прошли evidence.sh import)"
  else
    if ! grep -qx 'совпадений: 0' "$scan"; then
      problem "token-scan.txt не содержит «совпадений: 0» (секрет найден или поиск не выполнен)"
    fi
    if grep -Eq '^совпадений: [1-9]' "$scan"; then
      problem "token-scan.txt: секрет найден"
    fi
  fi

  # Кодировка: файл в UTF-16 grep -F не прочитает, а значит и секрет в нём не найдёт.
  nfiles=0
  while IFS= read -r -d '' f; do
    nfiles=$((nfiles + 1))
    sig=$(head -c 2 "$f" | od -An -tx1 | tr -d ' \n')
    if [ "$sig" = "fffe" ] || [ "$sig" = "feff" ]; then
      problem "файл в UTF-16, пересохраните в UTF-8: ${f#"$dir"/}"
    fi
  done < <(find "$dir" -type f -print0 | sort -z)

  # Повторный поиск секретов по уликам, если файл образцов под рукой (улики могли измениться после import).
  patterns=$(patterns_file)
  if [ -n "$patterns" ] && [ -f "$patterns" ]; then
    rescan=$(bash "$here/scan-token.sh" "$patterns" "$dir" 2>&1)
    rc=$?
    if [ "$rc" -eq 1 ]; then
      problem "повторный поиск нашёл секрет в уликах (файлы: $(printf '%s\n' "$rescan" | grep -v '^совпадений: ' | tr '\n' ' '))"
    elif [ "$rc" -ge 2 ]; then
      problem "повторный поиск секретов закончился ошибкой"
    fi
  fi

  if [ "$problems" -gt 0 ]; then
    return 1
  fi
  echo "улики в порядке: $nfiles файлов"
  return 0
}

[ "$#" -ge 1 ] || usage
sub=$1
shift
case $sub in
  import) cmd_import "$@" ;;
  check) cmd_check "$@" ;;
  *) usage ;;
esac
exit $?
