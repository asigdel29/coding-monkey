/*
   File: crates/cli/src/commands/skill.rs
   Purpose: list / run skills via the registry.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use clap::{Args as ClapArgs, Subcommand};
use monkey_skills::{create_default_registry, SkillContext};
use std::path::PathBuf;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
    /// List all skills.
    List,
    /// Run a skill by name.
    Run {
        /// Skill name.
        name: String,
        /// Working directory.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Base branch override.
        #[arg(long)]
        base: Option<String>,
        /// Persist report under .monkey/skills/<name>/.
        #[arg(long, default_value_t = false)]
        persist: bool,
        /// JSON-encoded input.
        #[arg(long)]
        input: Option<String>,
    },
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let registry = create_default_registry();
    match args.action {
        Action::List => {
            for s in registry.list() {
                println!("  {:14}  {}", s.name(), s.description());
            }
        }
        Action::Run { name, cwd, base, persist, input } => {
            let ctx = SkillContext {
                cwd: cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into())),
                base_branch: base,
                persist_reports: persist,
            };
            let payload: serde_json::Value = match input {
                Some(s) => serde_json::from_str(&s)?,
                None => serde_json::json!({}),
            };
            let r = registry.run(&name, payload, &ctx).await?;
            if let Some(md) = r.markdown { println!("{md}"); } else { println!("{}", r.summary); }
            std::process::exit(if r.ok { 0 } else { 1 });
        }
    }
    Ok(())
}
