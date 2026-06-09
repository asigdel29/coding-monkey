# coding-monkey

A single-binary coding-agent platform written in Rust. Drop `monkey` into any
repo, bring your own API key, and run a swarm of AI agents — from the terminal
or a web dashboard — scaled to whatever your machine can handle.

No runtime, no GC, no Node. One `monkey` binary.

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

### Self-hosted models

Point `monkey` at your own OpenAI-compatible server — [Ollama](https://ollama.com),
llama.cpp's `server`, [vLLM](https://github.com/vllm-project/vllm), or LM Studio —
with no API cost. Great on a Pi or LAN.

```bash
# 1. Run a local model (example: Ollama)
ollama serve &
ollama pull qwen2.5-coder

# 2. Point monkey at it (base or full URL both work; key usually not needed)
export MONKEY_SELF_HOSTED_URL=http://localhost:11434      # → /v1/chat/completions
# export MONKEY_SELF_HOSTED_KEY=...                        # only if your server needs one

# 3. Select the self-hosted provider in .monkey/config.json
#    "default_provider": "self-hosted"
monkey doctor       # shows the configured self-hosted endpoint
monkey deck         # native agents now run against your local model
```

| Provider value | Env | Notes |
| --- | --- | --- |
| `self-hosted` | `MONKEY_SELF_HOSTED_URL` (+ optional `MONKEY_SELF_HOSTED_KEY`) | Any OpenAI-compatible endpoint. No key required for most local servers. |

Any provider that exposes the OpenAI Chat Completions API works. Pick the default
provider and model in `.monkey/config.json`:

```json
{
  "default_agent": "auto",
  "default_provider": "openrouter",
  "default_tier": "balanced",
  "fail_on": "high"
}
```

See the built-in model lineup with `monkey models`.

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
| `monkey doctor` | Diagnostics: keys, git, agent CLI, host capacity. |
| `monkey models` | List registered models with cost tiers. |

---

## The `.monkey/` directory

`monkey init` scaffolds a per-project directory. The `context/` and `tentacles/`
folders are meant to be committed — they are the project's shared "agent brain".

```
.monkey/
├── config.json          default agent, provider, tier, fail-on
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

Eight crates in a Cargo workspace:

| Crate | Role |
| --- | --- |
| `monkey-core` | Types, errors, model registry, repo detection, concurrency cap, memory watchdog, rate limiter. |
| `monkey-agents` | Context assembly, secret redaction, audit log, PTY harness spawn (codex/claude-code/hermes). |
| `monkey-runtime` | Native in-process agent engine: tools, agent loop, provider limiter, scheduler. |
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
