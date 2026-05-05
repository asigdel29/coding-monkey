/*
   File: crates/cli/src/commands/doctor.rs
   Purpose: environment diagnostics — keys, git, agent CLIs.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use monkey_agents::doctor;

pub async fn run() -> anyhow::Result<()> {
    let r = doctor();
    println!("monkey doctor");
    println!("  {} claude on PATH", mark(r.claude_present));
    println!("  {} codex on PATH", mark(r.codex_present));
    println!("  {} git on PATH", mark(r.git_present));
    println!("  {} ANTHROPIC_API_KEY", mark(r.anthropic_key));
    println!("  {} OPENAI_API_KEY", mark(r.openai_key));
    for n in &r.notes {
        println!("  ! {n}");
    }
    std::process::exit(if r.ok { 0 } else { 1 });
}

fn mark(ok: bool) -> &'static str {
    if ok { "[ok]  " } else { "[fail]" }
}
