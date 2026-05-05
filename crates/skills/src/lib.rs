/*
   File: crates/skills/src/lib.rs

   Purpose
   Production gstack-equivalent skills:
       review       — multi-model pre-merge diff review
       investigate  — four-phase root-cause debugging w/ tier escalation
       cso          — security audit composer (engulf scan + dep advisories)
       ship         — typecheck → review → cso → pentest → bump → push

   Each skill implements the `Skill` trait and reports `SkillResult`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; Skill trait + registry
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-skills` — composable pre-merge / pre-push gates.

pub mod registry;
pub mod skills;
pub mod types;

pub use registry::{create_default_registry, Registry};
pub use types::{Skill, SkillContext, SkillResult, Severity};
