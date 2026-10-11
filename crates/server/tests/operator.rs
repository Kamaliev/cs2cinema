//! Тесты бинарника `cs2-server`: команды оператора в отдельных процессах.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const ENV_KEYS: &[&str] = &[
    "CS2_SERVER_CONFIG",
    "CS2_SERVER_BIND",
    "CS2_SERVER_DATA_DIR",
    "CS2_SERVER_THREADS",
    "CS2_SERVER_LEASE_TTL_SECS",
    "CS2_SERVER_HEARTBEAT_SECS",
    "CS2_SERVER_LONGPOLL_SECS",
    "CS2_SERVER_MAX_GRANTS",
    "CS2_SERVER_STARTUP_GRACE_SECS",
    "CS2_SERVER_MAX_DEMO_MIB",
    "CS2_SERVER_MAX_RESULT_MIB",
    "CS2_SERVER_TELEGRAM_BOT_TOKEN",
    "TELEGRAM_BOT_TOKEN",
    "STATE_DIRECTORY",
];

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cs2-server"));
    for k in ENV_KEYS {
        c.env_remove(k);
    }
    c
}

fn test_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let d = std::env::temp_dir().join(format!("cs2srv-op-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fake_demo(path: &Path, size: usize) {
    let mut bytes = b"PBDEMS2\0".to_vec();
    let mut x = 12345u32;
    while bytes.len() < size {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bytes.push((x >> 24) as u8);
    }
    std::fs::write(path, bytes).unwrap();
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = bin().args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn submitted_demo_is_queued_and_seen_by_a_new_process() {
    let d = test_dir("submit");
    let data = d.join("data");
    let demo = d.join("match.dem");
    fake_demo(&demo, 3 * 1024 * 1024);
    let (code, out, err) = run(&[
        "submit",
        demo.to_str().unwrap(),
        "--player",
        "s1mple",
        "--data-dir",
        data.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("задача #1"), "{out}");
    assert!(out.contains("sha256"), "{out}");

    let (code, out, err) = run(&["job", "list", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("#1") && out.contains("queued") && out.contains("s1mple"),
        "{out}"
    );

    let stored = data.join("blobs/incoming/1/demo");
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        std::fs::read(&demo).unwrap(),
        "демка побайтно равна исходнику"
    );
    assert!(!stored.with_file_name("demo.part").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn garbage_file_is_refused_before_a_job_appears() {
    let d = test_dir("garbage");
    let data = d.join("data");
    let junk = d.join("junk.dem");
    std::fs::write(&junk, b"just some text, not a demo").unwrap();
    let (code, _, err) = run(&[
        "submit",
        junk.to_str().unwrap(),
        "--player",
        "s1mple",
        "--data-dir",
        data.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(
        err.contains("ошибка:") && err.contains("не похоже на демку CS2"),
        "{err}"
    );
    let (code, out, _) = run(&["job", "list", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(!out.contains("#1"), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn job_show_prints_the_demo_hash() {
    let d = test_dir("show");
    let data = d.join("data");
    let demo = d.join("match.dem");
    fake_demo(&demo, 64 * 1024);
    let (code, out, err) = run(&[
        "submit",
        demo.to_str().unwrap(),
        "--player",
        "s1mple",
        "--data-dir",
        data.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{err}");
    let hash = out.split("sha256 ").nth(1).unwrap().trim().to_owned();
    assert_eq!(hash.len(), 64);
    let (code, out, err) = run(&["job", "show", "1", "--data-dir", data.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("queued") && out.contains("s1mple") && out.contains(&hash),
        "{out}"
    );
    let _ = std::fs::remove_dir_all(&d);
}
