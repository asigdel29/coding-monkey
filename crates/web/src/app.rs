/*
   File: crates/web/src/app.rs

   Purpose
   Top-level leptos component. Three panes:
       left  — tentacle list
       main  — terminal tabs (xterm.js)
       right — CONTEXT.md + todo.md editor

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial scaffold; layout + signals
*/

use leptos::prelude::*;

/// Root component.
#[component]
pub fn App() -> impl IntoView {
    let (active_tentacle, set_active_tentacle) = signal(String::from("main"));
    view! {
        <div class="deck-root">
            <aside class="left-rail">
                <h2>"tentacles"</h2>
                <button on:click=move |_| set_active_tentacle.set("main".into())>
                    "main"
                </button>
                <p class="dim">{move || format!("active: {}", active_tentacle.get())}</p>
            </aside>
            <main class="terminal-area">
                <p>"terminal area — xterm.js mounts here"</p>
            </main>
            <aside class="right-rail">
                <h2>"context + todo"</h2>
                <textarea placeholder="CONTEXT.md"></textarea>
                <textarea placeholder="todo.md"></textarea>
            </aside>
        </div>
    }
}
