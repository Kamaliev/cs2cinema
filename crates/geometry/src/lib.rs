//! Геометрия карты (стены, полы) для проверки видимости и столкновений камеры.
//!
//! В демке геометрии нет — она лежит в файлах игры. Берём готовые коллизионные меши сообщества
//! (проект awpy-data, формат `AWMH`: вершины и индексы треугольников в мировых координатах демки)
//! и строим по ним BVH для трассировки лучей. Сами меши — собственность Valve, поэтому они не
//! хранятся в репозитории, а скачиваются при первом запуске (`fetch`).

use std::{
    error::Error,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

pub type Vec3 = [f32; 3];

const MAGIC: &[u8; 4] = b"AWMH";
const LEAF_SIZE: usize = 4;

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Лист: индекс первого треугольника в `order`; узел: индекс левого потомка (правый = +1).
    first: u32,
    /// Лист: число треугольников (> 0); узел: 0.
    count: u32,
}

pub struct Mesh {
    tris: Vec<[Vec3; 3]>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mesh({} triangles)", self.tris.len())
    }
}

impl Mesh {
    pub fn from_bytes(data: &[u8]) -> Result<Self, Box<dyn Error>> {
        if data.len() < 16 || &data[..4] != MAGIC {
            return Err("это не .mesh (AWMH)".into());
        }
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        if u32_at(4) != 1 {
            return Err(format!("неизвестная версия .mesh: {}", u32_at(4)).into());
        }
        let (nv, nt) = (u32_at(8) as usize, u32_at(12) as usize);
        if data.len() != 16 + nv * 12 + nt * 12 {
            return Err("размер .mesh не сходится с заголовком".into());
        }
        let verts: Vec<Vec3> = (0..nv).map(|i| {
            let o = 16 + i * 12;
            [f32_at(o), f32_at(o + 4), f32_at(o + 8)]
        }).collect();
        let base = 16 + nv * 12;
        let mut tris = Vec::with_capacity(nt);
        for i in 0..nt {
            let idx = |k: usize| u32_at(base + i * 12 + k * 4) as usize;
            let (a, b, c) = (idx(0), idx(1), idx(2));
            if a >= nv || b >= nv || c >= nv {
                return Err("индекс вершины вне диапазона".into());
            }
            tris.push([verts[a], verts[b], verts[c]]);
        }
        Ok(Self::from_triangles(tris))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        Self::from_bytes(&fs::read(path)?)
    }

    pub fn from_triangles(tris: Vec<[Vec3; 3]>) -> Self {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len() / 2 + 1);
        if !tris.is_empty() {
            nodes.push(Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 });
            build(&tris, &mut order, &mut nodes, 0, 0, tris.len());
        }
        Self { tris, nodes, order }
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Доля отрезка `from -> to` до первого пересечения (0..1], `None` — путь свободен.
    pub fn raycast(&self, from: Vec3, to: Vec3) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        let dir = sub(to, from);
        let inv = [1.0 / dir[0], 1.0 / dir[1], 1.0 / dir[2]];
        let mut best = f32::INFINITY;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if !slab(n, from, inv, best.min(1.0)) {
                continue;
            }
            if n.count > 0 {
                for &t in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    if let Some(h) = hit(&self.tris[t as usize], from, dir) {
                        best = best.min(h);
                    }
                }
            } else {
                stack[sp] = n.first;
                stack[sp + 1] = n.first + 1;
                sp += 2;
            }
        }
        (best <= 1.0).then_some(best)
    }

    pub fn segment_clear(&self, a: Vec3, b: Vec3) -> bool {
        self.raycast(a, b).is_none()
    }
}

fn centroid(t: &[Vec3; 3], axis: usize) -> f32 {
    (t[0][axis] + t[1][axis] + t[2][axis]) / 3.0
}

fn build(tris: &[[Vec3; 3]], order: &mut [u32], nodes: &mut Vec<Node>, at: usize, start: usize, end: usize) {
    let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for &i in &order[start..end] {
        for v in &tris[i as usize] {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
    }
    let n = end - start;
    if n <= LEAF_SIZE {
        nodes[at] = Node { min, max, first: start as u32, count: n as u32 };
        return;
    }
    let ext = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 } else if ext[1] >= ext[2] { 1 } else { 2 };
    let mid = n / 2;
    order[start..end].select_nth_unstable_by(mid, |&a, &b| {
        centroid(&tris[a as usize], axis).total_cmp(&centroid(&tris[b as usize], axis))
    });
    let left = nodes.len();
    nodes.push(Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 });
    nodes.push(Node { min: [0.0; 3], max: [0.0; 3], first: 0, count: 0 });
    nodes[at] = Node { min, max, first: left as u32, count: 0 };
    build(tris, order, nodes, left, start, start + mid);
    build(tris, order, nodes, left + 1, start + mid, end);
}

fn slab(n: &Node, o: Vec3, inv: Vec3, tmax: f32) -> bool {
    let (mut t0, mut t1) = (0.0f32, tmax);
    for k in 0..3 {
        let a = (n.min[k] - o[k]) * inv[k];
        let b = (n.max[k] - o[k]) * inv[k];
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        // NaN (0 * inf) сравнения не проходят — пропускаем ось
        if lo > t0 { t0 = lo; }
        if hi < t1 { t1 = hi; }
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Möller–Trumbore, двусторонний; параметр вдоль `dir` в (0, 1].
fn hit(t: &[Vec3; 3], o: Vec3, dir: Vec3) -> Option<f32> {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let p = cross(dir, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, t[0]);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let d = dot(e2, q) * inv;
    (d > 1e-6 && d <= 1.0).then_some(d)
}

/// Где лежат меши по умолчанию: `$CS2CINEMA_MAPS`, иначе `./maps`.
pub fn default_dir() -> PathBuf {
    std::env::var_os("CS2CINEMA_MAPS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("maps"))
}

const DATA_REPO: &str = "https://github.com/pnxenopoulos/awpy-data";
const STAMP: &str = "maps.json";

/// Откуда взяты меши в папке и какой версии игры они соответствуют.
/// Файл `maps.json` пишут и эта библиотека, и `scripts/update_maps.py`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp {
    /// `"release"` — скачано из awpy-data; `"game"` — извлечено из локальной установки CS2.
    pub source: String,
    /// ClientVersion игры (он же тег релиза awpy-data).
    pub version: String,
    /// Когда последний раз сверялись с источником, unix-секунды.
    pub checked_at: u64,
    /// Для `game`: откуда извлекали.
    pub cs2_dir: Option<String>,
}

impl Stamp {
    pub fn read(dir: &Path) -> Option<Stamp> {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(STAMP)).ok()?).ok()?;
        Some(Stamp {
            source: v["source"].as_str()?.to_owned(),
            version: v["version"].as_str()?.to_owned(),
            checked_at: v["checked_at"].as_u64().unwrap_or(0),
            cs2_dir: v["cs2_dir"].as_str().map(str::to_owned),
        })
    }

    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let v = serde_json::json!({
            "source": self.source, "version": self.version,
            "checked_at": self.checked_at, "cs2_dir": self.cs2_dir,
        });
        fs::write(dir.join(STAMP), serde_json::to_vec_pretty(&v).unwrap())
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Сверяться с источником, даже если проверяли недавно.
    pub refresh: bool,
    /// Не ходить в сеть вообще.
    pub offline: bool,
    /// Как часто проверять наличие нового релиза.
    pub max_age_secs: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { refresh: false, offline: false, max_age_secs: 24 * 3600 }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Нужно ли идти за списком релизов.
fn needs_check(stamp: Option<&Stamp>, mesh_exists: bool, now: u64, o: &Options) -> bool {
    if o.offline {
        return false;
    }
    match stamp {
        _ if o.refresh || !mesh_exists => true,
        None => false, // меши положили вручную — не трогаем
        Some(s) if s.source == "game" => false, // своё извлечение из игры сильнее релиза
        Some(s) => now.saturating_sub(s.checked_at) >= o.max_age_secs,
    }
}

/// Версия игры из `<cs2>/game/csgo/steam.inf` (`ClientVersion=...`).
pub fn game_version(cs2_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(cs2_dir.join("game/csgo/steam.inf")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("ClientVersion=")).map(|v| v.trim().to_owned())
}

/// Самый свежий релиз awpy-data: максимальный числовой тег (= ClientVersion игры).
pub fn latest_release_tag() -> Result<String, Box<dyn Error>> {
    let out = std::process::Command::new("git")
        .args(["ls-remote", "--tags", "--refs", DATA_REPO])
        .output()
        .map_err(|e| format!("не удалось запустить git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-remote: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    newest_tag(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "в awpy-data нет тегов".into())
}

fn newest_tag(ls_remote: &str) -> Option<String> {
    ls_remote
        .lines()
        .filter_map(|l| l.rsplit_once("refs/tags/")?.1.trim().parse::<u64>().ok())
        .max()
        .map(|v| v.to_string())
}

#[derive(Debug)]
pub struct Loaded {
    pub mesh: Mesh,
    /// Что стоит сообщить пользователю (откуда меши, устарели ли и т. п.).
    pub notes: Vec<String>,
}

/// Меш карты. Свежесть:
/// * извлечено из игры (`scripts/update_maps.py`) — используется как есть; если установленная игра
///   с тех пор обновилась, возвращается подсказка перезапустить скрипт;
/// * скачано из релиза — раз в `max_age_secs` проверяется, не вышел ли новый релиз (тег = версия игры),
///   и при необходимости скачивается заново;
/// * положено вручную без `maps.json` — не трогается.
pub fn load_map(dir: &Path, map: &str, opts: &Options) -> Result<Loaded, Box<dyn Error>> {
    let path = dir.join(format!("{map}.mesh"));
    let mut notes = Vec::new();
    let mut stamp = Stamp::read(dir);

    if needs_check(stamp.as_ref(), path.exists(), now(), opts) {
        let tag = latest_release_tag().and_then(|tag| {
            let current = stamp.as_ref().filter(|s| s.source == "release").map(|s| s.version.as_str());
            if opts.refresh || current != Some(tag.as_str()) || !path.exists() {
                notes.push(format!("скачиваю геометрию карт, релиз {tag}"));
                fetch(dir, &tag)?;
            }
            Ok(tag)
        });
        match tag {
            Ok(tag) => {
                let new = Stamp { source: "release".into(), version: tag, checked_at: now(), cs2_dir: None };
                new.write(dir)?;
                stamp = Some(new);
            }
            Err(e) if path.exists() => notes.push(format!("не удалось проверить обновления карт ({e}), использую сохранённые")),
            Err(e) => return Err(e),
        }
    }

    if let Some(s) = &stamp {
        if s.source == "game" {
            let installed = s.cs2_dir.as_deref().and_then(|d| game_version(Path::new(d)));
            match installed {
                Some(v) if v != s.version => notes.push(format!(
                    "игра обновилась ({} → {v}), а геометрия извлечена из старой версии: запустите scripts/update_maps.py",
                    s.version
                )),
                _ => notes.push(format!("геометрия из файлов игры, версия {}", s.version)),
            }
        } else {
            notes.push(format!("геометрия из релиза awpy-data {}", s.version));
        }
    }

    if !path.exists() {
        return Err(format!("в {} нет карты {map}", dir.display()).into());
    }
    Ok(Loaded { mesh: Mesh::load(path)?, notes })
}

/// Скачивает меши релиза `tag` в `dir` (плоские `*.mesh`, без путей из архива).
pub fn fetch(dir: &Path, tag: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(dir)?;
    let url = format!("{DATA_REPO}/releases/download/{tag}/geometry.zip");
    // системное хранилище сертификатов: работает и за корпоративными прокси
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .build()
        .into();
    let mut resp = agent.get(&url).call()?;
    let bytes = resp.body_mut().with_config().limit(512 * 1024 * 1024).read_to_vec()?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let Some(name) = Path::new(f.name()).file_name().map(|n| n.to_owned()) else { continue };
        if Path::new(&name).extension().is_some_and(|e| e == "mesh") {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            fs::write(dir.join(name), buf)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> Mesh {
        // квадрат 100x100 в плоскости x=50
        let a = [50.0, -50.0, -50.0];
        let b = [50.0, 50.0, -50.0];
        let c = [50.0, 50.0, 50.0];
        let d = [50.0, -50.0, 50.0];
        Mesh::from_triangles(vec![[a, b, c], [a, c, d]])
    }

    #[test]
    fn ray_hits_wall_and_reports_fraction() {
        let m = wall();
        let h = m.raycast([0.0, 0.0, 0.0], [100.0, 0.0, 0.0]).unwrap();
        assert!((h - 0.5).abs() < 1e-5);
        assert!(!m.segment_clear([0.0, 0.0, 0.0], [100.0, 0.0, 0.0]));
        // не доходим до стены, идём мимо, идём параллельно
        assert!(m.segment_clear([0.0, 0.0, 0.0], [40.0, 0.0, 0.0]));
        assert!(m.segment_clear([0.0, 200.0, 0.0], [100.0, 200.0, 0.0]));
        assert!(m.segment_clear([0.0, 0.0, 0.0], [0.0, 0.0, 100.0]));
    }

    #[test]
    fn bvh_agrees_with_brute_force_on_random_scene() {
        // детерминированный ГПСЧ
        let mut s = 12345u64;
        let mut r = move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f32 / (1u64 << 31) as f32) * 1000.0
        };
        let tris: Vec<[Vec3; 3]> = (0..2000)
            .map(|_| {
                let o = [r(), r(), r()];
                [o, [o[0] + r() * 0.05, o[1] + r() * 0.05, o[2]], [o[0], o[1] + r() * 0.05, o[2] + r() * 0.05]]
            })
            .collect();
        let m = Mesh::from_triangles(tris.clone());
        let mut agree = 0;
        for _ in 0..300 {
            let (a, b) = ([r(), r(), r()], [r(), r(), r()]);
            let dir = sub(b, a);
            let brute = tris.iter().filter_map(|t| hit(t, a, dir)).fold(f32::INFINITY, f32::min);
            let fast = m.raycast(a, b).unwrap_or(f32::INFINITY);
            assert!((brute - fast).abs() < 1e-4 || (brute.is_infinite() && fast.is_infinite()), "{brute} vs {fast}");
            agree += 1;
        }
        assert_eq!(agree, 300);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Mesh::from_bytes(b"nope").is_err());
        let mut bad = b"AWMH".to_vec();
        bad.extend_from_slice(&1u32.to_le_bytes());
        bad.extend_from_slice(&5u32.to_le_bytes());
        bad.extend_from_slice(&0u32.to_le_bytes());
        assert!(Mesh::from_bytes(&bad).is_err());
    }

    fn stamp(source: &str, version: &str, checked_at: u64) -> Stamp {
        Stamp { source: source.into(), version: version.into(), checked_at, cs2_dir: None }
    }

    #[test]
    fn freshness_policy() {
        let o = Options::default();
        let day = 24 * 3600;
        let rel = stamp("release", "2000927", 1_000_000);
        // свежая проверка — не ходим в сеть
        assert!(!needs_check(Some(&rel), true, 1_000_000 + day - 1, &o));
        // сутки прошли — проверяем
        assert!(needs_check(Some(&rel), true, 1_000_000 + day, &o));
        // нет файла карты — качаем
        assert!(needs_check(Some(&rel), false, 1_000_001, &o));
        // принудительно
        assert!(needs_check(Some(&rel), true, 1_000_001, &Options { refresh: true, ..o.clone() }));
        // офлайн — никогда
        assert!(!needs_check(None, false, 0, &Options { offline: true, ..o.clone() }));
        // своё извлечение из игры и ручные меши релизом не перезаписываются
        assert!(!needs_check(Some(&stamp("game", "2000900", 0)), true, 99 * day, &o));
        assert!(!needs_check(None, true, 99 * day, &o));
    }

    #[test]
    fn newest_tag_is_numeric_max() {
        let out = "aaa\trefs/tags/2000905\nbbb\trefs/tags/2000927\nccc\trefs/tags/v1-draft\nddd\trefs/tags/999999\n";
        assert_eq!(newest_tag(out).as_deref(), Some("2000927"));
        assert_eq!(newest_tag("nothing"), None);
    }

    #[test]
    fn stamp_roundtrip_and_game_version() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-stamp-{}", std::process::id()));
        fs::create_dir_all(dir.join("game/csgo")).unwrap();
        let s = Stamp { source: "game".into(), version: "2000927".into(), checked_at: 42, cs2_dir: Some("/x".into()) };
        s.write(&dir).unwrap();
        assert_eq!(Stamp::read(&dir), Some(s));
        fs::write(dir.join("game/csgo/steam.inf"), "ClientVersion=2000999\nServerVersion=2000999\n").unwrap();
        assert_eq!(game_version(&dir).as_deref(), Some("2000999"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stale_game_extraction_is_reported_without_network() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-stale-{}", std::process::id()));
        let game = dir.join("cs2");
        fs::create_dir_all(game.join("game/csgo")).unwrap();
        fs::write(game.join("game/csgo/steam.inf"), "ClientVersion=2000999\n").unwrap();
        // минимальный меш: один треугольник
        let mut mesh = b"AWMH".to_vec();
        for v in [1u32, 3, 1] { mesh.extend_from_slice(&v.to_le_bytes()); }
        for f in [0f32, 0., 0., 1., 0., 0., 0., 1., 0.] { mesh.extend_from_slice(&f.to_le_bytes()); }
        for i in [0u32, 1, 2] { mesh.extend_from_slice(&i.to_le_bytes()); }
        fs::write(dir.join("de_test.mesh"), mesh).unwrap();
        Stamp { source: "game".into(), version: "2000927".into(), checked_at: 0, cs2_dir: Some(game.to_string_lossy().into()) }
            .write(&dir).unwrap();

        let loaded = load_map(&dir, "de_test", &Options::default()).unwrap();
        assert_eq!(loaded.mesh.triangle_count(), 1);
        assert!(loaded.notes.iter().any(|n| n.contains("update_maps.py") && n.contains("2000999")), "{:?}", loaded.notes);
        fs::remove_dir_all(&dir).unwrap();
    }
}
