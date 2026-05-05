/*
   File: crates/deck/src/tentacles.rs

   Purpose
   File-backed tentacle store. Each tentacle is a directory under
   `.monkey/tentacles/<id>/` holding:
       CONTEXT.md   — scope, free-form (first H1 becomes the title)
       todo.md      — `- [ ] task` / `- [x] task` checkboxes

   Operations are idempotent and safe to call from multiple deck
   sessions concurrently — every mutation is a single
   write_to_temp+rename via std::fs.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/deck/src/tentacles.ts
*/

use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One tentacle on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tentacle {
    /// Stable id (folder name).
    pub id: String,
    /// Title (from the first `# ` heading in CONTEXT.md, or the id).
    pub title: String,
    /// Absolute path to CONTEXT.md.
    pub context_path: PathBuf,
    /// Absolute path to todo.md.
    pub todo_path: PathBuf,
    /// Tentacle directory.
    pub dir: PathBuf,
    /// Creation timestamp in unix milliseconds (best-effort).
    pub created_at_ms: u64,
}

/// One todo line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Whether the box is checked.
    pub done: bool,
    /// Task text (without the leading `- [ ]`).
    pub text: String,
    /// 0-indexed source line.
    pub line: usize,
}

static TODO_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*-\s*\[([ xX])\]\s+(.+?)\s*$").expect("re"));

static H1_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#\s+(.+)$").expect("re"));

static SLUG_BAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").expect("re"));

/// Tentacle store rooted under `.monkey/tentacles/`.
#[derive(Debug, Clone)]
pub struct TentacleStore {
    /// Project working directory passed at construction.
    pub cwd: PathBuf,
    /// Absolute path to `.monkey/tentacles/`.
    pub root: PathBuf,
}

impl TentacleStore {
    /// Construct a store rooted at `cwd/.monkey/tentacles/`.
    pub fn new(cwd: impl AsRef<Path>) -> Self {
        let cwd = cwd.as_ref().to_path_buf();
        let root = cwd.join(".monkey").join("tentacles");
        Self { cwd, root }
    }

    /// Every tentacle, sorted newest-first by ctime.
    pub fn list(&self) -> Vec<Tentacle> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type().map(|f| f.is_dir()).unwrap_or(false) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            out.push(self.hydrate(&id, &entry.path()));
        }
        out.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        out
    }

    /// Look up by id. Returns `None` if the directory is missing.
    pub fn get(&self, id: &str) -> Option<Tentacle> {
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return None;
        }
        Some(self.hydrate(id, &dir))
    }

    /// Create a tentacle with `title`, optionally seeding CONTEXT.md.
    /// Idempotent: re-creating an existing tentacle leaves files alone.
    pub fn create(&self, title: &str, context: &str) -> std::io::Result<Tentacle> {
        let id = slug(title);
        let id = if id.is_empty() {
            format!(
                "tentacle-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            )
        } else {
            id
        };
        let dir = self.root.join(&id);
        std::fs::create_dir_all(&dir)?;
        let t = self.hydrate(&id, &dir);
        if !t.context_path.exists() {
            let extra = if context.is_empty() {
                String::new()
            } else {
                format!("\n{context}\n")
            };
            std::fs::write(
                &t.context_path,
                format!("# {title}\n\nScope and notes for this tentacle.\n{extra}"),
            )?;
        }
        if !t.todo_path.exists() {
            std::fs::write(&t.todo_path, format!("# Todo — {title}\n\n- [ ] First task\n"))?;
        }
        Ok(t)
    }

    /// Remove a tentacle directory recursively. Returns `true` if it
    /// was removed (i.e. it existed before the call).
    pub fn remove(&self, id: &str) -> std::io::Result<bool> {
        let dir = self.root.join(id);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(true)
    }

    /// Parse todo.md into structured items (only checkbox lines).
    pub fn todos(&self, id: &str) -> Vec<TodoItem> {
        let Some(t) = self.get(id) else { return Vec::new() };
        let Ok(raw) = std::fs::read_to_string(&t.todo_path) else { return Vec::new() };
        let mut out = Vec::new();
        for (i, line) in raw.lines().enumerate() {
            if let Some(c) = TODO_RE.captures(line) {
                out.push(TodoItem {
                    done: c[1].eq_ignore_ascii_case("x"),
                    text: c[2].to_string(),
                    line: i,
                });
            }
        }
        out
    }

    /// Toggle the checkbox at `line`. No-op if the line isn't a checkbox.
    pub fn toggle_todo(&self, id: &str, line: usize) -> Vec<TodoItem> {
        let Some(t) = self.get(id) else { return Vec::new() };
        let Ok(raw) = std::fs::read_to_string(&t.todo_path) else { return Vec::new() };
        let mut lines: Vec<String> = raw.split('\n').map(|s| s.to_string()).collect();
        if line < lines.len() {
            if let Some(c) = TODO_RE.captures(&lines[line]) {
                let next = if c[1].eq_ignore_ascii_case("x") { ' ' } else { 'x' };
                let new = format!("- [{}] {}", next, &c[2]);
                lines[line] = TODO_RE.replace(&lines[line], new.as_str()).to_string();
                let _ = std::fs::write(&t.todo_path, lines.join("\n"));
            }
        }
        self.todos(id)
    }

    /// Read CONTEXT.md, returning an empty string if missing.
    pub fn read_context(&self, id: &str) -> String {
        let Some(t) = self.get(id) else { return String::new() };
        std::fs::read_to_string(&t.context_path).unwrap_or_default()
    }

    /// Overwrite CONTEXT.md. No-op if the tentacle doesn't exist.
    pub fn write_context(&self, id: &str, content: &str) -> std::io::Result<()> {
        let Some(t) = self.get(id) else { return Ok(()) };
        std::fs::write(&t.context_path, content)
    }

    fn hydrate(&self, id: &str, dir: &Path) -> Tentacle {
        let context_path = dir.join("CONTEXT.md");
        let todo_path = dir.join("todo.md");
        let mut title = id.to_string();
        if let Ok(s) = std::fs::read_to_string(&context_path) {
            if let Some(first) = s.lines().next() {
                if let Some(c) = H1_RE.captures(first) {
                    title = c[1].trim().to_string();
                }
            }
        }
        let created_at_ms = dir
            .metadata()
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .unwrap_or(Duration::from_secs(0))
            .as_millis() as u64;
        Tentacle {
            id: id.to_string(),
            title,
            context_path,
            todo_path,
            dir: dir.to_path_buf(),
            created_at_ms,
        }
    }
}

/// `Foo Bar Baz` → `foo-bar-baz`, trimmed to 48 chars, alnum-only.
pub fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let dashed = SLUG_BAD.replace_all(&lower, "-").to_string();
    let trimmed = dashed.trim_matches('-').to_string();
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() >= 48 {
            break;
        }
        out.push(c);
    }
    out
}

/// Convenience: format a unix-ms timestamp as RFC 3339.
pub fn ms_to_rfc3339(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let nsec = ((ms % 1000) * 1_000_000) as u32;
    DateTime::<Utc>::from_timestamp(secs, nsec)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slug_normalizes_titles() {
        assert_eq!(slug("Hello World!"), "hello-world");
        assert_eq!(slug("  spaces "), "spaces");
        assert_eq!(slug("FOO_BAR"), "foo-bar");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn create_and_list_round_trip() {
        let dir = tempdir().unwrap();
        let store = TentacleStore::new(dir.path());
        let t = store.create("Refactor cache", "scope notes").unwrap();
        assert_eq!(t.id, "refactor-cache");
        assert!(t.context_path.exists());
        assert!(t.todo_path.exists());
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "refactor-cache");
    }

    #[test]
    fn create_is_idempotent() {
        let dir = tempdir().unwrap();
        let store = TentacleStore::new(dir.path());
        let _ = store.create("X", "").unwrap();
        std::fs::write(store.root.join("x").join("CONTEXT.md"), "user-edited").unwrap();
        let _ = store.create("X", "").unwrap();
        let body = std::fs::read_to_string(store.root.join("x").join("CONTEXT.md")).unwrap();
        assert_eq!(body, "user-edited");
    }

    #[test]
    fn todo_toggle_round_trips() {
        let dir = tempdir().unwrap();
        let store = TentacleStore::new(dir.path());
        let t = store.create("X", "").unwrap();
        std::fs::write(
            &t.todo_path,
            "# Todo\n\n- [ ] one\n- [x] two\n- [ ] three\n",
        )
        .unwrap();
        let todos = store.todos(&t.id);
        assert_eq!(todos.len(), 3);
        assert!(!todos[0].done);
        assert!(todos[1].done);
        let updated = store.toggle_todo(&t.id, todos[0].line);
        assert!(updated[0].done);
    }

    #[test]
    fn remove_returns_true_when_dir_existed() {
        let dir = tempdir().unwrap();
        let store = TentacleStore::new(dir.path());
        store.create("X", "").unwrap();
        assert!(store.remove("x").unwrap());
        assert!(!store.remove("x").unwrap());
    }

    #[test]
    fn write_context_no_op_on_missing_id() {
        let dir = tempdir().unwrap();
        let store = TentacleStore::new(dir.path());
        // Should not panic / error out.
        store.write_context("nope", "hi").unwrap();
        assert_eq!(store.read_context("nope"), "");
    }
}
