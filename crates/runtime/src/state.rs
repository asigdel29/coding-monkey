/*
   File: crates/runtime/src/state.rs

   Purpose
   Conversation state for a native agent: the message transcript, the
   per-call tool-call records, and the configuration and outcome of a run.
   These types are shared by the LLM client (which produces `ToolCall`s and
   appends `Message`s) and the agent loop (which drives them), so they live
   here rather than in either layer.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — transcript, config, outcome types
*/

use std::path::PathBuf;

use monkey_core::{ModelTier, Provider, TaskType, TokenUsage};
use serde::{Deserialize, Serialize};

/// Author of a transcript [`Message`], in OpenAI chat-completion terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The system prompt assembled from `.monkey/` context.
    System,
    /// A user/task instruction.
    User,
    /// A model turn (may carry `tool_calls`).
    Assistant,
    /// A tool result fed back to the model (carries `tool_call_id`).
    Tool,
}

/// A single tool invocation requested by the model.
///
/// `arguments` is the raw JSON *string* the provider returns; it is parsed
/// at execution time so a malformed value can be surfaced to the model as a
/// correctable error rather than aborting the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call id, echoed back on the matching tool message.
    pub id: String,
    /// Tool name to dispatch.
    pub name: String,
    /// Raw JSON-encoded arguments (unparsed).
    pub arguments: String,
}

/// One entry in an agent's conversation transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,
    /// Text content (may be empty when an assistant turn is only tool calls).
    pub content: String,
    /// Tool calls requested by an assistant turn; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For a `Tool` message, the `ToolCall::id` it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// System message from assembled context.
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }

    /// User/task message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }

    /// Assistant message with optional tool calls.
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            tool_call_id: None,
        }
    }

    /// Tool-result message answering `tool_call_id`.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

/// Static configuration for one agent run.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The task/prompt the agent should accomplish.
    pub task: String,
    /// Task class, used to pick a default model tier.
    pub task_type: TaskType,
    /// Force a specific tier regardless of `task_type`.
    pub force_tier: Option<ModelTier>,
    /// Provider override; falls back to the client default.
    pub provider: Option<Provider>,
    /// Working directory the agent's tools are jailed to.
    pub cwd: PathBuf,
    /// Optional tentacle scope whose context is loaded into the prompt.
    pub tentacle_id: Option<String>,
    /// Hard cap on agent turns before the loop stops (runaway guard).
    pub max_turns: u32,
    /// Output-token cap per LLM call.
    pub max_output_tokens_per_turn: u32,
}

impl AgentConfig {
    /// A config for `task` in `cwd` with conservative defaults.
    pub fn new(task: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            task: task.into(),
            task_type: TaskType::Edit,
            force_tier: None,
            provider: None,
            cwd,
            tentacle_id: None,
            max_turns: 20,
            max_output_tokens_per_turn: 2048,
        }
    }
}

/// Mutable per-run state threaded through the agent loop.
#[derive(Debug, Clone, Default)]
pub struct AgentState {
    /// Full message history sent to the model each turn.
    pub transcript: Vec<Message>,
    /// Completed turns so far.
    pub turn: u32,
    /// Accumulated token usage and cost.
    pub usage: TokenUsage,
    /// Set once a terminal condition is reached.
    pub done: bool,
}

/// How an agent run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// The model called `finish` with this summary.
    Finished {
        /// Final summary the agent reported.
        summary: String,
    },
    /// A cap (turns or tokens) stopped the run.
    LimitReached {
        /// Which cap, in human terms.
        reason: String,
    },
    /// The run was cancelled.
    Cancelled,
    /// The run failed irrecoverably.
    Failed {
        /// Error description.
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_message_round_trips_json() {
        let m = Message::tool_result("call_1", "ok");
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["role"], "tool");
        assert_eq!(j["tool_call_id"], "call_1");
        // Empty tool_calls is omitted from the wire form.
        assert!(j.get("tool_calls").is_none());
    }

    #[test]
    fn assistant_with_tools_keeps_calls() {
        let m = Message::assistant(
            "",
            vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            }],
        );
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["tool_calls"][0]["name"], "read_file");
    }

    #[test]
    fn default_config_has_runaway_guard() {
        let c = AgentConfig::new("do a thing", PathBuf::from("."));
        assert_eq!(c.max_turns, 20);
        assert!(c.max_output_tokens_per_turn >= 1);
    }
}
