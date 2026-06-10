/*
   File: crates/runtime/src/tools/run_command.rs

   Purpose
   The one tool that spawns an external process. Safety comes from three
   constraints: a program allowlist (the model may only run vetted tools
   like git/cargo/ls), argv execution with NO shell (so `;`, `|`, `$()` and
   friends are inert), and bounds on wall-clock time and captured output.
   The child is killed on timeout, cancellation, or drop.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — allowlisted, no-shell command tool
*/

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// Default per-command wall-clock limit.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Programs the agent may run unless the caller overrides the allowlist.
/// Read-only/build/VCS tools only — no shells, no network fetchers, no
/// destructive utilities.
pub fn default_allowlist() -> HashSet<String> {
    [
        "git", "cargo", "rustc", "ls", "cat", "echo", "pwd", "head", "tail", "wc", "grep", "find",
        "node", "npm", "pnpm", "yarn", "python3", "pytest", "go", "make",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// `run_command` — run an allowlisted program (no shell) in the agent's cwd.
#[derive(Debug, Clone)]
pub struct RunCommand {
    allowed: Arc<HashSet<String>>,
    timeout: Duration,
}

impl RunCommand {
    /// Build with an explicit allowlist and timeout.
    pub fn new(allowed: impl IntoIterator<Item = String>, timeout: Duration) -> Self {
        Self {
            allowed: Arc::new(allowed.into_iter().collect()),
            timeout,
        }
    }

    /// Build with the default allowlist and timeout.
    pub fn with_defaults() -> Self {
        Self {
            allowed: Arc::new(default_allowlist()),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Default for RunCommand {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run an allowlisted program (no shell) in the working directory and \
         return its combined output and exit code."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string", "description": "Program to run (must be on the allowlist)." },
                "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments (no shell interpretation)." }
            },
            "required": ["cmd"]
        })
    }

    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let Some(cmd) = args.get("cmd").and_then(|v| v.as_str()) else {
            return ToolResult::error("run_command: missing required string argument 'cmd'");
        };
        if !self.allowed.contains(cmd) {
            let mut allowed: Vec<&str> = self.allowed.iter().map(String::as_str).collect();
            allowed.sort_unstable();
            return ToolResult::error(format!(
                "run_command: '{cmd}' is not allowed. Allowed: {}",
                allowed.join(", ")
            ));
        }
        let argv: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut command = Command::new(cmd);
        command
            .args(&argv)
            .current_dir(&ctx.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = match command.spawn() {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("run_command: cannot start '{cmd}': {e}")),
        };

        tokio::select! {
            _ = ctx.cancel.cancelled() => {
                // Dropping `child` (kill_on_drop) terminates it.
                ToolResult::error("run_command: cancelled")
            }
            r = tokio::time::timeout(self.timeout, child.wait_with_output()) => match r {
                Err(_) => ToolResult::error(format!(
                    "run_command: '{cmd}' timed out after {}s", self.timeout.as_secs()
                )),
                Ok(Err(e)) => ToolResult::error(format!("run_command: '{cmd}' failed: {e}")),
                Ok(Ok(out)) => {
                    let mut body = String::new();
                    body.push_str(&String::from_utf8_lossy(&out.stdout));
                    if !out.stderr.is_empty() {
                        body.push_str("\n[stderr]\n");
                        body.push_str(&String::from_utf8_lossy(&out.stderr));
                    }
                    let code = out.status.code().unwrap_or(-1);
                    let result = ToolResult::ok(format!("exit {code}\n{body}"))
                        .truncate_to(ctx.output_budget);
                    if out.status.success() {
                        result
                    } else {
                        ToolResult { is_error: true, ..result }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_allowed_command() {
        let ctx = ToolCtx::new(std::env::temp_dir(), 4096);
        let r = RunCommand::with_defaults()
            .call(&ctx, serde_json::json!({ "cmd": "echo", "args": ["hi"] }))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("hi"));
        assert!(r.content.contains("exit 0"));
    }

    #[tokio::test]
    async fn rejects_disallowed_command() {
        let ctx = ToolCtx::new(std::env::temp_dir(), 4096);
        let r = RunCommand::with_defaults()
            .call(
                &ctx,
                serde_json::json!({ "cmd": "rm", "args": ["-rf", "/"] }),
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("not allowed"));
    }

    #[tokio::test]
    async fn enforces_timeout() {
        let ctx = ToolCtx::new(std::env::temp_dir(), 4096);
        let tool = RunCommand::new(["sleep".to_string()], Duration::from_millis(50));
        let r = tool
            .call(&ctx, serde_json::json!({ "cmd": "sleep", "args": ["5"] }))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("timed out"));
    }

    #[tokio::test]
    async fn honors_pre_cancellation() {
        let ctx = ToolCtx::new(std::env::temp_dir(), 4096);
        ctx.cancel.cancel();
        let tool = RunCommand::new(["sleep".to_string()], Duration::from_secs(30));
        let r = tool
            .call(&ctx, serde_json::json!({ "cmd": "sleep", "args": ["30"] }))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("cancelled"));
    }
}
