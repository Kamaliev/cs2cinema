//! Весь конвейер «демка → монтажный план → файлы для HLAE» одной библиотекой,
//! чтобы им пользовались и окно, и консоль. Каждый тяжёлый шаг сообщает о ходе работы через `progress`.

use std::path::{Path, PathBuf};

pub use host;
pub mod record;
pub use record::{Deadlines, Event, FailCode, RecordFailure, RecordReport, RecordRequest, RecordState, record};

use cs2::{Match, PlayerId};
use director::{Order, PositionSource, Timeline};
use highlights::Highlight;

pub type Result<T> = std::result::Result<T, String>;

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
    /// Типизированное событие записи; по умолчанию — строка журнала.
    fn event(&self, e: &record::Event) {
        self.log(&e.to_string())
    }
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

pub fn analyze(src: &Source, p: &dyn Progress) -> Result<Analysis> {
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
    let data = demo::read_demo(&demo_path).map_err(|e| format!("{}: {e}", demo_path.display()))?;
    let parsed = demo::parse(&data).map_err(|e| e.to_string())?;
    let m = Match::from_parsed(&parsed);
    if m.kills.is_empty() {
        return Err("убийств не найдено: демка пустая или не распознана".into());
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

/// Игрок демки по FACEIT: сначала по SteamID, затем по нику (точно, потом подстрокой без учёта регистра).
pub fn find_player(m: &Match, steam_id: Option<u64>, nick: &str) -> Option<PlayerId> {
    if let Some(p) = steam_id.filter(|&s| s != 0).and_then(|s| m.players.iter().find(|p| p.xuid == s)) {
        return Some(p.id);
    }
    let want = nick.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    m.players
        .iter()
        .find(|p| p.name.to_lowercase() == want)
        .or_else(|| m.players.iter().find(|p| p.name.to_lowercase().contains(&want)))
        .map(|p| p.id)
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
    /// Кладёт распакованную демку в `path` (CS2 читает `.dem` из `game/csgo`, архивы `.zst` не понимает).
    pub fn write_demo(&self, path: &Path) -> Result<()> {
        std::fs::write(path, &self.data).map_err(|e| format!("{}: {e}", path.display()))
    }
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
