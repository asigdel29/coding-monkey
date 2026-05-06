/*
   File: crates/cli/src/commands/ship.rs
   Purpose: thin wrapper around the `ship` skill.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use clap::Args as ClapArgs;
use monkey_skills::{create_default_registry, SkillContext};

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[arg(long, default_value = "patch")]
    pub bump: String,
    #[arg(long = "no-build", default_value_t = false)]
    pub no_build: bool,
    #[arg(long = "no-review", default_value_t = false)]
    pub no_review: bool,
    #[arg(long = "no-audit", default_value_t = false)]
    pub no_audit: bool,
    #[arg(long = "no-pentest", default_value_t = false)]
    pub no_pentest: bool,
    #[arg(long = "no-push", default_value_t = false)]
    pub no_push: bool,
    #[arg(long, default_value_t = false)]
    pub pr: bool,
    #[arg(long)]
    pub pentest_target: Option<String>,
    #[arg(long, default_value = "high")]
    pub fail_on: String,
    #[arg(long, default_value_t = false)]
    pub persist: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let r = create_default_registry()
        .run(
            "ship",
            serde_json::json!({
                "bump": args.bump,
                "skipBuild": args.no_build,
                "review": !args.no_review,
                "audit": !args.no_audit,
                "pentest": !args.no_pentest,
                "push": !args.no_push,
                "openPr": args.pr,
                "pentestTarget": args.pentest_target,
                "failOn": args.fail_on,
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
