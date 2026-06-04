/*
   File: crates/engulf/src/llm.rs

   Purpose
   Minimal HTTP client for the OpenAI-compatible Chat Completions API.
   The same wire format serves both OpenAI directly and OpenRouter (one
   key, many upstream models), so a single code path covers both. Engulf
   uses this for the security/docs/deploy phases. Skills share the module
   via re-export from monkey-engulf until enough callers exist to justify
   lifting it into a dedicated crate.

   Why hand-roll instead of using a vendor SDK
   - Vendor SDKs pull in heavy async/codegen dependencies
   - We only need the "send a prompt, get text back" path
   - Keeping it in-tree means provider quirks (rate-limit headers,
     non-streaming JSON shape) are visible and tweakable

   Design
   - Errors are anyhow::Result so callers can `?` cleanly
   - Provider authentication is read from the provider's key env var
     ($OPENROUTER_API_KEY / $OPENAI_API_KEY) unless a key is passed
   - Response text is unwrapped from the first message choice
   - Markdown JSON fences (```json … ```) are stripped before parse

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port — minimal LLM client used
                                 by the security phase
   2026-06-03   Anubhav Sigdel  collapse to one OpenAI-compatible path;
                                 add OpenRouter; drop bespoke provider clients
*/

use anyhow::{anyhow, Context};
use serde::Deserialize;
use std::time::Duration;

/// Provider knobs. Both speak the OpenAI Chat Completions wire format.
#[derive(Debug, Clone, Copy)]
pub enum Provider {
    /// OpenRouter — `https://openrouter.ai/api/v1/chat/completions`.
    /// One key proxies to many upstream models.
    OpenRouter,
    /// OpenAI — `https://api.openai.com/v1/chat/completions`.
    Openai,
}

impl Provider {
    /// Chat-completions endpoint for this provider.
    fn endpoint(self) -> &'static str {
        match self {
            Provider::OpenRouter => "https://openrouter.ai/api/v1/chat/completions",
            Provider::Openai => "https://api.openai.com/v1/chat/completions",
        }
    }

    /// Environment variable holding this provider's API key.
    pub fn key_env(self) -> &'static str {
        match self {
            Provider::OpenRouter => "OPENROUTER_API_KEY",
            Provider::Openai => "OPENAI_API_KEY",
        }
    }
}

/// Generic prompt request. `system` is optional; user prompts are required.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    /// Provider to call.
    pub provider: Provider,
    /// Provider's model id (`openai/gpt-4o`, `gpt-5-mini`, …).
    pub model: String,
    /// Optional system prompt.
    pub system: Option<String>,
    /// User prompt.
    pub user: String,
    /// Cap on tokens emitted.
    pub max_tokens: u32,
    /// API key override; defaults to env var.
    pub api_key: Option<String>,
}

/// Send `req` and return the model's text response. Errors carry
/// enough detail that the caller can decide whether to surface them
/// or fall back silently (engulf's security phase prefers silent
/// fallback so a key-less run still produces static findings).
pub async fn complete(req: PromptRequest) -> anyhow::Result<String> {
    let key = match req.api_key.clone() {
        Some(k) => k,
        None => {
            let env = req.provider.key_env();
            std::env::var(env).map_err(|_| anyhow!("{env} not set"))?
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build http client")?;

    chat_completions(&client, &key, &req).await
}

// ─── OpenAI-compatible Chat Completions ──────────────────────────────────────

/// POST a single non-streaming chat completion to the request's provider
/// endpoint and return the first choice's text.
async fn chat_completions(
    client: &reqwest::Client,
    key: &str,
    req: &PromptRequest,
) -> anyhow::Result<String> {
    let mut messages = Vec::new();
    if let Some(sys) = &req.system {
        messages.push(serde_json::json!({ "role": "system", "content": sys }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": req.user }));

    let body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
    });
    let resp = client
        .post(req.provider.endpoint())
        .bearer_auth(key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("chat completion request failed")?;
    let status = resp.status();
    let raw = resp.text().await.context("read response body")?;
    if !status.is_success() {
        return Err(anyhow!("{status}: {raw}"));
    }
    let parsed: OpenAIResp =
        serde_json::from_str(&raw).with_context(|| format!("decode response: {raw}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| anyhow!("response had no choices"))
}

#[derive(Deserialize)]
struct OpenAIResp {
    choices: Vec<OpenAIChoice>,
}

#[derive(Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize)]
struct OpenAIMessage {
    content: String,
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// Strip `\`\`\`json … \`\`\`` or `\`\`\` … \`\`\`` fences if the model
/// wrapped its JSON output. Idempotent — returns `s` unchanged if no
/// fences are present.
pub fn strip_json_fences(s: &str) -> &str {
    let s = s.trim();
    let candidates = [
        s.strip_prefix("```json")
            .and_then(|x| x.strip_suffix("```")),
        s.strip_prefix("```JSON")
            .and_then(|x| x.strip_suffix("```")),
        s.strip_prefix("```").and_then(|x| x.strip_suffix("```")),
    ];
    if let Some(stripped) = candidates.into_iter().flatten().next() {
        return stripped.trim();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_handles_json_label() {
        assert_eq!(strip_json_fences("```json\n[1,2]\n```"), "[1,2]");
    }

    #[test]
    fn strip_fences_handles_unlabeled() {
        assert_eq!(strip_json_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
    }

    #[test]
    fn strip_fences_passes_clean_input_through() {
        assert_eq!(strip_json_fences("[1,2]"), "[1,2]");
        assert_eq!(strip_json_fences("  [1,2]  "), "[1,2]");
    }
}
