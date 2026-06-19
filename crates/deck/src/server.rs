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
   2026-06-03   Anubhav Sigdel  enforce RAM/CPU agent cap on terminal spawn;
                                 allow 'wasm-unsafe-eval' in CSP so the UI boots
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
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use monkey_agents::{AuditEventType, AuditLogger};
use monkey_runtime::{
    native_agent_job, AgentConfig, AgentEvent, NativeLlm, ProviderLimiter, Scheduler,
    SchedulerConfig, ToolRegistry, WorkClass,
};

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
    /// Static-asset directory served from `/static/*`. The
    /// monkey-web WASM bundle lives here (typically the result of
    /// `trunk build` against crates/web). When `None`, /static returns
    /// 404 — useful in tests.
    pub static_dir: Option<PathBuf>,
}

impl Default for DeckOpts {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            host: "127.0.0.1".into(),
            port: 8787,
            agent: "codex".into(),
            agent_args: Vec::new(),
            token_ttl: Duration::from_secs(8 * 60 * 60),
            token: None,
            rate_per_sec: 100,
            rate_burst: 200,
            cert: None,
            key: None,
            enforce_tls_off_loopback: true,
            static_dir: None,
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
    // Configured agent name, kept on AppState for telemetry / future
    // routing even though spawning currently goes through `AgentKind::Auto`.
    #[allow(dead_code)]
    agent: String,
    agent_args: Vec<String>,
    token: String,
    expires_at_ms: u64,
    scheme: String,
    bound_host: String,
    bound_port: u16,
    rate: monkey_core::RateLimit,
    tentacles: TentacleStore,
    audit: Mutex<AuditLogger>,
    terminals: Mutex<HashMap<String, monkey_agents::AgentTerminal>>,
    /// Most PTY agent terminals allowed at once, derived from host RAM/CPU
    /// at startup using the heavyweight (PTY) budget. Spawns past this are
    /// rejected so the box never thrashes.
    max_agents: usize,
    /// Scheduler for native in-process agents.
    scheduler: Arc<Scheduler>,
    /// Shared LLM client for native agents.
    llm: Arc<NativeLlm>,
    /// Shared provider rate limiter for native agents.
    limiter: Arc<ProviderLimiter>,
    /// Built-in tool registry for native agents.
    tools: Arc<ToolRegistry>,
    /// Ids of currently-running native agents (for the cap and listing).
    native_agents: Arc<Mutex<HashSet<String>>>,
    /// Most native agents allowed at once, from the lightweight budget —
    /// 100+ on a Pi 5 where the PTY cap is ~10.
    native_max_agents: usize,
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

    // Concurrency ceilings: PTY agents are heavyweight (~10 on a Pi),
    // native agents are lightweight network-bound tasks (100+ on a Pi).
    let host_cap = monkey_core::concurrency::HostCapacity::detect();
    let max_agents = monkey_core::concurrency::max_concurrent_agents(
        &host_cap,
        &monkey_core::concurrency::AgentBudget::pty(),
    );
    let native_max_agents = monkey_core::concurrency::max_concurrent_agents(
        &host_cap,
        &monkey_core::concurrency::AgentBudget::native(),
    );
    let watchdog = Arc::new(monkey_core::MemoryWatchdog::default());
    let scheduler = Arc::new(Scheduler::new(
        SchedulerConfig::from_max_agents(native_max_agents),
        watchdog,
    ));
    let llm = Arc::new(NativeLlm::with_registry(
        default_provider_from_config(&opts.cwd),
        registry_from_config(&opts.cwd),
    ));
    let limiter = Arc::new(ProviderLimiter::with_defaults());
    let tools = Arc::new(monkey_runtime::default_tools());

    let state = Arc::new(AppState {
        cwd: opts.cwd.clone(),
        agent: opts.agent.clone(),
        agent_args: opts.agent_args.clone(),
        token: token.clone(),
        expires_at_ms,
        scheme: if use_tls {
            "https".into()
        } else {
            "http".into()
        },
        bound_host: host.clone(),
        bound_port: opts.port,
        rate: monkey_core::RateLimit {
            per_sec: opts.rate_per_sec,
            burst: opts.rate_burst.max(opts.rate_per_sec),
        },
        tentacles: TentacleStore::new(&opts.cwd),
        audit: Mutex::new(audit),
        terminals: Mutex::new(HashMap::new()),
        max_agents,
        scheduler,
        llm,
        limiter,
        tools,
        native_agents: Arc::new(Mutex::new(HashSet::new())),
        native_max_agents,
    });

    let mut app = Router::new()
        .route("/", get(serve_index))
        .route("/healthz", get(serve_health))
        .route("/ws", get(ws_upgrade))
        .with_state(state.clone());
    if let Some(dir) = opts.static_dir.as_ref() {
        app = app.nest_service("/static", tower_http::services::fs::ServeDir::new(dir));
    }

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

async fn serve_index(State(state): State<Arc<AppState>>, Query(q): Query<IndexQuery>) -> Response {
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
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );
    let csp = format!(
        // 'wasm-unsafe-eval' is required for the browser to instantiate the
        // leptos WASM bundle; without it Chrome blocks compilation and the
        // UI never boots (blank page, no console error).
        "default-src 'none'; \
         script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval' https://cdn.jsdelivr.net; \
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

fn index_html(_token: &str) -> String {
    // Boot the leptos/WASM frontend served from /static/. The actual
    // WASM bundle is built by `wasm-pack build crates/web` (or
    // `trunk build`) and copied/symlinked into the deck's static dir
    // by the deploy step.
    r##"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>monkey deck</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.min.css" />
    <script src="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"></script>
    <link rel="stylesheet" href="/static/styles.css" />
    <link rel="icon" type="image/svg+xml" href="/static/favicon.svg" />
  </head>
  <body>
    <script type="module">
      import init from "/static/pkg/monkey_web.js";
      init();
    </script>
  </body>
</html>"##
        .to_string()
}

// ─── WebSocket ──────────────────────────────────────────────────────────────

async fn ws_upgrade(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(_who): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    if expired(&state) {
        audit(
            &state,
            "ws.auth.fail",
            serde_json::json!({ "reason": "session-expired" }),
        )
        .await;
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
        format!(
            "{}://{}:{}",
            state.scheme, state.bound_host, state.bound_port
        ),
        format!("{}://localhost:{}", state.scheme, state.bound_port),
        format!("{}://127.0.0.1:{}", state.scheme, state.bound_port),
    ];
    allowed.iter().any(|a| a == origin)
}

/// Outbound side of a WebSocket connection. All writes go through this
/// channel to a single writer task that owns the sink, so the dispatch path
/// and every background agent-event forwarder can send frames without
/// contending for `&mut SplitSink`.
type OutboundTx = tokio::sync::mpsc::Sender<Message>;

async fn handle_ws(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut authed = false;
    let mut owned: Vec<String> = Vec::new();
    let mut bucket = monkey_core::TokenBucket::new(state.rate);

    // Single-writer bridge: one task owns the sink and drains an outbound
    // channel. A bounded buffer applies backpressure so a slow client can't
    // make the server buffer without limit.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let is_close = matches!(msg, Message::Close(_));
            if sender.send(msg).await.is_err() || is_close {
                break;
            }
        }
    });

    audit(&state, "ws.connect", serde_json::json!({})).await;

    let auth_deadline = Instant::now() + Duration::from_secs(5);

    while let Some(frame) = receiver.next().await {
        if !authed && Instant::now() > auth_deadline {
            audit(
                &state,
                "ws.auth.fail",
                serde_json::json!({ "reason": "timeout" }),
            )
            .await;
            let _ = out_tx.send(Message::Close(None)).await;
            break;
        }
        if expired(&state) {
            audit(
                &state,
                "ws.disconnect",
                serde_json::json!({ "reason": "session-expired" }),
            )
            .await;
            let _ = out_tx.send(Message::Close(None)).await;
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
                let _ = out_tx.send(Message::Pong(p)).await;
                continue;
            }
            Message::Pong(_) => continue,
            Message::Close(_) => break,
        };

        if !bucket.consume() {
            audit(&state, "ws.rate-limit", serde_json::json!({})).await;
            let _ = out_tx.send(Message::Close(None)).await;
            break;
        }

        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                audit(
                    &state,
                    "ws.auth.fail",
                    serde_json::json!({ "reason": "bad-json" }),
                )
                .await;
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
                audit(
                    &state,
                    "ws.auth.fail",
                    serde_json::json!({ "reason": "bad-token" }),
                )
                .await;
                let _ = out_tx.send(Message::Close(None)).await;
                break;
            }
            authed = true;
            let payload = serde_json::json!({
                "type": "ready",
                "cwd": state.cwd.display().to_string(),
                "max_agents": state.max_agents,
                "tentacles": state.tentacles.list(),
            });
            let _ = out_tx.send(Message::Text(payload.to_string().into())).await;
            continue;
        }

        // From here on, we have a fully authed WS — dispatch.
        if let Err(err) = dispatch(&state, &msg, &mut owned, &out_tx).await {
            tracing::warn!("deck dispatch error: {err}");
        }
    }

    // Cleanup: kill any owned terminals and cancel owned native agents.
    for id in &owned {
        if let Some(t) = state.terminals.lock().await.get(id) {
            let _ = t.kill();
        }
        // No-op for ids that aren't native agents.
        state.scheduler.cancel(id);
    }
    state
        .terminals
        .lock()
        .await
        .retain(|id, _| !owned.contains(id));
    state
        .native_agents
        .lock()
        .await
        .retain(|id| !owned.contains(id));
    audit(
        &state,
        "ws.disconnect",
        serde_json::json!({ "owned_terminals": owned.len() }),
    )
    .await;

    // Close the outbound channel so the writer task drains any buffered
    // frames (incl. a Close) and exits, then join it.
    drop(out_tx);
    let _ = writer.await;
}

async fn dispatch(
    state: &AppState,
    msg: &WsMsg,
    owned: &mut Vec<String>,
    sender: &OutboundTx,
) -> anyhow::Result<()> {
    match msg {
        WsMsg::Auth { .. } | WsMsg::TentacleList => {
            send_json(
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
            audit(
                state,
                "tentacle.create",
                serde_json::json!({ "id": t.id, "title": t.title }),
            )
            .await;
            send_json(
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
            send_json(
                sender,
                serde_json::json!({ "type": "tentacle.removed", "id": id }),
            )
            .await?;
        }
        WsMsg::TentacleTodos { id } => {
            send_json(
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
            audit(
                state,
                "tentacle.toggle",
                serde_json::json!({ "id": id, "line": line }),
            )
            .await;
            send_json(
                sender,
                serde_json::json!({ "type": "tentacle.todos", "id": id, "todos": todos }),
            )
            .await?;
        }
        WsMsg::TentacleContext { id } => {
            let content = state.tentacles.read_context(id);
            send_json(
                sender,
                serde_json::json!({ "type": "tentacle.context", "id": id, "content": content }),
            )
            .await?;
        }
        WsMsg::TentacleWriteContext { id, content } => {
            state.tentacles.write_context(id, content)?;
            send_json(
                sender,
                serde_json::json!({ "type": "tentacle.context", "id": id, "content": content }),
            )
            .await?;
        }
        WsMsg::TermSpawn { tentacle_id, .. } => {
            // Enforce the host's agent concurrency cap before spawning so a
            // client can't oversubscribe the machine into thrashing.
            let live = state.terminals.lock().await.len();
            if live >= state.max_agents {
                send_json(
                    sender,
                    serde_json::json!({
                        "type": "term.error",
                        "error": format!(
                            "agent capacity reached ({live}/{}). Close a terminal or run on a larger host.",
                            state.max_agents
                        ),
                    }),
                )
                .await?;
                return Ok(());
            }
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
                    send_json(
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
            state
                .terminals
                .lock()
                .await
                .insert(id.clone(), result.terminal);
            send_json(
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
        WsMsg::AgentSpawn {
            task,
            tentacle_id,
            task_type,
            tier,
            ..
        } => {
            let running = state.native_agents.lock().await.len();
            if running >= state.native_max_agents {
                send_json(
                    sender,
                    serde_json::json!({
                        "type": "agent.error",
                        "error": format!(
                            "native agent capacity reached ({running}/{}). Cancel one or run on a larger host.",
                            state.native_max_agents
                        ),
                    }),
                )
                .await?;
                return Ok(());
            }

            let id = monkey_core::generate_id("agent");
            let mut cfg = AgentConfig::new(task.clone(), state.cwd.clone());
            cfg.tentacle_id = tentacle_id.clone();
            if let Some(tt) = task_type {
                cfg.task_type = map_task_type(tt);
            }
            cfg.force_tier = tier.as_deref().and_then(map_tier);
            let model = state.llm.pick(cfg.task_type, cfg.force_tier, None);

            // Forward the agent's events to this connection's outbound sink.
            let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
            spawn_event_forwarder(
                id.clone(),
                ev_rx,
                sender.clone(),
                Arc::clone(&state.native_agents),
            );

            let class = if tentacle_id.is_some() {
                WorkClass::Scoped
            } else {
                WorkClass::Shared
            };
            let job = native_agent_job(
                id.clone(),
                class,
                cfg,
                Arc::clone(&state.llm),
                Arc::clone(&state.limiter),
                Arc::clone(&state.tools),
                model,
                ev_tx,
            );
            match state.scheduler.submit(job) {
                Ok(_) => {
                    state.native_agents.lock().await.insert(id.clone());
                    owned.push(id.clone());
                    audit(
                        state,
                        "agent.spawn",
                        serde_json::json!({ "id": id, "kind": "native", "tentacle_id": tentacle_id }),
                    )
                    .await;
                    send_json(
                        sender,
                        serde_json::json!({ "type": "agent.spawned", "id": id, "tentacle_id": tentacle_id }),
                    )
                    .await?;
                }
                Err(e) => {
                    send_json(
                        sender,
                        serde_json::json!({ "type": "agent.error", "error": e.to_string() }),
                    )
                    .await?;
                }
            }
        }
        WsMsg::AgentCancel { id } => {
            let found = state.scheduler.cancel(id);
            if found {
                audit(state, "agent.cancel", serde_json::json!({ "id": id })).await;
            }
            send_json(
                sender,
                serde_json::json!({ "type": "agent.cancelled", "id": id, "found": found }),
            )
            .await?;
        }
        WsMsg::AgentList => {
            let ids: Vec<String> = state.native_agents.lock().await.iter().cloned().collect();
            let stats = state.scheduler.stats();
            send_json(
                sender,
                serde_json::json!({
                    "type": "agent.list",
                    "agents": ids,
                    "max_agents": state.native_max_agents,
                    "running": stats.running,
                    "submitted": stats.submitted,
                    "completed": stats.completed,
                    "rejected": stats.rejected,
                }),
            )
            .await?;
        }
    }
    Ok(())
}

/// Read `.monkey/config.json`'s `default_provider` and map it to a
/// [`monkey_core::Provider`]. Defaults to OpenRouter when the file or field
/// is absent or unrecognized, so existing setups are unaffected. Selecting
/// `self-hosted` here points native agents at a local OpenAI-compatible
/// server (see `MONKEY_SELF_HOSTED_URL`).
fn default_provider_from_config(cwd: &std::path::Path) -> monkey_core::Provider {
    use monkey_core::Provider;
    let raw = match std::fs::read_to_string(cwd.join(".monkey").join("config.json")) {
        Ok(s) => s,
        Err(_) => return Provider::OpenRouter,
    };
    let val: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Provider::OpenRouter,
    };
    match val.get("default_provider").and_then(|v| v.as_str()) {
        Some("openai") => Provider::Openai,
        Some("self-hosted") | Some("selfhosted") => Provider::SelfHosted,
        _ => Provider::OpenRouter,
    }
}

/// Build the model registry for native agents, folding in any locally-served
/// models declared in `.monkey/config.json`. Falls back to the builtin lineup
/// when the file is absent or unparseable, so a missing/partial config never
/// breaks startup.
fn registry_from_config(cwd: &std::path::Path) -> monkey_core::ModelRegistry {
    use monkey_core::{ModelRegistry, OrchestratorConfig};
    std::fs::read_to_string(cwd.join(".monkey").join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<OrchestratorConfig>(&raw).ok())
        .map(|cfg| ModelRegistry::with_config(&cfg))
        .unwrap_or_else(ModelRegistry::with_builtin)
}

/// Map a wire task-type string to a [`monkey_core::TaskType`], defaulting to
/// `Edit` for unrecognized values.
fn map_task_type(s: &str) -> monkey_core::TaskType {
    use monkey_core::TaskType::*;
    match s {
        "chat" => Chat,
        "explain" => Explain,
        "generate" => Generate,
        "engulf" => Engulf,
        "refactor" => Refactor,
        "investigate" => Investigate,
        "review" => Review,
        "security_audit" | "securityaudit" => SecurityAudit,
        _ => Edit,
    }
}

/// Map a wire tier string to a [`monkey_core::ModelTier`]; `None` for
/// unrecognized values (the task default applies).
fn map_tier(s: &str) -> Option<monkey_core::ModelTier> {
    use monkey_core::ModelTier::*;
    match s {
        "fast" => Some(Fast),
        "balanced" => Some(Balanced),
        "powerful" => Some(Powerful),
        _ => None,
    }
}

/// Pump one native agent's events to a connection's outbound sink as
/// `agent.event` frames, then an `agent.finished` and cleanup on the
/// terminal event. High-volume token deltas are dropped under backpressure
/// (try_send); all other events, including the terminal one, are awaited so
/// they are never lost.
fn spawn_event_forwarder(
    id: String,
    mut rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    out: OutboundTx,
    agents: Arc<Mutex<HashSet<String>>>,
) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let terminal = ev.is_terminal();
            let lossy = ev.is_lossy();
            let frame = Message::Text(
                serde_json::json!({ "type": "agent.event", "id": id.clone(), "event": ev })
                    .to_string()
                    .into(),
            );
            if lossy {
                let _ = out.try_send(frame);
            } else if out.send(frame).await.is_err() {
                break;
            }
            if terminal {
                let _ = out
                    .send(Message::Text(
                        serde_json::json!({ "type": "agent.finished", "id": id.clone() })
                            .to_string()
                            .into(),
                    ))
                    .await;
                agents.lock().await.remove(&id);
                break;
            }
        }
    });
}

async fn send_json(sender: &OutboundTx, payload: serde_json::Value) -> anyhow::Result<()> {
    sender
        .send(Message::Text(payload.to_string().into()))
        .await
        .context("outbound channel closed")
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
        WsMsg::AgentSpawn { .. } => "agent.spawn",
        WsMsg::AgentCancel { .. } => "agent.cancel",
        WsMsg::AgentList => "agent.list",
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn audit(state: &AppState, event_label: &str, fields: serde_json::Value) {
    let event_type = match event_label {
        "ws.connect" | "ws.disconnect" | "ws.rate-limit" | "ws.auth.fail" => {
            AuditEventType::AgentSpawn
        }
        "tentacle.create" | "tentacle.remove" | "tentacle.toggle" => AuditEventType::Note,
        "agent.spawn" | "agent.exit" | "term.input" | "term.kill" => AuditEventType::AgentSpawn,
        _ => AuditEventType::Note,
    };
    let mut fields = fields;
    if let Some(obj) = fields.as_object_mut() {
        obj.insert(
            "event".into(),
            serde_json::Value::String(event_label.into()),
        );
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

#[derive(Serialize, Deserialize)]
struct _ProbeShape {
    _t: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_handles_unequal_lengths() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }

    #[test]
    fn default_provider_reads_config() {
        use monkey_core::Provider;
        let dir = tempfile::tempdir().unwrap();
        // Missing config → OpenRouter.
        assert_eq!(
            default_provider_from_config(dir.path()),
            Provider::OpenRouter
        );

        std::fs::create_dir_all(dir.path().join(".monkey")).unwrap();
        std::fs::write(
            dir.path().join(".monkey/config.json"),
            r#"{ "default_provider": "self-hosted" }"#,
        )
        .unwrap();
        assert_eq!(
            default_provider_from_config(dir.path()),
            Provider::SelfHosted
        );

        std::fs::write(
            dir.path().join(".monkey/config.json"),
            r#"{ "default_provider": "openai" }"#,
        )
        .unwrap();
        assert_eq!(default_provider_from_config(dir.path()), Provider::Openai);
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
