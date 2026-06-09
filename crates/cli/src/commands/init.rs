/*
   File: crates/cli/src/commands/init.rs

   Purpose
   Scaffold .monkey/ in the project — context dirs, templates, default
   tentacle, config.json, optional engulf prompt.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-06-03   Anubhav Sigdel  scaffold AGENT.md; default_provider=openrouter
*/

use clap::Args as ClapArgs;
use std::path::PathBuf;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Target directory (default: cwd).
    pub path: Option<PathBuf>,
    /// Accept defaults; do not prompt.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Skip the engulf prompt entirely.
    #[arg(long = "no-engulf")]
    pub no_engulf: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let cwd = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let monkey = cwd.join(".monkey");
    std::fs::create_dir_all(monkey.join("context"))?;
    std::fs::create_dir_all(monkey.join("tentacles").join("main"))?;
    std::fs::create_dir_all(monkey.join("plans"))?;

    // Adopt any existing agent prompts (CLAUDE.md, AGENTS.md, …) before
    // writing templates, so a user's current setup is carried over and the
    // templates only fill the gaps.
    let imported = crate::commands::import::import_existing_prompts(&cwd, false)?;
    for (src, dest) in &imported.imported {
        eprintln!("imported {src} -> .monkey/context/{dest}");
    }

    write_if_missing(&monkey.join("config.json"), DEFAULT_CONFIG)?;
    write_if_missing(&monkey.join("context").join("PROJECT.md"), TEMPLATE_PROJECT)?;
    write_if_missing(
        &monkey.join("context").join("CONVENTIONS.md"),
        TEMPLATE_CONVENTIONS,
    )?;
    write_if_missing(&monkey.join("context").join("AGENT.md"), TEMPLATE_AGENT)?;
    write_if_missing(&monkey.join("context").join("CODEX.md"), TEMPLATE_CODEX)?;
    write_if_missing(
        &monkey.join("context").join("GLOSSARY.md"),
        TEMPLATE_GLOSSARY,
    )?;
    write_if_missing(
        &monkey.join("tentacles").join("main").join("CONTEXT.md"),
        "# main tentacle\n",
    )?;
    write_if_missing(
        &monkey.join("tentacles").join("main").join("todo.md"),
        "- [ ] first task\n",
    )?;

    eprintln!("scaffolded {} (.monkey/ written)", cwd.display());
    Ok(())
}

fn write_if_missing(p: &std::path::Path, body: &str) -> std::io::Result<()> {
    if p.exists() {
        return Ok(());
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(p, body)
}

const DEFAULT_CONFIG: &str = r#"{
  "default_agent": "auto",
  "default_provider": "openrouter",
  "default_tier": "balanced",
  "fail_on": "high"
}
"#;

const TEMPLATE_PROJECT: &str =
    "# Project\n\n_Stack and key files. Run `monkey engulf` to populate._\n";
const TEMPLATE_CONVENTIONS: &str = "# Conventions\n\n_How this team writes code. Hand-edited._\n";
const TEMPLATE_AGENT: &str =
    "# Agent guidance\n\n_Instructions shared by every agent. Hand-edited._\n";
const TEMPLATE_CODEX: &str = "# Codex-only guidance\n";
const TEMPLATE_GLOSSARY: &str = "# Glossary\n\n_Project terms._\n";
