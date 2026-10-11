---
phase: 06-ranner-zaschita-vhoda-i-pervyy-zhivoy-progon
plan: 02
subsystem: security
tags: [rust, sanitization, cfg-injection, ffmpeg, drawtext, faststart, ffprobe]
status: complete

requires:
  - phase: 04 (запись и склейка)
    provides: pipeline::assemble (part_args, run_ffmpeg), Host::run с дедлайном и отменой, record::verify как образец разбора ffprobe
provides:
  - cs2::text::{NAME_MAX, clean_text, clean_map} и очистка имён игроков и карты в Match::build (SEC-01)
  - cfg CS2 без текста из демки (нет карты и заголовков шотов); drawtext читает подписи из cap_NNN.txt (textfile=, expansion=none) (SEC-01)
  - Encoder::args (pub) с -movflags +faststart в обоих кодерах склейки (RUN-06)
  - pipeline::assemble::{is_faststart, result_probe_args, ResultFacts, judge_result, verify_result} (RUN-06)
affects: [06-10 (раннер зовёт verify_result перед загрузкой и приклеивает код RESULT_INVALID), 06-07 (inspect видит уже очищенные имена)]

estimate:
  tokens: 85000
  raw_tokens: 85000
  tasks: 2
  confidence: low
actuals:
  tokens: 10500
  tasks: 2
  commits: 2
plan_head_before: 5fb8a95873ef59382f9b51e409f329a8f8fa9d40
plan_head_after: cc5ae504b663cd363e6b16bb3f673947efbde6d8

tech-stack:
  added: []
  patterns:
    - "Недоверенный текст из демки очищается один раз на границе доверия (Match::build); в cfg консоли он не пишется вовсе"
    - "Подписи ffmpeg лежат в файлах cap_NNN.txt рядом с assemble.ffgraph; в графе только textfile='cap_NNN.txt' и expansion=none"
    - "Проверка результата возвращает Result<ResultFacts, String> (причину), код сбоя добавляет вызывающий"

key-files:
  created:
    - crates/cs2/src/text.rs
  modified:
    - crates/cs2/src/lib.rs
    - crates/renderer/src/lib.rs
    - crates/pipeline/src/assemble.rs
    - crates/pipeline/src/lib.rs

key-decisions:
  - "Очистка: вырезать is_control и невидимые/bidi символы, любой пробельный символ становится одним пробелом, длина до 64 символов; карта только [A-Za-z0-9_-], пусто даёт unknown"
  - "Пустой ник после очистки становится «игрок {userid}»"
  - "Текст из демки не попадает в cfg вообще (а не экранируется): из комментариев убраны карта и заголовок шота"
  - "verify_result возвращает причину строкой; негодный ролик не удаляется, остаётся уликой"

requirements-completed: [SEC-01, RUN-06]

coverage:
  - id: D1
    description: "Ник и карта из демки не меняют грамматику highlights.cfg и cs2c_NN.cfg"
    requirement: "SEC-01"
    verification:
      - kind: unit
        ref: "crates/renderer/src/lib.rs#hostile_text_never_reaches_a_cfg_line, match_build_cleans_names_and_map_before_render"
        status: pass
  - id: D2
    description: "Очистка имён и карты на границе доверия"
    requirement: "SEC-01"
    verification:
      - kind: unit
        ref: "crates/cs2/src/text.rs#clean_text_strips_control_bidi_and_invisible_chars, clean_map_keeps_only_safe_chars; crates/cs2/src/lib.rs#empty_player_name_becomes_player_with_userid"
        status: pass
  - id: D3
    description: "Подписи drawtext читаются из файлов cap_NNN.txt, текста подписи в графе нет"
    requirement: "SEC-01"
    verification:
      - kind: unit
        ref: "crates/renderer/src/lib.rs#audio_is_stretched_per_piece_and_captions_use_font"
        status: pass
  - id: D4
    description: "highlights.mp4 пишется с +faststart в обоих кодерах"
    requirement: "RUN-06"
    verification:
      - kind: unit
        ref: "crates/renderer/src/lib.rs#encoder_args_put_faststart_before_the_output"
        status: pass
  - id: D5
    description: "verify_result принимает только H.264 + yuv420p + звук + mp4 + длительность > 0 + moov раньше mdat"
    requirement: "RUN-06"
    verification:
      - kind: unit
        ref: "crates/pipeline/src/assemble.rs#faststart_is_detected_from_box_order, judge_result_accepts_only_h264_yuv420p_with_audio_in_mp4, verify_result_runs_ffprobe_through_the_host"
        status: pass
      - kind: integration
        ref: "crates/pipeline/src/assemble.rs#real_ffmpeg_output_is_faststart_and_verified (настоящий ffmpeg 9.0.2)"
        status: pass
---

# Phase 06 Plan 02: Очистка текста из демки и проверка готового ролика Summary

Враждебный ник и карта из чужой демки не могут стать командой консоли CS2 или сломать граф ffmpeg, а ролик проверяется на пригодность для Telegram до загрузки.

## Accomplishments

- `cs2::text` (`clean_text`, `clean_map`, `NAME_MAX = 64`) вызывается из `Match::build`: имена игроков очищаются, карта сводится к `[A-Za-z0-9_-]` и не уводит путь `maps/{карта}.mesh`; пустой ник становится `игрок {userid}`.
- `renderer::render` не пишет текст из демки в cfg: из первой строки `highlights.cfg` убрана карта, из комментария шота в `cs2c_NN.cfg` заголовок. Тест на таблице враждебных строк проверяет, что каждая строка каждого cfg начинается с известного префикса.
- Подписи `drawtext` лежат в `cap_NNN.txt` (однострочный текст), в графе только `textfile='cap_NNN.txt'` и `expansion=none`; функция экранирования `text` удалена. Файлы подписей идут в `Output.files` вместе с `assemble.ffgraph`.
- `Encoder::args` стал `pub` и добавляет `-movflags +faststart` перед `highlights.mp4` в обоих кодерах; `assemble.sh` и `assemble.bat` получили флаг автоматически.
- `pipeline::assemble::verify_result` проверяет размер, порядок боксов MP4 (чтение заголовков через `File` + `Seek`) и ответ ffprobe через `Host::run` с дедлайном и отменой; `judge_result` — чистая функция с русскими причинами отказа. Тест с настоящим ffmpeg подтверждает: ролик с `+faststart` проходит, без него отклоняется.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Тест выбора игрока из 06-01 ожидал двойной пробел в имени**
- **Found during:** Task 2 (полный прогон `cargo test --workspace`)
- **Issue:** `resolve_player_ignores_case_and_spaces_after_the_exact_tier` сравнивал имя `Big  Boss` с двойным пробелом, а `Match::build` теперь схлопывает пробелы (так требует план).
- **Fix:** ожидаемое имя в тесте стало `Big Boss`; запрос с двойным пробелом по-прежнему находит игрока на ярусе «без регистра и пробелов».
- **Files modified:** `crates/pipeline/src/lib.rs`
- **Commit:** cc5ae50

**2. [Порядок работ] `+faststart` в `Encoder::args` и его тест попали в коммит Task 1**
- Правка `Encoder::args` лежала в том же файле `renderer/src/lib.rs`, что и правки Task 1, поэтому вошла в коммит e9fb03f; Task 2 содержит только `assemble.rs` и правку теста 06-01.

Больше отклонений нет.

## Known Stubs

Нет.

## Threat Flags

Нет: новых сетевых точек, путей авторизации и схем на границах доверия не появилось (ffprobe запускается через `Host::run`).

## Commits

- e9fb03f: feat(06-02): текст из демки очищается в Match::build и не попадает в cfg, подписи ffmpeg читаются из файлов
- cc5ae50: feat(06-02): verify_result проверяет готовый ролик (H.264, yuv420p, звук, mp4, faststart) до загрузки

## Verification

`cargo test --release --workspace` зелёный (включая `cli --test pipeline` и `--test assemble`, `real_ffmpeg_output_is_faststart_and_verified` выполнен на реальном ffmpeg).

## Self-Check: PASSED

- FOUND: crates/cs2/src/text.rs
- FOUND: e9fb03f, cc5ae50 (предки HEAD)
- `grep -c "Карта: {}" crates/renderer/src/lib.rs` = 0
