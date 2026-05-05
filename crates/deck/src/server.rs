/*
   File: crates/deck/src/server.rs

   Purpose
   axum-based HTTP+WS server. Bind/TTL/rate-limit policy lives here;
   actual route handlers split into submodules over follow-up commits.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; DeckOpts + DeckHandle
*/

use std::path::PathBuf;
use std::time::Duration;

/// Options for [`start_deck`]. All fields have safe defaults.
#[derive(Debug, Clone)]
pub struct DeckOpts {
    /// Project working directory.
    pub cwd: PathBuf,
    /// Bind address. Default `127.0.0.1`.
    pub host: String,
    /// Port. Default `8787`.
    pub port: u16,
    /// Agent binary spawned for new terminals (default `claude`).
    pub agent: String,
    /// Extra args for the agent.
    pub agent_args: Vec<String>,
    /// Session TTL.
    pub token_ttl: Duration,
    /// WS messages/sec/connection.
    pub rate_per_sec: u32,
    /// TLS cert path (PEM).
    pub cert: Option<PathBuf>,
    /// TLS key path (PEM).
    pub key: Option<PathBuf>,
    /// Enforce TLS off-loopback.
    pub enforce_tls_off_loopback: bool,
}

impl Default for DeckOpts {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            host: "127.0.0.1".into(),
            port: 8787,
            agent: "claude".into(),
            agent_args: Vec::new(),
            token_ttl: Duration::from_secs(8 * 60 * 60),
            rate_per_sec: 100,
            cert: None,
            key: None,
            enforce_tls_off_loopback: true,
        }
    }
}

/// Live deck server. Drop / `close()` to shut down.
#[derive(Debug)]
pub struct DeckHandle {
    /// URL the server is listening on (`https://host:port` or http for loopback).
    pub url: String,
    /// Bind host.
    pub host: String,
    /// Scheme (`http` | `https`).
    pub scheme: String,
    /// When the issued session token expires.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl DeckHandle {
    /// Trigger a graceful shutdown.
    pub async fn close(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

/// Start the deck server. Stub — returns an immediately-closeable handle
/// while route handlers are being ported.
pub async fn start_deck(opts: DeckOpts) -> anyhow::Result<DeckHandle> {
    let scheme = if opts.cert.is_some() { "https" } else { "http" };
    let url = format!("{}://{}:{}", scheme, opts.host, opts.port);
    let expires_at = chrono::Utc::now() + chrono::Duration::from_std(opts.token_ttl)?;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    Ok(DeckHandle {
        url,
        host: opts.host,
        scheme: scheme.into(),
        expires_at,
        shutdown: Some(tx),
    })
}
