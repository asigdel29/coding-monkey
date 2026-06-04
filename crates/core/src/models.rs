/*
   File: crates/core/src/models.rs

   Purpose
   Model registry and tier-based selector. The selector picks the
   cheapest model that satisfies a task's complexity class — this is
   the core "right model for the job" routing logic.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/core/src/models/
   2026-06-03   Anubhav Sigdel  de-brand catalogue → OpenRouter/OpenAI defaults
*/

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::TaskType;

/// Model performance tier. Selectors pick the cheapest tier that
/// satisfies a task's class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    /// Cheapest, fastest. For trivial work (chat, classification).
    Fast,
    /// Default for everyday code work.
    Balanced,
    /// Most capable. For multi-step reasoning, hard debugging.
    Powerful,
}

/// Model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// OpenRouter — a single API key that proxies to many upstream
    /// models. OpenAI-compatible wire format. The default for
    /// clone-and-run setups.
    OpenRouter,
    /// OpenAI (GPT family).
    Openai,
}

/// Single registered model with cost + tier metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Provider's model id (used directly in API calls).
    pub id: String,
    /// Human-friendly display name.
    pub display_name: String,
    /// Provider hosting the model.
    pub provider: Provider,
    /// Performance tier.
    pub tier: ModelTier,
    /// USD per 1k input tokens.
    pub input_cost_per_1k: f64,
    /// USD per 1k output tokens.
    pub output_cost_per_1k: f64,
    /// Approximate context window in tokens.
    pub context_window: u32,
}

/// In-memory catalogue of available models.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    by_id: HashMap<String, ModelSpec>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::with_builtin()
    }
}

impl ModelRegistry {
    /// Empty registry. Use [`with_builtin`](Self::with_builtin) for the
    /// production set.
    pub fn empty() -> Self {
        Self {
            by_id: HashMap::new(),
        }
    }

    /// Registry pre-populated with the production model lineup. Update
    /// when providers ship new tiers — the rest of the workspace reads
    /// only `id`, `tier`, and cost fields, so adding rows is safe.
    pub fn with_builtin() -> Self {
        let mut r = Self::empty();
        for m in builtin_models() {
            r.register(m);
        }
        r
    }

    /// Add or replace a model by id.
    pub fn register(&mut self, m: ModelSpec) {
        self.by_id.insert(m.id.clone(), m);
    }

    /// Fetch a registered model by id.
    pub fn get(&self, id: &str) -> Option<&ModelSpec> {
        self.by_id.get(id)
    }

    /// All registered models, deterministic order (sorted by id).
    pub fn list_all(&self) -> Vec<ModelSpec> {
        let mut v: Vec<_> = self.by_id.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// All models in a given tier.
    pub fn list_tier(&self, tier: ModelTier) -> Vec<ModelSpec> {
        let mut v: Vec<_> = self
            .by_id
            .values()
            .filter(|m| m.tier == tier)
            .cloned()
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

/// Routes tasks to the cheapest model in the appropriate tier.
#[derive(Debug, Clone)]
pub struct ModelSelector<'a> {
    registry: &'a ModelRegistry,
    /// Provider preference when multiple providers offer the chosen tier.
    pub preferred_provider: Option<Provider>,
}

impl<'a> ModelSelector<'a> {
    /// New selector backed by `registry`. No provider preference by default.
    pub fn new(registry: &'a ModelRegistry) -> Self {
        Self {
            registry,
            preferred_provider: None,
        }
    }

    /// Pin the selector to a single provider. Useful when only one API
    /// key is configured.
    pub fn prefer(mut self, provider: Provider) -> Self {
        self.preferred_provider = Some(provider);
        self
    }

    /// Return the cheapest model in the appropriate tier for `task_type`.
    /// Falls back across tiers if the registry is sparse.
    pub fn select(&self, task_type: TaskType) -> Option<&ModelSpec> {
        let tier = tier_for_task(task_type);
        self.cheapest_in_tier(tier)
            .or_else(|| self.cheapest_in_tier(ModelTier::Balanced))
            .or_else(|| self.cheapest_in_tier(ModelTier::Powerful))
            .or_else(|| self.cheapest_in_tier(ModelTier::Fast))
    }

    fn cheapest_in_tier(&self, tier: ModelTier) -> Option<&ModelSpec> {
        let mut candidates: Vec<&ModelSpec> = self
            .registry
            .by_id
            .values()
            .filter(|m| m.tier == tier)
            .filter(|m| {
                self.preferred_provider
                    .map(|p| m.provider == p)
                    .unwrap_or(true)
            })
            .collect();
        candidates.sort_by(|a, b| {
            (a.input_cost_per_1k + a.output_cost_per_1k)
                .partial_cmp(&(b.input_cost_per_1k + b.output_cost_per_1k))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.first().copied()
    }
}

/// Default tier for a task class. Tunable as we learn from production
/// usage; keep it monotone (fast tasks should never escalate to powerful).
pub fn tier_for_task(t: TaskType) -> ModelTier {
    match t {
        TaskType::Chat | TaskType::Explain => ModelTier::Fast,
        TaskType::Edit | TaskType::Generate | TaskType::Engulf => ModelTier::Balanced,
        TaskType::Refactor | TaskType::Investigate | TaskType::Review | TaskType::SecurityAudit => {
            ModelTier::Powerful
        }
    }
}

/// Production model lineup. Update on provider releases.
fn builtin_models() -> Vec<ModelSpec> {
    use ModelTier::*;
    use Provider::*;
    vec![
        ModelSpec {
            id: "google/gemini-2.0-flash-001".into(),
            display_name: "Gemini 2.0 Flash (OpenRouter)".into(),
            provider: OpenRouter,
            tier: Fast,
            input_cost_per_1k: 0.0001,
            output_cost_per_1k: 0.0004,
            context_window: 1_000_000,
        },
        ModelSpec {
            id: "openai/gpt-4o".into(),
            display_name: "GPT-4o (OpenRouter)".into(),
            provider: OpenRouter,
            tier: Balanced,
            input_cost_per_1k: 0.0025,
            output_cost_per_1k: 0.01,
            context_window: 128_000,
        },
        ModelSpec {
            id: "meta-llama/llama-3.1-405b-instruct".into(),
            display_name: "Llama 3.1 405B (OpenRouter)".into(),
            provider: OpenRouter,
            tier: Powerful,
            input_cost_per_1k: 0.003,
            output_cost_per_1k: 0.003,
            context_window: 128_000,
        },
        ModelSpec {
            id: "gpt-5-mini".into(),
            display_name: "GPT-5 mini".into(),
            provider: Openai,
            tier: Fast,
            input_cost_per_1k: 0.0008,
            output_cost_per_1k: 0.004,
            context_window: 256_000,
        },
        ModelSpec {
            id: "gpt-5".into(),
            display_name: "GPT-5".into(),
            provider: Openai,
            tier: Balanced,
            input_cost_per_1k: 0.0025,
            output_cost_per_1k: 0.012,
            context_window: 256_000,
        },
        ModelSpec {
            id: "gpt-5-pro".into(),
            display_name: "GPT-5 Pro".into(),
            provider: Openai,
            tier: Powerful,
            input_cost_per_1k: 0.012,
            output_cost_per_1k: 0.06,
            context_window: 512_000,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_three_tiers() {
        let r = ModelRegistry::with_builtin();
        assert!(!r.list_tier(ModelTier::Fast).is_empty());
        assert!(!r.list_tier(ModelTier::Balanced).is_empty());
        assert!(!r.list_tier(ModelTier::Powerful).is_empty());
    }

    #[test]
    fn selector_picks_cheapest_in_tier() {
        let r = ModelRegistry::with_builtin();
        let s = ModelSelector::new(&r);
        let chosen = s.select(TaskType::Chat).unwrap();
        assert_eq!(chosen.tier, ModelTier::Fast);
        // Among Fast-tier models, total cost should be minimum.
        let cheapest = r
            .list_tier(ModelTier::Fast)
            .into_iter()
            .map(|m| m.input_cost_per_1k + m.output_cost_per_1k)
            .fold(f64::INFINITY, f64::min);
        let chosen_cost = chosen.input_cost_per_1k + chosen.output_cost_per_1k;
        assert!((chosen_cost - cheapest).abs() < f64::EPSILON);
    }

    #[test]
    fn selector_respects_provider_preference() {
        let r = ModelRegistry::with_builtin();
        let s = ModelSelector::new(&r).prefer(Provider::Openai);
        let chosen = s.select(TaskType::Refactor).unwrap();
        assert_eq!(chosen.provider, Provider::Openai);
        assert_eq!(chosen.tier, ModelTier::Powerful);
    }

    #[test]
    fn tier_routing_is_monotone() {
        // Fast tasks must not route to Powerful.
        assert_eq!(tier_for_task(TaskType::Chat), ModelTier::Fast);
        assert_eq!(tier_for_task(TaskType::Review), ModelTier::Powerful);
    }
}
