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
}
