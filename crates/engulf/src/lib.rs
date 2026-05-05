/*
   File: crates/engulf/src/lib.rs

   Purpose
   "Deep-learn the codebase" pipeline. Five phases, any subset:
       scan      — stack, deps, env vars, git, CI, routes, security hints
       security  — LLM-assisted OWASP-style audit
       docs      — README/ARCHITECTURE/CONTRIBUTING drafts
       vault     — Obsidian-shaped Markdown knowledge graph
       deploy    — production-ready deployment runbook

   Output goes to .monkey/context/*.md + .monkey/vault/. Subsequent
   `monkey chat` / `monkey deck` spawns pick it up automatically.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-05-05   Anubhav Sigdel  scanner phase ported in full; pipeline now
                                 calls CodebaseScanner::scan() during the
                                 `scan` phase
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-engulf` — repo intelligence: scanner, security audit, deployer,
//! Obsidian vault writer.

pub mod deployer;
pub mod llm;
pub mod prompts;
pub mod scanner;
pub mod security;
pub mod vault;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use scanner::{
    APIRoute, CIConfig, CIType, CodebaseScanner, DepKind, DepSource, DependencyInfo, EnvVarInfo,
    FileInfo, GitInfo, HintSeverity, ScanResult, SchemaInfo, SchemaKind, SecurityHint,
    TechStackInfo,
};
pub use security::{
    audit as run_security_audit, audit_with as run_security_audit_with, AuditOptions,
    SecurityAuditResult, SecurityFinding, Severity,
};

/// User-facing config for [`run_engulf`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngulfConfig {
    /// Repo to analyze.
    pub target_path: PathBuf,
    /// Where to write `.monkey/`. Defaults to `target_path/.monkey`.
    pub output_path: Option<PathBuf>,
    /// Phases to run. Empty = all five.
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

/// Run the engulf pipeline. Returns a summary that can be printed.
///
/// Currently runs the scan phase end-to-end and stages the rest as
/// no-ops while their modules are being ported. Each follow-up commit
/// flips one phase from `phases_skipped` to `phases_completed`.
pub async fn run_engulf(config: EngulfConfig) -> anyhow::Result<EngulfSummary> {
    let target = config.target_path.canonicalize().unwrap_or(config.target_path.clone());
    let phases = if config.phases.is_empty() {
        vec![Phase::Scan, Phase::Security, Phase::Docs, Phase::Vault, Phase::Deploy]
    } else {
        config.phases.clone()
    };

    let mut summary = EngulfSummary::default();
    let mut scan_result: Option<ScanResult> = None;
    let mut audit_result: Option<SecurityAuditResult> = None;

    for phase in phases {
        match phase {
            Phase::Scan => {
                let r = scanner::scan(&target)?;
                tracing::info!(
                    files = r.files.len(),
                    deps = r.dependencies.len(),
                    routes = r.api_routes.len(),
                    "scan complete"
                );
                scan_result = Some(r);
                summary.phases_completed.push(Phase::Scan);
            }
            Phase::Security => {
                let Some(scan) = scan_result.as_ref() else {
                    summary
                        .phases_skipped
                        .push((Phase::Security, "no scan result available".into()));
                    continue;
                };
                let opts = AuditOptions {
                    provider: Some(config.provider),
                    skip_llm: false,
                    ..Default::default()
                };
                let r = security::audit_with(scan, opts).await?;
                tracing::info!(
                    findings = r.findings.len(),
                    critical = r.critical_count,
                    high = r.high_count,
                    "security audit complete"
                );
                if let Some(out) = output_path_for(&config, &target) {
                    let path = out.join("context").join("SECURITY.md");
                    write_file(&path, &r.markdown)?;
                    summary.files_written.push(path);
                }
                audit_result = Some(r);
                summary.phases_completed.push(Phase::Security);
            }
            Phase::Docs => {
                summary.phases_skipped.push((Phase::Docs, "docs phase port pending".into()));
            }
            Phase::Vault => {
                summary.phases_skipped.push((Phase::Vault, "vault phase port pending".into()));
            }
            Phase::Deploy => {
                summary.phases_skipped.push((Phase::Deploy, "deploy phase port pending".into()));
            }
        }
    }
    let _ = audit_result; // reserved for the docs/vault phases.
    Ok(summary)
}

fn output_path_for(config: &EngulfConfig, target: &std::path::Path) -> Option<PathBuf> {
    Some(
        config
            .output_path
            .clone()
            .unwrap_or_else(|| target.join(".monkey")),
    )
}

fn write_file(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)
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
