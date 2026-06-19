/*
   File: crates/cli/src/commands/doctor.rs
   Purpose: environment diagnostics — keys, git, agent CLIs, repo state.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-05-06   Anubhav Sigdel  print versions + repo state; add --json
   2026-06-03   Anubhav Sigdel  de-brand keys; add Capacity (RAM/agent cap)
   2026-06-19   Anubhav Sigdel  probe configured local model endpoints
*/

use std::time::Duration;

use clap::Parser;
use monkey_agents::{doctor, DoctorReport};
use monkey_core::concurrency::{max_concurrent_agents, AgentBudget, HostCapacity};
use monkey_core::{LocalHost, OrchestratorConfig};

/// Timeout for each local endpoint reachability probe.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Reachability of one configured local model, for the report.
struct LocalModelStatus {
    display_name: String,
    host: LocalHost,
    base_url: String,
    reachable: bool,
}

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
    // Probe local endpoints (informational only — never changes the exit code).
    let local = probe_local_models(&r.cwd).await;
    print_human(&r, &local);
    std::process::exit(if r.ok { 0 } else { 1 });
}

/// Probe every local model declared in `.monkey/config.json` for reachability.
/// Returns an empty list when no config or no local models are present.
async fn probe_local_models(cwd: &std::path::Path) -> Vec<LocalModelStatus> {
    let cfg: OrchestratorConfig = std::fs::read_to_string(cwd.join(".monkey").join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(cfg.local_models.len());
    for m in &cfg.local_models {
        let reachable = monkey_runtime::endpoint_reachable(&m.base_url, PROBE_TIMEOUT).await;
        out.push(LocalModelStatus {
            display_name: m.display_name.clone(),
            host: m.host,
            base_url: m.base_url.clone(),
            reachable,
        });
    }
    out
}

fn print_human(r: &DoctorReport, local: &[LocalModelStatus]) {
    println!("monkey doctor");
    println!("  cwd: {}", r.cwd.display());
    println!();
    println!("  Harnesses");
    println!(
        "    {} codex        {}",
        mark(r.codex_present()),
        version_or_dash(&r.codex_version)
    );
    println!(
        "    {} claude-code  {}",
        mark(r.claude_code_present()),
        version_or_dash(&r.claude_code_version)
    );
    println!(
        "    {} hermes       {}",
        mark(r.hermes_present()),
        version_or_dash(&r.hermes_version)
    );
    println!(
        "    {} git          {}",
        mark(r.git_present()),
        version_or_dash(&r.git_version)
    );
    println!();
    println!("  Keys & endpoints");
    println!("    {} OPENROUTER_API_KEY", mark(r.openrouter_key));
    println!("    {} OPENAI_API_KEY", mark(r.openai_key));
    println!(
        "    {} self-hosted endpoint  {}",
        mark(r.self_hosted_url.is_some()),
        r.self_hosted_url.as_deref().unwrap_or("-")
    );
    if !local.is_empty() {
        println!();
        println!("  Local models");
        for m in local {
            let host = match m.host {
                LocalHost::Pi => "pi   ",
                LocalHost::Lan => "lan  ",
                LocalHost::Cloud => "cloud",
            };
            println!(
                "    {} {host}  {:24} {}",
                mark(m.reachable),
                m.display_name,
                m.base_url,
            );
        }
    }
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
    let max_pty = max_concurrent_agents(&cap, &AgentBudget::pty());
    let max_native = max_concurrent_agents(&cap, &AgentBudget::native());
    println!(
        "    [info] RAM {avail} MiB free / {total} MiB total   CPUs {cpus}",
        avail = cap.available_mem_mb,
        total = cap.total_mem_mb,
        cpus = cap.logical_cpus,
    );
    println!("    [info] max native agents: {max_native}   max PTY agents: {max_pty}");
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
