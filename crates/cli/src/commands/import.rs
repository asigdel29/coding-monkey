/*
   File: crates/cli/src/commands/import.rs

   Purpose
   Bring a user's existing agent setup into `.monkey/`. Many projects
   already carry agent-instruction files (CLAUDE.md, AGENTS.md, GEMINI.md,
   …); rather than make the user re-author them, `monkey import` copies them
   into `.monkey/context/` under the names the context assembler reads, so a
   user's current prompts "just work" with the native engine and every
   harness. `monkey init` calls the same routine so a fresh scaffold adopts
   whatever is already there.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — import existing agent prompts
*/

use std::path::Path;

use clap::Args as ClapArgs;

/// Known source prompt files mapped to the `.monkey/context/` file the
/// assembler loads. First match wins per destination.
const MAPPINGS: &[(&str, &str)] = &[
    ("CLAUDE.md", "CLAUDE.md"),
    ("HERMES.md", "HERMES.md"),
    ("CODEX.md", "CODEX.md"),
    ("AGENTS.md", "AGENT.md"),
    ("AGENT.md", "AGENT.md"),
    ("GEMINI.md", "AGENT.md"),
    (".github/copilot-instructions.md", "AGENT.md"),
];

/// What an import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// `(source, destination)` pairs copied.
    pub imported: Vec<(String, String)>,
    /// Sources found but skipped because the destination already existed.
    pub skipped: Vec<String>,
}

/// Copy recognized agent-prompt files from `root` into
/// `root/.monkey/context/`. Existing destinations are left intact unless
/// `force` is set. Copied files get a provenance header.
pub fn import_existing_prompts(root: &Path, force: bool) -> std::io::Result<ImportSummary> {
    let ctx = root.join(".monkey").join("context");
    std::fs::create_dir_all(&ctx)?;
    let mut summary = ImportSummary::default();
    let mut written: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for (src, dest) in MAPPINGS {
        let src_path = root.join(src);
        if !src_path.is_file() {
            continue;
        }
        let dest_path = ctx.join(dest);
        if (dest_path.exists() && !force) || written.contains(dest) {
            summary.skipped.push((*src).to_string());
            continue;
        }
        let body = std::fs::read_to_string(&src_path)?;
        std::fs::write(&dest_path, format!("<!-- imported from {src} -->\n\n{body}"))?;
        summary.imported.push(((*src).to_string(), (*dest).to_string()));
        written.insert(dest);
    }
    Ok(summary)
}

/// `monkey import` arguments.
#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Project directory (default: cwd).
    pub path: Option<std::path::PathBuf>,
    /// Overwrite existing `.monkey/context/` files.
    #[arg(short = 'f', long)]
    pub force: bool,
}

/// Run the standalone import command.
pub async fn run(args: Args) -> anyhow::Result<()> {
    let root = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let summary = import_existing_prompts(&root, args.force)?;
    if summary.imported.is_empty() && summary.skipped.is_empty() {
        eprintln!("no known agent prompt files found to import");
    } else {
        for (src, dest) in &summary.imported {
            eprintln!("imported {src} -> .monkey/context/{dest}");
        }
        for src in &summary.skipped {
            eprintln!("skipped {src} (destination exists; use --force to overwrite)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_known_files_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "be terse").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "use tools").unwrap();

        let s = import_existing_prompts(dir.path(), false).unwrap();
        assert!(s.imported.contains(&("CLAUDE.md".into(), "CLAUDE.md".into())));
        assert!(s.imported.contains(&("AGENTS.md".into(), "AGENT.md".into())));

        let claude = std::fs::read_to_string(
            dir.path().join(".monkey/context/CLAUDE.md"),
        )
        .unwrap();
        assert!(claude.contains("imported from CLAUDE.md"));
        assert!(claude.contains("be terse"));
    }

    #[test]
    fn skips_existing_destination_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".monkey/context")).unwrap();
        std::fs::write(dir.path().join(".monkey/context/CLAUDE.md"), "keep me").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "new").unwrap();

        let s = import_existing_prompts(dir.path(), false).unwrap();
        assert_eq!(s.skipped, vec!["CLAUDE.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".monkey/context/CLAUDE.md")).unwrap(),
            "keep me"
        );

        let s = import_existing_prompts(dir.path(), true).unwrap();
        assert!(s.imported.iter().any(|(src, _)| src == "CLAUDE.md"));
        assert!(std::fs::read_to_string(dir.path().join(".monkey/context/CLAUDE.md"))
            .unwrap()
            .contains("new"));
    }

    #[test]
    fn no_files_is_empty_summary() {
        let dir = tempfile::tempdir().unwrap();
        let s = import_existing_prompts(dir.path(), false).unwrap();
        assert_eq!(s, ImportSummary::default());
    }
}
