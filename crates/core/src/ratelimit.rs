/*
   File: crates/core/src/ratelimit.rs

   Purpose
   A small, monotonic-clock token-bucket rate limiter shared across the
   workspace. The deck server uses it to bound WebSocket messages per
   connection; the native agent runtime's provider limiter uses it to
   bound outbound LLM calls per provider. Keeping one implementation
   here avoids two subtly-different copies drifting apart.

   The bucket is intentionally synchronous and `&mut self`-based: it
   holds no locks of its own, so callers wrap it in whatever sharing
   primitive (per-connection owner, `Mutex`, etc.) fits their context.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — lifted from crates/deck/src/server.rs
*/

use std::time::Instant;

/// Steady-state rate and burst capacity for a [`TokenBucket`].
///
/// `per_sec` is the long-run refill rate (tokens added per second).
/// `burst` is the maximum number of tokens the bucket may hold, i.e. the
/// largest instantaneous burst permitted after an idle period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Tokens replenished per second in steady state.
    pub per_sec: u32,
    /// Maximum tokens held — the burst ceiling. Should be `>= per_sec` in
    /// practice; callers that pass a smaller burst simply get less burst.
    pub burst: u32,
}

/// A monotonic-clock token bucket.
///
/// Tokens accrue continuously at `per_sec` up to `burst`. Each
/// [`TokenBucket::consume`] spends one token and returns whether one was
/// available. Time is read from [`Instant::now`], so the bucket is immune
/// to wall-clock adjustments.
#[derive(Debug)]
pub struct TokenBucket {
    tokens: f64,
    burst: f64,
    per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    /// Build a bucket that starts full (`burst` tokens available).
    ///
    /// Starting full lets the first burst through immediately; this matches
    /// the deck's prior behavior and is the conventional token-bucket start.
    pub fn new(rate: RateLimit) -> Self {
        Self {
            tokens: rate.burst as f64,
            burst: rate.burst as f64,
            per_sec: rate.per_sec as f64,
            last: Instant::now(),
        }
    }

    /// Refill for elapsed time, then attempt to spend one token.
    ///
    /// @return `true` if a token was available and spent, `false` if the
    /// caller is currently rate-limited. Refill is clamped to `burst`, so an
    /// idle bucket never accrues more than its ceiling.
    pub fn consume(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + dt * self.per_sec).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn allows_burst_then_blocks() {
        let mut b = TokenBucket::new(RateLimit {
            per_sec: 1,
            burst: 3,
        });
        assert!(b.consume());
        assert!(b.consume());
        assert!(b.consume());
        assert!(!b.consume());
    }

    #[test]
    fn refills_over_time() {
        let mut b = TokenBucket::new(RateLimit {
            per_sec: 1000,
            burst: 1,
        });
        assert!(b.consume());
        std::thread::sleep(Duration::from_millis(20));
        assert!(b.consume());
    }

    #[test]
    fn refill_is_clamped_to_burst() {
        // Idle far longer than needed to overflow; capacity must stay at burst.
        let mut b = TokenBucket::new(RateLimit {
            per_sec: 1000,
            burst: 2,
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(b.consume());
        assert!(b.consume());
        assert!(!b.consume());
    }
}
