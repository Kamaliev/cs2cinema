//! Окна процессов своего job: класс, заголовок и текст диалога (D-10, LCH-05).
//!
//! Читаются только `EnumWindows`, `EnumChildWindows` и `GetWindowTextW` (кэшированный заголовок
//! окна, без обращения к потоку окна). Окнам не отправляется ни одного сообщения: запросы текста
//! через очередь сообщений зависают на подвисшем окне чужого процесса и ломают отмену (T-02-04).

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible,
};

use crate::{WINDOW_TEXT_MAX, WindowInfo};

/// Вместимость буфера класса и заголовка в символах UTF-16.
const BUF: usize = 512;
/// Класс стандартного диалога (`MessageBox` и диалоги ошибок).
const DIALOG_CLASS: &str = "#32770";
/// Разделитель текстов дочерних окон диалога.
const TEXT_SEP: &str = " / ";

/// Колбэк `EnumWindows`/`EnumChildWindows`: складывает `HWND` в `Vec`, переданный через `lparam`.
unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: `lparam` — адрес живого `Vec<HWND>` из `enum_top`/`enum_children`, перечисление
    // выполняется синхронно в том же потоке, пока вектор жив.
    unsafe { (*(lparam as *mut Vec<HWND>)).push(hwnd) };
    1
}

fn enum_top() -> Vec<HWND> {
    let mut all: Vec<HWND> = Vec::new();
    // SAFETY: колбэк корректен, `lparam` указывает на `all`, который живёт до возврата из вызова.
    unsafe { EnumWindows(Some(collect), (&raw mut all) as LPARAM) };
    all
}

fn enum_children(parent: HWND) -> Vec<HWND> {
    let mut all: Vec<HWND> = Vec::new();
    // SAFETY: `parent` получен из EnumWindows; колбэк корректен, `all` живёт до возврата.
    unsafe { EnumChildWindows(parent, Some(collect), (&raw mut all) as LPARAM) };
    all
}

/// Обрезка до `WINDOW_TEXT_MAX` символов (по `chars`, не по байтам).
fn clip(s: String) -> String {
    if s.chars().count() <= WINDOW_TEXT_MAX {
        s
    } else {
        s.chars().take(WINDOW_TEXT_MAX).collect()
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; BUF];
    // SAFETY: буфер жив, его вместимость в символах передана вторым счётом.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), BUF as i32) };
    clip(String::from_utf16_lossy(&buf[..n.max(0) as usize]))
}

fn title_of(hwnd: HWND) -> String {
    let mut buf = [0u16; BUF];
    // SAFETY: буфер жив, его вместимость в символах передана третьим аргументом.
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), BUF as i32) };
    clip(String::from_utf16_lossy(&buf[..n.max(0) as usize]))
}

/// Текст дочерних окон `Static` диалога через « / ».
fn dialog_text(dialog: HWND) -> String {
    let parts: Vec<String> = enum_children(dialog)
        .into_iter()
        .filter(|h| class_of(*h).eq_ignore_ascii_case("Static"))
        .map(title_of)
        .filter(|t| !t.is_empty())
        .collect();
    clip(parts.join(TEXT_SEP))
}

/// Видимые верхние окна процессов из `pids`. Пустой список PID — пустой результат без вызовов ОС.
pub fn list(pids: &[u32]) -> Vec<WindowInfo> {
    if pids.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for hwnd in enum_top() {
        let mut pid = 0u32;
        // SAFETY: `hwnd` получен из EnumWindows, `pid` — живая переменная для результата.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if !pids.contains(&pid) {
            continue;
        }
        // SAFETY: `hwnd` получен из EnumWindows; функция только читает флаг видимости.
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            // невидимые служебные окна (`IME`, `MSCTFIME UI`) — шум
            continue;
        }
        let class = class_of(hwnd);
        let text = if class == DIALOG_CLASS {
            dialog_text(hwnd)
        } else {
            String::new()
        };
        out.push(WindowInfo {
            pid,
            class,
            title: title_of(hwnd),
            text,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::job::Job;
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn empty_pid_list_gives_no_windows() {
        assert!(list(&[]).is_empty());
    }

    #[test]
    fn clip_counts_chars_not_bytes() {
        let long = "ж".repeat(WINDOW_TEXT_MAX + 50);
        let cut = clip(long);
        assert_eq!(cut.chars().count(), WINDOW_TEXT_MAX);
        assert_eq!(clip("коротко".to_owned()), "коротко");
    }

    #[test]
    #[ignore = "показывает окно на рабочем столе: cargo test -p host --lib -- --ignored modal"]
    fn modal_dialog_of_a_process_in_our_job_is_listed_with_its_text() {
        let dir = std::env::temp_dir().join(format!("cs2cinema-modal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("modal.vbs");
        // UTF-16 LE с BOM: иначе wscript испортит кириллицу
        let src = "MsgBox \"AFXERROR: проверка окна\", 16, \"AfxHookSource2 - Error\"\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for u in src.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        std::fs::File::create(&script)
            .unwrap()
            .write_all(&bytes)
            .unwrap();

        let job = Job::new().unwrap();
        let mut child = Command::new("wscript.exe")
            .arg(&script)
            .spawn()
            .expect("wscript должен запуститься");
        job.assign_child(&child).unwrap();

        let start = Instant::now();
        let mut found = None;
        while start.elapsed() < Duration::from_secs(5) {
            let pids = job.pids().unwrap();
            found = list(&pids).into_iter().find(|w| {
                w.class == "#32770"
                    && w.title.contains("AfxHookSource2")
                    && w.text.contains("AFXERROR: проверка окна")
            });
            if found.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        job.terminate(1).unwrap();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found.is_some(), "диалог со своим текстом не найден за 5 с");
    }
}
