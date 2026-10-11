//! Охрана зависимостей сервера: он не тянет крейты Windows-части воркспейса, а `wire` — вообще ничего своего.

const SERVER_MANIFEST: &str = include_str!("../Cargo.toml");
const WIRE_MANIFEST: &str = include_str!("../../wire/Cargo.toml");

const WORKSPACE_CRATES: &[&str] = &[
    "host",
    "pipeline",
    "demo",
    "store",
    "cs2",
    "positions",
    "geometry",
    "app",
    "cli",
    "renderer",
    "director",
    "highlights",
    "faceit",
    "runner",
];

/// Строки зависимостей из секций `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`:
/// (имя, вся строка).
fn dependencies(manifest: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = matches!(
                line,
                "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
            );
            continue;
        }
        if inside && !line.is_empty() && !line.starts_with('#') {
            if let Some((name, _)) = line.split_once('=') {
                out.push((name.trim().to_owned(), line.to_owned()));
            }
        }
    }
    out
}

#[test]
fn server_depends_only_on_wire_inside_the_workspace() {
    let deps = dependencies(SERVER_MANIFEST);
    assert!(!deps.is_empty(), "секция зависимостей не прочитана");
    let with_path: Vec<_> = deps.iter().filter(|(_, l)| l.contains("path =")).collect();
    assert_eq!(
        with_path.len(),
        1,
        "path-зависимости сервера: {with_path:?}"
    );
    assert_eq!(with_path[0].0, "wire");
    assert!(with_path[0].1.contains("../wire"), "{:?}", with_path[0]);
    for (name, _) in &deps {
        assert!(
            !WORKSPACE_CRATES.contains(&name.as_str()),
            "сервер не должен зависеть от {name}"
        );
    }
}

#[test]
fn wire_has_no_workspace_dependencies() {
    let deps = dependencies(WIRE_MANIFEST);
    assert!(!deps.is_empty(), "секция зависимостей не прочитана");
    assert!(
        !WIRE_MANIFEST.contains("path ="),
        "wire не должен зависеть от крейтов воркспейса"
    );
    for (name, _) in &deps {
        assert!(
            name == "serde" || name == "serde_json",
            "лишняя зависимость wire: {name}"
        );
    }
}
