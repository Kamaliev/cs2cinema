---
phase: 04-ochered-bez-prismotra-i-priemka
plan: 03
subsystem: host
tags: [rust, host, win32, tray-notification, fake-host, spike]
status: complete

requires:
  - phase: 01-host
    provides: "crates/host: трейт Host, WinHost, FakeHost, правило D-01 (Win32 только в host)"
provides:
  - "host: Host::notify (по умолчанию false), notify_text, NOTIFY_TITLE_MAX (63), NOTIFY_TEXT_MAX (255)"
  - "host::win::notify: balloon(title, text) -> bool через Shell_NotifyIconW(NIF_INFO), NOTIFY_HOLD = 8 с"
  - "host::fake: FakeHost::notifications, FakeHost::sequence"
  - "пример balloon для спайка"
affects: [04-08, 04-09, 04-11]

actuals:
  tokens: 14000
  tasks: 3
  commits: 2

plan_head_before: f66b4f029747a6d34fc0f5085016ba899fc095de
plan_head_after: 6c48abe329c035409fbb9b0a971354d04f2b0b0b
commits: 2

tech-stack:
  added: []
  patterns:
    - "уведомление — собственный короткоживущий значок на окне HWND_MESSAGE на отдельном потоке; ожидание ответа не дольше 2 с"
    - "одна очистка текста (notify_text) и для настоящего хоста, и для фейка"
    - "FakeHost::sequence: сценарий переключается в момент launch, последний повторяется"

key-files:
  created:
    - crates/host/src/win/notify.rs
    - crates/host/examples/balloon.rs
  modified:
    - crates/host/Cargo.toml
    - crates/host/src/lib.rs
    - crates/host/src/win/mod.rs
    - crates/host/src/fake.rs

key-decisions:
  - "DC-6: уведомление через Shell_NotifyIconW, без PowerShell (текст сбоя содержит строки консоли игры)"
  - "модуль win::notify объявлен pub, чтобы пример balloon брал NOTIFY_HOLD; на не-Windows у примера своя константа"
  - "NOTIFY_HOLD оставлен 8 с: длительность на машине автора не измерена"

requirements-completed: [OBS-04, RET-05, DIF-04]

completed: 2026-10-11
---

# Phase 4 Plan 03: Уведомление в трее и сценарии фейка Summary

**`Host::notify` показывает уведомление Windows через `Shell_NotifyIconW` на отдельном потоке (текст чистится и режется до 63/255 единиц UTF-16), `FakeHost` записывает уведомления, а `FakeHost::sequence` даёт каждому запуску игры свой сценарий для теста авто-повтора.**

## Что сделано

- **Task 1 (tracer, `329eebd`).** Метод трейта `Host::notify`, `notify_text` (убирает управляющие символы, режет по границе символа с «…», не режет суррогатные пары), `win::notify::balloon` (окно `STATIC` с родителем `HWND_MESSAGE`, `NIM_ADD` с `NIF_ICON | NIF_TIP | NIF_INFO`, ответ в канал не дольше 2 с, удержание `NOTIFY_HOLD`, затем `NIM_DELETE` и `DestroyWindow`), `FakeHost::notifications`, пример `balloon`. Фича `Win32_UI_Shell` у `windows-sys`; других изменений манифеста нет.
- **Task 2 (`6c48abe`).** `FakeHost::sequence` (`pending`, `launch_calls` в `HostState`): k-й `launch` идёт по k-му сценарию, последний повторяется, пустой список паникует. Тесты `sequence_switches_the_scenario_on_each_launch`, `sequence_repeats_the_last_scenario`, `empty_sequence_panics`.
- **Task 3 (спайк).** Ответ автора записан ниже.

Проверки: `cargo test --release -p host --lib` — 70 passed, 0 failed, 1 ignored; `cargo build --release -p host --example balloon` и `cargo build --release -p app` — зелёные; `grep -rn "windows_sys" crates --include=*.rs | grep -v "^crates/host/"` — 0.

## Спайк A1–A3

Ответ автора, дословно: (1) «Балон уведомление работает», (2) «approve».

Автор НЕ предоставил: видимость кириллицы, сколько секунд держится уведомление, состояние режима «Фокусировка», время старта второй задачи и время открытия окна. Эти значения не выдуманы и не восстановлены.

| Допущение | Статус | Основание |
|-----------|--------|-----------|
| A2: уведомление показывается на Windows 11 | **подтверждено** | автор: «Балон уведомление работает» |
| A3: 8 с (`NOTIFY_HOLD`) хватает | **НЕ проверено** | длительность на экране не названа; кириллица тоже не подтверждена |
| A1: очередь продолжает работу, пока окно спрятано в трей | **НЕ проверено** | шаг 2 пропущен, дан только ответ «approve» |

**Принятый пробел.** Ответ «approve» не соответствует формату resume-signal плана («одно слово approved не принимается»). Он принят оркестратором как есть; пробел зафиксирован явно, ответы A1 и A3 отсутствуют. `NOTIFY_HOLD` не менялся.

**Следствия:**
- Планы 04-08/04-09 не должны опираться ни на истинность, ни на ложность A1: очередь проектируется на рабочем потоке независимо от того, продолжает ли нынешняя очередь работать при спрятанном окне.
- Запасной путь (баннер в окне и подсказка значка, план 04-09) остаётся по плану.
- Учение D6 в плане 04-11 должно на машине автора проверить кириллицу и длительность уведомления (A3) и поведение очереди при окне в трее (A1).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `fill` не гарантировал завершающий ноль**
- **Found during:** Task 1 (юнит-тест `fill_truncates_and_keeps_the_terminating_zero`)
- **Issue:** первая версия опиралась на заранее обнулённый буфер.
- **Fix:** после копирования явно пишется 0.
- **Files modified:** crates/host/src/win/notify.rs
- **Commit:** 329eebd

**2. [Rule 3 - Blocking] Модуль `win::notify` сделан `pub`**
- **Issue:** пример `balloon` берёт `NOTIFY_HOLD` из модуля; в плане `mod notify;`.
- **Fix:** `pub mod notify;`; на не-Windows у примера своя константа 8 с, пример собирается везде.
- **Commit:** 329eebd

Добавлены два теста сверх плана: `unsupported_host_does_not_notify` и `fill_truncates_and_keeps_the_terminating_zero`.

Auth gates: нет.

## Known Stubs

Нет.

## Threat Flags

Нет новых поверхностей сверх `<threat_model>` плана (T-04-06, T-04-07 учтены: нативный вызов без оболочки, очистка и обрезка текста, отдельный поток, ожидание 2 с, `false` вместо паники).

## Self-Check: PASSED

- FOUND: crates/host/src/win/notify.rs, crates/host/examples/balloon.rs
- FOUND: 329eebd, 6c48abe (предки HEAD)
