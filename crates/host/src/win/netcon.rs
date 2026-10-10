//! Консоль игры по netcon: подключение, чтение строк и запись команд (D-04).
//!
//! Безопасность (T-01-17): подключаемся только к `127.0.0.1` и к случайному порту этого запуска.
//! Спайк S2 показал, что сама игра слушает на `0.0.0.0` без пароля, поэтому строки из сокета —
//! недоверенные: хост их только пересылает, ничего из них не исполняет, а команды идут лишь
//! через `Session::send` (фиксированные шаблоны машины записи).

use std::io::{BufRead, BufReader, ErrorKind};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use super::{Shared, Sink, lock};
use crate::{SessionEvent, Source};

/// Сколько ждём ответа на одну попытку подключения.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Пауза между попытками подключения и переподключения.
const RETRY_PAUSE: Duration = Duration::from_secs(1);
/// Таймаут чтения: после него поток проверяет флаг остановки и читает дальше.
const READ_TIMEOUT: Duration = Duration::from_millis(500);
/// Запись команды не должна зависать: порт игры мог перестать читать.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Строка без перевода строки длиннее этого предела отдаётся как есть (защита от бесконечного буфера).
const MAX_LINE: usize = 1 << 20;

/// Свободный TCP-порт на `127.0.0.1` (если не нашёлся — привычный порт `2121`).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(2121)
}

/// Запустить поток консоли. Собственного дедлайна подключения нет: его держит машина записи.
pub(crate) fn spawn(port: u16, shared: Arc<Shared>, sink: Arc<Sink>) {
    std::thread::spawn(move || run(port, &shared, &sink));
}

fn finished(shared: &Shared) -> bool {
    shared.stopped() || shared.game_gone.load(std::sync::atomic::Ordering::SeqCst)
}

/// Пауза маленькими шагами, чтобы флаг остановки действовал быстро.
fn pause(shared: &Shared, total: Duration) {
    let step = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() && !finished(shared) {
        let d = left.min(step);
        std::thread::sleep(d);
        left -= d;
    }
}

fn run(port: u16, shared: &Shared, sink: &Sink) {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    while !finished(shared) {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                connected(stream, shared, sink);
                // порт мог закрыться вместе с игрой — не стучимся в него бесконечно после её смерти
                pause(shared, RETRY_PAUSE);
            }
            Err(_) => pause(shared, RETRY_PAUSE),
        }
    }
}

/// Одно подключение: читаем строки, пока сокет жив.
fn connected(stream: TcpStream, shared: &Shared, sink: &Sink) {
    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return;
    }
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    *lock(&shared.netcon) = Some(writer);
    sink.send(SessionEvent::ConsoleUp);

    let mut reader = BufReader::new(stream);
    // вывод консоли не обязан быть UTF-8 — читаем байты; частичная строка копится между таймаутами
    let mut buf: Vec<u8> = Vec::new();
    loop {
        if shared.stopped() {
            break;
        }
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => flush(&mut buf, sink),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if buf.len() > MAX_LINE {
                    flush(&mut buf, sink);
                }
            }
            Err(_) => break,
        }
    }
    flush(&mut buf, sink);
    *lock(&shared.netcon) = None;
    sink.send(SessionEvent::ConsoleDown);
}

/// Отдать накопленные байты одной строкой (пустые пропускаются).
fn flush(buf: &mut Vec<u8>, sink: &Sink) {
    let text = String::from_utf8_lossy(buf).trim().to_owned();
    buf.clear();
    if !text.is_empty() {
        sink.line(Source::Netcon, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_is_a_usable_loopback_port() {
        let p = free_port();
        assert!(p > 0);
        assert!(std::net::TcpListener::bind(("127.0.0.1", p)).is_ok());
    }
}
