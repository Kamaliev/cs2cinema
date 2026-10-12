//! Файл токена раннера (D-02): значение лежит только в файле `token` рабочей папки раннера, доступ к которому сужен до автора.
//!
//! Токена нет ни в `Settings`, ни в `settings.json`, ни в базе. Значение наружу не возвращается строкой: `load` отдаёт
//! `runner::Token`, у которого `Debug` печатает `Token(***)`. Тексты ошибок строятся без значения токена.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use pipeline::host::{CmdSpec, Host};
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

/// Сколько ждём `icacls`: утилита отрабатывает за доли секунды, дольше — зависла.
const ACL_DEADLINE: Duration = Duration::from_secs(15);

/// Сохраняет токен в файл рабочей папки так, чтобы значение никогда не лежало в файле с чужим доступом.
///
/// Порядок обязателен: пустой файл -> доступ сужается до автора -> только потом пишется значение. Если сузить доступ
/// не удалось, файл удаляется и токен не сохраняется (D-02). Тексты ошибок значения токена не содержат.
pub fn save(host: &dyn Host, dir: &Path, value: &str) -> Result<(), String> {
    let value = validate(value)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать папку раннера: {}", e.kind()))?;
    let file = path(dir);
    let result = write_private(host, &file, &value);
    if result.is_err() {
        let _ = std::fs::remove_file(&file);
    }
    result
}

fn write_private(host: &dyn Host, file: &Path, value: &str) -> Result<(), String> {
    // пустой файл: прежнее значение (если было) стёрто, чужого доступа к новому значению ещё нет
    std::fs::File::create(file).map_err(|e| format!("Не удалось создать файл токена: {}", e.kind()))?;
    narrow_access(host, file)?;
    let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(file).map_err(|e| format!("Не удалось открыть файл токена: {}", e.kind()))?;
    f.write_all(value.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| format!("Не удалось записать файл токена: {}", e.kind()))
}

/// Сужает доступ к файлу до автора: на Windows `icacls <файл> /inheritance:r /grant:r <пользователь>:F`, успех — только код 0.
#[cfg(windows)]
fn narrow_access(host: &dyn Host, file: &Path) -> Result<(), String> {
    let user = std::env::var("USERNAME").ok().filter(|u| !u.trim().is_empty()).ok_or("Не удалось узнать имя пользователя Windows — токен не сохранён")?;
    let spec = CmdSpec {
        program: PathBuf::from("icacls"),
        args: vec![file.to_string_lossy().into_owned(), "/inheritance:r".to_owned(), "/grant:r".to_owned(), format!("{user}:F")],
        cwd: None,
        deadline: ACL_DEADLINE,
    };
    match host.run(&spec, &|| false, &mut |_| {}) {
        Ok(out) if out.code == Some(0) => Ok(()),
        Ok(out) => {
            let code = out.code.map_or("нет кода".to_owned(), |c| c.to_string());
            Err(format!("Не удалось закрыть доступ к файлу токена для других учётных записей (icacls, код {code}) — токен не сохранён"))
        }
        Err(e) => Err(format!("Не удалось закрыть доступ к файлу токена для других учётных записей (icacls: {e}) — токен не сохранён")),
    }
}

/// На остальных ОС доступ сужается правами `0600`.
#[cfg(not(windows))]
fn narrow_access(_host: &dyn Host, file: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Не удалось закрыть доступ к файлу токена для других учётных записей ({}) — токен не сохранён", e.kind()))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

/// Удаляет файл токена; отсутствие файла не ошибка.
pub fn clear(dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(path(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Не удалось удалить файл токена: {}", e.kind())),
    }
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
    use pipeline::host::{
        CmdOutput,
        fake::{FakeHost, Scenario},
    };
    use std::time::Duration;

    fn icacls_host(code: i32) -> FakeHost {
        let out = Ok(CmdOutput { code: Some(code), stderr_tail: Vec::new(), elapsed: Duration::ZERO });
        FakeHost::new(Scenario::happy(1).program("icacls", &[], out))
    }

    fn icacls_runs(host: &FakeHost) -> Vec<Vec<String>> {
        host.runs().into_iter().filter(|(_, program, _)| program.contains("icacls")).map(|(_, _, args)| args).collect()
    }

    #[cfg(windows)]
    #[test]
    fn saved_token_loads_back_and_the_acl_is_narrowed_first() {
        let dir = TestDir::new("token-save");
        let host = icacls_host(0);
        save(
            &host,
            &dir.path,
            "  tok-123 
",
        )
        .expect("токен должен сохраниться");
        let token = load(&dir.path).unwrap().expect("токен читается обратно");
        assert_eq!(token.bearer(), "Bearer tok-123");
        let runs = icacls_runs(&host);
        assert_eq!(runs.len(), 1, "ровно один запуск icacls: {runs:?}");
        let user = std::env::var("USERNAME").expect("в Windows есть USERNAME");
        let file = dir.path.join(TOKEN_FILE).to_string_lossy().into_owned();
        assert_eq!(runs[0], vec![file, "/inheritance:r".to_owned(), "/grant:r".to_owned(), format!("{user}:F")]);
    }

    #[cfg(windows)]
    #[test]
    fn failed_acl_leaves_no_token_file() {
        let dir = TestDir::new("token-acl");
        let err = save(&icacls_host(5), &dir.path, "secret-value-1").expect_err("без суженного доступа токен не сохраняется");
        assert!(err.contains("доступ"), "{err}");
        assert!(!err.contains("secret-value-1"), "значение токена не попадает в ошибку: {err}");
        assert!(!dir.path.join(TOKEN_FILE).exists(), "файла токена быть не должно");
        assert!(!exists(&dir.path));
    }

    #[cfg(unix)]
    #[test]
    fn saved_token_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("token-unix");
        let host = icacls_host(0);
        save(&host, &dir.path, "tok-123").unwrap();
        let mode = std::fs::metadata(dir.path.join(TOKEN_FILE)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "права файла токена");
        assert!(icacls_runs(&host).is_empty(), "icacls на этой ОС не запускается");
    }

    #[test]
    fn invalid_token_is_not_written() {
        let dir = TestDir::new("token-bad");
        let host = icacls_host(0);
        let err = save(&host, &dir.path, "ab cd").expect_err("пробел внутри токена");
        assert!(!err.contains("ab cd"), "значение не попадает в ошибку: {err}");
        assert!(!dir.path.join(TOKEN_FILE).exists(), "файла токена быть не должно");
        assert!(host.runs().is_empty(), "утилита не запускалась: {:?}", host.runs());
    }

    #[test]
    fn clear_removes_the_token() {
        let dir = TestDir::new("token-clear");
        clear(&dir.path).expect("без файла очистка не ошибка");
        std::fs::write(dir.path.join(TOKEN_FILE), "tok-123").unwrap();
        assert!(exists(&dir.path));
        clear(&dir.path).unwrap();
        assert!(!exists(&dir.path), "после очистки токена нет");
        assert!(!dir.path.join(TOKEN_FILE).exists());
    }
}
