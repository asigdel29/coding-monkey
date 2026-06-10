/*
   File: crates/runtime/src/tools/search.rs

   Purpose
   Regex search across the agent's working directory, in-process. Uses the
   `ignore` crate so `.gitignore` and hidden files are respected (and big
   vendored trees skipped), and `regex` for matching — no dependency on an
   external `rg` binary, which may be absent on a Raspberry Pi and would
   reintroduce the process-spawn overhead the native engine avoids.

   The walk is synchronous and CPU/IO-bound, so it runs on a blocking thread
   rather than stalling the async runtime that hosts 100+ agents.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial
*/

use std::path::PathBuf;

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;

use crate::tool::{Tool, ToolCtx, ToolResult};

/// Files larger than this are skipped during search.
const MAX_FILE_BYTES: u64 = 512_000;

/// Default cap on reported matches.
const DEFAULT_MAX_MATCHES: usize = 200;

/// `search` — find lines matching a regex under the agent's cwd.
#[derive(Debug, Default, Clone, Copy)]
pub struct Search;

#[async_trait]
impl Tool for Search {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search files under the working directory for lines matching a regex \
         (respects .gitignore). Returns 'path:line: text' matches."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression to match per line." },
                "path": { "type": "string", "description": "Subdirectory to search (defaults to '.')." },
                "max_matches": { "type": "integer", "description": "Cap on matches returned." }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult {
        let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) else {
            return ToolResult::error("search: missing required string argument 'pattern'");
        };
        let re = match Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => return ToolResult::error(format!("search: invalid regex: {e}")),
        };
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = match ctx.fs.resolve(path) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("search: {e}")),
        };
        let max_matches = args
            .get("max_matches")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_MATCHES);
        let display_root = root.clone();

        let lines =
            tokio::task::spawn_blocking(move || run_walk(&root, &re, max_matches, &display_root))
                .await
                .unwrap_or_else(|e| vec![format!("search: worker failed: {e}")]);

        if lines.is_empty() {
            return ToolResult::ok("no matches");
        }
        ToolResult::ok(lines.join("\n")).truncate_to(ctx.output_budget)
    }
}

/// Walk `root`, collecting up to `max_matches` `rel:line: text` hits. Pure
/// of async; runs on a blocking thread.
fn run_walk(root: &PathBuf, re: &Regex, max_matches: usize, display_root: &PathBuf) -> Vec<String> {
    let mut out = Vec::new();
    for result in WalkBuilder::new(root).build() {
        if out.len() >= max_matches {
            break;
        }
        let Ok(entry) = result else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if entry
            .metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue; // unreadable or non-UTF-8 (binary) — skip
        };
        let rel = entry
            .path()
            .strip_prefix(display_root)
            .unwrap_or(entry.path())
            .display();
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                out.push(format!("{rel}:{}: {}", i + 1, line.trim_end()));
                if out.len() >= max_matches {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_matches_and_reports_location() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\nlet x = TODO;\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "clean\n").unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 4096);
        let r = Search
            .call(&ctx, serde_json::json!({ "pattern": "TODO" }))
            .await;
        assert!(!r.is_error);
        assert!(r.content.contains("a.rs:2:"), "got: {}", r.content);
        assert!(!r.content.contains("b.rs"));
    }

    #[tokio::test]
    async fn reports_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "nothing here\n").unwrap();
        let ctx = ToolCtx::new(dir.path().to_path_buf(), 4096);
        let r = Search
            .call(&ctx, serde_json::json!({ "pattern": "zzz" }))
            .await;
        assert_eq!(r.content, "no matches");
    }

    #[tokio::test]
    async fn invalid_regex_is_a_tool_error() {
        let ctx = ToolCtx::new(std::path::PathBuf::from("."), 1024);
        let r = Search
            .call(&ctx, serde_json::json!({ "pattern": "(" }))
            .await;
        assert!(r.is_error);
    }
}
