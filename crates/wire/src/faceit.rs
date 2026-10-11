//! Строгий разбор ссылки на комнату матча FACEIT, общий для сервера и раннера.
//!
//! В отличие от `faceit::parse_match_id` (он мягкий и принимает голый id), здесь принимается только ссылка вида
//! `https://www.faceit.com/<язык>/<игра>/room/<id>[/<хвост>]`. Сервер хранит в задаче только канонический вид
//! (`canonical_url`), построенный из id, и сам по ссылке никуда не ходит; раннер перепроверяет её этим же кодом.
//! Только разбор строк: ни `regex`, ни `url`, чтобы крейт остался без зависимостей воркспейса.

/// Самая длинная принимаемая строка (после обрезки пробелов по краям).
const MAX_LEN: usize = 300;

/// Id матча: `<1-3 цифры>-<uuid 8-4-4-4-12 hex>`, регистр hex любой.
pub fn is_match_id(s: &str) -> bool {
    let Some((prefix, uuid)) = s.split_once('-') else {
        return false;
    };
    if prefix.is_empty() || prefix.len() > 3 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let groups: Vec<&str> = uuid.split('-').collect();
    const LENS: [usize; 5] = [8, 4, 4, 4, 12];
    groups.len() == LENS.len()
        && groups
            .iter()
            .zip(LENS)
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Язык: две строчные латинские буквы и необязательный суффикс `-Xx`.
fn is_lang(s: &str) -> bool {
    let (head, tail) = match s.split_once('-') {
        Some((h, t)) => (h, Some(t)),
        None => (s, None),
    };
    head.len() == 2
        && head.bytes().all(|b| b.is_ascii_lowercase())
        && tail.is_none_or(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn is_game(s: &str) -> bool {
    (1..=16).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn is_tail(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Id матча (в нижнем регистре) из ссылки комнаты FACEIT; всё, что не подходит строго, — `None`.
pub fn room_match_id(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() || s.len() > MAX_LEN {
        return None;
    }
    // Только печатные ASCII без пробелов: управляющие символы и похожие на латиницу юникодные двойники отпадают.
    if !s.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let scheme = "https://";
    if s.len() <= scheme.len() || !s[..scheme.len()].eq_ignore_ascii_case(scheme) {
        return None;
    }
    let rest = &s[scheme.len()..];
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    let (host, path) = rest.split_once('/')?;
    if !(host.eq_ignore_ascii_case("www.faceit.com") || host.eq_ignore_ascii_case("faceit.com")) {
        return None;
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let seg: Vec<&str> = path.split('/').collect();
    if seg.len() != 4 && seg.len() != 5 {
        return None;
    }
    if seg.iter().any(|p| p.is_empty()) {
        return None;
    }
    if !is_lang(seg[0]) || !is_game(seg[1]) || seg[2] != "room" || !is_match_id(seg[3]) {
        return None;
    }
    if seg.len() == 5 && !is_tail(seg[4]) {
        return None;
    }
    Some(seg[3].to_ascii_lowercase())
}

/// Канонический вид ссылки по id; только он хранится на сервере и уходит раннеру.
pub fn canonical_url(match_id: &str) -> String {
    format!("https://www.faceit.com/en/cs2/room/{match_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
    const ID_UPPER: &str = "1-0F9A1B2C-3D4E-5F60-7182-93A4B5C6D7E8";

    #[test]
    fn room_link_gives_a_lowercase_match_id() {
        for link in [
            format!("https://www.faceit.com/en/cs2/room/{ID_UPPER}"),
            format!("https://faceit.com/ru/csgo/room/{ID}/"),
            format!("https://www.faceit.com/ru/cs2/room/{ID_UPPER}/scoreboard?x=1#y"),
            format!(" https://WWW.FACEIT.COM/en/cs2/room/{ID_UPPER} "),
            format!("HTTPS://www.faceit.com/pt-BR/cs2/room/{ID}"),
        ] {
            assert_eq!(room_match_id(&link).as_deref(), Some(ID), "{link}");
        }
    }

    #[test]
    fn lookalike_and_foreign_links_are_rejected() {
        let bad = [
            format!("https://www.faceit.com.evil.com/en/cs2/room/{ID}"),
            format!("https://evil.com/https://www.faceit.com/en/cs2/room/{ID}"),
            format!("https://www.faceit.com@evil.com/en/cs2/room/{ID}"),
            format!("https://user@www.faceit.com/en/cs2/room/{ID}"),
            format!("https://www.faceit.com:444/en/cs2/room/{ID}"),
            format!("http://www.faceit.com/en/cs2/room/{ID}"),
            format!("//www.faceit.com/en/cs2/room/{ID}"),
            format!("www.faceit.com/en/cs2/room/{ID}"),
            ID.to_owned(),
            format!("https://www.faceit.com/en/cs2/room/{ID} x"),
            format!(
                "https://www.faceit.com/en/cs2/room/{}\n{}",
                &ID[..10],
                &ID[10..]
            ),
            format!("https://www.f\u{0430}ceit.com/en/cs2/room/{ID}"),
            format!("https://www.fac\u{0435}it.com/en/cs2/room/{ID}"),
            format!("https://www.faceit.com/en/cs2/room/{ID}/a/b"),
            format!("https://www.faceit.com/en//cs2/room/{ID}"),
            format!("https://www.faceit.com/en/cs2/match/{ID}"),
            "https://www.faceit.com/en/cs2/room/1234-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8"
                .to_owned(),
            "https://www.faceit.com/en/cs2/room/1-0f9a1b2g-3d4e-5f60-7182-93a4b5c6d7e8".to_owned(),
            "https://www.faceit.com/en/cs2/room/1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e".to_owned(),
            format!("https://www.faceit.com/en/cs2/room/{ID}/{}", "a".repeat(33)),
            format!(
                "https://www.faceit.com/en/cs2/room/{ID}/scoreboard?{}",
                "a".repeat(300)
            ),
            String::new(),
            "   ".to_owned(),
            "https://".to_owned(),
            "https://www.faceit.com".to_owned(),
        ];
        for link in &bad {
            assert_eq!(room_match_id(link), None, "{link}");
        }
    }

    #[test]
    fn canonical_url_round_trips() {
        assert_eq!(
            canonical_url(ID),
            format!("https://www.faceit.com/en/cs2/room/{ID}")
        );
        assert_eq!(room_match_id(&canonical_url(ID)).as_deref(), Some(ID));
        let other = "12-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        assert_eq!(room_match_id(&canonical_url(other)).as_deref(), Some(other));
    }

    #[test]
    fn match_id_shape_is_strict() {
        assert!(is_match_id(ID) && is_match_id(ID_UPPER));
        assert!(!is_match_id("") && !is_match_id("1-") && !is_match_id("-0f9a1b2c"));
        assert!(!is_match_id("1-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8-00"));
        assert!(!is_match_id("a-0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8"));
    }

    #[test]
    fn long_but_valid_tail_is_not_cut_by_the_length_limit() {
        let base = format!("https://www.faceit.com/en/cs2/room/{ID}/scoreboard");
        assert!(base.len() < MAX_LEN);
        assert!(room_match_id(&base).is_some());
    }
}
