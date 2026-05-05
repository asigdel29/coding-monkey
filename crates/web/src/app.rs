/*
   File: crates/web/src/app.rs

   Purpose
   Leptos shell for the deck UI. Three-pane layout:

       left rail   — monkey-icon list of tentacles (replaces the
                     octogent octopus across every entry)
       center      — terminal tabs (one xterm.js per spawned PTY)
       right rail  — CONTEXT.md and todo.md editor for the active
                     tentacle

   The shell owns:
     - DeckClient (WS to the server, with auto-reconnect)
     - Tentacles signal (snapshot from `tentacle.list` + create/remove deltas)
     - Terminal map (id → xterm.Terminal handle)
     - Active tentacle id
     - Connection-state banner

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  full leptos shell + WS dispatch + xterm mount
                                 + monkey pixel icons replacing octogent
*/

use leptos::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::icon::{MONKEY_SVG_16, MONKEY_SVG_32};
use crate::ws::{
    token_from_query, ws_url_from_location, ClientMsg, ConnState, DeckClient, ServerMsg, Tentacle,
    Todo,
};
use crate::xterm::{default_terminal_options, Terminal};

/// Top-level component. Mounted by `lib.rs::main`.
#[component]
pub fn App() -> impl IntoView {
    // ── Shared state ────────────────────────────────────────────────────────
    let token = token_from_query().unwrap_or_default();
    let url = ws_url_from_location();

    let client = DeckClient::new(url, token.clone());
    let state = client.state;

    let (cwd, set_cwd) = signal(String::new());
    let (tentacles, set_tentacles) = signal(Vec::<Tentacle>::new());
    let (active_id, set_active_id) = signal(String::new());
    let (todos, set_todos) = signal(Vec::<Todo>::new());
    let (context_body, set_context_body) = signal(String::new());

    // Open terminal tabs (id → metadata for the tab strip).
    let (tabs, set_tabs) = signal(Vec::<TerminalTab>::new());
    let (active_tab, set_active_tab) = signal(String::new());

    // Native xterm handles, keyed by terminal id. Lives outside leptos
    // signals because the JS objects are not `Send` and we don't want
    // them in the reactive graph anyway.
    let terms: Rc<RefCell<HashMap<String, Terminal>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // ── Inbound WS dispatch ─────────────────────────────────────────────────
    {
        let terms = Rc::clone(&terms);
        let client_for_terms = client.clone();
        client.on_message(move |msg| match msg {
            ServerMsg::Ready { cwd: c, tentacles: t } => {
                set_cwd.set(c);
                set_tentacles.set(t);
            }
            ServerMsg::TentacleList { tentacles: t } => set_tentacles.set(t),
            ServerMsg::TentacleCreated { tentacle, todos: t } => {
                set_tentacles.update(|v| {
                    if !v.iter().any(|x| x.id == tentacle.id) {
                        v.insert(0, tentacle.clone());
                    }
                });
                set_active_id.set(tentacle.id.clone());
                set_todos.set(t);
            }
            ServerMsg::TentacleRemoved { id } => {
                set_tentacles.update(|v| v.retain(|t| t.id != id));
                if active_id.get_untracked() == id {
                    set_active_id.set(String::new());
                    set_todos.set(Vec::new());
                    set_context_body.set(String::new());
                }
            }
            ServerMsg::TentacleTodos { id, todos: t } => {
                if id == active_id.get_untracked() {
                    set_todos.set(t);
                }
            }
            ServerMsg::TentacleContext { id, content } => {
                if id == active_id.get_untracked() {
                    set_context_body.set(content);
                }
            }
            ServerMsg::TermSpawned { id, cmd, tentacle_id } => {
                let tab = TerminalTab {
                    id: id.clone(),
                    label: cmd
                        .clone()
                        .unwrap_or_else(|| "agent".into()),
                    tentacle_id,
                };
                set_tabs.update(|v| v.push(tab));
                set_active_tab.set(id.clone());
                mount_terminal(&id, &terms, &client_for_terms);
            }
            ServerMsg::TermData { id, data } => {
                if let Some(term) = terms.borrow().get(&id) {
                    term.write(&data);
                }
            }
            ServerMsg::TermExit { id, code } => {
                if let Some(term) = terms.borrow_mut().remove(&id) {
                    term.dispose();
                }
                set_tabs.update(|v| v.retain(|t| t.id != id));
                if active_tab.get_untracked() == id {
                    set_active_tab.set(String::new());
                }
                let msg = format!("[exit {code}]");
                web_sys::console::log_1(&msg.into());
            }
            ServerMsg::Error { error } => {
                web_sys::console::error_1(
                    &format!("deck error: {}", error.unwrap_or_default()).into(),
                );
            }
        });
    }
    client.connect();

    // When the active tentacle changes, fetch its context + todos.
    let client_for_select = client.clone();
    Effect::new(move |_| {
        let id = active_id.get();
        if id.is_empty() {
            return;
        }
        client_for_select.send(&ClientMsg::TentacleContext { id: id.clone() });
        client_for_select.send(&ClientMsg::TentacleTodos { id });
    });

    // ── Action callbacks ────────────────────────────────────────────────────
    let client_create = client.clone();
    let create_tentacle = move |_| {
        let title = prompt_for("New tentacle title").unwrap_or_default();
        if title.trim().is_empty() {
            return;
        }
        client_create.send(&ClientMsg::TentacleCreate {
            title: title.trim().into(),
            context: None,
        });
    };

    let client_spawn = client.clone();
    let spawn_terminal = move |_| {
        let tentacle = active_id.get_untracked();
        let tid = if tentacle.is_empty() { None } else { Some(tentacle) };
        client_spawn.send(&ClientMsg::TermSpawn {
            tentacle_id: tid,
            cols: Some(120),
            rows: Some(34),
        });
    };

    let client_save_ctx = client.clone();
    let save_context = move |_| {
        let id = active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        client_save_ctx.send(&ClientMsg::TentacleWriteContext {
            id,
            content: context_body.get_untracked(),
        });
    };

    // Wrap the toggle action in leptos::Callback so the For-children
    // closure can clone it once per row.
    let client_toggle = client.clone();
    let toggle_todo: Callback<u32> = Callback::new(move |line: u32| {
        let id = active_id.get_untracked();
        if id.is_empty() {
            return;
        }
        client_toggle.send(&ClientMsg::TentacleToggle { id, line });
    });

    // ── Render ─────────────────────────────────────────────────────────────
    view! {
        <div class="deck-root">
            <ConnectionBanner state=state />
            <header class="deck-header">
                <div class="brand">
                    <div class="brand-icon" inner_html=MONKEY_SVG_32></div>
                    <div class="brand-text">
                        <div class="brand-title">"monkey deck"</div>
                        <div class="brand-cwd">{move || cwd.get()}</div>
                    </div>
                </div>
                <div class="header-actions">
                    <button class="btn primary" on:click=spawn_terminal>"+ terminal"</button>
                </div>
            </header>

            <main class="deck-main">
                <aside class="left-rail">
                    <div class="rail-head">
                        <span>"tentacles"</span>
                        <button class="btn" on:click=create_tentacle>"+"</button>
                    </div>
                    <ul class="tentacle-list">
                        <For
                            each=move || tentacles.get()
                            key=|t| t.id.clone()
                            children=move |t| {
                                let id = t.id.clone();
                                let id_for_class = id.clone();
                                let id_for_click = id.clone();
                                let title = t.title.clone();
                                view! {
                                    <li
                                        class:active=move || active_id.get() == id_for_class
                                        on:click=move |_| set_active_id.set(id_for_click.clone())
                                    >
                                        <span class="tent-icon" inner_html=MONKEY_SVG_16></span>
                                        <span class="tent-title">{title}</span>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </aside>

                <section class="center-pane">
                    <div class="tab-strip">
                        <For
                            each=move || tabs.get()
                            key=|t| t.id.clone()
                            children=move |t| {
                                let id = t.id.clone();
                                let id_for_class = id.clone();
                                let id_for_click = id.clone();
                                let label = t.label.clone();
                                view! {
                                    <button
                                        class="tab"
                                        class:active=move || active_tab.get() == id_for_class
                                        on:click=move |_| set_active_tab.set(id_for_click.clone())
                                    >
                                        <span class="tab-icon" inner_html=MONKEY_SVG_16></span>
                                        <span>{label}</span>
                                    </button>
                                }
                            }
                        />
                    </div>
                    <div class="terminal-host" id="terminal-host">
                        // Terminals are mounted here imperatively when
                        // `term.spawned` arrives. The active tab swaps
                        // .visible on its container in `mount_terminal`.
                    </div>
                </section>

                <aside class="right-rail">
                    <div class="rail-head">"context"</div>
                    <textarea
                        class="ctx-editor"
                        prop:value=move || context_body.get()
                        on:input=move |ev| set_context_body.set(event_target_value(&ev))
                        placeholder="CONTEXT.md (saved on blur)"
                    />
                    <button class="btn" on:click=save_context>"save"</button>

                    <div class="rail-head">"todo"</div>
                    <ul class="todo-list">
                        <For
                            each=move || todos.get()
                            key=|t| t.line
                            children=move |t| {
                                let line = t.line;
                                let on_toggle = toggle_todo;
                                view! {
                                    <li class:done=t.done>
                                        <input
                                            type="checkbox"
                                            prop:checked=t.done
                                            on:change=move |_| on_toggle.run(line)
                                        />
                                        <span>{t.text}</span>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </aside>
            </main>
        </div>
    }
}

#[component]
fn ConnectionBanner(state: RwSignal<ConnState>) -> impl IntoView {
    view! {
        {move || match state.get() {
            ConnState::Ready => view! { <div class="banner banner-ok">"connected"</div> }.into_any(),
            ConnState::Connecting => view! { <div class="banner banner-info">"connecting…"</div> }.into_any(),
            ConnState::Reconnecting => view! { <div class="banner banner-warn">"reconnecting…"</div> }.into_any(),
            ConnState::AuthFailed => view! { <div class="banner banner-err">"auth failed — refresh from the deck startup output"</div> }.into_any(),
        }}
    }
}

#[derive(Clone)]
struct TerminalTab {
    id: String,
    label: String,
    #[allow(dead_code)]
    tentacle_id: Option<String>,
}

fn prompt_for(message: &str) -> Option<String> {
    web_sys::window()?
        .prompt_with_message(message)
        .ok()
        .flatten()
}

/// Imperatively mount an xterm.js Terminal into `#terminal-host` and
/// wire its `onData` handler to the WS. The container element gets a
/// `data-term-id` attribute so a future tab-switch can toggle visibility.
fn mount_terminal(id: &str, terms: &Rc<RefCell<HashMap<String, Terminal>>>, client: &DeckClient) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(host) = doc.get_element_by_id("terminal-host") else { return };

    let pane = match doc.create_element("div") {
        Ok(p) => p,
        Err(_) => return,
    };
    let _ = pane.set_attribute("class", "term-pane");
    let _ = pane.set_attribute("data-term-id", id);
    let _ = host.append_child(&pane);

    let opts = default_terminal_options(120, 34);
    let term = Terminal::new_with_opts(&opts);
    term.open(&pane);
    term.focus();

    // Wire onData → ws.send(TermInput).
    let id_owned = id.to_string();
    let client_for_input = client.clone();
    let cb = Closure::wrap(Box::new(move |s: String| {
        client_for_input.send(&ClientMsg::TermInput {
            id: id_owned.clone(),
            data: s,
        });
    }) as Box<dyn FnMut(String)>);
    let _ = term.on_data(&cb);
    cb.forget();

    terms.borrow_mut().insert(id.to_string(), term);
}

fn event_target_value(ev: &web_sys::Event) -> String {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}
