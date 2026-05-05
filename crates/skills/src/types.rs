/*
   File: crates/skills/src/types.rs

   Purpose
   The Skill trait + structured input/output every skill produces.
   Skills are composable: any skill can call others by looking them
   up in the registry handed to it via `SkillContext`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full port from packages/skills/src/types.ts
*/

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use monkey_core::{ModelTier, TokenUsage};

/// Severity of a skill finding. Same shape as the rest of the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational only. Never blocks.
    Info,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical. Always blocks.
    Critical,
}

impl Severity {
    /// Numeric rank used by the `--fail-on` gate.
    pub fn rank(self) -> u8 {
        match self {
            Severity::Info => 0,
            Severity::Low => 1,
            Severity::Medium => 2,
            Severity::High => 3,
            Severity::Critical => 4,
        }
    }

    /// Parse a `--fail-on` argument. Defaults to High on unknown input.
    pub fn from_threshold(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "critical" => Severity::Critical,
            "high" => Severity::High,
            "medium" => Severity::Medium,
            "low" => Severity::Low,
            _ => Severity::High,
        }
    }

    /// Uppercase label used in markdown.
    pub fn upper(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Low => "LOW",
            Severity::Medium => "MEDIUM",
            Severity::High => "HIGH",
            Severity::Critical => "CRITICAL",
        }
    }
}

/// One finding from a skill — uniform shape across review / investigate /
/// cso / ship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFinding {
    /// Severity bucket.
    pub severity: Severity,
    /// Short title.
    pub title: String,
    /// Source file, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// 1-indexed line number, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// One-sentence recommended remediation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    /// Free-form detail block (multiline OK).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Per-invocation context handed to every skill.
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Working directory.
    pub cwd: PathBuf,
    /// Base branch override (review/ship).
    pub base_branch: Option<String>,
    /// Persist reports under `.monkey/skills/<name>/`.
    pub persist_reports: bool,
    /// Default LLM provider.
    pub provider: Option<Provider>,
    /// Force a model tier for this skill (telemetry / cost knob).
    pub force_tier: Option<ModelTier>,
    /// Skip side-effecting actions (push, commit) when true.
    pub dry_run: bool,
}

impl SkillContext {
    /// Build a default context rooted at `cwd`.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            base_branch: None,
            persist_reports: false,
            provider: None,
            force_tier: None,
            dry_run: false,
        }
    }
}

/// LLM provider preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic.
    Anthropic,
    /// OpenAI.
    Openai,
}

/// What every skill returns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillResult {
    /// Whether the skill passed the configured gate.
    pub ok: bool,
    /// Convenience: `!ok` when `ok=false` is due to a blocking failure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// One-line summary.
    pub summary: String,
    /// All findings.
    pub findings: Vec<SkillFinding>,
    /// Pre-rendered markdown report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Aggregate token usage from any LLM calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// Free-form structured payload (skill-specific).
    #[serde(default)]
    pub data: serde_json::Value,
}

/// One skill. Implementations live under `crates/skills/src/skills/`.
#[async_trait]
pub trait Skill: Send + Sync {
    /// Stable name used by the registry and CLI.
    fn name(&self) -> &str;
    /// Short description for `monkey skill list`.
    fn description(&self) -> &str;
    /// Category bucket (`"review"`, `"debug"`, `"security"`, `"release"`).
    fn category(&self) -> &str;
    /// Other skill names this one composes (used for `--persist` layout).
    fn composes(&self) -> &[&str] {
        &[]
    }
    /// Run the skill.
    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult>;
}

/// Helper: in-place merge of optional usages.
pub fn merge_usage(a: Option<TokenUsage>, b: TokenUsage) -> TokenUsage {
    match a {
        Some(u) => TokenUsage::merge(&u, &b),
        None => b,
    }
}
