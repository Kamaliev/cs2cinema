//! Самоограничение памяти: тест перезапускает собственный бинарь, и дочерний процесс ограничивает
//! себя 256 МиБ. Родитель проверяет код выхода и время жизни ребёнка.
#![cfg(windows)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: u64 = 256 * 1024 * 1024;

/// Режим ребёнка из переменной окружения: `alloc:N` — выделить N МиБ.
fn child_mode() -> Option<String> {
    std::env::var("CS2C_SELF_LIMIT_CHILD").ok()
}

/// Выделяет `mib` МиБ кусками по 1 МиБ с записью в каждую страницу.
fn allocate(mib: usize) -> Vec<Vec<u8>> {
    let mut held = Vec::new();
    for _ in 0..mib {
        let mut chunk = vec![0u8; 1024 * 1024];
        for page in chunk.chunks_mut(4096) {
            page[0] = 1;
        }
        held.push(chunk);
    }
    held
}

/// Точка входа ребёнка; без переменной окружения — пустой проходящий тест.
#[test]
fn child_entry() {
    let Some(mode) = child_mode() else { return };
    let _limit = host::limit_self_memory(LIMIT).expect("самоограничение");
    if let Some(n) = mode.strip_prefix("alloc:") {
        let held = allocate(n.parse().expect("число МиБ"));
        drop(held);
    }
}

/// Запускает ребёнка с режимом, ждёт не больше 30 с; возвращает код выхода, вывод и время.
fn run_child(mode: &str) -> (Option<i32>, String, Duration) {
    let started = Instant::now();
    let mut child = Command::new(std::env::current_exe().expect("путь теста"))
        .args(["child_entry", "--exact", "--nocapture", "--test-threads=1"])
        .env("CS2C_SELF_LIMIT_CHILD", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("запуск ребёнка");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().expect("try_wait") {
            break Some(s);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read;
        let _ = so.read_to_string(&mut out);
    }
    (status.and_then(|s| s.code()), out, started.elapsed())
}

#[test]
fn allocation_within_the_limit_succeeds() {
    let (code, out, _) = run_child("alloc:100");
    assert_eq!(code, Some(0), "ребёнок упал в пределах лимита: {out}");
}

#[test]
fn allocation_over_the_limit_kills_the_child_quickly() {
    let (code, _, took) = run_child("alloc:600");
    assert!(
        matches!(code, Some(c) if c != 0),
        "ребёнок должен умереть с ненулевым кодом, получено {code:?}"
    );
    assert!(
        took < Duration::from_secs(10),
        "умирал слишком долго: {took:?}"
    );
}
