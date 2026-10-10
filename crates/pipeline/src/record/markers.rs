//! Метки консоли `CS2C <nonce> …`: разбор строк игры. Формат общий с генератором скриптов
//! (`renderer::marker`), поэтому здесь нет собственных строковых шаблонов метки.
//!
//! Строки консоли недоверенные: в них эхо наших команд, трассировка `mirv_cmd` и строки старых
//! запусков. Метка принимается, только когда ВСЯ строка — это `CS2C <nonce> <вид> [n]`.

/// Команда, запускающая скрипт записи. Отправляется ровно один раз после READY (D-10).
pub const EXEC: &str = "exec highlights";

/// Метка из консоли игры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Ready,
    Script,
    TakeStart(u32),
    TakeEnd(u32),
    AllDone,
}

impl Marker {
    /// Как метка выглядит в консоли (без `CS2C <nonce>`): для сообщений о сбое.
    pub fn label(self) -> String {
        match self {
            Marker::Ready => "READY".to_owned(),
            Marker::Script => "SCRIPT".to_owned(),
            Marker::TakeStart(n) => format!("TAKE_START {n}"),
            Marker::TakeEnd(n) => format!("TAKE_END {n}"),
            Marker::AllDone => "ALL_DONE".to_owned(),
        }
    }
}

/// Разобрать строку консоли. `None` — это не наша метка (чужой nonce, эхо, лишние слова).
pub fn parse_marker(line: &str, nonce: &str) -> Option<Marker> {
    let s = line.trim_start_matches('\u{feff}').trim_start();
    let s = strip_timestamp(s).trim();
    let mut it = s.split_whitespace();
    if it.next()? != renderer::MARKER_TAG || it.next()? != nonce {
        return None;
    }
    let kind = it.next()?;
    let arg = it.next();
    if it.next().is_some() {
        return None;
    }
    match (kind, arg) {
        ("READY", None) => Some(Marker::Ready),
        ("SCRIPT", None) => Some(Marker::Script),
        ("ALL_DONE", None) => Some(Marker::AllDone),
        ("TAKE_START", Some(n)) => number(n).map(Marker::TakeStart),
        ("TAKE_END", Some(n)) => number(n).map(Marker::TakeEnd),
        _ => None,
    }
}

/// Только цифры: `+3` и `-1` меткой не считаются.
fn number(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Снять префикс времени `MM/DD HH:MM:SS ` (ровно этот вид), если он есть.
pub fn strip_timestamp(s: &str) -> &str {
    let b = s.as_bytes();
    let digits = [0, 1, 3, 4, 6, 7, 9, 10, 12, 13];
    let seps = [(2, b'/'), (5, b' '), (8, b':'), (11, b':'), (14, b' ')];
    if b.len() >= 15
        && digits.iter().all(|&i| b[i].is_ascii_digit())
        && seps.iter().all(|&(i, c)| b[i] == c)
    {
        &s[15..]
    } else {
        s
    }
}

/// Запрос READY: срабатывает на 64-м тике идущей демки, поэтому метка значит «демка загрузилась».
pub fn ready_request(nonce: &str) -> String {
    format!(
        "mirv_cmd clear; mirv_cmd addAtTick 64 \"echo {}\"",
        renderer::marker(nonce, "READY")
    )
}

/// Запрос READY напрямую, без привязки к тику: когда демка уже идёт, а `addAtTick 64` не сработает.
pub fn ready_direct(nonce: &str) -> String {
    format!("echo {}", renderer::marker(nonce, "READY"))
}

/// Строка консоли говорит, что демка уже идёт.
pub fn is_demo_running_evidence(line: &str) -> bool {
    line.contains("Host activate: Playing Demo") || line.contains("-> CSGO_GAME_UI_STATE_INGAME")
}

/// Строка — ошибка HLAE (`AFXERROR`).
pub fn is_afxerror(line: &str) -> bool {
    line.contains("AFXERROR")
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = "ab12cd34";

    #[test]
    fn marker_parser_accepts_only_whole_lines_with_our_nonce() {
        assert_eq!(parse_marker("CS2C ab12cd34 READY", N), Some(Marker::Ready));
        assert_eq!(
            parse_marker("CS2C ab12cd34 SCRIPT", N),
            Some(Marker::Script)
        );
        assert_eq!(
            parse_marker("CS2C ab12cd34 ALL_DONE", N),
            Some(Marker::AllDone)
        );
        assert_eq!(
            parse_marker("CS2C ab12cd34 TAKE_START 0", N),
            Some(Marker::TakeStart(0))
        );
        assert_eq!(
            parse_marker("\u{feff}10/09 01:37:32 CS2C ab12cd34 TAKE_END 3", N),
            Some(Marker::TakeEnd(3)),
            "BOM и префикс времени снимаются"
        );
        for bad in [
            "echo CS2C ab12cd34 READY",
            "mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C ab12cd34 READY\"",
            "3: 64 (eps=1) -> echo CS2C ab12cd34 READY",
            "CS2C ffff0000 READY",
            "CS2C ab12cd34 TAKE_END",
            "CS2C ab12cd34 READY extra",
            "CS2C ab12cd34 TAKE_END +3",
            "CS2C ab12cd34 TAKE_END 3 4",
            "CS2C ab12cd34 FOO",
            "",
        ] {
            assert_eq!(parse_marker(bad, N), None, "{bad:?} не метка");
        }
    }

    #[test]
    fn requests_are_built_through_the_shared_marker_format() {
        assert_eq!(ready_direct(N), "echo CS2C ab12cd34 READY");
        assert_eq!(
            ready_request(N),
            "mirv_cmd clear; mirv_cmd addAtTick 64 \"echo CS2C ab12cd34 READY\""
        );
        assert!(
            parse_marker(&ready_direct(N), N).is_none(),
            "эхо своей команды меткой не считается"
        );
        assert!(parse_marker(&ready_request(N), N).is_none());
    }

    #[test]
    fn evidence_and_afxerror_are_recognised_by_substring() {
        assert!(is_demo_running_evidence(
            "[HostStateManager] Host activate: Playing Demo (x)"
        ));
        assert!(is_demo_running_evidence(
            "[ClientStateManager] -> CSGO_GAME_UI_STATE_INGAME"
        ));
        assert!(!is_demo_running_evidence("CS2C ab12cd34 READY"));
        assert!(is_afxerror(
            "AFXERROR: Failed writing image for screen recording."
        ));
        assert!(!is_afxerror("все хорошо"));
    }
}
