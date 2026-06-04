/*
   File: crates/cli/src/commands/deck.rs
   Purpose: launch monkey-deck server.
   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
   2026-06-03   Anubhav Sigdel  default agent binary → codex
*/

use clap::Args as ClapArgs;
use monkey_deck::{start_deck, DeckOpts};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[arg(long, default_value_t = 8787)]
    pub port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(long, default_value = "codex")]
    pub agent: String,
    #[arg(long, value_delimiter = ',')]
    pub agent_args: Vec<String>,
    #[arg(long, default_value_t = 28800)]
    pub ttl: u64,
    #[arg(long, default_value_t = 100)]
    pub rate: u32,
    #[arg(long)]
    pub cert: Option<PathBuf>,
    #[arg(long)]
    pub key: Option<PathBuf>,
    #[arg(long = "insecure-no-tls", default_value_t = false)]
    pub insecure_no_tls: bool,
    /// Static asset directory served from `/static/*`. The WASM
    /// bundle (crates/web/dist) goes here so the leptos UI can boot.
    #[arg(long = "static-dir")]
    pub static_dir: Option<PathBuf>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let opts = DeckOpts {
        cwd: args
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into())),
        host: args.host,
        port: args.port,
        agent: args.agent,
        agent_args: args.agent_args,
        token_ttl: Duration::from_secs(args.ttl),
        rate_per_sec: args.rate,
        cert: args.cert,
        key: args.key,
        enforce_tls_off_loopback: !args.insecure_no_tls,
        static_dir: args.static_dir,
        ..Default::default()
    };
    let handle = start_deck(opts).await?;
    eprintln!("deck listening on {}", handle.url);
    eprintln!("    expires: {}", handle.expires_at.to_rfc3339());
    tokio::signal::ctrl_c().await.ok();
    handle.close().await?;
    Ok(())
}
