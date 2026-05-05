/*
   File: crates/skills/src/skills/review.rs

   Purpose
   Multi-model pre-merge review of the current branch vs base. Runs
   the diff through the configured provider, parses findings by
   severity, and returns markdown.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; trait wiring
*/

use async_trait::async_trait;

use crate::types::{Skill, SkillContext, SkillResult};

/// Implementation of `monkey review`.
#[derive(Debug, Clone, Copy)]
pub struct Review;

#[async_trait]
impl Skill for Review {
    fn name(&self) -> &str { "review" }
    fn description(&self) -> &str { "Multi-model pre-merge review of the current branch vs base" }

    async fn run(
        &self,
        _input: serde_json::Value,
        _ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        // TODO(0.1.x): port from packages/skills/src/skills/review.ts
        Ok(SkillResult {
            ok: true,
            summary: "review (stub)".into(),
            markdown: Some("# review\n\n_stub — not yet ported_".into()),
        })
    }
}
