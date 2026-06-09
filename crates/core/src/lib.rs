/*
   File: crates/core/src/lib.rs

   Purpose
   Foundational types, errors, model registry, and repo detection. Every
   other crate in the workspace depends on `monkey-core`. Keep this crate
   small, dependency-light, and free of side effects (no I/O at module
   load time, no panicking constructors).

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port; types + errors + model
                                 registry + repo detector
   2026-06-03   Anubhav Sigdel  add concurrency module (RAM/CPU agent cap)
   2026-06-09   Anubhav Sigdel  add ratelimit module (shared token bucket)
*/

#![deny(missing_debug_implementations)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-core` — shared primitives for the coding-monkey workspace.
//!
//! - [`types`] — task/usage/session/repo data models
//! - [`errors`] — workspace-wide error enum
//! - [`models`] — model registry + tier selector
//! - [`repos`] — repo detection (stack, complexity)
//! - [`concurrency`] — RAM/CPU-aware agent concurrency cap
//! - [`ratelimit`] — shared token-bucket rate limiter
//! - [`ids`] — short prefixed IDs

/// RAM/CPU-aware policy for how many agents may run at once.
pub mod concurrency;
/// Workspace-wide error enum bubbled up via `?` from every internal crate.
pub mod errors;
/// Short, prefixed, time-sortable IDs for tasks, workers, and sessions.
pub mod ids;
/// Model registry and tier-based selector — the "right model for the job" router.
pub mod models;
/// Heuristic detection of a repo's tech stack and complexity from disk.
pub mod repos;
/// Shared token-bucket rate limiter (deck WS limits, provider call limits).
pub mod ratelimit;
/// Serde data models shared across the workspace (tasks, sessions, configs).
pub mod types;

pub use concurrency::{max_concurrent_agents, AgentBudget, AgentClass, HostCapacity};
pub use ratelimit::{RateLimit, TokenBucket};
pub use errors::Error;
pub use ids::generate_id;
pub use models::{tier_for_task, ModelRegistry, ModelSelector, ModelSpec, ModelTier, Provider};
pub use repos::{detect_repo, discover_repos, RepoComplexity, TechStack};
pub use types::{
    OrchestratorConfig, RepoConfig, SessionState, TaskState, TaskStatus, TaskType, TokenUsage,
};
