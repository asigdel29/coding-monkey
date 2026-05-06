/*
   File: crates/agents/src/doctor.rs

   Purpose
   Diagnose the agent runtime — which CLIs are on PATH, which API keys
   are set, which features are degraded, and what the surrounding repo
   looks like. This is the source of truth for both `monkey doctor` and
   the `pick_auto` resolver used by spawn.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/doctor.ts
   2026-05-06   Anubhav Sigdel  capture CLI versions, repo state, monkey scaffold
*/

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

use monkey_core::repos::{detect_repo, RepoComplexity, TechStack};

use crate::types::AgentKind;

/// Result of a doctor check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Whether all required checks passed.
    pub ok: bool,
    /// `claude --version` stdout, trimmed. `None` if not on PATH.
    pub claude_version: Option<String>,
    /// `codex --version` stdout, trimmed. `None` if not on PATH.
    pub codex_version: Option<String>,
    /// `git --version` stdout, trimmed. `None` if not on PATH.
    pub git_version: Option<String>,
    /// Whether `ANTHROPIC_API_KEY` is set.
    pub anthropic_key: bool,
    /// Whether `OPENAI_API_KEY` is set.
    pub openai_key: bool,
    /// Working directory the report was produced in.
    pub cwd: PathBuf,
    /// Whether `cwd` (or an ancestor) is inside a git work tree.
    pub in_git_repo: bool,
    /// Whether `cwd/.monkey/` exists (i.e. `monkey init` has been run).
    pub monkey_initialized: bool,
    /// Detected tech stack of `cwd`, if any.
    pub tech_stack: Option<TechStack>,
    /// Heuristic complexity of `cwd`, if a stack was detected.
    pub repo_complexity: Option<RepoComplexity>,
    /// Human-readable warnings.
    pub notes: Vec<String>,
}

impl DoctorReport {
    /// Whether the `claude` CLI is on PATH.
    pub fn claude_present(&self) -> bool {
        self.claude_version.is_some()
    }
    /// Whether the `codex` CLI is on PATH.
    pub fn codex_present(&self) -> bool {
        self.codex_version.is_some()
    }
    /// Whether `git` is on PATH.
    pub fn git_present(&self) -> bool {
        self.git_version.is_some()
    }
}

/// Run a full doctor check. Side effects: exec each candidate CLI with
/// `--version`, stat `cwd/.monkey/`, and walk up `cwd` looking for `.git`.
pub fn doctor() -> DoctorReport {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    doctor_at(&cwd)
}

/// Same as [`doctor`] but rooted at an explicit path. Exposed for tests.
pub fn doctor_at(cwd: &Path) -> DoctorReport {
    let claude_version = bin_version("claude");
    let codex_version = bin_version("codex");
    let git_version = bin_version("git");
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let openai_key = std::env::var("OPENAI_API_KEY").is_ok();

    let in_git_repo = find_git_root(cwd).is_some();
    let monkey_initialized = cwd.join(".monkey").is_dir();
    let detected = detect_repo(cwd).ok().flatten();
    let tech_stack = detected.as_ref().map(|r| r.tech_stack);
    let repo_complexity = detected.as_ref().map(|r| r.complexity);

    let mut notes = Vec::new();
    if claude_version.is_none() && codex_version.is_none() {
        notes.push("no agent CLI on PATH (install `claude` or `codex`)".into());
    }
    if git_version.is_none() {
        notes.push("git not found — repo detection and skills will degrade".into());
    }
    if !anthropic_key && !openai_key {
        notes.push("no LLM API key set (export ANTHROPIC_API_KEY or OPENAI_API_KEY)".into());
    }
    if !in_git_repo {
        notes.push("cwd is not inside a git work tree — ship/review will fail".into());
    }
    if !monkey_initialized {
        notes.push(".monkey/ not found — run `monkey init` to scaffold context".into());
    }

    let ok = (claude_version.is_some() || codex_version.is_some())
        && git_version.is_some()
        && (anthropic_key || openai_key);

    DoctorReport {
        ok,
        claude_version,
        codex_version,
        git_version,
        anthropic_key,
        openai_key,
        cwd: cwd.to_path_buf(),
        in_git_repo,
        monkey_initialized,
        tech_stack,
        repo_complexity,
        notes,
    }
}

/// Resolve `AgentKind::Auto` into a concrete kind, preferring `claude`
/// over `codex`. Returns `None` if neither is installed.
pub fn pick_auto(report: &DoctorReport) -> Option<AgentKind> {
    if report.claude_present() {
        Some(AgentKind::Claude)
    } else if report.codex_present() {
        Some(AgentKind::Codex)
    } else {
        None
    }
}

fn bin_version(name: &str) -> Option<String> {
    let out = Command::new(name)
        .arg("--version")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        Some(name.to_string())
    } else {
        Some(s)
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn empty_report() -> DoctorReport {
        DoctorReport {
            ok: false,
            claude_version: None,
            codex_version: None,
            git_version: None,
            anthropic_key: false,
            openai_key: false,
            cwd: PathBuf::from("."),
            in_git_repo: false,
            monkey_initialized: false,
            tech_stack: None,
            repo_complexity: None,
            notes: vec![],
        }
    }

    #[test]
    fn doctor_runs_without_panicking() {
        let r = doctor();
        if r.ok {
            assert!(r.notes.iter().all(|n| !n.is_empty()));
        }
    }

    #[test]
    fn doctor_at_detects_monkey_dir() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".monkey")).unwrap();
        let r = doctor_at(dir.path());
        assert!(r.monkey_initialized);
    }

    #[test]
    fn doctor_at_detects_git_repo() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let r = doctor_at(dir.path());
        assert!(r.in_git_repo);
    }

    #[test]
    fn doctor_at_walks_up_for_git() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let r = doctor_at(&nested);
        assert!(r.in_git_repo);
    }

    #[test]
    fn pick_auto_picks_claude_first() {
        let r = DoctorReport {
            claude_version: Some("claude 1.0".into()),
            codex_version: Some("codex 1.0".into()),
            ..empty_report()
        };
        assert_eq!(pick_auto(&r), Some(AgentKind::Claude));
    }

    #[test]
    fn pick_auto_returns_none_when_neither_installed() {
        let r = empty_report();
        assert_eq!(pick_auto(&r), None);
    }
}
