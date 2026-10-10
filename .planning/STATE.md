---
gsd_state_version: "1.0"
current_phase: 02
current_phase_name: Предпроверка, отмена и только свои процессы
status: executing
stopped_at: Completed 02-04-PLAN.md
last_updated: "2026-10-10T16:14:47.016Z"
last_activity: 2026-10-10
last_activity_desc: Phase 02 execution started
state_head: 1b79ba64d5a8c9324d7980b773d0a613df4cf0f0
progress:
  total_phases: 4
  completed_phases: 0
  total_plans: 16
  completed_plans: 12
  percent: 0
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-10)

**Core value:** Задача на монтаж из очереди доходит до готового `highlights.mp4` без ручного вмешательства, а если это невозможно — останавливается быстро и объясняет, что именно пошло не так.
**Current focus:** Phase 02 — Предпроверка, отмена и только свои процессы

## Current Position

Phase: 02 (Предпроверка, отмена и только свои процессы) — EXECUTING
Plan: 5 of 8
Status: Ready to execute
Last activity: 2026-10-10 — Phase 02 execution started

Progress: [░░░░░░░░░░] 0%

## Performance Metrics

**Velocity:**
- Total plans completed: 0
- Average duration: -
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| - | - | - | - |

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

Last session: 2026-10-10T16:14:46.910Z
Stopped at: Completed 02-04-PLAN.md
Resume file: None
