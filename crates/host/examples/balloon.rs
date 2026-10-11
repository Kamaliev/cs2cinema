//! Спайк A2/A3 фазы 4: показывает уведомление через `Host::notify`.
//!
//! Заголовок и текст берутся из аргументов (по умолчанию «CS2 Cinema» и «5 готово / 0 упало · проверка
//! уведомления»). Процесс живёт `NOTIFY_HOLD + 4 с`, чтобы поток значка успел убрать его до выхода.
//! Код зовёт только `host::default_host().notify(..)`, поэтому собирается на всех платформах.

use std::time::Duration;

#[cfg(windows)]
use host::win::notify::NOTIFY_HOLD;
#[cfg(not(windows))]
const NOTIFY_HOLD: Duration = Duration::from_secs(8);

fn main() {
    let mut args = std::env::args().skip(1);
    let title = args.next().unwrap_or_else(|| "CS2 Cinema".to_string());
    let text = args
        .next()
        .unwrap_or_else(|| "5 готово / 0 упало · проверка уведомления".to_string());
    let shown = host::default_host().notify(&title, &text);
    println!("notify: {shown}");
    std::thread::sleep(NOTIFY_HOLD + Duration::from_secs(4));
}
