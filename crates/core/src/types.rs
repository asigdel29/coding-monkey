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

use crate::models::ModelTier;
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

/// Which host serves a local model. Documents the tiered topology so tools
/// (`monkey models`, `monkey doctor`) can show where inference runs and why a
/// large model is unreachable when the LAN box is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalHost {
    /// Runs on the Raspberry Pi itself (small quantized model, offline-capable).
    Pi,
    /// Runs on a separate LAN box (large model: e.g. GLM-5.2, Kimi K2.6).
    Lan,
}

/// A locally-served, open-weights model declared in `.monkey/config.json`.
///
/// Each entry becomes a [`crate::models::ModelSpec`] on the
/// [`crate::models::Provider::SelfHosted`] provider with its own `base_url`,
/// which is what lets a small Pi-local model and large LAN-box models coexist
/// in one registry (see [`crate::models::ModelRegistry::with_config`]). Cost is
/// always zero for local inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelDef {
    /// Model id sent to the server (e.g. `glm-5.2`), also the registry key.
    pub id: String,
    /// Human-friendly display name.
    pub display_name: String,
    /// Performance tier this model fills.
    pub tier: ModelTier,
    /// Base or full chat-completions URL of the OpenAI-compatible server.
    pub base_url: String,
    /// Env var holding this endpoint's API key, if it needs one. Usually none.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Approximate context window in tokens.
    pub context_window: u32,
    /// Where this model runs (Pi vs LAN box).
    pub host: LocalHost,
}

/// How the orchestrator chooses (and re-chooses) a model for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrchestratorStrategy {
    /// Score task difficulty to pick the initial model, then escalate to a
    /// stronger tier when a run trips an escalation trigger. The default.
    DifficultyEscalation,
    /// Static `task_type → tier` routing only; never escalate.
    TierOnly,
}

/// An agent outcome that justifies retrying a task on a stronger model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationTrigger {
    /// The run errored (e.g. the model's endpoint was unreachable).
    Failed,
    /// The run hit a guard (max turns, or stuck repeating a tool call).
    LimitReached,
}

/// Orchestration policy, deserialized from `.monkey/config.json`'s
/// `orchestrator` object. All fields default, so the section is optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorPolicy {
    /// Selection strategy.
    #[serde(default = "default_strategy")]
    pub strategy: OrchestratorStrategy,
    /// Outcomes that trigger an escalation to the next-stronger model.
    #[serde(default = "default_escalate_on")]
    pub escalate_on: Vec<EscalationTrigger>,
    /// Maximum number of escalations for a single task (a small cap keeps a
    /// failing LAN box from looping the ladder).
    #[serde(default = "default_max_escalations")]
    pub max_escalations: u32,
}

fn default_strategy() -> OrchestratorStrategy {
    OrchestratorStrategy::DifficultyEscalation
}
fn default_escalate_on() -> Vec<EscalationTrigger> {
    vec![EscalationTrigger::Failed, EscalationTrigger::LimitReached]
}
fn default_max_escalations() -> u32 {
    1
}

impl Default for OrchestratorPolicy {
    fn default() -> Self {
        Self {
            strategy: default_strategy(),
            escalate_on: default_escalate_on(),
            max_escalations: default_max_escalations(),
        }
    }
}

impl OrchestratorPolicy {
    /// Whether `trigger` should cause an escalation under this policy.
    pub fn escalates_on(&self, trigger: EscalationTrigger) -> bool {
        self.strategy != OrchestratorStrategy::TierOnly && self.escalate_on.contains(&trigger)
    }
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
    /// Preferred model id for everyday coding work, overriding tier-based
    /// selection when set (e.g. `glm-5.2`). The orchestrator still escalates
    /// to a stronger tier for hard tasks. `None` keeps pure tier routing.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Locally-served open-weights models folded into the registry at startup.
    #[serde(default)]
    pub local_models: Vec<LocalModelDef>,
    /// Model-selection and escalation policy.
    #[serde(default)]
    pub orchestrator: OrchestratorPolicy,
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
            default_model: None,
            local_models: Vec::new(),
            orchestrator: OrchestratorPolicy::default(),
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

    #[test]
    fn config_parses_local_models_and_default_model() {
        // A minimal config naming a local model deserializes via serde
        // defaults; absent legacy fields fall back without error.
        let raw = r#"{
            "default_provider": "self-hosted",
            "default_model": "glm-5.2",
            "local_models": [
                {
                    "id": "glm-5.2",
                    "display_name": "GLM-5.2 (LAN)",
                    "tier": "balanced",
                    "base_url": "http://lan-box.local:8000",
                    "context_window": 200000,
                    "host": "lan"
                }
            ]
        }"#;
        let cfg: OrchestratorConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.default_model.as_deref(), Some("glm-5.2"));
        assert_eq!(cfg.local_models.len(), 1);
        assert_eq!(cfg.local_models[0].tier, ModelTier::Balanced);
        assert_eq!(cfg.local_models[0].host, LocalHost::Lan);
    }
}
