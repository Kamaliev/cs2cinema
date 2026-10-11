//! Часы сервера. Все сроки в базе — абсолютные unix-миллисекунды по часам сервера; часам раннера не верим.
//! Часы внедряются в ядро, поэтому тесты двигают время рукой и не ждут.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Текущее время, unix-миллисекунды.
    fn now_ms(&self) -> i64;
}

/// Настоящие часы.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Ручные часы для тестов: время идёт только когда скажут. Копии делят одно время.
#[derive(Clone)]
pub struct ManualClock {
    ms: Arc<AtomicI64>,
}

impl ManualClock {
    pub fn new(start_ms: i64) -> Self {
        Self {
            ms: Arc::new(AtomicI64::new(start_ms)),
        }
    }

    pub fn advance(&self, d: Duration) {
        self.ms.fetch_add(d.as_millis() as i64, Ordering::SeqCst);
    }

    pub fn set(&self, ms: i64) {
        self.ms.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_told() {
        let c = ManualClock::new(5);
        assert_eq!(c.now_ms(), 5);
        let copy = c.clone();
        c.advance(Duration::from_secs(2));
        assert_eq!(c.now_ms(), 2005);
        assert_eq!(copy.now_ms(), 2005, "копии делят время");
        c.set(10);
        assert_eq!(copy.now_ms(), 10);
        assert!(SystemClock.now_ms() > 1_700_000_000_000);
    }
}
