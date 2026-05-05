/*
   File: crates/web/src/lib.rs

   Purpose
   WASM frontend for the deck dashboard. Built with leptos (CSR mode);
   compiled to wasm32-unknown-unknown via `wasm-pack build --target web`
   or `trunk serve`. Talks to the deck server via WebSocket.

   The xterm.js terminal widget is rendered into a <div> via wasm-bindgen
   JS interop (see `xterm.rs`).

   Build (from this directory):
       trunk serve
   or
       wasm-pack build --target web --release && cp index.html pkg/

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; leptos shell + WS plumbing
*/

#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `monkey-web` — leptos CSR frontend for the deck dashboard.

mod app;
mod ws;
mod xterm;

pub use app::App;

use wasm_bindgen::prelude::*;

/// Mount the leptos app to `<body>`. Called from `index.html`.
#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
