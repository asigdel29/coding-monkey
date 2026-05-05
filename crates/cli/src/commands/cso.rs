/*
   File: crates/cli/src/commands/cso.rs
   Purpose: thin wrapper around the `cso` skill.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use clap::Args as ClapArgs;
use monkey_skills::{create_default_registry, SkillContext};

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[arg(long, default_value = "daily")]
    pub mode: String,
    #[arg(long)]
    pub pentest_target: Option<String>,
    #[arg(long, default_value = "high")]
    pub fail_on: String,
    #[arg(long, default_value_t = false)]
    pub skip_llm: bool,
    #[arg(long, default_value_t = false)]
    pub persist: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let r = create_default_registry()
        .run(
            "cso",
            serde_json::json!({
                "mode": args.mode,
                "pentestTarget": args.pentest_target,
                "failOn": args.fail_on,
                "skipLLM": args.skip_llm,
            }),
            &SkillContext {
                cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
                base_branch: None,
                persist_reports: args.persist,
                ..Default::default()
            },
        )
        .await?;
    if let Some(md) = r.markdown { println!("{md}"); }
    std::process::exit(if r.ok { 0 } else { 1 });
}
