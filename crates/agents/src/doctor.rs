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
   2026-06-03   Anubhav Sigdel  report OpenRouter key; codex-only pick_auto
   2026-06-09   Anubhav Sigdel  detect codex/claude-code/hermes harnesses
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
    /// `codex --version` stdout, trimmed. `None` if not on PATH.
    pub codex_version: Option<String>,
    /// Claude Code (`claude --version`) stdout, trimmed. `None` if absent.
    pub claude_code_version: Option<String>,
    /// Hermes (`hermes --version`) stdout, trimmed. `None` if absent.
    pub hermes_version: Option<String>,
    /// `git --version` stdout, trimmed. `None` if not on PATH.
    pub git_version: Option<String>,
    /// Whether `OPENROUTER_API_KEY` is set.
    pub openrouter_key: bool,
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
    /// Whether the `codex` CLI is on PATH.
    pub fn codex_present(&self) -> bool {
        self.codex_version.is_some()
    }
    /// Whether the Claude Code CLI is on PATH.
    pub fn claude_code_present(&self) -> bool {
        self.claude_code_version.is_some()
    }
    /// Whether the Hermes CLI is on PATH.
    pub fn hermes_present(&self) -> bool {
        self.hermes_version.is_some()
    }
    /// Whether any external harness CLI is installed.
    pub fn any_harness_present(&self) -> bool {
        self.codex_present() || self.claude_code_present() || self.hermes_present()
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
    let codex_version = bin_version("codex");
    let claude_code_version = bin_version("claude");
    let hermes_version = bin_version("hermes");
    let git_version = bin_version("git");
    let openrouter_key = std::env::var("OPENROUTER_API_KEY").is_ok();
    let openai_key = std::env::var("OPENAI_API_KEY").is_ok();

    let in_git_repo = find_git_root(cwd).is_some();
    let monkey_initialized = cwd.join(".monkey").is_dir();
    let detected = detect_repo(cwd).ok().flatten();
    let tech_stack = detected.as_ref().map(|r| r.tech_stack);
    let repo_complexity = detected.as_ref().map(|r| r.complexity);

    let mut notes = Vec::new();
    if codex_version.is_none() && claude_code_version.is_none() && hermes_version.is_none() {
        notes.push(
            "no external agent harness on PATH (install `codex`, `claude`, or `hermes` for the \
             PTY path — the native engine needs only an API key)"
                .into(),
        );
    }
    if git_version.is_none() {
        notes.push("git not found — repo detection and skills will degrade".into());
    }
    if !openrouter_key && !openai_key {
        notes.push("no LLM API key set (export OPENROUTER_API_KEY or OPENAI_API_KEY)".into());
    }
    if !in_git_repo {
        notes.push("cwd is not inside a git work tree — ship/review will fail".into());
    }
    if !monkey_initialized {
        notes.push(".monkey/ not found — run `monkey init` to scaffold context".into());
    }

    // The API path only needs a key + git; the codex CLI is optional and
    // only required for the interactive REPL hand-off.
    let ok = git_version.is_some() && (openrouter_key || openai_key);

    DoctorReport {
        ok,
        codex_version,
        claude_code_version,
        hermes_version,
        git_version,
        openrouter_key,
        openai_key,
        cwd: cwd.to_path_buf(),
        in_git_repo,
        monkey_initialized,
        tech_stack,
        repo_complexity,
        notes,
    }
}

/// Resolve `AgentKind::Auto` into a concrete harness, preferring `codex`,
/// then Claude Code, then Hermes. Returns `None` if none are installed.
pub fn pick_auto(report: &DoctorReport) -> Option<AgentKind> {
    if report.codex_present() {
        Some(AgentKind::Codex)
    } else if report.claude_code_present() {
        Some(AgentKind::ClaudeCode)
    } else if report.hermes_present() {
        Some(AgentKind::Hermes)
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
            codex_version: None,
            claude_code_version: None,
            hermes_version: None,
            git_version: None,
            openrouter_key: false,
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
    fn pick_auto_picks_codex_when_present() {
        let r = DoctorReport {
            codex_version: Some("codex 1.0".into()),
            ..empty_report()
        };
        assert_eq!(pick_auto(&r), Some(AgentKind::Codex));
    }

    #[test]
    fn pick_auto_returns_none_when_neither_installed() {
        let r = empty_report();
        assert_eq!(pick_auto(&r), None);
    }

    #[test]
    fn pick_auto_prefers_codex_then_claude_then_hermes() {
        let claude_only = DoctorReport {
            claude_code_version: Some("claude 1.0".into()),
            ..empty_report()
        };
        assert_eq!(pick_auto(&claude_only), Some(AgentKind::ClaudeCode));

        let hermes_only = DoctorReport {
            hermes_version: Some("hermes 1.0".into()),
            ..empty_report()
        };
        assert_eq!(pick_auto(&hermes_only), Some(AgentKind::Hermes));

        let both = DoctorReport {
            codex_version: Some("codex".into()),
            claude_code_version: Some("claude".into()),
            ..empty_report()
        };
        assert_eq!(pick_auto(&both), Some(AgentKind::Codex));
    }

    #[test]
    fn harness_binary_and_context_file_mapping() {
        assert_eq!(AgentKind::Codex.binary(), Some("codex"));
        assert_eq!(AgentKind::ClaudeCode.binary(), Some("claude"));
        assert_eq!(AgentKind::Hermes.binary(), Some("hermes"));
        assert_eq!(AgentKind::Auto.binary(), None);
        assert_eq!(AgentKind::ClaudeCode.context_file(), "CLAUDE.md");
    }
}
