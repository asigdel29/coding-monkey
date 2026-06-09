/*
   File: crates/runtime/src/tool.rs

   Purpose
   The tool interface a native agent calls into. A `Tool` advertises a
   JSON-Schema for its parameters (sent to the model as a callable function)
   and executes a request against a `ToolCtx` — the per-agent sandbox
   carrying the working-directory jail, a cancellation signal, and an output
   byte budget. Concrete tools (read_file, write_file, search, run_command,
   finish) are added in later changes; this defines the contract and the
   registry that the agent loop and the LLM client share.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — Tool trait, ToolCtx, ToolResult,
                                 ToolRegistry
*/

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Per-agent execution sandbox handed to every [`Tool::call`].
#[derive(Debug, Clone)]
pub struct ToolCtx {
    /// Directory the agent is scoped to. Filesystem tools must reject paths
    /// that escape this root (enforced by the fs guard added later).
    pub cwd: PathBuf,
    /// Cooperative cancellation: long-running tools should watch this and
    /// abort promptly when it fires.
    pub cancel: CancellationToken,
    /// Maximum bytes a tool may return to the model. Tools truncate to this
    /// and set [`ToolResult::truncated`] so the model knows output was cut.
    pub output_budget: usize,
}

impl ToolCtx {
    /// Build a context jailed to `cwd` with a fresh cancellation token and
    /// the given output budget.
    pub fn new(cwd: PathBuf, output_budget: usize) -> Self {
        Self {
            cwd,
            cancel: CancellationToken::new(),
            output_budget,
        }
    }
}

/// The result of a [`Tool::call`], appended to the transcript as a tool
/// message and surfaced as an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// Text returned to the model.
    pub content: String,
    /// Whether this represents a tool error (the model should adapt, not
    /// the run abort).
    pub is_error: bool,
    /// Whether `content` was truncated to fit the output budget.
    pub truncated: bool,
}

impl ToolResult {
    /// A successful, untruncated result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            truncated: false,
        }
    }

    /// An error result the model can read and correct against.
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            truncated: false,
        }
    }

    /// Truncate `content` to `budget` bytes (on a char boundary), marking
    /// [`ToolResult::truncated`] when anything was dropped.
    pub fn truncate_to(mut self, budget: usize) -> Self {
        if self.content.len() > budget {
            let mut end = budget;
            while end > 0 && !self.content.is_char_boundary(end) {
                end -= 1;
            }
            self.content.truncate(end);
            self.truncated = true;
        }
        self
    }
}

/// A callable tool. Object-safe so tools live behind `Arc<dyn Tool>`.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable tool name the model invokes (e.g. `read_file`).
    fn name(&self) -> &str;

    /// One-line description shown to the model.
    fn description(&self) -> &str;

    /// JSON Schema for this tool's parameters, embedded in the LLM `tools`
    /// request field.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute against `ctx`. `args` is the parsed JSON arguments object.
    /// Implementations should never panic; return [`ToolResult::error`] for
    /// recoverable failures so the model can adapt.
    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult;
}

/// Shared handle to a tool.
pub type ToolRef = Arc<dyn Tool>;

/// A name-indexed set of tools available to an agent.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<ToolRef>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.names())
            .finish()
    }
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `tool`. A later registration with the same name shadows an
    /// earlier one on lookup (last wins), keeping override simple.
    pub fn register(&mut self, tool: ToolRef) {
        self.tools.push(tool);
    }

    /// Look up a tool by name (last registered wins).
    pub fn get(&self, name: &str) -> Option<&ToolRef> {
        self.tools.iter().rev().find(|t| t.name() == name)
    }

    /// Registered tool names, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    /// The OpenAI-style `tools` array describing every registered tool,
    /// for the LLM request.
    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo the input text"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }
        async fn call(&self, _ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
            let t = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            ToolResult::ok(t)
        }
    }

    #[tokio::test]
    async fn registry_dispatch_and_schema() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Echo));
        assert_eq!(reg.names(), vec!["echo"]);
        let schemas = reg.schemas();
        assert_eq!(schemas[0]["function"]["name"], "echo");

        let ctx = ToolCtx::new(PathBuf::from("."), 1024);
        let tool = reg.get("echo").unwrap();
        let r = tool.call(&ctx, serde_json::json!({ "text": "hi" })).await;
        assert_eq!(r.content, "hi");
        assert!(!r.is_error);
    }

    #[test]
    fn truncate_marks_and_respects_char_boundary() {
        let r = ToolResult::ok("héllo world").truncate_to(2);
        assert!(r.truncated);
        // Byte 2 splits the 'é'; truncation backs off to a boundary.
        assert!(r.content.len() <= 2);
    }
}
