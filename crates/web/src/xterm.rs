/*
   File: crates/web/src/xterm.rs

   Purpose
   wasm-bindgen JS interop for xterm.js. `index.html` loads xterm.js
   via CDN; this module provides a typed handle so leptos components
   can mount it into a <div>, write data, and listen for input.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold
*/

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// xterm.js Terminal handle.
    pub type Terminal;

    #[wasm_bindgen(constructor, js_namespace = ["window"], js_name = "Terminal")]
    pub fn new() -> Terminal;

    /// Mount the terminal into a DOM element.
    #[wasm_bindgen(method)]
    pub fn open(this: &Terminal, container: &web_sys::Element);

    /// Write a chunk to the terminal display.
    #[wasm_bindgen(method)]
    pub fn write(this: &Terminal, data: &str);

    /// Subscribe to user input (return value is xterm's IDisposable).
    #[wasm_bindgen(method, js_name = "onData")]
    pub fn on_data(this: &Terminal, cb: &Closure<dyn FnMut(String)>) -> JsValue;

    /// Resize.
    #[wasm_bindgen(method)]
    pub fn resize(this: &Terminal, cols: u32, rows: u32);
}
