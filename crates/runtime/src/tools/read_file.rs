/*
   File: crates/runtime/src/tools/read_file.rs

   Purpose
   Read a UTF-8 text file from inside the agent's working directory. The
   path is resolved through the jail, the content is capped to the smaller
   of an optional `max_bytes` argument and the context output budget, and
   truncation is reported back to the model so it knows to narrow its read
   rather than assume it saw the whole file.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial
*/

use async_trait::async_trait;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// `read_file` — return the contents of a file under the agent's cwd.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file within the working directory. Output is truncated \
         to the byte budget; narrow with max_bytes for large files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the working directory." },
                "max_bytes": { "type": "integer", "description": "Optional cap on bytes returned." }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error("read_file: missing required string argument 'path'");
        };
        let resolved = match ctx.fs.resolve(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("read_file: {e}")),
        };
        match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => {
                let budget = args
                    .get("max_bytes")
                    .and_then(|v| v.as_u64())
                    .map(|n| (n as usize).min(ctx.output_budget))
                    .unwrap_or(ctx.output_budget);
                ToolResult::ok(content).truncate_to(budget)
            }
            Err(e) => ToolResult::error(format!("read_file: cannot read '{path}': {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn reads_a_file_within_jail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi there").unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 1024);
        let r = ReadFile
            .call(&ctx, serde_json::json!({ "path": "hello.txt" }))
            .await;
        assert!(!r.is_error);
        assert_eq!(r.content, "hi there");
    }

    #[tokio::test]
    async fn refuses_escape() {
        let ctx = ToolCtx::new(PathBuf::from("."), 1024);
        let r = ReadFile
            .call(&ctx, serde_json::json!({ "path": "../../etc/passwd" }))
            .await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn truncates_to_budget() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), "0123456789").unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 4);
        let r = ReadFile
            .call(&ctx, serde_json::json!({ "path": "big.txt" }))
            .await;
        assert!(r.truncated);
        assert_eq!(r.content, "0123");
    }
}
