# Deploying the strong tier to a personal cloud

coding-monkey's strong models — **GLM-5.2** (everyday coding) and **Kimi K2.6**
(hardest tasks) — are far too large for a Raspberry Pi. This runbook deploys
them on a **personal cloud**: a self-managed multi-GPU box you rack in a colo,
reached from the home Pi over a **private VPN**. The Pi keeps serving the small
Qwen2.5-Coder model locally and runs the orchestrator. Nothing is publicly
exposed; inference is reachable only through the tunnel.

For the Pi-local Fast tier and the overall tiered model design, see
[`local-models.md`](./local-models.md).

## Topology

```
Home                                  Colo (personal cloud)
┌──────────────────────────┐         ┌─────────────────────────────────────┐
│ Raspberry Pi             │  WG VPN │ Multi-GPU box                        │
│  monkey (orchestrator)   │◀───────▶│  WireGuard gateway (private IP only) │
│  Qwen2.5-Coder (Fast)    │  :51820 │  llama-swap  ── OpenAI API :8080     │
│  host: pi  localhost     │         │    ├─ GLM-5.2  (warm default)        │
└──────────────────────────┘         │    └─ Kimi K2.6 (cold, on demand)    │
                                      │  weights on NVMe volume              │
                                      └─────────────────────────────────────┘
```

A single **llama-swap** gateway exposes one OpenAI-compatible endpoint and keeps
**one large model resident at a time**: GLM-5.2 stays warm; requesting Kimi K2.6
swaps it in (and an idle TTL swaps it back out). Both models share one
`base_url` on the Pi — the orchestrator's `model` id selects which.

## Sizing (read first)

"One box, swap models" means the box never holds both models at once, but it
**must still fit the larger one (Kimi K2.6)**:

| Model | Quant | Approx. weights | GPU floor |
|-------|-------|-----------------|-----------|
| GLM-5.2 | INT4 | ~180 GB | ~3×80 GB |
| Kimi K2.6 | INT4 | ~600 GB | ~8×80 GB (or ~6×141 GB H200) |
| Kimi K2.6 | ~2-bit | ~350 GB | ~5×80 GB (quality loss) |

- **Disk:** size the NVMe data volume for both weight sets (≈800 GB at INT4).
- **Cold start is minutes.** Loading 350–600 GB into VRAM takes time, so GLM-5.2
  is kept warm and Kimi cold-loads only on escalation (rare). The orchestrator
  already tolerates a temporarily-unreachable strong tier.
- **Quants:** confirm current GGUF quants for both models (Unsloth publishes
  dynamic GGUFs) and pin the URL + sha256 in `ansible/group_vars/all.yml`.

## Deploy

All commands run from `deploy/cloud/`. Secrets (WireGuard keys) live in an
`ansible-vault` file; pass the password with `VAULT="--ask-vault-pass"`.

1. **Generate WireGuard keys** for the gateway and the Pi, and store them in
   `ansible/group_vars/vault.yml` (`ansible-vault create …`) as
   `vault_wg_gateway_private_key`, `vault_wg_gateway_public_key`,
   `vault_wg_pi_private_key`, `vault_wg_pi_public_key`.
2. **Edit** `ansible/inventory.ini` (hosts) and `ansible/group_vars/all.yml`
   (VPN IPs, `colo_public_host`, model URLs/checksums, `models_dir`,
   `pi_monkey_config`).
3. **Provision the GPU box** — Docker + NVIDIA Container Toolkit, firewall
   (only UDP/51820 open), WireGuard gateway:
   ```bash
   make provision
   ```
4. **Deploy the models** — fetch weights, bring up the swapping gateway, wait
   for health:
   ```bash
   make deploy
   ```
5. **Connect the Pi** — install the WireGuard peer and repoint GLM-5.2 / Kimi
   K2.6 in `.monkey/config.json` to the gateway (`host: cloud`):
   ```bash
   make connect
   ```

The resulting Pi config has the small model local and the two large models on
the cloud gateway:

```jsonc
"local_models": [
  { "id": "qwen2.5-coder-3b", "tier": "fast",     "base_url": "http://localhost:11434", "host": "pi"    },
  { "id": "glm-5.2",          "tier": "balanced", "base_url": "http://10.10.0.1:8080",   "host": "cloud" },
  { "id": "kimi-k2.6",        "tier": "powerful", "base_url": "http://10.10.0.1:8080",   "host": "cloud" }
]
```

## Verify

```bash
# On the Pi
wg show                       # tunnel handshake is recent
monkey doctor                 # "Local models": cloud endpoints reachable
monkey models --probe         # glm-5.2 / kimi-k2.6 show host=cloud, [up]
```

- **Swap behavior:** a medium task routes to GLM-5.2 (warm, immediate); a hard
  task escalates and triggers a one-time Kimi cold load (watch `nvidia-smi` —
  GLM unloads, Kimi loads; only one resident). After the idle TTL, Kimi unloads.
- **Offline:** with the colo box down, trivial tasks still run on the Pi-local
  Qwen tier; harder tasks fail cleanly (the probe and agent error say why).
- **Security:** from outside the VPN the inference port is unreachable — only
  UDP/51820 is open and llama-swap binds to the tunnel address.

## Teardown

```bash
make down     # stops serving, frees the GPUs; weights stay on the NVMe volume
```

## Alternatives

- **Always-on / higher throughput:** if you later run a model continuously,
  swap llama-swap for **vLLM** (better concurrency under coding-monkey's agent
  fleet) on a fixed port per model and drop the swap TTLs. This needs enough
  VRAM to hold whichever model(s) you keep resident.
- **Public endpoint instead of VPN:** put a TLS reverse proxy (Caddy/nginx) in
  front of llama-swap with an API key, and set `api_key_env` on the cloud model
  entries. The VPN path is preferred — it keeps the endpoint entirely private.
