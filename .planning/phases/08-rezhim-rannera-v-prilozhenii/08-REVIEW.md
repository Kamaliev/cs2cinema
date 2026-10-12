---
phase: 08-rezhim-rannera-v-prilozhenii
reviewed: 2026-10-12T00:00:00Z
depth: standard
files_reviewed: 27
files_reviewed_list:
  - Cargo.lock
  - crates/app/Cargo.toml
  - crates/app/src/app.rs
  - crates/app/src/jobs.rs
  - crates/app/src/jobs_page.rs
  - crates/app/src/main.rs
  - crates/app/src/queue.rs
  - crates/app/src/remote/control.rs
  - crates/app/src/remote/e2e.rs
  - crates/app/src/remote/hub.rs
  - crates/app/src/remote/mod.rs
  - crates/app/src/remote/testkit.rs
  - crates/app/src/remote/tests.rs
  - crates/app/src/remote/token.rs
  - crates/app/src/settings.rs
  - crates/app/src/settings_page.rs
  - crates/cli/src/acceptance.rs
  - crates/cli/src/runner_cmd.rs
  - crates/pipeline/src/inspect.rs
  - crates/pipeline/src/lib.rs
  - crates/runner/src/exec.rs
  - crates/runner/src/lib.rs
  - crates/runner/tests/common/mod.rs
  - crates/runner/tests/exec.rs
  - crates/runner/tests/gate.rs
  - crates/runner/tests/observer.rs
  - crates/store/src/lib.rs
findings:
  critical: 0
  warning: 3
  info: 3
  total: 6
status: issues_found
---

# Phase 8: Code Review Report

**Reviewed:** 2026-10-12
**Depth:** standard
**Files Reviewed:** 27
**Status:** issues_found

## Summary

Просмотрены изменения фазы 8 (`git diff 01e2ffd..HEAD`): хаб удалённых карточек, поток раннера в приложении, ворота допуска, наблюдатель, миграция `external_ref`, страница настроек и карточки сервера в окне. Блокирующих дефектов нет: токен нигде не попадает в логи, базу и настройки (проверено по коду и по тесту `issued_token_is_found_only_in_its_file`), чужие строки (ник, карта, имя игрока) очищаются `clean_text`, имя файла демки строится только из номера задачи, `http://` на внешний адрес `Link::new` не принимает. Без адреса сервера поток не создаётся и очередь идёт прежним путём (`start_with_hook` с хуком, который для локальных строк ничего не меняет).

Найдены три предупреждения: два участка, где состояние карточки в базе может разойтись с тем, что делает поток очереди (оба связаны с безусловными `UPDATE` статуса), и синхронный `icacls` в потоке окна.

Оценка двух наблюдений из аудита безопасности:

1. `Dropped.reason` без `clean_text` — **реальным дефектом не является**. Строка проходит `Runner::emit` -> `scrub` -> `one_line` (управляющие символы заменяются пробелами, токен вычёркивается) до того, как попадёт в наблюдателя, а все пять вызовов `drop_job` строят причину из констант (`STALE_EPOCH`, `JOB_TERMINAL`, `NOT_FOUND`, фиксированные фразы); текст от сервера (`message`) в причину не входит. Подделать строку `итог попытки N: …` перевода строки не получится. Остаётся защита вглубь — см. IN-01.
2. Гонка «карточка не удержана раннером, локальный повтор, `adopt_remote` возвращает её в очередь» — **реальна**, см. WR-01. Остаточная гонка между `next_queued` и `start_job` при отмене сервером из сводки 08-03 тоже реальна, см. WR-02.

## Warnings

### WR-01: `adopt_remote` возвращает в `queued` карточку, которую очередь прямо сейчас выполняет локально

**File:** `crates/store/src/lib.rs:608-611` (также `crates/app/src/remote/control.rs:389,397,568`, `crates/app/src/remote/hub.rs:85-98`)
**Issue:** `UPDATE jobs SET status = 'queued', …` в ветке `Some(id)` не смотрит на текущий статус и срабатывает для строки в `running` и `cancelling`. Цепочка, которая к этому приводит:

1. Раннер выключен или не стартовал (нет токена, нет `cs2-cli.exe`, замок занят, адрес пуст): `start()` вызывает `hub.set_held_ref(None)`, хотя в `runner_state.json` выдача всё ещё лежит.
2. Карточка этой выдачи остаётся в `interrupted`/`failed`/`cancelled`, `is_held` = false, и `remote_card_buttons` показывает «Повторить».
3. Автор нажимает «Повторить»: строка встаёт в `queued` с `resume = 1`, поток очереди берёт её, `hub.take` не находит записи, срабатывает ветка `job.resume` -> `local(job, rep)` (локальная запись без отправки).
4. Автор чинит причину (сохраняет токен), поток раннера стартует, поднимает выдачу из состояния и зовёт `RemoteExecutor::open` -> `adopt_remote`, пока локальная запись ещё идёт.

Итог: статус идущей записи молча меняется на `queued`, создаётся запись хаба, и когда локальный прогон кончается, `SettleHook::on_settled` находит новую запись и отправляет раннеру итог **локального** прогона (в том числе `CANCELLED`, если автор отменял локальный повтор — сервер получит отмену задачи друга). Кнопка «Отмена» для строки со статусом `queued` идёт через `store.cancel_job`, а не через `CancelToken`, поэтому отменить идущий прогон из окна в это время уже нельзя. Если же выдача по какой-то причине не поймает итог, карточка остаётся `done`, а раннер ждёт `Settled` до перезапуска. Тесты `adopt_remote_*` и `tests.rs` этот порядок не покрывают.

Корень проблемы двойной: (а) `adopt_remote` не различает «ждёт» и «выполняется», (б) «удержана раннером» определяется живым потоком, а не содержимым `runner_state.json`, поэтому при выключенном раннере кнопка «Повторить» предлагает локально выполнить задачу, чья выдача ещё жива.

**Fix:**
```rust
// store: не трогать выполняемые строки, сообщить об этом вызывающему
"UPDATE jobs SET status = 'queued', … WHERE id = ?1 AND status NOT IN ('running', 'cancelling')"
// если затронуто 0 строк — вернуть Err("карточка ещё выполняется в очереди приложения")
```
В `RemoteExecutor::open` на такую ошибку ждать конца прогона (опрос строки раз в `ROW_STEP`, пока статус не станет терминальным, с проверкой `p.cancelled()`), а не завершать выдачу сбоем. Дополнительно сделать `RemoteHub::is_held` независимым от жизни потока: читать `RunnerState` из `runner_dir` (или не сбрасывать `held_ref` в `start()` при отказе/выключении), чтобы «Повторить» не предлагалась для карточки с живой выдачей. Добавить тест: карточка в `running` + `adopt_remote` -> статус не меняется.

### WR-02: `start_job` без условия на статус — отмена ждущей карточки сервером между `next_queued` и `start_job` не работает

**File:** `crates/store/src/lib.rs:428-437`, `crates/app/src/queue.rs:358-372` (взаимодействие с `crates/app/src/remote/hub.rs:203-216`)
**Issue:** Поток очереди делает `next_queued()` и затем отдельным вызовом `start_job(id)`, который безусловно ставит `status = 'running'`. Если между ними поток раннера выполняет `on_server_cancel`, то `cancel_if_queued` успешно переводит карточку в `cancelled`, запись хаба удаляется, серверу уходит отмена — а `start_job` сразу перезаписывает `cancelled` на `running` и увеличивает `attempt`. Дальше `queue_runner` не находит записи в хабе:
- если `job.resume = false`, карточка получает ошибку «задача с сервера уже закрыта…» и статус `failed` вместо `cancelled`, в журнале появляется ложная попытка;
- если `job.resume = true` (карточка продолжается после перезапуска), срабатывает ветка «результат не отправляется» и **выполняется полная локальная запись в CS2** по задаче, которую сервер уже отменил, пока очередь заблокирована.

То же окно существует для ручной отмены ждущей карточки автором (до фазы 8 оно просто не сказывалось из-за единственного писателя, теперь писателей двое). Окно узкое (между двумя обращениями к SQLite), но последствие — лишний запуск игры, а исправление дешёвое.

**Fix:**
```rust
pub fn claim_queued(&self, id: i64) -> Result<Option<u32>> {
    self.with(|c| {
        let n = c.execute("UPDATE jobs SET status = 'running', … WHERE id = ?1 AND status = 'queued'", params![id, now()])?;
        if n == 0 { return Ok(None); }
        let a: i64 = c.query_row("SELECT attempt FROM jobs WHERE id = ?1", [id], |r| r.get(0))?;
        Ok(Some(a.max(0) as u32))
    })
}
```
В `queue.rs` при `Ok(None)` просто `continue` (карточку уже отменили). `start_job` оставить для тестов или перевести на то же условие. Добавить тест «`cancel_if_queued` до `claim_queued` -> очередь карточку не запускает».

### WR-03: `save_token` запускает `icacls` синхронно в потоке окна

**File:** `crates/app/src/remote/token.rs:70-89,101-117` (вызов: `crates/app/src/settings_page.rs`, обработчик `save_token`)
**Issue:** `RemoteRunner::save_token` -> `token::save` -> `host.run(icacls, deadline = 15 с)` выполняется прямо в `update` egui по нажатию «Сохранить токен». Обычно `icacls` отрабатывает быстро, но при антивирусе, сетевом профиле или зависшей утилите окно замирает до 15 секунд (на это же время зависает и `app.poll()` очереди). Остальные внешние вызовы приложения вынесены в `App::spawn` (CLAUDE.md: фоновая работа идёт потоком и сообщает через `Msg`). Побочно: `PathBuf::from("icacls")` ищется по правилам `CreateProcess`, то есть сначала в папке приложения (`%LOCALAPPDATA%\cs2cinema`), а не в `System32`.

**Fix:** выполнять `token::save` через `App::spawn` и принимать результат сообщением `Msg` (после успеха вызывать `restart()` уже в потоке окна). Путь к утилите брать явно: `PathBuf::from(std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32").join("icacls.exe")`.

## Info

### IN-01: `journal()` доверяет строкам события, а разбор журнала ищет метки в любом месте строки

**File:** `crates/app/src/remote/control.rs:514`, `crates/store/src/journal.rs:79-101`
**Issue:** Сейчас безопасно (см. Summary: `Dropped.reason` очищается в `emit` и собирается из констант), но защита держится на соглашении между двумя крейтами. `Observer` — публичный тип, `RunnerEvent::Dropped.reason` может получить любую строку, а `journal::outcomes`/`attempts`/`stage_summary` ищут `итог попытки `/`──── попытка `/`⏱ этап ` через `l.find(..)` **в любой позиции** строки. Строка вида `сервер снял задачу: итог попытки 1: готово: ролик` будет прочитана командой `cs2-cli acceptance` как итог попытки.
**Fix:** в `journal()` пропускать текст через `pipeline::record::log::one_line` и обрезать до разумной длины (например 300 знаков); по возможности в `journal.rs` искать метки только после префикса `[… #N] ` либо в начале строки.

### IN-02: Строка состояния обещает «продолжится сама», даже если раннер запуститься не может

**File:** `crates/app/src/app.rs:294-297`
**Issue:** При `reset.interrupted_remote > 0` безусловно пишется «продолжится сама после подключения к серверу». Если адрес пуст, токена нет или нет `cs2-cli.exe`, раннер не стартует, карточка остаётся `interrupted` (и в карточке `server_note` честно ничего не обещает), а в строке состояния обещание остаётся.
**Fix:** формировать сообщение после `remote.apply(..)` по `remote.status()`: при `Off`/`NoToken`/`NoCli`/`BadToken` говорить, что задача с сервера прервана и раннер не запущен, и называть причину (`status.text()`).

### IN-03: Сквозной `#![allow(dead_code)]` в `remote/mod.rs` с устаревшим комментарием

**File:** `crates/app/src/remote/mod.rs:6`, строки 11-14 (`#[allow(unused_imports)]`)
**Issue:** Комментарий «хаб подключается к окну и настройкам следующими планами фазы 8» неверен — планы выполнены. Модульный `allow(dead_code)` скрывает действительно мёртвый код во всём `remote` (`RemoteHub::slot_cards` и `RemoteRunner::dir` нигде не вызываются), а CLAUDE.md просит держать `allow` минимальными.
**Fix:** убрать `#![allow(dead_code)]` и `#[allow(unused_imports)]`, оставить точечные `#[cfg(test)]`/`#[allow]` на конкретных элементах, которые действительно нужны только тестам.

---

_Reviewed: 2026-10-12_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
