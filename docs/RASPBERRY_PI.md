<!--
   File: docs/RASPBERRY_PI.md
   Purpose: Build and run monkey's native agent engine on a Raspberry Pi.
   History:
     2026-06-09  Anubhav Sigdel  initial
-->

# Running on a Raspberry Pi

`monkey`'s **native agents** are lightweight async tasks that spend almost all
their time waiting on the model API, so a Raspberry Pi 5 (8 GB) comfortably runs
**100+ at once**. This guide covers building for the Pi and tuning the host.

The PTY harness path (`codex`/`claude`/`hermes`) is heavyweight and tops out
around ~10 agents on a Pi — use native agents for scale.

## Build

### On the Pi (simplest)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/asigdel29/coding-monkey && cd coding-monkey
cargo install --path crates/cli           # uses .cargo/config.toml (Cortex-A76 tuning)
```

### Cross-compile from an x86 machine

```bash
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu -p monkey-cli
# binary: target/aarch64-unknown-linux-gnu/release/monkey  → copy to the Pi
```

`.cargo/config.toml` already tunes codegen for the Pi 5's Cortex-A76. For a bare
`cargo build --target aarch64-unknown-linux-gnu` (no `cross`), install the linker
(`apt install gcc-aarch64-linux-gnu`) and uncomment the `linker` line there.

## Host tuning

1. **Open-file limit.** 100+ agents hold HTTP sockets plus transient file
   handles. Raise the soft limit (`monkey doctor` warns if it's below 4096):

   ```bash
   ulimit -n 8192                          # this shell
   # persistent (systemd service): LimitNOFILE=8192
   ```

2. **Swap as a safety net.** The memory watchdog stops admitting new agents
   before RAM runs out, but a modest swap absorbs spikes. Keep swappiness low so
   it stays a backstop, not the default:

   ```bash
   sudo dphys-swapfile swapoff
   sudo sed -i 's/^CONF_SWAPSIZE=.*/CONF_SWAPSIZE=2048/' /etc/dphys-swapfile
   sudo dphys-swapfile setup && sudo dphys-swapfile swapon
   echo 'vm.swappiness=10' | sudo tee /etc/sysctl.d/99-swappiness.conf
   ```

## Run

```bash
export OPENROUTER_API_KEY=sk-or-...
cd your-project
monkey setup            # scaffold + import + diagnose
monkey doctor           # confirm: "max native agents: N"
monkey deck             # dashboard at http://127.0.0.1:8787 — spawn agents
```

`monkey doctor` reports the native ceiling for your Pi (RAM ÷ ~12 MiB per agent,
clamped to 128). Spawns past it are refused rather than thrashing the box; the
watchdog also pauses admission if free RAM drops near the floor.

## Run a local model on the Pi (no API cost)

A Pi 5 can serve a small coding model locally. Point native agents at it:

```bash
ollama serve & ollama pull qwen2.5-coder:3b
export MONKEY_SELF_HOSTED_URL=http://localhost:11434
# in .monkey/config.json:  "default_provider": "self-hosted"
monkey deck
```

A single local model serializes generation, so it suits a handful of agents
rather than 100; for high fan-out, keep native agents on a hosted provider and
use the local model for cheaper, private work. See the self-hosted section in
the [README](../README.md#self-hosted-models).

## What bounds concurrency on a Pi

- **RAM** — the memory watchdog (`crates/core/src/watchdog.rs`) holds back new
  agents near a floor.
- **Provider rate limit** — the per-provider limiter
  (`crates/runtime/src/limiter.rs`) paces calls and backs the whole fleet off on
  a `429`, so 100 agents don't stampede your quota.
- **Not CPU** — native agents are network-bound, so the `cpus × 4` guard that
  caps PTY agents does not apply to them.
