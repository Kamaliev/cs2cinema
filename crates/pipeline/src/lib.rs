//! Весь конвейер «демка → монтажный план → файлы для HLAE» одной библиотекой,
//! чтобы им пользовались и окно, и консоль. Каждый тяжёлый шаг сообщает о ходе работы через `progress`.

use std::path::{Path, PathBuf};

pub use host;
pub mod acceptance;
pub mod assemble;
pub mod gamedir;
pub mod inspect;
pub mod job;
pub mod manifest;
pub mod precheck;
pub mod record;
pub use record::{Deadlines, Event, FailCode, RecordFailure, RecordReport, RecordRequest, RecordState, record};

use cs2::{Match, PlayerId};
use director::{Order, PositionSource, Timeline};
use highlights::Highlight;

pub type Result<T> = std::result::Result<T, String>;

/// Ошибка задачи: текст для строки задачи и, если это сбой записи, его типизированное описание.
#[derive(Debug)]
pub struct RunError {
    pub message: String,
    pub fail: Option<RecordFailure>,
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        RunError { message, fail: None }
    }
}

impl From<&str> for RunError {
    fn from(message: &str) -> Self {
        RunError::from(message.to_owned())
    }
}

/// Этапы конвейера — для строки состояния в окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Download,
    Parse,
    Positions,
    Geometry,
    Plan,
    Record,
    Assemble,
    Done,
}

impl Stage {
    pub const ALL: [Stage; 8] = [Stage::Download, Stage::Parse, Stage::Positions, Stage::Geometry, Stage::Plan, Stage::Record, Stage::Assemble, Stage::Done];

    pub fn title(self) -> &'static str {
        match self {
            Stage::Download => "Скачивание",
            Stage::Parse => "Разбор",
            Stage::Positions => "Позиции",
            Stage::Geometry => "Геометрия",
            Stage::Plan => "План",
            Stage::Record => "Запись",
            Stage::Assemble => "Склейка",
            Stage::Done => "Готово",
        }
    }
}

/// Куда конвейер сообщает о ходе работы — и откуда узнаёт, что работу отменили.
pub trait Progress: Sync {
    fn stage(&self, s: Stage);
    fn log(&self, msg: &str);
    /// Пользователь отменил задачу: конвейер останавливается на ближайшей границе этапа.
    fn cancelled(&self) -> bool {
        false
    }
    /// Второе нажатие «Отмена» (D-12): закрыть свой job сразу, без `quit` и ожидания.
    fn force_cancelled(&self) -> bool {
        false
    }
    /// Типизированное событие записи; по умолчанию — строка журнала.
    fn event(&self, e: &record::Event) {
        self.log(&e.to_string())
    }
    /// Итог шага, который вызывающий может сохранить: демка, папка задачи, отчёт предпроверки, пара удачной
    /// записи. `Quiet` и консоль его игнорируют; приложение кладёт в свою базу.
    fn artifact(&self, _a: &Artifact) {}
}

/// Итог шага задачи для вызывающего (см. `Progress::artifact`): `pipeline` ничего не знает о том, куда это сохранят.
#[derive(Debug, Clone, PartialEq)]
pub enum Artifact {
    /// Скачанная демка матча FACEIT: id матча и путь к файлу.
    Demo { match_id: String, path: PathBuf },
    /// Папка результата задачи: известна после разбора демки (или из прошлой попытки при повторе).
    OutDir(PathBuf),
    /// Отчёт предпроверки в JSON; `passed` — нет ли в нём ошибок уровня `fail`.
    Precheck { json: String, passed: bool },
    /// Пара «сборка CS2 / версия HLAE» удачной записи.
    LastGood { cs2_build: String, hlae_version: String },
}

/// Сообщение об отмене — по нему окно отличает отмену от ошибки.
pub const CANCELLED: &str = "отменено";

/// Проверка отмены между этапами.
pub fn check(p: &dyn Progress) -> Result<()> {
    if p.cancelled() { Err(CANCELLED.to_owned()) } else { Ok(()) }
}

/// Молчаливый вариант для тестов и консоли.
pub struct Quiet;
impl Progress for Quiet {
    fn stage(&self, _: Stage) {}
    fn log(&self, _: &str) {}
}

/// Разобранный матч: то, что показывается в списке моментов.
pub struct Analysis {
    pub label: String,
    pub demo_path: PathBuf,
    pub m: Match,
    pub found: Vec<Highlight>,
    data: Vec<u8>,
    track: Option<positions::Track>,
}

/// Откуда брать демку.
pub enum Source {
    File(PathBuf),
    /// Ссылка или id матча FACEIT; скачивается в `dir`.
    Faceit { link: String, key: String, demo_index: usize, dir: PathBuf },
}

/// Разбор для вызывающих, которым достаточно текста ошибки (окно): потолки по умолчанию, сбой — строкой.
pub fn analyze(src: &Source, p: &dyn Progress) -> Result<Analysis> {
    analyze_with(src, &demo::DemoLimits::default(), p).map_err(|e| e.message)
}

/// Типизированный сбой разбора демки: до папки задачи и запуска игры, класс «прочее».
fn bad_demo(reason: &str) -> RunError {
    precheck_failure(FailCode::BadDemo, record::failure::msg_bad_demo(reason))
}

/// Скачивает (при необходимости) и разбирает демку с потолками `limits`. Бомба и файл сверх лимита —
/// `DEMO_TOO_LARGE`; мусор, обрезанная, испорченная и пустая демка — `BAD_DEMO`; ошибки скачивания FACEIT
/// остаются строковыми.
pub fn analyze_with(src: &Source, limits: &demo::DemoLimits, p: &dyn Progress) -> std::result::Result<Analysis, RunError> {
    let (demo_path, label) = match src {
        Source::File(p) => {
            let label = p.file_stem().and_then(|s| s.to_str()).unwrap_or("match");
            let label = label.strip_suffix(".dem").unwrap_or(label).to_owned();
            (p.clone(), label)
        }
        Source::Faceit { link, key, demo_index, dir } => {
            check(p)?;
            p.stage(Stage::Download);
            let id = faceit::parse_match_id(link).ok_or("не похоже на ссылку матча FACEIT")?;
            let client = if key.trim().is_empty() { faceit::Client::from_env().map_err(|e| e.to_string())? } else { faceit::Client::new(key.trim()) };
            p.log(&format!("матч {id}: ищу демку…"));
            let urls = client.demo_urls(&id).map_err(|e| e.to_string())?;
            let url = urls
                .get(*demo_index)
                .ok_or_else(|| format!("в матче {} демок, а запрошена #{demo_index}", urls.len()))?;
            p.log("качаю демку…");
            let path = client.download(url, dir).map_err(|e| e.to_string())?;
            (path, id)
        }
    };
    check(p)?;
    p.stage(Stage::Parse);
    p.log("разбираю демку…");
    let data = demo::read_demo_limited(&demo_path, limits).map_err(|e| match e {
        demo::DemoError::TooLarge { .. } => precheck_failure(FailCode::DemoTooLarge, record::failure::msg_demo_too_large(&e.to_string())),
        demo::DemoError::Bad(_) => bad_demo(&e.to_string()),
    })?;
    let parsed = demo::parse(&data).map_err(|e| bad_demo(&e.to_string()))?;
    let m = Match::from_parsed(&parsed);
    if m.kills.is_empty() {
        return Err(bad_demo("убийств не найдено: демка пустая или не распознана"));
    }
    let found = highlights::find(&m, &highlights::Config::default());
    Ok(Analysis { label, demo_path, m, found, data, track: None })
}

#[derive(Debug, Clone)]
pub struct PlanOptions {
    pub fps: u32,
    pub chronological: bool,
    pub flybys: bool,
    pub walls: bool,
    pub refresh_maps: bool,
    pub offline: bool,
    pub maps_dir: Option<PathBuf>,
    /// Размер кадра: и окно игры, и ролик.
    pub size: (u32, u32),
    /// Кодер склейки.
    pub encoder: renderer::Encoder,
    /// Дубли HLAE кодировать на видеокарте (`renderer::TAKE_NVENC`).
    pub take_nvenc: bool,
    /// Подробный ролик про игрока: весь KAST и все ракурсы (`director::Density::Full`).
    pub detailed: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            fps: 60,
            chronological: false,
            flybys: true,
            walls: true,
            refresh_maps: false,
            offline: false,
            maps_dir: None,
            size: (1280, 720),
            encoder: renderer::Encoder::default(),
            take_nvenc: false,
            detailed: false,
        }
    }
}

pub struct Plan {
    pub timeline: Timeline,
    pub out_dir: PathBuf,
    /// Аргументы ffmpeg для склейки дублей (запускать в `out_dir`).
    pub ffmpeg_args: Vec<String>,
    /// Длина готового ролика, с.
    pub video_secs: f32,
    /// Ожидаемые дубли в порядке записи — выводятся из плана до начала записи.
    pub takes: Vec<renderer::TakePlan>,
    /// Метка задачи в маркерах консоли (тот же nonce записан в cfg).
    pub nonce: String,
}

/// Случайная метка плана для маркеров консоли (8 строчных hex-символов), не секрет.
pub fn new_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    format!("{:08x}", h.finish() as u32)
}

/// Сколько кандидатов (или игроков демки) показывает текст сбоя выбора игрока.
pub const CANDIDATES_MAX: usize = 10;

/// Почему игрока не удалось выбрать. Списки — строки «имя (SteamID64)», не длиннее `CANDIDATES_MAX`;
/// `more` — сколько не вошло.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerError {
    /// Ни SteamID, ни ник никого не нашли; `players` — кто есть в демке.
    NotFound { query: String, players: Vec<String>, more: usize },
    /// Первый сработавший ярус дал двух и более игроков.
    Ambiguous { query: String, candidates: Vec<String>, more: usize },
}

impl PlayerError {
    /// Типизированный сбой записи в состоянии предпроверки: до папки задачи и запуска игры.
    pub fn into_run_error(self) -> RunError {
        use record::failure;
        let (code, message) = match self {
            PlayerError::NotFound { query, players, more } => (FailCode::PlayerNotFound, failure::msg_player_not_found(&query, &players, more)),
            PlayerError::Ambiguous { query, candidates, more } => (FailCode::PlayerAmbiguous, failure::msg_player_ambiguous(&query, &candidates, more)),
        };
        precheck_failure(code, message)
    }
}

/// `RunError` с сбоем `code` в состоянии предпроверки.
fn precheck_failure(code: FailCode, message: String) -> RunError {
    let f = RecordFailure::new(code, RecordState::Precheck, message, std::time::Duration::ZERO);
    RunError { message: f.message.clone(), fail: Some(f) }
}

/// Ник для сравнения: без краёв, серии пробельных символов — один пробел, без учёта регистра.
fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// «имя (SteamID64)»: имя из демки очищено от управляющих символов и обрезано.
fn player_label(p: &cs2::Player) -> String {
    let name = record::failure::clean_text(&p.name, record::failure::TEXT_NAME_MAX);
    if p.xuid == 0 { format!("{name} (без SteamID)") } else { format!("{name} ({})", p.xuid) }
}

/// Не больше `CANDIDATES_MAX` подписей и число не вошедших.
fn labels(players: &[&cs2::Player]) -> (Vec<String>, usize) {
    (players.iter().take(CANDIDATES_MAX).map(|p| player_label(p)).collect(), players.len().saturating_sub(CANDIDATES_MAX))
}

/// Из совпавших на одном ярусе: один — он; все записи одного человека (тот же SteamID64 и то же имя
/// после нормализации) — запись с наибольшим числом убийств и брошенных гранат; иначе неоднозначность.
fn settle(m: &Match, query: &str, hits: Vec<&cs2::Player>) -> std::result::Result<PlayerId, PlayerError> {
    if let [one] = hits.as_slice() {
        return Ok(one.id);
    }
    let first = hits[0];
    if first.xuid != 0 && hits.iter().all(|p| p.xuid == first.xuid && normalized(&p.name) == normalized(&first.name)) {
        let activity = |p: &cs2::Player| m.kills.iter().filter(|k| k.attacker == p.id).count() + m.grenades.iter().filter(|g| g.thrower == p.id).count();
        // при равенстве берётся первая запись (max_by_key отдаёт последнюю, поэтому идём с конца)
        let best = hits.iter().rev().max_by_key(|p| activity(p)).expect("hits не пуст");
        return Ok(best.id);
    }
    let (candidates, more) = labels(&hits);
    Err(PlayerError::Ambiguous { query: query.to_owned(), candidates, more })
}

/// Игрок демки по SteamID64 и нику. Ярусы по порядку: SteamID64 → точное имя → имя без регистра и с
/// одинарными пробелами → подстрока без регистра. На ярусе одно совпадение — игрок, несколько —
/// `PlayerError::Ambiguous` (угадывать нельзя: ролик не про того человека), ни одного — следующий ярус.
/// SteamID, которого нет в демке (или 0), не мешает выбору по нику. Минимальной длины запроса нет.
pub fn resolve_player(m: &Match, steam: Option<u64>, nick: &str) -> std::result::Result<PlayerId, PlayerError> {
    let steam = steam.filter(|&s| s != 0);
    let query = if nick.trim().is_empty() { steam.map(|s| s.to_string()).unwrap_or_else(|| nick.to_owned()) } else { nick.to_owned() };
    if let Some(s) = steam {
        let hits: Vec<&cs2::Player> = m.players.iter().filter(|p| p.xuid == s).collect();
        if !hits.is_empty() {
            return settle(m, &query, hits);
        }
    }
    let want = normalized(nick);
    if !want.is_empty() {
        let tiers: [&dyn Fn(&cs2::Player) -> bool; 3] = [&|p| p.name == nick, &|p| normalized(&p.name) == want, &|p| normalized(&p.name).contains(&want)];
        for tier in tiers {
            let hits: Vec<&cs2::Player> = m.players.iter().filter(|p| tier(p)).collect();
            if !hits.is_empty() {
                return settle(m, &query, hits);
            }
        }
    }
    let everyone: Vec<&cs2::Player> = m.players.iter().collect();
    let (players, more) = labels(&everyone);
    Err(PlayerError::NotFound { query, players, more })
}

/// Игрок демки по FACEIT: только точное или единственное совпадение (`resolve_player`);
/// неоднозначность и отсутствие — `None`.
pub fn find_player(m: &Match, steam_id: Option<u64>, nick: &str) -> Option<PlayerId> {
    resolve_player(m, steam_id, nick).ok()
}

/// У героя есть что снимать: убийства или полезные гранаты. Иначе `NO_HIGHLIGHTS` до записи
/// (та же проверка стоит в начале `plan_player`, но там её видит только приложение, строкой).
pub fn ensure_highlights(m: &Match, hero: PlayerId) -> std::result::Result<(), RunError> {
    if highlights::for_player(m, hero, &highlights::Config::default()).is_empty() && highlights::utility_for(m, hero).is_empty() {
        return Err(precheck_failure(FailCode::NoHighlights, record::failure::msg_no_highlights(&m.name(hero))));
    }
    Ok(())
}

/// Ролик про одного игрока (как `cs2-cli --player`): по раундам его глазами, повторы сильных
/// моментов, гранаты и их эффект глазами противников. Файлы для HLAE пишутся в `out_dir`.
pub fn plan_player(a: &mut Analysis, hero: PlayerId, o: &PlanOptions, out_dir: &Path, p: &dyn Progress) -> Result<Plan> {
    let found = highlights::for_player(&a.m, hero, &highlights::Config::default());
    let utility = highlights::utility_for(&a.m, hero);
    if found.is_empty() && utility.is_empty() {
        return Err(format!("у игрока {} в этом матче нет ни убийств, ни полезных гранат", a.m.name(hero)));
    }
    p.log(&format!("{}: моментов {}, полезных гранат {}", a.m.name(hero), found.len(), utility.len()));
    let cfg = director::Config {
        order: if o.chronological { Order::Chronological } else { Order::BuildUp },
        cinematic: true,
        density: if o.detailed { director::Density::Full } else { director::Density::Balanced },
        ..Default::default()
    };
    let mesh = load_positions(a, o, p)?;

    check(p)?;
    p.stage(Stage::Plan);
    p.log("строю монтаж…");
    let timeline = match (&a.track, &mesh) {
        (Some(t), Some(mesh)) => {
            let p = positions::WithGeometry { track: t, mesh };
            director::direct_all(&a.m, hero, &found, &utility, &cfg, &p as &dyn PositionSource)
        }
        (Some(t), None) => director::direct_all(&a.m, hero, &found, &utility, &cfg, t as &dyn PositionSource),
        _ => director::direct_all(&a.m, hero, &found, &utility, &cfg, &director::NoPositions),
    };

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let (title, outro) = director::cards(&a.m, hero);
    // HLAE ищет campath относительно game\bin\win64, поэтому в скрипте нужен абсолютный путь
    let mut rcfg = renderer::Config {
        fps: o.fps,
        size: o.size,
        out_dir: absolute_slash(out_dir),
        font_file: font_file(),
        title: Some(title),
        outro: Some(outro),
        encoder: o.encoder,
        ..Default::default()
    };
    if o.take_nvenc {
        rcfg.dialect.record_ffmpeg = Some(renderer::TAKE_NVENC.into());
    }
    let nonce = new_nonce();
    rcfg.nonce = nonce.clone();
    let out = renderer::render(&a.m, &timeline, &rcfg);
    let takes = renderer::takes(&timeline, &rcfg);
    let (ffmpeg_args, video_secs) = renderer::ffmpeg_args(&timeline, &rcfg).unwrap_or_default();
    // скрипты и пути камер прошлого плана (их могло быть больше) — убираем, чтобы не путали
    for e in std::fs::read_dir(out_dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("cs2c_") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    out.write_to(out_dir).map_err(|e| format!("запись в {}: {e}", out_dir.display()))?;
    Ok(Plan { timeline, out_dir: out_dir.to_path_buf(), ffmpeg_args, video_secs, takes, nonce })
}

/// Позиции игроков (для пролётов) и геометрия карты (чтобы камера не смотрела сквозь стены).
fn load_positions(a: &mut Analysis, o: &PlanOptions, p: &dyn Progress) -> Result<Option<geometry::Mesh>> {
    if !o.flybys {
        return Ok(None);
    }
    if a.track.is_none() {
        check(p)?;
        p.stage(Stage::Positions);
        p.log("снимаю позиции игроков…");
        a.track = Some(positions::track(&a.data, 4).map_err(|e| e.to_string())?);
    }
    if !o.walls {
        return Ok(None);
    }
    let dir = o.maps_dir.clone().unwrap_or_else(geometry::default_dir);
    check(p)?;
    p.stage(Stage::Geometry);
    p.log(&format!("геометрия карты {}…", a.m.map));
    let opts = geometry::Options { refresh: o.refresh_maps, offline: o.offline, ..Default::default() };
    match geometry::load_map(&dir, &a.m.map, &opts) {
        Ok(l) => Ok(Some(l.mesh)),
        Err(e) => {
            p.log(&format!("предупреждение: без стен — {e}"));
            Ok(None)
        }
    }
}

/// Шрифт подписей в ролике (Arial Bold из Windows; `CS2CINEMA_FONT` — свой).
fn font_file() -> Option<String> {
    let candidates = [
        std::env::var("CS2CINEMA_FONT").ok(),
        Some(r"C:\Windows\Fonts\arialbd.ttf".to_owned()),
        Some(r"C:\Windows\Fonts\arial.ttf".to_owned()),
    ];
    candidates.into_iter().flatten().find(|p| Path::new(p).is_file()).map(|p| p.replace('\\', "/"))
}

/// Абсолютный путь с `/` — так его понимает консоль CS2.
pub fn absolute_slash(p: &Path) -> String {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_owned()
}

impl Analysis {
    /// Размер распакованной демки в байтах — столько занимает её копия в `game/csgo` (диск CS2, D-03).
    pub fn demo_len(&self) -> u64 {
        self.data.len() as u64
    }

    /// Кладёт распакованную демку в `path` (CS2 читает `.dem` из `game/csgo`, архивы `.zst` не понимает).
    pub fn write_demo(&self, path: &Path) -> Result<()> {
        std::fs::write(path, &self.data).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Кладёт распакованную копию демки `src` в `dest` без разбора (повтор не скачивает и не планирует,
/// ему нужен только файл для `playdemo`, D-08). Ошибки называют путь.
pub fn write_demo_copy(src: &Path, dest: &Path) -> Result<()> {
    let data = demo::read_demo(src).map_err(|e| format!("{}: {e}", src.display()))?;
    std::fs::write(dest, data).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Имя, безопасное для консоли CS2 (без пробелов и кавычек).
pub fn console_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if n.is_empty() { "match".into() } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_copy_is_written_unpacked() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-demo-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dest) = (dir.join("a.dem"), dir.join("copy.dem"));
        let body = b"PBDEMS2\0 demo body".to_vec();
        std::fs::write(&src, &body).unwrap();
        write_demo_copy(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let gone = dir.join("нет.dem");
        let e = write_demo_copy(&gone, &dest).unwrap_err();
        assert!(e.contains("нет.dem"), "ошибка называет путь: {e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Матч только из игроков (без раундов) — для проверок выбора по нику.
    fn match_of(players: &[(&str, u64)]) -> Match {
        let list = players.iter().enumerate().map(|(i, (name, xuid))| (i as i32, i as i32, name.to_string(), *xuid)).collect();
        Match::build("de_mirage".into(), 64.0, list, &[])
    }

    fn name_of(m: &Match, id: PlayerId) -> String {
        m.name(id)
    }

    fn crowd() -> Match {
        let names: Vec<String> = (1..=12).map(|i| format!("player{i:02}")).collect();
        let list: Vec<(&str, u64)> = names.iter().enumerate().map(|(i, n)| (n.as_str(), 76561198000000100 + i as u64)).collect();
        match_of(&list)
    }

    fn ambiguous(r: std::result::Result<PlayerId, PlayerError>) -> (Vec<String>, usize) {
        match r {
            Err(PlayerError::Ambiguous { candidates, more, .. }) => (candidates, more),
            other => panic!("ожидалась неоднозначность, а вышло {other:?}"),
        }
    }

    #[test]
    fn resolve_player_ignores_case_and_spaces_after_the_exact_tier() {
        // имена очищаются в Match::build (06-02): двойной пробел схлопывается, запрос с ним всё равно находит игрока
        let m = match_of(&[("Big  Boss", 1), ("other", 2)]);
        for q in [" big boss ", "BIG BOSS", "Big  Boss"] {
            let id = resolve_player(&m, None, q).unwrap_or_else(|e| panic!("{q:?}: {e:?}"));
            assert_eq!(name_of(&m, id), "Big Boss", "{q:?}");
        }
        // точный ярус раньше регистрового
        let m = match_of(&[("Ace", 10), ("ace", 11)]);
        let id = resolve_player(&m, None, "ace").unwrap();
        assert_eq!(name_of(&m, id), "ace");
        let (candidates, more) = ambiguous(resolve_player(&m, None, "ACE"));
        assert_eq!(more, 0);
        assert!(candidates.iter().any(|c| c == "Ace (10)") && candidates.iter().any(|c| c == "ace (11)"), "{candidates:?}");
    }

    #[test]
    fn resolve_player_prefers_steam_id_over_nick() {
        let m = match_of(&[("s1mple", 76561198034202275), ("ZywOo", 76561198000000003)]);
        let id = resolve_player(&m, Some(76561198000000003), "s1mple").unwrap();
        assert_eq!(name_of(&m, id), "ZywOo", "SteamID сильнее ника");
        // SteamID, которого нет, — выбор по нику; ноль — «нет SteamID»
        let id = resolve_player(&m, Some(76561198999999999), "s1mple").unwrap();
        assert_eq!(name_of(&m, id), "s1mple");
        assert_eq!(resolve_player(&m, Some(0), "zywoo"), resolve_player(&m, None, "zywoo"));
        // SteamID без ника
        let id = resolve_player(&m, Some(76561198034202275), "").unwrap();
        assert_eq!(name_of(&m, id), "s1mple");
    }

    #[test]
    fn resolve_player_merges_steam_id_twins_with_one_name() {
        use demo::events::{GameEvent, Value};
        let ev = |tick, name: &str, f: &[(&str, i64)]| GameEvent { tick, name: name.into(), fields: f.iter().map(|(k, v)| (k.to_string(), Value::Int(*v))).collect() };
        let players = vec![(0, 0, "Twin".to_owned(), 77), (1, 1, "twin ".to_owned(), 77), (2, 2, "other".to_owned(), 78)];
        let events = vec![
            ev(10, "round_start", &[]),
            ev(20, "player_death", &[("userid", 2), ("attacker", 1), ("headshot", 0)]),
            ev(30, "player_death", &[("userid", 2), ("attacker", 1), ("headshot", 0)]),
            ev(40, "player_death", &[("userid", 2), ("attacker", 0), ("headshot", 0)]),
        ];
        let m = Match::build("de_mirage".into(), 64.0, players, &events);
        assert_eq!(resolve_player(&m, Some(77), "").unwrap(), PlayerId(1), "берётся запись с большим числом убийств");
        // разные имена при одном SteamID — не один человек
        let m = match_of(&[("Alpha", 77), ("Beta", 77)]);
        let (candidates, _) = ambiguous(resolve_player(&m, Some(77), ""));
        assert_eq!(candidates, vec!["Alpha (77)".to_owned(), "Beta (77)".to_owned()]);
    }

    #[test]
    fn resolve_player_caps_candidates_at_ten() {
        let m = crowd();
        let r = resolve_player(&m, None, "player");
        let (candidates, more) = ambiguous(r.clone());
        assert_eq!((candidates.len(), more), (CANDIDATES_MAX, 2));
        let message = r.unwrap_err().into_run_error().message;
        assert!(message.contains("и ещё 2"), "{message}");
        assert!(message.contains("player01 (") && !message.contains("player11"), "{message}");
    }

    #[test]
    fn resolve_player_empty_nick_is_not_found() {
        let m = crowd();
        for q in ["", "   ", "\t\n"] {
            match resolve_player(&m, None, q) {
                Err(PlayerError::NotFound { players, more, .. }) => assert_eq!((players.len(), more), (CANDIDATES_MAX, 2), "{q:?}"),
                other => panic!("{q:?}: {other:?}"),
            }
        }
        // чужой SteamID и пустой ник — тоже «не найден»
        assert!(matches!(resolve_player(&m, Some(5), ""), Err(PlayerError::NotFound { .. })));
        assert_eq!(find_player(&m, None, "player"), None);
    }

    #[test]
    fn resolve_player_cleans_candidates_and_cuts_a_long_query() {
        let m = match_of(&[("bad\nname", 1), ("bad\u{7}name2", 2), ("clean", 3)]);
        let message = resolve_player(&m, None, "bad").unwrap_err().into_run_error().message;
        assert!(!message.chars().any(char::is_control), "{message:?}");
        assert!(message.contains("badname (1)") && message.contains("badname2 (2)"), "{message}");

        let long = "x".repeat(500);
        let e = resolve_player(&m, None, &long).unwrap_err().into_run_error();
        let run = e.message.chars().filter(|&c| c == 'x').count();
        assert_eq!(run, record::failure::TEXT_NAME_MAX, "запрос обрезан до 64 символов: {}", e.message);
        // обрезка по символам, а не по байтам
        let cyr = "ж".repeat(200);
        let e = resolve_player(&m, None, &cyr).unwrap_err().into_run_error();
        assert_eq!(e.message.chars().filter(|&c| c == 'ж').count(), record::failure::TEXT_NAME_MAX);
        assert_eq!(e.fail.unwrap().code, FailCode::PlayerNotFound);
    }

    #[test]
    fn console_name_is_safe() {
        assert_eq!(console_name("1-abc def/ü"), "1-abc_def__");
        assert_eq!(console_name(""), "match");
    }

    #[test]
    fn new_nonce_is_eight_lowercase_hex_and_console_safe() {
        let a = new_nonce();
        let b = new_nonce();
        for n in [&a, &b] {
            assert_eq!(n.len(), 8, "{n}");
            assert!(n.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{n}");
            assert!(renderer::console_safe(n));
        }
        assert_ne!(a, b, "каждый план получает новую метку");
    }

    /// Реальная демка (CS2_TEST_DEMO или демка из репозитория, если она скачана из LFS, а не указатель):
    /// разбор → игрок → план ролика про него и файлы.
    #[test]
    fn analyze_player_plan() {
        let demo = std::env::var_os("CS2_TEST_DEMO")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../1-3e7db9e3-8e92-4761-a1db-3729fb7de11c-1-1.dem.zst"));
        if !demo.metadata().is_ok_and(|m| m.len() > 1 << 20) {
            eprintln!("нет реальной демки — тест пропущен");
            return;
        }
        let mut a = analyze(&Source::File(demo), &Quiet).unwrap();
        assert!(!a.found.is_empty());
        let out = std::env::temp_dir().join(format!("cs2cinema-pipeline-{}", std::process::id()));
        let o = PlanOptions { walls: false, ..Default::default() };
        let hero = a.found[0].player;
        let name = a.m.name(hero);
        assert_eq!(find_player(&a.m, None, &name.to_uppercase()), Some(hero));
        let p = plan_player(&mut a, hero, &o, &out, &Quiet).unwrap();
        assert!(!p.timeline.shots.is_empty() && !p.ffmpeg_args.is_empty());
        assert!(out.join("highlights.cfg").is_file() && out.join("plan.json").is_file());
        assert_eq!(p.takes.len(), p.timeline.shots.len());
        let cfg = std::fs::read_to_string(out.join("highlights.cfg")).unwrap();
        assert!(cfg.contains(&format!("echo CS2C {} SCRIPT", p.nonce)), "{cfg}");
        let _ = std::fs::remove_dir_all(out);
    }
}
