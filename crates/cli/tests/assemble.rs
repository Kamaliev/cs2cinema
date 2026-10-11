//! Бинарник `cs2-cli assemble <папка задачи>`: склейка живёт в `pipeline::assemble`, CLI лишь вызывает её.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

use pipeline::manifest::{RecordJson, TakeSpec};
use renderer::TakePlan;

/// Пустая временная папка задачи; id процесса и метка теста исключают столкновения.
fn job_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cs2cinema-cli-assemble-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `record.json` на два дубля по образцу задачи приложения: входы только `clips/takeNNNN/…`, последний аргумент `highlights.mp4`.
fn sample_record_json() -> RecordJson {
    let takes: Vec<TakePlan> = (0..2u32)
        .map(|index| TakePlan {
            index,
            shot: index as usize,
            game_secs: 5.0,
            capture_fps: 60,
        })
        .collect();
    let mut args: Vec<String> = vec!["-y".into()];
    for t in &takes {
        for f in TakePlan::FILES {
            args.extend(["-i".to_owned(), format!("clips/{}/{f}", t.dir_name())]);
        }
    }
    args.extend(["-map", "[v]", "highlights.mp4"].map(String::from));
    RecordJson {
        version: 1,
        nonce: "ab12cd34".into(),
        demo_path: PathBuf::from("match.dem"),
        demo_name: "cs2cinema_test".into(),
        demo_len: 12,
        frame: (1280, 720),
        fps: 60,
        take_nvenc: false,
        takes: takes.iter().map(TakeSpec::from).collect(),
        ffmpeg_args: args,
        video_secs: 10.0,
        shots: takes.len(),
        secs: 10.0,
        plan_hash: "h".into(),
        created_unix: 1,
    }
}

fn run_assemble(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cs2-cli"))
        .arg("assemble")
        .args(args)
        .output()
        .expect("cs2-cli запускается")
}

#[test]
fn assemble_without_manifest_fails_with_take_invalid_before_ffmpeg() {
    let dir = job_dir("no-manifest");
    sample_record_json().save(&dir).unwrap();
    let out = run_assemble(&[&dir]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("TAKE_INVALID"), "{stderr}");
    assert!(stderr.contains("манифест дублей не найден"), "{stderr}");
    assert!(
        !dir.join("highlights.mp4").exists() && !dir.join("highlights.part.mp4").exists(),
        "ffmpeg не запускался"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn assemble_without_record_json_names_the_file() {
    let dir = job_dir("no-record");
    let out = run_assemble(&[&dir]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("record.json"), "{stderr}");
    let _ = std::fs::remove_dir_all(dir);
}
