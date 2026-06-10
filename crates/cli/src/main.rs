/*
   File: crates/cli/src/main.rs

   Purpose
   The `monkey` binary. Single executable, every subcommand routes to
   exactly one workspace crate. Like the TS port, we keep startup cheap
   by only initializing what each subcommand actually needs.

   Subcommands
       chat        REPL hand-off to the agent CLI (default)
       init        Scaffold .monkey/ in the project
       engulf      Deep-learn pipeline
       deck        Web frontend dashboard
       orchestrate Multi-repo coordinator
       skill       List or run a skill
       review      Multi-model pre-merge review
       investigate Four-phase root-cause debugging
       cso         Security audit
       ship        Full ship gauntlet
       pentest     Mandatory pre-push pentest gate
       compliance  SOC 2 status / verify / evidence
       doctor      Environment diagnostics
       models      List models with cost tiers

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; clap subcommand surface
*/

#![deny(unsafe_code)]

mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "monkey", version, about = "Unified AI agent platform — Rust", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    /// Free-text prompt for the default `chat` command.
    #[arg(global = true)]
    prompt: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Scaffold .monkey/ in the project.
    Init(commands::init::Args),
    /// Import existing agent prompts (CLAUDE.md, AGENTS.md, …) into .monkey/.
    Import(commands::import::Args),
    /// Deep-learn the codebase.
    Engulf(commands::engulf::Args),
    /// Web frontend dashboard.
    Deck(commands::deck::Args),
    /// Multi-repo orchestrator REPL.
    Orchestrate(commands::orchestrate::Args),
    /// List or run a skill.
    Skill(commands::skill::Args),
    /// Multi-model pre-merge review.
    Review(commands::review::Args),
    /// Four-phase root-cause debugging.
    Investigate(commands::investigate::Args),
    /// CSO security audit.
    Cso(commands::cso::Args),
    /// Full ship gauntlet.
    Ship(commands::ship::Args),
    /// Mandatory pre-push pentest gate.
    Pentest(commands::pentest::Args),
    /// SOC 2 audit-readiness pipeline.
    Compliance(commands::compliance::Args),
    /// Environment diagnostics.
    Doctor(commands::doctor::Args),
    /// List available AI models.
    Models,
    /// Interactive REPL (default).
    Chat(commands::chat::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Init(a)) => commands::init::run(a).await,
        Some(Cmd::Import(a)) => commands::import::run(a).await,
        Some(Cmd::Engulf(a)) => commands::engulf::run(a).await,
        Some(Cmd::Deck(a)) => commands::deck::run(a).await,
        Some(Cmd::Orchestrate(a)) => commands::orchestrate::run(a).await,
        Some(Cmd::Skill(a)) => commands::skill::run(a).await,
        Some(Cmd::Review(a)) => commands::review::run(a).await,
        Some(Cmd::Investigate(a)) => commands::investigate::run(a).await,
        Some(Cmd::Cso(a)) => commands::cso::run(a).await,
        Some(Cmd::Ship(a)) => commands::ship::run(a).await,
        Some(Cmd::Pentest(a)) => commands::pentest::run(a).await,
        Some(Cmd::Compliance(a)) => commands::compliance::run(a).await,
        Some(Cmd::Doctor(a)) => commands::doctor::run(a).await,
        Some(Cmd::Models) => commands::models::run().await,
        Some(Cmd::Chat(a)) => commands::chat::run(a, cli.prompt).await,
        None => commands::chat::run(commands::chat::Args::default(), cli.prompt).await,
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
}
