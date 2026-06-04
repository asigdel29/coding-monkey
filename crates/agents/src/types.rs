/*
   File: crates/agents/src/types.rs

   Purpose
   Shared types for the agents crate. Kept separate so doctor/spawn can
   evolve without touching the public surface in lib.rs.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/agents/src/types.ts
   2026-06-03   Anubhav Sigdel  drop legacy agent kind; codex-only roster
*/

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which agent CLI to spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Pick whichever supported CLI is installed (currently `codex`).
    Auto,
    /// Force the codex CLI.
    Codex,
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
