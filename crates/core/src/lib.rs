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
//! - [`ids`] — short prefixed IDs

pub mod errors;
pub mod ids;
pub mod models;
pub mod repos;
pub mod types;

pub use errors::Error;
pub use ids::generate_id;
pub use models::{tier_for_task, ModelRegistry, ModelSelector, ModelSpec, ModelTier, Provider};
pub use repos::{detect_repo, discover_repos, RepoComplexity, TechStack};
pub use types::{
    OrchestratorConfig, RepoConfig, SessionState, TaskState, TaskStatus, TaskType, TokenUsage,
};
