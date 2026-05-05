/*
   File: crates/skills/src/skills/investigate.rs

   Purpose
   Four-phase root-cause debugging with model-tier escalation.
   Phase 1: collect symptoms. Phase 2: hypothesize. Phase 3: verify
   on Balanced. Phase 4: escalate to Powerful if still ambiguous.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use async_trait::async_trait;

use crate::types::{Skill, SkillContext, SkillResult};

/// Implementation of `monkey investigate`.
#[derive(Debug, Clone, Copy)]
pub struct Investigate;

#[async_trait]
impl Skill for Investigate {
    fn name(&self) -> &str { "investigate" }
    fn description(&self) -> &str { "Four-phase root-cause debugging with model-tier escalation" }

    async fn run(
        &self,
        _input: serde_json::Value,
        _ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        Ok(SkillResult {
            ok: true,
            summary: "investigate (stub)".into(),
            markdown: Some("# investigate\n\n_stub — not yet ported_".into()),
        })
    }
}
