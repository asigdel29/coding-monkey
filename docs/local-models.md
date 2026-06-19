# Running coding-monkey on local open-weights models

coding-monkey routes native agents to open-weights models served on **your own
hardware** — no cloud, no per-token cost. Because the strongest open models do
not fit a Raspberry Pi, the recommended topology is **tiered**:

| Tier | Model (default) | Runs on | Role |
|------|-----------------|---------|------|
| Fast | Qwen2.5-Coder 3B (Q4) | the Pi itself | trivial work, offline-capable |
| Balanced | **GLM-5.2** | a LAN box | everyday coding (the default) |
| Powerful | **Kimi K2.6** | a LAN box | hardest reasoning / debugging |

The orchestrator scores each task's difficulty and picks the initial tier, then
**escalates** to the next-stronger model if a run fails or stalls (Pi → GLM-5.2
→ Kimi K2.6).

> **Why not all on the Pi?** GLM-5 (355B–744B params) and Kimi K2.6 (1T params)
> need roughly 135 GB and 350 GB of RAM even at 2-bit quantization — far beyond a
> Pi's 16 GB. The Pi runs a small model locally and orchestrates; the large
> models live on a capable box elsewhere.

> **LAN box or personal cloud?** The "LAN box" below is the simplest case — a
> capable machine on your home network. To run the large models off-site on a
> self-managed colo GPU server reached over a private VPN (with model-swapping
> to fit one box), follow [`cloud-deployment.md`](./cloud-deployment.md)
> instead; the only config difference is the `base_url` and `host: cloud`.

## 1. Serve a small model on the Pi (Fast tier)

Any OpenAI-compatible server works. With [Ollama](https://ollama.com):

```bash
# On the Raspberry Pi
ollama pull qwen2.5-coder:3b
ollama serve            # listens on http://localhost:11434
```

Or with `llama.cpp`'s server and a GGUF:

```bash
llama-server -m qwen2.5-coder-3b-q4_k_m.gguf --host 0.0.0.0 --port 11434
```

## 2. Serve GLM-5.2 and Kimi K2.6 on a LAN box (Balanced / Powerful)

On a machine with enough RAM/VRAM, serve each model on its own port with an
OpenAI-compatible endpoint. With vLLM:

```bash
# GLM-5.2 → port 8000
vllm serve zai-org/GLM-5.2 --port 8000

# Kimi K2.6 → port 8001
vllm serve moonshotai/Kimi-K2.6 --port 8001
```

`llama.cpp`, SGLang, and LM Studio expose the same `/v1/chat/completions` API
and work identically. Note the box's hostname/IP (e.g. `lan-box.local`).

## 3. Point `.monkey/config.json` at your hosts

`monkey init` scaffolds a local-first config. Edit the `base_url`s to match:

```jsonc
{
  "default_provider": "self-hosted",
  "default_model": "glm-5.2",
  "local_models": [
    { "id": "qwen2.5-coder-3b", "display_name": "Qwen2.5-Coder 3B (Pi-local)",
      "tier": "fast",     "base_url": "http://localhost:11434",     "context_window": 32768,  "host": "pi"  },
    { "id": "glm-5.2",          "display_name": "GLM-5.2 (LAN)",
      "tier": "balanced", "base_url": "http://lan-box.local:8000",  "context_window": 200000, "host": "lan" },
    { "id": "kimi-k2.6",        "display_name": "Kimi K2.6 (LAN)",
      "tier": "powerful", "base_url": "http://lan-box.local:8001",  "context_window": 256000, "host": "lan" }
  ],
  "orchestrator": {
    "strategy": "difficulty-escalation",
    "escalate_on": ["failed", "limit_reached"],
    "max_escalations": 1
  }
}
```

Each entry becomes a self-hosted model with **its own endpoint**, so the Pi
model and the LAN models coexist. A `base_url` may be a base (`http://host:port`)
or a full `/v1/chat/completions` URL. If a server needs an API key, set
`"api_key_env": "MY_KEY_VAR"` on that entry and export the variable.

## 4. Verify

```bash
monkey models --probe     # lists tier / host / endpoint, [up] or [down]
monkey doctor             # includes a "Local models" reachability section
```

Both probe each `base_url` with a short timeout; `[down]` means the server isn't
reachable from the Pi (wrong host/port, not running, or a firewall). Fix that
before running agents that depend on the LAN tier.

## Notes

- **Offline fallback.** Trivial tasks route to the Pi-local Fast model and run
  with no network. If the LAN box is down, harder tasks fail cleanly (the probe
  and the agent's error say why) rather than silently degrading.
- **Selection precedence.** An explicit per-task tier override wins; otherwise
  difficulty scoring picks the tier, and `default_model` is preferred at the
  Balanced/everyday tier.
- **Cost.** Local models are recorded at zero cost, so usage reports show `$0`.
- **Hosted fallback.** To use a hosted API instead, set `default_provider` to
  `openrouter` or `openai` and export the matching key.
