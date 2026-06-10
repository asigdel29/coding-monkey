/*
   File: crates/runtime/src/limiter.rs

   Purpose
   Bound and pace outbound LLM calls per provider so 100+ agents don't
   stampede the API. Each provider has a gate with three controls:
     - a Semaphore capping concurrent in-flight calls,
     - a token bucket pacing requests per second,
     - a SHARED pause window that every agent observes after a 429.

   The shared pause is the load-bearing detail. Without it, each agent
   retries its own 429 independently and they re-synchronize into a
   stampede the instant the quota reopens; with it, one rate-limit response
   backs off the whole fleet together, with exponential growth and full
   jitter so they don't all resume on the same tick.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — per-provider gate, shared 429
                                 backoff, retry helper
*/

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use monkey_core::{Provider, RateLimit, TokenBucket};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::llm::LlmError;

/// Tunables for a [`ProviderLimiter`].
#[derive(Debug, Clone, Copy)]
pub struct LimiterConfig {
    /// Max concurrent in-flight calls per provider.
    pub max_in_flight: usize,
    /// Requests-per-second pacing per provider.
    pub rps: u32,
    /// Token-bucket burst per provider.
    pub burst: u32,
    /// First backoff step after a rate-limit/transient error.
    pub base_backoff: Duration,
    /// Backoff ceiling.
    pub max_backoff: Duration,
    /// Retries for a single call before surfacing the error.
    pub max_retries: u32,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            max_in_flight: 16,
            rps: 8,
            burst: 16,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            max_retries: 5,
        }
    }
}

/// Per-provider concurrency, pacing, and shared backoff.
#[derive(Debug)]
pub struct ProviderLimiter {
    gates: Mutex<HashMap<Provider, Arc<Gate>>>,
    cfg: LimiterConfig,
}

#[derive(Debug)]
struct Gate {
    sem: Arc<Semaphore>,
    rps: Mutex<TokenBucket>,
    state: Mutex<GateState>,
}

#[derive(Debug)]
struct GateState {
    /// Until when no agent on this provider may issue a call.
    pause_until: Option<Instant>,
    /// Consecutive failures, driving exponential backoff growth.
    consecutive_failures: u32,
}

/// RAII proof that a call holds an in-flight slot; the slot frees on drop
/// (including panic), so a crashing call can't leak capacity.
#[derive(Debug)]
pub struct InFlight {
    _permit: OwnedSemaphorePermit,
}

impl ProviderLimiter {
    /// New limiter with the given config.
    pub fn new(cfg: LimiterConfig) -> Self {
        Self {
            gates: Mutex::new(HashMap::new()),
            cfg,
        }
    }

    /// New limiter with default config.
    pub fn with_defaults() -> Self {
        Self::new(LimiterConfig::default())
    }

    /// Configured retry ceiling for a single call.
    pub fn max_retries(&self) -> u32 {
        self.cfg.max_retries
    }

    fn gate(&self, provider: Provider) -> Arc<Gate> {
        let mut gates = self.gates.lock().expect("limiter lock");
        gates
            .entry(provider)
            .or_insert_with(|| {
                Arc::new(Gate {
                    sem: Arc::new(Semaphore::new(self.cfg.max_in_flight)),
                    rps: Mutex::new(TokenBucket::new(RateLimit {
                        per_sec: self.cfg.rps,
                        burst: self.cfg.burst,
                    })),
                    state: Mutex::new(GateState {
                        pause_until: None,
                        consecutive_failures: 0,
                    }),
                })
            })
            .clone()
    }

    /// Wait out any shared pause, pace against the token bucket, then take a
    /// concurrency slot. Returns the in-flight guard.
    pub async fn acquire(&self, provider: Provider) -> InFlight {
        let gate = self.gate(provider);
        loop {
            // Observe the shared backoff window.
            let wait = {
                let st = gate.state.lock().expect("gate state");
                st.pause_until
                    .and_then(|t| t.checked_duration_since(Instant::now()))
            };
            if let Some(w) = wait {
                tokio::time::sleep(w).await;
                continue;
            }
            // Pace: spend an rps token, sleeping briefly if none is ready.
            let ok = { gate.rps.lock().expect("gate rps").consume() };
            if ok {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let permit = gate
            .sem
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore never closed");
        InFlight { _permit: permit }
    }

    /// Record a retryable failure: grow the shared pause window for the
    /// whole provider so every agent backs off together.
    pub fn note_failure(&self, provider: Provider) {
        let gate = self.gate(provider);
        let mut st = gate.state.lock().expect("gate state");
        st.consecutive_failures = st.consecutive_failures.saturating_add(1);
        let backoff = self.backoff_for(st.consecutive_failures);
        st.pause_until = Some(Instant::now() + backoff);
    }

    /// Record a success: clear the backoff window and failure streak.
    pub fn note_success(&self, provider: Provider) {
        let gate = self.gate(provider);
        let mut st = gate.state.lock().expect("gate state");
        st.consecutive_failures = 0;
        st.pause_until = None;
    }

    /// Run `f` under the limiter, retrying retryable errors with shared
    /// backoff up to `max_retries`. Non-retryable errors return at once.
    pub async fn run_with_retry<F, Fut, T>(
        &self,
        provider: Provider,
        mut f: F,
    ) -> Result<T, LlmError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, LlmError>>,
    {
        let mut attempt = 0;
        loop {
            let permit = self.acquire(provider).await;
            match f().await {
                Ok(v) => {
                    self.note_success(provider);
                    return Ok(v);
                }
                Err(e) if e.is_retryable() && attempt < self.cfg.max_retries => {
                    drop(permit);
                    self.note_failure(provider); // shared pause; acquire() waits next loop
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Exponential backoff with full jitter, capped at `max_backoff`.
    fn backoff_for(&self, consecutive_failures: u32) -> Duration {
        let exp = consecutive_failures.saturating_sub(1).min(16);
        let scaled = self
            .cfg
            .base_backoff
            .saturating_mul(1u32 << exp)
            .min(self.cfg.max_backoff);
        // Full jitter: uniform in [0, scaled]. SystemTime nanos seed avoids
        // a fleet resuming on the same tick without pulling in an rng crate.
        let span = scaled.as_millis().max(1) as u64;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        Duration::from_millis(seed % span)
    }
}

impl Default for ProviderLimiter {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn fast_cfg() -> LimiterConfig {
        LimiterConfig {
            max_in_flight: 4,
            rps: 1000,
            burst: 1000,
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
            max_retries: 5,
        }
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let lim = ProviderLimiter::new(fast_cfg());
        let calls = AtomicU32::new(0);
        let out: Result<u32, LlmError> = lim
            .run_with_retry(Provider::OpenRouter, || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n < 2 {
                        Err(LlmError::Http {
                            status: 429,
                            body: String::new(),
                        })
                    } else {
                        Ok(n)
                    }
                }
            })
            .await;
        assert_eq!(out.unwrap(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let lim = ProviderLimiter::new(fast_cfg());
        let calls = AtomicU32::new(0);
        let out: Result<u32, LlmError> = lim
            .run_with_retry(Provider::OpenRouter, || {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(LlmError::Http {
                        status: 503,
                        body: String::new(),
                    })
                }
            })
            .await;
        assert!(out.is_err());
        // initial try + max_retries attempts
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn non_retryable_returns_immediately() {
        let lim = ProviderLimiter::new(fast_cfg());
        let calls = AtomicU32::new(0);
        let out: Result<u32, LlmError> = lim
            .run_with_retry(Provider::OpenRouter, || {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Err(LlmError::Http {
                        status: 401,
                        body: "bad key".into(),
                    })
                }
            })
            .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn success_clears_failure_streak() {
        let lim = ProviderLimiter::new(fast_cfg());
        lim.note_failure(Provider::OpenRouter);
        lim.note_failure(Provider::OpenRouter);
        lim.note_success(Provider::OpenRouter);
        // After success the pause window is cleared, so acquire is immediate.
        let _g = lim.acquire(Provider::OpenRouter).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn caps_concurrent_in_flight() {
        let cfg = LimiterConfig {
            max_in_flight: 4,
            rps: 100_000,
            burst: 100_000,
            ..fast_cfg()
        };
        let lim = Arc::new(ProviderLimiter::new(cfg));
        let inflight = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));
        let mut handles = Vec::new();
        for _ in 0..50 {
            let lim = Arc::clone(&lim);
            let inflight = Arc::clone(&inflight);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                let _g = lim.acquire(Provider::OpenRouter).await;
                let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 4,
            "in-flight exceeded the cap"
        );
    }
}
