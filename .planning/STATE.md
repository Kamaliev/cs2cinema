---
gsd_state_version: "1.0"
milestone: v1.1
milestone_name: Сервер и Telegram-бот
current_phase: 07
current_phase_name: Telegram-бот и доставка
status: executing
stopped_at: Completed 07-01-PLAN.md
last_updated: "2026-10-11T19:47:23.752Z"
last_activity: 2026-10-12
last_activity_desc: Phase 07 execution started
state_head: 800a122400901b7a240b4b7a195b08a8eabad1f7
progress:
  total_phases: 5
  completed_phases: 2
  total_plans: 27
  completed_plans: 17
  percent: 40
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-11)

**Core value:** Задача на монтаж доходит до готового `highlights.mp4` без ручного вмешательства, а если это невозможно — останавливается быстро и объясняет, что именно пошло не так. Для v1.1 то же верно от демки, отправленной боту, до ролика в чате.
**Current focus:** Phase 07 — Telegram-бот и доставка

## Current Position

Phase: 07 (Telegram-бот и доставка) — EXECUTING
Plan: 2 of 11
Status: Ready to execute
Last activity: 2026-10-12 — Phase 07 execution started

Progress: [████░░░░░░] 40%

## Performance Metrics

**Velocity:**
- Total plans completed: 32
- Average duration: -
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 02 | 8 | - | - |
| 03 | 8 | - | - |
| 06 | 11 | - | - |
| 05 | 5 | - | - |

**Recent Trend:**
- Last 5 plans: -
- Trend: -

*Updated after each plan completion*
**Per-Plan Metrics:**

| Plan | Duration | Tasks | Files |
|------|----------|-------|-------|
| Phase 01 P01 | 11 min | 3 tasks | 10 files |
| Phase 01 P02 | 10 min | 2 tasks | 3 files |
| Phase 01 P03 | 12 min | 3 tasks | 1 files |
| Phase 01 P04 | 35 min | 2 tasks | 9 files |
| Phase 01 P05 | 25min | 3 tasks | 5 files |
| Phase 01 P06 | 11min | 3 tasks | 11 files |
| Phase 01 P08 | 25min | 3 tasks | 6 files |
| Phase 01 P07 | 25 min | 3 tasks | 6 files |
| Phase 02 P01 | 10 min | 3 tasks | 9 files |
| Phase 02 P02 | 8min | 2 tasks | 2 files |
| Phase 02 P03 | 25 min | 2 tasks | 9 files |
| Phase 02 P04 | 35 min | 3 tasks | 8 files |
| Phase 02 P05 | 25 min | 2 tasks | 4 files |
| Phase 02 P06 | 35 min | 3 tasks | 7 files |
| Phase 02 P07 | 25 min | 3 tasks | 4 files |
| Phase 03 P01 | 25min | 3 tasks | 5 files |
| Phase 03 P02 | 25min | 2 tasks | 6 files |
| Phase 03 P03 | 35min | 3 tasks | 8 files |
| Phase 03 P04 | 50min | 3 tasks | 6 files |
| Phase 03 P05 | 20min | 2 tasks | 1 files |
| Phase 03 P06 | 40min | 3 tasks | 1 files |
| Phase 03 P07 | 35min | 2 tasks | 3 files |
| Phase 04 P01 | 25min | 2 tasks | 9 files |
| Phase 04 P02 | 15min | 2 tasks | 3 files |
| Phase 04 P03 | 25min | 3 tasks | 6 files |
| Phase 04 P04 | 75min | 3 tasks | 8 files |
| Phase 04 P05 | 35 min | 2 tasks | 7 files |
| Phase 04 P06 | 12 min | 2 tasks | 3 files |
| Phase 04 P07 | 10 min | 2 tasks | 8 files |
| Phase 04 P08 | 25 min | 2 tasks | 6 files |
| Phase 04 P09 | 13min | 3 tasks | 4 files |
| Phase 04 P10 | 35min | 3 tasks | 9 files |
| Phase 04 P11 | n/a | 2 tasks | 7 files |
| Phase 05 P01 | 35min | 3 tasks | 20 files |
| Phase 05 P02 | 40min | 2 tasks | 12 files |
| Phase 05 P03 | 45min | 3 tasks | 11 files |
| Phase 05 P04 | 80m | 3 tasks | 12 files |
| Phase 05 P05 | 1h30m | 3 tasks | 9 files |
| Phase 06 P01 | 25 min | 2 tasks | 4 files |
| Phase 06 P02 | 40min | 2 tasks | 6 files |
| Phase 06 P03 | 15min | 2 tasks | 5 files |
| Phase 06 P04 | 15min | 2 tasks | 5 files |
| Phase 06 P05 | 25min | 2 tasks | 9 files |
| Phase 06 P06 | 55min | 2 tasks | 5 files |
| Phase 06 P07 | 40 min | 2 tasks | 6 files |
| Phase 06 P08 | 70min | 2 tasks | 7 files |
| Phase 06 P09 | 60 min | 3 tasks | 9 files |
| Phase 06 P10 | 75min | 3 tasks | 10 files |
| Phase 07 P01 | 35min | 2 tasks | 10 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- [v1.1 Roadmap]: 5 фаз (5–9), нумерация продолжается после v1.0; порядок по исследованию — ядро сервера и протокол → раннер, защита входа и первый живой прогон → Telegram-бот и доставка → режим раннера в приложении → уборка, развёртывание и учения
- [v1.1 Roadmap]: SEC-01, SEC-02, SEC-03 (инъекция текста из демки в cfg, нет лимитов разбора, `find_player` по подстроке) закрываются в фазе 6; фаза 7 зависит от фазы 6, чужие демки принимаются только после защиты входа
- [v1.1 Roadmap]: SEC-04 (токены не в журналах) вводится в фазе 5, где появляются токены раннеров, и перепроверяется в фазе 7 (живой бот) и на учениях фазы 9
- [v1.1 Roadmap]: фаза 7 может начаться параллельно с фазой 6 сразу после фазы 5 (на `cs2-server submit` и фейковом раннере), но закрывается после фазы 6; фаза 8 может идти параллельно с фазой 7
- [v1.1 Roadmap]: первый живой прогон настоящей демки через сервер и настоящий CS2 (фаза 6) неформально закрывает долг v1.0 по живой проверке записи; приёмка 04-12 не возобновляется
- Roadmap: 4 фазы (coarse), вертикальные срезы; после каждой фазы приложение записывает и остаётся рабочим
- Phase 1: приложение переходит на `pipeline::record` уже в первой фазе; `cs2-cli` переводится на общий код и старый `launch.rs` удаляется в Phase 4
- Phase 2: свои процессы держатся в Job Object, запасной вариант — (PID, время создания); выбор подтверждается спайком в Phase 1
- [Phase 01]: Фейк host: задержка метки сдвигает всю цепочку, убранная метка не приходит, но файлы дубля остаются; отклонённые команды не попадают в sent()
- [Phase 01]: Стенд спайков: при запущенном cs2.exe отказ (код 2); без --fix запуски netcon чередуются off/on (5+5 = два запуска)
- [Phase 01]: 01-02: старый путь записи ищет TAKE_END с nonce плана; plan_hash нормализует nonce в NONCE
- [Phase 01]: 01-03: колонки jobs — six-columns: к четырём fail_* D-05 добавлены fail_elapsed_ms и phase вторым шагом миграции (user_version = 2)
- [Phase 01]: Метка не по порядку (включая ALL_DONE вместо TAKE_END) не меняет состояние и дедлайн машины записи, а только попадает в текст сбоя; повтор принятой метки молча игнорируется
- [Phase 01]: Тайминги дублей отдаёт Machine::timings(); Event::TakeDone остаётся {n, m}, чтобы строка «записан дубль N/M» для status.rs не менялась
- [Phase 01]: exec highlights уходит один раз; прямой READY запрашивается один раз через ready_grace, если демка уже идёт
- [Phase 01]: 01-05: cs2.exe попадает в job через HLAE, netcon слушает 0.0.0.0, READY принимать только после Host activate: Playing Demo, abs_playdemo=fails для кириллицы
- [Phase 01]: READY и любая метка задачи до подтверждения демки отбрасываются (Note), а не удерживаются; потерянный READY возвращает машина через ready_direct
- [Phase 01]: abs_playdemo=fails: WinHost::launch отвергает не-ASCII/пробельные имена демки типизированной ошибкой Spawn до запуска; копия в game/csgo остаётся за приложением (01-07)
- [Phase 01]: закрытие всегда заканчивается Job::terminate(); поток наблюдения живёт 3 с после terminate; netcon не переподключается после смерти игры
- [Phase 01]: netcon на 0.0.0.0 без пароля: хост подключается к 127.0.0.1 на случайный порт, строки недоверенные и не исполняются, остаточный риск записан в SUMMARY 01-06
- [Phase 01]: 01-08: HLAE_ERROR срабатывает на любом истёкшем дедлайне при увиденной AFXERROR; закрепляются первая AFXERROR, самая частая повторяющаяся AFXERROR и последняя метка
- [Phase 01]: 01-08: record.log и хвост консоли очищают управляющие символы, профиль пользователя и секреты (недоверенный netcon)
- [Phase 01]: 01-07: Status.take_now (разбор «запись: дубль N из M») даёт лестнице «дубль N/M» с начала дубля; файлы app форматировать только rustfmt с max_width=160,use_small_heuristics=Max, а не cargo fmt -p app
- [Phase 02]: 02-01: свои игры, пережившие сессию, живут в static OWN_GAMES с открытым handle; у Host нет завершения по PID или имени
- [Phase 02]: 02-02:set_cancelling-меняет-только-running-(T-02-07)
- [Phase 02]: Предпроверка: своя застрявшая игра исключается из проверки чужого CS2 по PID; код сбоя по приоритету PREV_GAME_STUCK > FOREIGN_GAME_RUNNING > PRECHECK_FAILED
- [Phase 02]: StateFlags Steam: идущее обновление и битые файлы - fail, ожидающее обновление и нераспознанное - warn; сборка CS2 сравнивается по buildid
- [Phase 02]: Режим закрытия игры по коду сбоя: успех/GAME_EXITED без ожидания, HLAE_ERROR с дочиткой строк 1 с, остальное с quit
- [Phase 02]: Окна опрашиваются в состояниях Launcher..Take; мгновенно роняет только диалог #32770 с afxhook/hlae/afxerror
- [Phase 02]: Сначала wait_previous_game (D-14), потом снимок процессов: своя зависшая игра не должна выглядеть чужим CS2
- [Phase 02]: hlae_ffmpeg: Path= из ffmpeg.ini берётся как есть, без ini - ffmpeg/bin/ffmpeg.exe, иначе None
- [Phase 02]: Второй проход места делается в ветке «дубли ещё не записаны»: задача с одной склейкой не блокируется нехваткой места под запись
- [Phase 02]: recover помечает дубли .partial только при существующем installed.json: без него запись убрана после успеха и дубли целые
- [Phase 02]: Файлы удаляются из папки игры только по installed.json через is_ours (папка CS2 из настроек, канонизация, шаблоны имён)
- [Phase 02]: 02-07: пауза очереди после отмены хранится только в памяти; «Продолжить», новая задача и «Повторить» снимают её
- [Phase 02]: 02-07: кнопка «Проверить окружение» проверяет значения черновика настроек и неактивна во время задачи
- [Phase 03]: 03-01: прерванная задача — статус interrupted, stage остаётся, phase -> fail_state; пауза очереди с PauseReason
- [Phase 03]: kill_if_same: выход процесса проверяется до чтения образа (у вышедшего образ не читается); завершение по PID только через Host::kill_orphan(&ProcIdentity)
- [Phase 03]: 03-03: звук проверяется в audio.wav, разрешение и видео в video.mp4; допуск длительности ±10% но не меньше ±0.5 с; NotReady отличается от Invalid по тексту stderr ffprobe
- [Phase 03]: 03-04: проверки дублей идут по одной за шаг цикла драйвера; после закрытия игры оставшиеся дубли проверяются по разу без повторов
- [Phase 03]: 03-06: повтор с невалидным дублем записывает все дубли заново, прежний clips уходит в clips_old_<ts> (D-07, запасной вариант)
- [Phase 04]: 04-01: cs2-cli assemble принимает папку задачи (record.json + clips/manifest.json); старая склейка renderer::assemble и флаги --plan/--clips удалены (DC-1); RunError переехала в pipeline
- [Phase 04]: 04-02: бюджет авто-повтора — колонка auto_retries, обнуляется ручным Повторить/Заново; партия приёмки — batch + set_batch; GAME_EXITED до Take — класс «запуск»
- [Phase 04]: 04-03: Host::notify через Shell_NotifyIconW на отдельном потоке (DC-6); A1 и A3 спайка не проверены, кириллица не подтверждена, A2 подтверждено; 04-08 не опирается на A1, D6 в 04-11 проверяет A1/A3
- [Phase 04]: 04-04: граница pipeline без Store — Progress::artifact, JobSpec/JobConfig; Debug JobConfig без ключа FACEIT
- [Phase 04]: 04-05: record_request принимает Verify обязательно — запись через pipeline::job всегда с проверкой дублей (DC-1 закреплён типом)
- [Phase 04]: cs2-cli retry отвергает флаги плана (--player/--out/--size/--fps): план берётся из record.json
- [Phase 04]: 04-07: launch.rs удалён; recover_after_crash в app::jobs, find_cs2/find_hlae в app::settings, no_window в app::ui, old_logs в app::app
- [Phase 04]: 04-08: очередь в потоке cs2cinema-queue; Cmd только Resume (Wake не нужен), Resume действует и сразу в виде, и по порядку в потоке
- [Phase 04]: 04-09: политика очереди чистая (QueuePolicy); сбой «запуск» повторяется один раз, два одинаковых сбоя «окружение» (и «запуск» без бюджета) — пауза с объяснением; итог очереди объявляется и при паузе без ждущих задач
- [Phase 04]: 04-10: ручные действия приёмки выводятся только из журнала (First/Auto) и attempt == 1 + auto_retries; серия B доказывается другим PID в имени партии и постановкой после конца серии A
- [Phase 04]: 04-10: acceptance check/state/job читают базу только Store::open_read_only; ничего не удаляют и не завершают
- [Phase 04]: 04-11: учения D1-D6 не проводились, приняты по указанию автора без улик; ACC-03 остаётся Pending, A1/A3/кириллица в трее не проверены
- [Phase 05]: Настройки сервера — JSON-файл без крейта toml; время в базе — unix-мс по часам сервера; пути хранилища только из целых id
- [Phase 05]: tiny_http 0.12 подтверждён как HTTP-стек (D-01): тесты протокола проходят, axum не нужен
- [Phase 05]: http::start_with_flag: общий флаг остановки у Ctx и HttpServer; Core::claim без фильтра max_grants (потолок — в плане аренд)
- [Phase 05]: 05-03: срок аренды <= now считается истёкшим; last_seq/phase/take сбрасываются при новой выдаче; потеря аренды обратима и не считается сбоем
- [Phase 05]: 05-04: итог done по HTTP не принимается до 05-05; FinishKind::Done и таблица переходов готовы
- [Phase 05]: 05-04: claim освобождает прежнюю потерянную аренду того же раннера (раннер не ведёт две задачи)
- [Phase 05]: 05-04: finish на окончательной задаче — повтор того же итога 200, иначе job_terminal
- [Phase 05]: 05-05: Chunk ставит phase=upload, Touch делегирует в Reattach; Reject несёт заголовки; highlights.mp4 удаляется, если вторая транзакция finish done отказала
- [Phase 06]: Слияние двойников SteamID применяется на каждом ярусе resolve_player; find_player остаётся обёрткой resolve_player(..).ok()
- [Phase 06]: JOB_ERROR — только строка в wire::fail, в FailCode его нет; Phase::Precheck добавлен аддитивно (ALL = 10)
- [Phase 06]: 06-02: текст из демки очищается в Match::build и не пишется в cfg вовсе; подписи drawtext читаются из cap_NNN.txt
- [Phase 06]: 06-03: стартовые лимиты чтения демки — файл 2000 МиБ, распакованная 3 ГиБ; команда демки до 128 МиБ после snappy; read_demo/decompress не менялись
- [Phase 06]: 06-04: самоограничение памяти через самоприсвоение Job Object без KILL_ON_JOB_CLOSE; лимит задаёт вызывающий
- [Phase 06]: 06-05: job_id раннера = номер задачи на сервере; демка всегда remote/srv{id}.dem, orig_name в путях не участвует
- [Phase 06]: 06-05: JOB_ERROR и BAD_DEMO в runner пока локальные константы, 06-09 заменит на wire::fail
- [Phase 06]: 06-06: TransferEnd обобщён (Done(JobState) для загрузки результата); Dropped у демки = повреждена при передаче (BAD_DEMO), Err = сбой связи
- [Phase 06]: 06-06: cancel из hello сохраняет итог Cancelled (Reporting) и уходит finish, файлы задачи убираются после ответа сервера
- [Phase 06]: 06-06: out/srv{id} пишется в runner_state.json до первого запуска исполнителя; первый запуск без resume, после перезапуска resume=true
- [Phase 06]: Тексты PLAYER_AMBIGUOUS и PLAYER_NOT_FOUND родитель собирает сам из очищенных кандидатов ребёнка
- [Phase 06]: Не запустившийся ребёнок (Spawn и прочее, кроме таймаута и отмены) даёт PRECHECK_FAILED, а не BAD_DEMO
- [Phase 06]: Недоступный файл демки остаётся строкой с путём без типизации: это неисправность вызывающего
- [Phase 06]: Uplink::start принимает Timing вместо двух Duration: семь аргументов укладываются в лимит clippy
- [Phase 06]: Токен отозван (401): раннер останавливается с сообщением, задача остаётся в состоянии; stale_epoch и job_terminal бросают задачу без finish
- [Phase 06]: Остановка раннера не отмена: серверная отмена важнее локальной остановки, при одной остановке задача остаётся в Executing
- [Phase 06]: Строка эскалации мягкой отмены пишется один раз через inner.log; пульс не запускается на этапе Reporting
- [Phase 06]: 06-09: release всегда засчитывает выдачу (counts = true, D-02 буквально); потолок — TOO_MANY_GRANTS сервера, пауза раннера — после двух одинаковых сбоев окружения или запуска
- [Phase 06]: 06-09: отмена сервера проверяется до release (обход WR-01), решение о release пишется в runner_state.json (pending_release) и повторяется после перезапуска
- [Phase 06]: 06-10: сверка SHA-256 результата перед загрузкой только при sha_retries == 0; после отказа сервера решает сервер
- [Phase 06]: 06-10: Runner::run возвращает Result — Err при отклонённом токене и несовпадении протокола (код выхода cs2-cli runner 1)
- [Phase 06]: 06-10: токен раннера только печатные ASCII-знаки (не-ASCII в Authorization давал вечные повторы)
- [Phase 07]: 07-01: копия tiny_http 0.12.0 в vendor/ с ограниченным дочитыванием тела (CR-01); MAX_BODY = CHUNK_SIZE + 64 КиБ
- [Phase 07]: 07-01: Event::Release отменённой задачи даёт Cancelled (WR-01); Connection: close в ответе tiny_http игнорируется

### Pending Todos

None yet.

### Blockers/Concerns

- Phase 5: модель доступа к БД сервера (поток-актор или `Arc<Mutex<Connection>>`) и поведение `tiny_http` под `PUT` по 8 МиБ, `Expect: 100-continue` и обрывы нужно решить коротким спайком в начале фазы
- Phase 6: изоляция разбора демки в дочернем процессе с лимитом памяти (Job Object) не опробована; значения дедлайнов записи из v1.0 (консоль ~90 с, простой дубля ~60 с) и допуск `ffprobe` не подстроены на живом прогоне
- Phase 7: поведение локального `telegram-bot-api` (общий том и права, `getFile` с абсолютным путём, `sendVideo` по `file://` на ~1 ГБ) требует спайка; приём ссылок FACEIT подтвердить с автором (риск SSRF)
- Не выбраны ОС сервера (исследование рекомендует Linux + Tailscale Serve) и способ доступа к API раннера (Tailscale или Caddy); влияет на документацию фазы 9, не на код
- В REQUIREMENTS.md число требований v1.1 было записано как 41, фактически их 37; счётчик исправлен при создании roadmap

### Quick Tasks Completed

| # | Description | Date | Commit | Directory |
|---|-------------|------|--------|-----------|
| 261011-hb5 | Фикс проверки стабильности дубля: пересверка размера после ffprobe и сверка validated-дублей с диском после закрытия игры | 2026-10-11 | e1be156 | [261011-hb5-fiks-proverki-stabilnosti-dublya-perepro](./quick/261011-hb5-fiks-proverki-stabilnosti-dublya-perepro/) |

## Deferred Items

Items acknowledged and deferred at milestone close, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| *(none)* | | | | |

## Session Continuity

Last session: 2026-10-11T19:47:23.668Z
Stopped at: Completed 07-01-PLAN.md
Resume file: None
