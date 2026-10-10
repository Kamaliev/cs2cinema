//! Проверка дубля ffprobe: что годно и почему нет (D-01..D-03).
//!
//! Дубль годен, только если у `video.mp4` есть видеопоток нужного размера, у `audio.wav` есть
//! аудиопоток (звук обязателен), обе длительности больше нуля и в допуске от плановой. Вывод
//! ffprobe — недоверенный текст чужого процесса: он ограничен по длине и разбирается типизированно.

use std::path::Path;

use serde::Deserialize;

use super::deadlines::{TAKE_TOLERANCE_MIN_SECS, TAKE_TOLERANCE_REL};
use super::log::one_line;

/// Аргументы ffprobe для одного файла дубля. Путь — последний аргумент (на этом держится ответчик
/// фейкового хоста).
pub fn probe_args(path: &Path) -> Vec<String> {
    [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,duration:format=duration",
        "-i",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(path.to_string_lossy().into_owned()))
    .collect()
}

/// Один поток из ответа ffprobe.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stream {
    /// `video`, `audio` и т. д.; пусто, если ffprobe не назвал.
    pub codec_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Длительность потока, секунды; `None` — нет, не число или отрицательная.
    pub duration: Option<f32>,
}

/// Разобранный ответ ffprobe по одному файлу.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    pub streams: Vec<Stream>,
    /// Длительность контейнера (`format.duration`), секунды.
    pub duration: Option<f32>,
}

/// Число или строка: ffprobe отдаёт длительность строкой (`"5.000000"`), но число тоже терпим.
#[derive(Deserialize)]
#[serde(untagged)]
enum Secs {
    Text(String),
    Number(f64),
}

impl Secs {
    /// Конечная неотрицательная длительность; всё прочее (`N/A`, NaN, минус) — «нет».
    fn get(&self) -> Option<f32> {
        let v = match self {
            Secs::Text(s) => s.trim().parse::<f64>().ok()?,
            Secs::Number(n) => *n,
        };
        (v.is_finite() && v >= 0.0).then_some(v as f32)
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<Secs>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawFormat {
    duration: Option<Secs>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawProbe {
    streams: Option<Vec<RawStream>>,
    format: Option<RawFormat>,
}

/// Разбор ответа ffprobe. Пустой текст и `{ }` — пустой [`Probe`] (потоков нет); не JSON — ошибка.
pub fn parse_probe(text: &str) -> Result<Probe, String> {
    if text.trim().is_empty() {
        return Ok(Probe::default());
    }
    let raw: RawProbe = serde_json::from_str(text).map_err(|e| {
        let head: String = text.chars().take(120).collect();
        format!(
            "ffprobe вернул не JSON: {e}; начало ответа: {}",
            one_line(&head)
        )
    })?;
    Ok(Probe {
        streams: raw
            .streams
            .unwrap_or_default()
            .into_iter()
            .map(|s| Stream {
                codec_type: s.codec_type.unwrap_or_default(),
                width: s.width,
                height: s.height,
                duration: s.duration.and_then(|d| d.get()),
            })
            .collect(),
        duration: raw.format.and_then(|f| f.duration).and_then(|d| d.get()),
    })
}

/// Допуск длительности дубля от плановой, секунды: 10%, но не меньше 0.5 с (D-01).
pub fn tolerance(expected: f32) -> f32 {
    (expected.max(0.0) * TAKE_TOLERANCE_REL).max(TAKE_TOLERANCE_MIN_SECS)
}

/// Укладывается ли измеренная длительность в допуск от плановой. NaN и отрицательное — нет.
pub fn within_tolerance(expected: f32, measured: f32) -> bool {
    if !expected.is_finite() || !measured.is_finite() || measured < 0.0 {
        return false;
    }
    (measured - expected).abs() <= tolerance(expected)
}

/// Что известно о годном дубле: измеренное и размеры файлов.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeFacts {
    pub video_secs: f32,
    pub audio_secs: f32,
    pub width: u32,
    pub height: u32,
    pub video_bytes: u64,
    pub audio_bytes: u64,
}

fn find<'a>(p: &'a Probe, kind: &str) -> Option<&'a Stream> {
    p.streams.iter().find(|s| s.codec_type == kind)
}

/// Длительность файла: контейнера, а если её нет — потока.
fn length(p: &Probe, s: &Stream) -> Option<f32> {
    p.duration.or(s.duration)
}

/// Решение «годен»: `Err` — причина одной фразой с именем файла и числами. `sizes` — размеры
/// `video.mp4` и `audio.wav` в байтах, `frame` — заданный размер кадра.
pub fn judge(
    video: &Probe,
    audio: &Probe,
    expected_secs: f32,
    frame: (u32, u32),
    sizes: (u64, u64),
) -> Result<TakeFacts, String> {
    let v = find(video, "video").ok_or("video.mp4: нет видеопотока")?;
    let a = find(audio, "audio").ok_or("audio.wav: нет аудиопотока")?;
    let video_secs = length(video, v).filter(|s| *s > 0.0);
    let video_secs = video_secs.ok_or("video.mp4: длительность не определена или равна нулю")?;
    let audio_secs = length(audio, a).filter(|s| *s > 0.0);
    let audio_secs = audio_secs.ok_or("audio.wav: длительность не определена или равна нулю")?;
    for (file, secs) in [("video.mp4", video_secs), ("audio.wav", audio_secs)] {
        if !within_tolerance(expected_secs, secs) {
            return Err(format!(
                "{file}: длительность {secs:.1} с, ожидалась {expected_secs:.1} с (допуск ±{:.1} с)",
                tolerance(expected_secs)
            ));
        }
    }
    let (w, h) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
    if (w, h) != frame {
        let seen = if w == 0 || h == 0 {
            "неизвестно".to_owned()
        } else {
            format!("{w}x{h}")
        };
        return Err(format!(
            "video.mp4: разрешение {seen}, ожидалось {}x{}",
            frame.0, frame.1
        ));
    }
    Ok(TakeFacts {
        video_secs,
        audio_secs,
        width: w,
        height: h,
        video_bytes: sizes.0,
        audio_bytes: sizes.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"{
    "programs": [],
    "streams": [
        {
            "codec_type": "video",
            "codec_name": "h264",
            "width": 1280,
            "height": 720,
            "duration": "5.000000"
        }
    ],
    "format": {
        "duration": "5.000000"
    }
}"#;

    fn video(w: u32, h: u32, secs: f32) -> Probe {
        Probe {
            streams: vec![Stream {
                codec_type: "video".into(),
                width: Some(w),
                height: Some(h),
                duration: Some(secs),
            }],
            duration: Some(secs),
        }
    }

    fn audio(secs: f32) -> Probe {
        Probe {
            streams: vec![Stream {
                codec_type: "audio".into(),
                width: None,
                height: None,
                duration: Some(secs),
            }],
            duration: Some(secs),
        }
    }

    #[test]
    fn probe_args_end_with_the_file_path() {
        let args = probe_args(Path::new("C:/out/clips/take0000/video.mp4"));
        assert!(args.iter().any(|a| a == "-show_entries"));
        assert_eq!(args.last().unwrap(), "C:/out/clips/take0000/video.mp4");
    }

    #[test]
    fn tolerance_is_ten_percent_but_not_below_half_a_second() {
        assert!(within_tolerance(5.0, 5.4));
        assert!(within_tolerance(5.0, 4.6));
        assert!(!within_tolerance(5.0, 4.4));
        assert!(within_tolerance(20.0, 21.9));
        assert!(!within_tolerance(20.0, 22.1));
        assert!(!within_tolerance(5.0, f32::NAN));
        assert!(!within_tolerance(5.0, -1.0));
        assert!(!within_tolerance(f32::NAN, 5.0));
    }

    #[test]
    fn real_ffprobe_answer_parses_into_a_video_stream() {
        let p = parse_probe(REAL).unwrap();
        assert_eq!(p.streams.len(), 1);
        assert_eq!(p.streams[0].codec_type, "video");
        assert_eq!(p.streams[0].width, Some(1280));
        assert_eq!(p.duration, Some(5.0));
    }

    #[test]
    fn empty_answer_has_no_streams_and_garbage_is_an_error() {
        assert_eq!(parse_probe("{ }").unwrap(), Probe::default());
        assert_eq!(parse_probe("").unwrap(), Probe::default());
        assert_eq!(
            parse_probe(r#"{"streams": null, "format": {"duration": "N/A"}}"#).unwrap(),
            Probe::default()
        );
        let e = parse_probe("это не json\u{7}").unwrap_err();
        assert!(e.contains("ffprobe вернул не JSON"), "{e}");
        assert!(!e.contains('\u{7}'), "управляющие символы вычищены: {e:?}");
    }

    #[test]
    fn good_take_is_judged_valid_with_its_facts() {
        let f = judge(
            &video(1280, 720, 5.0),
            &audio(5.0),
            5.0,
            (1280, 720),
            (10, 20),
        )
        .unwrap();
        assert_eq!((f.width, f.height), (1280, 720));
        assert_eq!((f.video_bytes, f.audio_bytes), (10, 20));
        assert_eq!((f.video_secs, f.audio_secs), (5.0, 5.0));
    }

    #[test]
    fn each_defect_has_its_own_russian_reason_with_numbers() {
        let frame = (1280, 720);
        let no_audio = Probe::default();
        let e = judge(&video(1280, 720, 5.0), &no_audio, 5.0, frame, (1, 1)).unwrap_err();
        assert_eq!(e, "audio.wav: нет аудиопотока");
        let e = judge(&Probe::default(), &audio(5.0), 5.0, frame, (1, 1)).unwrap_err();
        assert_eq!(e, "video.mp4: нет видеопотока");
        let e = judge(&video(1280, 720, 1.5), &audio(5.0), 5.0, frame, (1, 1)).unwrap_err();
        assert_eq!(
            e,
            "video.mp4: длительность 1.5 с, ожидалась 5.0 с (допуск ±0.5 с)"
        );
        let e = judge(&video(1280, 720, 5.0), &audio(1.0), 5.0, frame, (1, 1)).unwrap_err();
        assert!(e.starts_with("audio.wav: длительность 1.0 с"), "{e}");
        let e = judge(&video(1920, 1080, 5.0), &audio(5.0), 5.0, frame, (1, 1)).unwrap_err();
        assert_eq!(e, "video.mp4: разрешение 1920x1080, ожидалось 1280x720");
        let e = judge(&video(1280, 720, 0.0), &audio(5.0), 5.0, frame, (1, 1)).unwrap_err();
        assert!(e.contains("равна нулю"), "{e}");
    }

    #[test]
    fn container_duration_wins_over_the_stream_duration() {
        let mut v = video(1280, 720, 9.0);
        v.duration = Some(5.0);
        assert!(judge(&v, &audio(5.0), 5.0, (1280, 720), (1, 1)).is_ok());
        v.duration = None;
        assert!(judge(&v, &audio(5.0), 5.0, (1280, 720), (1, 1)).is_err());
    }
}
