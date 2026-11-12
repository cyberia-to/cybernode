---
tags: cybernode, audit, chain, mining, rewards, pussy
crystal-type: report
crystal-domain: cyber
status: measured
date: 2026-11-12
---

# ticket-pow — settlement mining on two nodes (P15b receipt)

The settlement ticket of tru `specs/rewards.md` §7–§8 is the chain's proof of
work (cybernode#18, soft3#682). This receipt records two nodes mining by
tickets on build-a's release binary, on a real-scale chain (pussy-rc,
26,805 axons) and a short-clock dev chain (equal epoch records reached).

## revisions

- cybernode `feat/ticket-pow` `f42382d` (the review-round-1 head); binary
  sha256 `1203ec6787d304c9325e0f0d36c99f8e2ee98432a1b1c423ec5bec1414e67b87`,
  built on build-a (`cargo build --release`, glibc 2.41, x86_64).
- tru `feat/coalition-marginal` `3cd4e45` (the epoch engine, hemera 0.4).
- genesis `pussy-rc.tade` sha256
  `2923fcd3a76abcf3b1906c5830c39556dc08cde2707f017dad7854d070fc6388`
  (`cybernode genesis build --chain pussy-rc --time 1793523600`), block 0
  `ff4b56ea65f8b81f86c8e58b7d3c84d1fe4e48676358efe720fa21f8ebf213b3`,
  `GENESIS_BITS = 0x20020000` (2^249), `W_TICKET` 64, `V_BUDGET` 6.

## host and layout

agent-a (32 cores, Debian glibc 2.41, container), under
`/srv/forge/claude/chain-dev/` as user `claude`, loopback only, never
touching the alpha's `pussy-rc` services or ports 7783/7793:

- pussy-rc: node dev-a `127.0.0.1:17781` (radio 17791), node dev-b
  `127.0.0.1:17782` (17792); each runs a `cybernode mine … --workers 12`.
- dev (short clock): node fa `17784`/`17794`, node fb `17785`/`17795`,
  `SOFT3_DEV_EPOCH=10:5:25 SOFT3_DEV_W_TICKET=2 SOFT3_DEV_V_BUDGET=6`.

Commands: `cybernode node --chain <c> --home <h> --genesis <f> --bind … --radio … --peers …`
and `cybernode mine --chain <c> --node http://… --address <neuron> --workers N`
(the miner has no backend — an attempt is a settlement ticket). The peers
file lists the sibling's endpoint id (`cybernode node id --home …`).

## pussy-rc (real scale, 26,805 axons)

- the game: `GET /game/{beacon}` built in 0.20 s (0 players, 26,805 axons,
  one cluster) — epoch 0's game, the genesis graph the first epoch samples.
- an attempt is `W_TICKET + 1 = 65` cold solves of the 26,805-axon engine.
  On agent-a's 12 shared workers the chain made about one ticket a second
  aggregate; at `GENESIS_BITS` 2^249 (≈128 expected attempts) a block landed
  about every 120–160 s (header times: h10 1794493013, h14 1794493500
  → 122 s/block, h14→h18 1794494147 → 162 s/block), consistent with tru#46
  §3.2 (1.59 samples/s on 32 Xeon workers) scaled to 12 loaded workers.
- verification is one sample recompute per block by **every** node, hash
  first: tru#46 `audit/coalition-marginal-2026-11-04.md` §3 measures it at
  7.8 s on one build-a core and 2.15 s on one M4 Max core at this size; the
  live evidence is that **both nodes accepted every one of the other's
  blocks** — dev-a and dev-b held the identical tip at each height checked
  (h18 `950dc123…`, h19 `5f77a264…`, h20 `3f2c769c…` chainwork 412 on both),
  which is only possible if each recomputed the other's sample and it matched.
- the first settlement of pussy-rc is at height 600 (`L=240`, `D=120`:
  `settle(1) = boundary(2)+120`), about a day at this block time; it was not
  reached in the receipt window. Equal epoch **records** on two nodes are
  shown on the short-clock chain below and in the lib test
  `two_nodes_agree_on_every_epoch_and_a_restart_replays_it`.

## dev (short clock) — equal epoch records on two nodes

With `L=10, D=5` the first settlement is at height 25. Both nodes mined by
tickets past height 270 and held matching tips within two blocks. The epoch
record digest (`Record::digest`, the consensus bytes) is byte-identical on
the two nodes at every epoch checked:

| epoch | settled at | dev-a digest | dev-b digest | |
|---|---|---|---|---|
| 1 | 25 | `64ebcbef…c48efc4` | `64ebcbef…c48efc4` | equal |
| 5 | 65 | `55652a90…674e564` | `55652a90…674e564` | equal |
| 10 | 115 | `f26ffc36…306f7f4` | `f26ffc36…306f7f4` | equal |
| 20 | 215 | `e9b63b3a…26d0bd3` | `e9b63b3a…26d0bd3` | equal |

These epochs are empty games (no stakes on the dev net), so each pays 0 and
the record's value is its determinism: two independent nodes settle to the
same bytes. A staked settlement with a nonzero payout is exercised in-process
by `multi_epoch_settlement_end_to_end`.

## R18-2 observed live

Under the pre-review binary, dev node-a stalled at height 16 while node-b
reached 228: initial sync verified under `V_BUDGET` (6 per 30 s), so a node
behind could not catch up. On the post-review binary (`f42382d`), node-a
replayed and synced at full speed — it went from 16 to 271 within the first
minute and then tracked node-b within two blocks. This is the fix of R18-2:
`V_BUDGET` governs only gossip headers competing at the tip.

## R18-4 note

Sample verification still runs under the chain mutex; one recompute (~8 s on
a shared build-a core at pussy's size) holds it. The budget (6/interval) and
the unverified store cap (`UNVERIFIED_CAP = 512`) bound the exposure; moving
the solve to `spawn_blocking` (verify, then lock to apply) is the follow-up.

## forged-header flood

`rs/src/chain/tests/flood.rs` is the in-process conformance test (the class
of the corpus generator's invalid blocks; it opens no socket and runs only
against a 127.0.0.1 test chain): a zero sample is refused before a budget
slot, at most `V_BUDGET` forged target-passing headers are recomputed per
window, the unverified set stays bounded, no forged header becomes the tip,
and the honest block is served once the window renews.
