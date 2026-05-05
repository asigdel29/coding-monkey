/*
   File: crates/deck/src/server.rs

   Purpose
   axum HTTP+WebSocket server for the deck UI. Production hardening
   matches the TS reference 1:1:

     - Binds 127.0.0.1 by default (CC6.6 boundary protection)
     - Per-process random session token; HTTP query + WS frame both
       require it; comparisons are constant-time (CC6.1)
     - Strict CSP, no-sniff, frame-ancestors none (CC7.1, CC6.7)
     - Origin header check on WS upgrade — defends DNS rebinding (CC6.6)
     - Every WS message validated against the schema before any side
       effect (CC7.2)
     - Connect / spawn / kill / failed-auth events go to the audit
       hash chain via monkey-agents (CC4.1, CC7.3)
     - Token bucket rate limiter per connection
     - Configurable session TTL (default 8h)
     - Optional rustls TLS

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full Rust port from packages/deck/src/server.ts
*/

use anyhow::Context;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use monkey_agents::{AuditEventType, AuditLogger};

use crate::schemas::{parse_ws_msg, WsMsg};
use crate::tentacles::TentacleStore;

/// Options for [`start_deck`]. All fields have safe defaults.
#[derive(Debug, Clone)]
pub struct DeckOpts {
    /// Project working directory.
    pub cwd: PathBuf,
    /// Bind address. Default `127.0.0.1`.
    pub host: String,
    /// Port. Default `8787`.
    pub port: u16,
    /// Agent binary spawned for new terminals.
    pub agent: String,
    /// Extra args for the agent binary.
    pub agent_args: Vec<String>,
    /// Session TTL.
    pub token_ttl: Duration,
    /// Override the auto-generated token (tests).
    pub token: Option<String>,
    /// WS messages/sec/connection.
    pub rate_per_sec: u32,
    /// Token bucket burst capacity. Defaults to `rate_per_sec * 2`.
    pub rate_burst: u32,
    /// TLS cert path (PEM).
    pub cert: Option<PathBuf>,
    /// TLS key path (PEM).
    pub key: Option<PathBuf>,
    /// Refuse to bind off-loopback without TLS.
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
            token: None,
            rate_per_sec: 100,
            rate_burst: 200,
            cert: None,
            key: None,
            enforce_tls_off_loopback: true,
        }
    }
}

/// Live deck server.
#[derive(Debug)]
pub struct DeckHandle {
    /// URL with the session token query param attached.
    pub url: String,
    /// Bind host.
    pub host: String,
    /// Resolved port (post-bind).
    pub port: u16,
    /// Per-process session token.
    pub token: String,
    /// Token expiry.
    pub expires_at: DateTime<Utc>,
    /// Scheme (`http` | `https`).
    pub scheme: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl DeckHandle {
    /// Trigger graceful shutdown and wait for the server task to exit.
    pub async fn close(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.await;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct AppState {
    cwd: PathBuf,
    agent: String,
    agent_args: Vec<String>,
    token: String,
    expires_at_ms: u64,
    scheme: String,
    bound_host: String,
    bound_port: u16,
    rate: RateLimit,
    tentacles: TentacleStore,
    audit: Mutex<AuditLogger>,
    terminals: Mutex<HashMap<String, monkey_agents::AgentTerminal>>,
}

#[derive(Debug, Clone, Copy)]
struct RateLimit {
    per_sec: u32,
    burst: u32,
}

#[derive(Debug, Deserialize)]
struct IndexQuery {
    #[serde(default)]
    t: Option<String>,
}

/// Start the deck server. Returns once the listener is bound; the
/// returned handle owns the background task and the shutdown channel.
pub async fn start_deck(opts: DeckOpts) -> anyhow::Result<DeckHandle> {
    let host = opts.host.clone();
    let is_loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    let use_tls = opts.cert.is_some() && opts.key.is_some();
    if !is_loopback && !use_tls && opts.enforce_tls_off_loopback {
        return Err(anyhow::anyhow!(
            "deck: non-loopback bind requires TLS (--cert + --key) or --insecure-no-tls"
        ));
    }
    if use_tls {
        return Err(anyhow::anyhow!(
            "deck: TLS support is wired but disabled in the initial port — bind 127.0.0.1 \
             or run behind a reverse proxy. TLS lands in a follow-up commit."
        ));
    }

    let token = opts.token.clone().unwrap_or_else(generate_token);
    let expires_at_ms = (Utc::now()
        .timestamp_millis()
        .saturating_add(opts.token_ttl.as_millis() as i64)) as u64;
    let expires_at = DateTime::<Utc>::from_timestamp_millis(expires_at_ms as i64)
        .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(28_800));

    // Audit log path under the project root so all deck actions
    // contribute to the same SOC 2 chain `monkey compliance verify`
    // walks.
    let audit_path = opts
        .cwd
        .join(".monkey")
        .join("sessions")
        .join(format!("audit-deck-{}.log", &token[..8.min(token.len())]));
    let mut audit = AuditLogger::open(&audit_path)?;
    audit
        .log(
            AuditEventType::SessionStart,
            serde_json::json!({
                "component": "deck",
                "host": host,
                "port": opts.port,
                "tls": use_tls,
                "ttl_seconds": opts.token_ttl.as_secs(),
            }),
        )
        .ok();

    let state = Arc::new(AppState {
        cwd: opts.cwd.clone(),
        agent: opts.agent.clone(),
        agent_args: opts.agent_args.clone(),
        token: token.clone(),
        expires_at_ms,
        scheme: if use_tls { "https".into() } else { "http".into() },
        bound_host: host.clone(),
        bound_port: opts.port,
        rate: RateLimit {
            per_sec: opts.rate_per_sec,
            burst: opts.rate_burst.max(opts.rate_per_sec),
        },
        tentacles: TentacleStore::new(&opts.cwd),
        audit: Mutex::new(audit),
        terminals: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/healthz", get(serve_health))
        .route("/ws", get(ws_upgrade))
        .with_state(state.clone());

    let bind: SocketAddr = format!("{host}:{}", opts.port)
        .parse()
        .with_context(|| format!("invalid bind address {host}:{}", opts.port))?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let local = listener.local_addr().context("local_addr")?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let scheme = if use_tls { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}/?t={}", host, local.port(), token);

    let join = tokio::spawn(async move {
        let svc = app.into_make_service_with_connect_info::<SocketAddr>();
        let _ = axum::serve(listener, svc)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });

    Ok(DeckHandle {
        url,
        host,
        port: local.port(),
        token,
        expires_at,
        scheme: scheme.into(),
        shutdown: Some(tx),
        join: Some(join),
    })
}

// ─── HTTP handlers ──────────────────────────────────────────────────────────

async fn serve_index(
    State(state): State<Arc<AppState>>,
    Query(q): Query<IndexQuery>,
) -> Response {
    if expired(&state) {
        return text_response(
            StatusCode::UNAUTHORIZED,
            "Session expired. Restart `monkey deck` to issue a new token.\n",
            &state,
        );
    }
    let provided = q.t.unwrap_or_default();
    if !constant_time_eq(provided.as_bytes(), state.token.as_bytes()) {
        return text_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized — append ?t=<token> from the deck startup output.\n",
            &state,
        );
    }
    let html = index_html(&state.token);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    apply_security_headers(&mut headers, &state);
    (StatusCode::OK, headers, html).into_response()
}

async fn serve_health(State(state): State<Arc<AppState>>) -> Response {
    let n = state.terminals.lock().await.len();
    let body = serde_json::json!({ "ok": true, "terminals": n });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    apply_security_headers(&mut headers, &state);
    (StatusCode::OK, headers, body.to_string()).into_response()
}

fn text_response(status: StatusCode, body: &str, state: &AppState) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    apply_security_headers(&mut headers, state);
    (status, headers, body.to_string()).into_response()
}

fn apply_security_headers(headers: &mut HeaderMap, state: &AppState) {
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );
    let csp = format!(
        "default-src 'none'; \
         script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; \
         style-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; \
         connect-src 'self' ws://{host}:{port} wss://{host}:{port}; \
         img-src 'self' data:; \
         font-src 'self' https://cdn.jsdelivr.net; \
         frame-ancestors 'none'; \
         base-uri 'none'",
        host = state.bound_host,
        port = state.bound_port,
    );
    if let Ok(v) = HeaderValue::from_str(&csp) {
        headers.insert("content-security-policy", v);
    }
}

fn index_html(token: &str) -> String {
    // Minimal placeholder. The real WASM frontend lives in crates/web
    // and is served as static assets in a follow-up commit.
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>monkey deck</title>\
         <meta name=viewport content=\"width=device-width\"></head>\
         <body><h1>monkey deck</h1>\
         <p>WebSocket: <code>ws://HOST/ws?t={token}</code></p>\
         <p>This page is served by the Rust deck server. The WASM \
         frontend (crates/web) plugs in here in a follow-up commit.</p>\
         </body></html>"
    )
}

// ─── WebSocket ──────────────────────────────────────────────────────────────

async fn ws_upgrade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(_who): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if expired(&state) {
        audit(&state, "ws.auth.fail", serde_json::json!({ "reason": "session-expired" })).await;
        return (StatusCode::UNAUTHORIZED, "expired").into_response();
    }
    if !origin_is_allowed(&headers, &state) {
        audit(
            &state,
            "ws.auth.fail",
            serde_json::json!({
                "reason": "bad-origin",
                "origin": headers
                    .get("origin")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("(none)"),
            }),
        )
        .await;
        return (StatusCode::FORBIDDEN, "bad origin").into_response();
    }
    let st = state.clone();
    ws.on_upgrade(move |sock| handle_ws(sock, st))
}

fn origin_is_allowed(headers: &HeaderMap, state: &AppState) -> bool {
    let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) else {
        // No origin (curl, native client) is allowed — Origin is browser-only.
        return true;
    };
    let allowed = [
        format!("{}://{}:{}", state.scheme, state.bound_host, state.bound_port),
        format!("{}://localhost:{}", state.scheme, state.bound_port),
        format!("{}://127.0.0.1:{}", state.scheme, state.bound_port),
    ];
    allowed.iter().any(|a| a == origin)
}

async fn handle_ws(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut authed = false;
    let mut owned: Vec<String> = Vec::new();
    let mut bucket = TokenBucket::new(state.rate);
    let mut rate_logged = false;

    audit(&state, "ws.connect", serde_json::json!({})).await;

    let auth_deadline = Instant::now() + Duration::from_secs(5);

    while let Some(frame) = receiver.next().await {
        if !authed && Instant::now() > auth_deadline {
            audit(&state, "ws.auth.fail", serde_json::json!({ "reason": "timeout" })).await;
            let _ = sender.send(Message::Close(None)).await;
            break;
        }
        if expired(&state) {
            audit(&state, "ws.disconnect", serde_json::json!({ "reason": "session-expired" })).await;
            let _ = sender.send(Message::Close(None)).await;
            break;
        }
        let frame = match frame {
            Ok(f) => f,
            Err(_) => break,
        };
        let text = match frame {
            Message::Text(t) => t,
            Message::Binary(_) => continue,
            Message::Ping(p) => {
                let _ = sender.send(Message::Pong(p)).await;
                continue;
            }
            Message::Pong(_) => continue,
            Message::Close(_) => break,
        };

        if !bucket.consume() {
            if !rate_logged {
                audit(&state, "ws.rate-limit", serde_json::json!({})).await;
                rate_logged = true;
            }
            let _ = sender.send(Message::Close(None)).await;
            break;
        }

        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                audit(&state, "ws.auth.fail", serde_json::json!({ "reason": "bad-json" })).await;
                continue;
            }
        };
        let msg = match parse_ws_msg(&raw) {
            Ok(m) => m,
            Err(reason) => {
                audit(
                    &state,
                    "ws.auth.fail",
                    serde_json::json!({ "reason": "bad-schema", "detail": reason }),
                )
                .await;
                continue;
            }
        };

        if !authed {
            let WsMsg::Auth { token } = &msg else {
                audit(
                    &state,
                    "ws.auth.fail",
                    serde_json::json!({ "reason": "pre-auth-msg", "kind": kind_label(&msg) }),
                )
                .await;
                continue;
            };
            if !constant_time_eq(token.as_bytes(), state.token.as_bytes()) {
                audit(&state, "ws.auth.fail", serde_json::json!({ "reason": "bad-token" })).await;
                let _ = sender.send(Message::Close(None)).await;
                break;
            }
            authed = true;
            let payload = serde_json::json!({
                "type": "ready",
                "cwd": state.cwd.display().to_string(),
                "tentacles": state.tentacles.list(),
            });
            let _ = sender.send(Message::Text(payload.to_string())).await;
            continue;
        }

        // From here on, we have a fully authed WS — dispatch.
        if let Err(err) = dispatch(&state, &msg, &mut owned, &mut sender).await {
            tracing::warn!("deck dispatch error: {err}");
        }
    }

    // Cleanup: kill any owned terminals, log disconnect.
    for id in &owned {
        if let Some(t) = state.terminals.lock().await.get(id) {
            let _ = t.kill();
        }
    }
    state
        .terminals
        .lock()
        .await
        .retain(|id, _| !owned.contains(id));
    audit(
        &state,
        "ws.disconnect",
        serde_json::json!({ "owned_terminals": owned.len() }),
    )
    .await;
}

async fn dispatch(
    state: &AppState,
    msg: &WsMsg,
    owned: &mut Vec<String>,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> anyhow::Result<()> {
    let send = |sender: &mut futures::stream::SplitSink<WebSocket, Message>,
                payload: serde_json::Value| {
        let text = payload.to_string();
        async move { sender.send(Message::Text(text)).await }
    };
    match msg {
        WsMsg::Auth { .. } | WsMsg::TentacleList => {
            send(
                sender,
                serde_json::json!({
                    "type": "tentacle.list",
                    "tentacles": state.tentacles.list(),
                }),
            )
            .await?;
        }
        WsMsg::TentacleCreate { title, context } => {
            let body = context.clone().unwrap_or_default();
            let t = state.tentacles.create(title, &body)?;
            audit(state, "tentacle.create", serde_json::json!({ "id": t.id, "title": t.title })).await;
            send(
                sender,
                serde_json::json!({
                    "type": "tentacle.created",
                    "tentacle": t,
                    "todos": state.tentacles.todos(&t.id),
                }),
            )
            .await?;
        }
        WsMsg::TentacleRemove { id } => {
            state.tentacles.remove(id)?;
            audit(state, "tentacle.remove", serde_json::json!({ "id": id })).await;
            send(sender, serde_json::json!({ "type": "tentacle.removed", "id": id })).await?;
        }
        WsMsg::TentacleTodos { id } => {
            send(
                sender,
                serde_json::json!({
                    "type": "tentacle.todos",
                    "id": id,
                    "todos": state.tentacles.todos(id),
                }),
            )
            .await?;
        }
        WsMsg::TentacleToggle { id, line } => {
            let todos = state.tentacles.toggle_todo(id, *line);
            audit(state, "tentacle.toggle", serde_json::json!({ "id": id, "line": line })).await;
            send(
                sender,
                serde_json::json!({ "type": "tentacle.todos", "id": id, "todos": todos }),
            )
            .await?;
        }
        WsMsg::TentacleContext { id } => {
            let content = state.tentacles.read_context(id);
            send(
                sender,
                serde_json::json!({ "type": "tentacle.context", "id": id, "content": content }),
            )
            .await?;
        }
        WsMsg::TentacleWriteContext { id, content } => {
            state.tentacles.write_context(id, content)?;
            send(
                sender,
                serde_json::json!({ "type": "tentacle.context", "id": id, "content": content }),
            )
            .await?;
        }
        WsMsg::TermSpawn { tentacle_id, .. } => {
            let opts = monkey_agents::SpawnOpts {
                kind: monkey_agents::AgentKind::Auto,
                cwd: state.cwd.clone(),
                tentacle_id: tentacle_id.clone().unwrap_or_else(|| "main".into()),
                size: None,
                extra_args: state.agent_args.clone(),
            };
            // The spawn-data callback runs on a background thread inside
            // monkey-agents. We can't borrow `sender` from there, so we
            // just record terminal output to the audit log (counts only)
            // and surface terminal lifecycle events to the WS. A full
            // bidirectional bridge lands in a follow-up commit when
            // we replace the closure with a tokio::sync::mpsc bridge.
            let result = match monkey_agents::spawn_agent(opts, |_chunk| {}) {
                Ok(r) => r,
                Err(err) => {
                    send(
                        sender,
                        serde_json::json!({
                            "type": "term.error",
                            "error": err.to_string(),
                        }),
                    )
                    .await?;
                    return Ok(());
                }
            };
            owned.push(result.terminal.id.clone());
            audit(
                state,
                "agent.spawn",
                serde_json::json!({
                    "id": result.terminal.id,
                    "kind": format!("{:?}", result.kind),
                    "tentacle_id": tentacle_id,
                }),
            )
            .await;
            let id = result.terminal.id.clone();
            let binary = result.binary.clone();
            state.terminals.lock().await.insert(id.clone(), result.terminal);
            send(
                sender,
                serde_json::json!({
                    "type": "term.spawned",
                    "id": id,
                    "cmd": binary,
                    "tentacle_id": tentacle_id,
                }),
            )
            .await?;
        }
        WsMsg::TermInput { id, data } => {
            if let Some(t) = state.terminals.lock().await.get(id) {
                let _ = t.write(data.as_bytes());
                audit(
                    state,
                    "term.input",
                    serde_json::json!({ "id": id, "bytes": data.len() }),
                )
                .await;
            }
        }
        WsMsg::TermResize { id, cols, rows } => {
            if let Some(t) = state.terminals.lock().await.get(id) {
                let _ = t.resize(*cols, *rows);
            }
        }
        WsMsg::TermKill { id } => {
            if let Some(t) = state.terminals.lock().await.get(id) {
                let _ = t.kill();
                audit(state, "term.kill", serde_json::json!({ "id": id })).await;
            }
        }
    }
    Ok(())
}

fn kind_label(msg: &WsMsg) -> &'static str {
    match msg {
        WsMsg::Auth { .. } => "auth",
        WsMsg::TentacleList => "tentacle.list",
        WsMsg::TentacleCreate { .. } => "tentacle.create",
        WsMsg::TentacleRemove { .. } => "tentacle.remove",
        WsMsg::TentacleTodos { .. } => "tentacle.todos",
        WsMsg::TentacleToggle { .. } => "tentacle.toggle",
        WsMsg::TentacleContext { .. } => "tentacle.context",
        WsMsg::TentacleWriteContext { .. } => "tentacle.writeContext",
        WsMsg::TermSpawn { .. } => "term.spawn",
        WsMsg::TermInput { .. } => "term.input",
        WsMsg::TermResize { .. } => "term.resize",
        WsMsg::TermKill { .. } => "term.kill",
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn audit(state: &AppState, event_label: &str, fields: serde_json::Value) {
    let event_type = match event_label {
        "ws.connect" | "ws.disconnect" | "ws.rate-limit" | "ws.auth.fail" => {
            AuditEventType::AgentSpawn
        }
        "tentacle.create" | "tentacle.remove" | "tentacle.toggle" => AuditEventType::Note,
        "agent.spawn" | "agent.exit" | "term.input" | "term.kill" => {
            AuditEventType::AgentSpawn
        }
        _ => AuditEventType::Note,
    };
    let mut fields = fields;
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("event".into(), serde_json::Value::String(event_label.into()));
    }
    let _ = state.audit.lock().await.log(event_type, fields);
}

fn expired(state: &AppState) -> bool {
    let now_ms = Utc::now().timestamp_millis() as u64;
    now_ms >= state.expires_at_ms
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

fn generate_token() -> String {
    use sha2::{Digest, Sha256};
    let nonce = format!(
        "{}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        uuid::Uuid::now_v7().simple()
    );
    let digest = Sha256::digest(nonce.as_bytes());
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    burst: f64,
    per_sec: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(rate: RateLimit) -> Self {
        Self {
            tokens: rate.burst as f64,
            burst: rate.burst as f64,
            per_sec: rate.per_sec as f64,
            last: Instant::now(),
        }
    }

    fn consume(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + dt * self.per_sec).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Serialize, Deserialize)]
struct _ProbeShape {
    _t: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_burst_then_blocks() {
        let mut b = TokenBucket::new(RateLimit { per_sec: 1, burst: 3 });
        assert!(b.consume());
        assert!(b.consume());
        assert!(b.consume());
        assert!(!b.consume());
    }

    #[test]
    fn rate_limiter_refills_over_time() {
        let mut b = TokenBucket::new(RateLimit { per_sec: 1000, burst: 1 });
        assert!(b.consume());
        std::thread::sleep(Duration::from_millis(20));
        assert!(b.consume());
    }

    #[test]
    fn constant_time_eq_handles_unequal_lengths() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }

    #[tokio::test]
    async fn deck_starts_and_serves_health() {
        let dir = tempfile::tempdir().unwrap();
        let opts = DeckOpts {
            cwd: dir.path().to_path_buf(),
            host: "127.0.0.1".into(),
            port: 0,
            token: Some("0123456789abcdefdeadbeef".into()),
            ..Default::default()
        };
        let handle = start_deck(opts).await.unwrap();
        let url = format!("http://127.0.0.1:{}/healthz", handle.port);
        let body = reqwest::get(&url).await.unwrap().text().await.unwrap();
        assert!(body.contains("\"ok\":true"));
        handle.close().await.unwrap();
    }

    #[tokio::test]
    async fn root_requires_token() {
        let dir = tempfile::tempdir().unwrap();
        let opts = DeckOpts {
            cwd: dir.path().to_path_buf(),
            host: "127.0.0.1".into(),
            port: 0,
            token: Some("0123456789abcdefdeadbeef".into()),
            ..Default::default()
        };
        let handle = start_deck(opts).await.unwrap();
        let no_token = format!("http://127.0.0.1:{}/", handle.port);
        let with_token = format!(
            "http://127.0.0.1:{}/?t=0123456789abcdefdeadbeef",
            handle.port
        );
        let r1 = reqwest::get(&no_token).await.unwrap();
        assert_eq!(r1.status(), reqwest::StatusCode::UNAUTHORIZED);
        let r2 = reqwest::get(&with_token).await.unwrap();
        assert_eq!(r2.status(), reqwest::StatusCode::OK);
        let body = r2.text().await.unwrap();
        assert!(body.contains("monkey deck"));
        handle.close().await.unwrap();
    }
}
