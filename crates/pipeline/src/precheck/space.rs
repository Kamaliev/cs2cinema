//! Оценка свободного места на дисках (PRE-06, D-03): все константы размера и вердикт в одном месте.
//!
//! Проверка двухпроходная: до скачивания демки известен только минимум ([`floors`]), после плана —
//! оценка по дублям ([`estimate`]). Оба прохода сравниваются через [`verdict`]: меньше потребности —
//! `fail`, меньше потребности с запасом [`MARGIN`] — `warn`.

use renderer::TakePlan;

use super::Level;

/// Сырое видео дубля, МБ на игровую секунду при кадре 1920×1080 и записи 60 к/с. Измерено по трём
/// локальным проектам (6.3–8.9 МБ/с), взято с запасом; уточняется по первым реальным записям (A4).
pub const VIDEO_MB_PER_GAME_SEC_1080P60: f64 = 10.0;
/// Звук (WAV 48 кГц, 16 бит, стерео), МБ на игровую секунду.
pub const AUDIO_MB_PER_SEC: f64 = 0.192;
/// Запас: свободного места меньше потребности × `MARGIN` — предупреждение.
pub const MARGIN: f64 = 1.5;
/// Минимум на диске результатов до плана: 3 ГиБ.
pub const FLOOR_RESULTS: u64 = 3 << 30;
/// Минимум на диске CS2 до плана: 1 ГиБ.
pub const FLOOR_CS2: u64 = 1 << 30;

const MB: f64 = 1e6;
/// Конфиги `highlights.cfg`/`cs2c_*` в папке игры, байт.
const CFG_BYTES: u64 = 1_000_000;

/// Сколько нужно места: на диске результатов и на диске CS2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Need {
    pub results: u64,
    pub cs2: u64,
}

/// Минимум до плана.
pub fn floors() -> Need {
    Need {
        results: FLOOR_RESULTS,
        cs2: FLOOR_CS2,
    }
}

/// Неконечное или отрицательное число — 0 (как `deadlines::finite`).
fn finite(x: f64) -> f64 {
    if x.is_finite() && x >= 0.0 { x } else { 0.0 }
}

/// Оценка по плану: на дубль — игровые секунды × (видео по площади кадра и fps записи + звук).
/// Диску CS2 нужна копия демки (`demo_bytes`) и конфиги.
pub fn estimate(takes: &[TakePlan], size: (u32, u32), demo_bytes: u64) -> Need {
    let area = size.0 as f64 * size.1 as f64 / (1920.0 * 1080.0);
    let mb: f64 = takes
        .iter()
        .map(|t| {
            let fps_k = (t.capture_fps as f64 / 60.0).max(1.0);
            finite(t.game_secs as f64)
                * (VIDEO_MB_PER_GAME_SEC_1080P60 * area * fps_k + AUDIO_MB_PER_SEC)
        })
        .sum();
    Need {
        results: (finite(mb) * MB) as u64,
        cs2: demo_bytes.saturating_add(CFG_BYTES),
    }
}

/// Первый компонент пути в нижнем регистре — «том» для путей без буквы диска.
fn root(p: &str) -> String {
    p.trim()
        .replace('\\', "/")
        .split('/')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_lowercase()
}

/// Буква диска `X:` в нижнем регистре, если путь начинается с неё.
fn drive(p: &str) -> Option<char> {
    let mut c = p.trim().chars();
    let (letter, colon) = (c.next()?, c.next()?);
    (letter.is_ascii_alphabetic() && colon == ':').then(|| letter.to_ascii_lowercase())
}

/// Лежат ли два пути на одном томе. По строке, без обращения к ОС: буквы дисков, иначе первые
/// компоненты путей (работает и на ubuntu).
pub fn same_volume(a: &str, b: &str) -> bool {
    match (drive(a), drive(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => root(a) == root(b),
        _ => false,
    }
}

/// Вердикт по свободному месту: меньше потребности — `Fail`, меньше потребности × `MARGIN` — `Warn`.
pub fn verdict(free: u64, need: u64) -> Level {
    if free < need {
        Level::Fail
    } else if (free as f64) < need as f64 * MARGIN {
        Level::Warn
    } else {
        Level::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(game_secs: f32, capture_fps: u32) -> TakePlan {
        TakePlan {
            index: 0,
            shot: 0,
            game_secs,
            capture_fps,
        }
    }

    #[test]
    fn estimate_grows_with_frame_area_and_capture_fps() {
        let base = estimate(&[take(10.0, 60)], (1920, 1080), 0);
        assert_eq!(base.results, ((10.0 * (10.0 + 0.192)) * 1e6) as u64);
        let big = estimate(&[take(10.0, 60)], (3840, 2160), 0);
        assert!(
            big.results > base.results * 3,
            "4K почти вчетверо больше 1080p"
        );
        let fast = estimate(&[take(10.0, 240)], (1920, 1080), 0);
        assert!(
            fast.results > base.results * 3,
            "240 к/с почти вчетверо больше 60"
        );
        let slow = estimate(&[take(10.0, 30)], (1920, 1080), 0);
        assert_eq!(
            slow.results, base.results,
            "fps ниже 60 оценку не сокращает"
        );
        let two = estimate(&[take(10.0, 60), take(10.0, 60)], (1920, 1080), 0);
        assert_eq!(two.results, base.results * 2);
        assert_eq!(
            estimate(&[take(f32::NAN, 60), take(-5.0, 60)], (1920, 1080), 0).results,
            0
        );
        assert_eq!(
            estimate(&[], (1920, 1080), 5_000_000).cs2,
            6_000_000,
            "демка и конфиги"
        );
    }

    #[test]
    fn floors_are_three_and_one_gib() {
        assert_eq!(
            floors(),
            Need {
                results: 3 * 1024 * 1024 * 1024,
                cs2: 1024 * 1024 * 1024
            }
        );
    }

    #[test]
    fn same_volume_compares_drive_letters_case_insensitively() {
        assert!(same_volume("C:\\Users\\a", "c:/Games/cs2"));
        assert!(!same_volume("C:/out", "D:/steam"));
        assert!(same_volume("/mnt/data/a", "/mnt/data/b"));
        assert!(!same_volume("C:/out", "/mnt/data"));
        assert!(!same_volume("/mnt/a", "/home/b"));
    }

    #[test]
    fn verdict_fails_below_need_and_warns_below_the_margin() {
        assert_eq!(verdict(99, 100), Level::Fail);
        assert_eq!(verdict(100, 100), Level::Warn);
        assert_eq!(verdict(149, 100), Level::Warn);
        assert_eq!(verdict(150, 100), Level::Ok);
        assert_eq!(verdict(0, 0), Level::Ok);
    }
}
