---
tags: cybernode, task, chain, mining, rewards, pussy
kind: task
status: doing
owner: claude
serves: pussy
lane: wide
date: 2026-11-04
---

# ticket-pow — settlement mining as the chain's proof of work (P15b)

Unit P15b of the director's plan (soft3 `release/director/plan.md` D20, row
P15 (b)): the chain's work is the settlement ticket of tru `specs/rewards.md`
§7–§8 (tru#45, head b1fd3f0), on the game, ordering, probes and commitment
of tru#46 (`feat/coalition-marginal`, head cf5f23b). The amended clauses of
chain.md, chain-economy, chain-wire and chain-conformance are the ones of tru
`audit/rewards-amendments-2026-11-04.md` §1–§2c; the specification files in
this repository are amended in their own pull request (the amendments file
names each entry as one), the code here follows the amended text.

## the claim

Every block at height `h ≥ 1` carries one sample of the game of epoch
`epoch(h) − 1` in its 200-byte header (`sample: [u8; 32]` at offset 168);
the block id over the 200 bytes below the ASERT target is the ticket, the
ordering weight and the subsidy; every full node recomputes the sample, hash
first, under `V_BUDGET` per interval, before relaying or accepting the block;
the settlement of epoch `E` at `boundary(E + 1) + D` pays each player the
mean of its sampled marginals when it has `K_MIN = 64` of them and its exact
path gradient otherwise, clipped, rescaled to `v(N)`, floored, the dust to
the pool; the reference miner produces tickets and grinds nothing else.

## design

- `chain/header.rs`, `chain/params.rs`: the 200-byte header, `TEMPLATE_BYTES
  = 168`, `SAMPLE_OFFSET = 168`; `POW_LIMIT = 2^252`, `GENESIS_BITS` pussy
  `0x20020000`, bostrom `0x20100000`; `W_TICKET` pussy 64, bostrom 2;
  `K_MIN = 64`; `V_BUDGET` pussy 6, bostrom 2; `K_BLOCK = 0`; the clock
  `settle(E) = boundary(E + 1) + D`, `MIN_LOCK ≥ 2L + D`.
- `chain/ticket.rs`: the players of an epoch (ascending by `card ‖ σ`),
  `cluster(E)`, the draw `h = hemera(template)`, `nonce = u64_le(h[0..8])`,
  `r = u64_le(h[8..16]) mod N`; `EpochGame` over tru's `Game` with the
  ticket's `W' + 1` prefix solves and the probes spread across the node's
  cores, bit-identical to tru's sequential `Game::ticket`;
  `sample = hemera(Ticket::preimage())`; `GameWire` for `GET /game/{beacon}`.
- `chain/engine.rs`: chain.md §5.5 step 8 — the sample recomputed on the
  parent state's game and compared, under `V_BUDGET` full verifications per
  `T` seconds (a sliding window of the last interval's verifications), served
  by chainwork, ties among unverified candidates by a draw from the node's
  own salt; a spent budget or a game this node cannot build yet is the
  node's own condition (`ChainError::VerifyBudget`, `ChainError::Game`): the
  block stays stored and is retried on the next sync pass; a failed sample is
  `BlockError::Sample`: the block and its descendants invalid, the id
  remembered as rejected, never re-queued; the peer scored +100
  (chain-wire §5 amended). Relay happens after step 8 (`net/gossip.rs`).
- `chain/economy/lottery.rs`, `settle.rs`, `run.rs`: the games and verified
  tickets cached (`$home/tickets/`), the per-player sums, the payout rule of
  §7 (`a(c)`, `a⁺`, `â` by 512-bit `mul_div`, floored), the settlement keyed
  by the three boundaries, the epoch record extended with `players`,
  `samples`, `by_mean`, `value` (218 bytes); `GET /epoch/{E}/lottery`.
- `chain/mine/`: the reference miner — `GET /template` names the game, `GET
  /game/{beacon}` serves it once per epoch, every attempt is one ticket; the
  Metal and WebGPU hash kernels are removed (below).
- `chain/corpus.rs`: the forged tickets `wrong_marginal`, `wrong_probe`,
  `wrong_preimage_order`, `garbage_sample`, `stale_base`, each
  `reject:block:sample`.
- genesis: `params.bits` of a chain's file is its `GENESIS_BITS`; the
  blessed pussy, pussy-rc and bostrom identities are re-derived from the
  burial snapshots on build-a (conformance/chain-genesis.toml, chain.snap).

## P15b questions

Each ambiguity of §7 met in the code, both readings, and the one taken (the
one closest to tru#46); none is resolved silently.

1. The probe's base. §7 step 2 and tru cf5f23b: the probe is against the
   base `A_{E−1}` (`J_U(φ*(A_{E−1} + u·e_a)) − J_U(φ*_{E−1})`); the
   chain-economy amendment 1.7 text writes `A_E` / `φ*_E`. Taken: `A_{E−1}`,
   tru#46's `Game::probe` on `Game::base`.
2. The rank when `N = 0`. `r = u64_le(h[8..16]) mod N` is undefined for an
   empty game. Taken: `r = 0` and no marginals, every entry a probe (tru#46
   `Game::ticket`).
3. `V_BUDGET` "per block interval": fixed intervals from some clock, or a
   sliding window of `T` seconds. Taken: sliding — a verification is spent
   until `T` seconds have passed since it; a cached ticket (one this node
   computed or verified before) costs nothing and is outside the budget.
4. The GPU. §7 binds the whole template into the seed and the sample into
   the id, so no header byte can vary without a fresh ticket and no hash can
   be ground after the ticket: the `metal` and `webgpu` features and the
   lens `gpu/` dependency are removed from the miner, not kept.
5. The exact gradient's unit. Chain-economy §5.2 forms `a_exact(c) = Δs·σ·ḡ`
   as a 256-bit product whose scale cancels in ratios; §7 compares it with
   the mean of sampled marginals, nats at 2^60. Taken: `a_exact` brought to
   nats at 2^60 by `⌊Δs·ḡ / 2^(grad_bits − 60)⌋` (tru's gradient at 2^112,
   the reference engine's at 2^100), so means and exact values share one
   unit in the rescale.
6. The epoch record. The amendments leave §5.6's 198-byte record untouched;
   the unit's brief names "epoch record fields". Taken: the record carries
   `players`, `samples`, `by_mean` (u32) and `v(N)` saturated to i64 after
   `payouts`, 218 bytes; the per-player estimate is served, not recorded.
7. The genesis file's `bits`. Reading A: the file keeps `0x1e010000` and the
   node anchors ASERT at `genesis_bits(chain)` regardless; reading B: the
   file's `params.bits` is the anchor (chain.md §1.5, §4.3 "`bits =
   GENESIS_BITS`") and carries the chain's value, so the pussy, pussy-rc and
   bostrom identities (particle, sha256, block 0) are re-blessed. Taken: B.
8. Dev and test nets. `W_TICKET`, `V_BUDGET` and `GENESIS_BITS` are named
   for pussy and bostrom only. Taken: a dev net reads `SOFT3_DEV_W_TICKET`
   (2 when unset) and `SOFT3_DEV_V_BUDGET` (unbounded when unset) and anchors
   at pussy's bits; the receipt's `pussy-rc` chain runs with both set.

## log

- 2026-11-04 · first attempt (claude subagent): the header, the ticket, the
  miner, the verifier, the payout and the corpus written in one working tree
  (cybernode `feat/ticket-pow`, soft3 `feat/ticket-pow`), never compiled to
  green; the session died on an API connection drop while writing the
  miner's API module.
- 2026-11-12 · resumed from the saved tree (deliver.md's resume edge): two
  compile errors in the test code fixed, `genesis_target` 2^232 → 2^249,
  the genesis file's `bits` per chain, `GET /epoch/{E}/lottery`; the tree
  split into the commits below, each built and tested on build-a.

## P15b questions (continued)

9. The payout's exact gradient and `v(N)`. §7 "the payout" pays the mean of
   a player's sampled marginals (tru's game, ΔJ in nats at 2^60), rescaled
   to the grand-coalition value `v(N) = J_U(φ*(A_E)) − J_U(φ*(A_{E−1}))`
   over the union particle set, and names the "exact path gradient" as the
   cap and floor of a player the window did not reach. Two readings of the
   exact gradient's source met in the code:
   - A: the adjoint gradient of the settlement engine (chain-economy §5.1,
     `FocusEngine::gradient`), `a_exact = Δs·σ·ḡ`, which the node runs on
     `Tru::for_node`;
   - B: the marginal on the same `tru::epoch::Game` the mean and `v(N)` come
     from, so one engine produces mean, exact and `v(N)`.
   Taken: A, because §5.2 forms `a_exact = Δs·σ·ḡ` from the adjoint and the
   node's engine is tru either way. But the rescale to `v(N)` mixes A's
   exact with the game's `v(N)`: on a toy graph where a particle appears,
   the adjoint (per-axon) and the game (`|U|`-normalised `v(N)`) disagree on
   the sign of the change, so an epoch the adjoint calls "earns" the game
   calls `v(N) ≤ 0` and pays nothing (chain-economy §5.2 amended: nothing is
   paid when `v(N) ≤ 0`, §16 "an epoch without positively attributed active
   stake pays nothing"). This is the behaviour of the two ignored tests
   (`multi_epoch_settlement_end_to_end`,
   `attribution_is_linear_in_stake_and_valence_cancels`). The spec author's
   §15 resolution (P14b) is to fix the exact-gradient source to one engine;
   when it lands the two tests are redrawn to a net-positive-syntropy epoch
   and un-ignored. The payout code follows reading A today; the mean path
   (`k_c ≥ K_MIN`) is unaffected and is what the launch chains run.

## remaining (2026-11-12)

- the two economy tests above are `#[ignore]`d pending §15 (P14b); the §7
  payout path compiles and the mean/rescale/clip/floor/dust arithmetic is in
  `economy/settle.rs`.
- `pussy-rc` two-node dev chain to ≥ 240 blocks with equal epoch records, and
  the receipt `audit/chain/ticket-pow-2026-11.md` (block times, tickets/s,
  verification time, `V_BUDGET` under a forged-header flood): not run.
- the forged-header flood tool (an `examples/` binary that submits
  target-passing headers with random samples to measure `V_BUDGET`): not
  written.
