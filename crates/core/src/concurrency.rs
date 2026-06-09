/*
   File: crates/core/src/concurrency.rs

   Purpose
   Bound the number of agents that may run at once to what the host can
   actually support. Each agent (a CLI process plus its model context and
   in-flight HTTP work) costs memory; oversubscribing thrashes the box and
   makes every agent slower. The cap is derived from currently-available
   RAM, lightly guarded by the logical CPU count, then clamped to any
   user-configured ceiling. The policy always permits at least one agent.

   This is the implementation behind "run as many agents as your machine
   allows": callers probe [`HostCapacity::detect`] once, then ask
   [`max_concurrent_agents`] for the ceiling to schedule against.

   History
   Date         Author          Changes
   2026-06-03   Anubhav Sigdel  initial — RAM/CPU-aware agent concurrency cap
   2026-06-09   Anubhav Sigdel  add AgentClass; native (in-process) budget
                                 profile that drops the CPU guard so a Pi can
                                 run 100+ lightweight network-bound agents
*/

use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Conservative per-agent working-set estimate for a *PTY* agent, in MiB.
/// An interactive agent CLI plus a loaded model context typically sits
/// below this; the estimate is deliberately generous so the host stays
/// responsive even when every scheduled agent is busy. Override via
/// [`AgentBudget`].
pub const DEFAULT_AGENT_MEM_MB: u64 = 512;

/// Per-agent working-set estimate for a *native* (in-process) agent, in
/// MiB. A native agent is a tokio task plus its transcript and one
/// in-flight HTTP buffer — it is network-bound and mostly idle, so its
/// footprint is roughly two orders of magnitude below a PTY agent's.
pub const DEFAULT_NATIVE_AGENT_MEM_MB: u64 = 12;

/// Fraction of *available* RAM that agents may collectively occupy. The
/// remainder is left for the orchestrator, the UI, and the OS.
pub const DEFAULT_MEM_HEADROOM: f64 = 0.75;

/// Default RAM floor for native scheduling, in MiB. The memory watchdog
/// stops admitting new native agents once available RAM drops below this,
/// leaving room for the orchestrator and OS. (Consumed by the watchdog;
/// carried on [`AgentBudget`] so policy lives in one place.)
pub const DEFAULT_MEM_FLOOR_MB: u64 = 512;

/// Default hard ceiling on concurrent *native* agents. RAM alone would
/// permit far more on a big host, but holding that many live HTTP
/// connections is pointless — the provider rate limiter is the real
/// constraint past this point. Keeps a Pi 5 / 8 GB around 100–128.
pub const DEFAULT_NATIVE_MAX_AGENTS: usize = 128;

/// How an agent runs, which determines its resource profile.
///
/// - [`AgentClass::Pty`] — a heavyweight external CLI process spawned in a
///   PTY (`codex`, Claude Code, Hermes). Hundreds of MiB each; CPU-bound
///   while active, so the `cpus × 4` guard applies.
/// - [`AgentClass::Native`] — a lightweight in-process tokio task driving
///   the LLM client directly. ~12 MiB each; network-bound and mostly idle,
///   so the CPU guard does *not* apply — the real ceilings are available
///   RAM (via the watchdog) and the provider rate limiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentClass {
    /// Heavyweight external CLI agent in a PTY.
    Pty,
    /// Lightweight in-process native agent.
    Native,
}

/// A snapshot of host capacity relevant to scheduling agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCapacity {
    /// Total physical RAM, in MiB.
    pub total_mem_mb: u64,
    /// RAM the OS reports as available right now, in MiB.
    pub available_mem_mb: u64,
    /// Logical CPUs (hardware threads) visible to the process.
    pub logical_cpus: usize,
}

impl HostCapacity {
    /// Probe the host for RAM and CPU counts.
    ///
    /// Side effect: reads `/proc`-equivalent system memory once. Cheap,
    /// but not free — call once and reuse the snapshot rather than calling
    /// per scheduling decision. `available_mem_mb` reflects the moment of
    /// the call and will drift as the machine is used.
    pub fn detect() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        // sysinfo reports memory in bytes (0.30+).
        let total_mem_mb = sys.total_memory() / (1024 * 1024);
        let available_mem_mb = sys.available_memory() / (1024 * 1024);
        let logical_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            total_mem_mb,
            available_mem_mb,
            logical_cpus,
        }
    }
}

/// Policy knobs for deriving the agent cap from a [`HostCapacity`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentBudget {
    /// Which execution model this budget describes.
    pub class: AgentClass,
    /// Per-agent RAM estimate, in MiB. Must be ≥ 1; values below 1 are
    /// treated as 1 to avoid division by zero.
    pub agent_mem_mb: u64,
    /// Fraction of available RAM agents may use, in `0.0..=1.0`.
    pub mem_headroom: f64,
    /// Apply the `logical_cpus × 4` guard. True for CPU-bound PTY agents;
    /// false for network-bound native agents (the guard would wrongly cap a
    /// Pi at 16 when it can host 100+ idle-waiting tasks).
    pub bind_to_cpu_guard: bool,
    /// RAM floor for the watchdog, in MiB. New agents are not admitted once
    /// available RAM drops below this.
    pub mem_floor_mb: u64,
    /// Optional hard ceiling regardless of hardware. `None` = no extra cap.
    pub max_agents: Option<usize>,
}

impl AgentBudget {
    /// Budget for heavyweight PTY agents — the original policy: 512 MiB
    /// each, `cpus × 4` guard on, no extra ceiling.
    pub fn pty() -> Self {
        Self {
            class: AgentClass::Pty,
            agent_mem_mb: DEFAULT_AGENT_MEM_MB,
            mem_headroom: DEFAULT_MEM_HEADROOM,
            bind_to_cpu_guard: true,
            mem_floor_mb: DEFAULT_MEM_FLOOR_MB,
            max_agents: None,
        }
    }

    /// Budget for lightweight native agents — ~12 MiB each, CPU guard off,
    /// clamped to [`DEFAULT_NATIVE_MAX_AGENTS`] so RAM-rich hosts don't try
    /// to hold an absurd number of live HTTP connections.
    pub fn native() -> Self {
        Self {
            class: AgentClass::Native,
            agent_mem_mb: DEFAULT_NATIVE_AGENT_MEM_MB,
            mem_headroom: DEFAULT_MEM_HEADROOM,
            bind_to_cpu_guard: false,
            mem_floor_mb: DEFAULT_MEM_FLOOR_MB,
            max_agents: Some(DEFAULT_NATIVE_MAX_AGENTS),
        }
    }
}

impl Default for AgentBudget {
    /// Defaults to the PTY profile, preserving the original behavior for
    /// existing callers (the deck's legacy terminal cap).
    fn default() -> Self {
        Self::pty()
    }
}

/// Compute how many agents may run concurrently on `cap` under `budget`.
///
/// RAM is the primary constraint: `available × headroom ÷ per-agent`. For
/// CPU-bound PTY agents (`budget.bind_to_cpu_guard == true`) the result is
/// then guarded by `logical_cpus × 4` so a huge-RAM / few-core box doesn't
/// schedule an absurd number of processes. Native agents are network-bound
/// and skip that guard — they are bounded instead by available RAM (and, at
/// runtime, by the memory watchdog and provider rate limiter). The result
/// is finally clamped to `budget.max_agents` when set.
///
/// @return always ≥ 1 — a single agent can always run, even on a tiny host.
pub fn max_concurrent_agents(cap: &HostCapacity, budget: &AgentBudget) -> usize {
    let headroom = budget.mem_headroom.clamp(0.0, 1.0);
    let per_agent = budget.agent_mem_mb.max(1);
    let usable_mb = (cap.available_mem_mb as f64 * headroom) as u64;
    let by_mem = (usable_mb / per_agent) as usize;

    let mut n = by_mem.max(1);
    if budget.bind_to_cpu_guard {
        // Guard against runaway scheduling on RAM-rich, core-poor machines.
        let cpu_guard = cap.logical_cpus.saturating_mul(4).max(1);
        n = n.min(cpu_guard);
    }
    if let Some(ceiling) = budget.max_agents {
        n = n.min(ceiling.max(1));
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(avail_mb: u64, cpus: usize) -> HostCapacity {
        HostCapacity {
            total_mem_mb: avail_mb,
            available_mem_mb: avail_mb,
            logical_cpus: cpus,
        }
    }

    #[test]
    fn always_allows_at_least_one() {
        // 64 MiB available, 1 CPU — far below one agent's budget.
        let n = max_concurrent_agents(&cap(64, 1), &AgentBudget::default());
        assert_eq!(n, 1);
    }

    #[test]
    fn scales_with_available_ram() {
        // 32 GiB, plenty of cores: 32768 × 0.75 / 512 = 48 by RAM.
        let budget = AgentBudget {
            max_agents: None,
            ..AgentBudget::default()
        };
        let n = max_concurrent_agents(&cap(32 * 1024, 64), &budget);
        assert_eq!(n, 48);
    }

    #[test]
    fn cpu_guards_ram_rich_hosts() {
        // 256 GiB but only 2 cores → guarded to 2 × 4 = 8 (PTY).
        let n = max_concurrent_agents(&cap(256 * 1024, 2), &AgentBudget::pty());
        assert_eq!(n, 8);
    }

    #[test]
    fn respects_configured_ceiling() {
        let budget = AgentBudget {
            max_agents: Some(4),
            ..AgentBudget::pty()
        };
        let n = max_concurrent_agents(&cap(32 * 1024, 64), &budget);
        assert_eq!(n, 4);
    }

    #[test]
    fn native_skips_cpu_guard_and_scales_on_pi() {
        // Pi 5 / 8 GiB / 4 cores: native = 8192 × 0.75 / 12 = 512 by RAM,
        // CPU guard does NOT apply, clamped to the native ceiling (128).
        let n = max_concurrent_agents(&cap(8 * 1024, 4), &AgentBudget::native());
        assert_eq!(n, DEFAULT_NATIVE_MAX_AGENTS);
        assert!(n >= 100, "native must reach 100+ on a Pi 5");
    }

    #[test]
    fn pty_caps_low_on_same_pi() {
        // Same Pi under the PTY profile: 8192 × 0.75 / 512 = 12 by RAM,
        // cpu_guard = 16 → 12. Far below native, as expected.
        let n = max_concurrent_agents(&cap(8 * 1024, 4), &AgentBudget::pty());
        assert_eq!(n, 12);
    }

    #[test]
    fn native_respects_explicit_lower_ceiling() {
        let budget = AgentBudget {
            max_agents: Some(50),
            ..AgentBudget::native()
        };
        let n = max_concurrent_agents(&cap(8 * 1024, 4), &budget);
        assert_eq!(n, 50);
    }

    #[test]
    fn detect_returns_sane_values() {
        let c = HostCapacity::detect();
        assert!(c.logical_cpus >= 1);
        // total RAM should be non-zero on any real host.
        assert!(c.total_mem_mb >= 1);
    }
}
