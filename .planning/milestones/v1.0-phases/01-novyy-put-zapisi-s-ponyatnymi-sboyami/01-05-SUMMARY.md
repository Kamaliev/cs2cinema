---
phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami
plan: 05
subsystem: infra
tags: [spikes, hlae, cs2, job-object, netcon, fixtures]

requires:
  - phase: 01-01
    provides: "стенд спайков crates/host/examples/spikes.rs на настоящем Job"
provides:
  - "01-SPIKES.md: вердикты S1-S4 (S1 PASS, S2 PASS, S3 PASS, S4 FAIL для кириллицы) и таблица «Решения для 01-06» с семью ключами"
  - "фикстуры консоли crates/pipeline/tests/fixtures/s3a_missing_demo.log, s3b_empty_demo.log, s3c_missing_cfg.log, s3e_late_connect.log без путей профиля и SteamID"
  - "наблюдённые тексты сбоев playdemo/exec и времена запуска для проверки Deadlines::default()"
affects: [01-06, 01-07, 01-08]

actuals:
  tokens: 14500
  tasks: 3
  commits: 1
plan_head_before: 2d9f2451d492fe666c3e0348f2fa4851a2940d87
plan_head_after: d60be89

key-files:
  created:
    - .planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md
    - crates/pipeline/tests/fixtures/s3a_missing_demo.log
    - crates/pipeline/tests/fixtures/s3b_empty_demo.log
    - crates/pipeline/tests/fixtures/s3c_missing_cfg.log
    - crates/pipeline/tests/fixtures/s3e_late_connect.log
  modified: []

key-decisions:
  - "cs2_in_job=yes: cs2.exe попадает в job при запуске через HLAE (16 из 16 запусков), принятие по PID остаётся запасным путём"
  - "fix_netcon=on, netcon_bind=0.0.0.0: порт консоли слушает на всех интерфейсах, это идёт в модель угроз 01-06"
  - "hlae_pipe_eof=at-game-exit: pipe HLAE закрываются только при выходе игры, потоки чтения не ждать через join"
  - "late_connect_ready=lost, и READY может прийти раньше демки на меню-карте: принимать его только после Host activate: Playing Demo"
  - "abs_playdemo=fails (кириллица искажается), ASCII с пробелами работает: копирование демки в фазе 1 остаётся"
  - "ffmpeg_in_job=unknown: стенд не довёл дубль до ffmpeg, действует запасной вариант (выход игры плюс close_settle)"

requirements-completed: []

coverage:
  - id: D1
    description: "01-SPIKES.md содержит блоки S1-S4 с командой, наблюдением, вердиктом, выбранным путём и следствием для WinHost"
    requirement: "TMO-02"
    verification:
      - kind: other
        ref: "grep '^## S[1-4]' и '**Вердикт:**' в 01-SPIKES.md: четыре блока, у каждого вердикт"
        status: pass
    human_judgment: true
    rationale: "Вердикты опираются на транскрипты реальных запусков CS2 на машине автора; содержательную полноту оценивает человек"
  - id: D2
    description: "Таблица «Решения для 01-06» содержит все семь ключей со значениями"
    requirement: "TMO-02"
    verification:
      - kind: other
        ref: "grep -cE '^\\| *.?(cs2_in_job|...|abs_playdemo)' 01-SPIKES.md = 7"
        status: pass
    human_judgment: false
  - id: D3
    description: "Фикстуры S3 и 01-SPIKES.md не содержат пути профиля, SteamID и имени аккаунта"
    requirement: "LCH-04"
    verification:
      - kind: other
        ref: "grep -i kamal|7656119|889086871|куритель по фикстурам и 01-SPIKES.md: 0 совпадений; слово SteamID встречается только в тексте пояснения"
        status: pass
    human_judgment: false
  - id: D4
    description: "Спайки запущены только после того, как пользователь закрыл свой CS2; чужой cs2.exe не завершался, после прогона игры не осталось"
    requirement: "TMO-02"
    verification:
      - kind: other
        ref: "перед каждой подкомандой tasklist без cs2.exe; в конце Get-Process cs2 пуст"
        status: pass
    human_judgment: false

duration: 25min
completed: 2026-10-10
status: complete
---

# Phase 1 Plan 05: Спайки S1-S4 на машине автора Summary

**Четыре спайка прогнаны стендом на настоящем CS2 и HLAE 2.192.7: `cs2.exe` попадает в наш Job Object, netcon слушает на `0.0.0.0`, READY теряется при позднем подключении и может прийти раньше демки, абсолютный путь с кириллицей не переживает `-cmdLine`; решения для `WinHost` записаны в `01-SPIKES.md`.**

## Performance

- **Duration:** около 25 мин (17 запусков игры)
- **Completed:** 2026-10-10
- **Tasks:** 3 (1 checkpoint уже снят пользователем, 1 прогон без коммита, 1 запись выводов)
- **Files created:** 5 (`01-SPIKES.md` и четыре фикстуры)

## Accomplishments

- **Task 1 (checkpoint:human-action)** — пользователь ответил «CS2 закрыт, можно запускать спайки» (трактуется как «готово»); оркестратор проверил, что `cs2.exe` не запущен. Перед каждой подкомандой я перепроверял это сам.
- **Task 2** — запущены `--dry-run`, `job`, `netcon --fix off --runs 5`, `netcon --fix on --runs 5`, `failures`, `abspath`. У каждого есть папка с `summary.txt` в `%LOCALAPPDATA%\cs2cinema\spikes\`. После прогона `Get-Process cs2` пуст.
- **Task 3** — `01-SPIKES.md` (по-русски) с блоками S1-S4, таблицей «Решения для 01-06», дополнительными находками и таблицей стартовых дедлайнов против наблюдённых времён; четыре фикстуры консоли (190-270 строк каждая, шум и личные данные убраны).

Решения для 01-06 (подробно в `01-SPIKES.md`):

| Ключ | Значение |
|------|----------|
| `cs2_in_job` | `yes` |
| `ffmpeg_in_job` | `unknown` |
| `fix_netcon` | `on` |
| `netcon_bind` | `0.0.0.0` |
| `hlae_pipe_eof` | `at-game-exit` |
| `late_connect_ready` | `lost` |
| `abs_playdemo` | `fails` (кириллица; ASCII с пробелами работает) |

## Task Commits

1. **Task 1:** без коммита (checkpoint, снят ответом пользователя).
2. **Task 2:** без коммита (прогон стенда; результаты вне репозитория).
3. **Task 3:** `d60be89` (docs) — `01-SPIKES.md` и четыре фикстуры.

## Files Created/Modified

- `.planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md` — вердикты и решения
- `crates/pipeline/tests/fixtures/s3a_missing_demo.log`, `s3b_empty_demo.log`, `s3c_missing_cfg.log`, `s3e_late_connect.log` — консоль при сбоях

## Decisions Made

См. `key-decisions` во frontmatter. Фаза не заблокирована: у `ffmpeg_in_job` значение `unknown` имеет запасной вариант, остальные шесть ключей получены наблюдением.

## Deviations from Plan

### Дополнения сверх плана

**1. [Дополнительный запуск] Ручная проверка ASCII-пути для S4**
- **Причина:** стенд проверяет путь сразу с пробелами и кириллицей и отвечает только «FAIL»; без разделения причин значение `abs_playdemo` для фазы 2 было бы вводящим в заблуждение.
- **Что сделано:** одноразовый скрипт в scratchpad запустил `HLAE.exe` напрямую с ASCII-путём с пробелами; свой `cs2.exe` закрыт по PID после проверки строки `Host activate: Playing Demo`, временная копия демки удалена. Чужих процессов не трогал.
- **Результат:** ASCII с пробелами работает, кириллица нет. Кода в репозитории не менялось.

**2. [Уборка] Пустая временная папка `cs2cinema spike демо`**
- Стенд оставил её после S4 (`remove_dir` вызывается раньше, чем отрабатывает `Cleanup`). Папка была пустой, удалена вручную; ошибка стенда записана в «Дополнительные находки» 01-SPIKES.md, стенд (`crates/host/examples/spikes.rs`, план 01-01) в этом плане не менялся.

### Deferred Issues (не исправлялись, вне границ плана)

- Стенд `spikes.rs`: S1-дубль не выставляет `mirv_streams record fps`, поэтому ffmpeg HLAE не стартует (`AFXERROR: ... FPS 0.000000 < 1`) и `ffmpeg_in_job` остался `unknown`. Чтобы получить наблюдение, достаточно добавить `mirv_streams record fps 30` перед `record start` и повторить `spikes -- job`.
- Стенд `spikes.rs`: S3a/S3b поле «улики» ловит шум (строка запуска, SDR), не вердикт; `remove_dir` после S4 (см. выше).

**Total deviations:** 0 auto-fix по правилам 1-4, 2 дополнения. Контракт плана выполнен.

## Issues Encountered

- Один из 16 `quit` (S2, с `-afxFixNetCon`, пятый запуск) не завершил игру за 30 с: процесс завис после `Source2Shutdown`; стенд закрыл job через `TerminateJobObject`, остатков нет. В 01-06 закрытие должно заканчиваться `Job::terminate()`.
- Фикстуры записаны с LF, а `.gitattributes` при проверке в рабочую копию превращает их в CRLF (git предупредил). Парсинг по `lines()` не зависит от этого, но тесты не должны сравнивать байты построчно.
- В Git Bash `node` вне PATH: `gsd_run` вызывается с `PATH="/c/Program Files/nodejs:$PATH"`.

## Known Stubs

Нет (план не пишет код продукта).

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: netcon-all-interfaces | 01-SPIKES.md (S2) | консольный порт CS2 слушает на `0.0.0.0` без пароля (A12 опровергнуто); входит в T-01-15, учесть в модели угроз 01-06 |

Митигации плана: T-01-14 — стенд и я не трогали чужой `cs2.exe`, предварительная проверка перед каждым запуском; T-01-16 — `grep` по `kamal|7656119|889086871|куритель` в фикстурах и `01-SPIKES.md` даёт 0 совпадений.

## Next Phase Readiness

01-06 берёт строку запуска и режимы `WinHost` из таблицы «Решения для 01-06». Обязательные поправки, которые касаются уже написанных планов: (1) машина 01-04/`WinHost` принимает READY только после `Host activate: Playing Demo` (ложный READY на меню-карте); (2) закрытие всегда заканчивается `Job::terminate()`; (3) потоки чтения pipe HLAE не ждут `join` до смерти игры; (4) `netcon` доступен по `0.0.0.0` — в модель угроз.

## Self-Check: PASSED

- Файлы: `01-SPIKES.md` и четыре фикстуры на месте.
- Коммит `d60be89` найден в истории ветки.
- `grep -c "Решения для 01-06"` = 1; ключи в таблице = 7; блоки `## S1`-`## S4` с `**Вердикт:**` на месте; `grep -i kamal` по фикстурам и `01-SPIKES.md` пуст; `Get-Process cs2` пуст.
