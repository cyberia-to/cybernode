# AGENTS.md — cybernode

> the doctrine of the home is `~/cyber/AGENTS.md`; it applies here first. Claude Code receives both files at session start through `~/.config/cyber/agents-md.nu`; other agents read them in order.

cybernode is rung 7 of the soft3 ladder: the node. `rs/` is the crate and the binary `cybernode`; `specs/` the chain, wire, conformance, genesis, native-node and routing contracts; `deploy/` the units of a running instance; `audit/` the receipts. the wire identifiers (`soft3/chain/…/v1`), the genesis files and the home directories are unchanged by the move from soft3 (2026-10-27): a change to any of them is a consensus change and follows specs/chain.md § 7. the operations of cyberia's own instance live in the private repository `apparatus`. the kept names (`SOFT3_*` variables, `/etc/soft3`, `~/.soft3`, the wire identifiers) are listed in README § kept names: never rename them in code; everything a person reads says cybernode.

## gates

the release train (`soft3/release/train_gates.py`, listed for this repository in `soft3/release/director/plan.md` on `release/phase1`) runs these gates over cybernode; a red gate ships red with its evidence, never hidden:

- `cybernode-tests` — the node's `cargo test` in `rs/` against the pinned siblings
- `cybernode-release` — the release executable of this crate, one per platform
- `node-status` — boots the release binary with `node --home DIR --bind ADDR` and expects `GET /status` 200 with `height` and `chain` within 20 s
- `stack-chain-genesis` — chain-conformance § 10: the pussy genesis file (`SOFT3_GENESIS_DIR`, default `~/.cache/cyber/genesis`) reproduces the blessed particle, state root and block-0 id of `conformance/chain.snap`; blocked, never green, without the file
- `stack-pussy-identity` — `rs/tests/pussy_identity.rs`: the blessed pussy file boots when cached (plan row P4a)

local checks before a commit: `bash -n` on every script under `deploy/`, `nu --ide-check 0 scripts/chain-fleet.nu`, `deploy/chain/tests/run.sh`.
