/*
   File: crates/core/src/errors.rs

   Purpose
   Workspace-wide error enum. Each variant carries enough context for
   the CLI to produce a useful, single-screen error message. Internal
   crates should bubble these up via `?`; only the binary should match
   on variants for exit-code mapping.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/core/src/core/errors.ts
*/

use thiserror::Error;

/// Top-level error type for the workspace. Variants are intentionally
/// coarse-grained — fine-grained context goes in `#[source]` chains, not
/// new variants.
#[derive(Error, Debug)]
pub enum Error {
    /// Budget for tokens, cost, or time was exceeded.
    #[error("budget exceeded: {kind} (limit: {limit}, used: {used})")]
    BudgetExceeded {
        /// What was capped (e.g. "tokens", "usd", "seconds").
        kind: String,
        /// The configured limit.
        limit: String,
        /// What we actually consumed.
        used: String,
    },

    /// An external CLI we depend on (claude, codex, gh, git) is missing or unusable.
    #[error("external tool unavailable: {tool}: {reason}")]
    ExternalTool {
        /// The binary we tried to invoke.
        tool: String,
        /// Why it failed (not on PATH, wrong version, exited non-zero).
        reason: String,
    },

    /// Configuration could not be parsed or violates an invariant.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// Required environment variable is missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    /// I/O failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic error wrapper for adapter boundaries.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    /// Convenience constructor for [`Error::ExternalTool`].
    pub fn external_tool(tool: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ExternalTool {
            tool: tool.into(),
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`Error::InvalidConfig`].
    pub fn invalid_config(msg: impl Into<String>) -> Self {
        Self::InvalidConfig(msg.into())
    }
}

/// Crate-wide `Result` alias.
pub type Result<T> = std::result::Result<T, Error>;
