---
gsd_state_version: "1.0"
current_phase: 04
current_phase_name: Очередь без присмотра и приёмка
status: executing
stopped_at: Completed 04-03-PLAN.md
last_updated: "2026-10-11T07:13:47.903Z"
last_activity: 2026-10-11
last_activity_desc: Phase 04 execution started
state_head: 6c48abe329c035409fbb9b0a971354d04f2b0b0b
progress:
  total_phases: 4
  completed_phases: 2
  total_plans: 36
  completed_plans: 27
  percent: 50
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-10)

**Core value:** Задача на монтаж из очереди доходит до готового `highlights.mp4` без ручного вмешательства, а если это невозможно — останавливается быстро и объясняет, что именно пошло не так.
**Current focus:** Phase 04 — Очередь без присмотра и приёмка

## Current Position

Phase: 04 (Очередь без присмотра и приёмка) — EXECUTING
Plan: 4 of 12
Status: Ready to execute
Last activity: 2026-10-11 — Phase 04 execution started

Progress: [█████░░░░░] 50%

## Performance Metrics

**Velocity:**
- Total plans completed: 16
- Average duration: -
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 02 | 8 | - | - |
| 03 | 8 | - | - |

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

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

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

### Pending Todos

None yet.

### Blockers/Concerns

- Phase 1: поведение HLAE и CS2 не документировано (наследование Job Object через загрузчик HLAE, `-afxFixNetCon`, тексты консоли при сбоях, абсолютный `+playdemo`); это решается спайками в начале фазы
- Phase 2: последовательность вызовов Job Object + IOCP в `windows-sys` 0.61 ещё не опробована; семантика `StateFlags` в `appmanifest_730.acf` не проверена
- Phase 4: значения дедлайнов (консоль ~90 с, простой дубля ~60 с) и допуск `ffprobe` нужно подстроить на реальном прогоне
- 01-05: реальные спайки S1-S4 требуют закрыть запущенный CS2 (PID 48660 на момент 01-01); стенд crates/host/examples/spikes.rs отказывается стартовать при чужом cs2.exe

## Deferred Items

Items acknowledged and deferred at milestone close, most recent first:

| Category | Item | Status | Deferred At | Milestone |
|----------|------|--------|-------------|-----------|
| *(none)* | | | | |

## Session Continuity

Last session: 2026-10-11T07:13:47.686Z
Stopped at: Completed 04-03-PLAN.md
Resume file: None
