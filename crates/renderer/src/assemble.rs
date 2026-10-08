//! Склейка клипов, записанных HLAE, в один ролик через ffmpeg.
//!
//! HLAE пишет каждый шот в `clips/shot_NN/takeMMMM/`: по умолчанию это последовательность картинок
//! (TGA/PNG/...), а при настроенном ffmpeg-профиле — видеофайл. Формат заранее неизвестен,
//! поэтому клип ищется по содержимому папки, а длительность считается по числу кадров.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const IMAGE_EXT: &[&str] = &["tga", "png", "jpg", "jpeg", "bmp", "tif", "tiff"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "avi", "mkv", "webm"];

#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Аргументы входа для ffmpeg.
    pub input: Vec<String>,
    pub secs: f32,
    pub frames: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub fps: u32,
    pub fade_secs: f32,
    pub out: PathBuf,
}

/// `00042` -> ("", 5 цифр, 42); `frame0007` -> ("frame", 4, 7).
fn split_stem(stem: &str) -> Option<(String, usize, u64)> {
    let digits = stem.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 18 {
        return None;
    }
    let (prefix, num) = stem.split_at(stem.len() - digits);
    Some((prefix.to_owned(), digits, num.parse().ok()?))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Последний `takeNNNN` в папке шота (повторные записи не перезаписывают старые); нет take — сама папка.
fn latest_take(shot_dir: &Path) -> PathBuf {
    let mut takes: Vec<PathBuf> = fs::read_dir(shot_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("take")))
        .collect();
    takes.sort();
    takes.pop().unwrap_or_else(|| shot_dir.to_path_buf())
}

#[derive(Debug, PartialEq)]
struct Sequence {
    pattern: String,
    start: u64,
    frames: usize,
}

/// Самая длинная непрерывная последовательность картинок среди `files`.
fn best_sequence(files: &[PathBuf]) -> Option<Sequence> {
    type Key = (PathBuf, String, usize, String);
    let mut groups: BTreeMap<Key, Vec<u64>> = BTreeMap::new();
    for f in files {
        let e = ext(f);
        if !IMAGE_EXT.contains(&e.as_str()) {
            continue;
        }
        let Some(stem) = f.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        let Some((prefix, width, n)) = split_stem(&stem) else { continue };
        groups.entry((f.parent()?.to_path_buf(), prefix, width, e)).or_default().push(n);
    }
    let mut best: Option<Sequence> = None;
    for ((dir, prefix, width, e), mut nums) in groups {
        nums.sort_unstable();
        nums.dedup();
        // непрерывный участок с начала: ffmpeg останавливается на первом пропуске
        let start = nums[0];
        let frames = nums.iter().enumerate().take_while(|(i, n)| **n == start + *i as u64).count();
        if best.as_ref().is_none_or(|b| frames > b.frames) {
            best = Some(Sequence {
                pattern: dir.join(format!("{prefix}%0{width}d.{e}")).to_string_lossy().into_owned(),
                start,
                frames,
            });
        }
    }
    best
}

fn video_secs(path: &Path) -> Result<f32, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .map_err(|e| format!("не удалось запустить ffprobe (он идёт вместе с ffmpeg): {e}"))?;
    String::from_utf8_lossy(&out.stdout).trim().parse().map_err(|_| format!("ffprobe не вернул длительность {}", path.display()))
}

/// Клип шота `index` (с нуля) из папки `root` (это `game/bin/win64` или сама папка `clips`).
pub fn find_clip(root: &Path, index: usize, fps: u32) -> Result<Clip, String> {
    let clips = if root.join("clips").is_dir() { root.join("clips") } else { root.to_path_buf() };
    let shot_dir = clips.join(format!("shot_{:02}", index + 1));
    if !shot_dir.is_dir() {
        return Err(format!("нет папки клипа {} — шот ещё не записан?", shot_dir.display()));
    }
    let take = latest_take(&shot_dir);
    let mut files = Vec::new();
    walk(&take, &mut files);

    let seq = best_sequence(&files);
    let video = files.iter().find(|f| VIDEO_EXT.contains(&ext(f).as_str()));
    match (seq, video) {
        (Some(s), _) if s.frames > 1 => Ok(Clip {
            input: vec![
                "-framerate".into(),
                fps.to_string(),
                "-start_number".into(),
                s.start.to_string(),
                "-i".into(),
                s.pattern,
            ],
            secs: s.frames as f32 / fps as f32,
            frames: Some(s.frames),
        }),
        (_, Some(v)) => Ok(Clip { input: vec!["-i".into(), v.to_string_lossy().into_owned()], secs: video_secs(v)?, frames: None }),
        _ => Err(format!("в {} нет ни кадров, ни видео (искал {IMAGE_EXT:?} и {VIDEO_EXT:?})", take.display())),
    }
}

/// Полная командная строка ffmpeg: все входы приводятся к одному fps и формату, между клипами — кроссфейд.
pub fn ffmpeg_args(clips: &[Clip], o: &Options) -> Vec<String> {
    let mut args: Vec<String> = vec!["-y".into()];
    for c in clips {
        args.extend(c.input.iter().cloned());
    }
    // кроссфейд не длиннее половины самого короткого клипа
    let shortest = clips.iter().map(|c| c.secs).fold(f32::INFINITY, f32::min);
    let fade = o.fade_secs.min(shortest / 2.0).max(0.0);

    let mut filter = String::new();
    for i in 0..clips.len() {
        filter += &format!("[{i}:v]fps={},format=yuv420p,setsar=1[v{i}];", o.fps);
    }
    let mut prev = "[v0]".to_owned();
    let mut length = clips.first().map_or(0.0, |c| c.secs);
    for (i, c) in clips.iter().enumerate().skip(1) {
        let label = if i == clips.len() - 1 { "[out]".to_owned() } else { format!("[x{i}]") };
        filter += &format!("{prev}[v{i}]xfade=transition=fade:duration={fade}:offset={:.3}{label};", length - fade);
        length += c.secs - fade;
        prev = label;
    }
    if clips.len() == 1 {
        filter += "[v0]null[out];";
    }
    filter.pop();
    args.extend(["-filter_complex".into(), filter, "-map".into(), "[out]".into()]);
    args.extend(["-c:v", "libx264", "-crf", "16", "-preset", "slow", "-pix_fmt", "yuv420p", "-an"].map(String::from));
    args.push(o.out.to_string_lossy().into_owned());
    args
}

#[derive(Debug)]
pub struct Report {
    pub clips: Vec<Clip>,
    pub total_secs: f32,
}

/// Находит клипы всех `shots` шотов в `root` и склеивает их в `o.out`.
pub fn assemble(root: &Path, shots: usize, o: &Options) -> Result<Report, String> {
    if shots == 0 {
        return Err("в плане нет шотов".into());
    }
    let clips: Vec<Clip> = (0..shots).map(|i| find_clip(root, i, o.fps)).collect::<Result<_, _>>()?;
    let fade = o.fade_secs.min(clips.iter().map(|c| c.secs).fold(f32::INFINITY, f32::min) / 2.0).max(0.0);
    let total_secs = clips.iter().map(|c| c.secs).sum::<f32>() - fade * (clips.len() - 1) as f32;

    let status = Command::new("ffmpeg")
        .args(ffmpeg_args(&clips, o))
        .status()
        .map_err(|e| format!("не удалось запустить ffmpeg (winget install Gyan.FFmpeg, затем перезапустите PowerShell): {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg завершился с ошибкой ({status})"));
    }
    Ok(Report { clips, total_secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs2cinema-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(dir: &Path, names: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        for n in names {
            fs::write(dir.join(n), b"x").unwrap();
        }
    }

    #[test]
    fn splits_frame_names() {
        assert_eq!(split_stem("00042"), Some((String::new(), 5, 42)));
        assert_eq!(split_stem("frame0007"), Some(("frame".into(), 4, 7)));
        assert_eq!(split_stem("audio"), None);
    }

    #[test]
    fn picks_the_longest_contiguous_sequence() {
        let d = tmp("seq");
        touch(&d.join("color"), &["00000.tga", "00001.tga", "00002.tga", "00003.tga"]);
        touch(&d.join("depth"), &["00000.tga", "00001.tga"]);
        touch(&d, &["audio.wav"]);
        let mut files = Vec::new();
        walk(&d, &mut files);
        let s = best_sequence(&files).unwrap();
        assert_eq!((s.frames, s.start), (4, 0));
        assert!(s.pattern.ends_with("%05d.tga") && s.pattern.contains("color"), "{}", s.pattern);
        // пропуск обрывает последовательность
        let d2 = tmp("gap");
        touch(&d2, &["0001.png", "0002.png", "0004.png"]);
        let mut files = Vec::new();
        walk(&d2, &mut files);
        let s = best_sequence(&files).unwrap();
        assert_eq!((s.frames, s.start), (2, 1));
    }

    #[test]
    fn takes_the_latest_take_and_counts_duration() {
        let root = tmp("take");
        touch(&root.join("clips/shot_01/take0000"), &["00000.tga"; 1]);
        let frames: Vec<String> = (0..120).map(|i| format!("{i:05}.tga")).collect();
        let names: Vec<&str> = frames.iter().map(String::as_str).collect();
        touch(&root.join("clips/shot_01/take0001"), &names);
        let clip = find_clip(&root, 0, 60).unwrap();
        assert_eq!(clip.frames, Some(120));
        assert!((clip.secs - 2.0).abs() < 1e-6);
        assert!(clip.input.contains(&"-start_number".to_owned()));
        // root может быть и самой папкой clips
        assert!(find_clip(&root.join("clips"), 0, 60).is_ok());
        assert!(find_clip(&root, 1, 60).unwrap_err().contains("shot_02"));
    }

    #[test]
    fn xfade_offsets_use_real_clip_durations() {
        let clip = |secs| Clip { input: vec!["-i".into(), "x".into()], secs, frames: None };
        let o = Options { fps: 60, fade_secs: 0.5, out: PathBuf::from("out.mp4") };
        let args = ffmpeg_args(&[clip(8.0), clip(6.0), clip(10.0)], &o);
        let filter = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        // 1-й переход на 8.0 - 0.5, 2-й на (8.0 + 6.0 - 0.5) - 0.5
        assert!(filter.contains("offset=7.500[x1]") && filter.contains("offset=13.000[out]"), "{filter}");
        assert!(filter.contains("[0:v]fps=60,format=yuv420p,setsar=1[v0]"));
        assert_eq!(args.last().unwrap(), "out.mp4");
        // кроссфейд не длиннее половины короткого клипа
        let short = ffmpeg_args(&[clip(0.4), clip(5.0)], &o);
        let f = &short[short.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(f.contains("duration=0.2:"), "{f}");
        // один клип — без xfade
        let one = ffmpeg_args(&[clip(3.0)], &o);
        let f = &one[one.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(!f.contains("xfade") && f.ends_with("[out]"), "{f}");
    }

    fn has_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
    }

    /// Сквозной тест с настоящим ffmpeg: 3 «записи HLAE» из PNG-кадров -> один ролик нужной длины.
    #[test]
    fn assembles_real_image_sequences_with_ffmpeg() {
        if !has_ffmpeg() {
            eprintln!("ffmpeg не найден — тест пропущен");
            return;
        }
        let root = tmp("e2e");
        for (i, secs) in [2u32, 3, 2].into_iter().enumerate() {
            let dir = root.join(format!("clips/shot_{:02}/take0000", i + 1));
            fs::create_dir_all(&dir).unwrap();
            let st = Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
                .arg(format!("testsrc=duration={secs}:size=160x90:rate=30"))
                .arg(dir.join("%05d.png"))
                .status()
                .unwrap();
            assert!(st.success());
        }
        let out = root.join("highlights.mp4");
        let o = Options { fps: 30, fade_secs: 0.5, out: out.clone() };
        let report = assemble(&root, 3, &o).unwrap();
        assert!((report.total_secs - (7.0 - 2.0 * 0.5)).abs() < 0.05, "{}", report.total_secs);
        let real = video_secs(&out).unwrap();
        assert!((real - report.total_secs).abs() < 0.15, "ожидали {}, получили {real}", report.total_secs);
    }
}
