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

fn data_arg(d: &Path) -> String {
    d.join("data").to_str().unwrap().to_owned()
}

/// Токен — строка `cs2r_` + 64 hex в выводе `runner add`.
fn token_from(out: &str) -> String {
    out.lines()
        .map(str::trim)
        .find(|l| l.starts_with("cs2r_"))
        .expect("токен в выводе")
        .to_owned()
}

fn read_all_db_bytes(data: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for name in ["server.db", "server.db-wal"] {
        if let Ok(b) = std::fs::read(data.join(name)) {
            all.extend(b);
        }
    }
    all
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn runner_token_is_shown_once_and_only_its_hash_is_stored() {
    let d = test_dir("token");
    let data = data_arg(&d);
    let (code, out, err) = run(&["runner", "add", "main", "--data-dir", &data]);
    assert_eq!(code, 0, "{err}");
    let token = token_from(&out);
    assert_eq!(token.len(), 5 + 64);
    let bytes = read_all_db_bytes(&d.join("data"));
    assert!(
        !contains(&bytes, token.as_bytes()),
        "токен не должен лежать в базе"
    );
    let hash = server::auth::hash_token(&token);
    assert!(
        contains(&bytes, hash.as_bytes()),
        "хеш токена должен лежать в базе"
    );
    let (code, out, err) = run(&["runner", "list", "--data-dir", &data]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("main") && out.contains(&hash[..8]), "{out}");
    assert!(!out.contains(&token), "{out}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn revoked_token_no_longer_authenticates() {
    let d = test_dir("revoke");
    let data = data_arg(&d);
    let (_, out, _) = run(&["runner", "add", "main", "--data-dir", &data]);
    let token = token_from(&out);
    let db = server::db::Db::open(&d.join("data/server.db")).unwrap();
    assert!(server::auth::authenticate(&db, &token, 1).is_ok());
    drop(db);
    let (code, _, err) = run(&["runner", "revoke", "main", "--data-dir", &data]);
    assert_eq!(code, 0, "{err}");
    let db = server::db::Db::open(&d.join("data/server.db")).unwrap();
    assert!(matches!(
        server::auth::authenticate(&db, &token, 2),
        Err(server::auth::AuthError::Revoked { .. })
    ));
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn runners_users_and_jobs_survive_a_restart() {
    let d = test_dir("restart");
    let data = data_arg(&d);
    let demo = d.join("match.dem");
    fake_demo(&demo, 64 * 1024);
    let (code, _, err) = run(&[
        "user",
        "add",
        "alice",
        "--tg",
        "42",
        "--admin",
        "--data-dir",
        &data,
    ]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = run(&["runner", "add", "main", "--data-dir", &data]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = run(&[
        "submit",
        demo.to_str().unwrap(),
        "--player",
        "s1mple",
        "--user",
        "alice",
        "--data-dir",
        &data,
    ]);
    assert_eq!(code, 0, "{err}");

    let (_, users, _) = run(&["user", "list", "--data-dir", &data]);
    assert!(
        users.contains("alice") && users.contains("admin") && users.contains("42"),
        "{users}"
    );
    let (_, runners, _) = run(&["runner", "list", "--data-dir", &data]);
    assert!(runners.contains("main"), "{runners}");
    let (_, jobs, _) = run(&["job", "list", "--data-dir", &data]);
    assert!(jobs.contains("#1") && jobs.contains("alice"), "{jobs}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn submit_for_an_unknown_user_is_refused() {
    let d = test_dir("nouser");
    let data = data_arg(&d);
    let demo = d.join("match.dem");
    fake_demo(&demo, 64 * 1024);
    let (code, _, err) = run(&[
        "submit",
        demo.to_str().unwrap(),
        "--player",
        "s1mple",
        "--user",
        "nobody",
        "--data-dir",
        &data,
    ]);
    assert_eq!(code, 1);
    assert!(
        err.contains("ошибка: пользователь «nobody» не найден"),
        "{err}"
    );
    let (_, jobs, _) = run(&["job", "list", "--data-dir", &data]);
    assert!(!jobs.contains("#1"), "{jobs}");
    let _ = std::fs::remove_dir_all(&d);
}
