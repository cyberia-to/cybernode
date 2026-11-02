---
title: launch spacepussy-test
tags: cyber, soft3, cybernode, guide, launch
crystal-type: guide
crystal-domain: cyber
alias: launch network, cybernode guide, node operator guide, start testnet, spacepussy-test guide
---
# launch spacepussy-test

moved from `soft3/docs/launch.md` on 2026-11-02 (proposal cybernode § 5); the node operator guide of [[cybernode]].

how to run the **real soft3 network** with cybernode — cybergraph + bbg as the chaosnet **spacepussy-test**. the phase-1 root chain (`pussy`, `bostrom`) runs from the same binary with `--chain`; its runbook is `deploy/chain/README.md`.

this is not a status stub. the node processes cyberlinks into authenticated state, advances height on finalize, and serves product RPC.

## network identity

| field | value |
|-------|-------|
| name | spacepussy-test |
| chain_id | `spacepussy-test` |
| engine | cybergraph + bbg |
| protocol | `soft3/spacepussy-test/v1` (a kept wire identifier) |
| public rpc | `https://cyb.ai/spacepussy-test` |
| host | cyberproxy (an apparatus host, the public edge) |
| local bind | `127.0.0.1:7780` |
| denom | `testpussy` |

cosmos **space-pussy** / **bostrom** on the apparatus hosts are bootloader chains (go-cyber). product tools reject those names.

## install

```bash
rustup update stable       # need rustc ≥ 1.85 (edition 2024 deps)
cargo install cybernode    # operator / node (or: cargo install --path rs)
cargo install true-cyber   # product face (thin client)
```

## run a node (local)

```bash
cybernode node --home ~/.spacepussy-test --bind 127.0.0.1:7780 --moniker dev-1
```

what starts:

- cybergraph processor (intend/seal/link lifecycle)
- bbg authenticated state
- durable log at `$home/log` + block height at `$home/blocks`
- HTTP RPC on the bind address

the root chain instead: `cybernode node --chain pussy --home ~/.soft3/pussy --bind 127.0.0.1:7781 --radio 0.0.0.0:7791 --peers peers.toml` (the home directory keeps its name; see README § kept names).

### RPC

| method | path | role |
|--------|------|------|
| GET | `/status` | chain_id, moniker, height, bbg_root, signals, particles |
| GET | `/health` | `ok` |
| GET | `/root` | BBG root hex |
| GET | `/stats` | height, root, graph counts |
| POST | `/v1/link` | submit cyberlink JSON |
| POST | `/v1/finalize` | close block (advance height) |

### submit a cyberlink

labels are hemera-hashed; pure hex (≤64 chars) is left-padded identity.

```bash
curl -sS -X POST http://127.0.0.1:7780/v1/link \
  -H 'content-type: application/json' \
  -d '{"neuron":"01","from":"0a","to":"0b","amount":1,"valence":0,"finalize":true}'
```

response:

```json
{"ok": true, "height": 1, "root": "…", "signals": 1}
```

`finalize: true` (default) calls `bbg.finalize_block` after the link so height advances.

### probe

```bash
cybernode sync
# or, against local bind:
# cybernode does not yet take --rpc; use curl for local-only, public edge via cybernode sync
curl -sS http://127.0.0.1:7780/status | head
cyber sync                 # product client → public edge
```

## run the public chaosnet (cyberproxy)

host: **cyberproxy** (edge for cyb.ai).

```bash
# on an operator machine
cargo install cybernode --force

# on cyberproxy
ssh cyberproxy
export PATH="$HOME/.cargo/bin:$PATH"
cargo install cybernode --force
```

the unit is `deploy/chain/cybernode@.service`, one instance per network
(`cybernode@pussy`, `cybernode@bostrom`), parameters in `/etc/soft3/<chain>.env`
(template `deploy/chain/chain.env.example`, a kept path), the health gate
`cybernode-check` as `ExecStartPre`:

```bash
sudo install -m 0755 deploy/chain/cybernode-check /usr/local/bin/
sudo cp deploy/chain/cybernode@.service /etc/systemd/system/
sudo cp deploy/chain/chain.env.example /etc/soft3/pussy.env   # edit bind, radio, moniker
sudo systemctl daemon-reload
sudo systemctl enable --now cybernode@pussy
sudo systemctl status cybernode@pussy --no-pager | head -20
curl -sS http://127.0.0.1:7781/status | head
```

the chaosnet on 7780 keeps its legacy unit `deploy/spacepussy-test.service`,
which now starts the cybernode binary; `deploy/deploy-cyberproxy.nu` installs
and restarts it. step by step for genesis day: `deploy/chain/README.md`.

nginx (already on cyberproxy): `cyb.ai/spacepussy-test/` → `127.0.0.1:7780`; the
root chain snippet is `deploy/chain/nginx-chain-api.conf.template`.

```bash
# public
curl -sS https://cyb.ai/spacepussy-test/status | head
cyber sync
```

redeploy after a cybernode upgrade:

```bash
ssh cyberproxy 'export PATH=$HOME/.cargo/bin:$PATH; cargo install cybernode --force; sudo systemctl restart cybernode@pussy'
```

## multi-node (current limits)

the chaosnet v1 is **single-node cybergraph**. each `cybernode node` without `--chain` is an independent local processor with its own store. the root chain (`--chain`) gossips over radio and syncs (`specs/chain-wire.md`); three nodes on one machine: `nu scripts/chain-fleet.nu`.

not yet on the public chaosnet:

| capability | status |
|------------|--------|
| cybergraph link + bbg state | **live** |
| height / root / stats RPC | **live** |
| durable log | **live** |
| radio gossip between nodes | live on the root chain (`--chain`), open (S4) on the chaosnet |
| foculus multi-node φ* finality | open (S5) |
| seal STARK binding at commit | open (S3) |

next work is wire [[radio]] + [[foculus]] so two cybernodes exchange signals and share one tip — see [[cyber/launch]] S3–S6.

until then: one public node on cyberproxy is the product spacepussy-test; clients sync to it with `cyber sync` / `cybernode sync`.

## product client

```bash
cargo install true-cyber --force
export PATH="$HOME/.cargo/bin:$PATH"
cyber version    # cyber … (true-cyber)
cyber sync       # probes the cybernode on cyberproxy
```

true-cyber is a thin product face over the soft3 network. the network itself is the cybernode.

## not soft3

| name | substrate | role |
|------|-----------|------|
| spacepussy-test | soft3 cybergraph+bbg, run by cybernode | product chaosnet |
| space-pussy | cosmos go-cyber | bootloader experimental |
| bostrom | cosmos go-cyber | bootloader mainnet history |

## related

- [[cybernode]] · [[soft3]] · [[soft3/status]] · [[install]] · [[bootloader]] · [[cyber/launch]] · `deploy/chain/README.md`
