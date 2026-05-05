/*
   File: crates/core/src/repos.rs

   Purpose
   Detect a repo's tech stack and complexity from its files. Used at
   session startup to populate `RepoConfig` and at engulf-scan time to
   route subsequent prompts.

   Detection is heuristic and intentionally conservative — when in doubt
   we report `TechStack::Unknown` rather than guess wrong.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from packages/core/src/repos/detector.ts
*/

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::types::RepoConfig;

/// Recognized tech stacks. Keep this list short — broad stacks only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TechStack {
    /// Rust (Cargo workspace or single crate).
    Rust,
    /// Node.js / TypeScript.
    Node,
    /// Python.
    Python,
    /// Go.
    Go,
    /// JVM (Java, Kotlin).
    Jvm,
    /// Ruby.
    Ruby,
    /// Could not determine.
    Unknown,
}

/// Heuristic complexity. Drives default model-tier selection — large or
/// polyglot repos get the Powerful tier by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoComplexity {
    /// Single small package, < 1k LoC.
    Small,
    /// Typical app or library, 1k–50k LoC.
    Medium,
    /// Monorepo or 50k+ LoC.
    Large,
}

/// Detect a single repo at `path`. Returns `Ok(None)` if `path` doesn't
/// look like a repo root (no manifest files at all).
pub fn detect_repo(path: &Path) -> std::io::Result<Option<RepoConfig>> {
    if !path.is_dir() {
        return Ok(None);
    }
    let stack = detect_stack(path);
    if stack == TechStack::Unknown && !has_any_manifest(path) {
        return Ok(None);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let complexity = estimate_complexity(path)?;
    Ok(Some(RepoConfig {
        name,
        path: PathBuf::from(path),
        tech_stack: stack,
        complexity,
        budget_override: None,
    }))
}

/// Discover repos under `root` (one level deep). Returns deterministic
/// order (sorted by name).
pub fn discover_repos(root: &Path) -> std::io::Result<Vec<RepoConfig>> {
    let mut out = Vec::new();
    if let Some(repo) = detect_repo(root)? {
        out.push(repo);
    }
    if root.is_dir() {
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if let Some(repo) = detect_repo(&p)? {
                if !out.iter().any(|r| r.path == repo.path) {
                    out.push(repo);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn has_any_manifest(path: &Path) -> bool {
    const FILES: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "build.gradle",
        "build.gradle.kts",
        "pom.xml",
        "Gemfile",
        ".git",
    ];
    FILES.iter().any(|f| path.join(f).exists())
}

fn detect_stack(path: &Path) -> TechStack {
    if path.join("Cargo.toml").exists() { return TechStack::Rust; }
    if path.join("package.json").exists() { return TechStack::Node; }
    if path.join("pyproject.toml").exists() || path.join("requirements.txt").exists() {
        return TechStack::Python;
    }
    if path.join("go.mod").exists() { return TechStack::Go; }
    if path.join("build.gradle").exists()
        || path.join("build.gradle.kts").exists()
        || path.join("pom.xml").exists()
    {
        return TechStack::Jvm;
    }
    if path.join("Gemfile").exists() { return TechStack::Ruby; }
    TechStack::Unknown
}

fn estimate_complexity(path: &Path) -> std::io::Result<RepoComplexity> {
    // Count files via the `ignore` crate so we honor .gitignore. Cap at
    // 50k entries — anything beyond that is "Large" by definition.
    let mut count = 0usize;
    let walker = ignore::WalkBuilder::new(path)
        .max_depth(Some(8))
        .build();
    for entry in walker.flatten() {
        if entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            count += 1;
            if count > 50_000 {
                return Ok(RepoComplexity::Large);
            }
        }
    }
    Ok(if count < 50 {
        RepoComplexity::Small
    } else if count < 5_000 {
        RepoComplexity::Medium
    } else {
        RepoComplexity::Large
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detect_rust_repo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let r = detect_repo(dir.path()).unwrap().unwrap();
        assert_eq!(r.tech_stack, TechStack::Rust);
    }

    #[test]
    fn detect_node_repo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        let r = detect_repo(dir.path()).unwrap().unwrap();
        assert_eq!(r.tech_stack, TechStack::Node);
    }

    #[test]
    fn unknown_when_empty() {
        let dir = tempdir().unwrap();
        let r = detect_repo(dir.path()).unwrap();
        assert!(r.is_none());
    }
}
