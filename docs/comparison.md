# coding-monkey vs. Claude Code vs. OpenAI Codex

A comparison of coding-monkey against the two best-known agentic coding
harnesses, framing where each fits. The headline difference: Claude Code and
Codex are polished single-developer assistants that drive **hosted frontier
models**; coding-monkey is a **fleet engine for self-hosted, open-weights
models** that runs on cheap hardware down to a Raspberry Pi.

## At a glance

| Dimension | coding-monkey | Claude Code | OpenAI Codex |
|-----------|---------------|-------------|--------------|
| Implementation | Rust, single static binary | TypeScript / Node CLI | TypeScript / Node CLI (Rust core in later builds) |
| Primary model | Open-weights, self-hosted (GLM-5.2 default, Kimi K2.6) | Anthropic Claude (hosted API) | OpenAI GPT / Codex (hosted API) |
| Local / offline | Yes — runs fully on your hardware | No — requires Anthropic API | No — requires OpenAI API |
| Concurrency | 100+ scheduled native agents on a Pi | One interactive session | One interactive session |
| Model routing | Difficulty scoring + escalation across a local tier ladder | Single configured model | Single configured model |
| Target hardware | Raspberry Pi → workstation; ARM64-first | Developer laptop | Developer laptop |
| Cost model | $0 inference (your hardware) | Per-token / subscription | Per-token / subscription |
| Built-in governance | Pentest gate, security/review skills, audit log | General tool use | General tool use + sandboxing |

## Architecture

**Claude Code and Codex** are interactive, single-session CLIs. A developer runs
one agent in one repo; the agent reads/edits files and runs commands through a
hosted frontier model, streaming a conversation in the terminal. Their strength
is model quality and turnkey UX — install, authenticate, and the
state-of-the-art model does the work. They are thin clients over a remote brain.

**coding-monkey** is an agent *engine*, not a single assistant. Its
`monkey-runtime` crate runs lightweight native agents as async tasks (~12 MiB
each), admitted by a `Scheduler` with a memory watchdog and per-class quotas, and
gated by a shared per-provider rate limiter so a fleet of agents backs off
together on `429`s. A web "deck" (`axum` + WebSocket) spawns and supervises agents
across isolated work contexts ("tentacles"). The brain is whatever
OpenAI-compatible endpoint you point it at — by default, open-weights models on
your own hardware.

The consequence: Claude Code/Codex scale *up* (a better model per session);
coding-monkey scales *out* (more agents per host) and *in* (onto a Pi).

## Model strategy

This is the sharpest divergence.

- **Claude Code / Codex**: one hosted model, chosen by the vendor's lineup. No
  local fallback; no offline mode; quality and latency are the vendor's. You get
  frontier capability and pay per token.
- **coding-monkey**: open-weights models you host, selected by an orchestrator.
  Tasks are scored for difficulty and routed across a tier ladder — a small
  Pi-local model for trivial work, **GLM-5.2** for everyday coding, **Kimi K2.6**
  for the hardest tasks — with automatic **escalation** to a stronger model when a
  run fails or stalls. Inference is free and private.

Because the largest open models (GLM-5: 355B–744B params; Kimi K2.6: 1T params)
cannot fit a Pi's 16 GB RAM, coding-monkey uses a **tiered topology**: the Pi runs
a small model locally and orchestrates, while the large models run on a capable
box reached over HTTP — a LAN machine
([`local-models.md`](./local-models.md)) or a self-managed colo GPU server
reached over a private VPN ([`cloud-deployment.md`](./cloud-deployment.md)). It
stays "fully local" (your hardware, your network) without pretending a Pi can
hold a trillion-parameter model.

## Concurrency and scale

Claude Code and Codex are built around one developer, one session. coding-monkey
is built to run **many agents at once on modest hardware**: a bounded scheduler,
a 12 MiB-per-agent native runtime, RAM-floor admission control, and fleet-wide
rate-limit backoff. This is the capability the other two don't target — e.g.
fanning a hundred scoped agents across a monorepo on a single board computer.

## Governance and safety

coding-monkey ships opinionated, built-in gates that the general-purpose
assistants leave to the user: a native-Rust **pentest** pre-push gate (whitebox
source analysis + blackbox HTTP probes), composable **skills**
(review / investigate / cso / ship), tamper-evident **audit logs**, filesystem
path-jailing per agent, and an allowlist-only command tool. Codex emphasizes
sandboxed execution; Claude Code emphasizes broad, well-integrated tool use. Both
are excellent general harnesses; coding-monkey trades some generality for a
security-and-fleet posture.

## Honest trade-offs

coding-monkey's positioning has real costs:

- **Model capability gap.** Self-hosted open-weights models — especially the
  small Pi-local tier — are less capable than the frontier models behind Claude
  Code and Codex. Difficulty-based escalation narrows the gap but does not close
  it.
- **Operational burden.** You run the inference servers. The strong tier needs a
  capable LAN box (GPU/large RAM); a lone Pi cannot serve GLM-5.2 or Kimi K2.6.
- **UX maturity.** Claude Code and Codex are highly polished single-developer
  tools with large ecosystems. coding-monkey optimizes for the fleet/self-hosted
  use case, not turnkey single-session ergonomics.

## When to use which

- **Claude Code / Codex** — you want the strongest model with zero infrastructure
  and are fine sending code to a hosted API and paying per token.
- **coding-monkey** — you need local/private inference, open-weights models, many
  concurrent agents, run on cheap or edge hardware (Raspberry Pi), or want
  built-in security gating. You accept running your own model servers in exchange
  for $0, private, offline-capable inference.

They are not strictly competitors: coding-monkey can even drive Claude Code or
Codex as PTY-spawned agents within its fleet, treating them as one more worker
class alongside its native open-weights agents.
