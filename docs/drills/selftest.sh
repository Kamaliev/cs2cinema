#!/usr/bin/env bash
# Самопроверка скриптов учений на синтетических данных (подставной токен собирается из частей).
#
#   bash docs/drills/selftest.sh     # последняя строка «selftest: OK», код 0
#
# Проверяет scan-token.sh и evidence.sh (import, check) и то, что значение подставного токена не попадает
# ни в вывод скриптов, ни в папку улик. Всё работает во временном каталоге, который удаляется при выходе.
set -euo pipefail

D=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

fail() {
  echo "selftest: ПРОВАЛ: $1"
  exit 1
}

# Весь вывод проверяемых скриптов складывается в один файл, пункт (е) ищет в нём токен.
ALL="$T/all-output.txt"
: >"$ALL"
OUT=""
RC=0
run() {
  OUT=$("$@" 2>&1) && RC=0 || RC=$?
  printf '%s\n' "$OUT" >>"$ALL"
}
expect_rc() { [ "$RC" -eq "$1" ] || fail "$2 (код $RC, ожидался $1)"; }
expect_out() { grep -qF -- "$1" <<<"$OUT" || fail "$2"; }

if git -C "$T" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  fail "временный каталог внутри репозитория git, задайте TMPDIR вне репозитория"
fi

# Подставной токен: целиком в тексте скрипта его нет.
TOK_ID=$(printf '%s%s' 1234 56789)
TOK_SEC=$(printf '%s%s%s' 'AAFsynth' 'Etic_Secret' '_Part_0123456789')
TOK="$TOK_ID:$TOK_SEC"
TILDE="$TOK_ID~$TOK_SEC"
OTHER=$(printf '%s%s' 'runner-secret-' 'value-xyz-0001')

# Файл образцов вне репозитория: с комментарием, пустой строкой и CRLF, как его сохранит Блокнот.
mkdir "$T/priv"
printf '# образцы\r\n\r\n%s\r\n%s\r\n' "$TOK" "$OTHER" >"$T/priv/patterns.txt"
export CS2_DRILL_PATTERNS="$T/priv/patterns.txt"

# --- (а) scan-token.sh находит токен в тексте, в форме с '~' и в двоичном файле ---
mkdir "$T/leaky"
printf 'строка\nключ=%s\n' "$TOK" >"$T/leaky/a.txt"
printf 'url=/bot%s/getMe\n' "$TILDE" >"$T/leaky/b.txt"
{
  printf 'ab\0cd'
  printf '%s' "$TOK"
  printf '\0\377tail'
} >"$T/leaky/c.bin"
run bash "$D/scan-token.sh" "$T/priv/patterns.txt" "$T/leaky"
expect_rc 1 "(а) токен в трёх файлах"
expect_out "a.txt" "(а) нет a.txt в выводе"
expect_out "b.txt" "(а) нет b.txt (форма с ~) в выводе"
expect_out "c.bin" "(а) нет c.bin (двоичный файл) в выводе"
expect_out "совпадений: 3" "(а) ожидалось «совпадений: 3»"

mkdir "$T/leaky2"
printf 'x %s y\n' "$TOK_SEC" >"$T/leaky2/secret-part.txt"
printf 'z %s z\n' "$OTHER" >"$T/leaky2/other.txt"
run bash "$D/scan-token.sh" "$T/priv/patterns.txt" "$T/leaky2"
expect_rc 1 "(а) секретная часть и второй образец"
expect_out "совпадений: 2" "(а) ожидалось «совпадений: 2» для секретной части и второго образца"

# Короткая секретная часть (меньше 16 знаков) отдельно не ищется: форма с '~' должна находиться сама.
SHORT_ID=$(printf '%s%s' 98765 4321)
SHORT_SEC=$(printf '%s%s' 'shortS' 'ecret')
mkdir "$T/leaky3"
printf '%s:%s\n' "$SHORT_ID" "$SHORT_SEC" >"$T/priv/short-patterns.txt"
printf '/bot%s~%s/getMe\n' "$SHORT_ID" "$SHORT_SEC" >"$T/leaky3/tilde-only.txt"
run bash "$D/scan-token.sh" "$T/priv/short-patterns.txt" "$T/leaky3"
expect_rc 1 "(а) форма с ~ при короткой секретной части"
expect_out "tilde-only.txt" "(а) форма с ~ не найдена без секретной части"

run bash "$D/scan-token.sh" "$T/priv/patterns.txt" "$T/нет-такого-пути"
expect_rc 2 "(а) несуществующий путь"
echo "selftest: (а) поиск находит токен в тексте, в форме с ~ и в двоичном файле"

# --- (б) чистая папка ---
mkdir "$T/clean"
printf 'обычный журнал без секретов\n' >"$T/clean/log.txt"
run bash "$D/scan-token.sh" "$T/priv/patterns.txt" "$T/clean"
expect_rc 0 "(б) чистая папка"
expect_out "совпадений: 0" "(б) ожидалось «совпадений: 0»"
echo "selftest: (б) чистая папка даёт код 0"

# --- (в) файл образцов внутри рабочего дерева git ---
git init -q "$T/fakerepo"
cp "$T/priv/patterns.txt" "$T/fakerepo/patterns.txt"
run bash "$D/scan-token.sh" "$T/fakerepo/patterns.txt" "$T/clean"
expect_rc 2 "(в) файл образцов внутри репозитория"
echo "selftest: (в) файл образцов в репозитории отвергается"

# --- (г) evidence.sh import ---
R="$T/raw/01-test"
EV="$T/ev/01-test"
mkdir -p "$R/raw-db" "$R/chat-export" "$R/sub"
printf 'строка журнала сервера\n' >"$R/server-journal.txt"
printf 'журнал раннера\n' >"$R/runner.log"
printf 'прошлый журнал\n' >"$R/runner.log.1"
printf 'переписка (роли)\n' >"$R/chat.txt"
printf 'вложенный файл\n' >"$R/sub/inner.txt"
printf 'база' >"$R/raw-db/server-x.db"
printf '{"messages":[]}' >"$R/chat-export/result.json"
printf 'wal' >"$R/state.db-wal"
printf 'not-a-secret\n' >"$R/token"
head -c 5300000 /dev/zero | tr '\0' 'a' >"$R/big.log"
run bash "$D/evidence.sh" import "$R" "$EV"
expect_rc 0 "(г) импорт чистой сырой папки"
for f in server-journal.txt runner.log runner.log.1 chat.txt sub/inner.txt token-scan.txt import-log.txt; do
  [ -f "$EV/$f" ] || fail "(г) в уликах нет $f"
done
for f in raw-db chat-export big.log token state.db-wal; do
  [ -e "$EV/$f" ] && fail "(г) в улики попало лишнее: $f"
done
grep -qx 'совпадений: 0' "$EV/token-scan.txt" || fail "(г) token-scan.txt без «совпадений: 0»"
grep -qF 'пропущен: big.log' "$EV/import-log.txt" || fail "(г) import-log.txt не знает про big.log"

R2="$T/raw/leak"
mkdir -p "$R2"
printf 'журнал с утечкой: %s\n' "$TOK" >"$R2/leak.txt"
run bash "$D/evidence.sh" import "$R2" "$T/ev/leak"
expect_rc 1 "(г) токен в сырой папке"
grep -qx 'совпадений: 2' "$T/ev/leak/token-scan.txt" || fail "(г) token-scan.txt утечки без «совпадений: 2» (сырое и улики)"

run env CS2_DRILL_PATTERNS="$T/нет-образцов.txt" bash "$D/evidence.sh" import "$R" "$T/ev/no-patterns"
expect_rc 2 "(г) нет файла образцов"
echo "selftest: (г) импорт отбирает текст, пропускает базы, чат и большие файлы, ловит токен"

# --- (д) evidence.sh check ---
write_result() {
  # $1 — result, $2 — drill, $3 — строки списка улик
  cat >"$EV/RESULT.md" <<EOF
drill: $2
result: $1
date: 2026-10-13

## Исход

Синтетический исход самопроверки.

## Улики

$3
EOF
}
GOOD='- server-journal.txt — журнал сервера
- runner.log — журнал раннера'

run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) нет RESULT.md"
expect_out "нет RESULT.md" "(д) нет сообщения про RESULT.md"

write_result pass 01-test "$GOOD"
run bash "$D/evidence.sh" check "$EV" --require-pass
expect_rc 0 "(д) правильная папка"
expect_out "улики в порядке" "(д) нет «улики в порядке»"

: >"$EV/empty.txt"
write_result pass 01-test "$GOOD
- empty.txt — пустой файл"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) пустой файл из списка улик"
expect_out "пуст: empty.txt" "(д) нет сообщения про пустой файл"
write_result pass 01-test "$GOOD
- missing.txt — нет такого"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) файл из списка улик отсутствует"
rm -f "$EV/empty.txt"

write_result fail 01-test "$GOOD"
run bash "$D/evidence.sh" check "$EV" --require-pass
expect_rc 1 "(д) result: fail с --require-pass"
run bash "$D/evidence.sh" check "$EV"
expect_rc 0 "(д) result: fail без --require-pass (записанный провал)"

write_result pass other-name "$GOOD"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) drill: не совпадает с именем папки"
write_result pass 01-test "$GOOD"

printf 'x' >"$EV/server-x.db"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) *.db в уликах"
rm -f "$EV/server-x.db"

printf '\377\376A\0B\0' >"$EV/u16.txt"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) файл в UTF-16"
expect_out "UTF-16" "(д) нет сообщения про UTF-16"
rm -f "$EV/u16.txt"

printf 'поздняя запись %s\n' "$TOK" >"$EV/late.txt"
run bash "$D/evidence.sh" check "$EV"
expect_rc 1 "(д) токен добавлен в улики после импорта"
rm -f "$EV/late.txt"

run bash "$D/evidence.sh" check "$EV" --require-pass
expect_rc 0 "(д) после уборки папка снова в порядке"

mkdir -p "$T/raw/empty"
run bash "$D/evidence.sh" import "$T/raw/empty" "$T/ev/02-empty"
expect_rc 0 "(д) импорт пустой сырой папки"
run bash "$D/evidence.sh" check "$T/ev/02-empty"
expect_rc 1 "(д) пустая сырая папка даёт папку без RESULT.md"
echo "selftest: (д) check отвергает неполные, пустые, с базами, в UTF-16 и с токеном улики"

# --- (е) значение токена нигде не напечатано и не попало в улики ---
for needle in "$TOK" "$TILDE" "$TOK_SEC" "$OTHER"; do
  if grep -qF -e "$needle" "$ALL"; then
    fail "(е) значение секрета оказалось в выводе скриптов"
  fi
  if grep -rqFa -e "$needle" "$EV" "$T/ev/leak/token-scan.txt" "$T/ev/leak/import-log.txt"; then
    fail "(е) значение секрета оказалось в папке улик или в token-scan.txt"
  fi
done
echo "selftest: (е) значения секретов нигде не напечатаны"

echo "selftest: OK"
