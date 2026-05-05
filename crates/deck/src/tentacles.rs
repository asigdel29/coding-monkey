/*
   File: crates/deck/src/tentacles.rs

   Purpose
   Read/write the .monkey/tentacles/<id>/ directory: CONTEXT.md and
   todo.md. The deck UI's right-rail editor reads this on load and
   writes it on every keystroke (debounced).

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Snapshot of a tentacle's persisted state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tentacle {
    /// Tentacle id (matches the directory name).
    pub id: String,
    /// CONTEXT.md body.
    pub context: String,
    /// todo.md body.
    pub todo: String,
}

/// Read a tentacle. Returns empty bodies if files don't exist.
pub fn read_tentacle(cwd: &Path, id: &str) -> std::io::Result<Tentacle> {
    let dir = cwd.join(".monkey").join("tentacles").join(id);
    let context = std::fs::read_to_string(dir.join("CONTEXT.md")).unwrap_or_default();
    let todo = std::fs::read_to_string(dir.join("todo.md")).unwrap_or_default();
    Ok(Tentacle { id: id.to_string(), context, todo })
}

/// Write a tentacle. Creates parent dirs.
pub fn write_tentacle(cwd: &Path, t: &Tentacle) -> std::io::Result<()> {
    let dir = cwd.join(".monkey").join("tentacles").join(&t.id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("CONTEXT.md"), &t.context)?;
    std::fs::write(dir.join("todo.md"), &t.todo)?;
    Ok(())
}
