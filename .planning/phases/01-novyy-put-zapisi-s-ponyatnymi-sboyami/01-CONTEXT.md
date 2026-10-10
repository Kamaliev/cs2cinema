# Phase 1: Новый путь записи с понятными сбоями - Context

**Gathered:** 2026-10-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Задача с записью в приложении идёт через новый `pipeline::record`: явная машина состояний поверх трейта `Host`, каждое ожидание ограничено именованным дедлайном, любой сбой старта игры останавливает задачу в срок с типизированной ошибкой, которую видно в карточке задачи и в `record.log`. Покрывает TST-01, LCH-01, LCH-02, LCH-03, LCH-04, LCH-06, TMO-01, TMO-02, TAK-01.

Не входит в фазу: предпроверка окружения (фаза 2), поэтапное завершение игры и отмена за секунды (фаза 2), `ffprobe` дублей и манифест (фаза 3), кнопка «Повторить» и восстановление после падения (фаза 3), перевод `cs2-cli` на общий код и удаление `launch.rs` (фаза 4). Фаза начинается со спайков на машине автора (см. Claude's Discretion).

</domain>

<decisions>
## Implementation Decisions

### Граница трейта Host
- **D-01:** Трейт `Host` и его реализации живут в новом листовом крейте `crates/host` (без зависимостей на `pipeline`): трейты, `WinHost` под `cfg(windows)`, фейк в памяти, заглушка для не-Windows (CI). `pipeline` зависит от `host`; `app` и `cli` получают его через `pipeline`. `windows-sys` подключается только в `crates/host`. — **Reversibility:** costly — переезд в модуль `pipeline` потом тронет все импорты и тесты, но без миграций.
- **D-02:** Свой `cs2.exe` находится через Windows Job Object уже в фазе 1: `WinHost` создаёт job с `KILL_ON_JOB_CLOSE` до запуска `HLAE.exe`, список PID и дескриптор `cs2.exe` берёт из job; живость — ожидание на handle (TMO-02). Запасной путь, если спайк покажет, что `cs2.exe` не наследует job через лоадер HLAE: снимок процессов после старта → `OpenProcess` → `AssignProcessToJobObject`. Фаза 2 добавляет только поэтапное завершение и отмену. — **Reversibility:** costly — отказ от Job Object потребует другой модели владения процессом во всех состояниях машины.
- **D-03:** Трейт прячет процессы (запуск сессии игры, события: строки консоли, выход лаунчера, смерть игры; отправка команд; завершение; запуск утилит с дедлайном), консоль и часы (`now()`). Файловая система не абстрагируется: машина работает с реальными путями, фейк в тестах пишет во временную папку.
- **D-04:** Основной канал консоли — netcon (`-netconport`, плюс `-afxFixNetCon` после спайка). Запасной — хвост `console.log` (`-condebug`), чтобы маркеры и `AFXERROR` приходили даже при неоткрывшемся порте. Оба потока сливаются в одну шину событий с дедупликацией маркеров. Отправка команд — только через netcon; если порт не открылся в срок — `CONSOLE_NOT_OPEN`.

### Ошибка в карточке и БД
- **D-05:** `pipeline::record` возвращает типизированную ошибку `RecordFailure { code, state, message, hint, elapsed, console_tail }`. В `store` добавляются колонки `fail_code`, `fail_state`, `fail_hint`, `fail_tail` (миграция схемы); `error` остаётся человеческим сообщением по-русски. `store` не зависит от `pipeline`: в БД идут строки. Карточка и будущий автоповтор (DIF-03/04) читают код без разбора строк. — **Reversibility:** one-way — колонки в SQLite на машине пользователя; откат потребует миграции вниз.
- **D-06:** Карточка упавшей задачи показывает блок ошибки: код и состояние мелким шрифтом, сообщение, подсказку, прошедшее время, сворачиваемый хвост консоли. Плюс лестница состояний «лаунчер → игра → консоль → демка → скрипт → дубль N/M → закрытие» с отметкой пройденных и упавшего шага; лестница видна и пока задача выполняется.
- **D-07:** Хвост консоли в ошибке — около 30 строк консоли игры и stdout/stderr HLAE без шума: подряд идущие повторы сворачиваются в «×N» (например, `Failed writing image`), строки с `AFXERROR` и маркерами сохраняются всегда. Полная консоль всегда в `record.log`.
- **D-08:** Коды ошибок — `SCREAMING_SNAKE` по-английски, текст рядом по-русски. Базовый набор фазы 1: `LAUNCHER_FAILED`, `GAME_NOT_STARTED`, `CONSOLE_NOT_OPEN`, `DEMO_NOT_LOADED`, `SCRIPT_NOT_RUN`, `TAKE_TIMEOUT`, `GAME_EXITED`, `HLAE_ERROR`, `CANCELLED`. Расширяется в фазах 2–3.

### Дедлайны и маркеры
- **D-09:** Дедлайны — структура `Deadlines` в `pipeline::record` с именованными полями (лаунчер, появление игры, консоль, загрузка демки, скрипт, первый дубль, каждый дубль, закрытие игры, ffprobe, ffmpeg) и константами по умолчанию из research (консоль ~90 с, простой дубля ~60 с как отправные точки). В настройках `load_wait_secs` заменяется одним множителем «запас по времени» (1.0 по умолчанию), который умножает всю таблицу. Тесты передают свою таблицу. Значения подстраиваются в фазе 4.
- **D-10:** `exec highlights` отправляется ровно один раз после маркера `READY`. Повторной отправки по таймеру нет: если `SCRIPT` не пришёл в срок — `SCRIPT_NOT_RUN`, если `TAKE_START 0` не пришёл — `TAKE_TIMEOUT`. Повтор задачи целиком — дело фаз 3–4.
- **D-11:** Маркеры консоли переходят на формат с nonce задачи: `CS2C <nonce> READY|SCRIPT|TAKE_START n|TAKE_END n|ALL_DONE`. Рендерер получает nonce в `Config` и пишет маркеры в `highlights.cfg` и `cs2c_NN.cfg`; парсер игнорирует эхо команд и чужой nonce (старый `console.log`, прошлая задача). Тесты рендерера обновляются; `CS2C_TAKE_DONE` уходит. — **Reversibility:** costly — формат маркеров зашит в генерируемые cfg и в парсер; старые папки с cfg станут несовместимы с новым парсером.
- **D-12:** Дедлайн одного дубля (от `TAKE_START n` до `TAKE_END n`) выводится из плана: игровая длина шота × коэффициент записи (от `capture_fps`, подстраивается в фазе 4) + фиксированный запас на перемотку `demo_gototick` и закрытие ffmpeg. Считается вместе с ожидаемым набором дублей из `plan.json` до начала записи (TAK-01).

### Claude's Discretion
- **Спайки.** Пользователь не стал обсуждать. Решение: четыре спайка из роадмапа (Job Object через лоадер HLAE, `-afxFixNetCon`, текст консоли при реальных сбоях `playdemo`/cfg, абсолютный путь в `+playdemo` через `-cmdLine`) оформляются как пример-бинарник в `crates/host/examples/` (или один бинарник с подкомандами). Запускает их пользователь на своей машине при запущенном Steam; результат — заметка `.planning/phases/01-novyy-put-zapisi-s-ponyatnymi-sboyami/01-SPIKES.md` с выводом по каждому спайку и выбранным путём (основной или запасной). Планировщик ставит спайки первым планом и делает `WinHost` зависимым от их результата; при любом исходе у каждого спайка есть запасной путь из research, так что фаза не блокируется.
- **Как лестница состояний доходит до UI.** Рекомендация из research: `Progress` получает типизированный метод `event(&self, e: Event)` с реализацией по умолчанию, `store.set_stage` хранит имя состояния строкой; UI не парсит строки журнала.
- **Шина событий.** `crossbeam-channel` (research) или `std::mpsc` с `recv_timeout` — на усмотрение планировщика; требование одно: каждое ожидание в цикле машины — срез ≤100 мс с проверкой `Progress::cancelled()`.
- **Закрытие игры в фазе 1.** Достаточно `quit` по консоли и ожидания выхода по handle с дедлайном; при неответе — закрыть job. Полная поэтапная уборка — фаза 2.
- **Формат `record.log`.** Строки с меткой времени и источником (`netcon`, `console.log`, `hlae-stdout`, `hlae-stderr`, `app`); пишется инкрементально, сбрасывается при любом исходе.
- **Копии демки в папке CS2.** Поведение `install_files` не меняется в фазе 1 (уборка — фаза 2, TMO-06).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Исследование этапа
- `.planning/research/SUMMARY.md` — сводка: рекомендованный стек (`windows-sys`, Job Object, шина событий, `thiserror`), архитектура `crates/host` + `pipeline::record`, список спайков с запасными путями, отправные значения дедлайнов
- `.planning/research/ARCHITECTURE.md` — компоненты `host`/`record`/`renderer`/`store`/`app`, чистая машина `step(now, Input) -> Vec<Action>`, цикл-драйвер
- `.planning/research/PITFALLS.md` — антипаттерны текущего `launch.rs`: fail-open живость, `taskkill /IM`, mtime-поиск дублей, повтор `exec highlights`, блокирующий `Command::output()`, `AFXERROR`-шторм
- `.planning/research/STACK.md` — версии крейтов и флаги запуска HLAE/CS2 (`-insecure -noConfig -afxFixNetCon -afxDisableSteamStorage -condebug`)
- `.planning/research/FEATURES.md` — таксономия ошибок и категории PRE/LCH/TMO/TAK/RET/OBS/TST/ACC

### Требования и роадмап
- `.planning/REQUIREMENTS.md` — TST-01, LCH-01..04, LCH-06, TMO-01, TMO-02, TAK-01 (фаза 1); остальные — для понимания, куда растёт машина
- `.planning/ROADMAP.md` §Phase 1 — цель, критерии успеха, заметка о спайках
- `.planning/PROJECT.md` — ограничения стека и языка, ключевые решения этапа

### Карта кодовой базы
- `.planning/codebase/CONCERNS.md` — известные баги записи (`game_alive`, `kill_game`, `wait_for_clips`), дублирование ffmpeg
- `.planning/codebase/ARCHITECTURE.md` — слои и поток данных, место `pipeline` и `store`
- `.planning/codebase/TESTING.md` — как устроены тесты и моки в `crates/cli/tests/pipeline.rs`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/app/src/launch.rs`: `launch()` (строка запуска HLAE с `-cmdLine`), `Console` (netcon: TCP + поток-читатель с `read_until`), `free_port()`, `install_files()`, `hlae_ffmpeg()`, `no_window()` — переносятся в `crates/host::WinHost` как основа, а не пишутся заново. `game_alive`/`kill_game`/`wait_for_clips` не переносятся (антипаттерны).
- `crates/app/src/jobs.rs`: `JobReporter` (реализация `Progress` поверх `store.append_log`/`set_stage`), последовательность `run()` → `record()` → `run_ffmpeg()`; `record()` заменяется вызовом `pipeline::record`, `run()` остаётся тонким.
- `crates/renderer/src/lib.rs`: генерация `highlights.cfg`/`cs2c_NN.cfg` с `echo` маркерами (`TAKE_DONE`, строки ~196–242) и `console_safe()`; сюда добавляется nonce и новые маркеры.
- `crates/pipeline/src/lib.rs`: `Progress`, `Stage`, `check()`, `CANCELLED`, `Result<T, String>` — расширяются, не ломаются: `record` возвращает `RecordFailure`, который на границе `run()` превращается в строку плюс колонки store.
- `crates/store/src/lib.rs`: `Store` на `Arc<Mutex<Connection>>`, `start_job`/`set_stage`/`append_log`/`finish_job`; схема создаётся в `open()` — сюда миграция с колонками `fail_*`.
- `crates/app/src/jobs_page.rs`: `job_card()` с ошибкой, кнопкой «Повторить» и сворачиваемым журналом — расширяется блоком ошибки и лестницей.

### Established Patterns
- Ошибки — `Result<T, String>`, отмена — сентинел `pipeline::CANCELLED`; GUI сравнивает строку. Типизированная ошибка должна сохранить этот контракт на границе (`CANCELLED` код ↔ сентинел).
- Длинная работа — `std::thread::spawn`, без tokio; одна задача записи за раз (`jobs::run_next`).
- Пользовательские строки по-русски, идентификаторы по-английски; тесты именуются предложениями.
- Windows-код под `#[cfg(windows)]` с заглушкой для CI (как в `game_alive`).
- Настройки — `Settings` в `settings.rs` с `DragValue` на странице настроек; `load_wait_secs` там уже есть и подлежит замене множителем.

### Integration Points
- `jobs::run()` после `plan_player` — точка вызова `pipeline::record(host, plan, deadlines, progress)`; после него — существующий `run_ffmpeg` (сборка в `pipeline` переезжает в фазе 4).
- `JobReporter` — получает `event()` для состояний; `store.finish_job` — принимает `RecordFailure` в виде строк.
- Workspace `Cargo.toml` — новый член `crates/host`; `crates/pipeline/Cargo.toml` — зависимость `host`.
- `crates/cli` остаётся на старом пути до фазы 4, но должен собираться: изменения в `renderer` (nonce в `Config`) должны иметь значение по умолчанию или минимальную правку CLI.

</code_context>

<specifics>
## Specific Ideas

- Лестница состояний в карточке: одна строка с отметками пройденных шагов и упавшего, видна и во время выполнения, не только после сбоя.
- Хвост консоли сворачивает повторы в «×N» — важно из-за `Failed writing image`-шторма HLAE (сотни строк).
- Множитель «запас по времени» в настройках вместо отдельных таймаутов: один регулятор для медленной машины.

</specifics>

<deferred>
## Deferred Ideas

- Поэтапное завершение игры и отмена за секунды в любом состоянии — фаза 2 (TMO-03, TMO-04).
- Уборка копий демки и cfg в папке CS2 — фаза 2 (TMO-06).
- Модальное окно HLAE и `AFXERROR` как мгновенный сбой — фаза 2 (LCH-05); в фазе 1 `AFXERROR` лишь сохраняется в хвосте.
- Кнопка «Открыть папку» и полный блок повторов — фаза 3 (OBS-02, RET-*).
- Таймауты по прогрессу («нет новых строк N секунд») — v2 (DIF-06).

</deferred>

---

*Phase: 01-novyy-put-zapisi-s-ponyatnymi-sboyami*
*Context gathered: 2026-10-10*
