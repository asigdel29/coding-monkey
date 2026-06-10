/*
   File: crates/core/src/watchdog.rs

   Purpose
   Runtime admission control for native agents. Where `concurrency` decides
   a *static* ceiling at startup, the watchdog makes a *live* decision: it
   samples available RAM and refuses to admit new agents once the host
   drops below a floor, so a burst of work can't push a small box (a
   Raspberry Pi) into swap-thrash or the OOM killer. Already-running agents
   are never touched — they run to completion and drain the pressure.

   Two design points keep it cheap and stable:
     - Sampling `sysinfo` is throttled (default once per second); admission
       reads the cached figure between refreshes.
     - A short hysteresis (N consecutive sub-floor samples) absorbs the
       transient dips that page-cache pressure produces, so the gate
       doesn't flap open and shut.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — RAM-floor admission control with
                                 throttled sampling and hysteresis
*/

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::System;

use crate::concurrency::DEFAULT_MEM_FLOOR_MB;

/// Default gap between `sysinfo` memory refreshes.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Default number of consecutive sub-floor samples required before the
/// watchdog denies admission. Absorbs transient dips.
pub const DEFAULT_HYSTERESIS_STRIKES: u32 = 3;

/// Returned by [`MemoryWatchdog::admit`] when the host is below the RAM
/// floor and the scheduler should hold off on starting new agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionDenied {
    /// Available RAM observed at the time of denial, in MiB.
    pub available_mb: u64,
    /// Configured floor, in MiB.
    pub floor_mb: u64,
}

impl std::fmt::Display for AdmissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "memory floor reached: {} MiB available < {} MiB floor",
            self.available_mb, self.floor_mb
        )
    }
}

impl std::error::Error for AdmissionDenied {}

/// Live RAM-floor admission gate for native-agent scheduling.
pub struct MemoryWatchdog {
    floor_mb: u64,
    refresh_interval: Duration,
    hysteresis_strikes: u32,
    inner: Mutex<Inner>,
}

struct Inner {
    sys: System,
    last_refresh: Instant,
    available_mb: u64,
    consecutive_low: u32,
}

impl std::fmt::Debug for MemoryWatchdog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryWatchdog")
            .field("floor_mb", &self.floor_mb)
            .field("refresh_interval", &self.refresh_interval)
            .field("hysteresis_strikes", &self.hysteresis_strikes)
            .finish_non_exhaustive()
    }
}

impl MemoryWatchdog {
    /// Build a watchdog guarding `floor_mb` with default sampling and
    /// hysteresis. Reads memory once up front to seed the cached figure.
    pub fn new(floor_mb: u64) -> Self {
        Self::with_policy(
            floor_mb,
            DEFAULT_REFRESH_INTERVAL,
            DEFAULT_HYSTERESIS_STRIKES,
        )
    }

    /// Build a watchdog with explicit sampling interval and hysteresis.
    pub fn with_policy(floor_mb: u64, refresh_interval: Duration, hysteresis_strikes: u32) -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        let available_mb = sys.available_memory() / (1024 * 1024);
        Self {
            floor_mb,
            refresh_interval,
            hysteresis_strikes: hysteresis_strikes.max(1),
            inner: Mutex::new(Inner {
                sys,
                last_refresh: Instant::now(),
                available_mb,
                consecutive_low: 0,
            }),
        }
    }

    /// Most recent available-RAM reading, in MiB. Cached between refreshes.
    pub fn available_mb(&self) -> u64 {
        self.inner.lock().expect("watchdog lock").available_mb
    }

    /// The configured RAM floor, in MiB.
    pub fn floor_mb(&self) -> u64 {
        self.floor_mb
    }

    /// Decide whether a new agent may start right now.
    ///
    /// Refreshes the memory reading if the throttle interval has elapsed,
    /// updates the consecutive-sub-floor counter, and denies once that
    /// counter reaches the hysteresis threshold. Cheap to call on the
    /// scheduling hot path: at most one `sysinfo` refresh per interval.
    ///
    /// @return `Ok(())` to admit, or [`AdmissionDenied`] to hold off.
    pub fn admit(&self) -> Result<(), AdmissionDenied> {
        let mut g = self.inner.lock().expect("watchdog lock");
        let now = Instant::now();
        if now.duration_since(g.last_refresh) >= self.refresh_interval {
            g.sys.refresh_memory();
            g.available_mb = g.sys.available_memory() / (1024 * 1024);
            g.last_refresh = now;
            g.consecutive_low = next_strike_count(g.available_mb, self.floor_mb, g.consecutive_low);
        }
        if g.consecutive_low >= self.hysteresis_strikes {
            Err(AdmissionDenied {
                available_mb: g.available_mb,
                floor_mb: self.floor_mb,
            })
        } else {
            Ok(())
        }
    }
}

impl Default for MemoryWatchdog {
    fn default() -> Self {
        Self::new(DEFAULT_MEM_FLOOR_MB)
    }
}

/// Pure hysteresis transition: given a fresh `available_mb` sample, the
/// `floor_mb`, and the current consecutive-sub-floor count, return the next
/// count. Below floor increments; at/above floor resets to zero. Factored
/// out so the policy is unit-testable without touching real memory.
fn next_strike_count(available_mb: u64, floor_mb: u64, consecutive_low: u32) -> u32 {
    if available_mb < floor_mb {
        consecutive_low.saturating_add(1)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn above_floor_resets_strikes() {
        assert_eq!(next_strike_count(1000, 512, 2), 0);
    }

    #[test]
    fn single_dip_does_not_trip() {
        // One sub-floor sample yields one strike — below the default of 3.
        let c = next_strike_count(100, 512, 0);
        assert_eq!(c, 1);
        assert!(c < DEFAULT_HYSTERESIS_STRIKES);
    }

    #[test]
    fn consecutive_dips_accumulate_to_threshold() {
        let mut c = 0;
        for _ in 0..DEFAULT_HYSTERESIS_STRIKES {
            c = next_strike_count(100, 512, c);
        }
        assert_eq!(c, DEFAULT_HYSTERESIS_STRIKES);
    }

    #[test]
    fn recovery_after_dips_resets() {
        let c = next_strike_count(100, 512, 2);
        assert_eq!(c, 3);
        let c = next_strike_count(900, 512, c);
        assert_eq!(c, 0);
    }

    #[test]
    fn floor_zero_always_admits() {
        // `available < 0` is impossible, so a zero floor never trips a strike
        // regardless of the host's actual free memory.
        let wd = MemoryWatchdog::new(0);
        assert!(wd.admit().is_ok());
        assert_eq!(wd.floor_mb(), 0);
    }
}
