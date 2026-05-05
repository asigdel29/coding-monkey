/*
   File: crates/skills/src/skills/ship.rs

   Purpose
   Full ship gauntlet: typecheck → review → cso → MANDATORY pentest →
   bump → push → optional GitHub PR. Each stage runs only if the
   previous one passes the configured fail-on threshold.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use async_trait::async_trait;

use crate::types::{Skill, SkillContext, SkillResult};

/// Implementation of `monkey ship`.
#[derive(Debug, Clone, Copy)]
pub struct Ship;

#[async_trait]
impl Skill for Ship {
    fn name(&self) -> &str { "ship" }
    fn description(&self) -> &str { "Full ship gauntlet: typecheck → review → cso → pentest → bump → push" }

    async fn run(
        &self,
        _input: serde_json::Value,
        _ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        Ok(SkillResult {
            ok: true,
            summary: "ship (stub)".into(),
            markdown: Some("# ship\n\n_stub — not yet ported_".into()),
        })
    }
}
