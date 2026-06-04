/*
   File: crates/cli/src/commands/doctor.rs
   Purpose: environment diagnostics — keys, git, agent CLIs, repo state.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-05-06   Anubhav Sigdel  print versions + repo state; add --json
   2026-06-03   Anubhav Sigdel  de-brand keys; add Capacity (RAM/agent cap)
*/

use clap::Parser;
use monkey_agents::{doctor, DoctorReport};
use monkey_core::concurrency::{max_concurrent_agents, AgentBudget, HostCapacity};

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Emit the report as JSON (one object, no trailing newline).
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let r = doctor();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        std::process::exit(if r.ok { 0 } else { 1 });
    }
    print_human(&r);
    std::process::exit(if r.ok { 0 } else { 1 });
}

fn print_human(r: &DoctorReport) {
    println!("monkey doctor");
    println!("  cwd: {}", r.cwd.display());
    println!();
    println!("  CLIs");
    println!(
        "    {} codex    {}",
        mark(r.codex_present()),
        version_or_dash(&r.codex_version)
    );
    println!(
        "    {} git      {}",
        mark(r.git_present()),
        version_or_dash(&r.git_version)
    );
    println!();
    println!("  Keys");
    println!("    {} OPENROUTER_API_KEY", mark(r.openrouter_key));
    println!("    {} OPENAI_API_KEY", mark(r.openai_key));
    println!();
    println!("  Repo");
    println!("    {} inside git work tree", mark(r.in_git_repo));
    println!("    {} .monkey/ initialized", mark(r.monkey_initialized));
    match (&r.tech_stack, &r.repo_complexity) {
        (Some(s), Some(c)) => println!("    [info] stack={s:?}  complexity={c:?}"),
        (Some(s), None) => println!("    [info] stack={s:?}"),
        _ => println!("    [info] stack=unknown"),
    }
    println!();
    println!("  Capacity");
    let cap = HostCapacity::detect();
    let max_agents = max_concurrent_agents(&cap, &AgentBudget::default());
    println!(
        "    [info] RAM {avail} MiB free / {total} MiB total   CPUs {cpus}",
        avail = cap.available_mem_mb,
        total = cap.total_mem_mb,
        cpus = cap.logical_cpus,
    );
    println!("    [info] max concurrent agents: {max_agents}");
    if !r.notes.is_empty() {
        println!();
        println!("  Notes");
        for n in &r.notes {
            println!("    ! {n}");
        }
    }
    println!();
    println!("  result: {}", if r.ok { "[ok]" } else { "[fail]" });
}

fn version_or_dash(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("-")
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "[ok]  "
    } else {
        "[fail]"
    }
}
