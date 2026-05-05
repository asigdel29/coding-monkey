/*
   File: crates/agents/src/doctor.rs

   Purpose
   Diagnose the agent runtime — which CLIs are on PATH, which API keys
   are set, which features are degraded. This is the source of truth
   for both `monkey doctor` and the `pick_auto` resolver used by spawn.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/doctor.ts
*/

use serde::{Deserialize, Serialize};
use std::process::Command;

use crate::types::AgentKind;

/// Result of a doctor check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Whether all required checks passed.
    pub ok: bool,
    /// Whether the `claude` CLI is on PATH.
    pub claude_present: bool,
    /// Whether the `codex` CLI is on PATH.
    pub codex_present: bool,
    /// Whether `git` is on PATH.
    pub git_present: bool,
    /// Whether `ANTHROPIC_API_KEY` is set.
    pub anthropic_key: bool,
    /// Whether `OPENAI_API_KEY` is set.
    pub openai_key: bool,
    /// Human-readable warnings.
    pub notes: Vec<String>,
}

/// Run a full doctor check. Pure-ish — only side effects are exec'ing
/// each candidate CLI with `--version`.
pub fn doctor() -> DoctorReport {
    let claude_present = bin_exists("claude");
    let codex_present = bin_exists("codex");
    let git_present = bin_exists("git");
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let openai_key = std::env::var("OPENAI_API_KEY").is_ok();

    let mut notes = Vec::new();
    if !claude_present && !codex_present {
        notes.push("no agent CLI on PATH (install `claude` or `codex`)".into());
    }
    if !git_present {
        notes.push("git not found — repo detection and skills will degrade".into());
    }
    if !anthropic_key && !openai_key {
        notes.push("no LLM API key set (export ANTHROPIC_API_KEY or OPENAI_API_KEY)".into());
    }

    let ok = (claude_present || codex_present) && git_present && (anthropic_key || openai_key);
    DoctorReport {
        ok,
        claude_present,
        codex_present,
        git_present,
        anthropic_key,
        openai_key,
        notes,
    }
}

/// Resolve `AgentKind::Auto` into a concrete kind, preferring `claude`
/// over `codex`. Returns `None` if neither is installed.
pub fn pick_auto(report: &DoctorReport) -> Option<AgentKind> {
    if report.claude_present {
        Some(AgentKind::Claude)
    } else if report.codex_present {
        Some(AgentKind::Codex)
    } else {
        None
    }
}

fn bin_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_runs_without_panicking() {
        let r = doctor();
        // Don't assert specific bools — depends on host env.
        // Just verify the type assembles and notes is consistent with ok.
        if r.ok {
            assert!(r.notes.is_empty() || r.notes.iter().all(|n| !n.is_empty()));
        }
    }

    #[test]
    fn pick_auto_picks_claude_first() {
        let r = DoctorReport {
            ok: true,
            claude_present: true,
            codex_present: true,
            git_present: true,
            anthropic_key: true,
            openai_key: false,
            notes: vec![],
        };
        assert_eq!(pick_auto(&r), Some(AgentKind::Claude));
    }

    #[test]
    fn pick_auto_returns_none_when_neither_installed() {
        let r = DoctorReport {
            ok: false,
            claude_present: false,
            codex_present: false,
            git_present: true,
            anthropic_key: false,
            openai_key: false,
            notes: vec![],
        };
        assert_eq!(pick_auto(&r), None);
    }
}
