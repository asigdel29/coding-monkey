/*
   File: crates/deck/src/schemas.rs

   Purpose
   Wire-format schemas shared between server and WASM frontend. Every
   inbound WS message and outbound event must be one of these — anything
   else is rejected.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use serde::{Deserialize, Serialize};

/// Inbound WS message from the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Open a new terminal scoped to a tentacle.
    OpenTerminal {
        /// Tentacle id.
        tentacle_id: String,
    },
    /// Send keystrokes to an existing terminal.
    Input {
        /// Terminal id.
        terminal_id: String,
        /// UTF-8 keystrokes.
        data: String,
    },
    /// Resize an existing terminal.
    Resize {
        /// Terminal id.
        terminal_id: String,
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
}

/// Outbound WS message to the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Terminal opened — id assigned by the server.
    TerminalOpened {
        /// Terminal id.
        terminal_id: String,
    },
    /// New PTY data chunk.
    Output {
        /// Terminal id.
        terminal_id: String,
        /// Bytes (utf-8 lossy).
        data: String,
    },
    /// Terminal exited.
    Exit {
        /// Terminal id.
        terminal_id: String,
        /// Exit code.
        code: i32,
    },
    /// Server-side error surfaced to the client.
    Error {
        /// Free-text message.
        message: String,
    },
}
