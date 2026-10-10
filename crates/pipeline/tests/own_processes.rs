//! Страж TMO-05: приложение закрывает только свои процессы - по handle и Job Object, а не по имени.

use std::path::{Path, PathBuf};

/// Все `*.rs` под `dir`, кроме `target`.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_process_is_ever_stopped_by_image_name() {
    // имена утилит собираются из частей, чтобы этот файл сам их не содержал
    let forbidden = [
        ["task", "kill"].concat(),
        ["task", "list"].concat(),
        ["Stop", "-Process"].concat(),
        ["pk", "ill"].concat(),
        ["kill", "all"].concat(),
    ];
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = vec![];
    rust_sources(&crates, &mut files);
    assert!(
        files.len() > 20,
        "обход крейтов не нашёл исходников: {}",
        files.len()
    );

    let mut found = vec![];
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let text = text.to_lowercase();
        for word in &forbidden {
            if text.contains(&word.to_lowercase()) {
                found.push(format!("{}: {word}", file.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "процессы нельзя завершать или перечислять системными утилитами по имени: {found:#?}"
    );
}

#[test]
fn host_trait_stops_a_process_only_by_its_full_identity() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../host/src/lib.rs");
    let text = std::fs::read_to_string(&path).expect("crates/host/src/lib.rs читается");
    assert!(text.contains("pub trait Host"), "трейт Host не найден");
    assert!(
        text.contains("pub created: u64"),
        "в идентичности процесса нет времени создания"
    );
    let killers: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("fn ") && l.contains("&self"))
        .filter(|l| l.to_lowercase().contains("kill"))
        .collect();
    assert!(
        killers.len() <= 1,
        "в трейтах хоста больше одного метода завершения: {killers:?}"
    );
    let bad: Vec<&&str> = killers
        .iter()
        .filter(|l| !l.contains("&ProcIdentity") || l.contains("pid:") || l.contains("name"))
        .collect();
    assert!(
        bad.is_empty(),
        "метод завершения должен принимать только &ProcIdentity, а не PID или имя: {bad:?}"
    );
}
