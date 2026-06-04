/*
   File: crates/cli/src/commands/chat.rs

   Purpose
   Default subcommand: REPL hand-off to the agent CLI. Resolves Auto via
   doctor, assembles context, spawns the PTY, and pipes host stdin/stdout.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial port from cli/src/index.ts (chat command)
   2026-06-03   Anubhav Sigdel  codex/auto agents only
*/

use clap::Args as ClapArgs;
use monkey_agents::{spawn_agent, AgentKind, SpawnOpts};
use std::path::PathBuf;

#[derive(Debug, Clone, ClapArgs, Default)]
pub struct Args {
    /// Agent to use: codex | auto.
    #[arg(long, default_value = "auto")]
    pub agent: String,
    /// Tentacle scope.
    #[arg(long, default_value = "main")]
    pub tentacle: String,
    /// Project directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}

pub async fn run(args: Args, prompt: Option<String>) -> anyhow::Result<()> {
    let kind = match args.agent.as_str() {
        "codex" => AgentKind::Codex,
        _ => AgentKind::Auto,
    };
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let opts = SpawnOpts {
        kind,
        cwd,
        tentacle_id: args.tentacle,
        ..SpawnOpts::default()
    };
    use std::io::Write as _;
    let result = spawn_agent(opts, |chunk| {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(chunk);
        let _ = stdout.flush();
    })?;
    eprintln!(
        "agent: {:?}  binary: {}  context: {} files, {:.1} KB",
        result.kind,
        result.binary,
        result.context.files.len(),
        result.context.bytes as f64 / 1024.0,
    );
    if let Some(p) = prompt {
        result.terminal.write(format!("{p}\n").as_bytes())?;
    }
    let code = result.terminal.wait()?;
    std::process::exit(code);
}
