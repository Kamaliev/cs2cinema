//! Уведомление в трее: собственный короткоживущий значок `Shell_NotifyIconW` с `NIF_INFO`.
//!
//! `tray-icon` всплывающих уведомлений не умеет, а итог очереди «N готово / M упало» нужен
//! именно тогда, когда окно спрятано (OBS-04, DC-6). Значок живёт на окне только для сообщений
//! и на отдельном потоке: поток добавляет значок, держит его [`NOTIFY_HOLD`] и убирает.
//! Оболочку (PowerShell) не используем: текст сбоя содержит строки консоли игры, а нативный
//! вызов не интерпретирует их (T-04-06).

use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, IDI_INFORMATION, LoadIconW,
};

use crate::{NOTIFY_TEXT_MAX, NOTIFY_TITLE_MAX, notify_text};

/// Сколько значок с уведомлением остаётся в трее, прежде чем его уберут.
pub const NOTIFY_HOLD: Duration = Duration::from_secs(8);

/// Сколько `balloon` ждёт ответа потока значка (T-04-07).
const ANSWER_WAIT: Duration = Duration::from_secs(2);

/// Подсказка значка.
const TIP: &str = "CS2 Cinema";

/// Показывает уведомление и возвращает, принял ли его Windows. Не блокирует дольше
/// [`ANSWER_WAIT`]: показ и уборка значка идут на своём потоке.
pub fn balloon(title: &str, text: &str) -> bool {
    let title = notify_text(title, NOTIFY_TITLE_MAX);
    let text = notify_text(text, NOTIFY_TEXT_MAX);
    let (tx, rx) = mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new()
        .name("cs2cinema-notify".into())
        .spawn(move || icon_thread(&title, &text, tx));
    if spawned.is_err() {
        return false;
    }
    rx.recv_timeout(ANSWER_WAIT).unwrap_or(false)
}

/// Копирует `s` в буфер Windows с завершающим нулём; лишнее отбрасывается.
fn fill(dst: &mut [u16], s: &str) {
    let room = dst.len().saturating_sub(1);
    let mut n = 0;
    for (slot, unit) in dst.iter_mut().zip(s.encode_utf16().take(room)) {
        *slot = unit;
        n += 1;
    }
    if let Some(end) = dst.get_mut(n) {
        *end = 0;
    }
}

/// Тело потока значка: окно, `NIM_ADD`, ответ в канал, пауза, `NIM_DELETE`, окно долой.
fn icon_thread(title: &str, text: &str, answer: mpsc::Sender<bool>) {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    // SAFETY: класс `STATIC` системный, строка оканчивается нулём; родитель `HWND_MESSAGE` делает
    // окно только для сообщений; результат проверяем на null.
    let hwnd: HWND = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        let _ = answer.send(false);
        return;
    }

    // SAFETY: структура из одних чисел, указателей и массивов — нулевое значение для неё валидно.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
    // SAFETY: системная иконка по стандартному идентификатору, `hinstance = null`.
    data.hIcon = unsafe { LoadIconW(ptr::null_mut(), IDI_INFORMATION) };
    fill(&mut data.szTip, TIP);
    fill(&mut data.szInfoTitle, title);
    fill(&mut data.szInfo, text);
    data.dwInfoFlags = NIIF_INFO;

    // SAFETY: `data` — полностью заполненная структура нужного размера, живёт до конца вызова.
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
    let _ = answer.send(added);
    if added {
        std::thread::sleep(NOTIFY_HOLD);
        // SAFETY: тот же `hWnd`/`uID`, что и при добавлении; значок убирает тот же поток.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }
    // SAFETY: окно создано этим потоком и больше нигде не используется.
    unsafe { DestroyWindow(hwnd) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_truncates_and_keeps_the_terminating_zero() {
        let mut buf = [0xFFFFu16; 4];
        fill(&mut buf, "abcdef");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        let mut buf = [0u16; 4];
        fill(&mut buf, "ab");
        assert_eq!(buf, [b'a' as u16, b'b' as u16, 0, 0]);
    }
}
