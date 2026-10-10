//! Журнал записи `record.log` и хвост консоли для ошибки.
//!
//! `record.log` лежит в папке задачи и открывается на дозапись до запуска лаунчера: повторная
//! попытка в ту же папку добавляет новый блок, а улики прошлых попыток остаются (LCH-04). Строки
//! консоли недоверенные (netcon слушает не только localhost), поэтому они попадают в файл только
//! как текст: управляющие символы заменяются пробелами, профиль пользователя и секреты вида
//! `token=…` скрываются. В исполнение ничто из этого не уходит.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use super::failure::{RecordFailure, secs};

/// Журнал записи в `out_dir/record.log`.
pub struct RecordLog {
    file: std::fs::File,
}

impl RecordLog {
    /// Открыть `record.log` на дозапись и написать разделитель попытки. Файл никогда не
    /// обрезается; если папки нет, она создаётся.
    pub fn open(out_dir: &Path, header: &str, nonce: &str, takes: usize) -> io::Result<Self> {
        std::fs::create_dir_all(out_dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(out_dir.join("record.log"))?;
        let mut log = RecordLog { file };
        let used = log.file.metadata().map(|m| m.len() > 0).unwrap_or(false);
        let who = match one_line(header).as_str() {
            "" => String::new(),
            h => format!(" · {h}"),
        };
        let sep = format!("==== запись{who} · nonce {nonce} · дублей {takes} ====\n");
        log.put(&format!("{}{sep}", if used { "\n" } else { "" }));
        Ok(log)
    }

    /// Строка журнала: `+<секунды от начала записи> [<источник>] <текст>`.
    pub fn line(&mut self, at: Duration, source: &str, text: &str) {
        let line = format!("+{:08.3} [{source}] {}\n", at.as_secs_f64(), one_line(text));
        self.put(&line);
    }

    /// Итог неудачной записи: код, состояние, сообщение, подсказка, прошедшее время.
    pub fn failure(&mut self, at: Duration, f: &RecordFailure) {
        let hint = if f.hint.is_empty() { "—" } else { &f.hint };
        let block = format!(
            "---- итог: сбой на {:.3} с ----\nкод: {}\nсостояние: {} ({})\nсообщение: {}\nподсказка: {}\nпрошло {} с\n",
            at.as_secs_f64(),
            f.code.as_str(),
            f.state.title(),
            f.state.code(),
            one_line(&f.message),
            one_line(hint),
            secs(f.elapsed),
        );
        self.put(&block);
    }

    /// Файл без буфера: каждая запись сразу уходит в ОС, поэтому журнал полон и при падении.
    fn put(&mut self, s: &str) {
        let _ = self.file.write_all(s.as_bytes());
        let _ = self.file.flush();
    }
}

impl Drop for RecordLog {
    fn drop(&mut self) {
        let _ = self.file.flush();
    }
}

/// Текст строки журнала: без управляющих символов, профиля пользователя и секретов.
pub fn one_line(s: &str) -> String {
    redact(&s.replace(|c: char| c.is_control(), " "))
}

/// Скрыть имя профиля в путях (`C:\Users\имя` -> `C:\Users\USER`) и значения вида `token=…`.
pub fn redact(s: &str) -> String {
    redact_secrets(&redact_profile(s))
}

fn is_sep(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

fn redact_profile(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let (sb, lb) = (s.as_bytes(), lower.as_bytes());
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < sb.len() {
        // `<буква>:` + разделитель(и) + `users` + разделитель(и) + имя
        if lb[i] == b':' && i > 0 && lb[i - 1].is_ascii_alphabetic() {
            let mut j = i + 1;
            let seps = |from: usize| lb[from..].iter().take_while(|&&b| is_sep(b)).count();
            let n1 = seps(j);
            j += n1;
            if n1 > 0 && lb[j..].starts_with(b"users") {
                j += 5;
                let n2 = seps(j);
                let name_start = j + n2;
                if n2 > 0 {
                    let mut e = name_start;
                    while e < sb.len() && !is_sep(sb[e]) && !b" \t\"'<>|,;".contains(&sb[e]) {
                        e += 1;
                    }
                    if e > name_start {
                        out.push_str(&s[i..name_start]);
                        out.push_str("USER");
                        i = e;
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Ключи, после которых (через `=` или `:`) следует секрет.
const SECRET_KEYS: &[&str] = &[
    "apikey", "api_key", "api-key", "token", "secret", "password", "passwd",
];

fn redact_secrets(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let (sb, lb) = (s.as_bytes(), lower.as_bytes());
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < sb.len() {
        let mut hidden_to = None;
        // `Bearer <значение>`
        if lb[i..].starts_with(b"bearer ") {
            hidden_to = Some((i + 7, value_end(sb, i + 7)));
        }
        for key in SECRET_KEYS {
            if lb[i..].starts_with(key.as_bytes()) {
                let mut j = i + key.len();
                while j < sb.len() && sb[j] == b' ' {
                    j += 1;
                }
                if j < sb.len() && (sb[j] == b'=' || sb[j] == b':') {
                    j += 1;
                    while j < sb.len() && sb[j] == b' ' {
                        j += 1;
                    }
                    hidden_to = Some((j, value_end(sb, j)));
                }
            }
        }
        match hidden_to {
            Some((from, to)) if to > from => {
                out.push_str(&s[i..from]);
                out.push_str("***");
                i = to;
            }
            _ => {
                let ch = s[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

fn value_end(b: &[u8], from: usize) -> usize {
    let mut e = from;
    while e < b.len() && !b" \t\"'&,;".contains(&b[e]) {
        e += 1;
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_and_secrets_are_hidden() {
        assert_eq!(
            redact("Requesting playback of 'C:/Users/kamal/Downloads/x.dem'"),
            "Requesting playback of 'C:/Users/USER/Downloads/x.dem'"
        );
        assert_eq!(
            redact("c:\\users\\Иван Петров\\x"),
            "c:\\users\\USER Петров\\x",
            "скрывается имя до первого пробела"
        );
        assert_eq!(
            redact("FACEIT_API_KEY=abc123 token: zzz Bearer q1w2 ok"),
            "FACEIT_API_KEY=*** token: *** Bearer *** ok"
        );
        assert_eq!(redact("CS2C ab12cd34 READY"), "CS2C ab12cd34 READY");
        assert_eq!(one_line("a\r\nb\0c"), "a  b c");
    }
}
