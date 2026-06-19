# coding-monkey

A single-binary coding-agent platform written in Rust. Drop `monkey` into any
repo, run a swarm of AI agents — from the terminal or a web dashboard — scaled
to whatever your machine can handle, against **hosted APIs or your own
open-weights models**.

No runtime, no GC, no Node. One `monkey` binary.

Run it **fully local and open-weights**: a small model on the Pi for trivial
work, **GLM-5.2** for everyday coding, and **Kimi K2.6** for the hardest tasks —
with an orchestrator that scores each task and escalates across the tiers. See
[**docs/local-models.md**](docs/local-models.md) to set it up, or bring your own
API key below.

```bash
# 1. Clone and build
git clone https://github.com/asigdel29/coding-monkey
cd coding-monkey
cargo install --path crates/cli

# 2. Bring your own key (one OpenRouter key reaches every major model)
export OPENROUTER_API_KEY=sk-or-v1-...     # or: export OPENAI_API_KEY=sk-...

# 3. Drop into a repo and go
cd your-project
monkey setup       # scaffold .monkey/, import existing prompts, run diagnostics
monkey deck        # open the dashboard — spawn native agents (100+ on a Pi 5)
```

`monkey setup` is the one-command on-ramp: it scaffolds `.monkey/`, imports any
agent prompts you already keep (`CLAUDE.md`, `AGENTS.md`, …), checks your keys
and host capacity, and tells you the next step.

---

## What it does

- **Bring-your-own-key, any model.** The LLM client speaks the OpenAI-compatible
  Chat Completions API, so OpenRouter (one key, hundreds of models) and OpenAI
  both work with zero config. Switch models by editing `.monkey/config.json`.
- **Local-first, open-weights option.** Run entirely on your own hardware: a
  small model on the Pi for trivial work, GLM-5.2 for everyday coding, Kimi K2.6
  for the hardest tasks. An orchestrator scores each task's difficulty, routes it
  to the right tier, and **escalates** to a stronger model when a run fails or
  stalls — each model reachable at its own endpoint.
- **Runs as many agents as your machine allows.** The deck probes free RAM and
  CPU at startup and caps concurrency so you never thrash the box. Check the
  number with `monkey doctor`.
- **A committed agent brain.** Everything under `.monkey/context/` is read as the
  system prompt for every agent, so your whole team shares the same context.
- **Generates that context for you.** `monkey engulf` scans the repo, detects the
  stack, runs a security audit, and writes a deployment runbook.
- **A web dashboard.** `monkey deck` runs multiple agent terminals in one browser
  tab, each scoped to a "tentacle" so parallel work doesn't collide.
- **A pre-push pentest gate.** `monkey pentest install-hook` blocks `git push` on
  a native-Rust security scan.

---

## Bring your own key

| Provider | Env var | Notes |
| --- | --- | --- |
| **OpenRouter** (default) | `OPENROUTER_API_KEY` | One key, many upstream models. Recommended. |
| **OpenAI** | `OPENAI_API_KEY` | Direct to OpenAI. |

### Local open-weights models & the orchestrator

Run `monkey` against open-weights models on your own hardware — served by
[Ollama](https://ollama.com), llama.cpp's `server`, [vLLM](https://github.com/vllm-project/vllm),
or LM Studio — with no API cost. Because the strongest open models don't fit a
Pi, the recommended shape is **tiered**, and each model has **its own endpoint**:

| Tier | Default model | Runs on | Role |
| --- | --- | --- | --- |
| Fast | Qwen2.5-Coder 3B | the Pi | trivial work, offline-capable |
| Balanced | **GLM-5.2** | a bigger box | everyday coding (the default) |
| Powerful | **Kimi K2.6** | a bigger box | hardest reasoning / debugging |

The orchestrator scores each task's difficulty, picks the initial tier, and
**escalates** to the next-stronger model if a run fails or stalls
(Pi → GLM-5.2 → Kimi K2.6). Declare the models in `.monkey/config.json` —
`monkey init` scaffolds this local-first:

```json
{
  "default_agent": "auto",
  "default_provider": "self-hosted",
  "default_tier": "balanced",
  "default_model": "glm-5.2",
  "fail_on": "high",
  "local_models": [
    { "id": "qwen2.5-coder-3b", "tier": "fast",     "base_url": "http://localhost:11434",    "context_window": 32768,  "host": "pi"  },
    { "id": "glm-5.2",          "tier": "balanced", "base_url": "http://lan-box.local:8000",  "context_window": 200000, "host": "lan" },
    { "id": "kimi-k2.6",        "tier": "powerful", "base_url": "http://lan-box.local:8001",  "context_window": 256000, "host": "lan" }
  ]
}
```

```bash
monkey models --probe   # lists tier / host / endpoint, [up] or [down]
monkey doctor           # includes a "Local models" reachability section
```

A single shared endpoint still works the old way (`MONKEY_SELF_HOSTED_URL` +
optional `MONKEY_SELF_HOSTED_KEY`); to use a hosted API instead, set
`default_provider` to `openrouter` or `openai` and export the matching key. See
the built-in lineup with `monkey models`.

- **Set it up:** [**docs/local-models.md**](docs/local-models.md) — serve the
  small model on the Pi and the large models on a LAN box.
- **Personal cloud:** [**docs/cloud-deployment.md**](docs/cloud-deployment.md) —
  run GLM-5.2 + Kimi K2.6 on a self-managed colo GPU box over a private VPN, with
  model-swapping and full Ansible automation under `deploy/cloud/`.
- **How it compares:** [**docs/comparison.md**](docs/comparison.md) — coding-monkey
  vs Claude Code vs OpenAI Codex.

---

## Two kinds of agent

| | **Native agents** (default) | **PTY harness agents** |
| --- | --- | --- |
| What runs | An in-process async task driving the LLM directly | An external CLI in a PTY (`codex`, `claude`, `hermes`) |
| Footprint | ~10 MiB each, network-bound | ~100–300 MiB each |
| Concurrency | **100+ on a Raspberry Pi 5** | ~10 on a Pi |
| Use for | Scale, throughput, running on small hardware | Full local power on a big machine |

Native agents are how `monkey` runs 100+ agents at once on modest hardware: each
is a lightweight task that spends most of its life waiting on the model API, so
the limit is RAM and the provider's rate limit, not CPU. The deck spawns them
through a scheduler bounded by a live memory watchdog and a per-provider limiter
that backs the whole fleet off together on a `429` (no retry storms).

**Bring your own agent.** If you already use Codex, Claude Code, or Hermes,
`monkey` detects them (`monkey doctor`) and can spawn them as PTY harnesses, and
`monkey import` pulls your existing `CLAUDE.md` / `AGENTS.md` prompts into
`.monkey/context/`.

Check your host's ceilings any time:

```bash
monkey doctor
#   Capacity
#     [info] RAM 6400 MiB free / 8192 MiB total   CPUs 4
#     [info] max native agents: 128   max PTY agents: 12
```

The ceilings come from `AgentBudget::native()` / `::pty()` in
`crates/core/src/concurrency.rs`. See **[docs/RASPBERRY_PI.md](docs/RASPBERRY_PI.md)**
for running on a Pi.

---

## Commands

Run `monkey <command> --help` for full flags.

| Command | What it does |
| --- | --- |
| `monkey setup` | One-command onboarding: scaffold, import prompts, diagnose, next steps. |
| `monkey init [path]` | Scaffold `.monkey/` (context, config, default tentacle). |
| `monkey import [path]` | Import existing agent prompts (`CLAUDE.md`, `AGENTS.md`, …) into `.monkey/`. |
| `monkey engulf [path]` | Scan the repo and write context files agents will read. |
| `monkey chat [prompt]` | Interactive REPL with an agent, using the assembled context. |
| `monkey deck` | Web dashboard: multiple agent terminals with tentacle contexts. |
| `monkey orchestrate` | Multi-repo orchestrator REPL. |
| `monkey review` | Review the current diff. |
| `monkey investigate <symptom>` | Root-cause a bug. |
| `monkey cso` | Security review (Chief Security Officer skill). |
| `monkey ship` | typecheck → review → cso → pentest → push. |
| `monkey pentest [install-hook \| status]` | Native-Rust pentest; optional pre-push gate. |
| `monkey compliance [status \| verify \| evidence]` | SOC 2 audit-readiness pipeline. |
| `monkey doctor` | Diagnostics: keys, git, agent CLI, host capacity, local-model reachability. |
| `monkey models [--probe]` | List registered models with tier/host/endpoint; `--probe` checks local endpoints. |

---

## The `.monkey/` directory

`monkey init` scaffolds a per-project directory. The `context/` and `tentacles/`
folders are meant to be committed — they are the project's shared "agent brain".

```
.monkey/
├── config.json          default agent, provider, tier, default_model, local_models, orchestrator
├── context/             committed — read as the agent system prompt
│   ├── PROJECT.md
│   ├── CONVENTIONS.md
│   ├── AGENT.md         generic agent guidance
│   ├── CODEX.md         codex-specific guidance
│   └── GLOSSARY.md
├── tentacles/<id>/      committed — scoped work containers (CONTEXT.md + todo.md)
├── vault/               gitignored — generated knowledge graph
└── sessions/            gitignored — transcripts + tamper-evident audit logs
```

Context is assembled in this order, capped at 32 KB:
`PROJECT.md → CONVENTIONS.md → GLOSSARY.md → {AGENT,CODEX}.md → tentacle CONTEXT.md → tentacle todo.md`.

---

## Build from source

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# From the workspace root
cargo build --workspace            # debug build
cargo test  --workspace            # run the tests
cargo install --path crates/cli    # install the `monkey` binary

# Web dashboard (leptos/WASM) — built with trunk
cargo install trunk
trunk build crates/web/index.html
```

---

## Project layout

Nine crates in a Cargo workspace:

| Crate | Role |
| --- | --- |
| `monkey-core` | Types, errors, model registry, repo detection, concurrency cap, memory watchdog, rate limiter. |
| `monkey-agents` | Context assembly, secret redaction, audit log, PTY harness spawn (codex/claude-code/hermes). |
| `monkey-runtime` | Native in-process agent engine: tools, agent loop, provider limiter, scheduler, model orchestrator + escalation. |
| `monkey-engulf` | Repo scanner, security audit, deployer, knowledge vault. |
| `monkey-skills` | review / investigate / cso / ship skills + registry. |
| `monkey-pentest-agent` | Pre-push hook + native-Rust pentest engine. |
| `monkey-deck` | axum HTTP + WebSocket server; tentacles. |
| `monkey-cli` | The `monkey` binary. |
| `monkey-web` | leptos WASM dashboard. |

---

## Credits

Built on the work of several open-source projects:

- **LLM providers** — the client targets the OpenAI-compatible Chat Completions
  API; defaults are [OpenRouter](https://openrouter.ai) and
  [OpenAI](https://platform.openai.com).
- **[Codex CLI](https://github.com/openai/codex)** — PTY-spawned for the
  interactive REPL hand-off (the agent binary is configurable).
- **[gitleaks](https://github.com/gitleaks/gitleaks)** — secret-scan regex shapes
  ported to Rust for the whitebox pentest.
- **[portable-pty](https://github.com/wez/wezterm), [axum](https://github.com/tokio-rs/axum),
  [leptos](https://github.com/leptos-rs/leptos), [xterm.js](https://xtermjs.org)** —
  the terminal, server, and UI foundations.

## License

MIT
