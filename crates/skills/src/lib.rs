/*
   File: crates/skills/src/lib.rs

   Purpose
   Composable gstack-equivalent skills:
       review       — multi-model pre-merge diff review
       investigate  — four-phase root-cause debugging w/ tier escalation
       cso          — security audit composer
       ship         — typecheck → review → cso → pentest → bump → push

   Each skill implements `Skill` (types.rs) and reports `SkillResult`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; Skill trait + registry
   2026-05-05   Anubhav Sigdel  full ports of review, investigate, cso, ship
                                 + git/llm helpers
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-skills` — composable pre-merge / pre-push gates.

pub mod git;
pub mod llm;
pub mod registry;
pub mod skills;
pub mod types;

pub use llm::{has_any_llm_key, LLMClient, LLMRequest, LLMResponse, LLMUnavailableError};
pub use registry::{create_default_registry, Registry};
pub use types::{merge_usage, Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult};
