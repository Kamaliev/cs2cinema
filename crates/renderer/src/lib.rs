//! Превращает монтажный план в файлы для записи ролика:
//! * `plan.json`       — сам план (стабильный формат, можно скармливать другим инструментам);
//! * `highlights.cfg`  — скрипт для CS2 под HLAE: настройки записи и запуск первого шота;
//! * `cs2c_NN.cfg`     — по скрипту на шот: камера, зум, запись дубля `clips/takeNNNN/video.mp4`,
//!                       в конце — переход к следующему шоту с перемоткой (в т.ч. назад);
//! * `campath_NN.xml`  — путь свободной камеры шота (если режиссёр его построил);
//! * `assemble.ffgraph` + `assemble.sh` / `assemble.bat` — ffmpeg-склейка: замедление,
//!                       порядок показа, кроссфейды.
//!
//! Что выяснено запуском в CS2 + HLAE 2.192 (октябрь 2026):
//! * `spec_player_by_accountid` в CS2 нет; `spec_player <слот + 1>`;
//!   `spec_mode`: 2 — от первого лица, 3 — chase, 4 — свободная камера;
//! * `mirv_cmd addAtTick` считает тики демки (как парсер), а campath — время игры:
//!   `(тик демки + server_start_tick) / tick_rate`; углы в XML: rx — крен, ry — тангаж, rz — рыскание;
//! * HLAE пишет через `startmovie`: каждый кадр двигает игру ровно на 1/fps, поэтому
//!   `host_timescale`/`demo_timescale` замедления не дают. Пишем с повышенным fps
//!   (`mirv_streams record fps`), а замедляем при склейке (`setpts`);
//! * консоль режет аргументы по `:`, а `mirv_cmd` теряет кавычки — абсолютные пути
//!   (папка записи, campath) задаются один раз в начале скрипта, в кавычках.
//!
//! Команды HLAE/CS2 собраны в `Dialect` и в `preamble` — правьте их здесь, а не в логике режиссёра.

pub mod assemble;

use std::{fmt::Write as _, io, path::Path};

use cs2::Match;
use director::{Camera, ScaleKey, Segment, Shot, Timeline};

/// Названия режимов наблюдателя и настройки записи.
#[derive(Debug, Clone)]
pub struct Dialect {
    pub spec_mode_pov: u8,
    pub spec_mode_chase: u8,
    /// Пресет `mirv_streams settings` для записи экрана.
    pub record_settings: String,
    /// Папка путей камеры относительно `game/bin/win64` (от неё HLAE считает относительные пути).
    pub campath_dir: String,
}

impl Default for Dialect {
    fn default() -> Self {
        Self {
            spec_mode_pov: 2,
            spec_mode_chase: 3,
            record_settings: "afxFfmpegYuv420p".into(),
            campath_dir: "../../csgo/cfg".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// fps готового ролика.
    pub fps: u32,
    /// Потолок fps записи: глубокое замедление при склейке дублирует кадры, а не тормозит запись.
    pub max_capture_fps: u32,
    /// Обычный FOV; зум режиссёра — множитель от него (`mirv_fov`).
    pub base_fov: f32,
    /// Размер кадра ролика (дубли HLAE пишутся в размере окна игры).
    pub size: (u32, u32),
    /// Шрифт для подписей и заставок (`C:/Windows/Fonts/arialbd.ttf`); `None` — без текста.
    pub font_file: Option<String>,
    /// Заставка в начале ролика: (заголовок, подзаголовок).
    pub title: Option<(String, String)>,
    /// Абсолютный путь к папке результата (через `/`): туда HLAE пишет `clips/`, оттуда грузится campath.
    pub out_dir: String,
    pub dialect: Dialect,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps: 60,
            max_capture_fps: 360,
            base_fov: 90.0,
            size: (1280, 720),
            font_file: None,
            title: None,
            out_dir: ".".into(),
            dialect: Dialect::default(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Output {
    /// `(имя файла, содержимое)`
    pub files: Vec<(String, String)>,
}

impl Output {
    pub fn write_to(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (name, body) in &self.files {
            std::fs::write(dir.join(name), body)?;
        }
        Ok(())
    }

    pub fn file(&self, name: &str) -> Option<&str> {
        self.files.iter().find(|(n, _)| n == name).map(|(_, b)| b.as_str())
    }
}

/// fps записи шота: чтобы на самом медленном его участке в ролике было `fps` настоящих кадров.
/// У каждого шота своё: игра с записью рендерит ~60 кадров/с, и лишние кадры там, где замедления нет,
/// растягивают запись в разы.
pub fn capture_fps(shot: &Shot, cfg: &Config) -> u32 {
    let slowest = shot.timescale.iter().map(|k| k.scale).fold(1.0f32, f32::min).max(0.05);
    ((cfg.fps as f32 / slowest).ceil() as u32).clamp(cfg.fps, cfg.max_capture_fps.max(cfg.fps))
}

/// Безопасно ли путь/имя вставлять в команду консоли внутри `mirv_cmd ... "..."` без кавычек.
pub fn console_safe(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || matches!(c, '"' | ';' | '\''))
}

pub fn render(m: &Match, t: &Timeline, cfg: &Config) -> Output {
    let mut out = Output::default();
    out.files.push(("plan.json".into(), serde_json::to_string_pretty(t).expect("план сериализуется")));

    // Шоты пишутся в порядке показа: у каждого свой скрипт `cs2c_NN.cfg`, который ставит свои команды
    // (`mirv_cmd clear` + addAtTick), а в конце вызывает следующий и перематывает демку — хоть назад.
    // Так шоты могут перекрываться по времени. HLAE нумерует дубли по порядку записи: take[i] — шот i.
    let d = &cfg.dialect;
    let mut s = String::new();
    let _ = writeln!(s, "// Сгенерировано cs2-cli. Карта: {}. Шотов: {}.", t.map, t.shots.len());
    let _ = writeln!(s, "// CS2 запущена через HLAE, демка открыта (playdemo), затем: exec highlights");
    let _ = writeln!(s, "// Рядом должны лежать cs2c_NN.cfg — по скрипту на шот.");
    let _ = writeln!(s, "// Дубли пишутся в {}/clips (takeNNNN по порядку показа). Перед повторной записью", cfg.out_dir);
    let _ = writeln!(s, "// удалите старую папку clips, иначе assemble склеит старые дубли.");
    let _ = writeln!(s, "mirv_streams record name \"{}/clips\"", cfg.out_dir);
    let _ = writeln!(s, "mirv_streams record screen enabled 1");
    let _ = writeln!(s, "mirv_streams record screen settings {}", d.record_settings);
    let _ = writeln!(s, "// чистый кадр: без HUD (кроме килфида), панели демки, подсветки сквозь стены и служебных надписей");
    // demo_ui_mode действует с загрузки демки (cs2-cli печатает его перед playdemo); demoui 0 лишь сворачивает
    // панель в полоску, которая попадает в кадр
    let _ = writeln!(s, "cl_drawhud 0; cl_drawhud_force_deathnotices 1; spec_show_xray 0; demo_ui_mode 0");
    let _ = writeln!(s, "r_show_build_info 0; cl_trueview_show_status 0");
    // без фокуса CS2 спит 20 мс на кадр — запись в свёрнутом окне идёт в разы медленнее
    let _ = writeln!(s, "engine_no_focus_sleep 0");
    if let Some(first) = t.shots.first() {
        let _ = writeln!(s, "exec {}", shot_cfg(0));
        let _ = writeln!(s, "demo_gototick {}", (first.start_tick - 2 * t.tick_rate as i32).max(0));
        let _ = writeln!(s, "demo_resume");
    }
    out.files.push(("highlights.cfg".into(), s));

    for (i, shot) in t.shots.iter().enumerate() {
        let mut s = String::new();
        let _ = writeln!(s, "// Шот {} -> take{i:04}: {} (score {:.0}, {:?})", i + 1, shot.title, shot.score, shot.tier);
        let _ = writeln!(s, "mirv_cmd clear");
        let _ = writeln!(s, "mirv_streams record fps {}", capture_fps(shot, cfg));
        let _ = writeln!(s, "mirv_campath enabled 0; mirv_campath clear; mirv_fov default");
        let at = |s: &mut String, tick: i32, cmds: &str| {
            let _ = writeln!(s, "mirv_cmd addAtTick {tick} \"{cmds}\"");
        };
        at(&mut s, shot.start_tick, "mirv_streams record start");
        for (si, seg) in shot.segments.iter().enumerate() {
            if let Some(style) = &seg.style {
                let _ = writeln!(s, "// камера {style} (красочность {:.2})", seg.colorfulness.unwrap_or_default());
            }
            // у каждого участка свой путь: соседние участки не влияют на сплайн, склейки чистые
            let path = match &seg.camera {
                Camera::Free { keys } => campath_xml(keys, t.tick_rate, m.server_start_tick).map(|xml| {
                    let name = format!("{}_{}.xml", shot_cfg(i), si + 1);
                    out.files.push((name.clone(), xml));
                    name
                }),
                _ => None,
            };
            at(&mut s, seg.start_tick, &camera_cmds(d, m, seg, path.as_deref()));
        }
        if let Some(curve) = zoom_curve(shot, cfg.base_fov) {
            let _ = writeln!(s, "{curve}");
        }
        let tail = match t.shots.get(i + 1) {
            Some(next) => format!("exec {}; demo_gototick {}", shot_cfg(i + 1), (next.start_tick - t.tick_rate as i32).max(0)),
            None => "mirv_cmd clear; engine_no_focus_sleep 20; demo_pause".to_owned(),
        };
        at(&mut s, shot.end_tick, &format!("mirv_streams record end; mirv_campath enabled 0; mirv_fov default; {tail}"));
        out.files.push((format!("{}.cfg", shot_cfg(i)), s));
    }

    let take_of: Vec<usize> = (0..t.shots.len()).collect();
    if let Some(a) = assembly(t, cfg, &take_of) {
        out.files.push(("assemble.ffgraph".into(), a.graph.clone()));
        out.files.push(("assemble.sh".into(), assemble_script(&a)));
        out.files.push(("assemble.bat".into(), assemble_bat(&a)));
    }
    out
}

/// Скрипт шота в `game/csgo/cfg` (плоское имя: `exec` в консоли без путей и двоеточий).
fn shot_cfg(i: usize) -> String {
    format!("cs2c_{:02}", i + 1)
}

fn clip_path(take: usize) -> String {
    format!("clips/take{take:04}/video.mp4")
}

fn audio_path(take: usize) -> String {
    format!("clips/take{take:04}/audio.wav")
}

/// `path` — имя файла пути камеры в `game/csgo/cfg` (для свободной камеры).
fn camera_cmds(d: &Dialect, m: &Match, seg: &Segment, path: Option<&str>) -> String {
    let follow = |mode: u8, player| {
        let slot = m.player(player).map(|p| p.slot).unwrap_or_default();
        format!("mirv_campath enabled 0; spec_player {}; spec_mode {mode}", slot + 1)
    };
    match (&seg.camera, path) {
        (Camera::Pov { player }, _) => follow(d.spec_mode_pov, *player),
        (Camera::Chase { player }, _) => follow(d.spec_mode_chase, *player),
        // относительный путь HLAE считает от game/bin/win64 — так обходимся без «C:» (консоль режет по ':')
        (Camera::Free { .. }, Some(file)) => format!("mirv_campath clear; mirv_campath load {}/{file}; mirv_campath enabled 1", d.campath_dir),
        (Camera::Free { .. }, None) => "mirv_campath enabled 0".to_owned(),
    }
}

/// Плавный зум одной кривой HLAE (`mirv_cmd addCurves`) вместо команды на каждый тик.
fn zoom_curve(shot: &Shot, base_fov: f32) -> Option<String> {
    let keys = &shot.zoom;
    let (first, last) = (keys.first()?, keys.last()?);
    if keys.iter().all(|k| k.scale >= 0.999) || last.tick <= first.tick {
        return None;
    }
    let span = (last.tick - first.tick) as f32;
    let mut s = format!("mirv_cmd addCurves tick {} {} - interp=linear", first.tick, last.tick);
    for k in keys {
        let _ = write!(s, " {:.4} {:.2}", (k.tick - first.tick) as f32 / span, base_fov * k.scale);
    }
    s.push_str(" -- mirv_fov {0}");
    if last.scale >= 0.999 {
        let _ = write!(s, "\nmirv_cmd addAtTick {} \"mirv_fov default\"", last.tick + 1);
    }
    Some(s)
}

/// Путь свободной камеры участка в XML HLAE. Время — время игры (секунды от нуля сервера).
/// По краям — копии крайних ключей: сплайн спокойно входит и выходит.
fn campath_xml(keys: &[director::CamKey], tick_rate: f32, server_start_tick: i32) -> Option<String> {
    let (first, last) = (keys.first()?, keys.last()?);
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<campath>\n<points>\n");
    let guard = |k: &director::CamKey, dt: i32| director::CamKey { tick: k.tick + dt, ..k.clone() };
    let mut prev_yaw: Option<f32> = None;
    for k in std::iter::once(guard(first, -1)).chain(keys.iter().cloned()).chain(std::iter::once(guard(last, 1))) {
        let time = (k.tick + server_start_tick) as f32 / tick_rate;
        // рыскание без скачков через ±180°, иначе интерполяция крутит камеру на полный оборот
        let mut yaw = k.ang[1];
        if let Some(p) = prev_yaw {
            yaw = p + (yaw - p + 180.0).rem_euclid(360.0) - 180.0;
        }
        prev_yaw = Some(yaw);
        let _ = writeln!(
            s,
            "<p t=\"{time:.6}\" x=\"{:.2}\" y=\"{:.2}\" z=\"{:.2}\" fov=\"{:.2}\" rx=\"{:.3}\" ry=\"{:.3}\" rz=\"{:.3}\"/>",
            k.pos[0], k.pos[1], k.pos[2], k.fov, k.ang[2], k.ang[0], yaw
        );
    }
    s.push_str("</points>\n</campath>\n");
    Some(s)
}

/// Кусок кривой скорости: на `[t0, t1)` (секунды клипа) скорость линейно идёт от `s0` к `s1`.
struct Piece {
    t0: f32,
    t1: f32,
    s0: f32,
    s1: f32,
}

impl Piece {
    /// Сколько секунд ролика занимает отрезок `[t0, t]` клипа: ∫ dt / s(t).
    fn out_secs(&self, t: f32) -> f32 {
        let dt = t - self.t0;
        let slope = (self.s1 - self.s0) / (self.t1 - self.t0);
        if !slope.is_finite() || slope.abs() < 1e-6 {
            dt / self.s0
        } else {
            ((self.s0 + slope * dt) / self.s0).ln() / slope
        }
    }

    /// То же выражением ffmpeg (`T` — время кадра клипа).
    fn expr(&self, o0: f32) -> String {
        let slope = (self.s1 - self.s0) / (self.t1 - self.t0);
        if !slope.is_finite() || slope.abs() < 1e-6 {
            format!("{o0:.6}+(T-{:.6})/{:.6}", self.t0, self.s0)
        } else {
            format!("{o0:.6}+log(({s0:.6}+(T-{t0:.6})*{slope:.6})/{s0:.6})/{slope:.6}", s0 = self.s0, t0 = self.t0)
        }
    }
}

/// Перерасчёт времени шота: выражение `setpts`, длина в ролике и куски `(начало, конец, длина в ролике)`.
struct Retime {
    expr: String,
    secs: f32,
    pieces: Vec<(f32, f32, f32)>,
}

/// Кривая замедления шота -> `setpts` и длины.
fn retime(shot: &Shot, tick_rate: f32) -> Retime {
    let total = (shot.end_tick - shot.start_tick) as f32 / tick_rate;
    let rel = |k: &ScaleKey| ((k.tick - shot.start_tick) as f32 / tick_rate).clamp(0.0, total);
    let mut pieces = Vec::new();
    match (shot.timescale.first(), shot.timescale.last()) {
        (Some(first), Some(last)) => {
            pieces.push(Piece { t0: 0.0, t1: rel(first), s0: first.scale, s1: first.scale });
            for w in shot.timescale.windows(2) {
                pieces.push(Piece { t0: rel(&w[0]), t1: rel(&w[1]), s0: w[0].scale, s1: w[1].scale });
            }
            pieces.push(Piece { t0: rel(last), t1: f32::INFINITY, s0: last.scale, s1: last.scale });
        }
        _ => pieces.push(Piece { t0: 0.0, t1: f32::INFINITY, s0: 1.0, s1: 1.0 }),
    }
    pieces.retain(|p| p.t1 > p.t0 && p.t0 < total);

    let mut starts = Vec::with_capacity(pieces.len());
    let mut spans = Vec::with_capacity(pieces.len());
    let mut o = 0.0;
    for p in &pieces {
        starts.push(o);
        let t1 = p.t1.min(total);
        let d = p.out_secs(t1);
        spans.push((p.t0, t1, d));
        o += d;
    }
    let mut expr = pieces.last().map(|p| p.expr(starts[pieces.len() - 1])).unwrap_or_else(|| "T".into());
    for (p, o0) in pieces.iter().zip(&starts).rev().skip(1) {
        expr = format!("if(lt(T,{:.6}),{},{expr})", p.t1, p.expr(*o0));
    }
    Retime { expr, secs: o, pieces: spans }
}

/// Склейка: граф фильтров ffmpeg (в файле — в командную строку cmd он не влезает), входы по порядку показа
/// (видео и звук дубля — отдельными файлами) и ожидаемая длина.
struct Assembly {
    inputs: Vec<String>,
    graph: String,
    length: f32,
}

/// Текст для `drawtext` внутри одинарных кавычек.
fn text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\'' => '’',
            '\\' => '/',
            ':' => '·',
            c => c,
        })
        .collect()
}

/// Подпись в кадре; без шрифта или текста — ничего.
fn caption(cfg: &Config, s: &str, style: &str) -> String {
    match (&cfg.font_file, s.is_empty()) {
        (Some(font), false) => format!(",drawtext=fontfile='{}':text='{}':expansion=none:{style}", font.replace(':', "\\:"), text(s)),
        _ => String::new(),
    }
}

const CAPTION: &str = "fontsize=30:fontcolor=white:borderw=2:bordercolor=black@0.85:x=36:y=h-th-36:enable='lt(t,3.5)'";
const CARD_TITLE: &str = "fontsize=66:fontcolor=white:x=(w-tw)/2:y=(h-th)/2-34";
const CARD_SUB: &str = "fontsize=34:fontcolor=white@0.85:x=(w-tw)/2:y=h/2+40";

/// Замедление звука кусками `atempo` (минимум 0.5 за фильтр — цепочкой).
fn atempo(tempo: f32) -> String {
    let mut s = String::new();
    let mut t = tempo;
    while t < 0.5 {
        s.push_str(",atempo=0.5");
        t /= 0.5;
    }
    if (t - 1.0).abs() > 1e-4 {
        let _ = write!(s, ",atempo={t:.6}");
    }
    s
}

fn assembly(t: &Timeline, cfg: &Config, take_of: &[usize]) -> Option<Assembly> {
    if t.shots.is_empty() {
        return None;
    }
    let (w, h) = cfg.size;
    let mut g = String::new();
    let mut order: Vec<String> = Vec::new(); // метки [v][a] в порядке показа
    let mut length = 0.0f32;
    let mut cards = 0usize;
    let mut card = |g: &mut String, order: &mut Vec<String>, length: &mut f32, title: &str, sub: &str, d: f32| {
        let _ = write!(g, "color=c=black:s={w}x{h}:r={}:d={d:.3},format=yuv420p,settb=AVTB", cfg.fps);
        g.push_str(&caption(cfg, title, CARD_TITLE));
        g.push_str(&caption(cfg, sub, CARD_SUB));
        let _ = writeln!(g, "[c{cards}];\nanullsrc=r=44100:cl=stereo:d={d:.3}[ca{cards}];");
        order.push(format!("[c{cards}][ca{cards}]"));
        *length += d;
        cards += 1;
    };
    if let Some((title, sub)) = &cfg.title {
        card(&mut g, &mut order, &mut length, title, sub, 2.2);
    }
    for (k, shot) in t.shots.iter().enumerate() {
        if let Some((title, sub)) = &shot.card {
            card(&mut g, &mut order, &mut length, title, sub, 1.4);
        }
        let r = retime(shot, t.tick_rate);
        let (vi, ai) = (2 * k, 2 * k + 1);
        let _ = write!(
            g,
            "[{vi}:v]setpts=PTS-STARTPTS,setpts='({})/TB',fps={},scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,format=yuv420p,settb=AVTB",
            r.expr, cfg.fps
        );
        g.push_str(&caption(cfg, &shot.caption, CAPTION));
        let _ = writeln!(g, "[v{k}];");
        // звук: каждый кусок кривой растягиваем в свою длину, затем подгоняем под видео точно
        let n = r.pieces.len();
        let _ = write!(g, "[{ai}:a]aformat=sample_rates=44100:channel_layouts=stereo");
        if n > 1 {
            let _ = write!(g, ",asplit={n}");
            for j in 0..n {
                let _ = write!(g, "[s{k}_{j}]");
            }
            g.push_str(";\n");
            for (j, &(a, b, d)) in r.pieces.iter().enumerate() {
                let tempo = ((b - a) / d).max(1e-3);
                let _ = writeln!(g, "[s{k}_{j}]atrim=start={a:.4}:end={b:.4},asetpts=PTS-STARTPTS{}[p{k}_{j}];", atempo(tempo));
            }
            for j in 0..n {
                let _ = write!(g, "[p{k}_{j}]");
            }
            let _ = write!(g, "concat=n={n}:v=0:a=1");
        } else if let Some(&(a, b, d)) = r.pieces.first() {
            let _ = write!(g, ",atrim=start={a:.4}:end={b:.4},asetpts=PTS-STARTPTS{}", atempo((b - a) / d));
        }
        let _ = writeln!(g, ",apad=whole_dur={0:.4},atrim=end={0:.4},asetpts=PTS-STARTPTS[a{k}];", r.secs);
        order.push(format!("[v{k}][a{k}]"));
        length += r.secs;
    }
    let fade = 0.6f32;
    let _ = writeln!(g, "{}concat=n={}:v=1:a=1[vc][ac];", order.concat(), order.len());
    let _ = writeln!(g, "[vc]fade=t=in:st=0:d={fade},fade=t=out:st={:.3}:d={fade}[v];", length - fade);
    let _ = writeln!(g, "[ac]afade=t=out:st={:.3}:d={fade}[a]", length - fade);
    let inputs = (0..t.shots.len()).flat_map(|i| [clip_path(take_of[i]), audio_path(take_of[i])]).collect();
    Some(Assembly { inputs, graph: g, length })
}

const ENCODE: &str = "-c:v libx264 -crf 16 -preset slow -pix_fmt yuv420p -c:a aac -b:a 192k highlights.mp4";

/// Аргументы ffmpeg для склейки (запускать в папке результата, где лежат `clips/` и `assemble.ffgraph`).
pub fn ffmpeg_args(t: &Timeline, cfg: &Config) -> Option<Vec<String>> {
    let take_of: Vec<usize> = (0..t.shots.len()).collect();
    let a = assembly(t, cfg, &take_of)?;
    let mut v = vec!["-y".to_owned()];
    for p in &a.inputs {
        v.push("-i".into());
        v.push(p.clone());
    }
    v.extend(["-/filter_complex", "assemble.ffgraph", "-map", "[v]", "-map", "[a]"].map(String::from));
    v.extend(ENCODE.split_whitespace().map(String::from));
    Some(v)
}

fn assemble_script(a: &Assembly) -> String {
    let mut s = String::from("#!/bin/sh\n# Склейка дублей HLAE в ролик (нужен ffmpeg 7+).\nset -e\ncd \"$(dirname \"$0\")\"\n");
    let inputs: Vec<String> = a.inputs.iter().map(|p| format!("-i {p}")).collect();
    let _ = writeln!(s, "ffmpeg -y {} \\\n  -/filter_complex assemble.ffgraph -map '[v]' -map '[a]' \\\n  {ENCODE}", inputs.join(" "));
    let _ = writeln!(s, "# ожидаемая длина: {:.1} c", a.length);
    s
}

/// То же для Windows (cmd / двойной клик): пути с обратными слэшами, кавычки cmd.
fn assemble_bat(a: &Assembly) -> String {
    let mut s = String::from("@echo off\r\nrem Склейка дублей HLAE в ролик (нужен ffmpeg 7+).\r\nchcp 65001 >nul\r\ncd /d \"%~dp0\"\r\n");
    let inputs: Vec<String> = a.inputs.iter().map(|p| format!("-i {}", p.replace('/', "\\"))).collect();
    let _ = write!(s, "ffmpeg -y {} -/filter_complex assemble.ffgraph -map \"[v]\" -map \"[a]\" {ENCODE}\r\n", inputs.join(" "));
    let _ = write!(s, "rem ожидаемая длина: {:.1} c\r\npause\r\n", a.length);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2::PlayerId;
    use director::Tier;

    fn shot(start: i32, end: i32, camera: Camera, ts: Vec<ScaleKey>) -> Shot {
        Shot {
            title: "t".into(),
            tier: Tier::Great,
            score: 1.0,
            player: PlayerId(0),
            round: 1,
            start_tick: start,
            end_tick: end,
            segments: vec![Segment { start_tick: start, end_tick: end, camera, style: None, colorfulness: None }],
            timescale: ts,
            zoom: vec![],
            caption: String::new(),
            card: None,
        }
    }

    fn key(tick: i32, scale: f32) -> ScaleKey {
        ScaleKey { tick, scale }
    }

    fn setup() -> (Match, Timeline) {
        // userid 7 в слоте 3 -> spec_player 4
        let m = Match::build("de_test".into(), 64.0, vec![(7, 3, "ace".into(), 1)], &[]);
        let t = Timeline {
            map: "de_test".into(),
            tick_rate: 64.0,
            server_start_tick: 9376,
            // порядок показа: сначала позднее по времени
            shots: vec![
                shot(5000, 5400, Camera::Pov { player: PlayerId(7) }, vec![]),
                shot(1000, 1400, Camera::Chase { player: PlayerId(7) }, vec![
                    key(1100, 1.0),
                    key(1110, 0.25),
                    key(1200, 0.25),
                    key(1210, 1.0),
                ]),
            ],
        };
        (m, t)
    }

    #[test]
    fn cfg_chains_shots_in_play_order_with_rewinds() {
        let (m, t) = setup();
        let cfg = Config { out_dir: "C:/out/m 1".into(), ..Config::default() };
        let out = render(&m, &t, &cfg);
        let s = out.file("highlights.cfg").unwrap();
        assert!(s.contains("mirv_streams record name \"C:/out/m 1/clips\""), "{s}");
        assert!(s.contains("mirv_streams record screen enabled 1"));
        assert!(s.contains("exec cs2c_01\ndemo_gototick 4872"), "первый шот 5000, запас 2 с: {s}");
        let first = out.file("cs2c_01.cfg").unwrap();
        let second = out.file("cs2c_02.cfg").unwrap();
        // fps записи — свой у шота: без замедления 60, с замедлением 0.25 — 240
        assert!(first.contains("mirv_streams record fps 60"), "{first}");
        assert!(second.contains("mirv_streams record fps 240"), "{second}");
        assert!(first.starts_with("// Шот 1 -> take0000") && first.contains("mirv_cmd clear"));
        assert!(first.contains("spec_player 4; spec_mode 2"), "POV, слот 3 -> 4: {first}");
        // после шота 5000..5400 — назад к шоту 1000..1400, команды которого ставит cs2c_02
        assert!(first.contains("exec cs2c_02; demo_gototick 936"), "{first}");
        assert!(second.contains("spec_player 4; spec_mode 3"), "chase");
        assert!(second.contains("demo_pause"), "последний шот останавливает демку");
        for f in [s, first, second] {
            assert!(!f.contains("timescale"), "замедление — при склейке, не в игре");
            // в mirv_cmd кавычки теряются, а консоль режет по ':' — абсолютных путей внутри быть не должно
            for line in f.lines().filter(|l| l.starts_with("mirv_cmd")) {
                assert!(!line.contains(":/"), "{line}");
            }
        }
        let ffgraph = out.file("assemble.ffgraph").unwrap();
        let bat = out.file("assemble.bat").unwrap();
        // видео и звук дубля — отдельными входами, в порядке показа
        assert!(bat.contains("-i clips\\take0000\\video.mp4 -i clips\\take0000\\audio.wav -i clips\\take0001\\video.mp4"), "{bat}");
        assert!(bat.contains("-/filter_complex assemble.ffgraph") && bat.contains("-map \"[a]\"") && bat.contains("\r\n"));
        assert!(ffgraph.contains("[0:v]") && ffgraph.contains("[2:v]") && ffgraph.contains("[1:a]") && ffgraph.contains("concat=n=2:v=1:a=1"));
        assert!(!ffgraph.contains("xfade"), "жёсткие склейки, без кроссфейдов");
    }

    #[test]
    fn retime_matches_analytic_length() {
        let tr = 64.0;
        // без замедления: длина = длина клипа
        let r = retime(&shot(0, 640, Camera::Pov { player: PlayerId(0) }, vec![]), tr);
        assert!((r.secs - 10.0).abs() < 1e-4 && r.expr.contains("T"));
        // 1 c обычной скорости, 1 c при 0.5 (=2 c), 1 c обычной
        let ts = vec![key(63, 1.0), key(64, 0.5), key(128, 0.5), key(129, 1.0)];
        let r = retime(&shot(0, 192, Camera::Pov { player: PlayerId(0) }, ts), tr);
        assert!((r.secs - 4.0).abs() < 0.03, "{}", r.secs);
        // куски: до замедления, рампа, медленный, рампа, после — их длины складываются в целое
        assert_eq!(r.pieces.len(), 5);
        let sum: f32 = r.pieces.iter().map(|p| p.2).sum();
        assert!((sum - r.secs).abs() < 1e-3);
        // вне ключей — крайнее значение, как в director::scale_at: всё при 0.5
        let ts = vec![key(64, 0.5), key(128, 0.5)];
        let r = retime(&shot(0, 192, Camera::Pov { player: PlayerId(0) }, ts), tr);
        assert!((r.secs - 6.0).abs() < 1e-3, "{}", r.secs);
        // линейная рампа 1 -> 0.5 за 1 c: ∫ dt / (1 - 0.5t) = 2 ln 2
        let ts = vec![key(0, 1.0), key(64, 0.5)];
        let r = retime(&shot(0, 64, Camera::Pov { player: PlayerId(0) }, ts), tr);
        assert!((r.secs - 2.0 * 2f32.ln()).abs() < 1e-3, "{}", r.secs);
        assert!(r.expr.contains("log("), "{}", r.expr);
    }

    #[test]
    fn audio_is_stretched_per_piece_and_captions_use_font() {
        let (m, mut t) = setup();
        t.shots[1].caption = "Глазами ace · it's".into();
        t.shots[1].card = Some(("Раунд 1".into(), "2 убийства".into()));
        let cfg = Config { font_file: Some("C:/Windows/Fonts/arialbd.ttf".into()), title: Some(("ace".into(), "de_test".into())), ..Config::default() };
        let out = render(&m, &t, &cfg);
        let g = out.file("assemble.ffgraph").unwrap();
        // шот 2 замедлен до 0.25: звук режется на куски, медленный — цепочкой atempo 0.5 x 0.5
        assert!(g.contains("[3:a]aformat=sample_rates=44100:channel_layouts=stereo,asplit=5"), "{g}");
        assert!(g.contains(",atempo=0.5,atempo=0.500000[p1_2]"), "{g}");
        assert!(g.contains("concat=n=5:v=0:a=1,apad=whole_dur="), "{g}");
        // шот 1 без замедления — без atempo
        assert!(g.contains("[1:a]aformat=sample_rates=44100:channel_layouts=stereo,atrim=start=0.0000:end=6.2500,asetpts=PTS-STARTPTS,apad"), "{g}");
        // подписи и заставки: кавычка и двоеточие в тексте заменены, путь к шрифту экранирован
        assert!(g.contains(r"drawtext=fontfile='C\:/Windows/Fonts/arialbd.ttf':text='Глазами ace · it’s':expansion=none"), "{g}");
        assert!(g.contains("text='Раунд 1'") && g.contains("text='de_test'"));
        // заставка ролика + заставка раунда + 2 шота
        assert!(g.contains("concat=n=4:v=1:a=1[vc][ac]"), "{g}");
        let d1 = retime(&t.shots[1], 64.0).secs;
        assert!(d1 > 400.0 / 64.0, "замедленный шот длиннее исходного");
        let total = 2.2 + 6.25 + 1.4 + d1;
        assert!(g.contains(&format!("fade=t=out:st={:.3}:d=0.6", total - 0.6)), "{g}");
        let bat = out.file("assemble.bat").unwrap();
        assert!(bat.contains(&format!("ожидаемая длина: {total:.1}")), "{bat}");
    }

    #[test]
    fn zoom_becomes_one_smooth_curve() {
        let (m, mut t) = setup();
        t.shots[1].zoom = vec![key(1100, 1.0), key(1110, 0.5), key(1200, 0.5), key(1210, 1.0)];
        let out = render(&m, &t, &Config { base_fov: 90.0, ..Config::default() });
        let s = out.file("cs2c_02.cfg").unwrap();
        assert!(s.contains("mirv_cmd addCurves tick 1100 1210 - interp=linear 0.0000 90.00 0.0909 45.00"), "{s}");
        assert!(s.contains("-- mirv_fov {0}"));
        assert!(s.contains("mirv_cmd addAtTick 1211 \"mirv_fov default\""));
    }

    #[test]
    fn campath_uses_game_time_and_hlae_angle_order() {
        let (mut m, mut t) = setup();
        m.server_start_tick = 6400;
        let keys = vec![
            director::CamKey { tick: 64, pos: [1.0, 2.0, 3.0], ang: [10.0, 90.0, 0.0], fov: 90.0 },
            director::CamKey { tick: 128, pos: [4.0, 5.0, 6.0], ang: [20.0, 45.0, 5.0], fov: 80.0 },
            // через ±180°: -170 должно стать 190, а не крутить камеру назад
            director::CamKey { tick: 192, pos: [4.0, 5.0, 6.0], ang: [0.0, 170.0, 0.0], fov: 80.0 },
            director::CamKey { tick: 256, pos: [4.0, 5.0, 6.0], ang: [0.0, -170.0, 0.0], fov: 80.0 },
        ];
        t.shots[0].segments[0].camera = Camera::Free { keys };
        let out = render(&m, &t, &Config { out_dir: "D:/x".into(), ..Config::default() });
        let xml = out.file("cs2c_01_1.xml").unwrap();
        // (64 + 6400) / 64 = 101 c
        assert!(xml.contains("t=\"101.000000\""), "{xml}");
        assert!(xml.contains("rx=\"5.000\" ry=\"20.000\" rz=\"45.000\""), "rx — крен, ry — тангаж, rz — рыскание: {xml}");
        assert!(xml.contains("rz=\"190.000\"") && !xml.contains("rz=\"-170"), "{xml}");
        assert_eq!(xml.matches("<p ").count(), 6, "по копии ключа на каждом краю");
        let s = out.file("cs2c_01.cfg").unwrap();
        assert!(s.contains("\"mirv_campath clear; mirv_campath load ../../csgo/cfg/cs2c_01_1.xml; mirv_campath enabled 1\""), "{s}");
    }
}
