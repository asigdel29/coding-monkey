```
/*
   File: README.md
   Project: coding-monkey — production AI agent platform in Rust

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust workspace scaffold; foundation
                                 crates (core, agents) ported with full impl;
                                 engulf/skills/pentest-agent/deck/cli/web
                                 scaffolded with module structure and stubs.

   Contents
   - Workspace map
   - Public surface (per `monkey <command>`)
   - Build instructions
   - Porting status
*/

package coding-monkey;
```

# coding-monkey

> Native-Rust port of [`anu`](https://github.com/asigdel29/anu) — a drop-in
> coding agent for any repo. Same five features (drop-in agent context,
> deep-learn, parallel terminals, multi-repo orchestrator, mandatory pentest
> gate), rebuilt for low-latency, predictable resource use, and single-binary
> deployment.

```bash
cargo install --path crates/cli           # builds the `monkey` binary
cd your-project
monkey init                               # scaffold .monkey/  (asks if you want engulf)
monkey deck                               # web frontend at https://127.0.0.1:8787
monkey chat                               # REPL with the auto-detected agent
monkey ship                               # typecheck → review → cso → pentest → push
```

---

## What this code does

`coding-monkey` is a Rust cargo workspace with **8 crates**. The `monkey-cli`
crate produces a single `monkey` binary; every subcommand lazy-loads exactly
one workspace crate so cold-start stays under a few ms. The five distinguishing
features are:

1. **Reads a committed `.monkey/context/` directory** and feeds it as a system
   prompt to whichever agent CLI is on `PATH`. Same context across teammates
   because `.monkey/context/` is in git.
2. **Generates that context automatically** with `monkey engulf` — scans the
   repo, detects stack/deps/API routes, runs an LLM-assisted security audit,
   writes a deployment runbook, emits an Obsidian-shaped knowledge vault.
3. **Runs many agent terminals in one browser tab** (`monkey deck`) — each
   scoped to a "tentacle" so parallel work doesn't trample shared context.
4. **Routes work across multiple repos to the right model** (`monkey orchestrate`)
   — picks fast/balanced/powerful tiers per task, tracks token cost.
5. **Gates `git push` on a mandatory native-Rust pentest** (`monkey pentest
   install-hook`) and ships a SOC 2 evidence pipeline (control-matrix checks,
   tamper-evident audit-log hash chain, auditor-ready tar.gz bundles).

---

## Workspace map

```
crates/
├── core/             monkey-core             types, errors, model registry, repo detection
├── agents/           monkey-agents           context, redact, audit hash chain, PTY spawn
├── engulf/           monkey-engulf           scanner, security, deployer, vault, prompts
├── skills/           monkey-skills           review, investigate, cso, ship + registry
├── pentest-agent/    monkey-pentest-agent    pre-push hook + native-Rust pentest engine
├── deck/             monkey-deck             axum HTTP+WS server, tentacles, schemas
├── cli/              monkey-cli              the `monkey` binary
└── web/              monkey-web              leptos CSR WASM frontend (xterm.js interop)
```

```
/**
 * @crate      monkey-core
 * @entry      crates/core/src/lib.rs
 * @purpose    Foundation. Types/errors/IDs/models/repo-detection. No I/O at
 *             load time. Every other crate depends on this.
 * @invariant  No top-level imports of std::process, std::net — keep deps light.
 * @exports    Error, ModelRegistry, ModelSelector, ModelTier, ModelSpec,
 *             TaskState, TaskStatus, TaskType, TokenUsage, RepoConfig,
 *             SessionState, generate_id, detect_repo, discover_repos
**/
```

```
/**
 * @crate      monkey-agents
 * @entry      crates/agents/src/lib.rs
 * @purpose    The "spawn an agent" primitive. Five responsibilities:
 *               assemble_context — read .monkey/context/* + tentacle, cap 32 KB
 *               redact          — scrub secrets from PTY stdout
 *               AuditLogger     — append-only, hash-chained .monkey/sessions/audit-*.log
 *               doctor          — environment diagnostics
 *               spawn_agent     — PTY-spawn the chosen CLI with the assembled prompt
 * @invariant  Missing context files skipped silently; oversize files trimmed
 *             and surfaced via context.truncated_files.
 * @invariant  Audit log is hash-chained — verify_audit_log walks end-to-end.
**/
```

```
/**
 * @crate      monkey-deck
 * @entry      crates/deck/src/lib.rs
 * @purpose    Web frontend server. axum HTTP+WS. Refuses to bind off-loopback
 *             without TLS unless --insecure-no-tls. Tentacles persisted as
 *             folders in .monkey/tentacles/<id>/. Each terminal tab is a
 *             portable-pty spawn of the configured agent. Each tentacle and
 *             tab is decorated with the pixel-monkey icon (replaces the
 *             octogent octopus from the TS reference).
 * @invariant  WS messages rate-limited (default 100/s/connection).
 * @invariant  Sessions expire (default 8h TTL).
**/
```

```
/**
 * @crate      monkey-pentest-agent
 * @entry      crates/pentest-agent/src/lib.rs
 * @purpose    Mandatory pre-push pentest gate. Native-Rust reimplementation
 *             of the Apex (Apache-2.0) agent that the TS port shelled out to.
 *             Two modes: whitebox source analysis (--cwd) and blackbox HTTP
 *             probing (--target).
 * @exports    install_pre_push_hook, uninstall_pre_push_hook,
 *             is_pre_push_installed, run_pentest, summarize
**/
```

---

## Public surface — `monkey` commands

```
/**
 * monkey init [path]
 * Scaffold .monkey/ in the project: context dirs, templates, default tentacle,
 * config.json, optional engulf prompt.
 *
 * @param  -y, --yes       Accept defaults.
 * @param  --no-engulf     Skip the post-init engulf prompt.
 * @return condition: .monkey/{config.json, context/*.md, tentacles/main/*} written.
 * @see    crates/cli/src/commands/init.rs
**/

/**
 * monkey engulf [path]
 * Deep-learn the codebase and write context files agents will consume.
 *
 * @param  --auto                Run all phases without prompts.
 * @param  --phases <list>       scan | security | docs | vault | deploy
 * @param  --output <dir>        Output dir (default: .monkey/ inside target).
 * @param  --provider <name>     anthropic | openai (default: anthropic).
 * @return condition: .monkey/context/*.md + .monkey/vault/ populated.
 * @see    crates/engulf/src/lib.rs
**/

/**
 * monkey chat [prompt]                                    (default command)
 * Interactive REPL. Hands stdin/stdout to the agent CLI with a system prompt
 * assembled from .monkey/.
 *
 * @param  --agent <agent>       claude | codex | auto (default: auto)
 * @param  --tentacle <id>       Tentacle scope (default: "main")
 * @param  --cwd <path>          Project directory.
 * @return condition: process exits with the agent's exit code.
 * @exception 1 IF (no agent CLI on PATH OR doctor fails)
 * @see    crates/cli/src/commands/chat.rs
**/

/**
 * monkey deck
 * Web frontend: multi-terminal agent dashboard with tentacle contexts.
 *
 * @param  --port <n>            HTTP/WS port (default: 8787)
 * @param  --host <addr>         Bind address (default: 127.0.0.1)
 * @param  --agent <bin>         Agent binary (default: claude)
 * @param  --ttl <seconds>       Session token TTL (default: 8h)
 * @param  --rate <perSec>       WS messages/sec/connection (default: 100)
 * @param  --cert <path>         TLS cert (PEM)  WHERE (required off-loopback)
 * @param  --key  <path>         TLS key  (PEM)  WHERE (required off-loopback)
 * @param  --insecure-no-tls     Allow non-loopback without TLS (DANGEROUS)
 * @return condition: HTTPS+WSS server bound; URL printed; serves until SIGINT.
 * @see    crates/deck/src/server.rs
**/

/**
 * monkey orchestrate
 * Multi-repo orchestrator REPL.
 *
 * @param  --dir <dir>           Workspace root (default: cwd)
 * @return condition: SessionManager initialized; REPL with /quit /usage /repos.
**/

/**
 * monkey skill (list | run <name> [...])
 * @see    crates/skills/src/registry.rs
**/

/**
 * monkey review                                         @see review skill
 * monkey investigate <symptom>                          @see investigate skill
 * monkey cso                                            @see cso skill
 * monkey ship                                           @see ship skill
**/

/**
 * monkey pentest [install-hook | uninstall-hook | status]
 * AI-driven penetration testing — mandatory before every push when the hook
 * is installed.
 *
 * @param  --target <url>        Blackbox target URL.
 * @param  --cwd <path>          Whitebox source path.
 * @param  --fail-on <severity>  critical | high | medium | low (default: high)
 * @param  --pre-push            Run as pre-push gate.
 * @return condition: result.ok=true means no findings ≥ fail-on severity.
 * @see    crates/pentest-agent/src/runner.rs
**/

/**
 * monkey compliance (status | verify | evidence)
 * SOC 2 audit-readiness pipeline.
 *
 *   verify   — Walk every audit-log hash chain in .monkey/sessions/.
 *   status   — Run automated checks for every control in CONTROL_MATRIX.json.
 *   evidence — Bundle audit logs + matrix + policies into auditor-ready tar.gz.
 *
 * @see    crates/cli/src/commands/compliance.rs
**/

/**
 * monkey doctor    Environment diagnostics — keys, git, agent CLIs.
 * monkey models    List registered AI models with cost tiers.
**/
```

---

## The `.monkey/` on-disk contract

```
.monkey/
├── config.json          // default agent, tier, fail-on
├── context/             // ⭐ committed; the project's "agent brain"
│   ├── PROJECT.md
│   ├── CONVENTIONS.md
│   ├── CLAUDE.md
│   ├── CODEX.md
│   ├── GLOSSARY.md
│   ├── SECURITY.md
│   └── DEPLOYMENT.md
├── tentacles/<id>/      // ⭐ committed; scoped work containers
│   ├── CONTEXT.md
│   └── todo.md
├── plans/               // design docs
├── reports/             // opt-in committed skill outputs
├── vault/               // gitignored: Obsidian-shaped knowledge graph
└── sessions/            // gitignored: ephemeral worker transcripts
                         //             + tamper-evident audit-*.log files
```

Read order, capped at 32 KB total:
```
.monkey/context/PROJECT.md
.monkey/context/CONVENTIONS.md
.monkey/context/GLOSSARY.md
.monkey/context/{CLAUDE,CODEX}.md          # whichever matches the agent kind
.monkey/tentacles/<active>/CONTEXT.md
.monkey/tentacles/<active>/todo.md
```

---

## Build

```bash
# Install rustup if you don't have it.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# From the workspace root:
cargo build --workspace                  # debug build
cargo build --workspace --release        # release build
cargo test  --workspace                  # all crate tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt   --all -- --check

# Install the binary:
cargo install --path crates/cli

# WASM frontend (requires trunk):
cargo install trunk wasm-bindgen-cli
trunk serve crates/web/index.html
```

---

## Porting status (vs. the TS reference)

| Crate                    | Status                                |
| ------------------------ | ------------------------------------- |
| `monkey-core`            | ✅ full impl + tests                  |
| `monkey-agents`          | ✅ full impl + tests (ctx/redact/audit/doctor/spawn) |
| `monkey-pentest-agent`   | hook ✅ full impl + tests; runner stub |
| `monkey-engulf`          | scaffolded (module layout + types)    |
| `monkey-skills`          | scaffolded (Skill trait + registry)   |
| `monkey-deck`            | scaffolded (DeckOpts + tentacles read/write) |
| `monkey-cli`             | scaffolded (full subcommand surface, dispatchers wired) |
| `monkey-web`             | leptos CSR shell ✅ + monkey pixel-icon UI + xterm.js mount + WS auto-reconnect |

Subsequent commits port crate-by-crate to full parity with the TS reference.

## License

MIT
