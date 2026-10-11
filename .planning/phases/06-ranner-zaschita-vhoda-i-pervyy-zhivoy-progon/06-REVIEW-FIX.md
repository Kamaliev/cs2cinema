---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
fixed_at: 2026-10-11T00:00:00Z
review_path: .planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-REVIEW.md
iteration: 1
findings_in_scope: 9
fixed: 9
skipped: 0
status: all_fixed
---

# Phase 6: Code Review Fix Report

**Fixed at:** 2026-10-11
**Source review:** `.planning/phases/06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon/06-REVIEW.md`
**Iteration:** 1

**Summary:**
- Findings in scope: 9 (критических 0, предупреждений 9; Info не трогались)
- Fixed: 9
- Skipped: 0

**Проверка:** все гейты запускались в основной рабочей копии (`workflow.use_worktrees = false`, изолированного
worktree не было), `cargo test --release --workspace` зелёный (0 упавших), `cargo test --release -p cli --test pipeline`
зелёный. Правки в логике (WR-02, WR-04, WR-05, WR-06) помечены «требуют проверки человеком»: тесты проверяют
синтаксис и существующие сценарии, но не весь новый поведенческий контракт.

## Fixed Issues

### WR-01: Лимиты демки из флагов раннера не доходят до `run_job`

**Статус:** fixed
**Commit:** a87db42
**Files modified:** `crates/pipeline/src/job.rs`, `crates/app/src/jobs.rs`, `crates/runner/src/exec.rs`
**Applied fix:** в `JobConfig` добавлено поле `demo_limits: demo::DemoLimits` (Default, Debug); `run_job` читает демку
с `cfg.demo_limits`; `PipelineExecutor::call_job` подставляет `self.inspect.limits.demo`, то есть те же числа, что у
дочерней предпроверки. Плумбинг через `runner_cmd` не понадобился: потолки уже лежат в `InspectSettings`.
Повторный разбор демки внутри процесса раннера (без песочницы) оставлен как есть: это отдельное архитектурное
решение, выходящее за минимальную правку.

### WR-02: Случайный Enter во время работы снимает паузу

**Статус:** fixed: requires human verification
**Commit:** 0f5f39d
**Files modified:** `crates/runner/src/lib.rs`
**Applied fix:** `start_pause` сбрасывает флаг `resume`, Enter, нажатый во время задачи, не считается снятием паузы.
Автотест не добавлен: нажатие посреди `tick` без хука в исполнителе стенда воспроизвести нельзя, версия теста без хука
проходила бы и до правки.

### WR-03: Успешные задачи не освобождают диск

**Статус:** fixed: requires human verification
**Commit:** fdfe4ed
**Files modified:** `crates/runner/src/lib.rs`, `crates/runner/tests/flow.rs`
**Applied fix:** после принятого результата (`TransferEnd::Done`) удаляются демка, ролик и папка `out/srvN` (только
внутри рабочей папки раннера). После итога `Failed` удаляется только демка; ролик и дубли остаются уликой, как и
задумано в D-03. Ограничение числа сохранённых упавших задач и очистка при старте не делались (нужна политика
хранения). Тест `flow` обновлён: вместо чтения демки после успеха проверяется, что её больше нет.

### WR-04: 401 на `finish`/`release` не останавливает раннер

**Статус:** fixed: requires human verification
**Commit:** 9e13261
**Files modified:** `crates/runner/src/lib.rs`
**Applied fix:** `report()` и `release()` при `Verdict::Retry` с `LinkError::Status{401}` возвращают ровно
`tee::TOKEN_REJECTED`, поэтому `run` останавливает раннер, а задача остаётся в состоянии. Автотеста нет: в стенде
нет способа отозвать токен посреди задачи.

### WR-05: Любой 404/409 без кода считался окончательным отзывом

**Статус:** fixed: requires human verification
**Commit:** 16c8f6e
**Files modified:** `crates/runner/src/transfer.rs`, `crates/runner/src/tee.rs`
**Applied fix:** минимальная безопасная версия. `judge` и пульс признают окончательными только распознанные коды
`ApiError` (`stale_epoch`, `job_terminal`, `not_found`); пустой 404/409 для POST/PATCH теперь `Retry`. Для `HEAD`
результата (ответ без тела по построению) добавлен `judge_head`, где пустой 404/409 по-прежнему отзыв. Подсчёт «N
подряд одинаковых ответов» не вводился (stateful-логика без тестового стенда).

### WR-06: Тело ответа демки не ограничено, сбой записи считался сетевым

**Статус:** fixed: requires human verification
**Commit:** 9327781
**Files modified:** `crates/runner/src/lib.rs`, `crates/runner/src/link.rs`, `crates/runner/src/transfer.rs`
**Applied fix:** `Link::demo_range` читает не больше `size - have` байт (`take`). `HashingFile` запоминает ошибку
записи, и `download_demo` возвращает `Err("ошибка записи демки на диск: ...")`; `Runner::fetch` по этому префиксу
закрывает задачу `JOB_ERROR` с подсказкой про свободное место вместо бесконечного повтора. Ошибки `flush`/`sync_all`
трактуются так же.

### WR-07: Токен раннера наследуется дочерними процессами

**Статус:** fixed
**Commit:** 848057f
**Files modified:** `crates/host/src/lib.rs`, `crates/host/src/runner.rs`, `crates/host/src/win/mod.rs`
**Applied fix:** константа `host::SECRET_ENV` (`CS2_RUNNER_TOKEN`); `run_bounded` (дочерний разбор, ffmpeg, ffprobe) и
команда запуска HLAE/CS2 вызывают `env_remove`. Добавлен тест: ребёнок не видит переменную.

### WR-08: Построчный буфер дочернего процесса не ограничен

**Статус:** fixed
**Commit:** ce6a98e
**Files modified:** `crates/host/src/runner.rs`
**Applied fix:** `read_line_capped` читает строку не длиннее 1 МиБ и выбрасывает хвост до перевода строки; канал
заменён на `sync_channel(4096)`. Добавлен тест на строку в 2 МиБ и чтение следующей строки.

### WR-09: `http://` отправляет токен открытым текстом

**Статус:** fixed
**Commit:** 83ed8a5
**Files modified:** `crates/runner/src/link.rs`, `crates/cli/src/runner_cmd.rs`
**Applied fix:** `Link::new` принимает `http://` только для `localhost`, loopback, частных и link-local IP-адресов
(IPv4 и IPv6), для остального возвращает ошибку «используйте https://». Флаг `--insecure-http` не вводился: имя
хоста по LAN придётся заменить на IP или поднять https. Справка `--server` обновлена, тест добавлен.

## Skipped Issues

None. Info-находки (IN-01..IN-07) вне области по условию запуска.

---

_Fixed: 2026-10-11_
_Fixer: Claude (gsd-code-fixer)_
_Iteration: 1_
