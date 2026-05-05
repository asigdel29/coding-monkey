/*
   File: crates/engulf/src/llm.rs

   Purpose
   Minimal HTTP clients for the Anthropic Messages API and the OpenAI
   Chat Completions API. Engulf uses these for the security/docs/deploy
   phases. Skills will share this module via re-export from monkey-engulf
   until enough callers exist to justify lifting it into a dedicated
   crate.

   Why hand-roll instead of using a vendor SDK
   - Vendor SDKs pull in heavy async/codegen dependencies
   - We only need the "send a prompt, get text back" path
   - Keeping it in-tree means provider quirks (rate-limit headers,
     non-streaming JSON shape) are visible and tweakable

   Design
   - Errors are anyhow::Result so callers can `?` cleanly
   - Provider authentication is read from $ANTHROPIC_API_KEY / $OPENAI_API_KEY
     unless an explicit key is passed
   - Response text is unwrapped from the first content/message block
   - Markdown JSON fences (```json … ```) are stripped before parse

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port — minimal Anthropic +
                                 OpenAI clients used by the security phase
*/

use anyhow::{anyhow, Context};
use serde::Deserialize;
use std::time::Duration;

/// Provider knobs.
#[derive(Debug, Clone, Copy)]
pub enum Provider {
    /// Anthropic — `https://api.anthropic.com/v1/messages`.
    Anthropic,
    /// OpenAI — `https://api.openai.com/v1/chat/completions`.
    Openai,
}

/// Generic prompt request. `system` is optional; user prompts are required.
#[derive(Debug, Clone)]
pub struct PromptRequest {
    /// Provider to call.
    pub provider: Provider,
    /// Provider's model id (`claude-haiku-4-5`, `gpt-5-mini`, …).
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
    let key = match req.api_key {
        Some(k) => k,
        None => match req.provider {
            Provider::Anthropic => std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow!("ANTHROPIC_API_KEY not set"))?,
            Provider::Openai => std::env::var("OPENAI_API_KEY")
                .map_err(|_| anyhow!("OPENAI_API_KEY not set"))?,
        },
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build http client")?;

    match req.provider {
        Provider::Anthropic => anthropic(&client, &key, &req).await,
        Provider::Openai => openai(&client, &key, &req).await,
    }
}

// ─── Anthropic ──────────────────────────────────────────────────────────────

async fn anthropic(
    client: &reqwest::Client,
    key: &str,
    req: &PromptRequest,
) -> anyhow::Result<String> {
    let body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
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
    let raw = resp.text().await.context("read anthropic body")?;
    if !status.is_success() {
        return Err(anyhow!("anthropic {}: {}", status, raw));
    }
    let parsed: AnthropicResp =
        serde_json::from_str(&raw).with_context(|| format!("anthropic decode: {raw}"))?;
    parsed
        .content
        .into_iter()
        .find_map(|b| if b.kind == "text" { Some(b.text) } else { None })
        .ok_or_else(|| anyhow!("anthropic response had no text block"))
}

#[derive(Deserialize)]
struct AnthropicResp {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

// ─── OpenAI ─────────────────────────────────────────────────────────────────

async fn openai(
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
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("openai request failed")?;
    let status = resp.status();
    let raw = resp.text().await.context("read openai body")?;
    if !status.is_success() {
        return Err(anyhow!("openai {}: {}", status, raw));
    }
    let parsed: OpenAIResp =
        serde_json::from_str(&raw).with_context(|| format!("openai decode: {raw}"))?;
    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| anyhow!("openai response had no choices"))
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
        s.strip_prefix("```json").and_then(|x| x.strip_suffix("```")),
        s.strip_prefix("```JSON").and_then(|x| x.strip_suffix("```")),
        s.strip_prefix("```").and_then(|x| x.strip_suffix("```")),
    ];
    for c in candidates {
        if let Some(stripped) = c {
            return stripped.trim();
        }
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
