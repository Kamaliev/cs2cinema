//! Склейка ролика из проверенных дублей: один код для приложения и `cs2-cli assemble` (TST-03).
//!
//! Склеиваются только дубли, которые манифест `clips/manifest.json` называет `validated` и чьи файлы
//! не менялись после проверки. ffmpeg пишет во временный `highlights.part.mp4`, и только после кода 0
//! файл переименовывается в `highlights.mp4`: недописанный ролик не выглядит результатом.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use host::{CmdSpec, Host, HostError};
use renderer::TakePlan;

use crate::{
    CANCELLED, FailCode, Progress, RecordFailure, RecordState, RunError,
    manifest::{Manifest, TakeStatus},
    record::failure,
};

/// Последний аргумент склейки — `highlights.mp4`; ролик пишется во временный `highlights.part.mp4` (D-06).
pub fn part_args(args: &[String]) -> Result<Vec<String>, String> {
    match args.last() {
        Some(last) if last == "highlights.mp4" => {
            let mut v = args.to_vec();
            if let Some(l) = v.last_mut() {
                *l = PART_NAME.to_owned();
            }
            Ok(v)
        }
        _ => Err(format!("неожиданные аргументы склейки: {}", args.join(" "))),
    }
}

/// Временное имя ролика: расширение `.mp4` сохранено — по нему ffmpeg выбирает формат.
pub const PART_NAME: &str = "highlights.part.mp4";

/// Дубль `take` годен к склейке, только если манифест называет его `validated`, оба файла на месте и их размеры
/// равны записанным (TAK-04, TAK-05, T-03-16).
fn check_take_for_assembly(m: &Manifest, clips: &Path, take: &TakePlan) -> Result<(), String> {
    let dir = take.dir_name();
    let Some(e) = m.entry(take.index) else {
        return Err("дубля нет в манифесте".to_owned());
    };
    if e.status != TakeStatus::Validated {
        let status = match e.status {
            TakeStatus::Pending => "не записан",
            TakeStatus::Recorded => "записан, но не проверен",
            TakeStatus::Validated => "проверен",
            TakeStatus::Invalid => "негоден",
        };
        return Err(match &e.error {
            Some(why) => format!("{status} ({why})"),
            None => status.to_owned(),
        });
    }
    for (file, want) in TakePlan::FILES.iter().zip([e.video_bytes, e.audio_bytes]) {
        let have = std::fs::metadata(clips.join(&dir).join(file))
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
        match (have, want) {
            (None, _) => return Err(format!("файл {file} пропал")),
            (Some(h), Some(w)) if h != w => {
                return Err(format!("размер {file} изменился: {h} байт вместо {w}"));
            }
            (Some(_), None) => return Err(format!("размер {file} в манифесте неизвестен")),
            _ => {}
        }
    }
    Ok(())
}

/// Склейка только из проверенных дублей и через временное имя (TAK-04..06, D-06): ffmpeg пишет `highlights.part.mp4`,
/// переименование в `highlights.mp4` — после кода 0. При сбое удаляется только `.part`, прежний ролик остаётся.
pub fn assemble_stage(
    host: &dyn Host,
    out_dir: &Path,
    takes: &[TakePlan],
    args: &[String],
    secs: f32,
    deadline: Duration,
    p: &dyn Progress,
) -> Result<PathBuf, RunError> {
    let clips = out_dir.join("clips");
    let Some(manifest) = Manifest::load(&clips) else {
        let f = take_invalid_failure(
            1,
            takes.len() as u32,
            &takes.first().map(TakePlan::dir_name).unwrap_or_default(),
            "манифест дублей не найден — запишите задачу заново",
        );
        return Err(RunError {
            message: f.message.clone(),
            fail: Some(f),
        });
    };
    let total = takes.len() as u32;
    for take in takes {
        if let Err(why) = check_take_for_assembly(&manifest, &clips, take) {
            let f = take_invalid_failure(
                take.index + 1,
                total,
                &take.dir_name(),
                &format!("не проверен по манифесту: {why}"),
            );
            return Err(RunError {
                message: f.message.clone(),
                fail: Some(f),
            });
        }
    }
    let part_args = part_args(args)?;
    let (part, out) = (out_dir.join(PART_NAME), out_dir.join("highlights.mp4"));
    p.log("склеиваю ролик через ffmpeg…");
    if let Err(e) = run_ffmpeg(host, out_dir, &part_args, secs, deadline, p) {
        // недописанный ролик не должен выглядеть результатом (Pitfall 11); прежний highlights.mp4 не трогаем
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    if !part.is_file() {
        return Err(format!("ffmpeg не создал {PART_NAME}").into());
    }
    std::fs::rename(&part, &out).map_err(|e| format!("не удалось заменить highlights.mp4: {e} — закройте плеер с прошлым роликом и нажмите «Повторить»"))?;
    p.log(&format!("ролик готов: {}", out.display()));
    Ok(out)
}

/// Типизированный сбой «дубль негоден»: состояние `Take`, сообщение и подсказка с номером дубля.
pub fn take_invalid_failure(n: u32, total: u32, dir: &str, reason: &str) -> RecordFailure {
    let mut f = RecordFailure::new(
        FailCode::TakeInvalid,
        RecordState::Take,
        failure::msg_take_invalid(n, total, dir, reason),
        Duration::ZERO,
    );
    f.hint = failure::hint_take_invalid(n, dir);
    f
}

/// Текст ошибки запуска утилиты склейки по-русски.
pub fn ffmpeg_error(e: &HostError, deadline: Duration) -> String {
    match e {
        HostError::Timeout => format!(
            "ffmpeg не уложился в {} с — склейка прервана; увеличьте «Запас по времени» в настройках",
            deadline.as_secs()
        ),
        HostError::Cancelled => CANCELLED.to_owned(),
        HostError::Spawn(e) => format!("не удалось запустить ffmpeg (он должен быть в PATH): {e}"),
        e => format!("ffmpeg: {e}"),
    }
}

/// Склейка: ffmpeg в `dir` через `Host::run` — с дедлайном и отменой; прогресс берётся из `-progress pipe:1`
/// (`out_time_us` в микросекундах готового ролика) и раз в 10 с пишется в журнал.
pub fn run_ffmpeg(
    host: &dyn Host,
    dir: &Path,
    args: &[String],
    secs: f32,
    deadline: Duration,
    p: &dyn Progress,
) -> Result<(), String> {
    let mut all: Vec<String> = [
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
    ]
    .map(String::from)
    .into();
    all.extend_from_slice(args);
    let cmd = CmdSpec {
        program: "ffmpeg".into(),
        args: all,
        cwd: Some(dir.to_path_buf()),
        deadline,
    };
    let started = std::time::Instant::now();
    let mut logged = 0u64;
    let mut on_line = |line: &str| {
        let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.trim().parse::<u64>().ok())
        else {
            return;
        };
        let tick = started.elapsed().as_secs() / 10;
        if tick > logged && secs > 0.0 {
            logged = tick;
            let pct = (us as f32 / 1e6 / secs * 100.0).min(100.0);
            p.log(&format!("склейка: {pct:.0}%"));
        }
    };
    let out = host
        .run(&cmd, &|| p.cancelled(), &mut on_line)
        .map_err(|e| ffmpeg_error(&e, deadline))?;
    if out.code != Some(0) {
        let tail: Vec<&str> = out
            .stderr_tail
            .iter()
            .rev()
            .take(6)
            .map(String::as_str)
            .collect();
        return Err(format!(
            "ffmpeg завершился с ошибкой: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    p.log(&format!(
        "склейка заняла {:.0} с",
        started.elapsed().as_secs_f32()
    ));
    Ok(())
}

// ---- Проверка готового ролика ------------------------------------------------------------------

/// Аргументы ffprobe для готового ролика. Путь — последний аргумент (как в `record::verify::probe_args`).
pub fn result_probe_args(path: &Path) -> Vec<String> {
    [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_entries",
        "stream=codec_type,codec_name,pix_fmt,width,height,duration:format=format_name,duration",
        "-i",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(path.to_string_lossy().into_owned()))
    .collect()
}

/// Что идёт раньше в MP4: `moov` (`true`, ролик играет без докачки) или `mdat` (`false`).
/// Читаются только заголовки боксов верхнего уровня (`File` + `Seek`), весь файл в память не берётся.
pub fn is_faststart(path: &Path) -> Result<bool, String> {
    use std::io::{Read, Seek, SeekFrom};
    let broken = |why: &str| format!("ролик повреждён: {why}");
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("не удалось открыть {}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut pos = 0u64;
    loop {
        if pos.saturating_add(8) > len {
            return Err(broken("в файле нет ни moov, ни mdat"));
        }
        f.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head).map_err(|e| e.to_string())?;
        let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let kind = [head[4], head[5], head[6], head[7]];
        let (header, size) = match size32 {
            // 64-битный размер следует сразу за типом
            1 => {
                let mut big = [0u8; 8];
                f.read_exact(&mut big)
                    .map_err(|_| broken("бокс с 64-битным размером обрезан"))?;
                (16, u64::from_be_bytes(big))
            }
            // последний бокс тянется до конца файла
            0 => (8, len - pos),
            n => (8, u64::from(n)),
        };
        if size < header || pos.saturating_add(size) > len {
            return Err(broken(&format!(
                "бокс {} выходит за пределы файла",
                String::from_utf8_lossy(&kind)
            )));
        }
        match &kind {
            b"moov" => return Ok(true),
            b"mdat" => return Ok(false),
            _ => pos += size,
        }
    }
}

/// Факты о годном ролике.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultFacts {
    pub size: u64,
    pub duration: f32,
    pub width: u32,
    pub height: u32,
}

/// Длительность ffprobe: строкой (`"5.000000"`) или числом.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Secs {
    Text(String),
    Number(f64),
}

impl Secs {
    /// Конечная положительная длительность; всё прочее (`N/A`, NaN, ноль, минус) — «нет».
    fn get(&self) -> Option<f32> {
        let v = match self {
            Secs::Text(s) => s.trim().parse::<f64>().ok()?,
            Secs::Number(n) => *n,
        };
        (v.is_finite() && v > 0.0).then_some(v as f32)
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ResStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<Secs>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ResFormat {
    format_name: Option<String>,
    duration: Option<Secs>,
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ResProbe {
    streams: Option<Vec<ResStream>>,
    format: Option<ResFormat>,
}

/// Годен ли ролик для Telegram: H.264 + yuv420p + звук + контейнер mp4 + длительность > 0 + faststart.
/// Чистая функция над недоверенным ответом ffprobe; `Err` — причина по-русски (код `RESULT_INVALID` добавляет раннер).
pub fn judge_result(probe_json: &str, size: u64, faststart: bool) -> Result<ResultFacts, String> {
    use crate::record::log::one_line;
    if size == 0 {
        return Err("файл ролика пуст".to_owned());
    }
    let raw: ResProbe = serde_json::from_str(probe_json).map_err(|e| {
        let head: String = probe_json.chars().take(120).collect();
        format!(
            "ffprobe вернул не JSON: {e}; начало ответа: {}",
            one_line(&head)
        )
    })?;
    let streams = raw.streams.unwrap_or_default();
    let format = raw.format.unwrap_or_default();
    let is_mp4 = format
        .format_name
        .as_deref()
        .is_some_and(|n| n.split(',').any(|p| p.trim() == "mp4"));
    if !is_mp4 {
        return Err(format!(
            "контейнер не mp4 ({})",
            one_line(format.format_name.as_deref().unwrap_or("неизвестен"))
        ));
    }
    let Some(video) = streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
    else {
        return Err("в ролике нет видеопотока".to_owned());
    };
    let codec = video.codec_name.as_deref().unwrap_or("неизвестен");
    if codec != "h264" {
        return Err(format!(
            "видео не H.264 ({}): Telegram такой ролик не проиграет",
            one_line(codec)
        ));
    }
    let pix = video.pix_fmt.as_deref().unwrap_or("неизвестен");
    if pix != "yuv420p" {
        return Err(format!(
            "формат пикселей не yuv420p ({}): Telegram такой ролик не проиграет",
            one_line(pix)
        ));
    }
    if !streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"))
    {
        return Err("в ролике нет звука".to_owned());
    }
    let duration = format
        .duration
        .as_ref()
        .and_then(Secs::get)
        .or_else(|| video.duration.as_ref().and_then(Secs::get));
    let Some(duration) = duration else {
        return Err("длительность ролика нулевая или неизвестна".to_owned());
    };
    if !faststart {
        return Err(
            "ролик без faststart (moov после mdat): в Telegram он не начнёт играть до полной загрузки"
                .to_owned(),
        );
    }
    Ok(ResultFacts {
        size,
        duration,
        width: video.width.unwrap_or_default(),
        height: video.height.unwrap_or_default(),
    })
}

/// Проверить готовый `highlights.mp4` перед загрузкой: размер, порядок боксов и ffprobe через `host`
/// (с дедлайном и отменой). Негодный ролик не удаляется — он остаётся уликой у вызывающего.
pub fn verify_result(
    host: &dyn Host,
    ffprobe: &Path,
    path: &Path,
    deadline: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<ResultFacts, String> {
    use crate::record::log::one_line;
    let size = std::fs::metadata(path)
        .map_err(|e| format!("ролик не найден: {e}"))?
        .len();
    let faststart = is_faststart(path)?;
    let spec = CmdSpec {
        program: ffprobe.to_path_buf(),
        args: result_probe_args(path),
        cwd: None,
        deadline,
    };
    let mut text = String::new();
    let mut too_long = false;
    let out = host
        .run(&spec, cancel, &mut |line| {
            if text.len() + line.len() + 1 > crate::record::verify::PROBE_STDOUT_MAX {
                too_long = true;
                return;
            }
            text.push_str(line);
            text.push('\n');
        })
        .map_err(|e| match e {
            HostError::Timeout => format!("ffprobe не уложился в {} с", deadline.as_secs()),
            HostError::Cancelled => CANCELLED.to_owned(),
            e => format!("ffprobe не запустился: {}", one_line(&e.to_string())),
        })?;
    if out.code != Some(0) {
        let tail: Vec<String> = out
            .stderr_tail
            .iter()
            .map(|l| one_line(l.trim()))
            .filter(|l| !l.trim().is_empty())
            .collect();
        let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
        return Err(match out.code {
            Some(c) => format!("ffprobe завершился с кодом {c}: {tail}"),
            None => format!("ffprobe остановлен: {tail}"),
        });
    }
    if too_long {
        return Err("ответ ffprobe слишком длинный".to_owned());
    }
    judge_result(&text, size, faststart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::fake::{FakeHost, Scenario};

    fn assemble_with(r: Result<host::CmdOutput, HostError>) -> Result<(), String> {
        let host = FakeHost::new(Scenario::happy(1).run_result(r));
        run_ffmpeg(
            &host,
            Path::new("."),
            &[],
            10.0,
            Duration::from_secs(600),
            &crate::Quiet,
        )
    }

    #[test]
    fn ffmpeg_failures_are_explained_in_russian() {
        let timeout = assemble_with(Err(HostError::Timeout)).unwrap_err();
        assert!(
            timeout.starts_with("ffmpeg не уложился в 600 с"),
            "{timeout}"
        );
        assert!(timeout.contains("Запас по времени"), "{timeout}");
        assert_eq!(
            assemble_with(Err(HostError::Cancelled)).unwrap_err(),
            CANCELLED
        );
        let spawn = assemble_with(Err(HostError::Spawn("нет файла".into()))).unwrap_err();
        assert!(spawn.contains("должен быть в PATH"), "{spawn}");
        let failed = host::CmdOutput {
            code: Some(1),
            stderr_tail: (1..=8).map(|i| format!("строка {i}")).collect(),
            elapsed: Duration::ZERO,
        };
        let msg = assemble_with(Ok(failed)).unwrap_err();
        assert_eq!(
            msg,
            "ffmpeg завершился с ошибкой: строка 3 | строка 4 | строка 5 | строка 6 | строка 7 | строка 8"
        );
        let ok = host::CmdOutput {
            code: Some(0),
            stderr_tail: vec![],
            elapsed: Duration::ZERO,
        };
        assert!(assemble_with(Ok(ok)).is_ok());
    }

    #[test]
    fn part_args_swaps_only_the_final_highlights_name() {
        let args: Vec<String> = ["-y", "-i", "clips/take0000/video.mp4", "highlights.mp4"]
            .map(String::from)
            .into();
        let part = part_args(&args).unwrap();
        assert_eq!(part.last().map(String::as_str), Some("highlights.part.mp4"));
        assert_eq!(part[..3], args[..3], "остальные аргументы не тронуты");
        let other: Vec<String> = ["-y", "result.mp4"].map(String::from).into();
        assert!(
            part_args(&other)
                .unwrap_err()
                .contains("неожиданные аргументы склейки")
        );
        assert!(part_args(&[]).is_err());
    }

    // ---- проверка готового ролика ----

    /// Бокс MP4: размер (big-endian), тип и тело из `body_len` нулей.
    fn box_bytes(kind: &[u8; 4], body_len: usize) -> Vec<u8> {
        let mut v = ((body_len + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend(std::iter::repeat_n(0u8, body_len));
        v
    }

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cs2cinema-result-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn mp4(order: &[&[u8; 4]]) -> Vec<u8> {
        order.iter().flat_map(|k| box_bytes(k, 16)).collect()
    }

    #[test]
    fn faststart_is_detected_from_box_order() {
        let good = temp_file("good.mp4", &mp4(&[b"ftyp", b"moov", b"mdat"]));
        assert_eq!(is_faststart(&good), Ok(true));
        let slow = temp_file("slow.mp4", &mp4(&[b"ftyp", b"mdat", b"moov"]));
        assert_eq!(is_faststart(&slow), Ok(false));

        // бокс с 64-битным размером перед moov
        let mut big = box_bytes(b"ftyp", 8);
        big.extend_from_slice(&1u32.to_be_bytes());
        big.extend_from_slice(b"free");
        big.extend_from_slice(&(16u64 + 4).to_be_bytes());
        big.extend_from_slice(&[0u8; 4]);
        big.extend(box_bytes(b"moov", 4));
        let big = temp_file("big.mp4", &big);
        assert_eq!(is_faststart(&big), Ok(true));

        // size == 0 у последнего бокса: до конца файла (mdat найден раньше конца)
        let mut zero = box_bytes(b"ftyp", 8);
        zero.extend_from_slice(&0u32.to_be_bytes());
        zero.extend_from_slice(b"mdat");
        zero.extend_from_slice(&[7u8; 20]);
        let zero = temp_file("zero.mp4", &zero);
        assert_eq!(is_faststart(&zero), Ok(false));
        // size == 0 у не-moov/mdat бокса: дальше читать нечего
        let mut tail = box_bytes(b"ftyp", 8);
        tail.extend_from_slice(&0u32.to_be_bytes());
        tail.extend_from_slice(b"free");
        tail.extend_from_slice(&[0u8; 12]);
        let tail = temp_file("tail.mp4", &tail);
        assert!(is_faststart(&tail).unwrap_err().contains("повреждён"));

        let short = temp_file("short.mp4", b"abc");
        assert!(is_faststart(&short).unwrap_err().contains("повреждён"));
        let mut over = box_bytes(b"ftyp", 8);
        over.extend_from_slice(&1000u32.to_be_bytes());
        over.extend_from_slice(b"moov");
        let over = temp_file("over.mp4", &over);
        assert!(is_faststart(&over).unwrap_err().contains("за пределы"));
        assert!(is_faststart(Path::new("нет-такого-файла.mp4")).is_err());
    }

    fn probe_json(codec: &str, pix: &str, audio: bool, format: &str, duration: &str) -> String {
        let audio = if audio {
            r#",{"codec_type":"audio","codec_name":"aac","duration":"5.000000"}"#
        } else {
            ""
        };
        format!(
            r#"{{"streams":[{{"codec_type":"video","codec_name":"{codec}","pix_fmt":"{pix}","width":1280,"height":720,"duration":"{duration}"}}{audio}],"format":{{"format_name":"{format}","duration":"{duration}"}}}}"#
        )
    }

    const MP4_FORMAT: &str = "mov,mp4,m4a,3gp,3g2,mj2";

    #[test]
    fn judge_result_accepts_only_h264_yuv420p_with_audio_in_mp4() {
        let ok = probe_json("h264", "yuv420p", true, MP4_FORMAT, "5.000000");
        let f = judge_result(&ok, 1234, true).unwrap();
        assert_eq!((f.size, f.width, f.height), (1234, 1280, 720));
        assert!((f.duration - 5.0).abs() < 1e-4);

        let bad = |json: &str, size: u64, fast: bool| judge_result(json, size, fast).unwrap_err();
        let e = bad(
            &probe_json("hevc", "yuv420p", true, MP4_FORMAT, "5"),
            9,
            true,
        );
        assert!(e.contains("H.264"), "{e}");
        let e = bad(
            &probe_json("h264", "yuv444p", true, MP4_FORMAT, "5"),
            9,
            true,
        );
        assert!(e.contains("yuv420p"), "{e}");
        let e = bad(
            &probe_json("h264", "yuv420p", false, MP4_FORMAT, "5"),
            9,
            true,
        );
        assert!(e.contains("звук"), "{e}");
        let e = bad(
            &probe_json("h264", "yuv420p", true, MP4_FORMAT, "0"),
            9,
            true,
        );
        assert!(e.contains("длительност"), "{e}");
        let no_duration = r#"{"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p"},{"codec_type":"audio"}],"format":{"format_name":"mov,mp4"}}"#;
        assert!(bad(no_duration, 9, true).contains("длительност"));
        let e = bad(
            &probe_json("h264", "yuv420p", true, "matroska,webm", "5"),
            9,
            true,
        );
        assert!(e.contains("mp4"), "{e}");
        assert!(bad(&ok, 0, true).contains("пуст"));
        let e = bad(&ok, 9, false);
        assert!(e.contains("faststart"), "{e}");
        let garbage = format!("не json\u{7}{}", "я".repeat(500));
        let e = bad(&garbage, 9, true);
        assert!(e.contains("не JSON"), "{e}");
        assert!(e.chars().count() < 260 && !e.contains('\u{7}'), "{e}");
    }

    fn exit(code: Option<i32>, stderr: &[&str]) -> Result<host::CmdOutput, HostError> {
        Ok(host::CmdOutput {
            code,
            stderr_tail: stderr.iter().map(|s| (*s).to_owned()).collect(),
            elapsed: Duration::ZERO,
        })
    }

    #[test]
    fn verify_result_runs_ffprobe_through_the_host() {
        let file = temp_file("verify.mp4", &mp4(&[b"ftyp", b"moov", b"mdat"]));
        let json = probe_json("h264", "yuv420p", true, MP4_FORMAT, "5.000000");
        let lines: Vec<&str> = json.lines().collect();
        let run = |sc: Scenario| {
            verify_result(
                &FakeHost::new(sc),
                Path::new("ffprobe"),
                &file,
                Duration::from_secs(30),
                &|| false,
            )
        };
        let ok = run(Scenario::happy(1).program("ffprobe", &lines, exit(Some(0), &[])));
        assert_eq!(ok.unwrap().width, 1280);
        let failed =
            run(Scenario::happy(1).program("ffprobe", &[], exit(Some(1), &["Invalid data"])));
        let e = failed.unwrap_err();
        assert!(e.contains("ffprobe") && e.contains("Invalid data"), "{e}");
        let timeout = run(Scenario::happy(1).run_result(Err(HostError::Timeout))).unwrap_err();
        assert!(timeout.contains("ffprobe"), "{timeout}");

        // порядок боксов проверяется до ffprobe
        let slow = temp_file("verify-slow.mp4", &mp4(&[b"ftyp", b"mdat", b"moov"]));
        let host = FakeHost::new(Scenario::happy(1).program("ffprobe", &lines, exit(Some(0), &[])));
        let e = verify_result(
            &host,
            Path::new("ffprobe"),
            &slow,
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap_err();
        assert!(e.contains("faststart"), "{e}");
    }

    #[test]
    fn real_ffmpeg_output_is_faststart_and_verified() {
        let host = host::default_host();
        let ffmpeg = |args: Vec<String>| {
            host.run(
                &CmdSpec {
                    program: "ffmpeg".into(),
                    args,
                    cwd: None,
                    deadline: Duration::from_secs(60),
                },
                &|| false,
                &mut |_| {},
            )
        };
        if !matches!(ffmpeg(vec!["-version".into()]), Ok(o) if o.code == Some(0)) {
            eprintln!("ffmpeg не запускается — тест пропущен");
            return;
        }
        let dir = std::env::temp_dir().join(format!("cs2cinema-real-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let make = |name: &str, faststart: bool| -> PathBuf {
            let out = dir.join(name);
            let mut args: Vec<String> = [
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=d=1:s=320x240:r=25",
                "-f",
                "lavfi",
                "-i",
                "sine=d=1",
            ]
            .map(String::from)
            .into();
            let enc: Vec<String> = renderer::Encoder::X264
                .args()
                .split_whitespace()
                .map(String::from)
                .collect();
            let mut enc: Vec<String> = if faststart {
                enc
            } else {
                let at = enc.iter().position(|a| a == "-movflags").unwrap();
                enc[..at].to_vec()
            };
            args.append(&mut enc);
            args.push(out.to_string_lossy().into_owned());
            if !faststart {
                // последним аргументом должен стоять выходной файл, а не `highlights.mp4`
                assert!(!args.iter().any(|a| a == "highlights.mp4"));
            } else {
                let n = args.len();
                args.remove(n - 2); // `highlights.mp4` заменён выходным файлом
            }
            let o = ffmpeg(args).unwrap();
            assert_eq!(o.code, Some(0), "{:?}", o.stderr_tail);
            out
        };
        let fast = make("fast.mp4", true);
        assert_eq!(is_faststart(&fast), Ok(true));
        let f = verify_result(
            host.as_ref(),
            Path::new("ffprobe"),
            &fast,
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap();
        assert!(f.duration > 0.5 && f.size > 0, "{f:?}");
        let slow = make("slow.mp4", false);
        let e = verify_result(
            host.as_ref(),
            Path::new("ffprobe"),
            &slow,
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap_err();
        assert!(e.contains("faststart"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
