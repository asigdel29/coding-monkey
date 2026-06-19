/*
   File: crates/runtime/src/probe.rs

   Purpose
   A liveness check for a local OpenAI-compatible model endpoint. `monkey
   models --probe` and `monkey doctor` use it to tell the user whether a
   configured host (the Pi-local server, the LAN box) is actually reachable
   before an agent run depends on it. Any HTTP response — even 404/405 —
   proves the server is up; only a transport failure or timeout counts as down.

   History
   Date         Author          Changes
   2026-06-19   Anubhav Sigdel  initial — endpoint reachability probe
*/

use std::time::Duration;

/// Whether a model server is reachable at `base_url` within `timeout`.
///
/// Probes the server's `/v1/models` listing (derived from the same URL
/// normalization the chat client uses), treating any HTTP reply as "up" and
/// only a transport error or timeout as "down". Never panics — a malformed URL
/// or client build failure simply reports unreachable.
pub async fn endpoint_reachable(base_url: &str, timeout: Duration) -> bool {
    let url =
        monkey_core::normalize_self_hosted_url(base_url).replace("/chat/completions", "/models");
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return false;
    };
    client.get(url).send().await.is_ok()
}
