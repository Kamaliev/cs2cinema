# Фаза 1: результаты спайков S1–S4

**Дата прогона:** 2026-10-10, 16:18–16:30 (машина автора, Windows 11, CS2 закрыт пользователем, Steam запущен).
**HLAE:** 2.192.7.0 (`HLAE.exe`), `x64\AfxHookSource2.dll` из той же установки.
**CS2:** `PatchVersion=1.41.9.0` (`game\csgo\steam.inf`).
**Стенд:** `crates/host/examples/spikes.rs` из плана 01-01, собран `cargo build --release -p host --example spikes`. Подкоманды запускались по одной, без параллельных запусков игры; перед каждой проверялось, что `cs2.exe` не запущен. Чужой `cs2.exe` не встречался и не трогался.
**Всего запусков игры:** 17 (S1 — 1, S2 — 10, S3 — 4, S4 — 1 по стенду, плюс 1 ручной дополнительный запуск S4 вне стенда, см. ниже). После прогона `Get-Process cs2` пуст.
**Транскрипты:** `%LOCALAPPDATA%\cs2cinema\spikes\<метка>\` (метки 1791631103 — S1, 1791631182 — S2 без флага, 1791631272 — S2 с флагом, 1791631372 — S3, 1791631582 — S4). Пути профиля в этом файле и в фикстурах заменены на `C:/Users/USER`; строки с SteamID и именем аккаунта из фикстур удалены.

Фикстуры консоли (сокращены до 190–270 строк: начало запуска, окрестность события, конец; шум вроде распределения render-target и путей Steam убран):
`crates/pipeline/tests/fixtures/s3a_missing_demo.log`, `s3b_empty_demo.log`, `s3c_missing_cfg.log`, `s3e_late_connect.log`. Формат строки — как у стенда: `+<секунды от старта> [<источник>] <текст>`; источники `netcon` (строки без метки времени) и `console.log` (с префиксом `MM/DD HH:MM:SS`).

Общая сводка: ни один из четырёх спайков не блокирует `WinHost`. Основной путь подтверждён для S1 и S2; S3 дал новые факты (ложный READY на меню-карте, тексты ошибок `playdemo`); S4 показал, что кириллица в абсолютном пути ломается, а пробелы нет.

---

## S1 — попадает ли `cs2.exe` (и ffmpeg HLAE) в наш Job Object

**Команда:** `cargo run --release -q -p host --example spikes -- job` (запуск `HLAE.exe -customLoader -noGui -autoStart -noConfig -hookDllPath ... -programPath ...cs2.exe -cmdLine "-insecure -steam -novid -allow_third_party_software -condebug -netconport <порт> -windowed -w 1280 -h 720 +demo_ui_mode 0 +playdemo <имя>"`, HLAE запускается своим процессом и сразу назначается в Job с `KILL_ON_JOB_CLOSE`).

**Наблюдение** (выдержка из `s1_job.log`):

```text
+0.2 [job] в job: PID 44796 HLAE.exe
+0.5 [job] в job: PID 35228 cs2.exe, IsProcessInJob=Some(true), образ C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe
+0.5 [job] в job: PID 33476 injector.exe
+0.5 [job] в job: PID 8028 conhost.exe
+2.9 [job] HLAE.exe завершился: exit code: 0
+3.2 [netcon] подключено
+22.1 [netcon] Host activate: Playing Demo (cs2cinema_1-....-1-1.dem)
+26.1 [netcon] CS2C spike READY
+31.3 [netcon] Finishing recording ... Finishing movie: movie/2026_10_10_16_18_49/advancedfx-....* [636 frames @ 30.000000 == 21.200001 seconds]
+34.5 [app] > quit
+37.0 [job] EOF hlae-stdout
+37.0 [job] EOF hlae-stderr
+37.1 [job] cs2.exe завершился, код Some(0)
+37.6 [job] итог: quit->пустой job Some(2.64) с, осталось в job [], cs2.exe в системе []
S1: PASS — cs2.exe в job: да (PID 35228, на 0.5 с); IsProcessInJob=Some(true); вне job: []; READY: пришёл; ffmpeg.exe в job: нет; выход лаунчера 2.9 с; выход игры 37.1 с; EOF pipe: [hlae-stdout@37.0, hlae-stderr@37.0]; quit->пустой job 2.6 с; остатки: нет
```

Подтверждения из остальных запусков: `cs2.exe` оказывался в job (`IsProcessInJob=Some(true)`, на 0.3–0.5 с) во всех 16 запусках стенда. Дерево в job через 0.5 с: `HLAE.exe`, `cs2.exe`, `injector.exe`, `conhost.exe`. `HLAE.exe` выходит с кодом 0 через 2.4–2.9 с, игра живёт дальше. Pipe-потоки HLAE (`hlae-stdout`, `hlae-stderr`) дают EOF только в момент выхода игры (+37.0 с при выходе лаунчера +2.9 с), то есть `cs2.exe` наследует их. HLAE в stdout/stderr ничего не печатает (в транскрипте только строки EOF, предположение A2 подтверждено).

**ffmpeg HLAE в job — не наблюдалось.** Дубль в стенде не стартовал ffmpeg: `AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: FPS 0.000000 < 1.`, после чего 1272 раза подряд `AFXERROR: Failed writing image for screen recording.` за ~5 с (стенд не выставил `mirv_streams record fps`; это ограничение стенда, а не поломка HLAE, ffmpeg по `ffmpeg.ini` существует и запускается вручную). Записался только `audio.wav`. Отсюда `ffmpeg_in_job = unknown`. По наследованию (`cs2.exe` сам был создан внутри job потомком HLAE) ffmpeg, порождённый `cs2.exe`, должен оказаться в job, но это вывод, а не наблюдение.

**Вердикт:** PASS (по вопросу про `cs2.exe`); ffmpeg — NOT RUN (дубль не дошёл до ffmpeg).
**Выбранный путь:** основной — job владеет деревом, искать `cs2.exe` в `Job::pids()`. Запасной путь (принятие по PID через ToolHelp) остаётся включённым при любом значении (решение D-02).
**Следствие для `WinHost`:**
- Назначать `HLAE.exe` в job сразу после `spawn`; `cs2.exe` приходит в job сам. Гонки не наблюдалось: `cs2.exe` в `pids()` на 0.3–0.5 с.
- Живость игры — по handle `cs2.exe`, найденного в `pids()`; `HLAE.exe` завершается рано и успешно (код 0) и не показатель смерти игры.
- Потоки чтения pipe HLAE не ждать через `join` до смерти игры: EOF приходит только при выходе `cs2.exe` (см. `hlae_pipe_eof`).
- Закрытие: `quit` по netcon, затем ждать пустой job; ffmpeg не гарантирован внутри job, поэтому после выхода игры добавить `close_settle` (RESEARCH A5) и не полагаться на пустоту job как на признак закрытых файлов.

---

## S2 — помогает ли `-afxFixNetCon` и к какому адресу привязан netcon

**Команда:** `spikes -- netcon --fix off --runs 5`, затем `spikes -- netcon --fix on --runs 5` (две подкоманды, чтобы уложиться в бюджет 8 минут). Запуск как в S1; на каждом запуске: время до первого успешного `connect`, `netstat -ano` по порту, `quit` и ожидание пустого job.

**Наблюдение:**

```text
S2: PASS — без флага: 5/5 подключений, медиана 2.7 с; адрес привязки: ["0.0.0.0:56202", "0.0.0.0:56232", "0.0.0.0:56277", "0.0.0.0:56331", "0.0.0.0:56409"]
S2: PASS — с флагом: 5/5 подключений, медиана 2.8 с; адрес привязки: ["0.0.0.0:56469", "0.0.0.0:61286", "0.0.0.0:61330", "0.0.0.0:61368", "0.0.0.0:61412"]

s2_fix_2.log:  +3.2 [job] netstat: TCP 0.0.0.0:61286 0.0.0.0:0 LISTENING 50912
s2_fix_5.log:  +3.2 [app] > quit
               +10.2 [netcon] L4S detection: no.  up_ecn1=bleach_ecn1  down_dscp45=bleach_dscp45
               +15.1 [netcon] Source2Shutdown
               +15.1 [netcon] соединение закрыто
               +33.3 [job] job не опустел за 30 с после quit — TerminateJobObject
               +33.8 [job] итог: quit->пустой job None с, осталось в job [], cs2.exe в системе [50536]
```

Выводы по наблюдению:
- Подключение к netcon на этой машине стабильно и без флага: 5/5, около 3.0 с после старта процесса. С `-afxFixNetCon` то же 5/5 и та же медиана (2.8 против 2.7 с — в пределах шума). Флаг ничего не ухудшает и ничего заметно не улучшает.
- **Порт слушает на `0.0.0.0`, а не на `127.0.0.1`** — во всех 10 запусках (в обоих режимах). Предположение A12 (только loopback) опровергнуто. В строке запуска нет параметра, который это меняет (отдельный параметр привязки не искали и не проверяли).
- `quit` по netcon завершает игру за 1.3–8.4 с до пустого job (при отправке в момент, когда игра ещё грузится, около 8 с). Один запуск из 10 (`s2_fix_5`, с флагом) не уложился в 30 с: `MainLoop returning` и `Source2Shutdown` были на 10–15 с, но процесс висел; стенд завершил job через `TerminateJobObject`, остатков нет. К флагу это отношения не имеет (зависание после `Source2Shutdown`, в самом конце выхода игры), но показывает, что вежливого `quit` недостаточно.

**Вердикт:** PASS
**Выбранный путь:** основной — всегда передавать `-afxFixNetCon` (с флагом не хуже). Netcon-only остаётся (D-04).
**Следствие для `WinHost`:**
- Строка запуска содержит `-afxFixNetCon`. Дедлайн подключения к консоли 90 с с большим запасом (наблюдалось ~3 с); это поймает только настоящее зависание.
- Закрытие: после `quit` ждать до `close` (30 с), затем `Job::terminate()`. Это не запасной редкий путь: он сработал 1 раз из 16 `quit`.
- Модель угроз 01-06: консольный порт доступен по всем интерфейсам без пароля, порт случайный на запуск, сессия живёт десятки секунд; брандмауэр Windows на входящие не проверялся. Минимизировать окно: подключаться сразу, не держать игру без дела, не логировать порт вне `record.log`.

---

## S3 — что печатает консоль при сбоях и теряется ли READY при позднем подключении

**Команда:** `spikes -- failures` (четыре запуска: a — `+playdemo` несуществующей демки; b — пустой `.dem`; c — настоящая демка + `exec` отсутствующего cfg по netcon; e — настоящая демка, netcon подключается через 20 с после её старта). Случай d (поломанный путь ffmpeg HLAE) стендом не воспроизводится; пример AFXERROR-шторма есть в S1.

**Наблюдение** (ключевые строки; полные транскрипты — фикстуры `s3a`, `s3b`, `s3c`, `s3e`):

```text
a (s3a_missing_demo.log)
+9.1  [netcon] DELAYED COMMAND: playdemo "cs2cinema_spike_missing"
+12.3 [netcon] CRealMemoryDemoBuffer::Init: demo file 'cs2cinema_spike_missing.dem' doesn't exist in pathid ''
+12.4 [console.log] 10/10 16:23:04 [Demo] Requesting playback of 'cs2cinema_spike_missing.dem'
+12.4 [console.log] 10/10 16:23:04 [Demo] cs2cinema_spike_missing.dem:  unable to extract addons from file
(игра остаётся на меню-карте ui/xpshop_item до нашего quit на +33.4)

b (s3b_empty_demo.log)
+12.4 [netcon] Executing deferred playdemo: "cs2cinema_spike_empty"
+12.5 [console.log] 10/10 16:23:40 [DemoFile] ReadDemoHeader: cs2cinema_spike_empty.dem has invalid demo header.
+12.5 [console.log] 10/10 16:23:40 [Demo] cs2cinema_spike_empty.dem:  unable to extract addons from file
(игра остаётся на меню-карте до quit на +33.4)

c (s3c_missing_cfg.log)
+12.0 [netcon] CS2C spike READY            <- первый READY, ДО старта демки, на меню-карте
+12.1 [app] > exec cs2cinema_spike_missing_cfg
+12.1 [netcon] exec: couldn't exec '{*}cfg/cs2cinema_spike_missing_cfg.cfg', unable to read file
+12.2 [console.log] 10/10 16:24:15 [InputService] exec: couldn't exec '{*}cfg/cs2cinema_spike_missing_cfg.cfg', unable to read file
+12.4 [console.log] 10/10 16:24:15 [Demo] Requesting playback of 'cs2cinema_1-....dem'
+14.3 [netcon] Host activate: Playing Demo (cs2cinema_1-....dem)
+18.2 [netcon] CS2C spike READY            <- второй READY, на демке

e (s3e_late_connect.log)
+14.1 [console.log] ChangeGameUIState: CSGO_GAME_UI_STATE_LOADINGSCREEN -> CSGO_GAME_UI_STATE_INGAME
+14.1 [console.log] Host activate: Playing Demo (cs2cinema_1-....dem)
+14.2 [app] поздний netcon: демка пошла, ждём 20 с до подключения
+34.3 [netcon] подключено
+34.3 [app] > mirv_cmd clear; mirv_cmd addAtTick 64 "echo CS2C spike READY"
(30 с тишины: READY не пришёл)
+64.4 [app] > echo CS2C spike READY2
+64.4 [netcon] CS2C spike READY2
```

Выводы:
1. **Несуществующая и битая демка не роняют игру и не дают явной ошибки в виде выхода.** CS2 остаётся на меню-карте. В `console.log` и в netcon есть опознаваемые строки: общая для обоих случаев `[Demo] <имя>.dem:  unable to extract addons from file`; для отсутствующего файла дополнительно `CRealMemoryDemoBuffer::Init: demo file '<имя>.dem' doesn't exist in pathid ''`; для битого/пустого — `[DemoFile] ReadDemoHeader: <имя>.dem has invalid demo header.` (в абсолютном пути без файла — `ReadDemoHeader: invalid pBuffer.`). Строка `Host activate: Playing Demo (...)` в этих случаях не появляется. Ошибка видна через ~3 с после запроса (около +12.4 с от старта процесса). Игра сама не завершается: без дедлайна задача висела бы.
2. **READY от `mirv_cmd addAtTick 64` не доказывает, что демка идёт.** В запуске c команда была отправлена на +3.2 с, меню-карта `ui/xpshop_item` стала активной на +10.5 с, `playdemo` выполнился на +12.2 с, и 64-й тик меню-карты наступил раньше, чем загрузилась демка, поэтому READY пришёл на +12.0 с. Команда не стёрлась при смене карты и сработала ещё раз на +18.2 с на демке. В S1 (команда на +5.5 с, меню-карта активна на +19.2 с, демка запрошена на +19.9 с) ложного READY не было — всё зависит от того, сколько меню-карта прожила. Нужно принимать READY как «демка идёт» только после строки `Host activate: Playing Demo (` / `[Demo] playing demo from` (netcon или `console.log`).
3. **Подтверждено A10: при позднем подключении READY теряется.** Демка пошла на +14.1 с, netcon подключился на +34.3 с, `addAtTick 64` не сработал за следующие 30 с. Прямой `echo` отвечает сразу, то есть по netcon можно просить READY напрямую, когда демка уже идёт. Это и есть схема 01-04.
4. **Подтверждено A9:** отсутствующий cfg печатает `exec: couldn't exec '{*}cfg/<имя>.cfg', unable to read file` и в netcon (без префикса), и в `console.log` с префиксом `[InputService]`. Строка `{*}cfg/gamemode_competitive_server.cfg` с тем же текстом приходит при каждом старте демки и не говорит об ошибке пользователя — шаблон должен включать имя нашего cfg.
5. Подтверждено A4: netcon-строки идут без префикса времени, по одной метке на строку; `console.log` — с префиксом `MM/DD HH:MM:SS`.

**Вердикт:** PASS (a, b, c, e выполнены; d не воспроизводился).
**Выбранный путь:** основной — парсер опирается на нонс-метки; тексты сбоев используются как необязательные шаблоны быстрой остановки (не обязательные для фазы 1, но фикстуры готовы), дедлайны остаются главной защитой. Запасной вариант для READY («ложное/потерянное») включается всегда.
**Следствие для `WinHost` и машины 01-04:**
- READY принимать только вместе с подтверждением «демка идёт» (строка `Host activate: Playing Demo` или `[Demo] playing demo from`), а при подтверждении демки без READY — самим отправить `echo <READY-метка>` напрямую. При позднем подключении так же.
- `DEMO_NOT_LOADED` по дедлайну остаётся обязательным; в сообщении об ошибке показывать `unable to extract addons from file` / `doesn't exist` / `invalid demo header`, если они есть в хвосте.
- Не считать ошибкой `couldn't exec '{*}cfg/gamemode_competitive_server.cfg'`.

---

## S4 — переживает ли `+playdemo` абсолютный путь через `-cmdLine`

**Команда:** `spikes -- abspath` (путь `C:/Users/USER/AppData/Local/Temp/cs2cinema spike демо/демо.dem` в кавычках внутри `-cmdLine`; пробелы и кириллица). Дополнительно, **вне стенда** (скрипт в scratchpad, запускал `HLAE.exe` напрямую, своего `cs2.exe` закрыл по PID): тот же запуск с ASCII-путём с пробелами `C:/Users/USER/AppData/Local/Temp/cs2cinema spike ascii/demo.dem`.

**Наблюдение:**

```text
(стенд, пробелы + кириллица)
+4.2  [netcon] -insecure ... +playdemo C:/Users/USER/AppData/Local/Temp/cs2cinema spike ����/����.dem
+12.3 [netcon] CRealMemoryDemoBuffer::Init: demo file 'C:/Users/USER/AppData/Local/Temp/cs2cinema spike ����/����.dem' doesn't exist in pathid ''
+12.3 [netcon] ReadDemoHeader: invalid pBuffer.
S4: FAIL — «Host activate: Playing Demo» за 90 с: нет, остатки: нет

(вручную, ASCII + пробелы)
[Demo] Requesting playback of 'C:/Users/USER/AppData/Local/Temp/cs2cinema spike ascii/demo.dem'
[HostStateManager] Host activate: Playing Demo (C:/Users/USER/AppData/Local/Temp/cs2cinema spike ascii/demo.dem)   (через ~17 с после старта)
```

Вывод: пробелы в кавычках через `-cmdLine` переживают; кириллица в пути — нет: игра получает искажённые байты (`����`) и сообщает, что файла нет. Причина, по-видимому, в кодировке командной строки между HLAE и игрой (не разбиралась). Для имён, которые генерирует приложение (`cs2cinema_<uuid>.dem` — ASCII), абсолютный путь возможен, если папка профиля пользователя тоже ASCII; при нелатинском имени профиля он сломается.

**Вердикт:** FAIL (для пути с кириллицей; для ASCII-пути с пробелами работает)
**Выбранный путь:** запасной — копирование демки в `game\csgo` и удаление после записи остаётся (TMO-06). Не блокирует фазу 1: `install_files` не меняется.
**Следствие для `WinHost`/фазы 2:** отказ от копирования в фазе 2 возможен только с проверкой «путь ASCII», иначе копия. Строка запуска фазы 1 остаётся на имени файла.

---

## Решения для 01-06

| Ключ | Значение | Основание | Комментарий |
|------|----------|-----------|-------------|
| `cs2_in_job` | `yes` | S1 PASS: 16 из 16 запусков `IsProcessInJob=Some(true)` на 0.3–0.5 с | Искать `cs2.exe` в `Job::pids()`; принятие по PID остаётся включённым как запасной путь |
| `ffmpeg_in_job` | `unknown` | S1: дубль не дошёл до ffmpeg (`FPS 0.000000 < 1`) | Закрытие: выход игры плюс `close_settle`; не считать пустой job признаком закрытых файлов. Вывод по наследованию — `yes`, но не наблюдался |
| `fix_netcon` | `on` | S2: 5/5 и 5/5, медианы 2.8 и 2.7 с | Передавать `-afxFixNetCon` всегда; флаг безвреден |
| `netcon_bind` | `0.0.0.0` | S2: `netstat` показал `0.0.0.0:<порт> LISTENING` в 10 из 10 запусков | Порт доступен по всем интерфейсам без пароля; случайный порт, короткая сессия, отметить в модели угроз 01-06 |
| `hlae_pipe_eof` | `at-game-exit` | S1: HLAE вышел на +2.9 с, EOF обоих pipe на +37.0 с вместе с выходом игры | Потоки чтения pipe не ждать через `join` до смерти игры; остановка читателей — по закрытию handle/выходу игры |
| `late_connect_ready` | `lost` | S3e: READY по `addAtTick 64` не пришёл за 30 с после позднего подключения; прямой `echo` отвечает | Машина 01-04 сама просит READY напрямую, когда демка идёт. Доп. находка: READY может прийти раньше демки (S3c) — принимать только после `Host activate: Playing Demo` |
| `abs_playdemo` | `fails` | S4: путь с кириллицей искажается; ASCII + пробелы — `works` (проверено вручную) | В фазе 1 копирование остаётся; для фазы 2 (TMO-06) допустим абсолютный путь только если он ASCII |

## Дополнительные находки (для 01-04, 01-06, 01-08)

1. Ложный READY на меню-карте (S3c): подробности и правило приёма — в S3, вывод 2. Затрагивает `crates/pipeline/src/record` (машина состояний) и `late_connect_ready`.
2. Игра при несуществующей или битой демке не завершается и остаётся на меню-карте `ui/xpshop_item`: единственная защита — дедлайн `DEMO_NOT_LOADED`; шаблоны `unable to extract addons from file`, `doesn't exist in pathid`, `invalid demo header` годятся для быстрой остановки (LCH-05, фаза 2).
3. Зависший выход: 1 из 16 `quit` не завершил игру за 30 с после `Source2Shutdown` (S2, `s2_fix_5`); сработал `TerminateJobObject`. В 01-06 закрытие обязано заканчиваться `Job::terminate()`, а не бесконечным ожиданием.
4. Форма AFXERROR-шторма (S1): одна строка `AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: FPS 0.000000 < 1.` и затем ~1272 одинаковых `AFXERROR: Failed writing image for screen recording.` за ~5 с. Подходит как синтетический тест для свёртки хвоста (01-08, `tail_folds_repeats_and_keeps_the_first_afxerror`).
5. Ограничения стенда (план 01-01, файл `crates/host/examples/spikes.rs`; в этом плане не править): (а) S1-дубль не выставляет `mirv_streams record fps`, поэтому ffmpeg не стартует; (б) временная папка `cs2cinema spike демо` остаётся после S4 (`remove_dir` вызывается раньше, чем отрабатывает `Cleanup`) — я удалил её вручную (пустая); (в) в итоговых строках S3a/S3b поле «улики» совпадает с шумом (строка запуска, SDR) и ничего не доказывает — верные строки приведены выше.

## Стартовые дедлайны на проверку

Таблица `Deadlines::default()` из 01-04 против наблюдённых времён этой машины (время от старта `HLAE.exe`, если не указано иное).

| Дедлайн | Значение по умолчанию | Наблюдалось | Замечание |
|---------|-----------------------|-------------|-----------|
| лаунчер (HLAE.exe завершился) | 30 с | 2.4–2.9 с | запас большой |
| игра (cs2.exe появился) | 20 с | 0.3–0.5 с | запас большой |
| консоль (netcon подключён) | 90 с | около 3.0 с (S2: медиана 2.7–2.8 с) | запас большой |
| демка (Host activate: Playing Demo) | 150 с | 14.3–22.4 с | холодный старт; на медленном диске/после обновления игры может вырасти, запас 7–10 раз |
| скрипт (ответ на exec/echo) | 30 с | меньше 0.2 с | запас большой |
| первый дубль | 90 с | не наблюдалось | стенд не запускал настоящий дубль |
| перемотка | 60 с | не наблюдалось | |
| дубль | игровые секунды × max(1, fps/60) × 2 + 30 с | не наблюдалось | настоящий дубль не снимался |
| закрытие (quit → пустой job) | 30 с | 1.3–8.4 с; один раз не уложились (зависание после Source2Shutdown) | дедлайн оправдан, после него `Job::terminate()` |

Строки, которые эти измерения не закрывают (первый дубль, перемотка, дубль): они проверяются в фазе 4 на настоящих записях.
