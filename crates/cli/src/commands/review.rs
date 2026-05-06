/*
   File: crates/cli/src/commands/review.rs
   Purpose: thin wrapper around the `review` skill.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use clap::Args as ClapArgs;
use monkey_skills::{create_default_registry, SkillContext};

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[arg(long)]
    pub base: Option<String>,
    #[arg(long, default_value_t = false)]
    pub second_opinion: bool,
    #[arg(long, default_value = "high")]
    pub fail_on: String,
    #[arg(long, default_value_t = false)]
    pub persist: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let r = create_default_registry()
        .run(
            "review",
            serde_json::json!({
                "base": args.base,
                "secondOpinion": args.second_opinion,
                "failOn": args.fail_on,
            }),
            &SkillContext {
                cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
                base_branch: args.base.clone(),
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
