/*
   File: crates/deck/src/lib.rs

   Purpose
   Web frontend server. axum HTTP + WebSocket; refuses to bind
   off-loopback without TLS unless --insecure-no-tls. Each terminal
   tab is a PTY-spawned agent (via monkey-agents). Tentacles are
   folders in .monkey/tentacles/<id>/ holding CONTEXT.md + todo.md.

   Invariants
   - WS messages rate-limited per connection (default 100/s).
   - Sessions expire (default 8h TTL); reconnect requires re-issue.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; module layout + DeckHandle
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-deck` — web frontend server.

/// WebSocket message schemas (typed parse + serialize).
pub mod schemas;
/// Axum HTTP + WebSocket server, session lifecycle, rate limiting.
pub mod server;
/// Tentacle store — `.monkey/tentacles/<id>/` workspaces with todo + context.
pub mod tentacles;

pub use schemas::{parse_ws_msg, WsMsg};
pub use server::{start_deck, DeckHandle, DeckOpts};
pub use tentacles::{slug, Tentacle, TentacleStore, TodoItem};
