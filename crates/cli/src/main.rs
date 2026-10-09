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
  --player <ник>    ролик про одного игрока: все его убийства по порядку (с пролётами)
  --cinematic       пролёт-интро, смена ракурса и замедление на каждом убийстве
  --fps <N>         fps записи                       (по умолчанию 60)
  --size <ШxВ>      размер кадра ролика = размер окна игры при записи (по умолчанию 1280x720)
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
    player: Option<String>,
    cinematic: bool,
    fps: u32,
    size: (u32, u32),
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
        player: None,
        cinematic: false,
        fps: 60,
        size: (1280, 720),
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
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size: нужно ШxВ, например 1920x1080")?;
                args.size = (w.parse().map_err(|_| "--size: нужно число")?, h.parse().map_err(|_| "--size: нужно число")?);
            }
            "--chronological" => args.order = Order::Chronological,
            "--player" => args.player = Some(value("--player")?),
            "--cinematic" => args.cinematic = true,
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

    let hcfg = highlights::Config::default();
    let hero = match &args.player {
        Some(name) => {
            let want = name.to_lowercase();
            let p = m
                .players
                .iter()
                .find(|p| p.name.to_lowercase() == want)
                .or_else(|| m.players.iter().find(|p| p.name.to_lowercase().contains(&want)))
                .ok_or_else(|| {
                    let names: Vec<&str> = m.players.iter().map(|p| p.name.as_str()).collect();
                    format!("игрок «{name}» не найден; в матче: {}", names.join(", "))
                })?;
            Some(p.id)
        }
        None => None,
    };
    let found = match hero {
        Some(id) => highlights::for_player(&m, id, &hcfg),
        None => highlights::find(&m, &hcfg),
    };
    let utility = hero.map(|id| highlights::utility_for(&m, id)).unwrap_or_default();
    if let Some(id) = hero {
        let kills = m.kills.iter().filter(|k| k.attacker == id).count();
        let thrown = m.grenades.iter().filter(|g| g.thrower == id).count();
        println!("\n{}: убийств {kills} (моментов {}), гранат {thrown}, важных {}", m.name(id), found.len(), utility.len());
        for u in &utility {
            println!("  {:>6.1}  R{:<2} {}", u.score, u.grenade.round, u.tags.join(", "));
        }
    }
    println!("\nХайлайты ({}):", found.len());
    for h in found.iter().take(20) {
        println!("  {:>6.1}  R{:<2} {:<20} {}", h.score, h.round, m.name(h.player), h.tags.join(", "));
    }
    if args.list_only {
        return Ok(());
    }

    let cfg = director::Config {
        target_secs: args.length,
        max_shots: args.max_shots,
        order: args.order,
        cinematic: args.cinematic || hero.is_some(),
        ..Default::default()
    };
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
    let timeline = match hero {
        Some(_) => director::direct_all(&m, &found, &utility, &cfg, pos),
        None => director::direct(&m, &found, &cfg, pos),
    };
    let out_dir = args.out.unwrap_or_else(|| PathBuf::from("out").join(&label));
    std::fs::create_dir_all(&out_dir)?;
    // HLAE пишет относительные пути от папки exe игры — в скрипт идут только абсолютные
    let out_dir = std::path::absolute(&out_dir)?;
    let title = hero.map(|id| {
        let kills = m.kills.iter().filter(|k| k.attacker == id).count();
        (m.name(id), format!("{} · {}", m.map, director::plural(kills, "убийство", "убийства", "убийств")))
    });
    let rcfg = renderer::Config {
        fps: args.fps,
        size: args.size,
        font_file: font_file(),
        title,
        out_dir: slashes(&out_dir),
        ..Default::default()
    };
    let out = renderer::render(&m, &timeline, &rcfg);
    // скрипты и пути камер прошлого плана (их могло быть больше) — убираем, чтобы не путали
    for entry in std::fs::read_dir(&out_dir)?.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("cs2c_") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    out.write_to(&out_dir)?;
    // старые дубли сбили бы нумерацию takeNNNN — откладываем в сторону (не удаляем: запись дорогая)
    let clips = out_dir.join("clips");
    if clips.is_dir() {
        let aside = out_dir.join(format!("clips_old_{}", std::time::UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0)));
        std::fs::rename(&clips, &aside)?;
        eprintln!("прошлые дубли отложены в {} (если план не менялся, верните их в clips)", aside.display());
    }

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

    // highlights.cfg и скрипты шотов cs2c_NN.cfg — в game/csgo/cfg
    let installed = cs2_csgo_dir().and_then(|dir| {
        let cfg_dir = dir.join("cfg");
        // и пути камеры cs2c_NN_K.xml: HLAE грузит их относительным путём от game/bin/win64
        let ours = |n: &str| n.ends_with(".cfg") || (n.starts_with("cs2c_") && n.ends_with(".xml"));
        for (name, body) in out.files.iter().filter(|(n, _)| ours(n)) {
            std::fs::write(cfg_dir.join(name), body).ok()?;
        }
        Some(cfg_dir.join("highlights.cfg"))
    });
    // CS2 играет только несжатые .dem — сжатую (.dem.zst/.gz) сохраняем рядом распакованной
    let playable = if demo_path.extension().is_some_and(|e| e == "dem") {
        demo_path.clone()
    } else {
        let name = demo_path.file_name().and_then(|n| n.to_str()).unwrap_or("match");
        let stem = name.split(".dem").next().unwrap_or(name);
        let dem = demo_path.with_file_name(format!("{stem}.dem"));
        if !dem.exists() {
            std::fs::write(&dem, &data)?;
        }
        dem
    };
    let demo_abs = std::path::absolute(&playable).unwrap_or(playable);
    println!("\nЗапись (CS2 запущена через HLAE):");
    match &installed {
        Some(p) => println!("  скрипт скопирован в {}", p.display()),
        None => println!("  скопируйте highlights.cfg и cs2c_*.cfg, cs2c_*.xml в game\\csgo\\cfg (папку CS2 не нашёл; можно указать $CS2_DIR)"),
    }
    println!("  в консоли CS2:  demo_ui_mode 0; playdemo \"{}\"", slashes(&demo_abs));
    println!("  когда демка загрузится:  exec highlights");
    println!("  после записи:  {}", out_dir.join("assemble.bat").display());
    Ok(())
}

/// Шрифт для подписей в ролике: `$CS2CINEMA_FONT` или стандартный Arial Bold из Windows.
fn font_file() -> Option<String> {
    let candidates = [
        env::var("CS2CINEMA_FONT").ok(),
        Some(r"C:\Windows\Fonts\arialbd.ttf".to_owned()),
        Some(r"C:\Windows\Fonts\arial.ttf".to_owned()),
        Some("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".to_owned()),
    ];
    candidates.into_iter().flatten().find(|p| PathBuf::from(p).is_file()).map(|p| p.replace('\\', "/"))
}

/// Путь для консоли CS2: прямые слэши (обратные она не любит), без префикса `\\?\`.
fn slashes(p: &std::path::Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s)
}

/// `game/csgo` установленной CS2: `$CS2_DIR` или библиотеки Steam из `libraryfolders.vdf`.
fn cs2_csgo_dir() -> Option<PathBuf> {
    let csgo = |root: PathBuf| Some(root.join("game").join("csgo")).filter(|p| p.join("cfg").is_dir());
    if let Some(dir) = env::var_os("CS2_DIR") {
        return csgo(dir.into());
    }
    let steams = [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"];
    for steam in steams {
        let Ok(vdf) = std::fs::read_to_string(PathBuf::from(steam).join("steamapps").join("libraryfolders.vdf")) else {
            continue;
        };
        let libraries = vdf.lines().filter_map(|l| {
            let mut parts = l.split('"').filter(|s| !s.trim().is_empty());
            (parts.next()? == "path").then(|| parts.next().map(|p| p.replace(r"\\", r"\")))?
        });
        for lib in std::iter::once(steam.to_owned()).chain(libraries) {
            let root = PathBuf::from(lib).join("steamapps").join("common").join("Counter-Strike Global Offensive");
            if let Some(dir) = csgo(root) {
                return Some(dir);
            }
        }
    }
    None
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
