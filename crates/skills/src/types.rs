/*
   File: crates/skills/src/types.rs

   Purpose
   The Skill trait + the structured input/output every skill produces.
   Skills are composable: a skill can call others by looking them up in
   the registry passed via `SkillContext`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Severity threshold for the `--fail-on` gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational only, never fails.
    Info,
    /// Low: surfaced but not blocking by default.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// Critical: always fails the gauntlet.
    Critical,
}

/// Per-invocation context handed to every skill.
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Working directory.
    pub cwd: PathBuf,
    /// Base branch override, if any.
    pub base_branch: Option<String>,
    /// Whether to persist reports under `.monkey/skills/<name>/`.
    pub persist_reports: bool,
}

/// What every skill returns. `markdown` is the renderable report; `ok`
/// is the gate verdict at the configured fail-on severity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResult {
    /// Whether the skill passed the configured gate.
    pub ok: bool,
    /// One-line summary for log lines.
    pub summary: String,
    /// Full markdown report.
    pub markdown: Option<String>,
}

/// One skill. Implementations live under `crates/skills/src/skills/`.
#[async_trait]
pub trait Skill: Send + Sync {
    /// Stable name used by the registry and CLI.
    fn name(&self) -> &str;
    /// Short description for `monkey skill list`.
    fn description(&self) -> &str;
    /// Run the skill.
    async fn run(
        &self,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult>;
}
