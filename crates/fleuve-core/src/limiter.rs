//! Limiteur de débit global (seau à jetons) partagé par toutes les connexions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct RateLimiter {
    /// Octets par seconde ; 0 = illimité.
    rate: AtomicU64,
    state: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        RateLimiter {
            rate: AtomicU64::new(bytes_per_sec),
            state: Mutex::new(Bucket {
                tokens: bytes_per_sec as f64,
                last: Instant::now(),
            }),
        }
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    /// Attend que `n` octets puissent passer. Le seau peut devenir négatif :
    /// la dette est rendue par une attente proportionnelle, ce qui répartit
    /// équitablement le débit entre les connexions.
    pub async fn acquire(&self, n: u64) {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 || n == 0 {
            return;
        }
        let wait = {
            let mut b = self.state.lock().await;
            let now = Instant::now();
            let refill = now.duration_since(b.last).as_secs_f64() * rate as f64;
            b.last = now;
            b.tokens = (b.tokens + refill).min(rate as f64);
            b.tokens -= n as f64;
            if b.tokens < 0.0 {
                Duration::from_secs_f64(-b.tokens / rate as f64)
            } else {
                Duration::ZERO
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}
