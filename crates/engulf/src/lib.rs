/*
   File: crates/engulf/src/lib.rs

   Purpose
   "Deep-learn the codebase" pipeline. Five phases, any subset:
       scan      — stack, deps, API routes, file inventory
       security  — LLM-assisted OWASP-style audit
       docs      — README/ARCHITECTURE/CONTRIBUTING drafts
       vault     — Obsidian-shaped Markdown knowledge graph
       deploy    — production-ready deployment runbook

   Output goes to .monkey/context/*.md + .monkey/vault/.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; module layout + types
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-engulf` — repo intelligence: scanner, security audit, deployer,
//! Obsidian vault writer.

pub mod deployer;
pub mod prompts;
pub mod scanner;
pub mod security;
pub mod vault;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// User-facing config for [`run_engulf`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngulfConfig {
    /// Repo to analyze.
    pub target_path: PathBuf,
    /// Where to write `.monkey/`. Defaults to `target_path/.monkey`.
    pub output_path: Option<PathBuf>,
    /// Phases to run. Empty = all.
    pub phases: Vec<Phase>,
    /// Provider for LLM-assisted phases.
    pub provider: Provider,
    /// Whether to run interactively (prompts) or fully automatic.
    pub auto_run: bool,
}

/// Pipeline phases. Order is fixed; subset selection happens via `phases`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Filesystem scan.
    Scan,
    /// Security audit.
    Security,
    /// Doc draft generation.
    Docs,
    /// Obsidian vault writer.
    Vault,
    /// Deployment runbook.
    Deploy,
}

/// LLM provider used for phases that call out to a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic.
    Anthropic,
    /// OpenAI.
    Openai,
}

/// Run the engulf pipeline. Returns a summary that can be printed to a TTY.
///
/// TODO(0.1.x): wire each phase to its module impl. Phases currently
/// no-op while the modules are being ported crate-by-crate.
pub async fn run_engulf(_config: EngulfConfig) -> anyhow::Result<EngulfSummary> {
    Ok(EngulfSummary::default())
}

/// What [`run_engulf`] returns. Used by the CLI to render a final report.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EngulfSummary {
    /// Files written under `.monkey/`.
    pub files_written: Vec<PathBuf>,
    /// Phases that ran successfully.
    pub phases_completed: Vec<Phase>,
    /// Phases that were skipped (with reason).
    pub phases_skipped: Vec<(Phase, String)>,
}
