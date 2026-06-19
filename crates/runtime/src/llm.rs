/*
   File: crates/runtime/src/llm.rs

   Purpose
   The agent-facing LLM client. Unlike `monkey_skills::llm` (one-shot
   system+user completions), a native agent needs multi-turn messages with
   `tool_calls`/`tool` roles and a `tools` request field, and it needs the
   response's tool calls parsed back out. It also shares ONE reqwest client
   across all agents — building a client per call (as the skill clients do)
   would open a fresh connection pool for every request, which collapses
   under 100 concurrent agents.

   The non-streaming `chat` and the SSE `chat_stream` share one request
   builder and one tool-call/usage model; streaming additionally folds
   incremental deltas (content fragments and per-index tool-call argument
   fragments) into the same `ChatResult`.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — tool-calling chat client, shared
                                 reqwest pool, pure body/response helpers
   2026-06-09   Anubhav Sigdel  add SSE streaming (chat_stream) with
                                 cancellation and delta accumulation
*/

use std::time::Duration;

use futures::StreamExt;
use monkey_core::{
    tier_for_task, ModelRegistry, ModelSelector, ModelSpec, ModelTier, Provider, TaskType,
    TokenUsage,
};
use serde::Deserialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::state::{Message, Role, ToolCall};

/// Failure modes of a chat call, classified so the provider limiter can
/// decide what is worth retrying.
#[derive(Debug, Error)]
pub enum LlmError {
    /// The provider's API key env var is not set.
    #[error("missing API key: {0}")]
    MissingKey(String),
    /// The provider returned a non-success HTTP status.
    #[error("http {status}: {body}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated by the caller for logs).
        body: String,
    },
    /// A transport-level failure (connection reset, timeout, DNS).
    #[error("transport: {0}")]
    Transport(String),
    /// The response body could not be decoded.
    #[error("decode: {0}")]
    Decode(String),
}

impl LlmError {
    /// Whether retrying the same request could plausibly succeed: rate
    /// limits, transient 5xx, and transport errors are retryable; 4xx
    /// (bad request / auth) and decode failures are not.
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Http { status, .. } => *status == 429 || (500..=599).contains(status),
            LlmError::Transport(_) => true,
            LlmError::MissingKey(_) | LlmError::Decode(_) => false,
        }
    }

    /// HTTP status, when this is an HTTP error.
    pub fn status(&self) -> Option<u16> {
        match self {
            LlmError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Outcome of one chat turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResult {
    /// Assistant text (may be empty when the turn is only tool calls).
    pub assistant_text: String,
    /// Tool calls the model requested this turn.
    pub tool_calls: Vec<ToolCall>,
    /// Provider `finish_reason` (e.g. `stop`, `tool_calls`, `length`).
    pub finish_reason: String,
    /// Token usage and estimated cost for this call.
    pub usage: TokenUsage,
}

/// Tier-aware, tool-calling chat client backed by a shared HTTP pool.
#[derive(Debug, Clone)]
pub struct NativeLlm {
    http: reqwest::Client,
    registry: ModelRegistry,
    default_provider: Provider,
}

impl NativeLlm {
    /// Build a client preferring `default_provider`, backed by the builtin
    /// model registry. The HTTP client (and its connection pool) is created
    /// once and cloned cheaply per call.
    pub fn new(default_provider: Provider) -> Self {
        Self::with_registry(default_provider, ModelRegistry::with_builtin())
    }

    /// Build a client preferring `default_provider` and backed by an explicit
    /// `registry` — used to inject the config's locally-served models (GLM-5.2,
    /// Kimi K2.6, a Pi-local small model) so `pick` can select among them.
    pub fn with_registry(default_provider: Provider, registry: ModelRegistry) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .pool_max_idle_per_host(8)
            .build()
            .expect("reqwest client");
        Self {
            http,
            registry,
            default_provider,
        }
    }

    /// The model registry backing this client — used by the orchestrator to
    /// ladder up tiers (find the next-stronger model) during escalation.
    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// Choose a model for `task_type`, honoring an explicit tier/provider.
    pub fn pick(
        &self,
        task_type: TaskType,
        force_tier: Option<ModelTier>,
        provider: Option<Provider>,
    ) -> ModelSpec {
        let p = provider.unwrap_or(self.default_provider);
        let tier = force_tier.unwrap_or_else(|| tier_for_task(task_type));
        // Prefer a model in the requested tier from the requested provider;
        // otherwise fall back to the selector, then to anything registered.
        self.registry
            .list_tier(tier)
            .into_iter()
            .find(|m| m.provider == p)
            .or_else(|| {
                ModelSelector::new(&self.registry)
                    .prefer(p)
                    .select(task_type)
                    .cloned()
            })
            .or_else(|| self.registry.list_all().into_iter().next())
            .expect("registry non-empty")
    }

    /// Run one non-streaming chat turn.
    pub async fn chat(
        &self,
        model: &ModelSpec,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_output_tokens: u32,
    ) -> Result<ChatResult, LlmError> {
        let wire = monkey_core::model_wire(model).map_err(LlmError::MissingKey)?;
        if wire.key_required && wire.key.is_none() {
            return Err(LlmError::MissingKey(format!(
                "no API key for {:?}",
                model.provider
            )));
        }
        let body = build_request_body(model, messages, tools, max_output_tokens, false);

        let mut req = self.http.post(&wire.url).json(&body);
        if let Some(key) = &wire.key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status();
        let raw = resp
            .text()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(LlmError::Http {
                status: status.as_u16(),
                body: raw,
            });
        }
        parse_chat_response(&raw, model)
    }

    /// Run one streaming chat turn. `on_delta` is invoked with each assistant
    /// text fragment as it arrives; the fully-assembled [`ChatResult`]
    /// (including tool calls and usage) is returned at the end.
    ///
    /// Honors `cancel`: when it fires, the HTTP stream is dropped and
    /// whatever has accumulated so far is returned, so the caller can stop
    /// promptly rather than waiting for the full generation.
    pub async fn chat_stream(
        &self,
        model: &ModelSpec,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_output_tokens: u32,
        cancel: &CancellationToken,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ChatResult, LlmError> {
        let wire = monkey_core::model_wire(model).map_err(LlmError::MissingKey)?;
        if wire.key_required && wire.key.is_none() {
            return Err(LlmError::MissingKey(format!(
                "no API key for {:?}",
                model.provider
            )));
        }
        let body = build_request_body(model, messages, tools, max_output_tokens, true);

        let mut req = self.http.post(&wire.url).json(&body);
        if let Some(key) = &wire.key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let raw = resp.text().await.unwrap_or_default();
            return Err(LlmError::Http {
                status: status.as_u16(),
                body: raw,
            });
        }

        let mut stream = resp.bytes_stream();
        let mut acc = StreamAccumulator::default();
        let mut buf = String::new();
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                item = stream.next() => match item {
                    Some(Ok(bytes)) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        if drain_sse_lines(&mut buf, &mut acc, on_delta)? {
                            break; // saw [DONE]
                        }
                    }
                    Some(Err(e)) => return Err(LlmError::Transport(e.to_string())),
                    None => break,
                },
            }
        }
        Ok(acc.into_result(model))
    }
}

/// Pull every complete `\n`-terminated SSE line out of `buf`, leaving any
/// partial trailing line in place. Feeds each `data:` payload to `acc` and
/// forwards content deltas to `on_delta`. @return `true` once `[DONE]` is
/// seen.
fn drain_sse_lines(
    buf: &mut String,
    acc: &mut StreamAccumulator,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> Result<bool, LlmError> {
    while let Some(nl) = buf.find('\n') {
        let line = buf[..nl].trim().to_string();
        buf.drain(..=nl);
        let Some(data) = line.strip_prefix("data:") else {
            continue; // comments, event: lines, blank separators
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            return Ok(true);
        }
        if let Some(delta) = acc.push(data)? {
            on_delta(&delta);
        }
    }
    Ok(false)
}

/// Folds streaming chunks into a final result. Content fragments append in
/// order; tool-call fragments accumulate per `index` (id/name set once,
/// arguments concatenated).
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tool_calls: Vec<ToolCall>,
    finish_reason: String,
    usage: Option<(u64, u64)>,
}

impl StreamAccumulator {
    fn push(&mut self, json: &str) -> Result<Option<String>, LlmError> {
        let chunk: StreamChunk =
            serde_json::from_str(json).map_err(|e| LlmError::Decode(format!("{e}: {json}")))?;
        if let Some(u) = chunk.usage {
            self.usage = Some((u.prompt_tokens, u.completion_tokens));
        }
        let mut delta_text = None;
        for choice in chunk.choices {
            if let Some(fr) = choice.finish_reason {
                self.finish_reason = fr;
            }
            if let Some(c) = choice.delta.content {
                if !c.is_empty() {
                    self.content.push_str(&c);
                    delta_text = Some(c);
                }
            }
            for tcd in choice.delta.tool_calls {
                let slot = self.slot(tcd.index);
                if let Some(id) = tcd.id {
                    if !id.is_empty() {
                        slot.id = id;
                    }
                }
                if let Some(f) = tcd.function {
                    if let Some(n) = f.name {
                        if !n.is_empty() {
                            slot.name = n;
                        }
                    }
                    if let Some(a) = f.arguments {
                        slot.arguments.push_str(&a);
                    }
                }
            }
        }
        Ok(delta_text)
    }

    fn slot(&mut self, index: usize) -> &mut ToolCall {
        while self.tool_calls.len() <= index {
            self.tool_calls.push(ToolCall {
                id: String::new(),
                name: String::new(),
                arguments: String::new(),
            });
        }
        &mut self.tool_calls[index]
    }

    fn into_result(self, model: &ModelSpec) -> ChatResult {
        let (inp, outp) = self.usage.unwrap_or((0, 0));
        ChatResult {
            assistant_text: self.content,
            tool_calls: self.tool_calls,
            finish_reason: self.finish_reason,
            usage: TokenUsage {
                input_tokens: inp,
                output_tokens: outp,
                total_tokens: inp + outp,
                estimated_cost_usd: cost(model, inp, outp),
            },
        }
    }
}

/// Build the OpenAI-compatible request body. Pure (no I/O) so the wire
/// shape is unit-testable. `stream` toggles SSE for the streaming path.
pub(crate) fn build_request_body(
    model: &ModelSpec,
    messages: &[Message],
    tools: &[serde_json::Value],
    max_output_tokens: u32,
    stream: bool,
) -> serde_json::Value {
    let wire_messages: Vec<serde_json::Value> = messages.iter().map(message_to_wire).collect();
    let mut body = serde_json::json!({
        "model": model.id,
        "max_tokens": max_output_tokens,
        "messages": wire_messages,
        "stream": stream,
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools.to_vec());
        body["tool_choice"] = serde_json::json!("auto");
    }
    if stream {
        // Ask the provider to emit a final usage chunk so streaming runs
        // still get token/cost accounting.
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    body
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn message_to_wire(m: &Message) -> serde_json::Value {
    let mut v = serde_json::json!({ "role": role_str(m.role), "content": m.content });
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = serde_json::Value::Array(
            m.tool_calls
                .iter()
                .map(|tc| {
                    serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments },
                    })
                })
                .collect(),
        );
    }
    if let Some(id) = &m.tool_call_id {
        v["tool_call_id"] = serde_json::json!(id);
    }
    v
}

/// Parse an OpenAI-compatible chat response into a [`ChatResult`], computing
/// USD cost from `model`'s rate. Pure (no I/O) for testing.
pub(crate) fn parse_chat_response(raw: &str, model: &ModelSpec) -> Result<ChatResult, LlmError> {
    let parsed: WireResp =
        serde_json::from_str(raw).map_err(|e| LlmError::Decode(format!("{e}: {raw}")))?;
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::Decode("no choices in response".into()))?;
    let tool_calls = choice
        .message
        .tool_calls
        .into_iter()
        .map(|tc| ToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments: arguments_to_string(tc.function.arguments),
        })
        .collect();
    let inp = parsed.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0);
    let outp = parsed
        .usage
        .as_ref()
        .map(|u| u.completion_tokens)
        .unwrap_or(0);
    Ok(ChatResult {
        assistant_text: choice.message.content.unwrap_or_default(),
        tool_calls,
        finish_reason: choice.finish_reason.unwrap_or_default(),
        usage: TokenUsage {
            input_tokens: inp,
            output_tokens: outp,
            total_tokens: inp + outp,
            estimated_cost_usd: cost(model, inp, outp),
        },
    })
}

/// Tool-call `arguments` are conventionally a JSON string; tolerate a raw
/// object by re-serializing it.
fn arguments_to_string(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Null => "{}".into(),
        other => other.to_string(),
    }
}

fn cost(model: &ModelSpec, input: u64, output: u64) -> f64 {
    input as f64 / 1000.0 * model.input_cost_per_1k
        + output as f64 / 1000.0 * model.output_cost_per_1k
}

#[derive(Deserialize)]
struct WireResp {
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFn,
}

#[derive(Deserialize)]
struct WireFn {
    name: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCallDelta>,
}

#[derive(Deserialize)]
struct StreamToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFnDelta>,
}

#[derive(Deserialize)]
struct StreamFnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelSpec {
        ModelRegistry::with_builtin()
            .list_tier(ModelTier::Fast)
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn pick_prefers_requested_provider_and_tier() {
        let llm = NativeLlm::new(Provider::OpenRouter);
        let m = llm.pick(
            TaskType::Chat,
            Some(ModelTier::Powerful),
            Some(Provider::Openai),
        );
        assert_eq!(m.provider, Provider::Openai);
        assert_eq!(m.tier, ModelTier::Powerful);
    }

    #[test]
    fn body_includes_tools_and_maps_roles() {
        let msgs = vec![
            Message::system("sys"),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a\"}".into(),
                }],
            ),
            Message::tool_result("c1", "contents"),
        ];
        let tools = vec![serde_json::json!({"type":"function","function":{"name":"read_file"}})];
        let body = build_request_body(&model(), &msgs, &tools, 256, false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "c1");
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn no_tools_omits_tool_fields() {
        let body = build_request_body(&model(), &[Message::user("hi")], &[], 64, false);
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn parse_extracts_tool_calls_and_usage() {
        let raw = r#"{
          "choices": [{
            "message": {
              "content": null,
              "tool_calls": [{"id":"c9","type":"function",
                "function":{"name":"search","arguments":"{\"q\":\"x\"}"}}]
            },
            "finish_reason": "tool_calls"
          }],
          "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }"#;
        let r = parse_chat_response(raw, &model()).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "search");
        assert_eq!(r.tool_calls[0].arguments, "{\"q\":\"x\"}");
        assert_eq!(r.finish_reason, "tool_calls");
        assert_eq!(r.usage.total_tokens, 15);
        assert!(r.usage.estimated_cost_usd > 0.0);
    }

    #[test]
    fn error_retryability_classification() {
        assert!(LlmError::Http {
            status: 429,
            body: String::new()
        }
        .is_retryable());
        assert!(LlmError::Http {
            status: 503,
            body: String::new()
        }
        .is_retryable());
        assert!(!LlmError::Http {
            status: 400,
            body: String::new()
        }
        .is_retryable());
        assert!(!LlmError::MissingKey("X".into()).is_retryable());
        assert!(LlmError::Transport("reset".into()).is_retryable());
    }

    #[test]
    fn streaming_body_requests_usage() {
        let body = build_request_body(&model(), &[Message::user("hi")], &[], 64, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn accumulator_folds_text_and_split_tool_args() {
        // Tool-call arguments arrive split across two chunks; content too.
        let chunks = [
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        ];
        let mut acc = StreamAccumulator::default();
        let mut seen = String::new();
        for c in chunks {
            if let Some(d) = acc.push(c).unwrap() {
                seen.push_str(&d);
            }
        }
        let r = acc.into_result(&model());
        assert_eq!(seen, "Hello");
        assert_eq!(r.assistant_text, "Hello");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "read_file");
        assert_eq!(r.tool_calls[0].arguments, "{\"path\":\"a\"}");
        assert_eq!(r.finish_reason, "tool_calls");
        assert_eq!(r.usage.total_tokens, 10);
    }

    #[test]
    fn drain_handles_partial_lines_and_done() {
        let mut acc = StreamAccumulator::default();
        let mut got = String::new();
        // First buffer ends mid-line; the remainder completes it next call.
        let mut buf =
            String::from("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\ndata: {\"choi");
        {
            let mut cb = |s: &str| got.push_str(s);
            assert!(!drain_sse_lines(&mut buf, &mut acc, &mut cb).unwrap());
        }
        assert_eq!(got, "hi");
        buf.push_str("ces\":[{\"delta\":{\"content\":\"!\"}}]}\ndata: [DONE]\n");
        {
            let mut cb = |s: &str| got.push_str(s);
            assert!(drain_sse_lines(&mut buf, &mut acc, &mut cb).unwrap());
        }
        assert_eq!(got, "hi!");
    }
}
