/*
   File: crates/runtime/src/tools/list_dir.rs

   Purpose
   List the immediate entries of a directory inside the agent's working
   directory. Directories are suffixed with `/`, entries are sorted, and the
   version-control metadata directory (`.git`) is hidden. Output is capped to
   the context budget.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial
*/

use async_trait::async_trait;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// `list_dir` — list immediate entries of a directory under the agent's cwd.
#[derive(Debug, Default, Clone, Copy)]
pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the immediate entries of a directory within the working directory. \
         Directories end with '/'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path relative to the working directory (defaults to '.')." }
            }
        })
    }

    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let resolved = match ctx.fs.resolve(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("list_dir: {e}")),
        };
        let mut rd = match tokio::fs::read_dir(&resolved).await {
            Ok(rd) => rd,
            Err(e) => return ToolResult::error(format!("list_dir: cannot read '{path}': {e}")),
        };
        let mut entries: Vec<String> = Vec::new();
        loop {
            match rd.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == ".git" {
                        continue;
                    }
                    let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                    entries.push(if is_dir { format!("{name}/") } else { name });
                }
                Ok(None) => break,
                Err(e) => return ToolResult::error(format!("list_dir: {e}")),
            }
        }
        entries.sort();
        ToolResult::ok(entries.join("\n")).truncate_to(ctx.output_budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_sorted_with_dir_suffix_and_hides_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 1024);
        let r = ListDir.call(&ctx, serde_json::json!({})).await;
        assert!(!r.is_error);
        assert_eq!(r.content, "a_dir/\nb.txt");
    }

    #[tokio::test]
    async fn errors_on_escape() {
        let ctx = ToolCtx::new(std::path::PathBuf::from("."), 1024);
        let r = ListDir
            .call(&ctx, serde_json::json!({ "path": "../.." }))
            .await;
        assert!(r.is_error);
    }
}
