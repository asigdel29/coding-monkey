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
   2026-06-19   Anubhav Sigdel  add run_agent_escalating retry driver
*/

use std::sync::Arc;

use monkey_core::{
    EscalationTrigger, ModelRegistry, ModelSpec, ModelTier, OrchestratorPolicy, RepoComplexity,
    TaskType,
};
use tokio::sync::mpsc::{self, Sender};
use tokio_util::sync::CancellationToken;

use crate::agent::{run_agent, ChatBackend};
use crate::event::AgentEvent;
use crate::state::{AgentConfig, AgentOutcome};
use crate::tool::ToolRegistry;

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

/// Run a task with difficulty-based selection and escalation.
///
/// Picks the initial model from the task's difficulty (an explicit
/// `cfg.force_tier` overrides the score), runs the agent, and — when the run
/// fails or hits a guard and `policy` permits — retries on the next-stronger
/// model, up to `policy.max_escalations`. Each attempt's progress is forwarded
/// on `events`, but a per-attempt terminal event is intercepted so consumers
/// see exactly one terminal for the whole task; an [`AgentEvent::Escalated`]
/// marks each handoff. Returns the final attempt's outcome.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_escalating(
    agent_id: String,
    cfg: AgentConfig,
    registry: &ModelRegistry,
    policy: &OrchestratorPolicy,
    default_model: Option<&str>,
    repo_complexity: Option<RepoComplexity>,
    tools: Arc<ToolRegistry>,
    backend: Arc<dyn ChatBackend>,
    events: Sender<AgentEvent>,
    cancel: CancellationToken,
) -> AgentOutcome {
    let difficulty = match cfg.force_tier {
        Some(ModelTier::Fast) => Difficulty::Low,
        Some(ModelTier::Balanced) => Difficulty::Medium,
        Some(ModelTier::Powerful) => Difficulty::High,
        None => score_difficulty(cfg.task_type, &cfg.task, repo_complexity),
    };
    let mut model = choose_model(registry, difficulty, default_model);
    let mut escalations = 0u32;

    loop {
        // Run one attempt into a private channel so its terminal event can be
        // intercepted; the agent loop borrows nothing, so it spawns cleanly
        // while this task drains its events.
        let (inner_tx, mut inner_rx) = mpsc::channel::<AgentEvent>(256);
        let attempt = {
            let (agent_id, cfg, tools, backend, model, cancel) = (
                agent_id.clone(),
                cfg.clone(),
                Arc::clone(&tools),
                Arc::clone(&backend),
                model.clone(),
                cancel.clone(),
            );
            tokio::spawn(async move {
                run_agent(agent_id, cfg, tools, backend, model, inner_tx, cancel).await
            })
        };
        // Forward live, non-terminal progress; the inner terminal is swallowed
        // and re-emitted once below, after the escalation decision.
        while let Some(ev) = inner_rx.recv().await {
            if !ev.is_terminal() {
                let _ = events.try_send(ev);
            }
        }
        let outcome = attempt.await.unwrap_or(AgentOutcome::Failed {
            error: "agent task panicked".into(),
        });

        let trigger = match &outcome {
            AgentOutcome::Failed { .. } => Some(EscalationTrigger::Failed),
            AgentOutcome::LimitReached { .. } => Some(EscalationTrigger::LimitReached),
            _ => None,
        };
        let wants_escalation =
            trigger.is_some_and(|t| policy.escalates_on(t)) && escalations < policy.max_escalations;
        if wants_escalation {
            if let Some(next) = next_stronger(registry, &model) {
                let _ = events.try_send(AgentEvent::Escalated {
                    from: model.id.clone(),
                    to: next.id.clone(),
                    reason: outcome_reason(&outcome),
                });
                model = next;
                escalations += 1;
                continue;
            }
        }
        emit_terminal(&events, &outcome).await;
        return outcome;
    }
}

/// One-line description of a non-terminal-success outcome, for the escalation
/// event's `reason`.
fn outcome_reason(outcome: &AgentOutcome) -> String {
    match outcome {
        AgentOutcome::Failed { error } => format!("failed: {error}"),
        AgentOutcome::LimitReached { reason } => format!("limit reached: {reason}"),
        _ => String::new(),
    }
}

/// Re-emit the single terminal event for the whole task, with guaranteed
/// delivery (terminal events must not be dropped under backpressure).
async fn emit_terminal(events: &Sender<AgentEvent>, outcome: &AgentOutcome) {
    let ev = match outcome {
        AgentOutcome::Finished { summary } => AgentEvent::Finished {
            summary: summary.clone(),
        },
        AgentOutcome::LimitReached { reason } => AgentEvent::LimitReached {
            reason: reason.clone(),
        },
        AgentOutcome::Failed { error } => AgentEvent::Failed {
            error: error.clone(),
        },
        AgentOutcome::Cancelled => AgentEvent::Cancelled,
    };
    let _ = events.send(ev).await;
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

    // --- escalation driver ---

    use crate::agent::ChatBackend;
    use crate::event::AgentEvent;
    use crate::llm::{ChatResult, LlmError};
    use crate::state::{AgentConfig, AgentOutcome, Message};
    use crate::tools::default_tools;
    use async_trait::async_trait;
    use monkey_core::TokenUsage;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    /// Fails on the Pi-local model, finishes on anything stronger — so a task
    /// must escalate once to succeed.
    struct FlakyFast;

    #[async_trait]
    impl ChatBackend for FlakyFast {
        async fn chat(
            &self,
            model: &ModelSpec,
            _messages: &[Message],
            _tools: &[serde_json::Value],
            _max_tokens: u32,
            _cancel: &CancellationToken,
            _on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
        ) -> Result<ChatResult, LlmError> {
            if model.id == "qwen2.5-coder-3b" {
                return Err(LlmError::Transport("pi model offline".into()));
            }
            Ok(ChatResult {
                assistant_text: "done on the strong model".into(),
                tool_calls: vec![],
                finish_reason: "stop".into(),
                usage: TokenUsage::empty(),
            })
        }
    }

    #[tokio::test]
    async fn escalates_from_failed_pi_model_then_finishes() {
        let r = registry_with_locals();
        let policy = OrchestratorPolicy::default(); // escalate on failure, cap 1
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AgentConfig::new("do the thing", dir.path().to_path_buf());
        cfg.force_tier = Some(ModelTier::Fast); // start at the Pi-local model

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let outcome = run_agent_escalating(
            "a1".into(),
            cfg,
            &r,
            &policy,
            None,
            None,
            Arc::new(default_tools()),
            Arc::new(FlakyFast),
            tx,
            CancellationToken::new(),
        )
        .await;

        assert_eq!(
            outcome,
            AgentOutcome::Finished {
                summary: "done on the strong model".into()
            }
        );
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        // Exactly one escalation, Pi → GLM, and exactly one terminal (Finished).
        let escalations: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Escalated { .. }))
            .collect();
        assert_eq!(escalations.len(), 1);
        assert!(matches!(
            escalations[0],
            AgentEvent::Escalated { from, to, .. } if from == "qwen2.5-coder-3b" && to == "glm-5.2"
        ));
        let terminals = events.iter().filter(|e| e.is_terminal()).count();
        assert_eq!(terminals, 1);
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::Finished { .. })));
    }

    #[tokio::test]
    async fn tier_only_policy_does_not_escalate() {
        let r = registry_with_locals();
        let policy = OrchestratorPolicy {
            strategy: monkey_core::OrchestratorStrategy::TierOnly,
            ..OrchestratorPolicy::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AgentConfig::new("do the thing", dir.path().to_path_buf());
        cfg.force_tier = Some(ModelTier::Fast);

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let outcome = run_agent_escalating(
            "a2".into(),
            cfg,
            &r,
            &policy,
            None,
            None,
            Arc::new(default_tools()),
            Arc::new(FlakyFast),
            tx,
            CancellationToken::new(),
        )
        .await;

        assert!(matches!(outcome, AgentOutcome::Failed { .. }));
        let mut saw_escalation = false;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, AgentEvent::Escalated { .. }) {
                saw_escalation = true;
            }
        }
        assert!(!saw_escalation);
    }
}
