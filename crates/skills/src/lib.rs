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

/// Git plumbing helpers used by skills (current branch, base, diff, …).
pub mod git;
/// LLM client abstractions and request/response types for skill prompts.
pub mod llm;
/// Skill registry — name → boxed `Skill` instance.
pub mod registry;
/// Concrete skills (review, investigate, cso, ship, pentest, …).
pub mod skills;
/// Shared types (`Skill` trait, `SkillContext`, `SkillResult`, `Severity`).
pub mod types;

pub use llm::{has_any_llm_key, LLMClient, LLMRequest, LLMResponse, LLMUnavailableError};
pub use registry::{create_default_registry, Registry};
pub use types::{merge_usage, Provider, Severity, Skill, SkillContext, SkillFinding, SkillResult};
