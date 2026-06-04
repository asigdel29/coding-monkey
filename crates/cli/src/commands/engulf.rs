/*
   File: crates/cli/src/commands/engulf.rs
   Purpose: dispatch to monkey-engulf::run_engulf.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-06-03   Anubhav Sigdel  provider default openrouter|openai
*/

use clap::Args as ClapArgs;
use monkey_engulf::{run_engulf, EngulfConfig, Provider};
use std::path::PathBuf;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    pub path: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub auto: bool,
    #[arg(long, default_value = "scan,security,docs,vault,deploy")]
    pub phases: String,
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long, default_value = "openrouter")]
    pub provider: String,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let provider = match args.provider.as_str() {
        "openai" => Provider::Openai,
        _ => Provider::OpenRouter,
    };
    let config = EngulfConfig {
        target_path: args
            .path
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into())),
        output_path: args.output,
        phases: vec![],
        provider,
        auto_run: args.auto,
    };
    let summary = run_engulf(config).await?;
    eprintln!(
        "engulf complete: {} files written",
        summary.files_written.len()
    );
    Ok(())
}
