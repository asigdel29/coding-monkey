/*
   File: crates/cli/src/commands/investigate.rs
   Purpose: thin wrapper around the `investigate` skill.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use clap::Args as ClapArgs;
use monkey_skills::{create_default_registry, SkillContext};

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Free-text symptom description.
    pub symptom: String,
    #[arg(long)]
    pub hint: Vec<String>,
    #[arg(long, default_value_t = 2)]
    pub max_escalations: u8,
    #[arg(long, default_value_t = false)]
    pub persist: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let r = create_default_registry()
        .run(
            "investigate",
            serde_json::json!({
                "symptom": args.symptom,
                "hints": args.hint,
                "maxEscalations": args.max_escalations,
            }),
            &SkillContext {
                cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
                base_branch: None,
                persist_reports: args.persist,
                ..Default::default()
            },
        )
        .await?;
    if let Some(md) = r.markdown {
        println!("{md}");
    }
    std::process::exit(if r.ok { 0 } else { 1 });
}
