/*
   File: crates/runtime/src/tools/finish.rs

   Purpose
   The terminal tool. When the model is done it calls `finish` with a
   summary; the agent loop intercepts the call by name and ends the run, so
   this `call` body is only a fallback (e.g. if a tool registry is used
   outside the loop). Advertising it as a tool gives the model an explicit,
   unambiguous way to stop instead of trailing off.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial
*/

use async_trait::async_trait;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// `finish` — signal the task is complete with a summary.
#[derive(Debug, Default, Clone, Copy)]
pub struct Finish;

#[async_trait]
impl Tool for Finish {
    fn name(&self) -> &str {
        "finish"
    }

    fn description(&self) -> &str {
        "Call when the task is complete. Provide a short summary of what was done."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "Short summary of the completed work." }
            },
            "required": ["summary"]
        })
    }

    async fn call(&self, _ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("done");
        ToolResult::ok(summary)
    }
}

/// Extract the `summary` argument from a `finish` tool call's raw JSON.
/// Falls back to a generic message when absent or unparseable.
pub fn parse_summary(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("summary").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_else(|| "task finished".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_reads_field_or_defaults() {
        assert_eq!(parse_summary(r#"{"summary":"shipped"}"#), "shipped");
        assert_eq!(parse_summary("not json"), "task finished");
        assert_eq!(parse_summary("{}"), "task finished");
    }
}
