/*
   File: crates/skills/src/llm.rs

   Purpose
   Skill-facing LLM client. Wraps the lower-level `monkey_engulf::llm`
   primitive with two pieces of skill-specific glue:

     1. Tier-based model selection. Skills request a TaskType and
        optional forceTier; the registry+selector picks the cheapest
        model that satisfies it.
     2. Token usage accounting. Anthropic and OpenAI both report
        prompt + completion tokens in different fields — we normalize
        to monkey_core::TokenUsage with USD cost computed from the
        registry rate at call time.

   The simpler `complete()` in monkey-engulf::llm is fine for
   one-shot prompts. Skills want richer telemetry, so this module
   re-implements the HTTP path with usage extraction.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/skills/src/llm.ts
*/

use anyhow::{anyhow, Context};
use serde::Deserialize;
use std::time::Duration;
use thiserror::Error;

use monkey_core::{
    tier_for_task, ModelRegistry, ModelSelector, ModelSpec, ModelTier, Provider as CoreProvider,
    TaskType, TokenUsage,
};

use crate::types::Provider;

/// Raised when no API key is available for the requested provider.
/// Skills handle this specifically to fall back to offline output.
#[derive(Debug, Error)]
#[error("LLM unavailable: {0}")]
pub struct LLMUnavailableError(pub String);

/// Request for a single completion.
#[derive(Debug, Clone)]
pub struct LLMRequest {
    /// Task class (drives default tier selection).
    pub task_type: TaskType,
    /// Force a specific tier regardless of `task_type`.
    pub force_tier: Option<ModelTier>,
    /// Provider override; defaults to the client's default.
    pub provider: Option<Provider>,
    /// System prompt.
    pub system: String,
    /// User prompt.
    pub user: String,
    /// Cap on output tokens.
    pub max_tokens: Option<u32>,
}

/// Response from [`LLMClient::complete`].
#[derive(Debug, Clone)]
pub struct LLMResponse {
    /// Text response.
    pub text: String,
    /// Model that produced it.
    pub model: ModelSpec,
    /// Token + cost accounting for this call.
    pub usage: TokenUsage,
}

/// Tier-aware client. Cheap to clone; holds a `ModelRegistry`.
#[derive(Debug, Clone)]
pub struct LLMClient {
    registry: ModelRegistry,
    default_provider: Provider,
}

impl LLMClient {
    /// Build a client preferring `default_provider`.
    pub fn new(default_provider: Provider) -> Self {
        Self {
            registry: ModelRegistry::with_builtin(),
            default_provider,
        }
    }

    /// Pick a model spec without calling out — useful for telemetry.
    pub fn pick(&self, task_type: TaskType, force_tier: Option<ModelTier>, provider: Option<Provider>) -> ModelSpec {
        let p = provider.unwrap_or(self.default_provider);
        let core_provider = match p {
            Provider::Anthropic => CoreProvider::Anthropic,
            Provider::Openai => CoreProvider::Openai,
        };
        let selector = ModelSelector::new(&self.registry).prefer(core_provider);
        let tier = force_tier.unwrap_or_else(|| tier_for_task(task_type));
        // Try forced tier with provider preference; fall back to selector
        // default if the preferred provider has no model in that tier.
        let candidates = self.registry.list_tier(tier);
        let preferred = candidates
            .iter()
            .find(|m| m.provider == core_provider)
            .cloned();
        preferred
            .or_else(|| selector.select(task_type).cloned())
            .unwrap_or_else(|| candidates.first().cloned().unwrap_or_else(|| {
                self.registry
                    .list_all()
                    .into_iter()
                    .next()
                    .expect("registry non-empty")
            }))
    }

    /// Send the request and return the model's text plus usage.
    pub async fn complete(&self, req: LLMRequest) -> anyhow::Result<LLMResponse> {
        let provider = req.provider.unwrap_or(self.default_provider);
        let model = self.pick(req.task_type, req.force_tier, Some(provider));
        let max_tokens = req.max_tokens.unwrap_or(2048);

        match provider {
            Provider::Anthropic => self.complete_anthropic(model, &req, max_tokens).await,
            Provider::Openai => self.complete_openai(model, &req, max_tokens).await,
        }
    }

    async fn complete_anthropic(
        &self,
        model: ModelSpec,
        req: &LLMRequest,
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| LLMUnavailableError("ANTHROPIC_API_KEY not set".into()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("build http client")?;
        let body = serde_json::json!({
            "model": model.id,
            "max_tokens": max_tokens,
            "system": req.system,
            "messages": [{ "role": "user", "content": req.user }],
        });
        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("anthropic request failed")?;
        let status = resp.status();
        let raw = resp.text().await.context("anthropic body")?;
        if !status.is_success() {
            return Err(anyhow!("anthropic {}: {}", status, raw));
        }
        let parsed: AnthropicResp = serde_json::from_str(&raw)
            .with_context(|| format!("anthropic decode: {raw}"))?;
        let text = parsed
            .content
            .into_iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");
        let usage = TokenUsage {
            input_tokens: parsed.usage.input_tokens,
            output_tokens: parsed.usage.output_tokens,
            total_tokens: parsed.usage.input_tokens + parsed.usage.output_tokens,
            estimated_cost_usd: cost(&model, parsed.usage.input_tokens, parsed.usage.output_tokens),
        };
        Ok(LLMResponse { text, model, usage })
    }

    async fn complete_openai(
        &self,
        model: ModelSpec,
        req: &LLMRequest,
        max_tokens: u32,
    ) -> anyhow::Result<LLMResponse> {
        let key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| LLMUnavailableError("OPENAI_API_KEY not set".into()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("build http client")?;
        let body = serde_json::json!({
            "model": model.id,
            "max_tokens": max_tokens,
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user },
            ],
        });
        let resp = client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("openai request failed")?;
        let status = resp.status();
        let raw = resp.text().await.context("openai body")?;
        if !status.is_success() {
            return Err(anyhow!("openai {}: {}", status, raw));
        }
        let parsed: OpenAIResp = serde_json::from_str(&raw)
            .with_context(|| format!("openai decode: {raw}"))?;
        let text = parsed
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        let inp = parsed.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0);
        let outp = parsed.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0);
        let usage = TokenUsage {
            input_tokens: inp,
            output_tokens: outp,
            total_tokens: inp + outp,
            estimated_cost_usd: cost(&model, inp, outp),
        };
        Ok(LLMResponse { text, model, usage })
    }
}

fn cost(model: &ModelSpec, input: u64, output: u64) -> f64 {
    let i = input as f64 / 1000.0 * model.input_cost_per_1k;
    let o = output as f64 / 1000.0 * model.output_cost_per_1k;
    i + o
}

/// True if at least one provider env var is set.
pub fn has_any_llm_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY").is_ok() || std::env::var("OPENAI_API_KEY").is_ok()
}

#[derive(Deserialize)]
struct AnthropicResp {
    content: Vec<AnthropicBlock>,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Deserialize)]
struct OpenAIResp {
    choices: Vec<OpenAIChoice>,
    #[serde(default)]
    usage: Option<OpenAIUsage>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIMessage {
    content: String,
}

#[derive(Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_chooses_provider_preference_first() {
        let c = LLMClient::new(Provider::Anthropic);
        let m = c.pick(TaskType::Chat, None, Some(Provider::Openai));
        assert_eq!(m.provider, CoreProvider::Openai);
    }

    #[test]
    fn pick_respects_force_tier() {
        let c = LLMClient::new(Provider::Anthropic);
        let m = c.pick(TaskType::Chat /* default Fast */, Some(ModelTier::Powerful), None);
        assert_eq!(m.tier, ModelTier::Powerful);
    }
}
