/*
   File: crates/runtime/src/orchestrator.rs

   Purpose
   Decide which model handles a task. Rather than the static `task_type → tier`
   map alone, the orchestrator scores a task's difficulty from cheap, already-
   available signals (task type, prompt size, repo complexity) and picks the
   initial model from that — routing trivial work to a Pi-local model, everyday
   coding to the configured default (GLM-5.2), and the hardest tasks to the
   strongest tier (Kimi K2.6). It also exposes the laddering helper the
   escalation wrapper uses to retry a failed task on a stronger model.

   Selection is local-first by construction: locally-served models carry zero
   cost, so "cheapest in tier" naturally prefers them over hosted builtins.

   All functions here are pure (registry lookups only, no I/O) so the routing
   policy is unit-testable without a network.

   History
   Date         Author          Changes
   2026-06-19   Anubhav Sigdel  initial — difficulty scoring + model choice
*/

use monkey_core::{ModelRegistry, ModelSpec, ModelTier, RepoComplexity, TaskType};

/// Prompt length (chars) beyond which a task is treated as heavier work.
const LONG_PROMPT_CHARS: usize = 2_000;

/// Coarse difficulty class for a task; maps one-to-one onto a [`ModelTier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// Trivial work — chat, classification, tiny edits. Pi-local Fast tier.
    Low,
    /// Everyday coding — the default workhorse (GLM-5.2). Balanced tier.
    Medium,
    /// Hard, multi-step reasoning or debugging. Powerful tier (Kimi K2.6).
    High,
}

impl Difficulty {
    /// The model tier this difficulty selects.
    pub fn tier(self) -> ModelTier {
        match self {
            Difficulty::Low => ModelTier::Fast,
            Difficulty::Medium => ModelTier::Balanced,
            Difficulty::High => ModelTier::Powerful,
        }
    }
}

/// Score a task's difficulty from its type, prompt, and repo complexity.
///
/// Starts from the task type's tier (so the existing `task_type → tier`
/// intuition is the floor) and only ever raises difficulty: a long prompt or a
/// large repo bumps it up one step. It never lowers below the task floor, which
/// keeps routing monotone — a review never silently drops to the Pi-local model.
pub fn score_difficulty(
    task_type: TaskType,
    prompt: &str,
    repo_complexity: Option<RepoComplexity>,
) -> Difficulty {
    let mut score = match monkey_core::tier_for_task(task_type) {
        ModelTier::Fast => 0i32,
        ModelTier::Balanced => 1,
        ModelTier::Powerful => 2,
    };
    if prompt.chars().count() > LONG_PROMPT_CHARS {
        score += 1;
    }
    if repo_complexity == Some(RepoComplexity::Large) {
        score += 1;
    }
    match score.clamp(0, 2) {
        0 => Difficulty::Low,
        1 => Difficulty::Medium,
        _ => Difficulty::High,
    }
}

/// Pick the initial model for `difficulty`.
///
/// At the everyday (Medium) tier an explicit `default_model` wins when it is
/// registered, so coding work routes to GLM-5.2 by configuration. Otherwise the
/// cheapest model in the target tier is chosen (local-first via zero cost),
/// falling back across tiers and finally to any registered model so the result
/// is always defined for a non-empty registry.
pub fn choose_model(
    registry: &ModelRegistry,
    difficulty: Difficulty,
    default_model: Option<&str>,
) -> ModelSpec {
    if difficulty == Difficulty::Medium {
        if let Some(spec) = default_model.and_then(|id| registry.get(id)) {
            return spec.clone();
        }
    }
    let tier = difficulty.tier();
    cheapest_in_tier(registry, tier)
        .or_else(|| cheapest_in_tier(registry, ModelTier::Balanced))
        .or_else(|| cheapest_in_tier(registry, ModelTier::Powerful))
        .or_else(|| cheapest_in_tier(registry, ModelTier::Fast))
        .or_else(|| registry.list_all().into_iter().next())
        .expect("registry non-empty")
}

/// The next-stronger model for escalation: the cheapest model in the tier above
/// `current`'s. Returns `None` when `current` is already in the top tier, so the
/// escalation wrapper knows the ladder is exhausted.
pub fn next_stronger(registry: &ModelRegistry, current: &ModelSpec) -> Option<ModelSpec> {
    let next = match current.tier {
        ModelTier::Fast => ModelTier::Balanced,
        ModelTier::Balanced => ModelTier::Powerful,
        ModelTier::Powerful => return None,
    };
    cheapest_in_tier(registry, next)
}

/// Cheapest registered model in `tier` (ties broken by id, via `list_tier`'s
/// deterministic order). Local models cost zero, so they win their tier.
fn cheapest_in_tier(registry: &ModelRegistry, tier: ModelTier) -> Option<ModelSpec> {
    registry.list_tier(tier).into_iter().min_by(|a, b| {
        let ca = a.input_cost_per_1k + a.output_cost_per_1k;
        let cb = b.input_cost_per_1k + b.output_cost_per_1k;
        ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use monkey_core::{LocalHost, LocalModelDef, OrchestratorConfig};

    fn registry_with_locals() -> ModelRegistry {
        let cfg = OrchestratorConfig {
            local_models: vec![
                LocalModelDef {
                    id: "qwen2.5-coder-3b".into(),
                    display_name: "Qwen (Pi)".into(),
                    tier: ModelTier::Fast,
                    base_url: "http://localhost:11434".into(),
                    api_key_env: None,
                    context_window: 32_768,
                    host: LocalHost::Pi,
                },
                LocalModelDef {
                    id: "glm-5.2".into(),
                    display_name: "GLM-5.2".into(),
                    tier: ModelTier::Balanced,
                    base_url: "http://lan:8000".into(),
                    api_key_env: None,
                    context_window: 200_000,
                    host: LocalHost::Lan,
                },
                LocalModelDef {
                    id: "kimi-k2.6".into(),
                    display_name: "Kimi K2.6".into(),
                    tier: ModelTier::Powerful,
                    base_url: "http://lan:8001".into(),
                    api_key_env: None,
                    context_window: 256_000,
                    host: LocalHost::Lan,
                },
            ],
            ..OrchestratorConfig::default()
        };
        ModelRegistry::with_config(&cfg)
    }

    #[test]
    fn difficulty_starts_at_task_floor() {
        assert_eq!(
            score_difficulty(TaskType::Chat, "hi", None),
            Difficulty::Low
        );
        assert_eq!(
            score_difficulty(TaskType::Edit, "tweak", None),
            Difficulty::Medium
        );
        assert_eq!(
            score_difficulty(TaskType::Review, "diff", None),
            Difficulty::High
        );
    }

    #[test]
    fn long_prompt_and_large_repo_raise_difficulty() {
        let long = "x".repeat(LONG_PROMPT_CHARS + 1);
        assert_eq!(
            score_difficulty(TaskType::Chat, &long, None),
            Difficulty::Medium
        );
        assert_eq!(
            score_difficulty(TaskType::Edit, "tweak", Some(RepoComplexity::Large)),
            Difficulty::High
        );
        // Bumps never exceed the top tier.
        assert_eq!(
            score_difficulty(TaskType::Review, &long, Some(RepoComplexity::Large)),
            Difficulty::High
        );
    }

    #[test]
    fn medium_honors_default_model() {
        let r = registry_with_locals();
        let m = choose_model(&r, Difficulty::Medium, Some("glm-5.2"));
        assert_eq!(m.id, "glm-5.2");
    }

    #[test]
    fn tiers_prefer_local_zero_cost_models() {
        let r = registry_with_locals();
        // No default model: Low → Pi-local Fast, High → Kimi, by zero cost.
        assert_eq!(
            choose_model(&r, Difficulty::Low, None).id,
            "qwen2.5-coder-3b"
        );
        assert_eq!(choose_model(&r, Difficulty::High, None).id, "kimi-k2.6");
    }

    #[test]
    fn escalation_ladder_climbs_then_stops() {
        let r = registry_with_locals();
        let fast = r.get("qwen2.5-coder-3b").unwrap().clone();
        let balanced = next_stronger(&r, &fast).unwrap();
        assert_eq!(balanced.id, "glm-5.2");
        let powerful = next_stronger(&r, &balanced).unwrap();
        assert_eq!(powerful.id, "kimi-k2.6");
        assert!(next_stronger(&r, &powerful).is_none());
    }
}
