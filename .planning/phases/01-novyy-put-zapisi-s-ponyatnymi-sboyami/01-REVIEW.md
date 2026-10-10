---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
reviewed: 2026-10-10T00:00:00Z
depth: standard
files_reviewed: 33
files_reviewed_list:
  - Cargo.toml
  - crates/host/Cargo.toml
  - crates/host/src/lib.rs
  - crates/host/src/gate.rs
  - crates/host/src/runner.rs
  - crates/host/src/unsupported.rs
  - crates/host/src/fake.rs
  - crates/host/src/win/mod.rs
  - crates/host/src/win/job.rs
  - crates/host/src/win/process.rs
  - crates/host/src/win/watch.rs
  - crates/host/src/win/netcon.rs
  - crates/host/src/win/console_log.rs
  - crates/host/tests/win_session.rs
  - crates/host/examples/spikes.rs
  - crates/pipeline/Cargo.toml
  - crates/pipeline/src/lib.rs
  - crates/pipeline/src/record/mod.rs
  - crates/pipeline/src/record/machine.rs
  - crates/pipeline/src/record/markers.rs
  - crates/pipeline/src/record/deadlines.rs
  - crates/pipeline/src/record/failure.rs
  - crates/pipeline/src/record/log.rs
  - crates/pipeline/tests/record.rs
  - crates/pipeline/tests/transcripts.rs
  - crates/renderer/src/lib.rs
  - crates/store/src/lib.rs
  - crates/app/src/jobs.rs
  - crates/app/src/jobs_page.rs
  - crates/app/src/launch.rs
  - crates/app/src/settings.rs
  - crates/app/src/settings_page.rs
  - crates/app/src/status.rs
findings:
  critical: 0
  warning: 5
  info: 7
  total: 12
status: issues_found
---

# Phase 1: Code Review Report

**Reviewed:** 2026-10-10
**Depth:** standard (плюс сквозная проверка шва Host/Session, pipeline::record, WinHost, миграции store и перехода app)
**Files Reviewed:** 33
**Status:** issues_found

## Summary

Проверены крейт `host` (включая весь `unsafe` в `win/`), машина записи `pipeline::record`, миграция `store`, переход `app` на `pipeline::record`, изменения `renderer`. `cargo test -p host -p store -p pipeline -p app` проходит; `cargo clippy` по `host` замечаний не даёт.

Блокирующих дефектов не найдено. Что проверено и оказалось корректным:

- **Unsafe в `host/src/win`.** Каждый блок имеет `// SAFETY:`, и они соответствуют коду. Буфер `Job::pids` выровнен по `usize`, длина больше запрошенной, `listed <= cap` соблюдается. Handles закрываются ровно один раз (`Job::drop`, `OwnedProc::drop`, `snapshot_processes`, `assign_pid`). `Send`/`Sync` для handle обоснованы.
- **Живость.** `OwnedProc::wait` отдаёт `Alive` только при `WAIT_TIMEOUT`, любая ошибка даёт `Unknown`. `record()` читает живость до `terminate`, поэтому `Alive` при ошибке не возникает.
- **Дедлайны и таймеры.** Спина и бесконечного ожидания нет: у каждого цикла есть `recv_timeout`, `sleep` или дедлайн. `run_bounded` ограничен дедлайном, отменой и `DRAIN_LIMIT`.
- **Метки.** `parse_marker` принимает только строку целиком (`CS2C <nonce> <вид> [n]`), эхо команд не принимается. `strip_timestamp` проверяет границы по байтам до среза.
- **Миграция.** Параметры привязаны через `params!`, шаг идёт в транзакции вместе с `user_version`. Тесты старой базы и базы версии 1 реальны.
- **Имя демки.** `console_name` на стороне app оставляет только `[A-Za-z0-9_-]`, `check_demo_name` на стороне хоста дополнительно отвергает ASCII-пробелы, кавычки и `;`.
- **Сборка не на Windows.** `win` закрыт `cfg(windows)`, `UnsupportedHost` собирается, `windows-sys` подключён только для Windows.

Замечания ниже касаются в основном краевых случаев «закрытие игры / успех записи», окна гонки в воротах демки, незащищённого от зависания вызова NVENC и неполной маскировки профиля в журнале.

## Warnings

### WR-01: Сбой отправки `quit` сразу убивает весь job и объявляет запись успешной

**File:** `crates/pipeline/src/record/machine.rs:343-348`
**Issue:** `on_send_failed` при `line == "quit"` выдаёт `Action::Terminate` и тут же `finish_ok`. Отправка `quit` может не пройти из-за кратковременного `ConsoleDown`/`NotConnected` (netcon переподключается раз в секунду). Тогда `TerminateJobObject` убивает `cs2.exe` и `ffmpeg` HLAE в момент, когда последний дубль только что закончился. ffmpeg мог не дописать `video.mp4` (нет moov-атома). Запись при этом считается успешной, а склейка позже упадёт с малопонятной ошибкой ffmpeg. Остальные пути закрытия (`JobEmpty`, `close_settle`, `game_close`) ждут процессы, этот путь не ждёт.
**Fix:** не завершать job сразу. Выдать факт о неотправленном `quit`, остаться в `Closing` и положиться на существующие ветки (`game_close`, `JobEmpty`). Можно также повторить `quit` на следующем `ConsoleUp`, пока состояние `Closing`:
```rust
if line == "quit" {
    acts.push(Action::Emit(Event::Fact(failure::msg_send_failed(line, error))));
    return; // остаёмся в Closing: дедлайн game_close завершит job сам
}
```
И в `on_event` для `ConsoleUp` при `self.state == RecordState::Closing` отправлять `quit` повторно.

### WR-02: Ворота «демка идёт» открываются строкой запроса демки, а не её активации

**File:** `crates/host/src/gate.rs:14-17` (`is_demo_playing`), используется в `DemoGate::admit` (gate.rs:61-64)
**Issue:** условие `line.contains("playing demo from")` истинно уже в момент запроса. В фикстуре `s3e_late_connect.log` это строка на +12.4 с, а `Host activate: Playing Demo` приходит на +14.1 с, то есть ворота открываются на ~1.7 с раньше, чем демка действительно идёт. Весь смысл ворот — отбросить ложный `READY` от `mirv_cmd addAtTick 64`, сработавший на меню-карте (спайк S3c). Если тик 64 меню-карты придётся на окно между запросом и активацией, `READY` пройдёт ворота. Машина отправит `exec highlights` на загрузочном экране. Дальше `SCRIPT`/`TAKE_START` не придут, и запись упадёт по `TAKE_TIMEOUT` через 90 с без восстановления: повторный `READY` напрямую (`ready_direct`) не запрашивается, поскольку `READY` уже принят. В `s3c_missing_cfg.log` строки `Host activate: Playing Demo` нет вовсе, и, вероятно, поэтому `playing demo from` был добавлен как подтверждение. Данные спайков противоречат друг другу, но окно гонки остаётся.
**Fix:** считать подтверждением только активацию (`Host activate: Playing Demo` либо переход `LOADINGSCREEN -> CSGO_GAME_UI_STATE_INGAME`), а `playing demo from` оставить как «запрошено». Проверить на `s3c` тем же тестом `ready_before_the_demo_is_dropped_in_the_s3c_transcript`. Если в s3c активации действительно нет, подтверждать по `INGAME` после запроса.

### WR-03: Проверка NVENC запускает ffmpeg без дедлайна и без отмены

**File:** `crates/app/src/launch.rs:136-147` (`nvenc_check`), вызовы в `crates/app/src/jobs.rs:184-186`
**Issue:** `nvenc_check` делает `Command::output()` без таймаута. Он вызывается дважды на задачу записи: напрямую для ffmpeg из PATH и через `has_nvenc` для ffmpeg из HLAE. Пробный энкод `h264_nvenc` на сбойном драйвере или при заблокированном GPU способен зависнуть. Тогда задача навсегда остаётся `running`, кнопка «Отмена» не работает (токен проверяется только между этапами), очередь стоит. Это противоречит цели фазы «задача либо доходит, либо быстро падает» и ограничению проекта «любой детектор должен иметь таймаут». При этом `Host::run` с дедлайном и отменой уже есть.
**Fix:** запускать пробу через `host.run(&CmdSpec { deadline: Duration::from_secs(20), .. }, &|| p.cancelled(), ..)` и разбирать `stderr_tail`. Либо добавить в `nvenc_check` ожидание с `try_wait` и kill по таймауту.

### WR-04: Маскировка профиля пользователя в `record.log` оставляет часть имени, если в нём есть пробел

**File:** `crates/pipeline/src/record/log.rs:110`; тест, закрепляющий поведение, `record/log.rs:355-359`
**Issue:** конец имени профиля ищется по набору `" \t\"'<>|,;"`, то есть по первому пробелу. Для `C:\Users\Иван Петров\x` результат `C:\Users\USER Петров\x`, а тест это прямо утверждает. Профили Windows с пробелом (`John Smith`) обычны, и фамилия утекает в журнал, который заявлен как безопасный для пересылки (`log.rs` module doc: «профиль пользователя … скрываются»). Журнал содержит пути демки и cs2/HLAE из `[Demo] Requesting playback…`, так что утечка реальна.
**Fix:** для путей с разделителями имя профиля заканчивается на следующем разделителе пути. Пробел как стоп-символ нужен только для не-путевого контекста, поэтому его можно убрать из набора, когда за именем есть разделитель:
```rust
// имя профиля — до ближайшего разделителя пути или кавычки; пробел допустим внутри имени
while e < sb.len() && !is_sep(sb[e]) && !b"\t\"'<>|,;".contains(&sb[e]) { e += 1; }
```
Тест обновить на `USER` целиком.

### WR-05: После выхода игры job добивается через 3 с даже если в нём ещё работает ffmpeg HLAE

**File:** `crates/pipeline/src/record/machine.rs:515-520`, `crates/pipeline/src/record/deadlines.rs:40,64` (`close_settle = 3 с`)
**Issue:** в `Closing`, получив `GameExited`, машина ждёт `close_settle` (3 с) и объявляет успех (`finish_ok`), не дожидаясь `JobEmpty`. Сразу после этого `Guard::drop` вызывает `terminate`, то есть `TerminateJobObject`. Непустой job после выхода игры означает именно то, что ffmpeg HLAE ещё дописывает последний дубль (энкод x264 с большим буфером или большой файл за 3 с мог не закончить). Принудительное завершение даёт обрезанный `video.mp4`, а запись считается успешной. Значение `close_settle` жёстко задано и масштабируется только `time_margin`. Явного ожидания «job пуст» вместо фиксированной паузы нет.
**Fix:** в `Closing` после `GameExited` ждать `JobEmpty` до `game_close` (или отдельного дедлайна «дописывание дубля», например 30 с), и только по его истечении завершать job с фактом в журнал. Фиксированную паузу `close_settle` оставить лишь для случая `JobEmpty`-события, которое хост не смог доставить.

## Info

### IN-01: Гонка при первом запуске двух экземпляров приложения на старой базе

**File:** `crates/store/src/lib.rs:255-264`
**Issue:** `user_version` читается вне транзакции, а шаг идёт в `unchecked_transaction()` (DEFERRED). Если два процесса одновременно открывают базу версии 0, оба прочитают 0, и второй упадёт на `ALTER TABLE ... ADD COLUMN` с `duplicate column name`. Приложение не защищено от второго запуска. Окно узкое (однократно при обновлении), но ошибка миграции делает приложение неоткрываемым.
**Fix:** `conn.execute_batch("BEGIN IMMEDIATE")` и чтение `user_version` внутри транзакции, либо `PRAGMA busy_timeout` и перечитывание версии после захвата записи.

### IN-02: Проверка «файл консоли пересоздан» может пропустить начало нового `console.log`

**File:** `crates/host/src/win/console_log.rs:50-54`
**Issue:** пересоздание файла определяется как `len < offset`. Если у прошлого запуска файл был небольшим (L байт), а новый успевает вырасти за L между двумя опросами (250 мс), хвост начнёт читать с середины нового файла: первые L байт теряются, в том числе возможная строка `Host activate` и строка запуска. Метки при этом не страдают (они позже), но диагностика запуска и `evidence_at` могут быть неполными.
**Fix:** различать пересоздание по времени создания файла (`metadata().created()`) или по файловому идентификатору, а не только по длине.

### IN-03: Предел длины строки netcon не действует при непрерывном потоке без перевода строки

**File:** `crates/host/src/win/netcon.rs:89-98`
**Issue:** `MAX_LINE` проверяется только в ветке таймаута чтения. `read_until(b'\n')` читает без ограничения, пока не встретит разделитель, а таймаут сокета срабатывает только при паузе в данных. Непрерывный поток байт без `\n` растёт в `buf` без границы, хотя комментарий называет это «защитой от бесконечного буфера». Источник — своя игра на loopback, поэтому риск низкий.
**Fix:** читать через `take(MAX_LINE as u64)` либо проверять `buf.len()` внутри собственного цикла `fill_buf`/`consume`.

### IN-04: Хвост консоли и `record.log` не ограничивают длину одной строки

**File:** `crates/pipeline/src/record/log.rs:79-81, 206-210, 249-269`
**Issue:** `clean`/`one_line` не усекают текст. Кольцо хвоста держит до 500 записей, а строка консоли может быть до 1 МиБ (`MAX_LINE` хоста), то есть теоретически сотни МБ в памяти. Строка хвоста затем сохраняется в SQLite (`fail_tail`) и рисуется `egui::Label`. Обычные строки CS2 короткие, поэтому практический риск мал.
**Fix:** усекать в `clean` до разумной длины (например, 500 символов) с суффиксом `…`.

### IN-05: Дублирующиеся определения и «рассинхронизируемые» копии логики

**File:** `crates/host/src/gate.rs:79-93` и `crates/pipeline/src/record/markers.rs:65-77` (`strip_timestamp`); `crates/host/src/gate.rs:11` и `renderer::MARKER_TAG`; `gate::is_demo_playing` и `markers::is_demo_running_evidence`
**Issue:** две копии разбора префикса времени, две константы тега (сверяются только тестом) и два разных предиката «демка идёт» (см. WR-02: условия не совпадают, и это уже даёт расхождение между воротами и машиной). Правка одной копии тихо оставит вторую старой.
**Fix:** держать `strip_timestamp`, тег метки и предикат «демка идёт» в одном месте (например, `host::gate`), а `pipeline` пусть переиспользует.

### IN-06: Лаунчер помещается в job после `spawn`, а не при создании

**File:** `crates/host/src/win/mod.rs:192-195`
**Issue:** `cmd.spawn()` затем `job.assign_child`. Если HLAE успеет порождать потомков до присвоения, они в job не попадут (и не будут убиты `KILL_ON_JOB_CLOSE`). На практике HLAE стартует игру спустя сотни миллисекунд, а запасной путь принятия по PID это покрывает, поэтому риск низкий.
**Fix:** при желании запускать с `CREATE_SUSPENDED` и возобновлять поток после присвоения, либо оставить как есть и зафиксировать допущение комментарием.

### IN-07: `check_demo_name` пропускает разделители пути, `..` и ведущие `-`/`+`

**File:** `crates/host/src/win/mod.rs:290-308`
**Issue:** публичный `WinHost::launch` принимает `sub/dir.dem`, `..\x` и имена вида `-quit` (тест сам утверждает, что `sub/dir.dem` допустим). Приложение через `console_name` передаёт безопасные имена, и пробел с `;`/кавычкой отвергаются, так что инъекции в `-cmdLine` нет. Но слой хоста — граница с ОС, и разрешать ему больше, чем нужно, незачем.
**Fix:** ограничить `[A-Za-z0-9_.-]`, не начинать с `-`/`+`, запретить `..`.

---

_Reviewed: 2026-10-10_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
