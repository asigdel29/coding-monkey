/*
   File: crates/cli/src/commands/models.rs
   Purpose: list registered AI models with tier, host, endpoint, and (with
            --probe) live reachability of local endpoints.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-06-19   Anubhav Sigdel  fold in local models; show host/endpoint;
                                 add --probe reachability
*/

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use monkey_core::{LocalHost, ModelRegistry, ModelTier, OrchestratorConfig, Provider};

/// How long to wait for a local endpoint to answer a `--probe`.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Parser, Debug, Default)]
pub struct Args {
    /// Project directory whose `.monkey/config.json` declares local models.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Probe each local endpoint and show whether it is reachable.
    #[arg(long)]
    pub probe: bool,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let cwd = args
        .cwd
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let cfg = load_config(&cwd);
    let registry = ModelRegistry::with_config(&cfg);
    // Map each local model id to where it runs, for the Host column.
    let hosts: HashMap<&str, LocalHost> = cfg
        .local_models
        .iter()
        .map(|m| (m.id.as_str(), m.host))
        .collect();

    println!("Available Models");
    if let Some(d) = &cfg.default_model {
        println!("  default coding model: {d}");
    }
    for m in registry.list_all() {
        let host = match hosts.get(m.id.as_str()) {
            Some(LocalHost::Pi) => "pi",
            Some(LocalHost::Lan) => "lan",
            Some(LocalHost::Cloud) => "cloud",
            None => match m.provider {
                Provider::SelfHosted => "self-hosted",
                _ => "hosted",
            },
        };
        let endpoint = m.base_url.as_deref().unwrap_or("(provider default)");
        let reach = if args.probe && m.base_url.is_some() {
            if monkey_runtime::endpoint_reachable(m.base_url.as_deref().unwrap(), PROBE_TIMEOUT)
                .await
            {
                " [up]"
            } else {
                " [down]"
            }
        } else {
            ""
        };
        println!(
            "  {:28} {:9} {:11} {}{}",
            m.display_name,
            tier_str(m.tier),
            host,
            endpoint,
            reach,
        );
    }
    Ok(())
}

fn tier_str(t: ModelTier) -> &'static str {
    match t {
        ModelTier::Fast => "fast",
        ModelTier::Balanced => "balanced",
        ModelTier::Powerful => "powerful",
    }
}

/// Load `.monkey/config.json`, falling back to defaults when absent or invalid
/// so `monkey models` always lists the builtin lineup.
fn load_config(cwd: &std::path::Path) -> OrchestratorConfig {
    std::fs::read_to_string(cwd.join(".monkey").join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}
