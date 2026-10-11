# Phase 3: Проверенные дубли, повтор и восстановление - Research

**Researched:** 2026-10-10
**Domain:** Rust workspace (std::thread, rusqlite, egui) driving HLAE/CS2 on Windows; take validation with ffprobe, retry-from-record, crash recovery, evidence retention
**Confidence:** HIGH on code seams and ffprobe behaviour (read/ran this session); MEDIUM on HLAE live behaviour (source read, not run live); LOW on real-world tolerance numbers (Phase 4 tunes them)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

Решения фаз 1 и 2 остаются в силе (`Host`, Job Object, `RecordFailure`, маркеры с nonce, `Deadlines`, `JobStatus::Cancelling`, уборка, `takeNNNN.partial`). Нумерация ниже своя для фазы 3.

**Проверка дубля и манифест**
- **D-01:** Допуск длительности дубля: измеренная длительность в пределах ±10% от плановой, но не меньше ±0.5 с. Значение — одна константа рядом с `Deadlines`; в фазе 4 подстраивается по приёмке.
- **D-02:** `ffprobe` запускается сразу после `TAKE_END n` (и завершения ffmpeg HLAE) для каждого дубля, а не одним проходом перед сборкой: обрезанный дубль виден за секунды. Вызов идёт через `Host::run` с дедлайном `ffprobe` из `Deadlines` и отменой. Файл дубля не читается до `TAKE_END n` с nonce задачи (TAK-02, TAK-05).
- **D-03:** Аудио обязательно: нет аудиопотока — дубль невалиден. Дополнительно проверяются видеопоток, длительность > 0 и разрешение (должно совпасть с настройкой записи).
- **D-04:** Невалидный дубль — сбой задачи с новым кодом (рабочее имя `TAKE_INVALID`) и номером дубля в сообщении и подсказке. Манифест помечает его `invalid`, остальные остаются `validated`. «Повторить» перезаписывает только невалидные и недостающие.
- **D-05:** `clips/manifest.json` ведётся инкрементально для каждой попытки: на дубль — ожидаемая длительность, статус (`pending/recorded/validated/invalid`), размеры, измеренная длительность, разрешение; на попытку — хеш плана, сборка CS2 (`steam.inf`), версия HLAE, версия ffmpeg, NVENC, свободное место, разрешение (OBS-03). Снимок окружения также лежит в папке задачи. «Уже записано» = «валидирован по манифесту»; прежняя проверка `plan.hash` + наличие файлов заменяется манифестом.
- **D-06:** `highlights.mp4` пишется во временное имя и переименовывается только после успешного завершения ffmpeg (TAK-06); при сбое временный файл удаляется.

**Повтор**
- **D-07:** «Повторить» дописывает только недостающие и невалидные дубли. Валидированные по манифесту остаются на месте, остальное (включая `.partial`) уходит в `clips_old_<ts>`. Если валидированы все — сразу сборка (RET-03). Требует `highlights.cfg`/`cs2c_*` на подмножество шотов с сохранением нумерации `takeNNNN` — выполнимость и способ выбирает исследование (запасной вариант: писать всё заново, кроме случая «все валидны»).
- **D-08:** Вход повтора лежит в папке задачи: `record.json` (nonce, пути к демке, настройки записи, ссылка на план) плюс существующие `plan.json`, `highlights.cfg`, `cs2c_*`. Нет данных (папку удалили) — повтор честно падает «нет данных, запустите задачу заново». Скачивание и планирование при повторе не выполняются (RET-02).
- **D-09:** В карточке две кнопки: «Повторить» (с этапа записи) и «Заново» (с самого начала, как сейчас). «Повторить» доступна только если есть `record.json`. Обе вместе с «Открыть папку» (OBS-02).

**Журнал и попытки**
- **D-10:** Журнал остаётся в SQLite (`append_log`); каждая строка получает префикс `[этап #попытка]`, при старте попытки пишется разделитель. Номер попытки — новая колонка `jobs.attempt` (новая миграция `user_version`). Журнал не стирается между попытками (OBS-01, RET-01).
- **D-11:** Длительность этапов: строка в журнале при смене этапа («этап Запись: 4 мин 12 с») плюс сводка «по этапам» в карточке из тех же меток. Отдельной таблицы нет.
- **D-12:** Улики: `record.log` → `record.log.N` при старте новой записи, без ограничения числа; дубли прошлой попытки — в `clips_old_<ts>` (RET-06).
- **D-13:** Карточка показывает «Попытка N», этап сбоя, код ошибки; прошлые попытки — ссылками на `record.log.N`.

**«Прервано» и осиротевшая игра**
- **D-14:** Новый вариант `JobStatus::Interrupted` — **Reversibility:** costly — новый статус в строках задач, старые версии приложения его не поймут (тот же случай, что `Cancelling` в фазе 2). В списке виден отдельным бейджем, показывается достигнутый этап и причина «приложение закрылось»; доступны «Повторить» и «Заново». `reset_running` ставит `Interrupted` вместо `failed` для статуса `running` (RET-04).
- **D-15:** При старте приложение завершает осиротевший процесс по сохранённым PID и времени создания (`cs2.exe`, `HLAE.exe`); совпадение обязательно по обоим признакам, чужие процессы не трогаются. Пары пишет `WinHost`, когда сессия поднялась. Хранилище — SQLite (миграция) или файл в папке задачи: форма на усмотрение планировщика. Уборка из D-08 фазы 2 идёт тем же проходом.
- **D-16:** Прерванные задачи в очереди не возобновляются сами: очередь стоит, пока пользователь не нажмёт «Продолжить» (как после отмены в фазе 2).

**Тесты (TST-02)**
- **D-17:** Все сценарии TST-02 идут на `FakeHost`: «лаунчер не стартовал», «игра не появилась», «окно ошибки HLAE», «консоль не открылась», «демка не загрузилась», «скрипт не запустился», «маркер дубля не пришёл», «дубль обрезан», «игра умерла посреди дубля», «отмена в каждом состоянии»; каждый заканчивается своим кодом ошибки. Фейк получает сценарий «обрезанный дубль» и подмену результата `ffprobe`.

### Claude's Discretion
- Имя и тексты нового кода `TAKE_INVALID` (и, если понадобится, кода для «нет данных для повтора») по-русски; формат `manifest.json` и `record.json`, версия схемы.
- Способ запуска записи подмножества шотов (D-07) и нумерация `takeNNNN` при этом.
- Где хранить PID и время создания процессов (D-15) и как их получать у `WinHost`.
- Формат сводки «по этапам» и разделителя попыток в карточке.
- Как fake отдаёт результат `ffprobe` в сценариях.

### Deferred Ideas (OUT OF SCOPE)
- Автоповтор сбоев класса «запуск», пауза очереди по повторяющемуся коду «окружение», уведомление в трее — фаза 4 (RET-05).
- Автоматический дозапуск невалидных дублей в том же прогоне — фаза 4 (автоповтор).
- Ограничение размера и ротация `record.log` по объёму/числу попыток — вне фазы.
- Авто-возобновление очереди после перезапуска — фаза 4.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TAK-02 | Take completion = `TAKE_END n` marker (+ HLAE ffmpeg done); size-stability only secondary; no mtime | `Machine` already gates on nonce markers; new `Action::Verify` fires on `TAKE_END`; HLAE `EndCapture` is async so the finalize-wait loop in "Verify design" is mandatory |
| TAK-03 | `clips/manifest.json` per attempt | "Manifest and record.json schemas"; module `pipeline::manifest`; atomic tmp+rename writes |
| TAK-04 | Each take ffprobe-validated before assembly (video+audio stream, duration>0 and in tolerance, resolution) | "ffprobe verified behaviour"; NOTE audio lives in `audio.wav`, video in `video.mp4` |
| TAK-05 | Take file not read while HLAE may write; "recorded" = "validated by manifest" | Verify only after `TAKE_END`; manifest replaces `plan.hash` + file-exists check (`jobs.rs:321-323`) |
| TAK-06 | `highlights.mp4` temp name then rename | "Assembly gate and temp name" |
| RET-01 | Failed stage, code, attempt number kept; log not wiped; attempts separated | store migration 4 (`attempt`), `start_job` stops clearing `log`, `[этап #N]` prefix |
| RET-02 | "Повторить" resumes at record stage without download/plan | `record.json` + resume branch in `jobs.rs::run` |
| RET-03 | All validated -> straight to assembly | `Manifest::missing()` empty => skip precheck+record |
| RET-04 | Orphan game killed at start; running jobs -> "прервано" | `JobStatus::Interrupted`, `processes.json` + `Host::kill_orphan` |
| RET-06 | Retry never overwrites evidence | `record.log` -> `record.log.N` rotation; `clips_old_<ts>` |
| OBS-01 | Log with timestamp/stage/attempt, stage durations | `JobReporter` prefix + stage-duration lines |
| OBS-02 | Card: code, state, message, hint, console tail, buttons "Повторить" + "Открыть папку" | `jobs_page.rs` changes; tail already shown by `error_block` |
| OBS-03 | Environment snapshot in job folder + manifest | `EnvSnapshot` built from `precheck::Report` (+NVENC flags, frame size) |
| TST-02 | Scenario tests on FakeHost | "TST-02 gap analysis" |
</phase_requirements>

## Summary

The phase is mostly **plumbing around three seams that already exist**: (1) `pipeline::record::record()` driver loop + pure `Machine`, (2) `store::Store` migrations/`JobStatus`, (3) `app::jobs::run()` which today re-downloads, re-parses and re-plans on every run and decides "already recorded" by a `plan.hash` file. The central new artefacts are `clips/manifest.json` (source of truth for "recorded"), `record.json` (inputs for retry-from-record), `processes.json` (orphan identities), a new `FailCode::TakeInvalid`, and `JobStatus::Interrupted`. No new crates are needed.

Four facts shape the design and were **not** obvious from CONTEXT.md: (a) HLAE numbers a new take as **max(existing `take????`)+1 — gaps are never filled** (source read at tag v2.192.7), so "record a subset while keeping `takeNNNN` numbering" is impossible by just leaving validated takes in place; (b) HLAE finishes `mirv_streams record end` **asynchronously on the render thread**, so the `TAKE_END n` echo arrives *before* `video.mp4` is finalized and an immediate `ffprobe` will race (`moov atom not found`); (c) the take's **audio lives in a separate `audio.wav`** (video.mp4 is consumed as video-only input), so D-03 "audio mandatory" must be checked on `audio.wav`, not `video.mp4`; (d) the app's assembly does **not** use `renderer::assemble::find_clip` (that is the CLI's legacy `shot_NN` path) — the app builds `clips/takeNNNN/video.mp4|audio.wav` inputs in `renderer::ffmpeg_args`, so the CLI path and its tests must stay untouched.

For D-07 (subset re-record) the recommendation is: **ship the all-or-nothing retry first** (satisfies RET-02/03/06 and every TAK/OBS requirement), and add the subset re-record as a separate, optional, last wave (it is requirement DIF-05, which REQUIREMENTS.md lists under *v2*). The subset mechanism that works without a `Match` is: observed take-dir binding in the manifest + text re-chaining of the generated cfg tails + cfg overrides at install time (details below).

**Primary recommendation:** Build `manifest.rs` + `verify.rs` in `pipeline`, hook them into the `record()` driver as a *non-blocking pending-probe list* (poll loop keeps running while HLAE finalizes), persist `record.json` after planning, and make `jobs.rs::run` branch on `resume` (column added by migration 4) — all-validated => assemble, otherwise re-record everything (baseline) or the missing subset (optional wave).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Take verification (ffprobe, tolerance, judge) | `pipeline::record` (library) | `host` (`Host::run`) | Platform-neutral policy; OS access only via `Host::run` with deadline+cancel |
| Manifest (`clips/manifest.json`) read/write/reconcile | `pipeline::manifest` | filesystem in job folder | Written by the record driver; read by app to decide retry scope |
| `record.json` (retry inputs) | `app::jobs` writes after planning; `pipeline` defines the DTO | filesystem | Planning happens in `pipeline::plan_player`, but the decision "this is a recordable job" is app-level |
| Attempt counter, `Interrupted`, resume flag | `store` (SQLite migration 4) | `app` | Persistent job state lives only in `store` |
| Stage prefix / stage-duration log lines | `app::jobs::JobReporter` | `store::append_log` | `JobReporter` is the single `Progress` impl that writes the DB log |
| Orphan process identity capture | `host` (`Session::processes`) | `pipeline::record` driver writes `processes.json` | Only `host` can read PID+creation time; pipeline owns the file next to `installed.json` |
| Orphan kill at app start | `host` (`Host::kill_orphan`, Windows) | `app::launch` recovery pass | Kill must hold one handle: open -> compare creation time+image -> terminate |
| Subset cfg re-chaining (optional) | `renderer` (pure text fn) | `app::launch::install_files` (overrides) | Needs only `Timeline` (Deserialize) — no `Match` |
| Assembly input list from manifest, temp output name | `renderer::ffmpeg_args*` + `app::jobs::run_ffmpeg` | — | `ffmpeg_args` is the real assembly builder for the app |
| Card UI (buttons, badges, attempt, stage summary, old logs) | `app::jobs_page` | `app::app` (cached fs lookups in `refresh_jobs`) | egui; avoid per-frame filesystem calls |

## Standard Stack

No new dependencies. Everything needed is already in the workspace.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde + serde_json | 1 (already in `pipeline`, `store`) | `manifest.json`, `record.json`, `processes.json`, ffprobe JSON | `[VERIFIED: crates/pipeline/Cargo.toml]` `serde = { version = "1", features = ["derive"] }` and `serde_json = "1"` |
| rusqlite (bundled) | 0.32 | migration 4 | `[VERIFIED: crates/store/Cargo.toml]` `rusqlite = { version = "0.32", features = ["bundled"] }` |
| windows-sys | 0.61 (features already enabled) | `OpenProcess`/`GetProcessTimes`/`TerminateProcess` already imported in `win/process.rs` | `[VERIFIED: crates/host/Cargo.toml]` features include `Win32_System_Threading`, `Win32_System_Diagnostics_ToolHelp`, `Win32_System_JobObjects`; no new feature needed |
| ffprobe (external) | 9.0.2 on the author's PATH | take validation | `[VERIFIED: ran ffprobe -version]` `ffprobe version 9.0.2-full_build-www.gyan.dev` |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Process identity in a job-folder file (`processes.json`) | SQLite table | File is written by `pipeline::record` (which has no `store` dependency) and read by the same recovery pass as `installed.json`; needs no migration. SQLite would force `pipeline` -> `store` coupling or an app-side callback. **Use the file.** |
| `renderer::TakePlan` with serde derive | pipeline-local DTO `TakeSpec` | Adding `serde` to `renderer` is allowed but unnecessary; a DTO in `pipeline` keeps `renderer` dependency-light. **Use a DTO.** |

**Installation:** none. **Version verification:** n/a (no new packages).

## Package Legitimacy Audit

This phase installs **no external packages**; all crates used are already in `Cargo.lock`. Audit table not applicable.

**Packages removed due to [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### Data flow (target)

```
                       "Заново" (resume=0)                         "Повторить" (resume=1, needs record.json)
 jobs::run ──► precheck ──► analyze/download ──► plan_player ──► write record.json ──┐        │
                                                                                       ▼        ▼
                                                                  load record.json + clips/manifest.json
                                                                                       │
                              reconcile(manifest, clips/, sizes) ──► all validated? ──┼── yes ──► assemble (no precheck, no game)
                                                                                       │ no
                                              space pass ► move non-validated to clips_old_<ts> ► install files
                                                                                       ▼
 pipeline::record(host, req{verify:Some}) ─ poll loop ─┬─ Machine: markers/deadlines (unchanged ladder)
                                                       ├─ TAKE_END n ─► Action::Verify(n) ─► pending_probes
                                                       │                 (every ≥1 s: stat sizes ► ffprobe video.mp4 + audio.wav
                                                       │                  via Host::run(deadline=ffprobe, cancel))
                                                       │       valid ──► manifest: validated        invalid/timeout ─► Input::TakeInvalid ─► Finish(Err TAKE_INVALID)
                                                       ├─ GameAppeared ─► Session::processes() ─► processes.json
                                                       └─ Finish(Ok) ─► shutdown ─► verify the last take(s) ─► remove processes.json
                                                                                       ▼
                  assemble: ffmpeg → highlights.part.mp4 ─ success ─► rename to highlights.mp4   failure ─► delete .part only
```

### Recommended structure (new/changed files)

```
crates/pipeline/src/
├── manifest.rs            # NEW  Manifest, TakeEntry, TakeStatus, AttemptEntry, EnvSnapshot, load/save(atomic), missing(), reconcile()
├── record/verify.rs       # NEW  Verify{ffprobe,size}, probe_file(), judge(), within_tolerance(), TAKE_TOLERANCE_*
├── record/mod.rs          # CHANGED  RecordRequest.verify: Option<Verify>; pending probes; manifest writes; processes.json
├── record/machine.rs      # CHANGED  Action::Verify(index); Input::TakeInvalid{index,message}
├── record/failure.rs      # CHANGED  FailCode::TakeInvalid (+ msg_* in Russian), maybe FailCode::RetryNoData
├── record/deadlines.rs    # CHANGED  take_finalize (+scaled), tolerance consts next to Deadlines
├── record/log.rs          # CHANGED  open(): rotate record.log -> record.log.N instead of append
├── precheck/mod.rs        # CHANGED  Report gets #[serde(default)] env facts (ffmpeg/ffprobe first lines, steam patch, free bytes)
crates/host/src/
├── lib.rs                 # CHANGED  ProcIdentity, OrphanOutcome, Session::processes() (default empty), Host::kill_orphan() (default Unsupported)
├── win/mod.rs, win/process.rs   # CHANGED  WinSession::processes(), process::kill_if_same()
├── fake.rs                # CHANGED  ffprobe auto/override responses, orphan kills, optional subset indices
crates/store/src/lib.rs    # CHANGED  MIGRATIONS[3], JobStatus::Interrupted, attempt/resume, start_job keeps log, reset_running -> interrupted
crates/app/src/
├── jobs.rs                # CHANGED  resume branch, record.json, manifest-driven "recorded", JobReporter prefix/durations, temp-name assembly
├── launch.rs              # CHANGED  recover(): orphan kill first, manifest-aware .partial marking, install overrides (optional wave)
├── app.rs                 # CHANGED  startup order, paused-after-interrupt, cached resumable/old-log lookups
├── jobs_page.rs           # CHANGED  badges, Повторить/Заново/Открыть папку, "Попытка N", stage summary, record.log.N links
crates/renderer/src/lib.rs # CHANGED  ffmpeg_args_with(t,cfg,dirs,out_name); (optional) rechain()
```

### Pattern 1: ffprobe through `Host::run` (verified behaviour)

`Host::run(&CmdSpec, cancel, on_line)` hands stdout lines to `on_line` (trimmed, empty lines dropped) and only `stderr_tail` (last 20 lines) in `CmdOutput`. `[VERIFIED: crates/host/src/lib.rs:246-253]` `fn run(&self, cmd: &CmdSpec, cancel: &dyn Fn() -> bool, on_line: &mut dyn FnMut(&str)) -> Result<CmdOutput, HostError>;` and `[VERIFIED: crates/host/src/runner.rs:41-47]` the reader does `let line = String::from_utf8_lossy(&buf).trim_end().to_owned();` and sends only `!line.is_empty()`. JSON therefore arrives as many lines: join with `\n` into a `String` (cap at e.g. 64 KiB; the output is untrusted file metadata) and parse with `serde_json`.

```rust
// Source: flags run on ffprobe 9.0.2 in this session (output reproduced below)
let args = ["-v", "error", "-print_format", "json",
            "-show_entries", "stream=codec_type,codec_name,width,height,duration:format=duration",
            "-i"]; // + absolute path
```
Real output for a 2 s 320x180 mp4 (stdout) `[VERIFIED: ran ffprobe 9.0.2]`:
`"streams": [{"codec_name": "h264", "codec_type": "video", "width": 320, "height": 180, "duration": "2.000000"}], "format": {"duration": "2.000000"}` — **durations are JSON strings**, width/height are numbers. A WAV gives `"codec_name": "pcm_s16le", "codec_type": "audio", "duration": "2.000000"` and no width/height. Parse `duration` as `str -> f64`, reject non-finite/negative.

Failure modes measured here (so `judge()` can be exact):
- mp4 without `moov` (cut file or unfinalized): **exit code 1**, stdout `{ }`, stderr `... moov atom not found` / `Invalid data found when processing input`. => "not finalized yet" signal.
- Missing file: exit 1, stderr `nope.mp4: No such file or directory`.
- Truncated WAV (header says 10 s, 50 000 bytes present): **exit 0** and a *shorter* duration (`0.566009` s). => caught by the duration tolerance, not by exit code.
- Where it is defined: `Deadlines.ffprobe` `[VERIFIED: crates/pipeline/src/record/deadlines.rs:40-41]` `/// \`ffprobe\` на одном файле.` / `pub ffprobe: Duration,`, default `[VERIFIED: deadlines.rs:79]` `ffprobe: unit(30),`, scaled at `deadlines.rs:118` (`ffprobe: d(self.ffprobe),`). Precheck already uses a `PathBuf` named `ffprobe` (`Inputs.ffprobe`, `[VERIFIED: crates/pipeline/src/precheck/gather.rs:47]`), the app passes `ffprobe: "ffprobe".into()` (`jobs.rs:177`) — reuse that path for verification.

### Pattern 2: Take validation = two probes + a pure judge

Per take, probe **both** files named by `TakePlan::FILES` `[VERIFIED: crates/renderer/src/lib.rs:282]` `pub const FILES: [&'static str; 2] = ["video.mp4", "audio.wav"];`:

| File | Must have | Notes |
|------|-----------|-------|
| `video.mp4` | a `codec_type == "video"` stream, `width/height == requested frame`, `format.duration` > 0 and `within_tolerance(expected_secs, d)` | Expected duration = `TakePlan.game_secs` (shot length in game seconds; video is written at capture fps so its duration equals game time — the ffgraph's `retime` uses the same clip-time basis, `lib.rs:427-429`) |
| `audio.wav` | a `codec_type == "audio"` stream, `duration` > 0, `within_tolerance(expected_secs, d)` | **D-03's "audio mandatory" applies here.** The assembly feeds video and audio as *separate* `-i` inputs: `[VERIFIED: crates/renderer/src/lib.rs:603]` `let inputs = (0..t.shots.len()).flat_map(\|i\| [clip_path(take_of[i]), audio_path(take_of[i])]).collect();` and consumes `[{vi}:v]` and `[{ai}:a]` (`lib.rs:545`, `lib.rs:568`). Requiring an audio stream inside `video.mp4` would invalidate every take. |

`within_tolerance(expected, measured) = (measured - expected).abs() <= max(0.10 * expected, 0.5)` (D-01; two `pub const` next to `Deadlines`). Judge returns `Result<TakeFacts, String>` with a Russian reason ("audio.wav: нет аудиопотока", "video.mp4: длительность 1.2 с, ожидалась 5.0 с (допуск ±0.5 с)", "разрешение 1920x1080, ожидалось 1280x720").

### Pattern 3: Finalize-wait as a non-blocking pending list inside the driver loop

`TAKE_END n` is printed by the same command batch as `mirv_streams record end`, but in CS2 the capture shutdown is queued to the render thread: `[CITED: github.com/advancedfx/advancedfx @v2.192.7 AfxHookSource2/RenderSystemDX11Hooks.cpp:3297-3317]` `EndCapture()` pushes `capture->ShutDown(pDeviceContext)` onto `AfterPresentOrContextLossReliable` and `delete capture` onto `FinalizeReliable`. So the mp4 (and the WAV header) are finalized **after** the marker; an immediate `ffprobe` can see `moov atom not found`. Existing code already knows the HLAE ffmpeg child lingers after the game exits (`Closing` waits for an empty job, `machine.rs:547-554`).

Algorithm (prescriptive), driver-side, called from the existing loop so events/cancel/deadlines keep flowing:
1. On `Action::Verify(n)`: push `Pending{ index, first_at: now, last_sizes: None, next_at: now + 200ms }`; mark manifest entry `recorded` (sizes may be unknown yet).
2. Each loop iteration (the loop already polls ≤100 ms): for the first pending with `now >= next_at`: stat both files, run the two probes (`Host::run`, `deadline = d.ffprobe`, `cancel = p.cancelled()`), then:
   - valid => manifest `validated`, drop from list, emit fact "дубль n/m проверен: 5.02 с, 1280x720, звук 5.00 с".
   - invalid and (`now - first_at < d.take_finalize` **and** (sizes changed since last attempt **or** `now - first_at < 2 s`)) => `next_at = now + 1 s` (still finalizing; size stability is only the *secondary* signal TAK-02 allows).
   - otherwise => final `invalid`: `machine.step(now, Input::TakeInvalid{ index, message })` => `Finish(Err(TAKE_INVALID))`.
3. `HostError::Cancelled` from `run` => treat exactly like `p.cancelled()` (`Input::Cancel`). `HostError::Timeout`/`Spawn` => invalid with the host error text (do not retry a missing ffprobe).
4. After `Action::Finish(Ok)` and `shutdown()` (job empty => HLAE ffmpeg is done): drain the remaining pending probes **without waiting** (one attempt each). If one fails, convert the `Ok` into `Err(TAKE_INVALID, state=Take)` — this is how the **last** take is covered.
5. Add `take_finalize` to `Deadlines` (start at 20 s; ARCHITECTURE.md suggested 30 s, `[ASSUMED]` either is fine — Phase 4 tunes) and to `scaled()`.

`FakeHost.run` does not move the virtual clock, but the pending list waits via `session.poll(TICK)`, which does — so retries cost microseconds in tests.

Fail-fast: the first invalid take aborts the run with `FailCode::TakeInvalid` (`state = RecordState::Take`), game shutdown reuses the existing `shutdown()` (`stop_for` falls to `Stop::Graceful` for unknown codes, `mod.rs:267`), and `mark_partial` keeps the invalid dir (it is in `machine.timings()`), which the retry later moves to `clips_old_<ts>` as evidence.

### Pattern 4: `record.json`, `manifest.json`, `processes.json` (schemas, version field on each)

`clips/manifest.json` (written atomically: write `manifest.json.tmp`, then `std::fs::rename` — replaces an existing file on Windows `[ASSUMED: Rust std::fs::rename docs]`). Created at the start of every recording attempt, updated on every state change, travels with `clips/` into `clips_old_<ts>/` (history for free):

```json
{
  "version": 1,
  "plan_hash": "9f3c…",          // FNV of highlights.cfg+cs2c_* with nonce normalized (reuse jobs.rs plan_hash)
  "nonce": "ab12cd34",
  "frame": [1280, 720],            // requested recording size — reconcile marks takes with another size invalid
  "takes": [{
    "index": 0, "dir": "take0000", "expected_secs": 5.0, "capture_fps": 240,
    "status": "validated",         // pending | recorded | validated | invalid
    "video_bytes": 123456, "audio_bytes": 998877,
    "measured_secs": 5.02, "audio_secs": 5.00, "width": 1280, "height": 720,
    "error": null, "attempt": 1
  }],
  "attempts": [{ "n": 1, "started_unix": 1760000000, "env": { /* EnvSnapshot */ }, "outcome": "TAKE_INVALID take 3" }]
}
```
`dir` is `Option<String>` and, for the baseline, always `take{index:04}`; it exists so the optional subset wave can bind HLAE-assigned dirs without a schema change. **Any dir name read back from disk must match `^take\d{4}$` before it is joined to a path** (the file is untrusted input; see Security).

`record.json` (written by `app::jobs::run` right after `plan_player` succeeds and the `console_safe` check passes, before `Stage::Record`; **delete a stale one when a fresh full run starts**): `{version, nonce, demo_path, demo_name, frame:[w,h], fps, take_nvenc, encoder, takes:[{index,shot,game_secs,capture_fps}], ffmpeg_args, video_secs, shots, secs, plan_hash, created_unix}`. `ffmpeg_args`/`video_secs` must be persisted because `Plan` is not stored anywhere today (`Plan` lives only in memory, `pipeline/src/lib.rs:169-180`). `demo_name` is `format!("cs2cinema_{}", pipeline::console_name(&a.label))` (`jobs.rs:336`) — store the result, do not recompute from a label that retry doesn't have. Resume must launch at `record.json.frame`, **not** the current `Settings.frame_size()` (user may have changed resolution; mixing it with old cfgs/ffgraph is a silent bug).

`processes.json` (in the job folder, next to `installed.json`): `{version, launcher:{pid,created,image}, game:{pid,created,image}}` where `created` is the Windows FILETIME `u64` from `OwnedProc::creation_time()` `[VERIFIED: crates/host/src/win/process.rs:137]` `pub fn creation_time(&self) -> Option<u64>`. Written by the driver on `SessionEvent::GameAppeared` (via the new `Session::processes()`), deleted after `shutdown()` returns a confirmed `Liveness::Exited`.

### Pattern 5: Retry flow in `jobs::run` (resume branch)

Today `run` always: precheck -> `analyze` -> `plan_player` -> decide `recorded` from `plan.hash` -> clips_old -> install -> record -> assemble `[VERIFIED: crates/app/src/jobs.rs:256-371]`. Prescriptive new shape:

```
let resume = job.resume (column) && record.json readable;
if job.resume && no record.json -> Err(typed RETRY_NO_DATA: «нет данных, запустите задачу заново»)   // D-08
if resume {
    rj = RecordJson::load(out_dir)?;              // out_dir = job.out_dir (already in DB via set_out_dir)
    manifest = Manifest::load(clips) ; reconcile(sizes; re-probe only entries whose sizes changed)
    if manifest.missing(&rj.takes).is_empty() { skip precheck + game; goto assemble }        // RET-03
    precheck(Demo::File(rj.demo_path))            // missing demo => existing PRECHECK_FAILED, hint «Заново»
    skip analyze/plan; build RecordRequest from rj (frame from rj, NOT Settings)
} else { existing flow (+ write record.json) }
```
`record_request()` currently takes `&pipeline::Plan` (`jobs.rs:136`); change it to take `(takes, nonce, frame)` so both paths share it. For the demo copy, `Analysis::write_demo` needs the parsed `Analysis` (`pipeline/src/lib.rs:320`); add a small `pipeline::write_demo_copy(demo_path, dest)` = `demo::read_demo` + `fs::write` (no `demo::parse`, so retry stays cheap).

"Заново" keeps today's behaviour, including reuse of takes whose manifest `plan_hash` equals the new plan's hash (that is what the removed `plan.hash` check did; see Open Question 2).

### Pattern 6: Store changes (template = how `Cancelling` was added in 5e3bea7)

- Current schema: `MIGRATIONS` has 3 entries, tests assert `user_version == 3` (`[VERIFIED: crates/store/src/lib.rs:710]` `assert_eq!(user_version(&s), 3);`, also lines 713, 728, 734, 752, 771). Add entry 4: `ALTER TABLE jobs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0; ALTER TABLE jobs ADD COLUMN resume INTEGER NOT NULL DEFAULT 0;` and bump those assertions to 4; add a "version 3 database gains attempt/resume" test in the same style as `version_two_database_gains_precheck_and_last_good`. The comment block above `MIGRATIONS` (`lib.rs:235-238`) documents versions — extend it.
- `JobStatus` `[VERIFIED: crates/store/src/lib.rs:45-53]`: `Queued, Running, /// Отмена … Cancelling, Done, Failed, Cancelled`. Add `Interrupted` with string `"interrupted"`, title `"прервана"`; `parse` currently maps unknown strings to `Failed` (`lib.rs:73` `_ => JobStatus::Failed,`) — so an *old* app reading a DB written by the new one shows "ошибка" (benign degradation; D-14's "costly" is only that rows can't be un-written).
- `is_active()` stays false for `Interrupted`.
- `start_job` today wipes the log: `[VERIFIED: crates/store/src/lib.rs:363]` `UPDATE jobs SET status = 'running', stage = '', error = NULL, result = NULL, log = '', fail_code = NULL, …`. Remove `log = ''`, add `attempt = attempt + 1, resume = 0` and make it return the new attempt number; write the separator line with `append_log` from the caller. `requeue` already keeps the log. Add `requeue_resume(id)` (sets `resume = 1`).
- `reset_running` today: `[VERIFIED: crates/store/src/lib.rs:497]` `'прервано: приложение закрылось во время выполнения'` with `status = 'failed'`. Change to `status = 'interrupted'`, **keep `stage`**, copy `phase` into `fail_state` (so `ladder()` can draw the reached step), `fail_code = 'INTERRUPTED'`, `error = 'приложение закрылось во время выполнения'`, `fail_hint` explaining «Повторить» vs «Заново». The existing test `crash_recovery_turns_cancelling_into_cancelled_and_running_into_failed` and `crash_recovery_and_caches` assert `Failed` and must be updated. `interrupted_jobs()` (running+cancelling rows) is unchanged and is the input of the recovery pass — it is a *different thing* from the new status; keep the name or rename to avoid confusion.

### Pattern 7: Log prefix, attempts, stage durations (OBS-01, D-10, D-11)

`JobReporter::log` writes `"{HH:MM:SS}  {m}"` to the DB `[VERIFIED: crates/app/src/jobs.rs:90]` and sends the **raw** `m` to the UI (`Msg::Log(m.to_owned())`). `status.rs` parses raw prefixes `"запись: дубль "` / `"записан дубль "` (`status.rs:68,75`) — so add the `[этап #N]` prefix **only to the DB line**, never to the `Msg::Log` payload.
- Add `attempt: u32` and `cur: Mutex<(String, Instant)>` to `JobReporter` (it must stay `Sync`).
- `stage(s)`: if a previous stage exists append `этап {prev}: {dur}` (format `fmt_dur`: «37 с», «4 мин 12 с», «1 ч 02 мин»), then switch. After `run()` returns (success **or** error) in `run_next`'s thread, close the current stage the same way. `Stage::Done` is never emitted by `run` today, so the closing call is required.
- Separator at attempt start (after `start_job`): e.g. `──── попытка {n} · {date} · {Заново|Повторить} ────`.
- Card summary "по этапам" = parse the **last attempt block** of `job.log` for lines matching the single-source `format_stage_line`/`parse_stage_line` pair (unit-test the round trip so the UI never drifts from the writer).
- Prefix label when no `Stage` is set yet (precheck phase): use «Предпроверка»/«Старт»; discretion.

### Pattern 8: `record.log` rotation (RET-06, D-12)

`RecordLog::open` currently opens `append(true)` `[VERIFIED: crates/pipeline/src/record/log.rs:28-31]` and the test `record_log_is_written_on_success_and_on_failure_with_source_tags` asserts two `==== запись` blocks in one file (`tests/record.rs:520`). New behaviour: if `record.log` exists and is non-empty, rename it to `record.log.{N}` with N = smallest unused integer ≥ 1 (scan the dir; do **not** derive N from `jobs.attempt`, because attempts that fail in precheck write no `record.log`, and two job rows can share one out_dir). Update that test: the first attempt moves to `record.log.1`, the new one is `record.log`. The card lists `record.log.*` files found by a scan cached in `refresh_jobs()` (like `JobsState.sizes`, `app.rs:326-341`), never per frame.

### Pattern 9: Assembly gate and temp name (TAK-06)

- The app's assembly is `jobs::run_ffmpeg` on `plan.ffmpeg_args` (built by `renderer::ffmpeg_args`, `lib.rs:609`), **not** `renderer::assemble` (CLI-only legacy; `crates/cli/src/main.rs:424` and its `shot_NN` layout). Do not modify `assemble.rs`.
- The output name is the last token of the encoder args: `[VERIFIED: crates/renderer/src/lib.rs:71]` `…-c:a aac -b:a 192k highlights.mp4"` (same on `lib.rs:72`). `ffmpeg_args` is only called from `pipeline::plan_player` and a renderer test (`lib.rs:724`), so extending it is low risk: add `ffmpeg_args_with(t, cfg, dirs: &[String] /*per record-order index*/, out_name: &str)`; keep `ffmpeg_args` as a wrapper (`take{i:04}` dirs, `highlights.mp4`). For retry, `record.json.ffmpeg_args` is already rendered, so do a checked token replace of the **last** arg (assert it equals `highlights.mp4`) with `highlights.part.mp4` (keep `.mp4` so ffmpeg infers the muxer; `-y` is already in the args).
- Before ffmpeg: gate on `manifest` — every planned take `validated`, dir exists, sizes match the manifest; otherwise fail with `TAKE_INVALID`-style message naming the take (TAK-04 "до сборки").
- On success `std::fs::rename(part, highlights.mp4)` (replaces an existing file on Windows `[ASSUMED]`; fails with a sharing violation if a player has the old file open — surface that error text). On any failure delete **only** the `.part`; today's code deletes `highlights.mp4` itself on failure (`[VERIFIED: jobs.rs:359]` `let _ = std::fs::remove_file(&out);`), which would destroy a previous good result — remove that line.
- Optional but cheap: `ffprobe` the `.part` (video+audio streams, duration within the same tolerance of `plan.video_secs`) before the rename; ACC-02 in Phase 4 wants this check anyway.

### Pattern 10: Orphan process kill (D-15)

- Why it is a *fallback*: the job uses `KILL_ON_JOB_CLOSE` `[VERIFIED: crates/host/src/win/job.rs:55]` `info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;`, defined by Microsoft as `[CITED: learn.microsoft.com/…/ns-winnt-jobobject_basic_limit_information]` "Causes all processes associated with the job to terminate when the last handle to the job is closed." When the app process dies its handles close, so the common crash case already kills the game. The identity file matters for a game that never entered the job (`adopted_without_job`, `watch.rs:223`) and for the HLAE ffmpeg child.
- Capture: the game identity is `Shared.game: Mutex<Option<OwnedProc>>` (`win/mod.rs:55`), set in `watch::find_game`/`adopt_game` right before `SessionEvent::GameAppeared { pid }` (`watch.rs:137-138`). The launcher's PID and creation time already exist in `launch_session` as `child.id()` and `launcher_created` (`win/mod.rs:205-207`) but HLAE.exe exits in 2-3 s (`watch.rs:3-4`), so it only matters in a crash within the first seconds. New API (defaults keep `UnsupportedHost` and old tests compiling):
  ```rust
  pub struct ProcIdentity { pub pid: u32, pub created: u64 /*FILETIME*/, pub image: String /*"cs2.exe"|"HLAE.exe"*/ }
  trait Session { fn processes(&self) -> Vec<ProcIdentity> { Vec::new() } }
  enum OrphanOutcome { Killed, Gone, Mismatch(String), Failed(String), Unsupported }
  trait Host { fn kill_orphan(&self, id: &ProcIdentity) -> OrphanOutcome { OrphanOutcome::Unsupported } }
  ```
- Kill (Windows, `win/process.rs`): `OwnedProc::open_owned(pid)` (has `PROCESS_TERMINATE`, `[VERIFIED: process.rs:57-73]`) -> `Err` => `Gone`; `creation_time() != Some(id.created)` => `Mismatch` (PID reused); `image_path()` file name must equal `id.image` case-insensitively **and** be in an allow-list `{cs2.exe, hlae.exe, ffmpeg.exe}` regardless of what the file says; already `Exited` => `Gone`; `kill(1)` then `wait(3 s)`. Everything runs on **one handle**, so there is no check-then-kill PID race. Never match by name alone.
- Fake: `Session::processes()` returns launcher+game identities after `GameAppeared`; `FakeHost` records `kill_orphan` calls and returns scripted outcomes so the startup pass is unit-testable.
- Startup order (refines CONTEXT's "тем же проходом"): in `App::new` (`app.rs:179-188`) for each `store.interrupted_jobs()` row with a folder: **(1) kill orphans from `processes.json`, (2) `launch::recover` (cleanup + `.partial`), (3) after the loop `reset_running`**. Kill must precede cleanup because a live `cs2.exe` holds the demo copy, and `remove_with_retry` gives up after 3×500 ms (`launch.rs:206-220`). Log every step with `append_log`.
- D-16: after `reset_running`, if it produced any interrupted rows set `jobs.paused = true` **before** `jobs::run_next` (`app.rs:229-232`). The banner text in `jobs_page.rs:56` says «после отмены» — generalize (e.g. keep a `paused_reason`).

### Pattern 11: Retry button logic (`jobs_page.rs`)

Existing: `Act::Retry(i64)` -> `app.store.requeue(id)` + `run_next` `[VERIFIED: crates/app/src/jobs_page.rs:77-85]`; the button exists only for `Failed | Cancelled` (`:214-218`). Both `match j.status` blocks (`:120-154`, `:188-219`) are exhaustive, so adding `Interrupted` forces the compiler to find every site. New: `Act::Retry(id)` (resume) and `Act::Redo(id)` ("Заново" = today's `requeue`), buttons for `Failed | Cancelled | Interrupted`; «Повторить» enabled only if `app.jobs.resumable.contains(&id)` (a `HashSet<i64>` filled in `refresh_jobs()` by checking `record.json` once, never per frame); «Открыть папку» (`ui::open_path(dir)`) for every status with an `out_dir`. "Попытка N" when `attempt >= 2` (always in the failure block). `ladder()` must treat `Interrupted` like `Failed` (use `fail_state`).

### Pattern 12 (OPTIONAL WAVE): subset re-record (D-07)

**Facts.** `[CITED: github.com/advancedfx/advancedfx @v2.192.7 shared/FileTools.cpp SuggestTakePath, and AfxHookSource2/RenderSystemDX11Hooks.cpp:5235 `SuggestTakePath(m_TakeDir.c_str(), 4, m_TakeDir)`]`: HLAE lists `take????` in the record folder, takes `iMax = max(last 4 chars as int)` and creates `take{iMax+1:04}`; gaps are never filled; names longer than 8 chars (`take0003.partial`) do not match. So leaving validated `take0000, take0002, take0004` in `clips/` and recording shots 1 and 3 yields `take0005, take0006`.

**Therefore (prescriptive):**
1. *Mapping, not numbering.* Move every non-validated dir (`invalid`, `recorded`, `.partial`, strays) into `clips_old_<ts>/`; leave validated dirs. Snapshot the set of `take????` dir names before launch. At each `TAKE_END n` list `clips/`, take the **one** new `take????` dir (0 or ≥2 new => `TAKE_INVALID` "HLAE не создал / создал несколько папок") and bind it to take `n` in the manifest (`dir`). Do not predict names; observe them.
2. *Consume the mapping.* Assembly inputs come from `ffmpeg_args_with(…, dirs, …)`; `mark_partial` and `launch::recover` must work from manifest dirs instead of shot indices (today `done` is a list of **shot** indices, `mod.rs:553`, `launch.rs:237`).
3. *Chain only the subset, without a `Match`.* `renderer::render` needs `Match` (slots for `spec_player`, `server_start_tick`) which is not persisted, so retry cannot call it. The generated files differ for a subset only in (a) the last three lines of `highlights.cfg` (`exec cs2c_NN`, `demo_gototick`, `demo_resume`, `lib.rs:199-203`) and (b) the **tail** after `echo CS2C <nonce> TAKE_END i; ` in each included `cs2c_NN.cfg`'s final `mirv_cmd addAtTick {end} "…"` line (`lib.rs:236-246`). Factor the tail computation out of `render` into `fn tail_for(t, order, i, next, nonce)` and add `renderer::rechain(files, &Timeline, &[usize], nonce) -> Vec<(String,String)>` (text surgery keyed on the unique end-marker string; `Timeline` derives `Deserialize` `[VERIFIED: crates/director/src/lib.rs:196]`, and `plan.json` is exactly `serde_json::to_string_pretty(t)`, `renderer/src/lib.rs:163`). Test: `rechain(all shots)` must be **byte-identical** to the original files; subset golden tests for first/middle/last tail variants (adjacent shots = plain `exec`, else `exec …; demo_gototick N`, last = `ALL_DONE` tail).
4. *Don't touch originals.* Write the re-chained scripts only into the game cfg folder via an `overrides: &[(String,String)]` parameter of `install_files` (recorded in `installed.json` so cleanup removes them). `plan_hash` (computed from out_dir scripts) stays stable.
5. *Markers/labels.* `Machine` already uses `takes[self.take].index` for `TAKE_START/END` and the 1-based position for "дубль n из m"; pass the subset `Vec<TakePlan>` with original `index` values. The fake must emit markers for those indices (`Scenario::take_indices(Vec<u32>)`; today it emits `0..take_durations.len()`, `fake.rs:625-643`).
6. *Live verification once (author's machine, not CI):* put a dummy `clips/take0005/`, record one take, confirm `take0006`. This is the only way to confirm the HLAE-numbering assumption on the installed build.

**Cost/risk.** ~1 renderer plan (rechain + tests), ~1 pipeline/app plan (mapping, overrides, fake). **Fallback = baseline** (move whole `clips/` to `clips_old_<ts>` exactly as `jobs.rs:332-334` does today and re-record everything unless all are validated); the planner can drop this wave without touching any requirement ID. REQUIREMENTS.md lists "Перезапись только отсутствующих или невалидных дублей" as **DIF-05 (v2)**, so this is scope beyond RET-02/03.

### Anti-Patterns to Avoid
- **Probing before `TAKE_END n` / using mtime or "quiet for N s"** as completion: forbidden by TAK-02 and listed under Out of Scope.
- **Requiring an audio stream in `video.mp4`**: every take would fail (audio is `audio.wav`).
- **Blocking the poll loop in a retry-sleep** while HLAE finalizes: game death, `AFXERROR`, cancel and the next `TAKE_START` would be delayed. Use the pending list.
- **Keeping validated takes under `takeNNNN` and recording lower indices** (numbering breaks, see Pattern 12).
- **Resuming with `Settings.frame_size()`** instead of `record.json.frame`.
- **Deleting `highlights.mp4` on assembly failure** (destroys a previous good result).
- **Trusting `manifest.json`/`record.json`/`processes.json` contents** (paths, dirs, PIDs) without validation.
- **Per-frame filesystem calls in `jobs_page`** (cache in `refresh_jobs`).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Running ffprobe/ffmpeg with timeout+cancel | `std::process::Command::output()` | `Host::run` / `CmdSpec` | Pipes drained on threads, 100 ms deadline/cancel polling, kill on exit; `Command::output()` without deadline is banned on the record path (TMO-03) |
| JSON read/write | manual string building | `serde_json` + `#[derive(Serialize, Deserialize)]`, `#[serde(default)]` on every new field | Old files/DB rows must keep parsing |
| Atomic file replace | in-place `fs::write` of manifest | write `*.tmp` then `std::fs::rename` | A crash mid-write must not corrupt the only record of validated takes |
| Process identity / kill | `taskkill`, name matching, `sysinfo` | `OwnedProc` (`open_owned`, `creation_time`, `image_path`, `kill`) | Single handle => no PID-reuse race; project forbids name-based kill (TMO-05) |
| Cfg for a subset | regenerating via `render` with a fake `Match` | `rechain` text transform with a byte-identical round-trip test | `Match` isn't persisted |
| Time/clock in tests | `thread::sleep` | `FakeHost` virtual clock via `Session::poll` | 35 existing record tests run in 0.03 s `[VERIFIED: ran cargo test -p pipeline --test record]` |
| Plan fingerprint | new hash | existing FNV `plan_hash` (`jobs.rs:376`) with nonce normalized | Already stable across runs and tested (`plan_hash_ignores_the_nonce`) |

**Key insight:** every "wait until the file is ready" and "which dir is mine" question has an evidence-based answer (marker + ffprobe success + observed dir set); anything based on time or file names is exactly what the project's research already rejected.

## Runtime State Inventory

Not a rename/refactor phase — but the phase **changes persisted formats**, so the equivalent audit:

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | SQLite `jobs` rows (schema user_version 3), `status` strings; existing `log` TEXT | Migration 4 (add columns only); `interrupted` is a new status string; old apps parse unknown status as `failed` (`lib.rs:73`) |
| Live service config | None — no external service holds job names | None — verified by reading `store`/`app` |
| OS-registered state | None (no scheduled tasks/registry); game processes only | None |
| Secrets/env vars | FACEIT key in `settings.json` (not touched); `processes.json` holds no secrets | None |
| Build artifacts | Existing out folders contain `clips/plan.hash`, `installed.json` (with shot-index `done_takes`), `record.log` (append format) from phase 1-2 runs | `plan.hash` becomes dead (ignore); `record.log` first rotation moves the old append-style file to `record.log.1`; `installed.json.done_takes` stays `#[serde(default)]` and is superseded by the manifest in `recover()` |

## Common Pitfalls

### Pitfall 1: `TAKE_END` arrives before the file is final
**What goes wrong:** immediate `ffprobe` returns exit 1 `moov atom not found`; a naive implementation fails every take.
**Why:** HLAE queues capture shutdown on the render thread (`EndCapture`, cited above); the in-game ffmpeg writes `moov` last. WAV header is also finalized late.
**How to avoid:** Pattern 3 (retry while sizes change / within `take_finalize`); last take verified after `shutdown()`.
**Warning signs:** first take always invalid on a fast machine; `moov atom not found` in `record.log`.

### Pitfall 2: Crash recovery marks validated takes `.partial`
**What goes wrong:** `install_files` resets `done_takes` (`launch.rs:128` `Installed { files: old.files, done_takes: Vec::new() }`) and `note_take_done` only records takes of the *current* run; a crash during a partial re-record would rename validated takes to `.partial` via `recover()` -> `mark_partial`.
**How to avoid:** `recover()` computes `done` from the manifest (`recorded` ∪ `validated` dirs); fall back to `done_takes` only when no manifest exists.

### Pitfall 3: Existing tests encode the old semantics
Known to change: `record_log_is_written_on_success_and_on_failure_with_source_tags` (append -> rotate), store tests asserting `user_version == 3` (6 places) and `Failed` after `reset_running` (`crash_recovery_and_caches`, `crash_recovery_turns_cancelling_into_cancelled_and_running_into_failed`), `typed_failure_round_trips_and_retry_clears_it` / `precheck_report_is_cleared_on_start_and_requeue` (they assume `start_job` clears state; the log must now survive). All `RecordRequest { … }` literals (tests/record.rs `request()`, `jobs.rs` tests `request()`) need the new field.

### Pitfall 4: `FakeHost` default `run` answers empty stdout
**What goes wrong:** `Scenario::happy` has `run: Ok(CmdOutput{code: Some(0), …})` and no stdout (`fake.rs:124-128`); an unmatched `ffprobe` call therefore yields empty JSON => every take invalid.
**How to avoid:** `RecordRequest.verify: Option<Verify>`; `None` (legacy tests, CLI) skips verification entirely; new TST-02 tests set `Some` + `clips_dir` and script ffprobe answers. Extend the fake with (a) an **arg-substring matcher** — today `programs` match only on program name: `[VERIFIED: crates/host/src/fake.rs:465]` `.find(|(p, _, _)| name.contains(p.as_str()))` — e.g. `program_for("ffprobe", "take0002/video.mp4", &[json…], Ok(out))`; (b) an optional auto-responder that synthesizes valid JSON (duration from the planned take, size from `LaunchSpec.size`) when args contain `-show_entries`, so happy-path tests need no scripting; (c) "not final until t_end + lag" (record `take_end_at` virtual time when the End marker is delivered, fail ffprobe with `moov atom not found` until the lag passes).

### Pitfall 5: Two job rows share one `out_dir`
`out_dir = out_root/console_name(label_hero)` (`jobs.rs:274`): the same match+hero queued twice shares `clips/`, `record.json`, manifest. This is how the old `plan.hash` reuse worked and is acceptable, but `record.log.N` numbering must therefore come from a directory scan (Pattern 8) and `jobs.attempt` is per row, not per folder.

### Pitfall 6: Resolution of the recorded video may differ from `-w/-h`
`WinHost` launches `-windowed -w W -h H` (`win/mod.rs:271`); DPI scaling or a window larger than the desktop can change the client area (PITFALLS.md rates this MEDIUM). Strict resolution equality (D-03) could fail every take on some setups. Mitigation: log measured vs requested in the failure text, store measured size in the manifest, and let the author confirm on the first real run (Open Question 4).

### Pitfall 7: Stale `record.json` offers a wrong "Повторить"
If a fresh full run fails before `plan_player` (parse error), the old `record.json` stays and «Повторить» would resume the *old* plan. Delete `record.json` right before `plan_player` rewrites scripts (it also deletes old `cs2c_*`, `pipeline/src/lib.rs:259-263`).

### Pitfall 8: `ffmpeg` renames while a player holds `highlights.mp4`
`ui::open_path(&out)` opens the result in the default player after success (`jobs.rs:369`); the next attempt's rename over it can fail on Windows. Report the OS error text verbatim in Russian and keep `.part` for inspection on rename failure.

### Pitfall 9: Orphan kill ordering and blocking startup
Kill before cleanup (Pattern 10); the kill+wait (≤3 s each) runs on the UI thread at startup — acceptable (rare), but never loop unbounded; each `wait` is capped.

## Code Examples

### Judge (pure, unit-testable; Russian reasons)
```rust
// Source: derived from ffprobe 9.0.2 output shown above; thresholds = D-01
pub const TAKE_TOLERANCE_REL: f32 = 0.10;
pub const TAKE_TOLERANCE_MIN_SECS: f32 = 0.5;

pub fn within_tolerance(expected: f32, measured: f32) -> bool {
    measured.is_finite() && (measured - expected).abs() <= (expected * TAKE_TOLERANCE_REL).max(TAKE_TOLERANCE_MIN_SECS)
}
```

### Parsing ffprobe JSON defensively
```rust
#[derive(Deserialize)] struct Out { #[serde(default)] streams: Vec<S>, #[serde(default)] format: Option<F> }
#[derive(Deserialize)] struct S { codec_type: Option<String>, width: Option<u32>, height: Option<u32>, duration: Option<String> }
#[derive(Deserialize)] struct F { duration: Option<String> }
// duration: s.parse::<f64>() and reject !is_finite() or < 0; prefer format.duration, fall back to the stream's.
```

### Atomic manifest save
```rust
let tmp = clips.join("manifest.json.tmp");
std::fs::write(&tmp, serde_json::to_string_pretty(&m)?)?;
std::fs::rename(&tmp, clips.join("manifest.json"))?;   // clips/ is created with create_dir_all first
```

### Startup recovery pass (app.rs, order matters)
```rust
for (id, status, dir) in store.interrupted_jobs().unwrap_or_default() {
    let Some(dir) = dir else { continue };
    for n in launch::kill_orphans(&*host, &dir) { let _ = store.append_log(id, &format!("уборка после перезапуска: {n}")); }
    for n in launch::recover(&s.cs2_dir, &dir)  { let _ = store.append_log(id, &format!("уборка после перезапуска: {n}")); }
}
let interrupted = store.reset_running()?;   // running -> interrupted, cancelling -> cancelled
```

## State of the Art

| Old Approach | Current Approach | Phase | Impact |
|--------------|------------------|-------|--------|
| "Recorded" = `clips/plan.hash` equals + files exist (`jobs.rs:321-323`) | "Recorded" = manifest `validated` + sizes unchanged | 3 | A truncated take can no longer count as recorded |
| `record.log` append-only | rotate to `record.log.N` | 3 | Per-attempt evidence files |
| `start_job` wipes log/error | log persists, `attempt` increments | 3 | OBS-01/RET-01 |
| `reset_running` -> `failed` | -> `interrupted` | 3 | RET-04 |
| ffprobe only in CLI `renderer::assemble::video_secs` via `Command::output()` | `Host::run` with deadline+cancel | 3 | TMO-03 consistency |

**Deprecated/outdated:** `clips/plan.hash` and `Installed.done_takes` (superseded by the manifest; keep readers for old folders).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | HLAE ffmpeg finalizes video/WAV within ~20 s after `TAKE_END` (`take_finalize = 20 s`) | Pattern 3 | False `TAKE_INVALID` on slow disks/AV scans; tune in Phase 4 |
| A2 | Recorded `video.mp4` width/height equals the requested `-w/-h` | Pitfall 6 | Every take invalid if CS2 clamps the window; needs one real run |
| A3 | `video.mp4` duration == `TakePlan.game_secs` within ±10%/0.5 s (video written at capture fps) | Pattern 2 | Tolerance mismatch; D-01 says Phase 4 tunes it |
| A4 | HLAE on the author's machine (2.192.7) numbers takes as max+1 (source at tag v2.192.7 read; not run live) | Pattern 12 | Subset wave only; verify live once |
| A5 | `std::fs::rename` replaces an existing file on Windows | Pattern 4/9 | Manifest/`highlights.mp4` update fails on Windows; add remove+rename fallback and a test on the Windows CI host |
| A6 | Text surgery on the final `addAtTick` line of `cs2c_NN.cfg` reproduces what `render` would emit | Pattern 12 | Guarded by the byte-identical round-trip test |
| A7 | An orphan `cs2.exe` is rare because the Job closes with the app (so D-15 is a safety net) | Pattern 10 | None for correctness; only affects priority/effort |
| A8 | `ffprobe` is the same binary precheck verified (`"ffprobe"` on PATH of the GUI process) | Pattern 1 | Precheck already fails the job if it does not answer |

## Open Questions

1. **Subset re-record (D-07) in this phase or not?**
   - Known: feasible (Pattern 12) but it is DIF-05 (v2) and adds a renderer+install+mapping chain; HLAE numbering gaps rule out the naive approach.
   - Unclear: whether the author wants it before Phase 4 acceptance.
   - Recommendation: plan the baseline (all-or-nothing + "all validated => assemble") as Waves 1-3; plan Pattern 12 as an isolated Wave 4 that can be dropped.
2. **Does "Заново" still reuse takes with an equal `plan_hash` (today's behaviour)?**
   - Recommendation: yes — same reconcile code path, only the entry point differs; document it on the button tooltip. If the author wants a truly clean re-record, add a checkbox later.
3. **Skip precheck when everything is already validated (RET-03)?** Recommendation: skip it (a foreign `cs2.exe` or low space must not block a pure assembly); `ffmpeg` absence is reported by `run_ffmpeg`'s existing Russian error.
4. **Strict resolution equality?** Recommendation: strict per D-03, but include measured/requested numbers in the message and confirm on the first real run; fall back to "equal aspect and ≥ requested" only if A2 proves false.
5. **Should `Interrupted` + queued jobs pause the queue only when other jobs are queued?** The banner already shows only when `waiting > 0` (`jobs_page.rs:50`); keep `paused = true` unconditionally after any interrupted row.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo/rustc | build + tests | ✓ | cargo 1.99.0 / rustc 1.99.0 | — |
| ffprobe / ffmpeg | real-ffprobe unit tests (optional), assembly | ✓ | 9.0.2 (gyan.dev, WinGet) | tests use `FakeHost`; real-ffmpeg tests self-skip like `has_ffmpeg()` in `assemble.rs` |
| HLAE + CS2 | live checks (take numbering, finalize lag, resolution) | ✗ in the research sandbox (✓ on the author's PC per README: HLAE 2.192.7) | — | Not needed for CI; list as manual verification items |
| `gh` CLI | — | ✗ | — | not needed |

**Missing dependencies with no fallback:** none for planning/CI. Live HLAE behaviours (A1, A2, A4) need one manual session by the author.

## Validation Architecture

(Nyquist is disabled for this project; this is a compact requirement→test map because the planner needs one.)

| Property | Value |
|----------|-------|
| Framework | built-in `cargo test` (workspace) |
| Quick run | `cargo test -p pipeline --test record` (35 tests, 0.03 s `[VERIFIED]`) |
| Full suite | `cargo test --workspace` (CI runs `cargo test` + `cargo build`; CLI test `crates/cli/tests/pipeline.rs` must stay green — it calls `renderer::render(&m, &timeline, &renderer::Config::default())`, so keep that signature) |

| Req | Behaviour | Test (new unless noted) |
|-----|-----------|-------------------------|
| TAK-04 | valid take passes; no audio stream, duration out of tolerance, wrong size, ffprobe missing/timeout each give a specific reason | `pipeline` unit tests on `judge`/`probe_file` with canned JSON |
| TAK-02/05 | probe never runs before `TAKE_END`; finalize lag (`moov atom not found` until t+lag) is waited out; stable-but-invalid fails fast | `tests/record.rs` with fake `finalize_lag` |
| D-17 "дубль обрезан" | `TAKE_INVALID`, `state=Take`, message names the take, manifest has `invalid`, others `validated` | `tests/record.rs` (new) |
| TAK-03 | manifest incremental writes, atomic, reconcile marks changed-size take for re-probe | `manifest.rs` unit tests (tempdir) |
| TAK-06 | `.part` renamed only on success; failure deletes only `.part`; previous `highlights.mp4` survives | `app::jobs` tests with `FakeHost::run_result` + tempdir |
| RET-01/OBS-01 | attempt increments, log survives `start_job`/`requeue`, prefix only on DB line, stage-duration lines round-trip | `store` + `app::jobs` tests |
| RET-02/03 | resume with `record.json` skips analyze; all validated => no `launch` (`fake.launches()==0`) | `app::jobs` tests |
| RET-04 | `reset_running` -> `Interrupted` keeps stage/phase; orphan with matching PID+created is killed, mismatch is not; kill precedes cleanup | `store` tests; `launch` tests on `FakeHost` |
| RET-06 | `record.log` -> `record.log.1`, `clips_old_<ts>` keeps invalid/partial takes | `tests/record.rs`, `app::jobs` |
| OBS-02/03 | card actions (`Act::Retry/Redo`), env snapshot content | pure functions factored out of `jobs_page`/`jobs.rs` |
| Migration | v3 DB gains `attempt`,`resume` | `store` test like `version_two_database_gains_…` |

### TST-02 gap analysis (what exists vs missing, from `crates/pipeline/tests/record.rs`)

| D-17 scenario | Existing test | Gap |
|---------------|---------------|-----|
| лаунчер не стартовал | `launcher_exit_code_is_reported_as_launcher_failed`, `launcher_that_never_exits_fails_after_the_launcher_deadline` | **Missing: `Scenario::launch_error(HostError::Spawn(..))` through `record()`** — `launch_error` is not used by any test outside `fake.rs` (grep); add one asserting `LauncherFailed` + `record.log` written |
| игра не появилась | `game_that_never_appears_fails_with_game_not_started` | — |
| окно ошибки HLAE | `hlae_error_dialog_fails_at_once_with_its_text`, `first_afxerror_fails_at_once_with_hlae_error` | — |
| консоль не открылась | `console_that_never_opens_fails_with_console_not_open` | — |
| демка не загрузилась | `missing_ready_marker_fails_with_demo_not_loaded_after_the_deadline` | — |
| скрипт не запустился | `missing_script_marker_fails_with_script_not_run_and_exec_is_sent_once` | — |
| маркер дубля не пришёл | `missing_first_take_start_fails_with_take_timeout`, `missing_take_end_fails_with_take_timeout_naming_the_take` | — |
| дубль обрезан | none | **Missing** (needs ffprobe fake) |
| игра умерла посреди дубля | `game_exit_mid_take_fails_with_game_exited`, `unfinished_take_is_renamed_partial_and_finished_ones_stay` | — |
| отмена в каждом состоянии | `cancel_during_demo_load_…` (Demo), `cancel_during_a_take_…` (Take), force variants, precheck cancels in `tests/precheck.rs:187,273` | **Missing: Launcher, Game, Console, Script, Closing, and "during ffprobe"**. `Capture::cancelling_after(state)` already exists (`tests/record.rs:64`); one table-driven test over `RecordState::{Launcher,Game,Console,Script,Closing}` plus a probe-time cancel |
| "каждый заканчивается своим кодом" | per-test asserts | **Add one table test mapping scenario -> `FailCode` and asserting the set of codes is distinct per failure class** (D-17 wording) |

## Security Domain

`security_enforcement` is enabled (ASVS level 1, block on high).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2/V3 Authentication/Session | no | local single-user desktop app |
| V4 Access Control | yes (process kill) | kill only on matching PID **and** creation time **and** image allow-list; one handle |
| V5 Input Validation | yes | `serde` typed parse of ffprobe/manifest/record/processes JSON; cap stdout size; validate dir names `^take\d{4}$`; finite/non-negative numbers |
| V6 Cryptography | no | `plan_hash` is a change fingerprint (FNV), not a security primitive — do not present it as integrity protection |
| V7 Error Handling & Logging | yes | ffprobe stderr/JSON fields go through `log::one_line` (control chars, profile path, secrets redacted) before reaching `record.log`/DB |
| V12 Files & Resources | yes | job-folder paths built from validated components only; rename/delete limited to children of `clips/`, `clips_old_*`, the `.part` file; reuse `is_ours` rules (`launch.rs:152-167`) for anything under the game folder |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Tampered `processes.json` makes the app kill an arbitrary process | Elevation/Tampering | image allow-list + creation-time match + only `OpenProcess`-able own-user processes |
| Path traversal via `dir`/`demo_path` in `manifest.json`/`record.json` | Tampering | validate `take\d{4}`; join only under `out_dir`; `demo_path` goes through precheck (exists, not LFS pointer) |
| Hostile media file crafted to abuse ffprobe output parsing | Tampering/DoS | deadline from `Deadlines.ffprobe`, stdout cap, typed parse, no shell, args as argv vector |
| Console text (netcon is untrusted) copied into logs | Info disclosure | already redacted by `log::redact`; apply the same to any new fact lines |
| Race: PID reused between check and kill | Tampering | single handle: open -> verify -> terminate on the same handle |

## Project Constraints (from CLAUDE.md)

- Rust workspace, `std::thread` (no tokio), SQLite via `rusqlite`; embed in existing crates; **no stack changes**.
- Windows-only recording; `cfg(windows)` code with stubs so Linux CI builds (`UnsupportedHost`).
- External tools (HLAE/CS2/ffmpeg) need timeouts and diagnostics; one recording job at a time.
- `cs2-cli` must keep building and `crates/cli/tests/pipeline.rs` must pass (do not change `renderer::render` signature or remove `plan.json`/`highlights.cfg`/`assemble.sh` from its output; do not touch `renderer::assemble`).
- User-facing strings and doc comments in **Russian**; identifiers in English. Errors: `Result<T, String>` at crate boundaries; cancellation via `pipeline::CANCELLED` + `check(p)?`.
- New long-running steps report through `Progress::stage/log/event`.
- Tests named as sentences; helpers small private fns; run `cargo clippy --workspace` locally (CI only runs test+build).
- Formatting: `pipeline`/`host` follow default rustfmt (as the files are laid out); `app`/`store` use a wide style — per STATE.md decision 01-07 format app files only with rustfmt `max_width=160,use_small_heuristics=Max`, not `cargo fmt -p app`.
- GSD workflow: file changes go through a GSD command (this research is read-only apart from this document).

## Suggested Plan Decomposition (for the planner)

1. **Store + status (W1):** migration 4 (`attempt`, `resume`), `Interrupted`, `start_job` keeps log/returns attempt, `requeue_resume`, `reset_running` -> interrupted; update 6+ test assertions. Covers RET-01 (part), RET-04 (part).
2. **Host process identity (W1, parallel):** `ProcIdentity`, `Session::processes`, `Host::kill_orphan`, Windows impl, fake support. Covers RET-04 (part).
3. **Pipeline verify + manifest (W1, parallel):** `manifest.rs`, `verify.rs`, `FailCode::TakeInvalid` (+Russian messages/hint), `Deadlines.take_finalize`, `record.log` rotation, `Report` env facts. Covers TAK-03/04, RET-06 (part), OBS-03 (part).
4. **Record driver integration + fake + TST-02 (W2):** `Action::Verify`/`Input::TakeInvalid`, pending probes, manifest/processes.json writes, fake ffprobe/lag/auto-responder, all TST-02 tests incl. launch-error and cancel-in-every-state. Covers TAK-02/04/05, TST-02.
5. **App job flow (W3):** `record.json`, resume branch, reconcile + clips_old moves, `write_demo_copy`, manifest gate + `.part` assembly (renderer `ffmpeg_args_with`), `JobReporter` prefix/durations/attempt, env snapshot file. Covers RET-02/03, TAK-05/06, OBS-01/03.
6. **App UI + startup (W3):** startup order (orphan kill -> recover -> reset), paused-after-interrupt, `jobs_page` badges/buttons/"Попытка N"/stage summary/old logs, cached lookups. Covers RET-04, OBS-02, D-09/D-13/D-16.
7. **Optional W4 — subset re-record:** Pattern 12 (renderer `rechain`, install overrides, dir mapping, fake indices, live numbering check).
8. **Manual verification (human, end-of-phase per config `human_verify_mode`):** real HLAE session: first-take validation passes on a real take (A1/A2/A3), kill the app mid-record and restart (orphan + Interrupted), «Повторить» after deleting one take dir.

## Sources

### Primary (HIGH confidence)
- Repository files read this session (cited inline with path:lines): `crates/pipeline/src/record/{mod,machine,deadlines,failure,log}.rs`, `crates/pipeline/src/{lib.rs,precheck/{mod,gather}.rs}`, `crates/pipeline/tests/record.rs`, `crates/host/src/{lib,fake,runner,unsupported}.rs`, `crates/host/src/win/{mod,process,watch,job}.rs`, `crates/store/src/lib.rs`, `crates/app/src/{jobs,jobs_page,app,launch,status}.rs`, `crates/renderer/src/{lib,assemble}.rs`, `crates/cli/{src/main.rs,tests/pipeline.rs}`.
- ffprobe 9.0.2 and ffmpeg 9.0.2 run locally (JSON shape, exit codes, truncated mp4/WAV behaviour).
- `cargo test -p pipeline --test record`: 35 passed, 0.03 s.

### Secondary (MEDIUM confidence)
- https://raw.githubusercontent.com/advancedfx/advancedfx/v2.192.7/shared/FileTools.cpp — `SuggestTakePath` (`iMax = -1 … iMax++`, pattern `take????`).
- https://raw.githubusercontent.com/advancedfx/advancedfx/v2.192.7/AfxHookSource2/RenderSystemDX11Hooks.cpp — `RecordStart` calls `SuggestTakePath` (line 5235); `EndCapture` queues shutdown on render-thread queues (line 3297). HLAE latest release is v2.192.7, published 2026-10-08 (GitHub API), matching the README's tested version.
- https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information — `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
- `.planning/research/PITFALLS.md` (Pitfalls 11, resolution), `.planning/research/ARCHITECTURE.md` (probe retry window, manifest) — project research, consistent with the above.

### Tertiary (LOW confidence)
- Tolerance and finalize-window numbers (A1-A3) — design defaults to be tuned in Phase 4.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new dependencies; all APIs read in-repo.
- Architecture: HIGH for store/app/pipeline seams; MEDIUM for HLAE async finalize and take numbering (source-level, not run live).
- Pitfalls: HIGH for those reproduced locally (ffprobe) or visible in code; MEDIUM for resolution/finalize timing.

**Research date:** 2026-10-10
**Valid until:** 2026-11-09 (HLAE releases frequently; re-check `SuggestTakePath`/`EndCapture` if HLAE is upgraded)
