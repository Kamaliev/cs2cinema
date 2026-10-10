//! Настоящие строки консоли CS2 против парсера меток и ворот «демка идёт».
//!
//! Строки — из спайков S1–S3 (`01-SPIKES.md`, фикстуры `tests/fixtures/s3*.log`). Фикстуры читаются
//! через `lines()`: `.gitattributes` может превратить LF в CRLF при проверке из git.

use host::gate::{DemoGate, Verdict};
use pipeline::record::markers::{Marker, is_afxerror, is_demo_running_evidence, parse_marker};

const NONCE: &str = "ab12cd34";
/// Код задачи, под которым стенд спайков писал метки в фикстуры.
const SPIKE_NONCE: &str = "spike";

/// Выдержка настоящих строк консоли (пути профиля заменены на `USER`).
const REAL_LINES: &[&str] = &[
    "\u{feff}10/09 01:37:32 [CommandLine] command line arguments: -insecure -steam -novid -condebug",
    "[Demo] Requesting playback of 'C:/Users/USER/Downloads/x.dem'",
    "[HostStateManager] Host activate: Playing Demo (cs2cinema_1-x.dem)",
    "ChangeGameUIState: CSGO_GAME_UI_STATE_LOADINGSCREEN -> CSGO_GAME_UI_STATE_INGAME",
    "AFXERROR: COutFFMPEGVideoStream::COutFFMPEGVideoStream: CreateProcessW.",
    "AFXERROR: Failed writing image for screen recording.",
    "[InputService] exec: couldn't exec '{*}cfg/gamemode_competitive_server.cfg', unable to read file",
];

/// Фикстура целиком: `(источник, текст)` для строк консоли и `(источник, текст)` служебных строк стенда.
fn fixture(name: &str) -> Vec<(String, String)> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    text.lines()
        .filter_map(|l| {
            // формат стенда: `+<секунды> [<источник>] <текст>`
            let rest = l.strip_prefix('+')?;
            let (_, rest) = rest.split_once(' ')?;
            let rest = rest.strip_prefix('[')?;
            let (source, text) = rest.split_once("] ")?;
            Some((source.to_owned(), text.to_owned()))
        })
        .collect()
}

const FIXTURES: &[&str] = &[
    "s3a_missing_demo.log",
    "s3b_empty_demo.log",
    "s3c_missing_cfg.log",
    "s3e_late_connect.log",
];

#[test]
fn real_console_lines_are_never_markers() {
    for line in REAL_LINES {
        assert_eq!(parse_marker(line, NONCE), None, "{line:?} не метка");
    }
    let mut total = 0;
    for name in FIXTURES {
        for (source, text) in fixture(name) {
            total += 1;
            assert_eq!(
                parse_marker(&text, NONCE),
                None,
                "{name} [{source}]: {text:?} не метка чужого nonce"
            );
        }
    }
    // фикстуры лежат в репозитории; пустой результат значил бы, что формат строк не разобран
    assert!(total > 500, "фикстуры не прочитаны: {total} строк");
}

#[test]
fn demo_running_evidence_is_recognised() {
    assert!(is_demo_running_evidence(REAL_LINES[2]), "Host activate");
    assert!(is_demo_running_evidence(REAL_LINES[3]), "-> INGAME");
    assert!(
        !is_demo_running_evidence(REAL_LINES[1]),
        "Requesting playback"
    );
    assert!(is_afxerror(REAL_LINES[4]));
    assert!(is_afxerror(REAL_LINES[5]));
    assert!(!is_afxerror(REAL_LINES[6]));
}

/// Строки консоли фикстуры (netcon и console.log) — то, что видит сессия; служебные строки стенда нет.
fn session_lines(name: &str) -> Vec<String> {
    fixture(name)
        .into_iter()
        .filter(|(source, _)| source == "netcon" || source == "console.log")
        .map(|(_, text)| text)
        .collect()
}

#[test]
fn ready_before_the_demo_is_dropped_in_the_s3c_transcript() {
    let mut gate = DemoGate::new(SPIKE_NONCE);
    let (mut dropped, mut passed) = (0, 0);
    for line in session_lines("s3c_missing_cfg.log") {
        let is_ready = parse_marker(&line, SPIKE_NONCE) == Some(Marker::Ready);
        match gate.admit(&line) {
            Verdict::Drop => {
                assert!(is_ready, "ворота отбросили не READY: {line:?}");
                assert!(!gate.demo_seen());
                dropped += 1;
            }
            Verdict::Pass if is_ready => {
                assert!(gate.demo_seen(), "READY пропущен до подтверждения демки");
                passed += 1;
            }
            Verdict::Pass => {}
        }
    }
    assert_eq!(dropped, 1, "ложный READY на меню-карте (S3c) отбрасывается");
    assert_eq!(passed, 1, "второй READY, уже на демке, проходит");
}

#[test]
fn failed_demo_transcripts_never_open_the_gate() {
    for name in ["s3a_missing_demo.log", "s3b_empty_demo.log"] {
        let mut gate = DemoGate::new(SPIKE_NONCE);
        let lines = session_lines(name);
        assert!(lines.len() > 50, "{name}: фикстура не прочитана");
        for line in &lines {
            assert_eq!(gate.admit(line), Verdict::Pass, "{name}: {line:?}");
        }
        assert!(!gate.demo_seen(), "{name}: демка не пошла, ворота закрыты");
    }
}

#[test]
fn late_connect_transcript_opens_the_gate_on_the_demo_lines() {
    let mut gate = DemoGate::new(SPIKE_NONCE);
    for line in session_lines("s3e_late_connect.log") {
        assert_eq!(gate.admit(&line), Verdict::Pass, "{line:?}");
    }
    assert!(gate.demo_seen());
}

#[test]
fn gate_marker_tag_is_the_renderer_tag() {
    assert_eq!(host::gate::MARKER_TAG, renderer::MARKER_TAG);
}
