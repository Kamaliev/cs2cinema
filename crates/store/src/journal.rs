//! Формат журнала задачи в базе: один источник для писателя (`app::jobs`) и читателя (карточка), D-10, D-11.
//!
//! Журнал не стирается между попытками. Каждая попытка начинается строкой-разделителем
//! («──── попытка N · дата · способ ────»), каждая строка несёт метку `[этап #N]`,
//! смена этапа пишет «⏱ этап X: длительность», а в конце попытки стоит строка итога.

use std::time::Duration;

/// Признак разделителя попыток: по нему `stage_summary` находит начало последней попытки.
const SEPARATOR_MARK: &str = "──── попытка ";
/// Конец разделителя после способа запуска.
const SEPARATOR_END: &str = " ────";
/// Начало строки с длительностью этапа.
const STAGE_MARK: &str = "⏱ этап ";
/// Начало строки итога попытки.
const OUTCOME_MARK: &str = "итог попытки ";

/// Как запущена попытка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartKind {
    /// Задача запускается впервые.
    First,
    /// «Повторить»: с записи по `record.json`, без скачивания и плана.
    Retry,
    /// «Заново»: с самого начала.
    Redo,
    /// Очередь сама повторила сбой класса «запуск» после уборки и паузы (DIF-04).
    Auto,
}

impl StartKind {
    /// Все способы запуска: по ним `attempts` узнаёт разделитель, который записал `separator`.
    pub const ALL: [StartKind; 4] = [StartKind::First, StartKind::Retry, StartKind::Redo, StartKind::Auto];

    pub fn title(self) -> &'static str {
        match self {
            StartKind::First => "первый запуск",
            StartKind::Retry => "Повторить",
            StartKind::Redo => "Заново",
            StartKind::Auto => "авто-повтор",
        }
    }
}

/// Длительность по-русски: «37 с», «4 мин 12 с», «1 ч 02 мин».
pub fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} с")
    } else if s < 3600 {
        format!("{} мин {} с", s / 60, s % 60)
    } else {
        format!("{} ч {:02} мин", s / 3600, s % 3600 / 60)
    }
}

/// Строка-разделитель в начале попытки.
pub fn separator(attempt: u32, when: &str, kind: StartKind) -> String {
    format!("{SEPARATOR_MARK}{attempt} · {when} · {} ────", kind.title())
}

/// Метка этапа и попытки в начале строки журнала.
pub fn prefix(stage: &str, attempt: u32) -> String {
    format!("[{stage} #{attempt}]")
}

/// Длительность завершённого этапа.
pub fn stage_line(stage: &str, d: Duration) -> String {
    format!("{STAGE_MARK}{stage}: {}", fmt_dur(d))
}

/// Итог попытки в конце её журнала.
pub fn outcome_line(attempt: u32, outcome: &str) -> String {
    format!("{OUTCOME_MARK}{attempt}: {outcome}")
}

/// Номера и способы запуска всех попыток по разделителям журнала. Строка может начинаться временем и меткой `[… #N]`.
/// Строка с неизвестным способом запуска пропускается: проверка приёмки сравнивает число разделителей с `job.attempt`.
pub fn attempts(log: &str) -> Vec<(u32, StartKind)> {
    log.lines()
        .filter_map(|l| {
            let rest = &l[l.find(SEPARATOR_MARK)? + SEPARATOR_MARK.len()..];
            let (n, rest) = rest.split_once(" · ")?;
            let (_when, kind) = rest.split_once(" · ")?;
            let kind = kind.trim_end().strip_suffix(SEPARATOR_END)?;
            let kind = StartKind::ALL.into_iter().find(|k| k.title() == kind)?;
            Some((n.trim().parse().ok()?, kind))
        })
        .collect()
}

/// Номера попыток и текст их итога по строкам `outcome_line`.
pub fn outcomes(log: &str) -> Vec<(u32, String)> {
    log.lines()
        .filter_map(|l| {
            let rest = &l[l.find(OUTCOME_MARK)? + OUTCOME_MARK.len()..];
            let (n, text) = rest.split_once(": ")?;
            Some((n.trim().parse().ok()?, text.trim().to_owned()))
        })
        .collect()
}

/// Этапы и их длительности последней попытки по журналу. Строка может начинаться временем и меткой `[… #N]`.
/// Журнал без разделителя читается целиком.
pub fn stage_summary(log: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = log.lines().collect();
    let from = lines.iter().rposition(|l| l.contains(SEPARATOR_MARK)).map_or(0, |i| i + 1);
    lines[from..]
        .iter()
        .filter_map(|l| {
            let rest = &l[l.find(STAGE_MARK)? + STAGE_MARK.len()..];
            let (name, dur) = rest.split_once(": ")?;
            Some((name.to_owned(), dur.trim().to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_written_in_russian_units() {
        assert_eq!(fmt_dur(Duration::from_secs(37)), "37 с");
        assert_eq!(fmt_dur(Duration::from_secs(252)), "4 мин 12 с");
        assert_eq!(fmt_dur(Duration::from_secs(3720)), "1 ч 02 мин");
        assert_eq!(fmt_dur(Duration::ZERO), "0 с");
    }

    #[test]
    fn stage_lines_round_trip_through_the_summary() {
        let log = format!(
            "12:00:01  {} старт\n12:00:09  {} {}\n12:04:21  {} {}\n",
            prefix("Скачивание", 1),
            prefix("Скачивание", 1),
            stage_line("Скачивание", Duration::from_secs(8)),
            prefix("Запись", 1),
            stage_line("Запись", Duration::from_secs(252)),
        );
        assert_eq!(stage_summary(&log), vec![("Скачивание".to_owned(), "8 с".to_owned()), ("Запись".to_owned(), "4 мин 12 с".to_owned())]);
        let bare = format!("{}\n", stage_line("Разбор", Duration::from_secs(3)));
        assert_eq!(stage_summary(&bare), vec![("Разбор".to_owned(), "3 с".to_owned())], "строка без времени и метки");
    }

    #[test]
    fn summary_reads_only_the_last_attempt() {
        let log = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            separator(1, "2026-10-11 10:00", StartKind::First),
            stage_line("Скачивание", Duration::from_secs(60)),
            outcome_line(1, "сбой CONSOLE_NOT_OPEN"),
            separator(2, "2026-10-11 10:30", StartKind::Retry),
            stage_line("Запись", Duration::from_secs(5)),
            outcome_line(2, "готово"),
        );
        assert_eq!(stage_summary(&log), vec![("Запись".to_owned(), "5 с".to_owned())]);
        assert!(log.contains("──── попытка 2 · 2026-10-11 10:30 · Повторить ────"));
        assert_eq!(separator(3, "д", StartKind::Redo), "──── попытка 3 · д · Заново ────");
        assert!(stage_summary("").is_empty());
    }

    #[test]
    fn attempts_reads_back_every_separator_kind() {
        for kind in StartKind::ALL {
            let log = format!("12:00:01  [Запись #2] {}\n", separator(7, "2026-10-11 10:00", kind));
            assert_eq!(attempts(&log), vec![(7, kind)], "{}", kind.title());
            let bare = format!("{}\n", separator(3, "д", kind));
            assert_eq!(attempts(&bare), vec![(3, kind)]);
        }
        let log = format!(
            "12:00:01  {} старт\n{}\n{}\n{}\n{}\n──── попытка 9 · д · неизвестный ────\n",
            prefix("Запись", 1),
            separator(1, "д", StartKind::First),
            stage_line("Запись", Duration::from_secs(5)),
            outcome_line(1, "сбой CONSOLE_NOT_OPEN"),
            separator(2, "д", StartKind::Auto),
        );
        assert_eq!(attempts(&log), vec![(1, StartKind::First), (2, StartKind::Auto)], "этапы, итоги и неизвестный способ не дают записей");
        assert!(attempts("").is_empty());
    }

    #[test]
    fn outcomes_read_back_what_outcome_line_wrote() {
        let log = format!("{}\n12:00:01  {}\n{}\n", outcome_line(1, "сбой CONSOLE_NOT_OPEN"), outcome_line(2, "готово: ролик"), stage_line("Запись", Duration::from_secs(1)));
        assert_eq!(outcomes(&log), vec![(1, "сбой CONSOLE_NOT_OPEN".to_owned()), (2, "готово: ролик".to_owned())]);
        assert!(outcomes("").is_empty());
    }
}
