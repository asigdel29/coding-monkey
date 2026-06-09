/*
   File: crates/agents/src/context.rs

   Purpose
   Assemble the system prompt for a spawned agent from `.monkey/`.
   Read order is fixed and documented in the README; this is the
   authoritative implementation.

   Read order (all paths relative to `cwd`):
       .monkey/context/PROJECT.md
       .monkey/context/CONVENTIONS.md
       .monkey/context/GLOSSARY.md
       .monkey/context/{AGENT,CODEX}.md    (whichever matches kind)
       .monkey/tentacles/<tentacle>/CONTEXT.md
       .monkey/tentacles/<tentacle>/todo.md

   Total bundle is capped at MAX_TOTAL_BYTES. Per-file cap is
   MAX_FILE_BYTES — anything bigger is truncated mid-line and the file
   path is added to `AssembledContext.truncated_files`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/context.ts
   2026-06-03   Anubhav Sigdel  per-agent context file → AGENT.md; codex/auto
*/

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::types::AgentKind;

/// 32 KB total — chosen empirically: large enough for full project context
/// plus tentacle scope, small enough that even fast models stay under
/// 8k input tokens for the system prompt alone.
pub const MAX_TOTAL_BYTES: usize = 32 * 1024;

/// 16 KB per-file — prevents one runaway document from monopolizing the
/// budget. Truncation is reported via `truncated_files`.
pub const MAX_FILE_BYTES: usize = 16 * 1024;

/// Result of assembling the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    /// The fully-assembled prompt body, ready to feed to the agent CLI.
    pub prompt: String,
    /// Files that contributed (in inclusion order).
    pub files: Vec<PathBuf>,
    /// Files that were trimmed because they exceeded `MAX_FILE_BYTES` or
    /// the total budget would have been exceeded.
    pub truncated_files: Vec<PathBuf>,
    /// Final byte count of `prompt`.
    pub bytes: usize,
}

/// Assemble the system prompt. `cwd` is the project root; `tentacle_id`
/// is the active tentacle (typically `"main"`).
///
/// Returns an empty `AssembledContext` if `.monkey/` does not exist —
/// callers should treat that as a soft signal (the project hasn't been
/// initialized) and not as an error.
pub fn assemble_context(cwd: &Path, kind: AgentKind, tentacle_id: &str) -> AssembledContext {
    let mut prompt = String::new();
    let mut included = Vec::new();
    let mut truncated = Vec::new();

    let monkey = cwd.join(".monkey");
    if !monkey.is_dir() {
        return AssembledContext {
            prompt,
            files: included,
            truncated_files: truncated,
            bytes: 0,
        };
    }

    let context = monkey.join("context");
    // Each harness can carry its own guidance file (CODEX.md, CLAUDE.md,
    // HERMES.md); Auto uses the generic AGENT.md.
    let agent_file = kind.context_file();

    let candidates: Vec<PathBuf> = vec![
        context.join("PROJECT.md"),
        context.join("CONVENTIONS.md"),
        context.join("GLOSSARY.md"),
        context.join(agent_file),
        monkey
            .join("tentacles")
            .join(tentacle_id)
            .join("CONTEXT.md"),
        monkey.join("tentacles").join(tentacle_id).join("todo.md"),
    ];

    let mut total_bytes = 0usize;
    for path in candidates {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            // Missing files are not errors — they just contribute nothing.
            continue;
        };
        let (chunk, was_truncated) = trim_to_per_file_limit(&raw);
        let header = format!("\n\n=== {} ===\n", relative_or_absolute(cwd, &path));
        let projected = total_bytes + header.len() + chunk.len();
        if projected > MAX_TOTAL_BYTES {
            // Out of budget — record what we tried to include, but don't
            // partially-emit a fragment that would mislead the agent.
            truncated.push(path);
            continue;
        }
        prompt.push_str(&header);
        prompt.push_str(chunk);
        included.push(path.clone());
        if was_truncated {
            truncated.push(path);
        }
        total_bytes = projected;
    }

    AssembledContext {
        bytes: prompt.len(),
        prompt,
        files: included,
        truncated_files: truncated,
    }
}

fn trim_to_per_file_limit(raw: &str) -> (&str, bool) {
    if raw.len() <= MAX_FILE_BYTES {
        return (raw, false);
    }
    // Trim back to the most recent line break to keep the cut clean.
    let mut end = MAX_FILE_BYTES;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(pos) = raw[..end].rfind('\n') {
        end = pos;
    }
    (&raw[..end], true)
}

fn relative_or_absolute(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn touch(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn empty_when_no_monkey_dir() {
        let dir = tempdir().unwrap();
        let r = assemble_context(dir.path(), AgentKind::Auto, "main");
        assert!(r.prompt.is_empty());
        assert!(r.files.is_empty());
    }

    #[test]
    fn includes_present_files_only() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(&p.join(".monkey/context/PROJECT.md"), "project body");
        touch(&p.join(".monkey/context/AGENT.md"), "agent body");
        // Intentionally omit CONVENTIONS.md, GLOSSARY.md, and tentacle files.
        let r = assemble_context(p, AgentKind::Auto, "main");
        assert!(r.prompt.contains("project body"));
        assert!(r.prompt.contains("agent body"));
        assert_eq!(r.files.len(), 2);
        assert!(r.truncated_files.is_empty());
    }

    #[test]
    fn picks_codex_file_for_codex_kind() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        touch(&p.join(".monkey/context/AGENT.md"), "agent only");
        touch(&p.join(".monkey/context/CODEX.md"), "codex only");
        let r = assemble_context(p, AgentKind::Codex, "main");
        assert!(r.prompt.contains("codex only"));
        assert!(!r.prompt.contains("agent only"));
    }

    #[test]
    fn truncates_oversized_file() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        let big: String = "line\n".repeat(MAX_FILE_BYTES);
        touch(&p.join(".monkey/context/PROJECT.md"), &big);
        let r = assemble_context(p, AgentKind::Auto, "main");
        assert!(!r.truncated_files.is_empty());
        assert!(r.bytes <= MAX_TOTAL_BYTES);
    }
}
