//! Хвост `console.log` (`-condebug`): строки консоли игры из файла, от смещения на момент запуска.

//!
//! Игра пишет файл с BOM и префиксом времени `MM/DD HH:MM:SS` в каждой строке; файл общий для всех
//! запусков, поэтому читается только то, что появилось после старта сессии (T-01-22), а метки
//! прошлых запусков дополнительно отсекает код задачи. Строки недоверенные — как и в netcon.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{Shared, Sink};
use crate::Source;

/// Период опроса файла.
const POLL: Duration = Duration::from_millis(250);
/// За один опрос читается не больше этого (остальное — в следующий раз).
const MAX_CHUNK: u64 = 4 << 20;
/// Неполная строка длиннее этого отдаётся как есть.
const MAX_LINE: usize = 1 << 20;
const BOM: [u8; 3] = [0xef, 0xbb, 0xbf];

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

    /// Новые целые строки с прошлого опроса. Отсутствие файла и ошибки чтения — не ошибки:
    /// игра могла ещё не создать файл или держать его в этот момент.
    pub fn poll(&mut self) -> Vec<String> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return Vec::new();
        };
        if len < self.offset {
            // файл пересоздан (новый запуск игры): читаем с начала
            self.offset = 0;
            self.pending.clear();
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut chunk = Vec::new();
        // ошибка посреди чтения не страшна: что успели прочитать, то и берём
        let _ = file
            .take(len.saturating_sub(self.offset).min(MAX_CHUNK))
            .read_to_end(&mut chunk);
        let from_start = self.offset == 0;
        self.offset += chunk.len() as u64;
        if from_start && chunk.starts_with(&BOM) {
            chunk.drain(..BOM.len());
        }
        self.pending.extend_from_slice(&chunk);

        let mut lines = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=end).collect();
            lines.extend(decode(&raw));
        }
        if self.pending.len() > MAX_LINE {
            let raw = std::mem::take(&mut self.pending);
            lines.extend(decode(&raw));
        }
        lines
    }
}

/// Одна строка: lossy UTF-8, без BOM и пробелов по краям; пустая — `None`.
fn decode(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let text = text.trim().trim_start_matches('\u{feff}').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Запустить поток хвоста: строки уходят в канал событий с меткой `console.log`.
pub(crate) fn spawn(path: PathBuf, offset: u64, shared: Arc<Shared>, sink: Arc<Sink>) {
    std::thread::spawn(move || {
        let mut tail = Tail::new(path, offset);
        while !shared.stopped() {
            for line in tail.poll() {
                sink.line(Source::ConsoleLog, line);
            }
            std::thread::sleep(POLL);
        }
    });
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
