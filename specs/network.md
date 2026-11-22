---
title: network repository
tags: cyber, soft3, spec, cybernode, network, genesis
crystal-type: spec
crystal-domain: cyber
alias: network repository, network standard, network.toml, the input of cybernode, network manifest
---

# network repository

A network is one repository. It holds everything that makes this network itself and nothing of how a node runs. cybernode takes it as its input for two acts: to raise the network, and to join it.

This page is the standard of that repository: its layout, the schema of each file, the lock, and what the node does with them. The values belong to the network; the format and the vocabulary of rules belong to this page. It is phase N0 of soft3 `projects/network-repos.md`.

## 1. the layout

```text
<network>/
  network.toml          identity and rules                                   required
  genesis/
    sources.toml        the pinned inputs of block 0, or none                required
    distribution.toml   who holds what at block 0, as rules                  required
    graph.toml          the starting graph, as rules                         required
    genesis.lock        the result, by digest; its particle is the chain id  required
    README.md           how to rebuild and check                             required
  endpoints.toml        where the network is met                             required
  contracts/            the programs carried at genesis, with their digests  when the network carries any
  audit/                receipts, the build receipts of every lock first     required
  docs/
    history.md          where the network comes from                         required
    economics.md        the token's economy, the planned beside the running  required
    run-a-node.md       how to stand a node of this network                  required
  site/                 the network's site: static files, no build           when it has one
  snapshot/             the burial the genesis is built from, with its tools when it has one
  README.md             the door
  AGENTS.md             what lives here and when a value may change
```

No node code lives in a network repository. A change to how a network runs is a change of cybernode; a change to what a network is, is a change of its repository.

## 2. `network.toml`

Every key is required unless marked. Integers are whole; a time is in seconds, a depth and a length in blocks.

| table | key | type | meaning |
|---|---|---|---|
| `network` | `name` | string | the network's name, lower case; it names the node's `--network` |
| | `hrp` | string | the prefix of a legacy address |
| | `denom` | string | the native coin |
| | `genesis_time` | integer | the time of block 0, unix seconds |
| `rules` | `rule_set` | integer | the rule set of block 0 |
| | `activations` | list of `[rule set, height]` | which rule set from which height ([chain](chain.md) §7.1) |
| `authorization` | `scheme` | string | what authorizes a signal: `NSIG1`, or a proof scheme a rule set names |
| `work` | `block_time` | integer | the target interval |
| | `half_life` | integer | of the difficulty correction |
| | `mtp_window` | integer | blocks of the median time past |
| | `max_future_drift` | integer | seconds a block's time may lead |
| | `genesis_bits`, `pow_limit_bits` | string, hex | the first target and the easiest |
| `blocks` | `header_bytes`, `max_block_bytes`, `max_block_signals`, `max_signal_bytes` | integer | the sizes |
| | `min_fee_per_byte` | integer | the fee floor |
| | `nonce_window`, `orphan_pool_headers`, `orphan_pool_seconds` | integer | admission |
| `finality` | `coinbase_maturity`, `finality_depth`, `max_reorg` | integer | `max_reorg = finality_depth − 1` |
| | `checkpoint_interval`, `checkpoint_sunset` | integer | the checkpoints of [chain](chain.md) §5.3 |
| | `checkpoint_key` | string, hex or empty | the key that signs them; empty when the network has none |
| `emission` | `blocks_per_year`, `subsidy_divisor`, `subsidy` | integer | `subsidy = floor(genesis supply / subsidy_divisor)` |
| `epoch` | `length`, `settle_delay`, `min_lock`, `settle_budget_seconds` | integer | the clock of [chain-economy](chain-economy.md) §2 |
| `focus` | `phi_steps`, `attribution_nodes` | integer | the epoch engine |
| `rewards` | the reward parameters of the rule set | integers | [chain-economy](chain-economy.md) §0; the keys follow the rule set |
| `ports` | `api`, `radio` | string, `host:port` | defaults of a node of this network; local, they enter no digest |

Everything but `ports` is consensus: two nodes that differ in one value are on two networks. The identity keys and `genesis_time` enter the genesis file and so the chain id; the rest binds through the rule set the chain id commits to.

A network may declare only values its rule set admits. The bounds are the rule set's: a block time the work cannot keep, a finality depth of zero, a subsidy that is not the function of the genesis supply are refused by `network check`.

## 3. `genesis/sources.toml`

```toml
[burial]                  # absent when the network is built from nothing
chain = "…"               # the chain that was buried
halt_height = 0
halt_time = "…"
manifest = "snapshot/manifest.json"
manifest_sha256 = "…"     # the pin: the manifest pins every file by content address, sha256 and size
site = "https://…"        # the fallback source

[burial.read]
files = ["…"]             # the files of the snapshot the build reads
layout = "…"              # how a file is addressed under its pin
cache = "~/.cache/cyber/snapshots/<name>/"

[states]                  # absent unless the network is built from other networks
networks = ["…"]          # the networks whose proven states it reads, by chain id
```

A build fetches every file by its pin and verifies it by sha256 and size, cached or not. A source that is not pinned is not a source.

## 4. the rules of block 0

`genesis/distribution.toml` and `genesis/graph.toml` are rules from this vocabulary, with the network's values. A rule outside the vocabulary is refused; a new rule is a change of cybernode.

| file | table | rule | what it does |
|---|---|---|---|
| distribution | `owners` | `with_secp256k1_key` | a key known to the burial becomes `neuron_of(pubkey)`, re-derived and refused on mismatch |
| | | `without_key` | an account that never signed becomes `legacy_owner(hrp, account)` |
| | `balances` | `carry` | which denoms of the burial become tokens |
| | | `delegated`, `unbonding` | staked and unbonding coins return to their accounts |
| | | `liquidity_positions` | a position is dissolved into its pool's reserves at the halt |
| | | `reserve_account` | how the reserve account of a pool is found |
| | | `pool_coins` | dropped |
| | | `exclude` | a denom that does not fit the amount's width is excluded whole and reported |
| | | `check` | the sum of every carried denom equals the burial's supply |
| | `supply` | `genesis_supply`, `subsidy` | what the native supply is, and the subsidy as its function |
| | `claims` | `from`, `rate`, `once` | a network built from other networks' states mints by claims against them |
| graph | `particles` | `readdress` | how an old content address becomes a particle |
| | | `naming_links` | whether a link names the old address |
| | `links` | `order`, `passports`, `restored`, `duplicates`, `stake` | which links block 0 carries, in which order, with what stake |
| | | `empty` | a network that starts with no graph |

Each file ends with a `result` table: the counts the rules produce from the pinned sources. They are checked by the build and are not inputs.

## 5. `genesis/genesis.lock`

```toml
[genesis]
particle = "…"            # the hash of the genesis file: the chain id
file_sha256 = "…"
file_bytes = 0
time = 0
block0_id = "…"
state_root = "…"

[contents]                # the counts and the supply, for a reader
[built_with]              # the pipeline, the hash and the state version of the build
```

The genesis is a function of the repository: the sources are pinned and the rules are fixed, so two builds give the same bytes. The lock is that result. It changes only when an input changes, and only together with a receipt in `audit/` of two independent builds with equal bytes. The genesis file itself is built, never committed.

## 6. `endpoints.toml`

`[public]` names the site, the explorer and its API; `[node]` repeats the default ports; `[peers]` carries `bootstrap`, a list of radio endpoint ids with their addresses. A network that is not launched has an empty list.

## 7. what the node does with it

To raise a network:

1. `cybernode network check <repo>` — every file parses, every key is known, every value is inside the rule set's bounds, every rule is in the vocabulary, the manifest's digest is the pinned one.
2. `cybernode genesis build --network <repo>` — the sources fetched and verified, the rules applied, the genesis file written with its report.
3. the particle, the block-0 id and the state root are compared with the lock. A first build writes the lock with its receipt; a later build that differs stops.
4. `cybernode run --network <repo>` — block 0 is that file, and the node runs by `network.toml`.

To join a network:

1. the node takes the repository at a revision, or a chain id it knows a lock for.
2. it obtains the genesis file — built from the repository, or fetched by its particle — and refuses a file whose particle is not the lock's.
3. it meets the peers of `endpoints.toml` and syncs. A peer on another chain id is another network and is dropped.

The node knows no network by name: ports, peers, parameters and the genesis all come from the repository.

## 8. the three networks

| network | coin | repository | sources |
|---|---|---|---|
| pussy | PUSSY | `space-pussy` | the burial of the space-pussy chain, halted at 19,940,993 |
| bostrom | BOOT | `bostrom` | the burial of the bostrom chain, halted at 25,120,712 |
| cyber | CYB | `true-cyber` | the proven states of the two above: CYB is minted by claims |

## 9. where the node stands

The node does not read a repository yet. This table is the distance, and it is closed by the phases N1 to N3 of the project.

| the standard | the node today |
|---|---|
| `network.toml` | constants of `rs/src/chain/params.rs`, one set for every network |
| `[ports]`, `endpoints.toml` | a `match` on the network's name in `rs/src/cli.rs` |
| the rules of `genesis/` | code of `rs/src/genesis/`, described in [genesis](genesis.md) § derivation |
| `genesis.lock` | soft3 `conformance/chain-genesis.toml`; the binary carries zero ids and `--genesis` is mandatory |
| `--network <repo>` | `--chain pussy\|bostrom` with `--snapshot DIR` and `--genesis FILE` |
| `network check` | no command |

The repositories of pussy and bostrom carry the files today, filled with the node's own values, and say in their headers that the node does not read them yet.

## 10. open

- the network key: who signs `endpoints.toml` and the lock.
- the bounds of a rule set: which values a network may set within it, and which only a new rule set changes.
- the reward keys: they follow soft3 `projects/complete-rewards.md` and change with it.
