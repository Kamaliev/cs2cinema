//! Превращает монтажный план в файлы для записи ролика:
//! * `plan.json`       — сам план (стабильный формат, можно скармливать другим инструментам);
//! * `highlights.cfg`  — скрипт для CS2 под HLAE: перематывает демку, переключает камеру,
//!                       крутит `host_timescale` и пишет каждый шот в отдельный клип;
//! * `campath_*.xml`   — пути свободной камеры (если режиссёр их построил);
//! * `assemble.sh`     — ffmpeg-склейка клипов в порядке показа с кроссфейдами.
//!
//! ВАЖНО: консольные команды HLAE/CS2 собраны в одном месте (`Dialect`) — это единственное,
//! что нельзя проверить без запущенной игры. Если какая-то команда в вашей сборке называется
//! иначе, поправьте её здесь, а не в логике режиссёра.

use std::{fmt::Write as _, io, path::Path};

use cs2::Match;
use director::{Camera, ScaleKey, Segment, Timeline};

const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// Названия команд и режимов наблюдателя.
#[derive(Debug, Clone)]
pub struct Dialect {
    pub spec_mode_pov: u8,
    pub spec_mode_chase: u8,
    /// Расширение клипов, которые пишет HLAE (ffmpeg-профиль).
    pub clip_ext: String,
}

impl Default for Dialect {
    fn default() -> Self {
        Self { spec_mode_pov: 4, spec_mode_chase: 5, clip_ext: "mp4".into() }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub fps: u32,
    /// Обычный FOV; зум режиссёра — множитель от него (`mirv_fov`).
    pub base_fov: f32,
    pub crossfade_secs: f32,
    pub dialect: Dialect,
}

impl Default for Config {
    fn default() -> Self {
        Self { fps: 60, base_fov: 90.0, crossfade_secs: 0.35, dialect: Dialect::default() }
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

pub fn render(m: &Match, t: &Timeline, cfg: &Config) -> Output {
    let mut out = Output::default();
    out.files.push(("plan.json".into(), serde_json::to_string_pretty(t).expect("план сериализуется")));

    let mut cfg_text = String::new();
    let _ = writeln!(cfg_text, "// Сгенерировано cs2-cli. Карта: {}. Шотов: {}.", t.map, t.shots.len());
    let _ = writeln!(cfg_text, "// Запуск: CS2 через HLAE -> playdemo <демка> -> exec highlights");
    let _ = writeln!(cfg_text, "mirv_cmd clear");
    let _ = writeln!(cfg_text, "mirv_streams record fps {}", cfg.fps);

    // Записываем в хронологическом порядке (перемотка только вперёд),
    // а имена клипов привязаны к порядку показа в монтаже.
    let mut order: Vec<usize> = (0..t.shots.len()).collect();
    order.sort_by_key(|&i| t.shots[i].start_tick);

    for (n, &i) in order.iter().enumerate() {
        let shot = &t.shots[i];
        let clip = clip_name(i);
        let _ = writeln!(cfg_text, "\n// Шот {}: {} (score {:.0}, {:?})", i + 1, shot.title, shot.score, shot.tier);
        let accountid = m
            .player(shot.player)
            .map(|p| p.xuid.saturating_sub(STEAM_ID64_BASE))
            .unwrap_or_default();

        let at = |cfg_text: &mut String, tick: i32, cmds: &str| {
            let _ = writeln!(cfg_text, "mirv_cmd addAtTick {tick} \"{cmds}\"");
        };

        at(&mut cfg_text, shot.start_tick, &format!("mirv_streams record name {clip}; mirv_streams record start"));
        for (si, seg) in shot.segments.iter().enumerate() {
            if let Some(style) = &seg.style {
                let _ = writeln!(cfg_text, "// камера {style} (красочность {:.2})", seg.colorfulness.unwrap_or_default());
            }
            at(&mut cfg_text, seg.start_tick, &camera_cmds(&cfg.dialect, seg, accountid, &campath_name(i, si)));
            if let Camera::Free { keys } = &seg.camera {
                let xml = campath_xml(t.tick_rate, keys);
                out.files.push((campath_name(i, si), xml));
            }
        }
        for (tick, scale) in curve_steps(&shot.timescale) {
            at(&mut cfg_text, tick, &format!("host_timescale {scale:.3}"));
        }
        for (tick, scale) in curve_steps(&shot.zoom) {
            let cmd = if scale >= 0.999 { "mirv_fov default".to_owned() } else { format!("mirv_fov {:.1}", cfg.base_fov * scale) };
            at(&mut cfg_text, tick, &cmd);
        }

        let tail = match order.get(n + 1) {
            Some(&next) => {
                format!("demo_gototick {}", (t.shots[next].start_tick - (t.tick_rate as i32)).max(0))
            }
            None => "disconnect".to_owned(),
        };
        at(&mut cfg_text, shot.end_tick, &format!("mirv_streams record end; mirv_campath enabled 0; mirv_fov default; host_timescale 1; {tail}"));
    }

    if let Some(&first) = order.first() {
        let _ = writeln!(cfg_text, "\ndemo_gototick {}", (t.shots[first].start_tick - 2 * t.tick_rate as i32).max(0));
        let _ = writeln!(cfg_text, "demo_resume");
    }

    out.files.push(("highlights.cfg".into(), cfg_text));
    out.files.push(("assemble.sh".into(), assemble_script(t, cfg)));
    out.files.push(("assemble.bat".into(), assemble_bat(t, cfg)));
    out
}

fn clip_name(i: usize) -> String {
    format!("clips/shot_{:02}", i + 1)
}

fn campath_name(shot: usize, seg: usize) -> String {
    format!("campath_{:02}_{}.xml", shot + 1, seg + 1)
}

fn camera_cmds(d: &Dialect, seg: &Segment, accountid: u64, campath: &str) -> String {
    match &seg.camera {
        Camera::Pov { .. } => format!(
            "mirv_campath enabled 0; spec_mode {}; spec_player_by_accountid {accountid}",
            d.spec_mode_pov
        ),
        Camera::Chase { .. } => format!(
            "mirv_campath enabled 0; spec_mode {}; spec_player_by_accountid {accountid}",
            d.spec_mode_chase
        ),
        Camera::Free { .. } => format!("mirv_campath clear; mirv_campath load {campath}; mirv_campath enabled 1"),
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

/// Путь камеры в XML-формате HLAE. Время — секунды демо-времени.
fn campath_xml(tick_rate: f32, keys: &[director::CamKey]) -> String {
    let mut s = String::from("<campath>\n<points>\n");
    for k in keys {
        let _ = writeln!(
            s,
            "<p t=\"{:.4}\" x=\"{:.2}\" y=\"{:.2}\" z=\"{:.2}\" rx=\"{:.3}\" ry=\"{:.3}\" rz=\"{:.3}\" fov=\"{:.2}\" selected=\"0\" />",
            k.tick as f32 / tick_rate, k.pos[0], k.pos[1], k.pos[2], k.ang[0], k.ang[1], k.ang[2], k.fov
        );
    }
    s.push_str("</points>\n</campath>\n");
    s
}

/// Аргументы ffmpeg для склейки клипов (общие для `assemble.sh` и `assemble.bat`) и ожидаемая длина.
fn ffmpeg_parts(t: &Timeline, cfg: &Config) -> Option<(Vec<String>, Option<String>, f32)> {
    let n = t.shots.len();
    if n == 0 {
        return None;
    }
    let durations: Vec<f32> = t.shots.iter().map(|s| s.output_secs(t.tick_rate)).collect();
    let f = cfg.crossfade_secs;
    let inputs: Vec<String> = (0..n).map(|i| format!("-i {}.{}", clip_name(i), cfg.dialect.clip_ext)).collect();

    if n == 1 {
        return Some((inputs, None, durations[0]));
    }
    let mut filter = String::new();
    let mut prev = "[0:v]".to_owned();
    let mut length = durations[0];
    for i in 1..n {
        let label = if i == n - 1 { "[v]".to_owned() } else { format!("[x{i}]") };
        let _ = write!(filter, "{prev}[{i}:v]xfade=transition=fade:duration={f}:offset={:.3}{label};", length - f);
        length += durations[i] - f;
        prev = label;
    }
    filter.pop();
    Some((inputs, Some(filter), length))
}

const ENCODE: &str = "-c:v libx264 -crf 16 -preset slow -pix_fmt yuv420p -an highlights.mp4";

fn assemble_script(t: &Timeline, cfg: &Config) -> String {
    let mut s = String::from("#!/bin/sh\n# Склейка клипов в порядке показа. Запускать из папки, куда HLAE пишет clips/.\nset -e\n");
    let Some((inputs, filter, length)) = ffmpeg_parts(t, cfg) else {
        s.push_str("echo 'нет шотов'\n");
        return s;
    };
    let _ = write!(s, "ffmpeg -y {} \\\n", inputs.join(" "));
    if let Some(filter) = filter {
        let _ = writeln!(s, "  -filter_complex \"{filter}\" -map '[v]' \\");
    }
    let _ = writeln!(s, "  {ENCODE}");
    let _ = writeln!(s, "# ожидаемая длина: {length:.1} c");
    s
}

/// То же для Windows (cmd / двойной клик): пути с обратными слэшами, кавычки cmd.
fn assemble_bat(t: &Timeline, cfg: &Config) -> String {
    let mut s = String::from("@echo off\r\nrem Склейка клипов в порядке показа. Запускать из папки, куда HLAE пишет clips\\.\r\n");
    let Some((inputs, filter, length)) = ffmpeg_parts(t, cfg) else {
        s.push_str("echo нет шотов\r\n");
        return s;
    };
    let inputs: Vec<String> = inputs.iter().map(|i| i.replace('/', "\\")).collect();
    let _ = write!(s, "ffmpeg -y {}", inputs.join(" "));
    if let Some(filter) = filter {
        let _ = write!(s, " -filter_complex \"{filter}\" -map \"[v]\"");
    }
    let _ = write!(s, " {ENCODE}\r\nrem ожидаемая длина: {length:.1} c\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2::PlayerId;
    use director::Tier;
    use director::Shot;

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
        let m = Match::build("de_test".into(), 64.0, vec![(0, 0, "ace".into(), STEAM_ID64_BASE + 42)], &[]);
        let t = Timeline {
            map: "de_test".into(),
            tick_rate: 64.0,
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

    #[test]
    fn cfg_records_chronologically_with_play_order_names() {
        let (m, t) = setup();
        let out = render(&m, &t, &Config::default());
        let cfg = &out.files.iter().find(|(n, _)| n == "highlights.cfg").unwrap().1;
        let first = cfg.find("record name clips/shot_02").unwrap();
        let second = cfg.find("record name clips/shot_01").unwrap();
        assert!(first < second, "сначала пишем то, что раньше по тикам");
        assert!(cfg.contains("spec_player_by_accountid 42"));
        assert!(cfg.contains("mirv_cmd addAtTick 1200 \"host_timescale 0.250\""));
        assert!(cfg.contains("demo_gototick 4936"), "после шота 1000..1400 перематываем к 5000 с запасом секунды");
        assert!(cfg.contains("disconnect"));
    }

    #[test]
    fn zoom_becomes_mirv_fov_and_resets() {
        let (m, mut t) = setup();
        t.shots[1].zoom = vec![
            ScaleKey { tick: 1100, scale: 1.0 },
            ScaleKey { tick: 1110, scale: 0.5 },
            ScaleKey { tick: 1200, scale: 0.5 },
            ScaleKey { tick: 1210, scale: 1.0 },
        ];
        let out = render(&m, &t, &Config { base_fov: 90.0, ..Config::default() });
        let cfg = &out.files.iter().find(|(n, _)| n == "highlights.cfg").unwrap().1;
        assert!(cfg.contains("mirv_cmd addAtTick 1110 \"mirv_fov 45.0\""), "{cfg}");
        assert!(cfg.contains("mirv_cmd addAtTick 1210 \"mirv_fov default\""));
    }

    #[test]
    fn assemble_chains_xfades_with_correct_offsets() {
        let (m, t) = setup();
        let out = render(&m, &t, &Config { crossfade_secs: 0.5, ..Config::default() });
        let sh = &out.files.iter().find(|(n, _)| n == "assemble.sh").unwrap().1;
        let d0 = t.shots[0].output_secs(64.0);
        assert!(sh.contains(&format!("offset={:.3}", d0 - 0.5)), "{sh}");
        assert!(sh.contains("clips/shot_01.mp4") && sh.contains("clips/shot_02.mp4"));
    }

    #[test]
    fn windows_batch_script_uses_backslashes_and_cmd_quoting() {
        let (m, t) = setup();
        let out = render(&m, &t, &Config::default());
        let bat = &out.files.iter().find(|(n, _)| n == "assemble.bat").unwrap().1;
        assert!(bat.contains("-i clips\\shot_01.mp4") && !bat.contains("clips/"), "{bat}");
        assert!(bat.contains("-map \"[v]\"") && bat.contains("\r\n"));
        let sh = &out.files.iter().find(|(n, _)| n == "assemble.sh").unwrap().1;
        // одинаковый фильтр и ожидаемая длина в обеих версиях
        let filter = |s: &str| s.split("-filter_complex ").nth(1).unwrap().split(' ').next().unwrap().to_owned();
        assert_eq!(filter(sh), filter(bat));
    }

    #[test]
    fn free_camera_emits_campath_file() {
        let (m, mut t) = setup();
        let keys = vec![
            director::CamKey { tick: 64, pos: [1.0, 2.0, 3.0], ang: [0.0, 90.0, 0.0], fov: 90.0 },
            director::CamKey { tick: 128, pos: [4.0, 5.0, 6.0], ang: [0.0, 90.0, 0.0], fov: 80.0 },
        ];
        t.shots[0].segments[0].camera = Camera::Free { keys };
        let out = render(&m, &t, &Config::default());
        let xml = &out.files.iter().find(|(n, _)| n == "campath_01_1.xml").unwrap().1;
        assert!(xml.contains("t=\"1.0000\"") && xml.contains("t=\"2.0000\""));
    }
}
