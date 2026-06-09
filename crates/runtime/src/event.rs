/*
   File: crates/runtime/src/event.rs

   Purpose
   The progress stream a running agent emits. The agent loop sends these
   over a bounded channel; consumers (the deck WebSocket bridge, a CLI
   renderer, the audit log) forward them. Lifecycle variants
   (Finished/Failed/Cancelled/LimitReached) are terminal and must never be
   dropped under backpressure; `AssistantDelta` is the only lossy variant.

   `AgentEvent` is `Serialize` so the deck can forward it verbatim as JSON.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — AgentEvent stream type
*/

use monkey_core::TokenUsage;
use serde::Serialize;

/// A single progress event from a running agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent task has been admitted and started.
    Started {
        /// Stable agent id.
        agent_id: String,
    },
    /// A streamed chunk of the current assistant turn (lossy under load).
    AssistantDelta {
        /// Text fragment.
        text: String,
    },
    /// A complete assistant turn's text.
    AssistantMessage {
        /// Full assistant text.
        text: String,
    },
    /// A tool call is about to execute.
    ToolCallStarted {
        /// Tool name.
        name: String,
        /// Short preview of the arguments.
        args_preview: String,
    },
    /// A tool call finished.
    ToolCallFinished {
        /// Tool name.
        name: String,
        /// Whether it succeeded.
        ok: bool,
        /// Short preview of the output.
        output_preview: String,
    },
    /// Cumulative token usage after a turn.
    Usage(TokenUsage),
    /// Terminal: the agent called `finish`.
    Finished {
        /// Final summary.
        summary: String,
    },
    /// Terminal: a turn or token cap stopped the run.
    LimitReached {
        /// Which cap was hit.
        reason: String,
    },
    /// Terminal: the run failed.
    Failed {
        /// Error description.
        error: String,
    },
    /// Terminal: the run was cancelled.
    Cancelled,
}

impl AgentEvent {
    /// True for the terminal variants that must not be dropped under
    /// backpressure.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentEvent::Finished { .. }
                | AgentEvent::LimitReached { .. }
                | AgentEvent::Failed { .. }
                | AgentEvent::Cancelled
        )
    }

    /// True for high-volume, lossy variants (token deltas) that may be
    /// dropped to keep a slow consumer from stalling the agent.
    pub fn is_lossy(&self) -> bool {
        matches!(self, AgentEvent::AssistantDelta { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_lossy_classification() {
        assert!(AgentEvent::Cancelled.is_terminal());
        assert!(AgentEvent::Finished {
            summary: "done".into()
        }
        .is_terminal());
        assert!(AgentEvent::AssistantDelta { text: "x".into() }.is_lossy());
        assert!(!AgentEvent::AssistantDelta { text: "x".into() }.is_terminal());
    }

    #[test]
    fn serializes_with_event_tag() {
        let j = serde_json::to_value(AgentEvent::Started {
            agent_id: "a1".into(),
        })
        .unwrap();
        assert_eq!(j["event"], "started");
        assert_eq!(j["agent_id"], "a1");
    }
}
