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
*/

use serde::{Deserialize, Serialize};
use sysinfo::System;

/// Conservative per-agent working-set estimate, in MiB. An interactive
/// agent CLI plus a loaded model context typically sits below this; the
/// estimate is deliberately generous so the host stays responsive even
/// when every scheduled agent is busy. Override via [`AgentBudget`].
pub const DEFAULT_AGENT_MEM_MB: u64 = 512;

/// Fraction of *available* RAM that agents may collectively occupy. The
/// remainder is left for the orchestrator, the UI, and the OS.
pub const DEFAULT_MEM_HEADROOM: f64 = 0.75;

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
    /// Per-agent RAM estimate, in MiB. Must be ≥ 1; values below 1 are
    /// treated as 1 to avoid division by zero.
    pub agent_mem_mb: u64,
    /// Fraction of available RAM agents may use, in `0.0..=1.0`.
    pub mem_headroom: f64,
    /// Optional hard ceiling regardless of hardware. `None` = no extra cap.
    pub max_agents: Option<usize>,
}

impl Default for AgentBudget {
    fn default() -> Self {
        Self {
            agent_mem_mb: DEFAULT_AGENT_MEM_MB,
            mem_headroom: DEFAULT_MEM_HEADROOM,
            max_agents: None,
        }
    }
}

/// Compute how many agents may run concurrently on `cap` under `budget`.
///
/// RAM is the primary constraint: `available × headroom ÷ per-agent`. The
/// result is then guarded by `logical_cpus × 4` so a huge-RAM / few-core
/// box doesn't schedule an absurd number of CPU-bound processes, and
/// finally clamped to `budget.max_agents` when set.
///
/// @return always ≥ 1 — a single agent can always run, even on a tiny host.
pub fn max_concurrent_agents(cap: &HostCapacity, budget: &AgentBudget) -> usize {
    let headroom = budget.mem_headroom.clamp(0.0, 1.0);
    let per_agent = budget.agent_mem_mb.max(1);
    let usable_mb = (cap.available_mem_mb as f64 * headroom) as u64;
    let by_mem = (usable_mb / per_agent) as usize;

    // Guard against runaway scheduling on RAM-rich, core-poor machines.
    let cpu_guard = cap.logical_cpus.saturating_mul(4).max(1);

    let mut n = by_mem.min(cpu_guard).max(1);
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
        // 256 GiB but only 2 cores → guarded to 2 × 4 = 8.
        let n = max_concurrent_agents(&cap(256 * 1024, 2), &AgentBudget::default());
        assert_eq!(n, 8);
    }

    #[test]
    fn respects_configured_ceiling() {
        let budget = AgentBudget {
            max_agents: Some(4),
            ..AgentBudget::default()
        };
        let n = max_concurrent_agents(&cap(32 * 1024, 64), &budget);
        assert_eq!(n, 4);
    }

    #[test]
    fn detect_returns_sane_values() {
        let c = HostCapacity::detect();
        assert!(c.logical_cpus >= 1);
        // total RAM should be non-zero on any real host.
        assert!(c.total_mem_mb >= 1);
    }
}
