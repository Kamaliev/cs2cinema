//! Файл токена раннера (D-02): значение лежит только в файле `token` рабочей папки раннера, доступ к которому сужен до автора.
//!
//! Токена нет ни в `Settings`, ни в `settings.json`, ни в базе. Значение наружу не возвращается строкой: `load` отдаёт
//! `runner::Token`, у которого `Debug` печатает `Token(***)`. Тексты ошибок строятся без значения токена.

use std::{
    io,
    path::{Path, PathBuf},
};

use runner::Token;

/// Имя файла токена в рабочей папке раннера.
pub const TOKEN_FILE: &str = "token";

/// Самый длинный допустимый токен: токен идёт в заголовок `Authorization`, длинное значение — признак ошибки ввода.
const TOKEN_MAX: usize = 512;

fn path(dir: &Path) -> PathBuf {
    dir.join(TOKEN_FILE)
}

/// Проверка введённого значения: пробелы и переводы строк по краям отбрасываются, остальное должно состоять из
/// печатных ASCII-знаков без пробелов (иначе заголовок запроса не соберётся, а раннер вечно повторял бы запрос).
/// Текст ошибки значения токена не содержит.
pub fn validate(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Токен пуст".to_owned());
    }
    if value.chars().count() > TOKEN_MAX {
        return Err(format!("Токен слишком длинный: больше {TOKEN_MAX} знаков"));
    }
    if !value.chars().all(|c| c.is_ascii_graphic()) {
        return Err("Токен должен состоять из печатных латинских знаков, цифр и знаков препинания без пробелов".to_owned());
    }
    Ok(value.to_owned())
}

/// Токен из файла рабочей папки. Нет файла или он пуст — `Ok(None)`; значение не прошло `validate` — ошибка.
pub fn load(dir: &Path) -> Result<Option<Token>, String> {
    let text = match std::fs::read_to_string(path(dir)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Файл токена не читается: {}", e.kind())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    validate(&text).map(|v| Some(Token::new(v)))
}

/// Есть ли непустой файл токена.
pub fn exists(dir: &Path) -> bool {
    std::fs::read_to_string(path(dir)).is_ok_and(|t| !t.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testkit::TestDir;

    #[test]
    fn token_file_value_is_trimmed_and_checked() {
        let dir = TestDir::new("token");
        assert!(load(&dir.path).unwrap().is_none(), "файла нет — токена нет");
        assert!(!exists(&dir.path));

        std::fs::write(dir.path.join(TOKEN_FILE), "abc-123\r\n").unwrap();
        let token = load(&dir.path).unwrap().expect("токен из файла");
        assert_eq!(token.bearer(), "Bearer abc-123", "пробелы и перевод строки по краям отброшены");
        assert!(exists(&dir.path));
        assert_eq!(format!("{token:?}"), "Token(***)", "значение в Debug не печатается");

        for bad in ["", "   ", "ab cd", "токен-кириллицей", "a\u{7}b", &"x".repeat(513)] {
            let err = validate(bad).expect_err("значение должно быть отвергнуто");
            assert!(!err.is_empty() && !err.contains("токен-кириллицей"), "ошибка по-русски и без значения: {err}");
        }
        assert_eq!(validate("  tok_1 \n").unwrap(), "tok_1");

        std::fs::write(dir.path.join(TOKEN_FILE), "ab cd").unwrap();
        let err = load(&dir.path).expect_err("пробел внутри токена");
        assert!(!err.contains("ab cd"), "значение не попадает в ошибку: {err}");
        std::fs::write(dir.path.join(TOKEN_FILE), "\n").unwrap();
        assert!(load(&dir.path).unwrap().is_none(), "пустой файл — токена нет");
    }
}
