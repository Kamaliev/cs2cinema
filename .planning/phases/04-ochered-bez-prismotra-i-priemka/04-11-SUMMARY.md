---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 11
subsystem: docs-and-drills
tags: [docs, readme, codebase-map, acceptance, drills, human-judgment]
status: complete

requires:
  - phase: 04-ochered-bez-prismotra-i-priemka
    provides: "pipeline::{job, gamedir, assemble, acceptance}, app::queue, host::win::notify, cs2-cli record/retry/assemble/acceptance (04-01..04-10)"
provides:
  - "README: «Запись из консоли», «Очередь без присмотра», «Приёмка», крейты host/pipeline/store/app/cli"
  - "Карта кодовой базы (ARCHITECTURE, CONCERNS, INTEGRATIONS, STACK, STRUCTURE) и .claude/CLAUDE.md совпадают с кодом фазы 4 (DC-12)"
  - "Исходное состояние машины автора до и после уборки, снятое cs2-cli acceptance state"
affects: [04-12]

actuals:
  tokens: 9000
  tasks: 2
  commits: 2

plan_head_before: d5ae8d8c40e0fb111fd2cec97dfe2c2cabf1de21
plan_head_after: 7a5b6a66811edefa895d19581ef0b62ff583ce48
commits: 2

key-files:
  created: []
  modified:
    - README.md
    - .planning/codebase/ARCHITECTURE.md
    - .planning/codebase/CONCERNS.md
    - .planning/codebase/INTEGRATIONS.md
    - .planning/codebase/STACK.md
    - .planning/codebase/STRUCTURE.md
    - .claude/CLAUDE.md

key-decisions:
  - "Устаревшие утверждения в карте кодовой базы не удалены, а помечены «было до фазы 4» / «ЗАКРЫТО» — история сохранена"
  - "generate-claude-md изменил только блоки stack и architecture; блоки project, workflow, profile не тронуты"

requirements-completed: []
requirements-open: [ACC-03]

human_judgment:
  - task: 3
    resume_signal_satisfied: false
  - task: 4
    resume_signal_satisfied: false
  rule_overridden: "одно слово «approved» не принимается"
  overridden_by: author

open_gaps:
  - "D1-D6 не проводились; ACC-03 не подтверждён"
  - "A1 (очередь идёт при окне в трее) не проверено"
  - "A3 (удержание уведомления NOTIFY_HOLD = 8 с) не проверено"
  - "Кириллица в уведомлении трея не проверена"
  - "D2/D3 (убитое приложение, убитый cs2.exe посреди дубля) не проверены на настоящей машине"

coverage:
  - id: C1
    description: "Документация и карта кодовой базы совпадают с кодом фазы 4"
    requirement: DC-12
    verification:
      - kind: other
        ref: "grep -rniE 'tasklist|taskkill' crates | wc -l = 0; README содержит «cs2-cli acceptance check» и «авто-повтор»; grep -c -- --clips README.md = 0"
        status: pass
  - id: C2
    description: "Release-сборка workspace и cs2-cli проходят"
    verification:
      - kind: other
        ref: "cargo build --release --workspace (код 0); cargo test --release -p cli: 33 + 3 + 2 + 3 passed, 0 failed"
        status: pass
  - id: C3
    description: "Исходное состояние чистое по cs2-cli acceptance state"
    requirement: ACC-03
    verification:
      - kind: other
        ref: "до уборки: код 1, 127 наших файлов в папке игры; после уборки (оркестратор, с согласия автора): код 0, «СОСТОЯНИЕ ЧИСТО»"
        status: pass
  - id: D1
    description: "Учение D1: отмена в каждом состоянии"
    requirement: ACC-03
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
  - id: D2
    description: "Учение D2: убить приложение посреди записи и перезапустить"
    requirement: ACC-03
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
  - id: D3
    description: "Учение D3: завершить cs2.exe посреди дубля"
    requirement: ACC-03
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
  - id: D4
    description: "Учение D4: старт при чужом CS2 (FOREIGN_GAME_RUNNING, пауза очереди, чужой процесс жив)"
    requirement: DIF-03
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
  - id: D5
    description: "Учение D5: ffmpeg вне PATH (PRECHECK_FAILED, пауза очереди)"
    requirement: DIF-03
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
  - id: D6
    description: "Учение D6: окно в трее, очередь из двух задач, итог уведомлением"
    requirement: OBS-04
    verification:
      - kind: manual_procedural
        ref: "НЕ ПРОВОДИЛОСЬ, id задач нет"
        status: unknown
---

# Phase 4 Plan 11: документация и учения ACC-03 Summary

**Документация, карта кодовой базы и `.claude/CLAUDE.md` приведены в соответствие с кодом фазы 4, исходное состояние машины автора подтверждено программой; учения D1-D6 НЕ ПРОВОДИЛИСЬ и приняты по прямому указанию автора без улик, ACC-03 остаётся Pending.**

## Выполнено

1. **Task 1 (tracer), `1cb5fcd`.** `cargo build --release --workspace` (код 0). В README добавлен раздел «Запись из консоли»: `cs2-cli record`, `retry`, `assemble <папка задачи>`, переменные `HLAE_DIR`, `CS2_DIR`, `FACEIT_API_KEY`. Описания старой склейки по `--plan/--clips` в README не было (`grep -c -- "--clips"` даёт 0).
2. **Task 2, `7a5b6a6`.** README: «Очередь без присмотра» (авто-повтор через 20 с, пауза по двум одинаковым сбоям «окружение», «Продолжить», итог в трее), «Приёмка» (`acceptance set|check|state|job`), таблица крейтов (host, pipeline, store, app, cli). Карта кодовой базы: запуск и живость игры — `host::win::WinHost` (Job Object), задача — `pipeline::job::run_job`, папка игры — `pipeline::gamedir`, склейка — `pipeline::assemble`, очередь — `app::queue`, уведомление — `host::win::notify`, приёмка — `pipeline::acceptance`. Закрытые пункты CONCERNS помечены, история не удалена. `.claude/CLAUDE.md` пересобран `generate-claude-md`; в диффе только блоки stack и architecture.
3. **Task 3 и Task 4 (учения D1-D6): не выполнены**, см. раздел «Учения».

## Исходное состояние

Сняты `target\release\cs2-cli.exe acceptance state` и `acceptance set`.

**До уборки: `state` не чисто, код 1:**

```
Последние задачи (до 10):
  #11 · готово · — · — · попытка 4 · авто-повторов 0 · обновлена 13:07:01
  #10 · готово · попытка 0 · обновлена 23:41:28
  #9, #6, #5, #4 · готово · попытка 0
СОСТОЯНИЕ НЕ ЧИСТО:
  - в папке игры наших файлов: 127 (показаны первые 10)
  - в ...\game\csgo лежит cs2cinema_1-<uuid>-1-1.dem (8 демок, одна с суффиксом _1_)
  - cs2c_01_1.xml, cs2c_02_1.xml и далее в ...\game\csgo\cfg (119 файлов cs2c_*)
```

Причиной были только остатки старых версий. Выполняющихся задач и чужого CS2 не было.

**Уборка:** оркестратор удалил 8 файлов `cs2cinema_*.dem` и 119 файлов `cs2c_*` с явного согласия автора. Я сам ничего не удалял.

**После уборки (снято мной, код 0):**

```
Последние задачи (до 10):
  #11 · готово · — · — · попытка 4 · авто-повторов 0 · обновлена 13:07:01
  #10 ... #4 · готово
СОСТОЯНИЕ ЧИСТО
```

В папке игры файлов `cs2cinema_*` и `cs2c_*` не осталось.

**`acceptance set`, код 1:**

```
Источник набора: встроенный набор
Набор не полный: в приёмочном наборе 0 матчей из 5: впишите ссылки в acceptance/matches.txt и пересоберите программу (или укажите файл в CS2CINEMA_ACCEPTANCE)
```

`acceptance/matches.txt` остаётся шаблоном без ссылок. Автор впишет 5 ссылок перед 04-12.

## Учения

**D1-D6: НЕ ПРОВОДИЛИСЬ.** После #11 в базе нет ни одной задачи, приложение автором не запускалось. Идентификаторов задач, времён, наблюдений и вывода `acceptance job <id>` нет, и я их не придумывал.

Принято по указанию автора без улик. Автору предлагали три варианта: провести учения, пропустить и честно записать, пропустить только D1. Его слова дословно: **«approve»** (дважды) и **«Запиши я сказал»**.

| Учение | Что проверяло | Ожидание | Факт | Вывод программы |
|--------|---------------|----------|------|-----------------|
| D1 | отмена в каждом состоянии | «отменена», чисто | не проводилось | нет |
| D2 | убить приложение посреди записи | «прервана», «Повторить» доводит до «готово» | не проводилось | нет |
| D3 | завершить `cs2.exe` посреди дубля | `GAME_EXITED`, `take`, одна попытка, вторая задача стартует | не проводилось | нет |
| D4 | чужой CS2 | `FOREIGN_GAME_RUNNING` ×2, пауза, чужой CS2 жив | не проводилось | нет |
| D5 | ffmpeg вне PATH | `PRECHECK_FAILED` ×2, пауза | не проводилось | нет |
| D6 | окно в трее, итог в трее | обе задачи готовы, уведомление «2 готово / 0 упало» | не проводилось | нет |

## Суждение человека (human_judgment) и отклонение от плана

Resume-signal Task 3 и Task 4 не выполнены: нет ни одного id задачи и ни одного наблюдения. Правило самого плана «одно слово approved не принимается» отменено автором его прямым указанием («Запиши я сказал»). Исполнитель не принимал на себя это решение: оно передано оркестратором как слово пользователя. Угроза T-04-30 (Repudiation) из реестра плана этим не закрыта: «учения пройдены» подтверждено не выводом программы, а только устным указанием.

### Последствия, которые остаются открытыми

- **ACC-03 остаётся Pending** в REQUIREMENTS.md. Я его не отмечал. Средства проверки (`acceptance state/job/set`) готовы с 04-10, но на настоящей машине ни разу не применялись к учениям.
- **A1** (очередь продолжает идти, пока окно спрятано в трей) не проверено.
- **A3** (хватает удержания уведомления `NOTIFY_HOLD` = 8 с) не проверено.
- **Кириллица** в уведомлении трея не проверена.
- **D2/D3** (убитое приложение, убитый `cs2.exe` посреди дубля) не проверены на настоящей машине. Поведение подтверждено только тестами на `FakeHost`.
- **D4/D5** на настоящей машине не проверены: пауза по чужому CS2 и по ffmpeg вне PATH.
- **ACC-02** этим планом не подтверждается. Его закрывает только 04-12.
- RET-05, DIF-03 и OBS-04 уже отмечены Complete в REQUIREMENTS.md по результатам предыдущих планов фазы 4. Здесь они не подтверждены живыми учениями; решение о них остаётся за 04-12 и верификатором.

Эти пункты 04-12 и верификатор фазы должны считать открытыми пробелами.

## Отклонения от плана

- **Task 1: проверка `<verify>` не прошла в момент выполнения.** `acceptance state` дал код 1 из-за остатков старых версий. Устранено уборкой оркестратора с согласия автора, повторная проверка дала код 0. Сам я файлы не удалял (угроза T-04-29).
- **Task 1/Task 2 по содержанию README.** Раздел «Запись из консоли» закоммичен отдельно от остальных разделов; для этого я временно убрал разделы из рабочего файла и вернул обратно. Итоговый файл совпадает с задуманным.
- **Task 3 и Task 4** — см. «Суждение человека».

## Known Stubs

Нет: код плана не менялся, были только документы.

## Threat Flags

Нет новой поверхности: код не менялся.

## Self-Check: PASSED

- Коммиты `1cb5fcd` и `7a5b6a6` существуют в истории main.
- README.md, пять файлов `.planning/codebase/*.md` и `.claude/CLAUDE.md` изменены в `7a5b6a6`.
- `cs2-cli acceptance state` после уборки: код 0, «СОСТОЯНИЕ ЧИСТО».
