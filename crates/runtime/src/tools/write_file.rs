/*
   File: crates/runtime/src/tools/write_file.rs

   Purpose
   Write a UTF-8 text file inside the agent's working directory. The write
   is atomic — content goes to a sibling temp file which is renamed over the
   target, so a reader never sees a half-written file — and serialized by
   the jail's per-path write lock so two writes to the same path can't race.
   Missing parent directories are created within the jail.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial
*/

use std::ffi::OsString;

use async_trait::async_trait;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// `write_file` — atomically write text to a file under the agent's cwd.
#[derive(Debug, Default, Clone, Copy)]
pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Atomically write a UTF-8 text file within the working directory, \
         creating parent directories as needed. Overwrites existing files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to the working directory." },
                "content": { "type": "string", "description": "Full file contents to write." }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
            return ToolResult::error("write_file: missing required string argument 'path'");
        };
        let Some(content) = args.get("content").and_then(|v| v.as_str()) else {
            return ToolResult::error("write_file: missing required string argument 'content'");
        };
        let resolved = match ctx.fs.resolve(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("write_file: {e}")),
        };

        let Some(_lock) = ctx.fs.try_lock_write(&resolved) else {
            return ToolResult::error(format!(
                "write_file: another write to '{path}' is in progress"
            ));
        };

        if let Some(parent) = resolved.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return ToolResult::error(format!(
                    "write_file: cannot create '{path}' parents: {e}"
                ));
            }
        }

        // Temp sibling + rename = atomic replace. The per-path write lock
        // makes a deterministic temp name safe (no concurrent writer).
        let mut tmp_name = resolved.clone().into_os_string();
        tmp_name.push(OsString::from(".monkey-tmp"));
        let tmp = std::path::PathBuf::from(tmp_name);

        if let Err(e) = tokio::fs::write(&tmp, content.as_bytes()).await {
            return ToolResult::error(format!("write_file: cannot write '{path}': {e}"));
        }
        if let Err(e) = tokio::fs::rename(&tmp, &resolved).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return ToolResult::error(format!("write_file: cannot finalize '{path}': {e}"));
        }
        ToolResult::ok(format!("wrote {} bytes to {path}", content.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 1024);
        let r = WriteFile
            .call(
                &ctx,
                serde_json::json!({ "path": "nested/dir/out.txt", "content": "hello" }),
            )
            .await;
        assert!(!r.is_error, "{}", r.content);
        let got = std::fs::read_to_string(dir.path().join("nested/dir/out.txt")).unwrap();
        assert_eq!(got, "hello");
    }

    #[tokio::test]
    async fn overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "old").unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 1024);
        let _ = WriteFile
            .call(
                &ctx,
                serde_json::json!({ "path": "f.txt", "content": "new" }),
            )
            .await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "new"
        );
        // No temp file is left behind.
        assert!(!dir.path().join("f.txt.monkey-tmp").exists());
    }

    #[tokio::test]
    async fn refuses_escape() {
        let ctx = ToolCtx::new(std::path::PathBuf::from("."), 1024);
        let r = WriteFile
            .call(
                &ctx,
                serde_json::json!({ "path": "../evil.txt", "content": "x" }),
            )
            .await;
        assert!(r.is_error);
    }
}
