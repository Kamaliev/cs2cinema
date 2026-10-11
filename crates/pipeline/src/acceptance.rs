//! Приёмочный набор: пять матчей FACEIT, по которым проверяется вся очередь (ACC-01, DC-7).
//!
//! Набор лежит в репозитории (`acceptance/matches.txt`) и встраивается в обе программы при сборке;
//! переменная `CS2CINEMA_ACCEPTANCE` подменяет его файлом без пересборки. Строки разбираются строгим
//! `faceit::parse_match_id`, поэтому в набор не попадёт ни путь, ни кусок URL. Партия задач названа
//! `acc-<unix>-<pid>`: по PID проверка узнаёт, что серия B шла в другом запуске приложения.

use std::collections::HashSet;

/// Сколько матчей в приёмочном наборе: по одному на карту.
pub const SET_SIZE: usize = 5;
/// Переменная окружения с путём к файлу набора вместо встроенного.
pub const SET_ENV: &str = "CS2CINEMA_ACCEPTANCE";
/// Встроенный набор (шаблон без ссылок, пока автор не вписал матчи).
pub const EMBEDDED: &str = include_str!("../../../acceptance/matches.txt");

/// Предел размера файла набора, байт.
const MAX_BYTES: usize = 64 * 1024;
/// Предел числа строк (вместе с комментариями).
const MAX_LINES: usize = 100;
/// Начало имени партии.
const BATCH_PREFIX: &str = "acc-";

/// Разбирает текст набора в список id матчей. Пустые строки и «#» пропускаются; мусор и повтор — ошибка.
pub fn parse_set(text: &str) -> Result<Vec<String>, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("набор больше {} КиБ", MAX_BYTES / 1024));
    }
    if text.lines().count() > MAX_LINES {
        return Err(format!("в наборе больше {MAX_LINES} строк"));
    }
    let mut ids: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let n = i + 1;
        let id = faceit::parse_match_id(line).ok_or_else(|| format!("строка {n}: не ссылка на матч FACEIT"))?;
        if !seen.insert(id.clone()) {
            return Err(format!("строка {n}: матч {id} уже есть в наборе"));
        }
        ids.push(id);
    }
    Ok(ids)
}

/// Набор и его источник: файл из `CS2CINEMA_ACCEPTANCE` или встроенный шаблон. `env` — чтение переменных окружения.
pub fn load_set(env: &dyn Fn(&str) -> Option<String>) -> Result<(Vec<String>, String), String> {
    match env(SET_ENV).filter(|p| !p.trim().is_empty()) {
        Some(path) => {
            let path = std::path::PathBuf::from(path.trim());
            let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.len() > MAX_BYTES as u64 {
                return Err(format!("{}: набор больше {} КиБ", path.display(), MAX_BYTES / 1024));
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let ids = parse_set(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((ids, path.display().to_string()))
        }
        None => Ok((parse_set(EMBEDDED)?, "встроенный набор".to_owned())),
    }
}

/// В наборе ровно [`SET_SIZE`] матчей; иначе — понятная причина.
pub fn require_full(set: &[String]) -> Result<(), String> {
    if set.len() == SET_SIZE {
        Ok(())
    } else {
        Err(format!(
            "в приёмочном наборе {} матчей из {SET_SIZE}: впишите ссылки в acceptance/matches.txt и пересоберите программу (или укажите файл в {SET_ENV})",
            set.len()
        ))
    }
}

/// Имя партии: секунды Unix и PID приложения.
pub fn batch_name(unix: u64, pid: u32) -> String {
    format!("{BATCH_PREFIX}{unix}-{pid}")
}

/// PID приложения из имени партии; чужое имя — `None`.
pub fn batch_pid(batch: &str) -> Option<u32> {
    let (unix, pid) = batch.strip_prefix(BATCH_PREFIX)?.split_once('-')?;
    unix.parse::<u64>().ok()?;
    pid.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1-3e7db9e3-8e92-4761-a1db-3729fb7de11c";
    const B: &str = "1-0b1c2d3e-4f50-4162-8394-a5b6c7d8e9f0";

    #[test]
    fn parse_set_takes_links_and_bare_ids_and_skips_comments() {
        let text = format!("# набор\n\nhttps://www.faceit.com/en/cs2/room/{A}/scoreboard\n  {B}  \n");
        assert_eq!(parse_set(&text).unwrap(), vec![A.to_owned(), B.to_owned()]);
    }

    #[test]
    fn parse_set_rejects_garbage_and_repeats_with_the_line_number() {
        let e = parse_set(&format!("{A}\n\nпросто текст\n")).unwrap_err();
        assert_eq!(e, "строка 3: не ссылка на матч FACEIT");
        let e = parse_set(&format!("{A}\nhttps://x/{B}\n{A}\n")).unwrap_err();
        assert!(e.starts_with("строка 3: матч ") && e.ends_with("уже есть в наборе"), "{e}");
    }

    #[test]
    fn parse_set_has_limits() {
        assert!(parse_set(&"#".repeat(MAX_BYTES + 1)).is_err());
        assert!(parse_set(&"# x\n".repeat(MAX_LINES + 1)).unwrap_err().contains("100 строк"));
        assert!(parse_set(&"# x\n".repeat(MAX_LINES)).unwrap().is_empty());
    }

    #[test]
    fn embedded_template_is_an_empty_set() {
        assert_eq!(parse_set(EMBEDDED).unwrap(), Vec::<String>::new());
        let (set, source) = load_set(&|_| None).unwrap();
        assert!(set.is_empty());
        assert_eq!(source, "встроенный набор");
        assert!(require_full(&set).unwrap_err().contains("0 матчей из 5"));
    }

    #[test]
    fn require_full_wants_exactly_five() {
        let five: Vec<String> = (0..SET_SIZE).map(|i| i.to_string()).collect();
        assert!(require_full(&five).is_ok());
        assert!(require_full(&five[..4]).is_err());
        let six: Vec<String> = (0..=SET_SIZE).map(|i| i.to_string()).collect();
        assert!(require_full(&six).is_err());
    }

    #[test]
    fn load_set_reads_the_file_named_by_the_variable() {
        let path = std::env::temp_dir().join(format!("cs2cinema-acc-set-{}.txt", std::process::id()));
        std::fs::write(&path, format!("# свой набор\n{A}\n")).unwrap();
        let p = path.display().to_string();
        let (set, source) = load_set(&|k| (k == SET_ENV).then(|| p.clone())).unwrap();
        assert_eq!((set, source), (vec![A.to_owned()], p.clone()));
        std::fs::write(&path, "мусор\n").unwrap();
        let e = load_set(&|k| (k == SET_ENV).then(|| p.clone())).unwrap_err();
        assert!(e.contains("строка 1"), "{e}");
        let _ = std::fs::remove_file(&path);
        let e = load_set(&|k| (k == SET_ENV).then(|| p.clone())).unwrap_err();
        assert!(e.contains(&p), "ошибка чтения называет путь: {e}");
    }

    #[test]
    fn batch_name_round_trips_the_pid() {
        assert_eq!(batch_name(1_700_000_000, 4242), "acc-1700000000-4242");
        assert_eq!(batch_pid("acc-1700000000-4242"), Some(4242));
        for junk in ["", "acc-", "acc-1-", "acc--5", "acc-x-5", "batch-1-2", "acc-1-2-3", "acc-1700000000-"] {
            assert_eq!(batch_pid(junk), None, "{junk}");
        }
    }
}
