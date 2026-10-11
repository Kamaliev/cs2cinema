//! Очистка текста из демки на границе доверия: имена игроков и карта.
//!
//! Правило: вырезать опасное, а не классифицировать весь Unicode. Ник из чужой демки
//! не должен ни менять грамматику cfg консоли CS2, ни уводить путь к геометрии карты,
//! ни прятать текст за управляющими символами направления письма.

/// Предел длины имени игрока в символах (не в байтах).
pub const NAME_MAX: usize = 64;

/// Символы, которые не видны, но меняют отображение или склеивают строки.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}'
        | '\u{061C}' | '\u{2060}' | '\u{FEFF}')
}

/// Однострочное имя без управляющих и невидимых символов, не длиннее `max_chars` символов.
/// Любой пробельный символ становится одним пробелом; эмодзи и кириллица остаются.
pub fn clean_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_control() || is_invisible(c) {
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out.chars().take(max_chars).collect::<String>().trim_end().to_string()
}

/// Имя карты, безопасное для пути `maps/{карта}.mesh`: только `[A-Za-z0-9_-]`, до 64 символов.
/// Пустой результат — `unknown`.
pub fn clean_map(s: &str) -> String {
    let m: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect();
    if m.is_empty() { "unknown".to_string() } else { m }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostile() -> Vec<String> {
        vec![
            "a\nquit".into(),
            "x\r\nexec evil".into(),
            "; quit".into(),
            "\" ; disconnect".into(),
            "%{pid}".into(),
            "'".into(),
            "\\".into(),
            "Я".repeat(4000),
            "\u{202E}evil".into(),
            "\u{2028}exec x".into(),
            "🔥fire🔥".into(),
            "\0".into(),
            "\u{7}bell".into(),
        ]
    }

    #[test]
    fn clean_text_strips_control_bidi_and_invisible_chars() {
        for s in hostile() {
            let c = clean_text(&s, 64);
            assert!(c.chars().count() <= 64, "{s:?}: длина");
            for ch in c.chars() {
                assert!(!ch.is_control(), "{s:?} -> {c:?}: управляющий {ch:?}");
                assert!(!matches!(ch, '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' | '\u{FEFF}'), "{s:?} -> {c:?}: {ch:?}");
            }
        }
        assert_eq!(clean_text("a\nquit", 64), "aquit");
    }

    #[test]
    fn clean_text_keeps_ordinary_names() {
        for s in ["s1mple", "ZywOo", "Глазами ace", "名字", "🔥fire🔥"] {
            assert_eq!(clean_text(s, 64), s);
        }
        assert_eq!(clean_text("  a \u{2028} b  ", 64), "a b");
    }

    #[test]
    fn clean_map_keeps_only_safe_chars() {
        assert_eq!(clean_map("de_mirage"), "de_mirage");
        assert_eq!(clean_map("../../x"), "x");
        assert_eq!(clean_map(r"C:\evil"), "Cevil");
        assert_eq!(clean_map(""), "unknown");
        assert_eq!(clean_map("x;exec evil"), "xexecevil");
    }
}
