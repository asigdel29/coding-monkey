/*
   File: crates/engulf/src/scanner.rs

   Purpose
   Filesystem scan: stack, deps, API routes, file inventory. Emits a
   `ScanResult` consumed by every subsequent phase.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Result of scanning a repo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanResult {
    /// Total files seen (post-gitignore).
    pub file_count: usize,
    /// Files grouped by extension.
    pub files: Vec<FileInfo>,
    /// Detected env-var declarations.
    pub env_vars: Vec<EnvVar>,
    /// Detected API routes (Next.js, Express, FastAPI, etc.).
    pub routes: Vec<Route>,
    /// Detected security hints (used by the security phase).
    pub hints: Vec<SecurityHint>,
}

/// One file in the scan inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Path relative to repo root.
    pub relative_path: PathBuf,
    /// Lowercased extension including the dot, or empty.
    pub extension: String,
    /// Size in bytes.
    pub size_bytes: u64,
}

/// One environment-variable declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    /// Variable name.
    pub name: String,
    /// Whether it's documented in `.env.example` (or similar).
    pub has_example: bool,
}

/// One detected API route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// HTTP method, uppercased.
    pub method: String,
    /// Logical path.
    pub path: String,
    /// Source file containing the handler.
    pub file_path: PathBuf,
    /// 1-indexed line number.
    pub line_number: usize,
}

/// One actionable hint surfaced for the security phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityHint {
    /// Hint category (e.g. `"hardcoded-secret"`, `"sql-injection"`).
    pub category: String,
    /// Source file the hint came from.
    pub file_path: PathBuf,
    /// Free-text detail.
    pub detail: String,
}

/// Scan a repo. Currently returns an empty `ScanResult` — full impl is
/// being ported in subsequent commits.
pub fn scan(_root: &Path) -> std::io::Result<ScanResult> {
    Ok(ScanResult::default())
}
