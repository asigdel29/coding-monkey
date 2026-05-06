/*
   File: crates/skills/src/registry.rs

   Purpose
   Skill registry — lookup by name, list-all, dispatch to run(). Skills
   register themselves in `create_default_registry()` so adding a new
   skill is one line.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use std::collections::HashMap;
use std::sync::Arc;

use crate::skills::{Cso, Investigate, Review, Ship};
use crate::types::{Skill, SkillContext, SkillResult};

/// Registry of skills keyed by name.
#[derive(Clone)]
pub struct Registry {
    by_name: HashMap<String, Arc<dyn Skill>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("skills", &self.by_name.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Registry {
    /// Empty registry.
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    /// Register a skill, replacing any existing one with the same name.
    pub fn register(&mut self, skill: Arc<dyn Skill>) {
        self.by_name.insert(skill.name().to_string(), skill);
    }

    /// All skills, deterministic order (sorted by name).
    pub fn list(&self) -> Vec<Arc<dyn Skill>> {
        let mut v: Vec<_> = self.by_name.values().cloned().collect();
        v.sort_by(|a, b| a.name().cmp(b.name()));
        v
    }

    /// Run a skill by name with structured input.
    pub async fn run(
        &self,
        name: &str,
        input: serde_json::Value,
        ctx: &SkillContext,
    ) -> anyhow::Result<SkillResult> {
        let skill = self
            .by_name
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown skill: {name}"))?
            .clone();
        skill.run(input, ctx).await
    }
}

impl Default for Registry {
    fn default() -> Self {
        create_default_registry()
    }
}

/// Production registry with all built-in skills.
pub fn create_default_registry() -> Registry {
    let mut r = Registry::new();
    r.register(Arc::new(Review));
    r.register(Arc::new(Investigate));
    r.register(Arc::new(Cso));
    r.register(Arc::new(Ship));
    r
}
