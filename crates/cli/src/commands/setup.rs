/*
   File: crates/cli/src/commands/setup.rs

   Purpose
   One command to get a fresh clone ready: scaffold `.monkey/` if needed,
   import any existing agent prompts, run diagnostics, and print exactly
   what to do next. The goal is the smoothest possible "clone → build → add
   keys → run agents" path — a single `monkey setup` should leave the user
   one obvious step from running agents.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — onboarding orchestrator
*/

use clap::Args as ClapArgs;

use monkey_core::concurrency::{max_concurrent_agents, AgentBudget, HostCapacity};

/// `monkey setup` arguments.
#[derive(Debug, Clone, Default, ClapArgs)]
pub struct Args {}

/// Run the onboarding orchestrator in the current directory.
pub async fn run(_args: Args) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());

    println!("monkey setup");
    println!();

    // 1. Scaffold .monkey/ (also imports existing prompts) or just import.
    if cwd.join(".monkey").is_dir() {
        let s = crate::commands::import::import_existing_prompts(&cwd, false)?;
        for (src, dest) in &s.imported {
            println!("  imported {src} -> .monkey/context/{dest}");
        }
        println!("  [ok]  .monkey/ present");
    } else {
        crate::commands::init::run(crate::commands::init::Args {
            path: Some(cwd.clone()),
            yes: true,
            no_engulf: true,
        })
        .await?;
        println!("  [ok]  .monkey/ scaffolded");
    }
    println!();

    // 2. Diagnostics.
    let report = monkey_agents::doctor();
    println!("  Harnesses (bring your own; the native engine needs no CLI)");
    print_mark("codex", report.codex_present());
    print_mark("claude-code", report.claude_code_present());
    print_mark("hermes", report.hermes_present());
    println!();

    let has_key = report.openrouter_key || report.openai_key;
    println!("  Keys");
    print_mark("OPENROUTER_API_KEY", report.openrouter_key);
    print_mark("OPENAI_API_KEY", report.openai_key);
    println!();

    // 3. Capacity.
    let cap = HostCapacity::detect();
    let native = max_concurrent_agents(&cap, &AgentBudget::native());
    println!(
        "  Capacity: up to {native} native agents on this host \
         ({} MiB free, {} CPUs)",
        cap.available_mem_mb, cap.logical_cpus
    );
    println!();

    // 4. Next steps — make the one blocking thing obvious.
    println!("  Next");
    if !has_key {
        println!("  ⚠ Add an API key, then re-run. Get one at https://openrouter.ai/keys:");
        println!("        export OPENROUTER_API_KEY=sk-or-...");
    } else {
        println!("  You're ready. Spawn agents with:");
        println!("        monkey deck      # web dashboard — spawn and watch 100+ agents");
        println!("        monkey chat      # interactive REPL");
    }
    Ok(())
}

fn print_mark(label: &str, ok: bool) {
    println!("    {} {label}", if ok { "[ok] " } else { "[--] " });
}
