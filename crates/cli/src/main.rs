use std::{env, error::Error, path::PathBuf, process::ExitCode};

use director::{NoPositions, Order, PositionSource};

const USAGE: &str = "\
cs2-cli — хайлайт-ролик из матча FACEIT

Использование:
  cs2-cli <ссылка на матч FACEIT | путь к .dem/.dem.zst> [параметры]

Параметры:
  --out <папка>     куда писать результат            (по умолчанию ./out/<id матча>)
  --demo <N>        номер демки в bo3, с нуля        (по умолчанию 0)
  --length <сек>    целевая длина ролика             (по умолчанию 60)
  --max-shots <N>   максимум моментов                (по умолчанию 12)
  --chronological   показывать по порядку матча, а не «от слабого к лучшему»
  --fps <N>         fps записи                       (по умолчанию 60)
  --list            только показать найденные хайлайты
  --no-flybys       не разбирать позиции игроков (быстрее, без свободных камер)
  --no-walls        не учитывать стены карты (не скачивать геометрию)
  --refresh-maps    принудительно сверить геометрию карт с источником
  --offline         не ходить в сеть за геометрией карт
  --maps <папка>    где лежат меши карт (по умолчанию ./maps или $CS2CINEMA_MAPS)
  --list-cameras    каталог камер и их красочность на тестовой сцене

Ключ FACEIT берётся из переменной FACEIT_API_KEY.";

struct Args {
    input: String,
    out: Option<PathBuf>,
    demo_index: usize,
    length: f32,
    max_shots: usize,
    order: Order,
    fps: u32,
    list_only: bool,
    list_cameras: bool,
    flybys: bool,
    walls: bool,
    maps: Option<PathBuf>,
    refresh_maps: bool,
    offline: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        input: String::new(),
        out: None,
        demo_index: 0,
        length: 60.0,
        max_shots: 12,
        order: Order::BuildUp,
        fps: 60,
        list_only: false,
        list_cameras: false,
        flybys: true,
        walls: true,
        maps: None,
        refresh_maps: false,
        offline: false,
    };
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} требует значение"));
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--out" => args.out = Some(value("--out")?.into()),
            "--demo" => args.demo_index = value("--demo")?.parse().map_err(|_| "--demo: нужно число")?,
            "--length" => args.length = value("--length")?.parse().map_err(|_| "--length: нужно число")?,
            "--max-shots" => args.max_shots = value("--max-shots")?.parse().map_err(|_| "--max-shots: нужно число")?,
            "--fps" => args.fps = value("--fps")?.parse().map_err(|_| "--fps: нужно число")?,
            "--chronological" => args.order = Order::Chronological,
            "--list" => args.list_only = true,
            "--no-flybys" => args.flybys = false,
            "--no-walls" => args.walls = false,
            "--refresh-maps" => args.refresh_maps = true,
            "--offline" => args.offline = true,
            "--maps" => args.maps = Some(value("--maps")?.into()),
            "--list-cameras" => args.list_cameras = true,
            s if s.starts_with("--") => return Err(format!("неизвестный параметр {s}")),
            s if args.input.is_empty() => args.input = s.to_owned(),
            s => return Err(format!("лишний аргумент {s}")),
        }
    }
    if args.input.is_empty() && !args.list_cameras {
        return Err(String::new());
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("ошибка: {msg}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ошибка: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
    if args.list_cameras {
        return list_cameras();
    }
    let (demo_path, label) = match faceit::parse_match_id(&args.input) {
        Some(id) if !PathBuf::from(&args.input).exists() => {
            let client = faceit::Client::from_env()?;
            eprintln!("матч {id}: ищу демку…");
            let urls = client.demo_urls(&id)?;
            let url = urls
                .get(args.demo_index)
                .ok_or_else(|| format!("в матче {} демок, а запрошена #{}", urls.len(), args.demo_index))?;
            eprintln!("качаю {url}");
            let path = client.download(url, &PathBuf::from("demos"))?;
            (path, id)
        }
        _ => {
            let path = PathBuf::from(&args.input);
            let label = path.file_stem().and_then(|s| s.to_str()).unwrap_or("match").to_owned();
            (path, label)
        }
    };

    eprintln!("разбираю {}…", demo_path.display());
    let data = demo::read_demo(&demo_path)?;
    let parsed = demo::parse(&data)?;
    let m = cs2::Match::from_parsed(&parsed);
    eprintln!(
        "карта {}, {:.0} tick/s, игроков {}, раундов {}, убийств {}",
        m.map, m.tick_rate, m.players.len(), m.rounds.len(), m.kills.len()
    );
    if m.kills.is_empty() {
        return Err("убийств не найдено: демка пустая или не распознана".into());
    }

    let found = highlights::find(&m, &highlights::Config::default());
    println!("\nХайлайты ({}):", found.len());
    for h in found.iter().take(20) {
        println!("  {:>6.1}  R{:<2} {:<20} {}", h.score, h.round, m.name(h.player), h.tags.join(", "));
    }
    if args.list_only {
        return Ok(());
    }

    let cfg = director::Config { target_secs: args.length, max_shots: args.max_shots, order: args.order, ..Default::default() };
    let (track, mesh);
    let walled;
    let pos: &dyn PositionSource = if args.flybys {
        eprintln!("снимаю позиции игроков…");
        track = positions::track(&data, 4)?;
        let dir = args.maps.clone().unwrap_or_else(geometry::default_dir);
        let loaded = if args.walls {
            eprintln!("геометрия карты {} ({})…", m.map, dir.display());
            let opts = geometry::Options { refresh: args.refresh_maps, offline: args.offline, ..Default::default() };
            match geometry::load_map(&dir, &m.map, &opts) {
                Ok(l) => {
                    l.notes.iter().for_each(|n| eprintln!("  {n}"));
                    Some(l.mesh)
                }
                Err(e) => {
                    eprintln!("предупреждение: без стен — {e}");
                    None
                }
            }
        } else {
            None
        };
        match loaded {
            Some(loaded) => {
                mesh = loaded;
                walled = positions::WithGeometry { track: &track, mesh: &mesh };
                &walled
            }
            None => &track,
        }
    } else {
        &NoPositions
    };
    let timeline = director::direct(&m, &found, &cfg, pos);
    let out_dir = args.out.unwrap_or_else(|| PathBuf::from("out").join(&label));
    let out = renderer::render(&m, &timeline, &renderer::Config { fps: args.fps, ..Default::default() });
    out.write_to(&out_dir)?;

    println!("\nМонтаж: {} шотов, ~{:.0} c", timeline.shots.len(), timeline.total_secs());
    for (i, s) in timeline.shots.iter().enumerate() {
        println!("  {:>2}. {:?} {:>6.1}  {}", i + 1, s.tier, s.score, s.title);
        let cams: Vec<String> = s
            .segments
            .iter()
            .filter_map(|g| Some(format!("{} ({:.2})", g.style.as_deref()?, g.colorfulness?)))
            .collect();
        if !cams.is_empty() {
            println!("      камеры: {}", cams.join(" → "));
        }
    }
    println!("\nФайлы записаны в {}", out_dir.display());
    Ok(())
}

/// Прогоняет весь каталог камер по встроенной сцене и печатает разбор красочности.
fn list_cameras() -> Result<(), Box<dyn Error>> {
    use director::{Tier, rig};

    let scene = rig::SyntheticDuel::scene(64.0);
    let picks = rig::rank(&rig::SyntheticDuel, &scene, 60, 140, rig::ANY, Tier::Epic, &[]);
    println!("{} камер; оценка на тестовой дуэли (Epic):\n", rig::PRESETS.len());
    println!("{:<20} {:<7} {:>3}  {:>5} {:>5} {:>5} {:>5} {:>5}  {:>5}", "камера", "тип", "dr", "килл", "люди", "кадр", "плавн", "драма", "итог");
    for p in picks {
        let b = p.score;
        println!(
            "{:<20} {:<7} {:>3}  {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2}  {:>5.2}",
            p.preset.name, format!("{:?}", p.preset.category), p.preset.drama,
            b.kill_visible, b.crowd, b.framing, b.smooth, b.drama_fit, b.total
        );
    }
    Ok(())
}
