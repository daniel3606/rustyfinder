//! Bandwidth limits (file-ops spec §5.5): one limiter per direction,
//! shared by every running transfer of that direction, so the limit is on
//! the total and not on each transfer.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A token bucket. Transfers take bytes from it and wait when it runs dry.
#[derive(Debug)]
pub struct RateLimiter {
    state: Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    /// Bytes per second; 0 means no limit.
    rate: u64,
    /// Bytes that may go now; negative is a debt to wait off.
    allowance: f64,
    last: Instant,
}

/// The bucket holds at most this much time's worth of bytes, so a transfer
/// that was idle cannot burst far past the limit.
const BURST_SECS: f64 = 0.25;

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(0)
    }
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { state: Mutex::new(Bucket { rate: bytes_per_sec, allowance: 0.0, last: Instant::now() }) }
    }

    /// Change the limit; 0 turns it off. Takes effect for the next chunk.
    pub fn set_rate(&self, bytes_per_sec: u64) {
        let mut b = self.state.lock().unwrap();
        if b.rate != bytes_per_sec {
            b.rate = bytes_per_sec;
            b.allowance = 0.0;
            b.last = Instant::now();
        }
    }

    pub fn rate(&self) -> u64 {
        self.state.lock().unwrap().rate
    }

    /// Bytes to move per read/write while limited: small enough that the
    /// waits between chunks stay short and Stop stays quick.
    pub fn chunk_size(&self, unlimited: usize) -> usize {
        match self.rate() {
            0 => unlimited,
            rate => ((rate / 8) as usize).clamp(4096, unlimited),
        }
    }

    /// Account for `bytes` just moved; returns how long to wait before
    /// moving more.
    pub fn take(&self, bytes: u64) -> Duration {
        let mut b = self.state.lock().unwrap();
        if b.rate == 0 {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let rate = b.rate as f64;
        let refill = now.duration_since(b.last).as_secs_f64() * rate;
        b.allowance = (b.allowance + refill).min(rate * BURST_SECS);
        b.last = now;
        b.allowance -= bytes as f64;
        if b.allowance >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-b.allowance / rate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn no_limit_never_waits() {
        let limiter = RateLimiter::new(0);
        assert_eq!(limiter.take(10 << 20), Duration::ZERO);
        assert_eq!(limiter.chunk_size(1 << 20), 1 << 20);
    }

    #[test]
    fn debt_turns_into_waiting_time() {
        let limiter = RateLimiter::new(100_000);
        // 50 KB over an empty bucket is half a second of debt.
        let wait = limiter.take(50_000);
        assert!(wait > Duration::from_millis(450) && wait <= Duration::from_millis(500), "{wait:?}");
        assert_eq!(limiter.chunk_size(1 << 20), 12_500);
    }

    #[test]
    fn fo14_two_transfers_share_one_limit() {
        // Two "uploads" pushing as fast as the limiter lets them: their
        // combined rate stays within 10 % of the limit.
        let rate = 400_000u64;
        let limiter = Arc::new(RateLimiter::new(rate));
        let total = 600_000u64;
        let started = Instant::now();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let limiter = Arc::clone(&limiter);
                std::thread::spawn(move || {
                    let chunk = limiter.chunk_size(1 << 20) as u64;
                    let mut sent = 0;
                    while sent < total {
                        let n = chunk.min(total - sent);
                        sent += n;
                        std::thread::sleep(limiter.take(n));
                    }
                })
            })
            .collect();
        workers.into_iter().for_each(|w| w.join().unwrap());
        let secs = started.elapsed().as_secs_f64();
        let combined = (2 * total) as f64 / secs;
        assert!(combined <= rate as f64 * 1.1, "combined rate {combined:.0} B/s over {secs:.2} s");
    }
}
