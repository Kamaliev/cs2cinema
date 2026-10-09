//! Весь конвейер «демка → монтажный план → файлы для HLAE» одной библиотекой,
//! чтобы им пользовались и окно, и консоль. Каждый тяжёлый шаг сообщает о ходе работы через `progress`.

use std::path::{Path, PathBuf};

use cs2::Match;
use director::{Order, PositionSource, Timeline};
use highlights::Highlight;

pub type Result<T> = std::result::Result<T, String>;

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

pub fn analyze(src: &Source, progress: &dyn Fn(&str)) -> Result<Analysis> {
    let (demo_path, label) = match src {
        Source::File(p) => {
            let label = p.file_stem().and_then(|s| s.to_str()).unwrap_or("match");
            let label = label.strip_suffix(".dem").unwrap_or(label).to_owned();
            (p.clone(), label)
        }
        Source::Faceit { link, key, demo_index, dir } => {
            let id = faceit::parse_match_id(link).ok_or("не похоже на ссылку матча FACEIT")?;
            let client = if key.trim().is_empty() { faceit::Client::from_env().map_err(|e| e.to_string())? } else { faceit::Client::new(key.trim()) };
            progress(&format!("матч {id}: ищу демку…"));
            let urls = client.demo_urls(&id).map_err(|e| e.to_string())?;
            let url = urls
                .get(*demo_index)
                .ok_or_else(|| format!("в матче {} демок, а запрошена #{demo_index}", urls.len()))?;
            progress("качаю демку…");
            let path = client.download(url, dir).map_err(|e| e.to_string())?;
            (path, id)
        }
    };
    progress("разбираю демку…");
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
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { fps: 60, chronological: false, flybys: true, walls: true, refresh_maps: false, offline: false, maps_dir: None }
    }
}

pub struct Plan {
    pub timeline: Timeline,
    pub out_dir: PathBuf,
}

/// Строит план по выбранным вручную моментам (индексы в `a.found`) и пишет файлы в `out_dir`.
pub fn plan(a: &mut Analysis, selected: &[usize], o: &PlanOptions, out_dir: &Path, progress: &dyn Fn(&str)) -> Result<Plan> {
    let chosen: Vec<Highlight> = selected.iter().filter_map(|&i| a.found.get(i).cloned()).collect();
    if chosen.is_empty() {
        return Err("не выбрано ни одного момента".into());
    }
    let cfg = director::Config {
        target_secs: f32::MAX,
        max_shots: chosen.len(),
        order: if o.chronological { Order::Chronological } else { Order::BuildUp },
        ..Default::default()
    };

    let mesh = if o.flybys {
        if a.track.is_none() {
            progress("снимаю позиции игроков…");
            a.track = Some(positions::track(&a.data, 4).map_err(|e| e.to_string())?);
        }
        if o.walls {
            let dir = o.maps_dir.clone().unwrap_or_else(geometry::default_dir);
            progress(&format!("геометрия карты {}…", a.m.map));
            let opts = geometry::Options { refresh: o.refresh_maps, offline: o.offline, ..Default::default() };
            match geometry::load_map(&dir, &a.m.map, &opts) {
                Ok(l) => Some(l.mesh),
                Err(e) => {
                    progress(&format!("предупреждение: без стен — {e}"));
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    progress("строю монтаж…");
    let timeline = match (&a.track, &mesh) {
        (Some(t), Some(mesh)) => {
            let p = positions::WithGeometry { track: t, mesh };
            director::direct(&a.m, &chosen, &cfg, &p as &dyn PositionSource)
        }
        (Some(t), None) => director::direct(&a.m, &chosen, &cfg, t as &dyn PositionSource),
        _ => director::direct(&a.m, &chosen, &cfg, &director::NoPositions),
    };

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    // HLAE ищет campath относительно game\bin\win64, поэтому в скрипте нужен абсолютный путь
    let abs = absolute_slash(out_dir);
    let out = renderer::render(&a.m, &timeline, &renderer::Config { fps: o.fps, campath_dir: Some(abs), ..Default::default() });
    out.write_to(out_dir).map_err(|e| format!("запись в {}: {e}", out_dir.display()))?;
    Ok(Plan { timeline, out_dir: out_dir.to_path_buf() })
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

    /// Демка из репозитория: разбор → ручной выбор → план и файлы.
    #[test]
    fn analyze_select_plan() {
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../1-3e7db9e3-8e92-4761-a1db-3729fb7de11c-1-1.dem.zst");
        if !demo.exists() {
            return;
        }
        let mut a = analyze(&Source::File(demo), &|_| {}).unwrap();
        assert!(!a.found.is_empty());
        let out = std::env::temp_dir().join(format!("cs2cinema-pipeline-{}", std::process::id()));
        let o = PlanOptions { walls: false, ..Default::default() };
        let p = plan(&mut a, &[0, 1], &o, &out, &|_| {}).unwrap();
        assert_eq!(p.timeline.shots.len(), 2);
        assert!(out.join("highlights.cfg").is_file() && out.join("plan.json").is_file());
        assert!(plan(&mut a, &[], &o, &out, &|_| {}).is_err());
        let _ = std::fs::remove_dir_all(out);
    }
}
