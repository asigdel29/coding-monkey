/*
   File: crates/web/src/ws.rs

   Purpose
   WebSocket client for the deck server. Wraps `web_sys::WebSocket`
   in a typed handle that:

     - Sends auth on open with the token from the page query string
     - Encodes outbound `ClientMsg`s as JSON; rejects on parse error
     - Decodes inbound `ServerMsg`s and routes to per-tab handlers
     - Auto-reconnects with exponential backoff (250 ms → 8 s)
     - Surfaces connection state via a leptos signal so the UI can
       render a banner when disconnected

   Wire format mirrors `monkey_deck::schemas`. Field names use the
   exact JSON key shape the deck server expects (no automatic
   camelCase / snake_case conversion).

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full WS client with auth + reconnect
*/

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{BinaryType, MessageEvent, WebSocket};

/// Outbound — must match the deck server's schemas exactly.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    /// First message after socket open. Token comes from `?t=` query.
    #[serde(rename = "auth")]
    Auth { token: String },
    /// Request the current tentacle list.
    #[serde(rename = "tentacle.list")]
    TentacleList,
    /// Create a tentacle with a title and optional context body.
    #[serde(rename = "tentacle.create")]
    TentacleCreate {
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
    /// Read CONTEXT.md.
    #[serde(rename = "tentacle.context")]
    TentacleContext { id: String },
    /// Overwrite CONTEXT.md.
    #[serde(rename = "tentacle.writeContext")]
    TentacleWriteContext { id: String, content: String },
    /// List todos.
    #[serde(rename = "tentacle.todos")]
    TentacleTodos { id: String },
    /// Toggle a checkbox.
    #[serde(rename = "tentacle.toggle")]
    TentacleToggle { id: String, line: u32 },
    /// Spawn a terminal (optionally scoped to a tentacle).
    #[serde(rename = "term.spawn")]
    TermSpawn {
        #[serde(rename = "tentacleId", skip_serializing_if = "Option::is_none")]
        tentacle_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cols: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
    },
    /// Forward keystrokes.
    #[serde(rename = "term.input")]
    TermInput { id: String, data: String },
    /// Resize a PTY.
    #[serde(rename = "term.resize")]
    TermResize { id: String, cols: u16, rows: u16 },
    /// Kill a terminal.
    #[serde(rename = "term.kill")]
    TermKill { id: String },
}

/// Inbound — the deck server emits these. Unknown shapes are ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// Sent immediately after auth succeeds.
    #[serde(rename = "ready")]
    Ready {
        cwd: String,
        tentacles: Vec<Tentacle>,
    },
    /// Tentacle list snapshot.
    #[serde(rename = "tentacle.list")]
    TentacleList { tentacles: Vec<Tentacle> },
    /// New tentacle was created.
    #[serde(rename = "tentacle.created")]
    TentacleCreated {
        tentacle: Tentacle,
        #[serde(default)]
        todos: Vec<Todo>,
    },
    /// Tentacle removed.
    #[serde(rename = "tentacle.removed")]
    TentacleRemoved { id: String },
    /// Todo list snapshot.
    #[serde(rename = "tentacle.todos")]
    TentacleTodos { id: String, todos: Vec<Todo> },
    /// CONTEXT.md content.
    #[serde(rename = "tentacle.context")]
    TentacleContext { id: String, content: String },
    /// New terminal spawned.
    #[serde(rename = "term.spawned")]
    TermSpawned {
        id: String,
        #[serde(default)]
        cmd: Option<String>,
        #[serde(rename = "tentacleId", default)]
        tentacle_id: Option<String>,
    },
    /// PTY data chunk.
    #[serde(rename = "term.data")]
    TermData { id: String, data: String },
    /// Terminal exited.
    #[serde(rename = "term.exit")]
    TermExit { id: String, code: i32 },
    /// Server-side error.
    #[serde(rename = "term.error", alias = "error")]
    Error {
        #[serde(default)]
        error: Option<String>,
    },
}

/// Mirror of `monkey_deck::tentacles::Tentacle` (subset the UI needs).
#[derive(Debug, Clone, Deserialize)]
pub struct Tentacle {
    /// Stable id (folder name).
    pub id: String,
    /// Title (first H1 of CONTEXT.md or the id).
    pub title: String,
    /// Created-at timestamp in ms (best-effort).
    #[serde(rename = "created_at_ms", default)]
    pub created_at_ms: u64,
}

/// Mirror of `monkey_deck::tentacles::TodoItem`.
#[derive(Debug, Clone, Deserialize)]
pub struct Todo {
    /// Whether the box is checked.
    pub done: bool,
    /// Task text.
    pub text: String,
    /// 0-indexed source line.
    pub line: u32,
}

/// Connection lifecycle states the UI cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Initial state before first connect.
    Connecting,
    /// Connected and authenticated.
    Ready,
    /// Connected but auth failed or timed out.
    AuthFailed,
    /// Disconnected; will retry shortly.
    Reconnecting,
}

/// Live connection handle. Cheap to clone; holds shared state via Rc.
#[derive(Clone)]
pub struct DeckClient {
    inner: Rc<RefCell<Inner>>,
    /// Current connection state, observable from leptos.
    pub state: RwSignal<ConnState>,
}

struct Inner {
    url: String,
    token: String,
    socket: Option<WebSocket>,
    on_msg: Option<Rc<dyn Fn(ServerMsg)>>,
    backoff_ms: u32,
}

impl DeckClient {
    /// Build a client. `url` is the absolute WS URL (e.g.
    /// `ws://127.0.0.1:8787/ws`). `token` is the `?t=` value.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                url: url.into(),
                token: token.into(),
                socket: None,
                on_msg: None,
                backoff_ms: 250,
            })),
            state: RwSignal::new(ConnState::Connecting),
        }
    }

    /// Register the inbound message handler. Called once before
    /// [`Self::connect`].
    pub fn on_message(&self, cb: impl Fn(ServerMsg) + 'static) {
        self.inner.borrow_mut().on_msg = Some(Rc::new(cb));
    }

    /// Open the socket. Reuses the existing handler / backoff state.
    pub fn connect(&self) {
        let state_sig = self.state;
        state_sig.set(ConnState::Connecting);

        let url = self.inner.borrow().url.clone();
        let ws = match WebSocket::new(&url) {
            Ok(s) => s,
            Err(_) => {
                state_sig.set(ConnState::Reconnecting);
                self.schedule_reconnect();
                return;
            }
        };
        ws.set_binary_type(BinaryType::Arraybuffer);

        // open: send auth.
        let inner_open = Rc::clone(&self.inner);
        let on_open = Closure::wrap(Box::new(move |_| {
            let auth = ClientMsg::Auth {
                token: inner_open.borrow().token.clone(),
            };
            if let Ok(s) = serde_json::to_string(&auth) {
                if let Some(sock) = inner_open.borrow().socket.as_ref() {
                    let _ = sock.send_with_str(&s);
                }
            }
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        // message: parse + dispatch.
        let inner_msg = Rc::clone(&self.inner);
        let state_msg = state_sig;
        let on_message = Closure::wrap(Box::new(move |evt: MessageEvent| {
            if let Some(s) = evt.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<ServerMsg>(&s) {
                    if matches!(msg, ServerMsg::Ready { .. }) {
                        state_msg.set(ConnState::Ready);
                        // Reset backoff on a successful auth.
                        inner_msg.borrow_mut().backoff_ms = 250;
                    }
                    if let Some(cb) = inner_msg.borrow().on_msg.clone() {
                        cb(msg);
                    }
                } else if let Ok(v) = serde_json::from_str::<Value>(&s) {
                    web_sys::console::warn_1(
                        &format!("deck: unhandled message {}", v).into(),
                    );
                }
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();

        // close: schedule reconnect.
        let me = self.clone();
        let on_close = Closure::wrap(Box::new(move |_| {
            me.state.set(ConnState::Reconnecting);
            me.schedule_reconnect();
        }) as Box<dyn FnMut(JsValue)>);
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        self.inner.borrow_mut().socket = Some(ws);
    }

    /// Send a typed message. No-op if the socket isn't OPEN.
    pub fn send(&self, msg: &ClientMsg) {
        let inner = self.inner.borrow();
        let Some(sock) = inner.socket.as_ref() else { return };
        if sock.ready_state() != WebSocket::OPEN {
            return;
        }
        if let Ok(s) = serde_json::to_string(msg) {
            let _ = sock.send_with_str(&s);
        }
    }

    fn schedule_reconnect(&self) {
        let me = self.clone();
        let delay = {
            let mut g = self.inner.borrow_mut();
            let d = g.backoff_ms;
            g.backoff_ms = (g.backoff_ms.saturating_mul(2)).min(8_000);
            d
        };
        let win = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        let cb = Closure::once_into_js(move || {
            me.connect();
        });
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            delay as i32,
        );
    }
}

/// Read `?t=<token>` from `window.location.search`.
pub fn token_from_query() -> Option<String> {
    let win = web_sys::window()?;
    let loc = win.location();
    let search = loc.search().ok()?;
    let trimmed = search.trim_start_matches('?');
    for pair in trimmed.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "t" {
            return Some(urlish_decode(v));
        }
    }
    None
}

/// Best-effort `?t=` URL-decode (token is hex anyway, so this is just
/// for safety against accidental %-encoding).
fn urlish_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let a = chars.next();
            let b = chars.next();
            match (a, b) {
                (Some(x), Some(y)) => {
                    let pair = format!("{x}{y}");
                    if let Ok(n) = u8::from_str_radix(&pair, 16) {
                        out.push(n as char);
                        continue;
                    }
                    out.push('%');
                    out.push(x);
                    out.push(y);
                }
                _ => out.push('%'),
            }
        } else if c == '+' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Compute the WebSocket URL from `window.location`. http→ws, https→wss.
pub fn ws_url_from_location() -> String {
    let win = match web_sys::window() {
        Some(w) => w,
        None => return "ws://127.0.0.1:8787/ws".into(),
    };
    let loc = win.location();
    let proto = loc.protocol().unwrap_or_else(|_| "http:".into());
    let host = loc.host().unwrap_or_else(|_| "127.0.0.1:8787".into());
    let scheme = if proto == "https:" { "wss" } else { "ws" };
    format!("{scheme}://{host}/ws")
}
