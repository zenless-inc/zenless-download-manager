//! Token-bucket speed limiter shared by any number of connections.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Bytes/s limiter. A rate of `0` means unlimited. Callers "borrow" tokens
/// and sleep off any deficit, which serialises fairly across connections.
#[derive(Debug)]
pub struct RateLimiter {
    inner: Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    rate: u64,
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(rate: u64) -> Self {
        Self {
            inner: Mutex::new(Bucket {
                rate,
                tokens: 0.0,
                last: Instant::now(),
            }),
        }
    }

    pub fn rate(&self) -> u64 {
        self.inner.lock().map(|b| b.rate).unwrap_or(0)
    }

    /// Changes the rate; `0` disables limiting.
    pub fn set_rate(&self, rate: u64) {
        if let Ok(mut b) = self.inner.lock()
            && b.rate != rate
        {
            b.rate = rate;
            b.tokens = 0.0;
            b.last = Instant::now();
        }
    }

    /// Reserves `n` bytes and returns how long the caller must wait before
    /// using them (zero when within budget).
    pub fn reserve(&self, n: usize) -> Duration {
        let Ok(mut b) = self.inner.lock() else {
            return Duration::ZERO;
        };
        if b.rate == 0 {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let rate = b.rate as f64;
        // Allow a burst of a quarter second (at least 16 KiB).
        let burst = (rate * 0.25).max(16.0 * 1024.0);
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.last = now;
        b.tokens = (b.tokens + elapsed * rate).min(burst);
        b.tokens -= n as f64;
        if b.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64((-b.tokens / rate).min(30.0))
        }
    }

    /// Async wait for `n` bytes of budget.
    pub async fn acquire(&self, n: usize) {
        let wait = self.reserve(n);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        let l = RateLimiter::new(0);
        for _ in 0..1000 {
            assert!(l.reserve(1 << 20).is_zero());
        }
    }

    #[test]
    fn limited_accumulates_debt() {
        let l = RateLimiter::new(100 * 1024);
        // Burst budget is 25 KiB, starting from zero tokens → 50 KiB costs ~0.5 s.
        let w1 = l.reserve(50 * 1024);
        assert!(w1 > Duration::from_millis(400) && w1 < Duration::from_millis(600), "{w1:?}");
        // A second reservation queues behind the first one.
        let w2 = l.reserve(50 * 1024);
        assert!(w2 > w1, "{w2:?} should exceed {w1:?}");
        l.set_rate(0);
        assert!(l.reserve(1 << 30).is_zero());
    }
}
