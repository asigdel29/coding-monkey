/*
   File: crates/runtime/src/fs_guard.rs

   Purpose
   The filesystem jail that every native-agent file tool resolves paths
   through. An agent is scoped to a working directory (its tentacle or repo
   root); the guard turns a model-supplied path into an absolute path proven
   to sit inside that root, rejecting `..` escapes and absolute paths that
   point elsewhere. Resolution is lexical, so it works for paths that do not
   exist yet (needed by write_file) — it never touches the disk.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — lexical path jail
*/

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Why a path could not be resolved within the jail.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FsError {
    /// The path escaped the jail root.
    #[error("path escapes the agent's working directory: {0}")]
    Escape(String),
}

/// A working-directory jail for an agent's file tools.
#[derive(Debug, Clone)]
pub struct FsGuard {
    root: PathBuf,
}

impl FsGuard {
    /// Root the jail at `root`. The path is canonicalized when it exists so
    /// symlinks are collapsed; if it cannot be canonicalized (e.g. a test
    /// path), it is used as given.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        Self { root }
    }

    /// The jail root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a model-supplied path to an absolute path inside the jail.
    ///
    /// `input` is taken relative to the root unless it is absolute. The
    /// result is normalized lexically (no `.`/`..` left) and must remain
    /// under the root, otherwise [`FsError::Escape`] is returned. No disk
    /// access, so non-existent paths resolve fine.
    pub fn resolve(&self, input: &str) -> Result<PathBuf, FsError> {
        let candidate = if Path::new(input).is_absolute() {
            PathBuf::from(input)
        } else {
            self.root.join(input)
        };

        let mut out = PathBuf::new();
        for comp in candidate.components() {
            match comp {
                Component::Prefix(p) => out.push(p.as_os_str()),
                Component::RootDir => out.push(Component::RootDir.as_os_str()),
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(c) => out.push(c),
            }
        }

        if out.starts_with(&self.root) {
            Ok(out)
        } else {
            Err(FsError::Escape(input.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_within_root() {
        let g = FsGuard::rooted("/srv/repo");
        assert_eq!(
            g.resolve("src/main.rs").unwrap(),
            PathBuf::from("/srv/repo/src/main.rs")
        );
    }

    #[test]
    fn collapses_interior_parent_dirs() {
        let g = FsGuard::rooted("/srv/repo");
        // Stays inside the root, so it's allowed and normalized.
        assert_eq!(
            g.resolve("src/../README.md").unwrap(),
            PathBuf::from("/srv/repo/README.md")
        );
    }

    #[test]
    fn rejects_parent_escape() {
        let g = FsGuard::rooted("/srv/repo");
        assert_eq!(
            g.resolve("../secrets").unwrap_err(),
            FsError::Escape("../secrets".into())
        );
    }

    #[test]
    fn rejects_absolute_outside_root() {
        let g = FsGuard::rooted("/srv/repo");
        assert!(matches!(
            g.resolve("/etc/passwd"),
            Err(FsError::Escape(_))
        ));
    }
}
