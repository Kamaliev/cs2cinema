//! Хвост `console.log` (`-condebug`): строки консоли игры из файла, от смещения на момент запуска.

use std::path::{Path, PathBuf};

/// Хвост файла: помнит смещение и неполную строку между опросами.
pub(crate) struct Tail {
    path: PathBuf,
    offset: u64,
    pending: Vec<u8>,
}

impl Tail {
    pub fn new(path: PathBuf, offset: u64) -> Self {
        Tail {
            path,
            offset,
            pending: Vec::new(),
        }
    }

    /// Новые целые строки с прошлого опроса. RED: пока ничего не читает.
    pub fn poll(&mut self) -> Vec<String> {
        let _ = (&self.path, self.offset, &self.pending);
        Vec::new()
    }
}

/// Длина файла сейчас (0, если файла нет): с этого смещения читается хвост.
pub(crate) fn current_offset(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Уникальный временный файл; удаляется в `Drop`.
    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Temp(std::env::temp_dir().join(format!(
                "cs2cinema-tail-{tag}-{}-{n}.log",
                std::process::id()
            )))
        }
        fn write(&self, bytes: &[u8]) {
            std::fs::write(&self.0, bytes).unwrap();
        }
        fn append(&self, bytes: &[u8]) {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.0)
                .unwrap();
            f.write_all(bytes).unwrap();
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn tail_starts_at_the_offset_and_joins_partial_lines() {
        let f = Temp::new("offset");
        f.write(b"\xef\xbb\xbf10/10 16:00:00 CS2C ab12cd34 READY\r\n");
        let mut tail = Tail::new(f.0.clone(), current_offset(&f.0));
        assert!(tail.poll().is_empty(), "старое содержимое не читается");

        f.append("строка 1\n".as_bytes());
        f.append("полови".as_bytes());
        assert_eq!(tail.poll(), vec!["строка 1".to_owned()]);
        assert!(tail.poll().is_empty(), "обрывок ждёт продолжения");

        f.append("на\r\n\r\n".as_bytes());
        assert_eq!(tail.poll(), vec!["половина".to_owned()]);
    }

    #[test]
    fn tail_from_zero_drops_the_bom() {
        let f = Temp::new("bom");
        f.write(b"\xef\xbb\xbfa\nb\n");
        let mut tail = Tail::new(f.0.clone(), 0);
        assert_eq!(tail.poll(), vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn tail_restarts_from_zero_when_the_file_is_recreated() {
        let f = Temp::new("recreate");
        f.write(b"\xef\xbb\xbfold line one\nold line two\n");
        let mut tail = Tail::new(f.0.clone(), current_offset(&f.0));
        f.write(b"\xef\xbb\xbfnew\n");
        assert_eq!(tail.poll(), vec!["new".to_owned()]);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let f = Temp::new("missing");
        assert_eq!(current_offset(&f.0), 0);
        let mut tail = Tail::new(f.0.clone(), 0);
        assert!(tail.poll().is_empty());
        f.write(b"x\n");
        assert_eq!(tail.poll(), vec!["x".to_owned()]);
    }
}
