/*
   File: crates/agents/src/types.rs

   Purpose
   Shared types for the agents crate. Kept separate so doctor/spawn can
   evolve without touching the public surface in lib.rs.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/agents/src/types.ts
   2026-06-03   Anubhav Sigdel  drop legacy agent kind; codex-only roster
   2026-06-09   Anubhav Sigdel  harness roster: codex, claude-code, hermes
*/

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which agent harness (external CLI) to spawn. Lets users bring the agent
/// they already use; `Auto` resolves to whichever is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    /// Pick whichever supported harness is installed.
    Auto,
    /// The `codex` CLI.
    Codex,
    /// The Claude Code CLI (`claude`).
    ClaudeCode,
    /// The Hermes CLI (`hermes`).
    Hermes,
}

impl AgentKind {
    /// The CLI binary this harness launches. `Auto` has no fixed binary —
    /// it is resolved to a concrete harness by the doctor first.
    pub fn binary(self) -> Option<&'static str> {
        match self {
            AgentKind::Codex => Some("codex"),
            AgentKind::ClaudeCode => Some("claude"),
            AgentKind::Hermes => Some("hermes"),
            AgentKind::Auto => None,
        }
    }

    /// The per-harness context file name under `.monkey/context/`. Lets a
    /// user keep harness-specific guidance (e.g. an existing `CLAUDE.md`).
    pub fn context_file(self) -> &'static str {
        match self {
            AgentKind::Codex => "CODEX.md",
            AgentKind::ClaudeCode => "CLAUDE.md",
            AgentKind::Hermes => "HERMES.md",
            AgentKind::Auto => "AGENT.md",
        }
    }
}

/// Options for [`crate::spawn::spawn_agent`].
#[derive(Debug, Clone)]
pub struct SpawnOpts {
    /// Which CLI to spawn.
    pub kind: AgentKind,
    /// Project working directory.
    pub cwd: PathBuf,
    /// Active tentacle id (default `"main"`).
    pub tentacle_id: String,
    /// Optional initial cols/rows for the PTY.
    pub size: Option<(u16, u16)>,
    /// Extra args to pass to the CLI after the assembled prompt.
    pub extra_args: Vec<String>,
}

impl Default for SpawnOpts {
    fn default() -> Self {
        Self {
            kind: AgentKind::Auto,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            tentacle_id: "main".into(),
            size: None,
            extra_args: Vec::new(),
        }
    }
}
