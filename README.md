---
tags: cyber, soft3, cybernode, node, network
alias: cybernode, the node, rung 7, the seventh rung
crystal-type: entity
crystal-domain: cyber
---
# cybernode — the node of the soft3 ladder

rung 7 of [[soft3]]: one binary that runs any compatible network instance as data, and owns everything networked — the phase-1 root chain kernel, the wire and sync, routing, mining, the genesis pipeline, the wallet, the HTTP surface. its planes stay where [[network-planes]] put them: [[radio]] reaches, sync reconciles, [[foculus]] agrees, solong rewards. **cybernode.ai is the reference instance**, operated by cyberia; an instance is cybernode plus a manifest.

decided and executed from [[soft3/proposals/cybernode|the cybernode proposal]] (the owner, 2026-10-17 and 2026-10-27). the code moved here from the `soft3` crate with its history on 2026-10-27, unchanged: the wire identifiers (`soft3/chain/…/v1`), the genesis files and the home directories keep their names, so every running node and every genesis hash stays valid. what changed is the crate, the binary and the unit names.

| here | holds |
|---|---|
| `rs/` | the crate and binary `cybernode`: `chain/` the kernel, `genesis/`, `node/`, `wallet/`, `network.rs`; the verbs `run` (`node`) · `mine` · `sync` (`status`) · `genesis` · `chain` · `conformance` · `network` |
| `specs/` | [[cybernode/specs/chain|chain]] · [[cybernode/specs/chain-wire|chain-wire]] · [[cybernode/specs/chain-conformance|chain-conformance]] · [[cybernode/specs/chain-economy|chain-economy]] · [[cybernode/specs/genesis|genesis]] · [[cybernode/specs/native-node|native-node]] · [[cybernode/specs/routing|routing]] |
| `deploy/` | `cybernode@<network>.service`, `cybernode-check`, journald, logrotate, nginx and ufw templates of an instance; `deploy/chain/README.md` is the runbook of the two apparatus hosts |
| `scripts/` | `chain-fleet.nu`, three nodes on one machine — the local rehearsal of the runbook |
| `docs/` | [[cybernode/docs/launch|launch]], the node operator guide: install, run, probe, the public host |
| `audit/` | the receipts: conformance, economy, mining, the net, the requirement map of the chain, block relation and proof authentication |

```
cargo install --path rs            # or: cargo install cybernode, once published
cybernode run --chain pussy --home ~/.soft3/pussy --bind 127.0.0.1:8545 --radio 0.0.0.0:7791 --peers peers.toml
cybernode mine --address NEURON --node http://127.0.0.1:8545
cybernode conformance check chain
```

operators start at [[cybernode/docs/launch|docs/launch.md]]; the root chain on the apparatus hosts follows `deploy/chain/README.md`.

## kept names

decided 2026-11-02 while closing the proposal (§ 5): the wire identifiers (`soft3/chain/…/v1`, `soft3/spacepussy-test/v1`), the genesis files, the home directories (`~/.soft3/<chain>`, `/etc/soft3`, `/var/lib/soft3`) and the environment variable names `SOFT3_CHAIN`, `SOFT3_BIND`, `SOFT3_RADIO`, `SOFT3_MONIKER`, `SOFT3_STALE_SECS`, `SOFT3_MIN_DISK_GIB`, `SOFT3_DEV_EPOCH`, `SOFT3_COST_*`, `SOFT3_CONFORMANCE_DIR` keep their names, for compatibility with the running hosts and every genesis hash. everything a person reads — strings, docs, unit names, scripts, specs — says cybernode. a change to a kept name is a consensus or an operations change and goes through specs/chain.md § 7 or the apparatus runbooks.

the `soft3` crate remains the stack facade: it depends on cybernode, re-exports its modules for one release and forwards the operational verbs, so nothing that built against `soft3::chain` breaks today. the soft3 repository keeps the ladder, the contracts of the stack, the release train and the SDKs; the private repository `apparatus` keeps cyberia's own hosts, secrets and runbooks.

what the node must still grow, so that cyberia runs all of its execution on it, is the capability table of the proposal § 4: proving workers and block aggregation, the index over bbg, snapshots and tip proofs, particles over radio blobs, operator keys through vault. each lands here as a spec, code and a receipt.
