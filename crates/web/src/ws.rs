/*
   File: crates/web/src/ws.rs

   Purpose
   WebSocket client for talking to the deck server. Wire format mirrors
   `monkey-deck::schemas`: ClientMsg outbound, ServerMsg inbound. Uses
   gloo-net's WebSocket so the same code works under CSR and SSR-with-
   client-rendered fallback.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};

/// Messages the client sends to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Open a new terminal scoped to a tentacle.
    OpenTerminal { tentacle_id: String },
    /// Forward keystrokes.
    Input { terminal_id: String, data: String },
    /// Resize the PTY.
    Resize { terminal_id: String, cols: u16, rows: u16 },
}

/// Messages the server sends to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Terminal opened with the given id.
    TerminalOpened { terminal_id: String },
    /// PTY output chunk.
    Output { terminal_id: String, data: String },
    /// PTY exited.
    Exit { terminal_id: String, code: i32 },
    /// Server-side error.
    Error { message: String },
}
