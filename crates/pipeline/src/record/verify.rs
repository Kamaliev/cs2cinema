//! Проверка дубля ffprobe: что годно и почему нет (D-01..D-03).
//!
//! Дубль годен, только если у `video.mp4` есть видеопоток нужного размера, у `audio.wav` есть
//! аудиопоток (звук обязателен), обе длительности больше нуля и в допуске от плановой. Вывод
//! ffprobe — недоверенный текст чужого процесса: он ограничен по длине и разбирается типизированно.

use std::path::Path;
use std::time::Duration;

use host::{CmdSpec, Host, HostError};
use renderer::TakePlan;
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

/// Потолок stdout одного вызова ffprobe: настоящий ответ по одному файлу — меньше килобайта.
pub const PROBE_STDOUT_MAX: usize = 64 * 1024;

/// Итог проверки дубля.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeOutcome {
    /// Дубль годен.
    Valid(TakeFacts),
    /// Файла нет или ffmpeg HLAE ещё дописывает его: повторять до `Deadlines::take_finalize`.
    NotReady(String),
    /// Файл прочитан и негоден, либо ffprobe не работает.
    Invalid(String),
    /// Пользователь отменил задачу.
    Cancelled,
}

/// Проверить дубль `take`: размеры файлов, два вызова ffprobe через `host` (с дедлайном и отменой)
/// и решение [`judge`]. `clips` — папка `clips/` задачи, `frame` — заданный размер кадра.
pub fn probe_take(
    host: &dyn Host,
    ffprobe: &Path,
    clips: &Path,
    take: &TakePlan,
    frame: (u32, u32),
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> ProbeOutcome {
    if cancel() {
        return ProbeOutcome::Cancelled;
    }
    let dir = take.dir_name();
    let mut sizes = [0u64; 2];
    for (size, file) in sizes.iter_mut().zip(TakePlan::FILES) {
        match std::fs::metadata(clips.join(&dir).join(file)) {
            Ok(m) if m.is_file() => *size = m.len(),
            _ => return ProbeOutcome::NotReady(format!("{dir}/{file}: файла нет")),
        }
    }
    let mut probes = Vec::with_capacity(2);
    for file in TakePlan::FILES {
        let path = clips.join(&dir).join(file);
        match probe_file(host, ffprobe, &path, file, deadline, cancel) {
            Ok(p) => probes.push(p),
            Err(outcome) => return outcome,
        }
    }
    match judge(
        &probes[0],
        &probes[1],
        take.game_secs,
        frame,
        (sizes[0], sizes[1]),
    ) {
        Ok(facts) => ProbeOutcome::Valid(facts),
        Err(reason) => ProbeOutcome::Invalid(reason),
    }
}

/// Один вызов ffprobe по файлу; `Err` — готовый исход, при котором дальше идти нельзя.
fn probe_file(
    host: &dyn Host,
    ffprobe: &Path,
    path: &Path,
    file: &str,
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<Probe, ProbeOutcome> {
    let spec = CmdSpec {
        program: ffprobe.to_path_buf(),
        args: probe_args(path),
        cwd: None,
        deadline,
    };
    let mut text = String::new();
    let mut too_long = false;
    let result = host.run(&spec, cancel, &mut |line| {
        if text.len() + line.len() + 1 > PROBE_STDOUT_MAX {
            too_long = true;
            return;
        }
        text.push_str(line);
        text.push('\n');
    });
    let out = match result {
        Ok(out) => out,
        Err(HostError::Cancelled) => return Err(ProbeOutcome::Cancelled),
        Err(HostError::Timeout) => {
            return Err(ProbeOutcome::Invalid(format!(
                "{file}: ffprobe не уложился в {} с",
                deadline.as_secs()
            )));
        }
        Err(e) => {
            return Err(ProbeOutcome::Invalid(format!(
                "ffprobe не запустился: {}",
                one_line(&e.to_string())
            )));
        }
    };
    if out.code != Some(0) {
        let tail: Vec<String> = out
            .stderr_tail
            .iter()
            .map(|l| one_line(l.trim()))
            .filter(|l| !l.trim().is_empty())
            .collect();
        let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
        let lower = tail.to_lowercase();
        let unfinished = ["moov atom not found", "invalid data found", "no such file"]
            .iter()
            .any(|m| lower.contains(m));
        return Err(if unfinished {
            ProbeOutcome::NotReady(format!("{file}: ffprobe не прочитал файл: {tail}"))
        } else {
            let code = out
                .code
                .map_or_else(|| "процесс остановлен".to_owned(), |c| format!("код {c}"));
            ProbeOutcome::Invalid(format!("{file}: ffprobe завершился ({code}): {tail}"))
        });
    }
    if too_long {
        return Err(ProbeOutcome::Invalid(format!(
            "{file}: ответ ffprobe слишком длинный (больше {} КиБ)",
            PROBE_STDOUT_MAX / 1024
        )));
    }
    parse_probe(&text).map_err(|e| ProbeOutcome::Invalid(format!("{file}: {e}")))
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

    // --- probe_take на фейковом хосте ---

    use host::CmdOutput;
    use host::fake::{FakeHost, Scenario};

    const BOTH: [&str; 1] = [
        r#"{"streams":[{"codec_type":"video","width":1280,"height":720,"duration":"5.0"},{"codec_type":"audio","duration":"5.0"}],"format":{"duration":"5.000000"}}"#,
    ];

    fn plan() -> TakePlan {
        TakePlan {
            index: 0,
            shot: 0,
            game_secs: 5.0,
            capture_fps: 60,
        }
    }

    /// Папка `clips/` с файлами дубля 0 (содержимое не важно: читает фейк).
    fn clips(name: &str) -> std::path::PathBuf {
        let clips = std::env::temp_dir()
            .join(format!("cs2cinema-verify-{}-{name}", std::process::id()))
            .join("clips");
        let _ = std::fs::remove_dir_all(&clips);
        std::fs::create_dir_all(clips.join("take0000")).unwrap();
        std::fs::write(clips.join("take0000/video.mp4"), b"video-bytes").unwrap();
        std::fs::write(clips.join("take0000/audio.wav"), b"audio").unwrap();
        clips
    }

    fn exit(code: i32, stderr: &[&str]) -> Result<CmdOutput, HostError> {
        Ok(CmdOutput {
            code: Some(code),
            stderr_tail: stderr.iter().map(|s| (*s).to_owned()).collect(),
            elapsed: Duration::ZERO,
        })
    }

    fn probe_with(
        name: &str,
        scenario: Scenario,
        cancel: &dyn Fn() -> bool,
    ) -> (ProbeOutcome, FakeHost) {
        let host = FakeHost::new(scenario);
        let out = probe_take(
            &host,
            Path::new("ffprobe"),
            &clips(name),
            &plan(),
            (1280, 720),
            Duration::from_secs(30),
            cancel,
        );
        (out, host)
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn take_with_both_streams_is_valid_and_carries_file_sizes() {
        let sc = Scenario::happy(1).program("ffprobe", &BOTH, exit(0, &[]));
        let (out, host) = probe_with("valid", sc, &never);
        let ProbeOutcome::Valid(f) = out else {
            panic!("ожидался Valid, получено {out:?}");
        };
        assert_eq!((f.video_bytes, f.audio_bytes), (11, 5));
        assert_eq!((f.width, f.height), (1280, 720));
        let runs = host.runs();
        assert_eq!(runs.len(), 2, "по вызову на файл");
        assert!(runs[0].2.last().unwrap().ends_with("video.mp4"));
        assert!(runs[1].2.last().unwrap().ends_with("audio.wav"));
    }

    #[test]
    fn unfinished_file_is_not_ready_but_other_failures_are_invalid() {
        let sc = Scenario::happy(1).program(
            "ffprobe",
            &[],
            exit(1, &[r"C:\Users\kamal\x\video.mp4: moov atom not found"]),
        );
        let (out, _) = probe_with("moov", sc, &never);
        let ProbeOutcome::NotReady(why) = out else {
            panic!("ожидался NotReady, получено {out:?}");
        };
        assert!(
            why.contains("video.mp4") && why.contains("moov atom not found"),
            "{why}"
        );
        assert!(!why.contains("kamal"), "профиль скрыт: {why}");

        let sc = Scenario::happy(1).program("ffprobe", &[], exit(1, &["Unsupported codec"]));
        let (out, _) = probe_with("other", sc, &never);
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("Unsupported codec")),
            "{out:?}"
        );
    }

    #[test]
    fn missing_file_is_not_ready_and_ffprobe_is_not_started() {
        let host = FakeHost::new(Scenario::happy(1));
        let empty =
            std::env::temp_dir().join(format!("cs2cinema-verify-{}-none", std::process::id()));
        let out = probe_take(
            &host,
            Path::new("ffprobe"),
            &empty,
            &plan(),
            (1280, 720),
            Duration::from_secs(30),
            &never,
        );
        assert!(
            matches!(out, ProbeOutcome::NotReady(ref s) if s.contains("take0000/video.mp4: файла нет")),
            "{out:?}"
        );
        assert!(host.runs().is_empty());
    }

    #[test]
    fn host_errors_map_to_invalid_and_cancel_stays_cancel() {
        let sc = Scenario::happy(1).program("ffprobe", &[], Err(HostError::Timeout));
        let (out, _) = probe_with("timeout", sc, &never);
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("не уложился в 30 с")),
            "{out:?}"
        );

        let sc =
            Scenario::happy(1).program("ffprobe", &[], Err(HostError::Spawn("нет файла".into())));
        let (out, _) = probe_with("spawn", sc, &never);
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("не запустился")),
            "{out:?}"
        );

        let sc = Scenario::happy(1).program("ffprobe", &[], Err(HostError::Cancelled));
        let (out, _) = probe_with("cancelled", sc, &never);
        assert_eq!(out, ProbeOutcome::Cancelled);

        let (out, host) = probe_with("raised", Scenario::happy(1), &|| true);
        assert_eq!(out, ProbeOutcome::Cancelled);
        assert!(
            host.runs().is_empty(),
            "поднятая отмена не запускает ffprobe"
        );
    }

    #[test]
    fn oversized_stdout_is_cut_and_reported_without_panic() {
        let big = "x".repeat(1000);
        let lines: Vec<&str> = (0..100).map(|_| big.as_str()).collect();
        let sc = Scenario::happy(1).program("ffprobe", &lines, exit(0, &[]));
        let (out, _) = probe_with("big", sc, &never);
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("слишком длинный")),
            "{out:?}"
        );
    }

    #[test]
    fn fake_ffprobe_responder_flags_truncated_and_silent_takes() {
        let (out, _) = probe_with("auto-ok", Scenario::happy(1), &never);
        assert!(matches!(out, ProbeOutcome::Valid(_)), "{out:?}");
        let (out, _) = probe_with("auto-cut", Scenario::happy(1).truncated_take(0), &never);
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("длительность")),
            "{out:?}"
        );
        let (out, _) = probe_with(
            "auto-mute",
            Scenario::happy(1).take_without_audio(0),
            &never,
        );
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("нет аудиопотока")),
            "{out:?}"
        );
        let (out, _) = probe_with(
            "auto-size",
            Scenario::happy(1).video_size(1920, 1080),
            &never,
        );
        assert!(
            matches!(out, ProbeOutcome::Invalid(ref s) if s.contains("1920x1080")),
            "{out:?}"
        );
    }
}
