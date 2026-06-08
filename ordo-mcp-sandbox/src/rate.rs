//! Per-server rate limiter. Token bucket with one-minute windows;
//! simple, enough for v1 — servers that need higher rates can
//! request higher limits via their declaration.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RateLimiter {
    limit_per_minute: u32,
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    window_start: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            inner: Mutex::new(Inner {
                window_start: Instant::now(),
                count: 0,
            }),
        }
    }

    /// Attempt to acquire one invocation's quota. Returns true if
    /// allowed. The window rolls over after 60s of wall-clock time.
    pub fn try_acquire(&self) -> bool {
        let mut inner = self.inner.lock().expect("rate limiter poisoned");
        if inner.window_start.elapsed() > Duration::from_secs(60) {
            inner.window_start = Instant::now();
            inner.count = 0;
        }
        if inner.count >= self.limit_per_minute {
            return false;
        }
        inner.count += 1;
        true
    }

    pub fn current_count(&self) -> u32 {
        self.inner.lock().expect("rate limiter poisoned").count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permits_up_to_limit_and_then_blocks() {
        let rl = RateLimiter::new(3);
        assert!(rl.try_acquire());
        assert!(rl.try_acquire());
        assert!(rl.try_acquire());
        assert!(!rl.try_acquire());
    }

    #[test]
    fn zero_limit_blocks_everything() {
        let rl = RateLimiter::new(0);
        assert!(!rl.try_acquire());
    }
}
