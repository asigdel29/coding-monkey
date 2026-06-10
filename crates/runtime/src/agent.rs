/*
   File: crates/runtime/src/agent.rs

   Purpose
   The native agent loop: assemble the system+task prompt, call the LLM with
   the tool schemas, execute any tool calls in order, append the results,
   and repeat until the model calls `finish`, a cap is hit, or cancellation
   fires. The LLM call is abstracted behind `ChatBackend` so the loop is
   testable with a scripted backend and so streaming vs non-streaming is an
   implementation detail of the backend.

   Guards against the two failure modes a tool-using loop invites: runaway
   length (turn cap) and getting stuck repeating one call (a repeat cap on
   identical tool calls). Malformed or unknown tool calls are surfaced to
   the model as tool errors so it can self-correct rather than aborting.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — agent loop + ChatBackend trait
*/

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use monkey_core::{ModelSpec, TokenUsage};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::event::AgentEvent;
use crate::limiter::ProviderLimiter;
use crate::llm::{ChatResult, LlmError, NativeLlm};
use crate::state::{AgentConfig, AgentOutcome, AgentState, Message};
use crate::tool::{ToolCtx, ToolRegistry};
use crate::tools::finish::parse_summary;

/// Max bytes any single tool may return to the model.
const TOOL_OUTPUT_BUDGET: usize = 8 * 1024;

/// How many times the exact same tool call may occur before the loop gives
/// up as stuck.
const MAX_IDENTICAL_CALLS: u32 = 4;

/// One LLM turn, abstracted so the agent loop can be driven by the real
/// `NativeLlm` or a scripted test backend. `on_delta` receives streamed
/// assistant text fragments.
#[async_trait]
pub trait ChatBackend: Send + Sync {
    /// Produce one assistant turn for `messages` with `tools` available.
    async fn chat(
        &self,
        model: &ModelSpec,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
        cancel: &CancellationToken,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ChatResult, LlmError>;
}

#[async_trait]
impl ChatBackend for NativeLlm {
    async fn chat(
        &self,
        model: &ModelSpec,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
        cancel: &CancellationToken,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ChatResult, LlmError> {
        self.chat_stream(model, messages, tools, max_tokens, cancel, on_delta)
            .await
    }
}

/// A [`ChatBackend`] that runs the real `NativeLlm` through a
/// [`ProviderLimiter`], so every agent's calls share the fleet's per-provider
/// concurrency, pacing, and 429 backoff. Retries are handled here (rather
/// than via `run_with_retry`) so the borrowed `on_delta` sink can be reused
/// across attempts.
#[derive(Debug, Clone)]
pub struct LimitedBackend {
    llm: Arc<NativeLlm>,
    limiter: Arc<ProviderLimiter>,
}

impl LimitedBackend {
    /// Wrap `llm` so its calls are gated by `limiter`.
    pub fn new(llm: Arc<NativeLlm>, limiter: Arc<ProviderLimiter>) -> Self {
        Self { llm, limiter }
    }
}

#[async_trait]
impl ChatBackend for LimitedBackend {
    async fn chat(
        &self,
        model: &ModelSpec,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
        cancel: &CancellationToken,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ChatResult, LlmError> {
        let provider = model.provider;
        let mut attempt = 0;
        loop {
            let permit = self.limiter.acquire(provider).await;
            match self
                .llm
                .chat_stream(model, messages, tools, max_tokens, cancel, on_delta)
                .await
            {
                Ok(v) => {
                    self.limiter.note_success(provider);
                    return Ok(v);
                }
                Err(e) if e.is_retryable() && attempt < self.limiter.max_retries() => {
                    drop(permit);
                    self.limiter.note_failure(provider);
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Build the system prompt that frames a native agent: who it is, the tool
/// discipline, and that it must call `finish` to stop.
fn system_prompt(cfg: &AgentConfig) -> String {
    format!(
        "You are a native coding agent operating inside {}. \
         Work towards the user's task using the provided tools. \
         Read before you write, keep changes minimal, and verify with run_command when useful. \
         All file paths are relative to the working directory. \
         When the task is complete, call the `finish` tool with a short summary.{}",
        cfg.cwd.display(),
        cfg.tentacle_id
            .as_ref()
            .map(|t| format!(" You are scoped to tentacle '{t}'."))
            .unwrap_or_default()
    )
}

/// Run an agent to completion. `agent_id` labels emitted events; `events`
/// is the (bounded) progress channel; `cancel` stops the run cooperatively.
///
/// Tool calls within a turn execute sequentially — a model expects ordered
/// results — while parallelism happens across agents, not within one.
pub async fn run_agent(
    agent_id: String,
    cfg: AgentConfig,
    registry: Arc<ToolRegistry>,
    backend: Arc<dyn ChatBackend>,
    model: ModelSpec,
    events: Sender<AgentEvent>,
    cancel: CancellationToken,
) -> AgentOutcome {
    emit(
        &events,
        AgentEvent::Started {
            agent_id: agent_id.clone(),
        },
    )
    .await;

    let mut ctx = ToolCtx::new(cfg.cwd.clone(), TOOL_OUTPUT_BUDGET);
    ctx.cancel = cancel.clone();

    let mut state = AgentState::default();
    state.transcript.push(Message::system(system_prompt(&cfg)));
    state.transcript.push(Message::user(cfg.task.clone()));

    let tool_schemas = registry.schemas();
    let mut call_counts: HashMap<String, u32> = HashMap::new();

    loop {
        if cancel.is_cancelled() {
            emit(&events, AgentEvent::Cancelled).await;
            return AgentOutcome::Cancelled;
        }
        if state.turn >= cfg.max_turns {
            let reason = format!("reached max turns ({})", cfg.max_turns);
            emit(
                &events,
                AgentEvent::LimitReached {
                    reason: reason.clone(),
                },
            )
            .await;
            return AgentOutcome::LimitReached { reason };
        }

        let mut delta_sink = |s: &str| {
            let _ = events.try_send(AgentEvent::AssistantDelta {
                text: s.to_string(),
            });
        };
        let chat = match backend
            .chat(
                &model,
                &state.transcript,
                &tool_schemas,
                cfg.max_output_tokens_per_turn,
                &cancel,
                &mut delta_sink,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                let error = e.to_string();
                emit(
                    &events,
                    AgentEvent::Failed {
                        error: error.clone(),
                    },
                )
                .await;
                return AgentOutcome::Failed { error };
            }
        };

        state.turn += 1;
        state.usage = TokenUsage::merge(&state.usage, &chat.usage);
        emit(&events, AgentEvent::Usage(chat.usage.clone())).await;
        if !chat.assistant_text.is_empty() {
            emit(
                &events,
                AgentEvent::AssistantMessage {
                    text: chat.assistant_text.clone(),
                },
            )
            .await;
        }

        state.transcript.push(Message::assistant(
            chat.assistant_text.clone(),
            chat.tool_calls.clone(),
        ));

        // No tool calls: the model is done talking. Treat its text as the
        // result rather than looping forever waiting for `finish`.
        if chat.tool_calls.is_empty() {
            let summary = if chat.assistant_text.is_empty() {
                "agent stopped without output".to_string()
            } else {
                chat.assistant_text.clone()
            };
            emit(
                &events,
                AgentEvent::Finished {
                    summary: summary.clone(),
                },
            )
            .await;
            return AgentOutcome::Finished { summary };
        }

        for call in chat.tool_calls {
            if call.name == "finish" {
                let summary = parse_summary(&call.arguments);
                emit(
                    &events,
                    AgentEvent::Finished {
                        summary: summary.clone(),
                    },
                )
                .await;
                return AgentOutcome::Finished { summary };
            }

            let sig = format!("{}:{}", call.name, call.arguments);
            let count = call_counts.entry(sig).or_insert(0);
            *count += 1;
            if *count > MAX_IDENTICAL_CALLS {
                let reason = format!("stuck repeating '{}' tool call", call.name);
                emit(
                    &events,
                    AgentEvent::LimitReached {
                        reason: reason.clone(),
                    },
                )
                .await;
                return AgentOutcome::LimitReached { reason };
            }

            emit(
                &events,
                AgentEvent::ToolCallStarted {
                    name: call.name.clone(),
                    args_preview: preview(&call.arguments),
                },
            )
            .await;

            let result = match registry.get(&call.name) {
                None => crate::tool::ToolResult::error(format!(
                    "unknown tool '{}'. Available: {}",
                    call.name,
                    registry.names().join(", ")
                )),
                Some(tool) => match serde_json::from_str::<serde_json::Value>(&call.arguments) {
                    Ok(args) => tool.call(&ctx, args).await,
                    Err(e) => crate::tool::ToolResult::error(format!(
                        "invalid JSON arguments for '{}': {e}",
                        call.name
                    )),
                },
            };

            emit(
                &events,
                AgentEvent::ToolCallFinished {
                    name: call.name.clone(),
                    ok: !result.is_error,
                    output_preview: preview(&result.content),
                },
            )
            .await;
            state
                .transcript
                .push(Message::tool_result(call.id, result.content));
        }
    }
}

/// A short, single-line preview of `s` for event payloads.
fn preview(s: &str) -> String {
    const MAX: usize = 120;
    let one_line: String = s.chars().take(MAX).collect::<String>().replace('\n', " ");
    if s.chars().count() > MAX {
        format!("{one_line}…")
    } else {
        one_line
    }
}

/// Send an event, preferring guaranteed delivery for terminal events and
/// best-effort for the rest. Errors (receiver dropped) are ignored — the
/// run continues regardless of whether anyone is listening.
async fn emit(events: &Sender<AgentEvent>, ev: AgentEvent) {
    if ev.is_terminal() {
        let _ = events.send(ev).await;
    } else {
        let _ = events.try_send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ToolCall;
    use crate::tools::default_tools;
    use monkey_core::{ModelRegistry, ModelTier};
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    /// A scripted backend that returns pre-canned turns in order.
    struct Script {
        turns: Mutex<std::collections::VecDeque<ChatResult>>,
    }

    impl Script {
        fn new(turns: Vec<ChatResult>) -> Arc<Self> {
            Arc::new(Self {
                turns: Mutex::new(turns.into()),
            })
        }
    }

    #[async_trait]
    impl ChatBackend for Script {
        async fn chat(
            &self,
            _model: &ModelSpec,
            _messages: &[Message],
            _tools: &[serde_json::Value],
            _max_tokens: u32,
            _cancel: &CancellationToken,
            _on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
        ) -> Result<ChatResult, LlmError> {
            Ok(self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| turn("", vec![])))
        }
    }

    fn model() -> ModelSpec {
        ModelRegistry::with_builtin()
            .list_tier(ModelTier::Fast)
            .into_iter()
            .next()
            .unwrap()
    }

    fn turn(text: &str, calls: Vec<ToolCall>) -> ChatResult {
        ChatResult {
            assistant_text: text.into(),
            tool_calls: calls,
            finish_reason: "stop".into(),
            usage: TokenUsage::empty(),
        }
    }

    fn call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.into(),
        }
    }

    async fn drain(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        // Channel may still have buffered items the loop sent; pull them.
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn reads_then_finishes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "data").unwrap();
        let mut cfg = AgentConfig::new("read f.txt", dir.path().to_path_buf());
        cfg.max_turns = 5;
        let backend = Script::new(vec![
            turn("", vec![call("c1", "read_file", r#"{"path":"f.txt"}"#)]),
            turn("", vec![call("c2", "finish", r#"{"summary":"read it"}"#)]),
        ]);
        let (tx, rx) = mpsc::channel(64);
        let outcome = run_agent(
            "a1".into(),
            cfg,
            Arc::new(default_tools()),
            backend,
            model(),
            tx,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome,
            AgentOutcome::Finished {
                summary: "read it".into()
            }
        );
        let events = drain(rx).await;
        assert!(events.iter().any(|e| matches!(e,
            AgentEvent::ToolCallFinished { name, ok: true, .. } if name == "read_file")));
    }

    #[tokio::test]
    async fn unknown_tool_reported_then_continues() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AgentConfig::new("x", dir.path().to_path_buf());
        let backend = Script::new(vec![
            turn("", vec![call("c1", "does_not_exist", "{}")]),
            turn("", vec![call("c2", "finish", r#"{"summary":"ok"}"#)]),
        ]);
        let (tx, rx) = mpsc::channel(64);
        let outcome = run_agent(
            "a1".into(),
            cfg,
            Arc::new(default_tools()),
            backend,
            model(),
            tx,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome,
            AgentOutcome::Finished {
                summary: "ok".into()
            }
        );
        let events = drain(rx).await;
        assert!(events.iter().any(|e| matches!(e,
            AgentEvent::ToolCallFinished { name, ok: false, .. } if name == "does_not_exist")));
    }

    #[tokio::test]
    async fn no_tool_call_finishes_with_text() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AgentConfig::new("x", dir.path().to_path_buf());
        let backend = Script::new(vec![turn("here is my answer", vec![])]);
        let (tx, _rx) = mpsc::channel(64);
        let outcome = run_agent(
            "a1".into(),
            cfg,
            Arc::new(default_tools()),
            backend,
            model(),
            tx,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome,
            AgentOutcome::Finished {
                summary: "here is my answer".into()
            }
        );
    }

    #[tokio::test]
    async fn max_turns_stops_runaway() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = AgentConfig::new("x", dir.path().to_path_buf());
        cfg.max_turns = 3;
        // Each turn lists a *different* dir so the repeat guard doesn't trip
        // first; the turn cap must.
        let backend = Script::new(vec![
            turn("", vec![call("c1", "list_dir", r#"{"path":"."}"#)]),
            turn("", vec![call("c2", "list_dir", r#"{"path":"a"}"#)]),
            turn("", vec![call("c3", "list_dir", r#"{"path":"b"}"#)]),
            turn("", vec![call("c4", "list_dir", r#"{"path":"c"}"#)]),
        ]);
        let (tx, _rx) = mpsc::channel(64);
        let outcome = run_agent(
            "a1".into(),
            cfg,
            Arc::new(default_tools()),
            backend,
            model(),
            tx,
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(outcome, AgentOutcome::LimitReached { .. }));
    }

    #[tokio::test]
    async fn pre_cancelled_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AgentConfig::new("x", dir.path().to_path_buf());
        let backend = Script::new(vec![turn("", vec![call("c1", "finish", "{}")])]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = mpsc::channel(64);
        let outcome = run_agent(
            "a1".into(),
            cfg,
            Arc::new(default_tools()),
            backend,
            model(),
            tx,
            cancel,
        )
        .await;
        assert_eq!(outcome, AgentOutcome::Cancelled);
    }
}
