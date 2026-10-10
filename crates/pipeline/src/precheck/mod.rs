//! Предпроверка окружения перед записью: сбор фактов (хост, файлы, утилиты — `gather`, план 02-05)
//! и чистая оценка [`evaluate`], которая превращает факты в отчёт из пунктов `ok`/`warn`/`fail`.
//!
//! Весь русский текст пунктов живёт здесь, чтобы задача из очереди и кнопка «Проверить окружение»
//! показывали один и тот же отчёт (D-01). Старт блокирует только `fail`, `warn` старт не
//! блокирует (D-02). Каждая ветка оценки — чистая функция от фактов, поэтому тестируется без ОС.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::record::failure::{FailCode, RecordFailure};
use crate::record::machine::RecordState;

mod gather;
pub mod space;
pub mod steam;

pub use gather::{Demo, Inputs, check_space, hlae_ffmpeg, run};

/// Уровень пункта предпроверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

/// Один пункт отчёта: что проверяли, чем кончилось и что делать.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Идентификатор из [`ids`].
    pub id: String,
    pub level: Level,
    pub message: String,
    /// Подсказка по-русски; у `Ok` пустая.
    pub hint: String,
}

impl Item {
    fn ok(id: &str, message: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Ok,
            message: message.into(),
            hint: String::new(),
        }
    }

    fn warn(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Warn,
            message: message.into(),
            hint: hint.into(),
        }
    }

    fn fail(id: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Item {
            id: id.to_owned(),
            level: Level::Fail,
            message: message.into(),
            hint: hint.into(),
        }
    }
}

/// Отчёт предпроверки: пункты, сборка CS2 и версия HLAE на момент проверки.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Report {
    pub items: Vec<Item>,
    pub cs2_build: Option<String>,
    pub hlae_version: Option<String>,
    /// Первая строка ответа `ffmpeg -version` (OBS-03).
    #[serde(default)]
    pub ffmpeg_version: Option<String>,
    /// Первая строка ответа `ffprobe -version`.
    #[serde(default)]
    pub ffprobe_version: Option<String>,
    /// Первая строка ответа `-version` у ffmpeg, которым пишет HLAE.
    #[serde(default)]
    pub hlae_ffmpeg_version: Option<String>,
    /// Свободно на диске результатов, байт.
    #[serde(default)]
    pub free_results: Option<u64>,
    /// Свободно на диске CS2, байт.
    #[serde(default)]
    pub free_cs2: Option<u64>,
}

impl Report {
    /// Худший уровень среди пунктов; пустой отчёт — `Ok`.
    pub fn worst(&self) -> Level {
        self.items
            .iter()
            .map(|i| i.level)
            .max()
            .unwrap_or(Level::Ok)
    }

    /// Блокирует ли отчёт старт записи: только `fail` (D-02).
    pub fn blocks(&self) -> bool {
        self.worst() == Level::Fail
    }

    /// Заменить пункты с теми же `id`, новые дописать в конец (второй проход оценки места).
    pub fn replace(&mut self, items: Vec<Item>) {
        for new in items {
            match self.items.iter_mut().find(|i| i.id == new.id) {
                Some(old) => *old = new,
                None => self.items.push(new),
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Option<Report> {
        serde_json::from_str(s).ok()
    }

    /// Сбой записи на ступени «предпроверка»; `None`, если проваленных пунктов нет.
    pub fn failure(&self, elapsed: Duration) -> Option<RecordFailure> {
        let failed: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.level == Level::Fail)
            .collect();
        let first = failed.first()?;
        let has = |id: &str| failed.iter().any(|i| i.id == id);
        let code = if has(ids::PREV_GAME) {
            FailCode::PrevGameStuck
        } else if has(ids::FOREIGN_GAME) {
            FailCode::ForeignGameRunning
        } else {
            FailCode::PrecheckFailed
        };
        let message = format!(
            "предпроверка: {}",
            failed
                .iter()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
        let mut f = RecordFailure::new(code, RecordState::Precheck, message, elapsed);
        if !first.hint.is_empty() {
            f.hint = first.hint.clone();
        }
        Some(f)
    }
}

/// Идентификаторы пунктов отчёта.
pub mod ids {
    pub const PREV_GAME: &str = "prev_game";
    pub const FOREIGN_GAME: &str = "foreign_game";
    pub const STEAM: &str = "steam";
    pub const STEAM_UPDATE: &str = "steam_update";
    pub const CS2_BUILD: &str = "cs2_build";
    pub const HLAE: &str = "hlae";
    pub const HOOK: &str = "hook";
    pub const CS2_EXE: &str = "cs2_exe";
    pub const FFMPEG: &str = "ffmpeg";
    pub const FFPROBE: &str = "ffprobe";
    pub const HLAE_FFMPEG: &str = "hlae_ffmpeg";
    pub const DEMO: &str = "demo";
    pub const OUT_DIR: &str = "out_dir";
    pub const SPACE_RESULTS: &str = "space_results";
    pub const SPACE_CS2: &str = "space_cs2";
}

/// Что известно о своей игре от прошлой задачи (D-14).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum PrevGameFact {
    /// Не проверялось: пункта в отчёте не будет.
    #[default]
    Unchecked,
    /// Прошлой игры нет или она уже закрыта.
    Clear,
    /// Игра закрылась после ожидания.
    Waited(Duration),
    /// Игра не закрылась за срок; `confirmed_alive` — выход проверен и игра жива.
    Stuck {
        pid: u32,
        secs: u64,
        confirmed_alive: bool,
    },
}

/// Факты окружения. Значение по умолчанию — «не проверялось»: такой пункт в отчёт не попадает.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub prev_game: PrevGameFact,
    /// Процессы системы: PID и имя образа; ошибка — список не получен.
    pub processes: Option<Result<Vec<(u32, String)>, String>>,
    /// Лежит ли `HLAE.exe` в папке HLAE.
    pub hlae_exe: Option<bool>,
    /// Лежит ли `x64\AfxHookSource2.dll` в папке HLAE.
    pub hook_dll: Option<bool>,
    /// Лежит ли `cs2.exe` в папке игры.
    pub cs2_exe: Option<bool>,
    pub ffmpeg: ToolFact,
    pub ffprobe: ToolFact,
    /// ffmpeg HLAE: путь из `ffmpeg\ffmpeg.ini` (`None` — не настроен) и ответ на `-version`.
    pub hlae_ffmpeg: Option<(Option<PathBuf>, ToolFact)>,
    pub demo: DemoFact,
    pub out_root: Option<OutFact>,
    /// Текст `appmanifest_730.acf` или причина, почему не прочитан.
    pub manifest: Option<Result<String, String>>,
    /// Текст `steam.inf` из папки игры.
    pub steam_inf: Option<String>,
    /// Пара (сборка CS2, версия HLAE) последней удачной записи.
    pub last_good: Option<(String, String)>,
    /// Версия HLAE сейчас.
    pub hlae_version: Option<String>,
    pub space: Option<SpaceFacts>,
}

/// Ответ утилиты на `-version`.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ToolFact {
    /// Не проверялось.
    #[default]
    Unchecked,
    Answered {
        first_line: String,
    },
    /// Не запустилась или завершилась с ошибкой.
    Failed(String),
    /// Не ответила в срок.
    TimedOut(Duration),
}

/// Что известно о демке.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DemoFact {
    #[default]
    Unchecked,
    /// Демка ещё не скачана: задача возьмёт её с FACEIT.
    Download,
    /// Файла нет.
    Missing(PathBuf),
    /// Первые байты файла.
    Header(PathBuf, Vec<u8>),
}

/// Путь результатов: пригоден ли для консоли HLAE и можно ли в него писать.
#[derive(Debug, Clone, PartialEq)]
pub struct OutFact {
    pub path: String,
    pub console_safe: bool,
    pub writable: Result<(), String>,
}

/// Свободное место и потребность (D-03).
#[derive(Debug, Clone)]
pub struct SpaceFacts {
    /// Свободно на диске результатов, байт.
    pub results: Result<u64, String>,
    /// Свободно на диске CS2, байт.
    pub cs2: Result<u64, String>,
    /// Результаты и CS2 на одном томе: сравнивается сумма.
    pub same_volume: bool,
    pub need: space::Need,
    /// Потребность посчитана по плану, а не взята минимумом.
    pub by_plan: bool,
}

const STEAM_HINT: &str = "Запустите Steam и войдите в аккаунт: без него CS2 не стартует.";

/// Оценить факты и собрать отчёт.
pub fn evaluate(f: &Facts) -> Report {
    let mut items = vec![];

    let own_pid = match &f.prev_game {
        PrevGameFact::Unchecked => None,
        PrevGameFact::Clear => {
            items.push(Item::ok(ids::PREV_GAME, "игра прошлой задачи закрыта"));
            None
        }
        PrevGameFact::Waited(d) => {
            items.push(Item::ok(
                ids::PREV_GAME,
                format!(
                    "игра прошлой задачи закрылась, ждал {} с",
                    d.as_secs_f64().round() as u64
                ),
            ));
            None
        }
        PrevGameFact::Stuck {
            pid,
            secs,
            confirmed_alive,
        } => {
            let tail = if *confirmed_alive {
                format!("не закрылась за {secs} с после завершения своего job")
            } else {
                format!(
                    "не закрылась за {secs} с после завершения своего job, выход не удалось подтвердить"
                )
            };
            items.push(Item::fail(
                ids::PREV_GAME,
                format!("игра прошлой задачи (cs2.exe, PID {pid}) {tail}"),
                FailCode::PrevGameStuck.hint(),
            ));
            Some(*pid)
        }
    };

    match &f.processes {
        None => {}
        Some(Err(e)) => {
            let hint = "Проверка чужого CS2 и Steam пропущена: закройте лишнее вручную.";
            items.push(Item::warn(
                ids::FOREIGN_GAME,
                format!("не удалось получить список процессов: {e}"),
                hint,
            ));
            items.push(Item::warn(
                ids::STEAM,
                format!("не удалось получить список процессов: {e}"),
                hint,
            ));
        }
        Some(Ok(list)) => {
            let foreign: Vec<u32> = list
                .iter()
                .filter(|(pid, name)| name.eq_ignore_ascii_case("cs2.exe") && Some(*pid) != own_pid)
                .map(|(pid, _)| *pid)
                .collect();
            if foreign.is_empty() {
                items.push(Item::ok(ids::FOREIGN_GAME, "чужой CS2 не запущен"));
            } else {
                let pids = foreign
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                items.push(Item::fail(
                    ids::FOREIGN_GAME,
                    format!("запущен CS2, который приложение не запускало (PID {pids})"),
                    FailCode::ForeignGameRunning.hint(),
                ));
            }
            if list
                .iter()
                .any(|(_, name)| name.eq_ignore_ascii_case("steam.exe"))
            {
                items.push(Item::ok(ids::STEAM, "Steam запущен"));
            } else {
                items.push(Item::fail(ids::STEAM, "Steam не запущен", STEAM_HINT));
            }
        }
    }

    let (steam_items, cs2_build) = steam_items(f);
    items.extend(steam_items);

    // Пункты файлов; тексты перенесены из `launch::check_paths` (PRE-03).
    let file_item = |id: &str, found: Option<bool>, ok: &str, bad: &str| {
        found.map(|b| {
            if b {
                Item::ok(id, ok)
            } else {
                Item::fail(id, bad, "")
            }
        })
    };
    items.extend(file_item(
        ids::HLAE,
        f.hlae_exe,
        "HLAE.exe найден",
        "Укажите папку HLAE в настройках (там должен лежать HLAE.exe)",
    ));
    items.extend(file_item(
        ids::HOOK,
        f.hook_dll,
        "x64\\AfxHookSource2.dll найден",
        "В папке HLAE нет x64\\AfxHookSource2.dll — нужна свежая версия HLAE с поддержкой CS2",
    ));
    items.extend(file_item(
        ids::CS2_EXE,
        f.cs2_exe,
        "cs2.exe найден",
        "Не найден cs2.exe: укажите папку «Counter-Strike Global Offensive» в настройках",
    ));

    items.extend(ffmpeg_item(ids::FFMPEG, "ffmpeg", &f.ffmpeg, Some(7)));
    items.extend(ffmpeg_item(ids::FFPROBE, "ffprobe", &f.ffprobe, None));
    if let Some((path, fact)) = &f.hlae_ffmpeg {
        items.extend(hlae_ffmpeg_item(path.as_deref(), fact));
    }

    items.extend(demo_item(&f.demo));
    items.extend(out_item(f.out_root.as_ref()));
    if let Some(s) = &f.space {
        items.extend(space_items(s));
    }

    let version = |t: &ToolFact| match t {
        ToolFact::Answered { first_line } => Some(first_line.clone()),
        _ => None,
    };
    Report {
        items,
        cs2_build,
        hlae_version: f.hlae_version.clone(),
        ffmpeg_version: version(&f.ffmpeg),
        ffprobe_version: version(&f.ffprobe),
        hlae_ffmpeg_version: f.hlae_ffmpeg.as_ref().and_then(|(_, t)| version(t)),
        free_results: f
            .space
            .as_ref()
            .and_then(|s| s.results.as_ref().ok().copied()),
        free_cs2: f.space.as_ref().and_then(|s| s.cs2.as_ref().ok().copied()),
    }
}

const FFMPEG_HINT: &str = "Установите ffmpeg 7+ и добавьте его в PATH: winget install Gyan.FFmpeg";

/// Старший номер версии из первой строки `ffmpeg version 9.0.2-…` (допустим префикс `n`) и
/// сама версия для сообщения; сборки из git (`N-12345-g…`) не разбираются.
fn parse_tool_version(first_line: &str) -> Option<(u32, String)> {
    let token = first_line
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)?;
    let token = token.strip_prefix(['n', 'N']).unwrap_or(token);
    let version: String = token
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let version = version.trim_end_matches('.').to_owned();
    let major = version.split('.').next()?.parse().ok()?;
    Some((major, version))
}

/// Пункт ffmpeg/ffprobe: ответ на `-version`; `min_major` — порог версии, если он есть.
fn ffmpeg_item(id: &str, name: &str, fact: &ToolFact, min_major: Option<u32>) -> Option<Item> {
    Some(match fact {
        ToolFact::Unchecked => return None,
        ToolFact::Failed(e) => Item::fail(
            id,
            format!("{name} не отвечает на -version: {e}"),
            FFMPEG_HINT,
        ),
        ToolFact::TimedOut(d) => Item::fail(
            id,
            format!(
                "{name} не отвечает на -version: нет ответа за {} с",
                d.as_secs_f64().round() as u64
            ),
            FFMPEG_HINT,
        ),
        ToolFact::Answered { first_line } => match (parse_tool_version(first_line), min_major) {
            (Some((major, version)), Some(min)) if major < min => Item::fail(
                id,
                format!("{name} {version} слишком старый — нужен {min} или новее"),
                FFMPEG_HINT,
            ),
            (Some((_, version)), _) => Item::ok(id, format!("{name} {version} отвечает")),
            (None, Some(_)) => Item::warn(
                id,
                format!("версию {name} определить не удалось: {first_line}"),
                "Нужен ffmpeg 7 или новее; если запись упадёт, обновите его: winget install Gyan.FFmpeg",
            ),
            (None, None) => Item::ok(id, format!("{name} отвечает")),
        },
    })
}

/// Пункт ffmpeg, которым пишет HLAE (Path в `ffmpeg\ffmpeg.ini`).
fn hlae_ffmpeg_item(path: Option<&std::path::Path>, fact: &ToolFact) -> Option<Item> {
    let id = ids::HLAE_FFMPEG;
    let hint = "Укажите в ffmpeg\\ffmpeg.ini папки HLAE путь Path= к рабочему ffmpeg.exe.";
    let Some(path) = path else {
        return Some(Item::fail(
            id,
            "у HLAE не настроен ffmpeg (Path в ffmpeg\\ffmpeg.ini)",
            hint,
        ));
    };
    let shown = path.display();
    Some(match fact {
        ToolFact::Unchecked => return None,
        ToolFact::Failed(e) => Item::fail(
            id,
            format!("ffmpeg HLAE ({shown}) не отвечает на -version: {e}"),
            hint,
        ),
        ToolFact::TimedOut(d) => Item::fail(
            id,
            format!(
                "ffmpeg HLAE ({shown}) не отвечает на -version: нет ответа за {} с",
                d.as_secs_f64().round() as u64
            ),
            hint,
        ),
        ToolFact::Answered { .. } => Item::ok(id, format!("ffmpeg HLAE отвечает ({shown})")),
    })
}

/// Пункт демки (PRE-07): есть ли файл и демка ли это.
fn demo_item(fact: &DemoFact) -> Option<Item> {
    let id = ids::DEMO;
    Some(match fact {
        DemoFact::Unchecked => return None,
        DemoFact::Download => Item::ok(id, "демка будет скачана с FACEIT"),
        DemoFact::Missing(p) => Item::fail(
            id,
            format!("демки нет: {}", p.display()),
            "Проверьте путь к файлу демки или скачайте демку заново.",
        ),
        DemoFact::Header(p, head) => {
            if head.starts_with(b"PBDEMS2\0") {
                Item::ok(id, "демка CS2 найдена")
            } else if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) || head.starts_with(&[0x1F, 0x8B])
            {
                Item::ok(id, "сжатая демка — распакуется при разборе")
            } else if head.starts_with(b"version https://git-lfs") {
                Item::fail(
                    id,
                    format!("демка — LFS-указатель, а не файл демки: {}", p.display()),
                    "Выполните git lfs pull или скачайте демку заново.",
                )
            } else {
                Item::fail(
                    id,
                    format!("файл не похож на демку CS2: {}", p.display()),
                    "Выберите файл .dem от CS2 или скачайте демку заново.",
                )
            }
        }
    })
}

/// Пункт пути результатов (PRE-07): пригодность для консоли HLAE и запись.
fn out_item(fact: Option<&OutFact>) -> Option<Item> {
    let id = ids::OUT_DIR;
    let o = fact?;
    Some(if !o.console_safe {
        Item::fail(
            id,
            format!(
                "в пути результатов есть пробелы или кавычки — HLAE не загрузит campath; выберите папку без пробелов ({})",
                o.path
            ),
            "Выберите папку результатов без пробелов и кавычек в настройках.",
        )
    } else if let Err(e) = &o.writable {
        Item::fail(
            id,
            format!("в папку результатов нельзя записать: {e}"),
            "Выберите другую папку результатов в настройках или проверьте права на запись.",
        )
    } else {
        Item::ok(id, format!("папка результатов подходит ({})", o.path))
    })
}

/// Пункты `steam_update` и `cs2_build` по манифесту Steam (DIF-01, DIF-02); вторым значением —
/// сборка CS2 для отчёта.
fn steam_items(f: &Facts) -> (Vec<Item>, Option<String>) {
    let Some(manifest) = &f.manifest else {
        return (vec![], None);
    };
    let unknown_hint =
        "Не удалось прочитать appmanifest_730.acf: убедитесь, что CS2 установлен и обновлён.";
    let text = match manifest {
        Ok(t) => t,
        Err(e) => {
            return (
                vec![
                    Item::warn(
                        ids::STEAM_UPDATE,
                        format!("не удалось определить состояние CS2 в Steam: {e}"),
                        unknown_hint,
                    ),
                    Item::warn(
                        ids::CS2_BUILD,
                        "сборку CS2 определить не удалось",
                        unknown_hint,
                    ),
                ],
                None,
            );
        }
    };
    let m = steam::parse_manifest(text);
    let update = match steam::steam_state(&m) {
        steam::SteamState::Idle => Item::ok(ids::STEAM_UPDATE, "CS2 в Steam не обновляется"),
        steam::SteamState::Updating(fl) => Item::fail(
            ids::STEAM_UPDATE,
            format!("Steam обновляет CS2 (StateFlags {fl})"),
            "Дождитесь окончания обновления в Steam и нажмите «Повторить» (или «Заново», если задача не дошла до записи).",
        ),
        steam::SteamState::NotInstalled(fl) => Item::fail(
            ids::STEAM_UPDATE,
            format!("CS2 установлен не полностью (StateFlags {fl})"),
            "Проверьте целостность файлов CS2 в Steam (свойства игры → Установленные файлы) и нажмите «Повторить» (или «Заново», если задача не дошла до записи).",
        ),
        steam::SteamState::UpdatePending(fl) => Item::warn(
            ids::STEAM_UPDATE,
            format!("для CS2 ждёт обновление (StateFlags {fl})"),
            "Обновите CS2 в Steam до записи: иначе игра может начать обновляться при запуске.",
        ),
        steam::SteamState::Unknown => Item::warn(
            ids::STEAM_UPDATE,
            "не удалось определить состояние CS2 в Steam",
            unknown_hint,
        ),
    };
    let patch = f.steam_inf.as_deref().and_then(steam::patch_version);
    let build = m.build_id.clone();
    let hlae = f.hlae_version.as_deref().unwrap_or("версия неизвестна");
    let shown = |b: &str| match &patch {
        Some(p) => format!("{b} (PatchVersion {p})"),
        None => b.to_owned(),
    };
    let build_item = match (&build, &f.last_good) {
        (None, _) => Item::warn(
            ids::CS2_BUILD,
            "сборку CS2 определить не удалось",
            unknown_hint,
        ),
        (Some(b), None) => Item::ok(
            ids::CS2_BUILD,
            format!(
                "удачной записи ещё не было — сравнивать не с чем (сборка CS2 {})",
                shown(b)
            ),
        ),
        (Some(b), Some((was, _))) if was == b => Item::ok(
            ids::CS2_BUILD,
            format!(
                "сборка CS2 та же, что в последней удачной записи: {}",
                shown(b)
            ),
        ),
        (Some(b), Some((was, _))) => Item::warn(
            ids::CS2_BUILD,
            format!(
                "сборка CS2 изменилась с последней удачной записи: была {was}, сейчас {} (HLAE {hlae})",
                shown(b)
            ),
            "Если запись упадёт, обновите HLAE до версии с поддержкой новой сборки CS2.",
        ),
    };
    (vec![update, build_item], build)
}

/// Гигабайты для сообщения.
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)
}

/// Пункты `space_results` и `space_cs2` (D-03, PRE-06): на одном томе оба сравниваются с суммой
/// потребностей, иначе каждый со своей.
pub fn space_items(s: &SpaceFacts) -> Vec<Item> {
    let basis = if s.by_plan {
        "по плану"
    } else {
        "минимум до плана"
    };
    let (need_results, need_cs2) = if s.same_volume {
        let sum = s.need.results.saturating_add(s.need.cs2);
        (sum, sum)
    } else {
        (s.need.results, s.need.cs2)
    };
    let one = |id: &str, disk: &str, free: &Result<u64, String>, need: u64| match free {
        Err(e) => Item::warn(
            id,
            format!("не удалось узнать свободное место: {e}"),
            "Проверьте вручную, что на диске достаточно места для записи.",
        ),
        Ok(free) => {
            let shared = if s.same_volume {
                ", один том с CS2 и результатами"
            } else {
                ""
            };
            let message = format!(
                "на диске {disk} свободно {} ГБ, нужно ~{} ГБ ({basis}{shared})",
                gb(*free),
                gb(need)
            );
            let hint =
                "Освободите место на диске или выберите другую папку результатов в настройках.";
            match space::verdict(*free, need) {
                Level::Ok => Item::ok(id, message),
                Level::Warn => Item::warn(id, message, hint),
                Level::Fail => Item::fail(id, message, hint),
            }
        }
    };
    vec![
        one(ids::SPACE_RESULTS, "результатов", &s.results, need_results),
        one(ids::SPACE_CS2, "CS2", &s.cs2, need_cs2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(list: &[(u32, &str)]) -> Option<Result<Vec<(u32, String)>, String>> {
        Some(Ok(list
            .iter()
            .map(|(p, n)| (*p, (*n).to_owned()))
            .collect()))
    }

    fn item<'a>(r: &'a Report, id: &str) -> &'a Item {
        r.items
            .iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("нет пункта {id}"))
    }

    #[test]
    fn foreign_cs2_blocks_with_foreign_game_running() {
        let r = evaluate(&Facts {
            processes: procs(&[(10, "steam.exe"), (77, "CS2.EXE")]),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::FOREIGN_GAME).level, Level::Fail);
        assert!(item(&r, ids::FOREIGN_GAME).message.contains("PID 77"));
        let f = r.failure(Duration::ZERO).expect("есть провал");
        assert_eq!(f.code, FailCode::ForeignGameRunning);
        assert_eq!(f.state, RecordState::Precheck);
        assert!(f.message.starts_with("предпроверка: "));
        assert!(f.hint.contains("Закройте CS2"));
    }

    #[test]
    fn own_stuck_game_is_prev_game_stuck_not_foreign() {
        let r = evaluate(&Facts {
            prev_game: PrevGameFact::Stuck {
                pid: 500,
                secs: 30,
                confirmed_alive: true,
            },
            processes: procs(&[(1, "Steam.exe"), (500, "cs2.exe")]),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::PREV_GAME).level, Level::Fail);
        assert_eq!(
            item(&r, ids::FOREIGN_GAME).level,
            Level::Ok,
            "свой PID чужим не считается"
        );
        assert_eq!(
            r.failure(Duration::ZERO).unwrap().code,
            FailCode::PrevGameStuck
        );
        let unsure = evaluate(&Facts {
            prev_game: PrevGameFact::Stuck {
                pid: 500,
                secs: 30,
                confirmed_alive: false,
            },
            ..Facts::default()
        });
        let msg = &item(&unsure, ids::PREV_GAME).message;
        assert!(msg.contains("выход не удалось подтвердить"));
        assert!(!msg.contains("жива"));
    }

    #[test]
    fn missing_steam_fails_and_an_unknown_process_list_only_warns() {
        let r = evaluate(&Facts {
            processes: procs(&[(5, "explorer.exe")]),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::STEAM).level, Level::Fail);
        assert!(item(&r, ids::STEAM).hint.contains("Запустите Steam"));
        assert_eq!(
            r.failure(Duration::ZERO).unwrap().code,
            FailCode::PrecheckFailed
        );
        let r = evaluate(&Facts {
            processes: Some(Err("отказано".to_owned())),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::STEAM).level, Level::Warn);
        assert_eq!(item(&r, ids::FOREIGN_GAME).level, Level::Warn);
        assert!(!r.blocks());
    }

    #[test]
    fn warnings_do_not_block_only_failures_do() {
        let mut r = Report::default();
        assert_eq!(r.worst(), Level::Ok);
        assert!(!r.blocks());
        assert!(r.failure(Duration::ZERO).is_none());
        r.items.push(Item::warn("a", "предупреждение", "подсказка"));
        assert_eq!(r.worst(), Level::Warn);
        assert!(!r.blocks());
        assert!(r.failure(Duration::ZERO).is_none());
        r.items.push(Item::fail("b", "провал", ""));
        assert!(r.blocks());
        let f = r.failure(Duration::ZERO).unwrap();
        assert_eq!(
            f.hint,
            FailCode::PrecheckFailed.hint(),
            "пустая подсказка пункта -> подсказка кода"
        );
    }

    #[test]
    fn report_without_env_facts_still_parses() {
        // отчёт фазы 2 из базы: полей версий утилит и свободного места ещё нет
        let old = r#"{"items":[{"id":"steam","level":"Ok","message":"Steam запущен","hint":""}],"cs2_build":"25815307","hlae_version":"2.192.7"}"#;
        let r = Report::from_json(old).expect("старый отчёт читается");
        assert_eq!(r.cs2_build.as_deref(), Some("25815307"));
        assert_eq!(r.ffmpeg_version, None);
        assert_eq!(r.free_results, None);
    }

    #[test]
    fn evaluate_carries_tool_versions_and_free_space_into_the_report() {
        let r = evaluate(&Facts {
            ffmpeg: ToolFact::Answered {
                first_line: "ffmpeg version 9.0.2".to_owned(),
            },
            ffprobe: ToolFact::Failed("нет".to_owned()),
            hlae_ffmpeg: Some((
                None,
                ToolFact::Answered {
                    first_line: "ffmpeg version 4.4".to_owned(),
                },
            )),
            space: Some(SpaceFacts {
                results: Ok(7),
                cs2: Err("нет диска".to_owned()),
                same_volume: false,
                need: space::floors(),
                by_plan: false,
            }),
            ..Facts::default()
        });
        assert_eq!(r.ffmpeg_version.as_deref(), Some("ffmpeg version 9.0.2"));
        assert_eq!(r.ffprobe_version, None);
        assert_eq!(r.hlae_ffmpeg_version.as_deref(), Some("ffmpeg version 4.4"));
        assert_eq!((r.free_results, r.free_cs2), (Some(7), None));
    }

    #[test]
    fn report_survives_a_json_round_trip() {
        let mut r = evaluate(&Facts {
            processes: procs(&[(1, "steam.exe")]),
            ..Facts::default()
        });
        r.cs2_build = Some("25815307".to_owned());
        r.hlae_version = Some("2.192.7".to_owned());
        let back = Report::from_json(&r.to_json()).expect("разбирается");
        assert_eq!(back, r);
        assert!(Report::from_json("не json").is_none());
    }

    #[test]
    fn replace_swaps_items_by_id() {
        let mut r = Report {
            items: vec![Item::ok("a", "1"), Item::ok("b", "2")],
            ..Report::default()
        };
        r.replace(vec![Item::fail("a", "новый", "x"), Item::ok("c", "3")]);
        assert_eq!(r.items.len(), 3);
        assert_eq!(r.items[0].message, "новый");
        assert_eq!(r.items[0].level, Level::Fail);
        assert_eq!(r.items[2].id, "c");
    }

    fn answered(line: &str) -> ToolFact {
        ToolFact::Answered {
            first_line: line.to_owned(),
        }
    }

    const IDLE_MANIFEST: &str = "\"AppState\"\n{\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25815307\"\n\t\"TargetBuildID\"\t\t\"0\"\n}\n";

    #[test]
    fn missing_hook_dll_fails_with_the_settings_text() {
        let r = evaluate(&Facts {
            hlae_exe: Some(true),
            hook_dll: Some(false),
            cs2_exe: Some(false),
            ..Facts::default()
        });
        assert_eq!(item(&r, ids::HLAE).level, Level::Ok);
        assert!(
            item(&r, ids::HOOK)
                .message
                .contains("В папке HLAE нет x64\\AfxHookSource2.dll")
        );
        assert!(
            item(&r, ids::CS2_EXE)
                .message
                .contains("Не найден cs2.exe: укажите папку")
        );
        let f = r.failure(Duration::ZERO).unwrap();
        assert_eq!(f.code, FailCode::PrecheckFailed);
        assert!(
            f.message.contains("; "),
            "сообщения проваленных пунктов через «; »"
        );
    }

    #[test]
    fn old_ffmpeg_fails_and_an_unparsable_version_only_warns() {
        let ver = |line: &str| {
            let r = evaluate(&Facts {
                ffmpeg: answered(line),
                ..Facts::default()
            });
            item(&r, ids::FFMPEG).clone()
        };
        let ok = ver("ffmpeg version 9.0.2-essentials_build-www.gyan.dev Copyright (c) 2000-2025");
        assert_eq!(ok.level, Level::Ok);
        assert_eq!(ok.message, "ffmpeg 9.0.2 отвечает");
        assert_eq!(ver("ffmpeg version n7.1 Copyright").level, Level::Ok);
        let old = ver("ffmpeg version 6.1.1 Copyright");
        assert_eq!(old.level, Level::Fail);
        assert!(old.message.contains("слишком старый — нужен 7 или новее"));
        assert!(old.hint.contains("winget install Gyan.FFmpeg"));
        let git = ver("ffmpeg version N-118000-gabcdef Copyright");
        assert_eq!(git.level, Level::Warn);
        assert!(git.message.contains("версию ffmpeg определить не удалось"));
        let dead = evaluate(&Facts {
            ffmpeg: ToolFact::TimedOut(Duration::from_secs(10)),
            ffprobe: ToolFact::Failed("не найден".to_owned()),
            ..Facts::default()
        });
        assert_eq!(item(&dead, ids::FFMPEG).level, Level::Fail);
        assert!(
            item(&dead, ids::FFMPEG)
                .message
                .contains("не отвечает на -version")
        );
        assert_eq!(item(&dead, ids::FFPROBE).level, Level::Fail);
        let probe = evaluate(&Facts {
            ffprobe: answered("ffprobe version 4.4 Copyright"),
            ..Facts::default()
        });
        assert_eq!(
            item(&probe, ids::FFPROBE).level,
            Level::Ok,
            "у ffprobe порога версии нет"
        );
        let hlae = |p: Option<&str>, t| {
            let r = evaluate(&Facts {
                hlae_ffmpeg: Some((p.map(PathBuf::from), t)),
                ..Facts::default()
            });
            item(&r, ids::HLAE_FFMPEG).level
        };
        assert_eq!(
            hlae(None, ToolFact::Unchecked),
            Level::Fail,
            "ffmpeg HLAE не настроен"
        );
        assert_eq!(
            hlae(Some("C:/f.exe"), ToolFact::Failed("x".into())),
            Level::Fail
        );
        assert_eq!(
            hlae(Some("C:/f.exe"), answered("ffmpeg version 7.0")),
            Level::Ok
        );
    }

    #[test]
    fn lfs_pointer_demo_fails_and_compressed_demo_passes() {
        let demo = |head: &[u8]| {
            let r = evaluate(&Facts {
                demo: DemoFact::Header(PathBuf::from("a.dem"), head.to_vec()),
                ..Facts::default()
            });
            item(&r, ids::DEMO).clone()
        };
        assert_eq!(demo(b"PBDEMS2\0rest").level, Level::Ok);
        assert_eq!(demo(&[0x28, 0xB5, 0x2F, 0xFD, 1]).level, Level::Ok);
        assert_eq!(demo(&[0x1F, 0x8B, 8]).level, Level::Ok);
        let lfs = demo(b"version https://git-lfs.github.com/spec/v1\noid sha256:00");
        assert_eq!(lfs.level, Level::Fail);
        assert!(lfs.message.contains("LFS-указатель"));
        assert!(lfs.hint.contains("git lfs pull"));
        let junk = demo(b"hello");
        assert_eq!(junk.level, Level::Fail);
        assert!(junk.message.contains("не похож на демку CS2"));
        let state = |d| {
            item(
                &evaluate(&Facts {
                    demo: d,
                    ..Facts::default()
                }),
                ids::DEMO,
            )
            .level
        };
        assert_eq!(state(DemoFact::Download), Level::Ok);
        assert_eq!(
            state(DemoFact::Missing(PathBuf::from("x.dem"))),
            Level::Fail
        );
        assert!(
            evaluate(&Facts::default()).items.is_empty(),
            "ничего не проверяли — пунктов нет"
        );
    }

    #[test]
    fn results_path_with_spaces_fails() {
        let out = |console_safe, writable| {
            let r = evaluate(&Facts {
                out_root: Some(OutFact {
                    path: "C:/мои ролики".to_owned(),
                    console_safe,
                    writable,
                }),
                ..Facts::default()
            });
            item(&r, ids::OUT_DIR).clone()
        };
        let spaces = out(false, Ok(()));
        assert_eq!(spaces.level, Level::Fail);
        assert!(spaces.message.contains("пробелы или кавычки"));
        let ro = out(true, Err("доступ запрещён".to_owned()));
        assert_eq!(ro.level, Level::Fail);
        assert!(ro.message.contains("нельзя записать"));
        assert_eq!(out(true, Ok(())).level, Level::Ok);
    }

    #[test]
    fn steam_update_blocks_the_start() {
        let with = |flags: &str| {
            let text = format!("\"StateFlags\"\t\t\"{flags}\"\n\"buildid\"\t\t\"1\"\n");
            evaluate(&Facts {
                manifest: Some(Ok(text)),
                ..Facts::default()
            })
        };
        let updating = with("1048580");
        let it = item(&updating, ids::STEAM_UPDATE);
        assert_eq!(it.level, Level::Fail);
        assert!(it.message.contains("StateFlags 1048580"));
        assert!(it.hint.contains("Повторить"));
        assert!(updating.blocks());
        assert_eq!(item(&with("1"), ids::STEAM_UPDATE).level, Level::Fail);
        assert_eq!(item(&with("6"), ids::STEAM_UPDATE).level, Level::Warn);
        assert_eq!(item(&with("4"), ids::STEAM_UPDATE).level, Level::Ok);
        let unread = evaluate(&Facts {
            manifest: Some(Err("нет файла".to_owned())),
            ..Facts::default()
        });
        assert_eq!(item(&unread, ids::STEAM_UPDATE).level, Level::Warn);
        assert!(!unread.blocks(), "нет файла — не повод блокировать старт");
        let garbage = evaluate(&Facts {
            manifest: Some(Ok("мусор".to_owned())),
            ..Facts::default()
        });
        assert_eq!(item(&garbage, ids::STEAM_UPDATE).level, Level::Warn);
        assert!(!garbage.blocks(), "по мусору старт не блокируется");
    }

    #[test]
    fn changed_cs2_build_warns_but_does_not_block() {
        let report = |last: Option<(&str, &str)>| {
            evaluate(&Facts {
                manifest: Some(Ok(IDLE_MANIFEST.to_owned())),
                steam_inf: Some("PatchVersion=1.41.9.0\n".to_owned()),
                last_good: last.map(|(b, h)| (b.to_owned(), h.to_owned())),
                hlae_version: Some("2.192.7".to_owned()),
                ..Facts::default()
            })
        };
        let first = report(None);
        assert_eq!(item(&first, ids::CS2_BUILD).level, Level::Ok);
        assert!(
            item(&first, ids::CS2_BUILD)
                .message
                .contains("сравнивать не с чем")
        );
        assert_eq!(first.cs2_build.as_deref(), Some("25815307"));
        assert_eq!(first.hlae_version.as_deref(), Some("2.192.7"));
        let same = report(Some(("25815307", "2.192.7")));
        assert_eq!(item(&same, ids::CS2_BUILD).level, Level::Ok);
        let changed = report(Some(("25000000", "2.192.7")));
        let it = item(&changed, ids::CS2_BUILD);
        assert_eq!(it.level, Level::Warn);
        assert!(it.message.contains("была 25000000, сейчас 25815307"));
        assert!(it.message.contains("1.41.9.0") && it.message.contains("HLAE 2.192.7"));
        assert!(it.hint.contains("обновите HLAE"));
        assert!(!changed.blocks());
    }

    fn space(
        results: u64,
        cs2: u64,
        same_volume: bool,
        need: (u64, u64),
        by_plan: bool,
    ) -> SpaceFacts {
        SpaceFacts {
            results: Ok(results),
            cs2: Ok(cs2),
            same_volume,
            need: space::Need {
                results: need.0,
                cs2: need.1,
            },
            by_plan,
        }
    }

    #[test]
    fn shared_volume_needs_the_sum_of_both_estimates() {
        const G: u64 = 1 << 30;
        // по отдельности каждому хватает (10 >= 8 и 10 >= 4), вместе 12 > 10
        let apart = space_items(&space(10 * G, 10 * G, false, (8 * G, 4 * G), true));
        assert!(apart.iter().all(|i| i.level != Level::Fail));
        let shared = space_items(&space(10 * G, 10 * G, true, (8 * G, 4 * G), true));
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|i| i.level == Level::Fail), "{shared:?}");
        assert!(
            shared[0]
                .message
                .contains("свободно 10.0 ГБ, нужно ~12.0 ГБ (по плану")
        );
        assert!(shared[0].message.starts_with("на диске результатов"));
        assert!(shared[1].message.starts_with("на диске CS2"));
        let floor = space_items(&space(20 * G, 20 * G, false, (3 * G, G), false));
        assert!(floor[0].message.contains("минимум до плана"));
        let warn = space_items(&space(10 * G, 100 * G, false, (8 * G, G), true));
        assert_eq!(warn[0].level, Level::Warn, "10 < 8*1.5");
        assert_eq!(warn[1].level, Level::Ok);
        let r = evaluate(&Facts {
            space: Some(space(G, 100 * G, false, (8 * G, G), true)),
            ..Facts::default()
        });
        assert!(item(&r, ids::SPACE_RESULTS).level == Level::Fail && r.blocks());
    }

    #[test]
    fn unknown_free_space_only_warns() {
        let items = space_items(&SpaceFacts {
            results: Err("диск недоступен".to_owned()),
            cs2: Ok(1 << 40),
            same_volume: false,
            need: space::floors(),
            by_plan: false,
        });
        assert_eq!(items[0].level, Level::Warn);
        assert!(
            items[0]
                .message
                .contains("не удалось узнать свободное место: диск недоступен")
        );
        assert_eq!(items[1].level, Level::Ok);
    }
}
