/*
   File: crates/web/src/xterm.rs

   Purpose
   wasm-bindgen JS interop for xterm.js. The HTML page loads xterm.js
   from CDN; this module surfaces a typed handle so Rust components
   can construct, mount, write, resize, and subscribe to user input.

   Pinned to xterm@5.x. The fit addon ships separately; for the
   initial port we set explicit cols/rows from the deck server's
   spawn response and call `resize()` on container resize observer
   ticks instead of pulling in `@xterm/addon-fit`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full xterm interop with on_data hook +
                                 resize + dispose
*/

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// Top-level xterm.js Terminal handle (constructed via `new Terminal()`).
    #[wasm_bindgen(js_namespace = ["window"], js_name = Terminal)]
    pub type Terminal;

    /// `new Terminal(options?)`.
    #[wasm_bindgen(constructor, js_namespace = ["window"], js_name = "Terminal")]
    pub fn new() -> Terminal;

    /// `new Terminal(options)` — pass an inline JS object.
    #[wasm_bindgen(constructor, js_namespace = ["window"], js_name = "Terminal")]
    pub fn new_with_opts(opts: &JsValue) -> Terminal;

    /// Mount the terminal into a DOM element.
    #[wasm_bindgen(method)]
    pub fn open(this: &Terminal, container: &web_sys::Element);

    /// Write a chunk to the terminal display.
    #[wasm_bindgen(method)]
    pub fn write(this: &Terminal, data: &str);

    /// Subscribe to keystrokes; the returned `IDisposable` is dropped
    /// when xterm tears down.
    #[wasm_bindgen(method, js_name = "onData")]
    pub fn on_data(this: &Terminal, cb: &Closure<dyn FnMut(String)>) -> JsValue;

    /// Resize the rendered grid.
    #[wasm_bindgen(method)]
    pub fn resize(this: &Terminal, cols: u32, rows: u32);

    /// Dispose all resources (event handlers, canvases, …).
    #[wasm_bindgen(method)]
    pub fn dispose(this: &Terminal);

    /// Focus the input.
    #[wasm_bindgen(method)]
    pub fn focus(this: &Terminal);
}

/// Build the Terminal options literal we want for every spawn — dark
/// theme, monospace, 14px, hardware acceleration off (better
/// compatibility on older browsers + WebKit).
pub fn default_terminal_options(cols: u16, rows: u16) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &"cols".into(), &JsValue::from_f64(cols as f64));
    let _ = js_sys::Reflect::set(&obj, &"rows".into(), &JsValue::from_f64(rows as f64));
    let _ = js_sys::Reflect::set(
        &obj,
        &"fontFamily".into(),
        &"'JetBrains Mono', 'SF Mono', Menlo, Consolas, monospace".into(),
    );
    let _ = js_sys::Reflect::set(&obj, &"fontSize".into(), &JsValue::from_f64(14.0));
    let _ = js_sys::Reflect::set(&obj, &"cursorBlink".into(), &JsValue::TRUE);
    let _ = js_sys::Reflect::set(&obj, &"allowTransparency".into(), &JsValue::FALSE);
    let _ = js_sys::Reflect::set(&obj, &"convertEol".into(), &JsValue::FALSE);

    // Dark theme tuned to match the deck's monkey palette.
    let theme = js_sys::Object::new();
    for (k, v) in [
        ("background", "#0b1f17"),
        ("foreground", "#f8e2c5"),
        ("cursor", "#c26b30"),
        ("cursorAccent", "#0b1f17"),
        ("selectionBackground", "#1d4a37"),
        ("black", "#1a1a1a"),
        ("red", "#e06c75"),
        ("green", "#98c379"),
        ("yellow", "#e5c07b"),
        ("blue", "#61afef"),
        ("magenta", "#c678dd"),
        ("cyan", "#56b6c2"),
        ("white", "#dcdfe4"),
    ] {
        let _ = js_sys::Reflect::set(&theme, &(*k).into(), &(*v).into());
    }
    let _ = js_sys::Reflect::set(&obj, &"theme".into(), &theme.into());
    obj.into()
}
