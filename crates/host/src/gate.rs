//! Ворота «демка идёт»: метки консоли нашей задачи, пришедшие раньше подтверждения демки, отбрасываются.
//!
//! Почему: спайк S3c (`01-SPIKES.md`) показал ложный `READY` — `mirv_cmd addAtTick 64` сработал на
//! меню-карте ещё до `+playdemo`. Для машины записи это выглядело бы как «демка загрузилась».
//! Ворота стоят на границе хоста (слой сессии), машина `pipeline::record` не меняется: если настоящий
//! `READY` отброшен, машина сама попросит его напрямую (`ready_direct`) после подтверждения демки.
//!
//! Решается по тексту строки, поэтому модуль не зависит от ОС и проверяется на фикстурах консоли.

/// Тег метки консоли (общий с `renderer::MARKER_TAG`; тест в `pipeline` сверяет значения).
pub const MARKER_TAG: &str = "CS2C";

/// Строка консоли подтверждает, что демка действительно запущена (а не только запрошена).
pub fn is_demo_playing(line: &str) -> bool {
    line.contains("Host activate: Playing Demo") || line.contains("playing demo from")
}

/// Решение ворот по строке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Строка уходит в канал событий как есть.
    Pass,
    /// Метка нашей задачи до подтверждения демки: в канал не попадает, хост пишет `Note`.
    Drop,
}

/// Состояние ворот одной сессии.
#[derive(Debug)]
pub struct DemoGate {
    nonce: String,
    demo_seen: bool,
}

impl DemoGate {
    /// Пустой `nonce` отключает ворота: меток нашей задачи быть не может, пропускается всё.
    pub fn new(nonce: &str) -> Self {
        DemoGate {
            nonce: nonce.to_owned(),
            demo_seen: false,
        }
    }

    /// Видели ли уже подтверждение демки.
    pub fn demo_seen(&self) -> bool {
        self.demo_seen
    }

    /// Пропустить строку или отбросить. Подтверждение демки открывает ворота навсегда.
    pub fn admit(&mut self, line: &str) -> Verdict {
        if self.demo_seen {
            return Verdict::Pass;
        }
        if is_demo_playing(line) {
            self.demo_seen = true;
            return Verdict::Pass;
        }
        if !self.nonce.is_empty() && is_task_marker(line, &self.nonce) {
            return Verdict::Drop;
        }
        Verdict::Pass
    }
}

/// Строка начинается с `CS2C <nonce>` (после BOM и префикса времени `MM/DD HH:MM:SS`).
/// Нарочно шире разбора меток машины: до подтверждения демки отбрасывается любая метка задачи.
fn is_task_marker(line: &str, nonce: &str) -> bool {
    let s = line.trim_start_matches('\u{feff}').trim_start();
    let s = strip_timestamp(s).trim_start();
    let mut it = s.split_whitespace();
    it.next() == Some(MARKER_TAG) && it.next() == Some(nonce)
}

/// Снять префикс времени `MM/DD HH:MM:SS ` (ровно этот вид), если он есть.
fn strip_timestamp(s: &str) -> &str {
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

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = "ab12cd34";

    #[test]
    fn marker_before_the_demo_is_dropped_and_after_it_passes() {
        let mut g = DemoGate::new(N);
        assert_eq!(g.admit("CS2C ab12cd34 READY"), Verdict::Drop);
        assert_eq!(
            g.admit("\u{feff}10/10 16:24:15 CS2C ab12cd34 READY"),
            Verdict::Drop,
            "BOM и префикс времени снимаются"
        );
        assert_eq!(
            g.admit("[Demo] Requesting playback of 'x.dem'"),
            Verdict::Pass
        );
        assert!(!g.demo_seen(), "запрос демки — ещё не подтверждение");
        assert_eq!(g.admit("CS2C ab12cd34 READY"), Verdict::Drop);
        assert_eq!(
            g.admit("[HostStateManager] Host activate: Playing Demo (x.dem)"),
            Verdict::Pass
        );
        assert!(g.demo_seen());
        assert_eq!(g.admit("CS2C ab12cd34 READY"), Verdict::Pass);
        assert_eq!(g.admit("CS2C ab12cd34 TAKE_START 0"), Verdict::Pass);
    }

    #[test]
    fn playing_demo_from_line_also_opens_the_gate() {
        let mut g = DemoGate::new(N);
        assert_eq!(
            g.admit("10/10 16:24:15 [Demo] playing demo from 'x.dem'"),
            Verdict::Pass
        );
        assert_eq!(g.admit("CS2C ab12cd34 SCRIPT"), Verdict::Pass);
    }

    #[test]
    fn foreign_nonce_and_plain_lines_are_not_gated() {
        let mut g = DemoGate::new(N);
        assert_eq!(g.admit("CS2C ffff0000 READY"), Verdict::Pass);
        assert_eq!(g.admit("echo CS2C ab12cd34 READY"), Verdict::Pass);
        assert_eq!(g.admit("AFXERROR: Failed writing image"), Verdict::Pass);
        assert!(!g.demo_seen());
    }

    #[test]
    fn empty_nonce_disables_the_gate() {
        let mut g = DemoGate::new("");
        assert_eq!(g.admit("CS2C  READY"), Verdict::Pass);
        assert_eq!(g.admit("CS2C ab12cd34 READY"), Verdict::Pass);
    }

    #[test]
    fn demo_playing_needs_activation_not_just_queueing() {
        assert!(is_demo_playing(
            "[HostStateManager] Host activate: Playing Demo (x)"
        ));
        assert!(is_demo_playing("[Demo] playing demo from 'x.dem'"));
        assert!(!is_demo_playing(
            "CHostStateMgr::QueueNewRequest( Playing Demo (x), 3 )"
        ));
        assert!(!is_demo_playing("[Demo] Requesting playback of 'x.dem'"));
    }
}
