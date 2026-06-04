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
monkey init        # scaffold .monkey/
monkey doctor      # check keys, git, and how many agents your box can run
monkey deck        # open the web dashboard at http://127.0.0.1:8787
```

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

## Scaling agents to your machine

`monkey deck` runs as many agent terminals as your hardware can sustain. At
startup it derives a cap from free RAM and CPU count:

```
max agents = min( free_RAM × 75% ÷ ~512 MiB per agent,  CPUs × 4 )   (at least 1)
```

Spawns past the cap are refused rather than thrashing the machine. Check your
number any time:

```bash
monkey doctor
#   Capacity
#     [info] RAM 12480 MiB free / 32768 MiB total   CPUs 10
#     [info] max concurrent agents: 18
```

Tune the per-agent budget or set a hard ceiling via `AgentBudget` in
`crates/core/src/concurrency.rs`.

---

## Commands

Run `monkey <command> --help` for full flags.

| Command | What it does |
| --- | --- |
| `monkey init [path]` | Scaffold `.monkey/` (context, config, default tentacle). |
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
| `monkey-core` | Types, errors, model registry, repo detection, agent concurrency cap. |
| `monkey-agents` | Context assembly, secret redaction, audit log, PTY agent spawn. |
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
