/*
   File: crates/core/src/types.rs

   Purpose
   Data models shared across the workspace — task lifecycle, token
   accounting, session state, repo configuration. All types implement
   serde so they can round-trip through `.monkey/sessions/<id>.json`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/core/src/core/types.ts
   2026-06-03   Anubhav Sigdel  add default_provider; de-brand agent-kind docs
*/

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::repos::TechStack;

// ─── Task lifecycle ─────────────────────────────────────────────────────────

/// What a task is asking the worker to do. Drives model-tier selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Conversational coding work (default).
    Chat,
    /// Reading + summarizing existing code.
    Explain,
    /// Localized code modification.
    Edit,
    /// Multi-step refactor across files.
    Refactor,
    /// Root-cause debugging.
    Investigate,
    /// Pre-merge diff review.
    Review,
    /// Security audit.
    SecurityAudit,
    /// Generate a new project artifact (file, scaffolding, doc).
    Generate,
    /// Run engulf project-intelligence pipeline.
    Engulf,
}

/// Where a task is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Created but not yet started.
    Queued,
    /// Worker is actively processing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled by the user or coordinator.
    Cancelled,
}

impl TaskStatus {
    /// `true` once the task can no longer make progress.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Snapshot of a single task. The coordinator keeps these in memory and
/// optionally persists them under `.monkey/sessions/<id>/tasks.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    /// Stable, sortable id (uuid v7 prefixed by `task_`).
    pub id: String,
    /// What the task is doing.
    pub task_type: TaskType,
    /// Free-text description from the user or coordinator.
    pub description: String,
    /// Repos the task is scoped to (by name in the session repo map).
    pub repos: Vec<String>,
    /// Model used for this task (registry id).
    pub model_id: String,
    /// Current status.
    pub status: TaskStatus,
    /// Wall-clock start time.
    pub start_time: DateTime<Utc>,
    /// Wall-clock end time, `None` until terminal.
    pub end_time: Option<DateTime<Utc>>,
    /// Final result text (set on completion).
    pub result: Option<String>,
    /// Error message (set on failure).
    pub error: Option<String>,
    /// Token + cost usage attributed to this task.
    pub usage: TokenUsage,
}

// ─── Token accounting ───────────────────────────────────────────────────────

/// Aggregate token + cost counters. Always serialized so reports survive
/// process restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    /// Tokens read by the model.
    pub input_tokens: u64,
    /// Tokens emitted by the model.
    pub output_tokens: u64,
    /// `input_tokens + output_tokens` (denormalized for cheap rollups).
    pub total_tokens: u64,
    /// Estimated cost in USD using the registry rate at submission time.
    pub estimated_cost_usd: f64,
}

impl TokenUsage {
    /// Identity element. Use for fold-style accumulation.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Pure functional merge — does not mutate either operand.
    pub fn merge(a: &Self, b: &Self) -> Self {
        Self {
            input_tokens: a.input_tokens + b.input_tokens,
            output_tokens: a.output_tokens + b.output_tokens,
            total_tokens: a.total_tokens + b.total_tokens,
            estimated_cost_usd: a.estimated_cost_usd + b.estimated_cost_usd,
        }
    }

    /// In-place accumulator (mirrors `merge` for hot loops).
    pub fn add_assign(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.estimated_cost_usd += other.estimated_cost_usd;
    }
}

// ─── Repo + session state ───────────────────────────────────────────────────

/// One repo a session knows about. Discovered via [`crate::repos::detect_repo`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    /// Unique name within the session (usually the directory name).
    pub name: String,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Detected stack.
    pub tech_stack: TechStack,
    /// Heuristic complexity (drives model-tier selection).
    pub complexity: crate::repos::RepoComplexity,
    /// Per-repo budget overrides (None = use orchestrator-wide).
    #[serde(default)]
    pub budget_override: Option<Budget>,
}

/// Optional caps on resource use. Coordinator raises [`crate::errors::Error::BudgetExceeded`]
/// when any limit is crossed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Budget {
    /// Cap on combined tokens. None = unlimited.
    pub max_tokens: Option<u64>,
    /// Cap on USD spend. None = unlimited.
    pub max_usd: Option<f64>,
    /// Wall-clock cap. None = unlimited.
    pub max_seconds: Option<u64>,
}

/// Top-level orchestrator config (deserialized from `.monkey/config.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Default agent kind (`auto`, `codex`).
    #[serde(default = "default_agent")]
    pub default_agent: String,
    /// Default LLM provider (`openrouter`, `openai`).
    #[serde(default = "default_provider")]
    pub default_provider: String,
    /// Default model tier (`fast`, `balanced`, `powerful`).
    #[serde(default = "default_tier")]
    pub default_tier: ModelTierStr,
    /// Severity that fails the gauntlet.
    #[serde(default = "default_fail_on")]
    pub fail_on: String,
    /// Workspace-wide budget defaults.
    #[serde(default)]
    pub budget: Budget,
}

/// String form of [`crate::models::ModelTier`] for config serialization.
pub type ModelTierStr = String;

fn default_agent() -> String {
    "auto".into()
}
fn default_provider() -> String {
    "openrouter".into()
}
fn default_tier() -> ModelTierStr {
    "balanced".into()
}
fn default_fail_on() -> String {
    "high".into()
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            default_agent: default_agent(),
            default_provider: default_provider(),
            default_tier: default_tier(),
            fail_on: default_fail_on(),
            budget: Budget::default(),
        }
    }
}

/// In-memory session state. Persists as JSON to `.monkey/sessions/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session id (uuid v7 prefixed by `sess_`).
    pub id: String,
    /// When the session started.
    pub started_at: DateTime<Utc>,
    /// Repos discovered at startup.
    pub repos: HashMap<String, RepoConfig>,
    /// All tasks submitted in this session, in submission order.
    pub tasks: Vec<TaskState>,
    /// Aggregate usage across tasks.
    pub total_usage: TokenUsage,
    /// Effective config for the session.
    pub config: OrchestratorConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_usage_merge_is_pure() {
        let a = TokenUsage {
            input_tokens: 100,
            total_tokens: 100,
            ..Default::default()
        };
        let b = TokenUsage {
            output_tokens: 50,
            total_tokens: 50,
            estimated_cost_usd: 0.001,
            ..Default::default()
        };
        let merged = TokenUsage::merge(&a, &b);
        assert_eq!(merged.input_tokens, 100);
        assert_eq!(merged.output_tokens, 50);
        assert_eq!(merged.total_tokens, 150);
        // originals unchanged
        assert_eq!(a.output_tokens, 0);
        assert_eq!(b.input_tokens, 0);
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(!TaskStatus::Queued.is_terminal());
    }

    #[test]
    fn config_round_trips() {
        let c = OrchestratorConfig::default();
        let s = serde_json::to_string(&c).unwrap();
        let back: OrchestratorConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.default_agent, c.default_agent);
        assert_eq!(back.fail_on, c.fail_on);
    }
}
