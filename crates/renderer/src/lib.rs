//! Превращает монтажный план в файлы для записи ролика:
//! * `plan.json`       — сам план (стабильный формат, можно скармливать другим инструментам);
//! * `highlights.cfg`  — скрипт для CS2 под HLAE: перематывает демку, включает пути камеры,
//!                       крутит `demo_timescale` и пишет каждый шот в отдельный клип;
//! * `campath_*.xml`   — пути камеры: свободные пролёты, а также «глаза игрока» и «из-за плеча»,
//!                       которые строятся по позициям игрока и не зависят от команд наблюдателя;
//! * `assemble.bat/.sh` — вызов `cs2-cli assemble`: склейка клипов HLAE в один ролик.
//!
//! Формат campath и время выверены по реальному файлу HLAE (`mirv_campath save`):
//! `t` — игровое время `(тик демки + server_start_tick) / tick_rate`, `rx` — крен, `ry` — pitch, `rz` — yaw.
//! Команды HLAE/CS2 собраны в `Dialect`: если что-то в вашей сборке называется иначе, правьте там.

pub mod assemble;

use std::{fmt::Write as _, io, path::Path};

use cs2::Match;
use director::{Camera, ScaleKey, Segment, Timeline};

/// Названия команд.
#[derive(Debug, Clone)]
pub struct Dialect {
    /// Скорость воспроизведения демо (`find timescale` в консоли CS2: «Sets demo replay speed»).
    pub timescale_cmd: String,
}

impl Default for Dialect {
    fn default() -> Self {
        Self { timescale_cmd: "demo_timescale".into() }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub fps: u32,
    /// Обычный FOV; нужен только для запасного `mirv_fov` (когда нет позиций игроков).
    pub base_fov: f32,
    /// Папка, где лежат `campath_*.xml`, абсолютная, с `/`. HLAE ищет файлы относительно
    /// `game/bin/win64`, поэтому без абсолютного пути `mirv_campath load` не найдёт файл.
    pub campath_dir: Option<String>,
    pub dialect: Dialect,
}

impl Default for Config {
    fn default() -> Self {
        Self { fps: 60, base_fov: 90.0, campath_dir: None, dialect: Dialect::default() }
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
}

/// Безопасно ли путь/имя вставлять в команду консоли внутри `mirv_cmd ... "..."` без кавычек.
pub fn console_safe(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || matches!(c, '"' | ';' | '\''))
}

pub fn render(m: &Match, t: &Timeline, cfg: &Config) -> Output {
    let mut out = Output::default();
    out.files.push(("plan.json".into(), serde_json::to_string_pretty(t).expect("план сериализуется")));

    let mut cfg_text = String::new();
    let _ = writeln!(cfg_text, "// Сгенерировано cs2-cli. Карта: {}. Шотов: {}.", t.map, t.shots.len());
    let _ = writeln!(cfg_text, "// Запуск: CS2 через HLAE -> playdemo <демка> (дождаться загрузки) -> exec highlights");
    let _ = writeln!(cfg_text, "mirv_cmd clear");
    let _ = writeln!(cfg_text, "mirv_streams record fps {}", cfg.fps);
    let ts = &cfg.dialect.timescale_cmd;

    // Записываем в хронологическом порядке (перемотка только вперёд),
    // а имена клипов привязаны к порядку показа в монтаже.
    let mut order: Vec<usize> = (0..t.shots.len()).collect();
    order.sort_by_key(|&i| t.shots[i].start_tick);

    for (n, &i) in order.iter().enumerate() {
        let shot = &t.shots[i];
        let clip = clip_name(i);
        let _ = writeln!(cfg_text, "\n// Шот {}: {} (score {:.0}, {:?})", i + 1, shot.title, shot.score, shot.tier);

        let at = |cfg_text: &mut String, tick: i32, cmds: &str| {
            let _ = writeln!(cfg_text, "mirv_cmd addAtTick {tick} \"{cmds}\"");
        };

        at(&mut cfg_text, shot.start_tick, &format!("mirv_streams record name {clip}; mirv_streams record start"));
        for (si, seg) in shot.segments.iter().enumerate() {
            if let Some(style) = &seg.style {
                match seg.colorfulness {
                    Some(c) => { let _ = writeln!(cfg_text, "// камера {style} (красочность {c:.2})"); }
                    None => { let _ = writeln!(cfg_text, "// камера {style}"); }
                }
            }
            let file = campath_name(i, si);
            at(&mut cfg_text, seg.start_tick, &camera_cmds(seg, m.name(shot.player).as_str(), &file, cfg));
            if let Camera::Free { keys } = &seg.camera {
                out.files.push((file, campath_xml(t.tick_rate, t.server_start_tick, keys)));
            }
        }
        for (tick, scale) in curve_steps(&shot.timescale) {
            at(&mut cfg_text, tick, &format!("{ts} {scale:.3}"));
        }
        // запасной зум: только если часть кадров идёт не по пути камеры
        for (tick, scale) in curve_steps(&shot.zoom) {
            let cmd = if scale >= 0.999 { "mirv_fov default".to_owned() } else { format!("mirv_fov {:.1}", cfg.base_fov * scale) };
            at(&mut cfg_text, tick, &cmd);
        }

        let mut end = format!("mirv_streams record end; mirv_campath enabled 0; {ts} 1");
        if !shot.zoom.is_empty() {
            end.push_str("; mirv_fov default");
        }
        match order.get(n + 1) {
            Some(&next) => {
                let _ = write!(end, "; demo_gototick {}", (t.shots[next].start_tick - (t.tick_rate as i32)).max(0));
            }
            // последний шот: выходим и чистим отложенные команды, чтобы они не сработали в меню
            None => end.push_str("; disconnect; mirv_cmd clear"),
        }
        at(&mut cfg_text, shot.end_tick, &end);
    }

    if let Some(&first) = order.first() {
        let _ = writeln!(cfg_text, "\ndemo_gototick {}", (t.shots[first].start_tick - 2 * t.tick_rate as i32).max(0));
        let _ = writeln!(cfg_text, "demo_resume");
    }

    out.files.push(("highlights.cfg".into(), cfg_text));
    out.files.push(("assemble.sh".into(), assemble_sh()));
    out.files.push(("assemble.bat".into(), assemble_bat()));
    out
}

fn clip_name(i: usize) -> String {
    format!("clips/shot_{:02}", i + 1)
}

fn campath_name(shot: usize, seg: usize) -> String {
    format!("campath_{:02}_{}.xml", shot + 1, seg + 1)
}

fn camera_cmds(seg: &Segment, player_name: &str, campath_file: &str, cfg: &Config) -> String {
    match &seg.camera {
        Camera::Free { .. } => {
            let path = match &cfg.campath_dir {
                Some(dir) => format!("{dir}/{campath_file}"),
                None => campath_file.to_owned(),
            };
            format!("mirv_campath clear; mirv_campath load {path}; mirv_campath enabled 1")
        }
        // запасной вариант без позиций игроков: наблюдатель по имени (spec_mode в демках не работает)
        Camera::Pov { .. } | Camera::Chase { .. } => {
            if console_safe(player_name) {
                format!("mirv_campath enabled 0; spec_player {player_name}")
            } else {
                "mirv_campath enabled 0".to_owned()
            }
        }
    }
}

/// Кусочно-линейная кривая -> редкие шаги команд (`host_timescale`, `mirv_fov`): в плавных участках шаг раз в 2 тика.
fn curve_steps(keys: &[ScaleKey]) -> Vec<(i32, f32)> {
    let mut steps = Vec::new();
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut t = a.tick;
        while t < b.tick {
            let u = (t - a.tick) as f32 / (b.tick - a.tick) as f32;
            steps.push((t, a.scale + (b.scale - a.scale) * u));
            t += if a.scale == b.scale { b.tick - a.tick } else { 2 };
        }
    }
    if let Some(last) = keys.last() {
        steps.push((last.tick, last.scale));
    }
    steps
}

/// Путь камеры в XML-формате HLAE (проверен на файле от `mirv_campath save`).
/// Время точки — игровое: `(тик + server_start_tick) / tick_rate`. В начало и конец добавлены
/// удерживающие ключи: иначе HLAE пишет «Campath enabled but can not be evaluated yet»,
/// когда команда включения срабатывает на тик раньше первого ключа.
fn campath_xml(tick_rate: f32, server_start_tick: i32, keys: &[director::CamKey]) -> String {
    const PAD_TICKS: i32 = 4;
    let time = |tick: i32| (tick + server_start_tick) as f64 / tick_rate as f64;
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<campath>\n\t<points>\n");
    let point = |s: &mut String, t: f64, k: &director::CamKey| {
        // rx — крен, ry — pitch, rz — yaw (углы Source: pitch, yaw, roll)
        let _ = writeln!(
            s,
            "\t\t<p t=\"{t:.6}\" x=\"{:.4}\" y=\"{:.4}\" z=\"{:.4}\" fov=\"{:.4}\" rx=\"{:.4}\" ry=\"{:.4}\" rz=\"{:.4}\"/>",
            k.pos[0], k.pos[1], k.pos[2], k.fov, k.ang[2], k.ang[0], k.ang[1]
        );
    };
    if let Some(first) = keys.first() {
        point(&mut s, time(first.tick - PAD_TICKS), first);
    }
    for k in keys {
        point(&mut s, time(k.tick), k);
    }
    if let Some(last) = keys.last() {
        point(&mut s, time(last.tick + PAD_TICKS), last);
    }
    s.push_str("\t</points>\n</campath>\n");
    s
}

fn assemble_sh() -> String {
    "#!/bin/sh\n\
     # Склейка клипов HLAE в highlights.mp4. Параметр — папка game/bin/win64 (где лежит clips/).\n\
     here=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
     exec cs2-cli assemble --plan \"$here/plan.json\" --clips \"${1:-.}\" --out \"$here/highlights.mp4\"\n"
        .to_owned()
}

fn assemble_bat() -> String {
    // CRLF: cmd.exe плохо переносит LF
    [
        "@echo off",
        "rem Склейка клипов HLAE в highlights.mp4. Параметр - папка game\\bin\\win64 (где лежит clips\\).",
        "set CLIPS=%~1",
        "if \"%CLIPS%\"==\"\" set CLIPS=C:\\Program Files (x86)\\Steam\\steamapps\\common\\Counter-Strike Global Offensive\\game\\bin\\win64",
        "cs2-cli assemble --plan \"%~dp0plan.json\" --clips \"%CLIPS%\" --out \"%~dp0highlights.mp4\"",
        "",
    ]
    .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2::PlayerId;
    use director::{CamKey, Shot, Tier};

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
        }
    }

    fn setup() -> (Match, Timeline) {
        let m = Match::build("de_test".into(), 64.0, vec![(0, 0, "ace".into(), 1)], &[]);
        let t = Timeline {
            map: "de_test".into(),
            tick_rate: 64.0,
            server_start_tick: 9376,
            // порядок показа: сначала позднее по времени
            shots: vec![
                shot(5000, 5400, Camera::Pov { player: PlayerId(0) }, vec![]),
                shot(1000, 1400, Camera::Chase { player: PlayerId(0) }, vec![
                    ScaleKey { tick: 1100, scale: 1.0 },
                    ScaleKey { tick: 1110, scale: 0.25 },
                    ScaleKey { tick: 1200, scale: 0.25 },
                    ScaleKey { tick: 1210, scale: 1.0 },
                ]),
            ],
        };
        (m, t)
    }

    fn file<'a>(out: &'a Output, name: &str) -> &'a str {
        &out.files.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("нет файла {name}")).1
    }

    #[test]
    fn cfg_records_chronologically_and_uses_demo_timescale() {
        let (m, t) = setup();
        let out = render(&m, &t, &Config::default());
        let cfg = file(&out, "highlights.cfg");
        let first = cfg.find("record name clips/shot_02").unwrap();
        let second = cfg.find("record name clips/shot_01").unwrap();
        assert!(first < second, "сначала пишем то, что раньше по тикам");
        assert!(cfg.contains("mirv_cmd addAtTick 1200 \"demo_timescale 0.250\""), "{cfg}");
        assert!(!cfg.contains("host_timescale"), "host_timescale в демках — cheat и не работает");
        assert!(!cfg.contains("spec_player_by_accountid") && !cfg.contains("spec_mode"));
        assert!(cfg.contains("spec_player ace"), "запасной выбор игрока по имени");
        assert!(cfg.contains("demo_gototick 4936"), "после шота 1000..1400 перематываем к 5000 с запасом секунды");
        // в конце чистим отложенные команды, иначе они сработают в главном меню
        assert!(cfg.contains("disconnect; mirv_cmd clear"));
    }

    #[test]
    fn unsafe_player_names_are_not_put_into_console_commands() {
        let (_, t) = setup();
        let m = Match::build("de_test".into(), 64.0, vec![(0, 0, "with space".into(), 1)], &[]);
        let cfg = file(&render(&m, &t, &Config::default()), "highlights.cfg").to_owned();
        assert!(!cfg.contains("spec_player with"), "{cfg}");
        assert!(console_safe("VOVA_EBAKIN") && !console_safe("a b") && !console_safe("a\"b") && !console_safe(""));
    }

    #[test]
    fn campath_matches_the_real_hlae_format() {
        let (m, mut t) = setup();
        let keys = vec![
            CamKey { tick: 640, pos: [1.0, 2.0, 3.0], ang: [10.0, 90.0, 5.0], fov: 90.0 },
            CamKey { tick: 704, pos: [4.0, 5.0, 6.0], ang: [-20.0, 135.0, 0.0], fov: 60.0 },
        ];
        t.shots[0].segments[0].camera = Camera::Free { keys };
        let out = render(&m, &t, &Config { campath_dir: Some("C:/out/test".into()), ..Config::default() });
        let xml = file(&out, "campath_01_1.xml");
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<campath>"));
        // t = (тик + 9376) / 64: (640 + 9376) / 64 = 156.5
        assert!(xml.contains("t=\"156.500000\""), "{xml}");
        // pitch -> ry, yaw -> rz, roll -> rx (как в файле от mirv_campath save)
        assert!(xml.contains("fov=\"90.0000\" rx=\"5.0000\" ry=\"10.0000\" rz=\"90.0000\""), "{xml}");
        // удерживающие ключи до и после: 4 тика = 0.0625 c
        assert!(xml.contains("t=\"156.437500\"") && xml.contains("t=\"157.562500\""), "{xml}");
        assert_eq!(xml.matches("<p ").count(), 4);
        // абсолютный путь в команде загрузки
        let cfg = file(&out, "highlights.cfg");
        assert!(cfg.contains("mirv_campath load C:/out/test/campath_01_1.xml"), "{cfg}");
    }

    #[test]
    fn zoom_fallback_becomes_mirv_fov_and_resets() {
        let (m, mut t) = setup();
        t.shots[1].zoom = vec![
            ScaleKey { tick: 1100, scale: 1.0 },
            ScaleKey { tick: 1110, scale: 0.5 },
            ScaleKey { tick: 1200, scale: 0.5 },
            ScaleKey { tick: 1210, scale: 1.0 },
        ];
        let out = render(&m, &t, &Config { base_fov: 90.0, ..Config::default() });
        let cfg = file(&out, "highlights.cfg");
        assert!(cfg.contains("mirv_cmd addAtTick 1110 \"mirv_fov 45.0\""), "{cfg}");
        assert!(cfg.contains("mirv_fov default"));
        // а если зум вшит в кампат (zoom пуст), mirv_fov вообще не используется
        t.shots[1].zoom.clear();
        let cfg = file(&render(&m, &t, &Config::default()), "highlights.cfg").to_owned();
        assert!(!cfg.contains("mirv_fov"));
    }

    #[test]
    fn assemble_wrappers_call_cs2_cli() {
        let (m, t) = setup();
        let out = render(&m, &t, &Config::default());
        let bat = file(&out, "assemble.bat");
        assert!(bat.contains("cs2-cli assemble --plan") && bat.contains("\r\n") && bat.contains("game\\bin\\win64"));
        assert!(file(&out, "assemble.sh").contains("cs2-cli assemble --plan"));
    }
}
