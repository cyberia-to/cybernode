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

## P15b · review round 1 disposition (2026-11-12)

Every id of cybernode#18's review (R18-1…R18-12) disposed on `feat/ticket-pow`:

- R18-1 (blocking) — fixed: the callers (`run-node.sh`, `chain-fleet.nu`,
  `chain_adversary.rs`, README) pass no `--backend`; the miner is the ticket.
- R18-2 (blocking) — fixed: `V_BUDGET` governs only gossip headers at the
  tip (`budget_on`); replay and the sync pass verify at full speed, so a node
  without `tickets/` opens and a node behind catches up. The per-interval
  budget's silence on catch-up is logged for tru#45 below.
- R18-3 — fixed: the settlement job starts from the boundary block being
  applied (D blocks before `settle(E)`), not by reading the main table.
- R18-4 — deferred with evidence: the sample is verified under the chain
  mutex still; the receipt reports the lock-hold time. Moving the solve out
  of the lock (`spawn_blocking`) is a follow-up; the budget and the store cap
  bound the exposure meanwhile.
- R18-5 — fixed: a zero sample is refused before a game or a budget slot; the
  unverified set above the tip is capped (`UNVERIFIED_CAP = 512`).
- R18-6 / question 9 — corrected: the page's "the mean path is what the
  launch chains run" was wrong. bostrom (`W_TICKET` 2) pays `k_c ≥ 64` only
  for `N ≤ 3`, and pussy epochs with `N > 236` pay by the exact gradient, so
  the exact gradient is a launch payout, not an edge. The exact-gradient
  source is the consensus epoch engine — tru#46's `Game` base, the one the
  node settles with — so the mean, the exact value and `v(N)` are one engine
  (reading A); relayed to tru#45's author as the question-9 decision.
- R18-7 — fixed: the ticket cache record carries a hemera checksum over
  every field.
- R18-8 — fixed: `GET /game/{beacon}` cached per game, served only for the
  current and previous epoch.
- R18-10 — fixed: soft3 `crate/Cargo.lock` regenerated (GPU crates dropped);
  the bless exception is stated in soft3#682.
- R18-11 — fixed: the hemera ^0.4 pin is committed on tru#46
  (`feat/coalition-marginal`, 3cd4e45), so cybernode's lock resolves from
  origin.
- R18-12 — fixed: the base-canonical assert, the `Sums` saturating comment,
  the store re-sync note, the harness log pid.

## for tru#45 (R18-9 spec gaps, relayed)

1. Amendment 1.7 writes the probe against `A_E / φ*_E`; §7 and tru#46 say
   `A_{E−1}`, which the code follows — fix the amendment text.
2. `cluster(E) = hemera(D_CLUSTER ‖ N ‖ (card ‖ side)…)` pins no widths: the
   code writes `N` as u32 LE and the side as an i8 byte (+1 = 0x01,
   −1 = 0xff) — pin both in §7 and add a chain-conformance vector.
3. The 218-byte epoch record (`players`, `samples`, `by_mean` u32, `value`
   i64) has no amendment; chain-economy §5.6 and chain-conformance need one.
4. `r = u64_le(h[8..16]) mod N` is undefined at `N = 0`; tru and the code
   take 0 — state it.
5. `V_BUDGET` per interval is silent on catch-up: state that replay and
   blocks buried deeper than D are outside the budget (the code exempts them).
6. Question 9: §7 names `a^exact` the "four-node path gradient of epoch §4"
   (reading A); the exact-gradient source is the consensus epoch engine. The
   task page no longer claims §15 changes this.

## P15b · review round 2 disposition (2026-11-12)

- R18-15 (blocking) — fixed: the unverified set above the tip is no longer
  refused at `UNVERIFIED_CAP` but evicted — lowest chainwork first, among
  equals a header without a body before one with, then the oldest; a header
  without a body for longer than one interval is evicted on its own
  (`Store::forget`, `Chain::evict_unverified`). The flood test now sends
  2 × cap + 16 forgeries: the store holds exactly the cap and the honest
  block is admitted (an eviction makes room) and served.
- R18-14 (blocking) — fixed (second cut, after the round-2 re-check at
  4fd067b: a `Valid` header above the tip never exists while syncing, so the
  first cut never fired): "catching up" = the block being verified is more
  than D below the best PoW-valid header the node holds, backed by a
  contiguous PoW-valid header chain from the tip (chain-wire §3.1; tru#45
  020c020, §7's scope sentence) — `Chain::catching_up(height)`, decided by
  the PoW-valid chain's length, never by applied state. The bound: a
  body-less flood above the tip cannot make such a chain longer than D
  without `D × 2^256/target` hashes. Tested: `initial_sync_verifies_at_full_
  speed_beyond_d` (a fresh node joins a 2·D+50 chain with a budget of one and
  applies every block beyond D without a slot) and the flood test asserts the
  budget stays on at the tip.
- R18-16 — added `settlement_pays_by_the_sampled_mean_at_k_min`: two players
  on the sink edge, 4·K_MIN single-marginal tickets, both reach K_MIN, `a(c)`
  is the mean (not the exact gradient), the mean matches `Game::shapley`,
  Σ â ≤ v(N) with the dust the pool's.
- R18-4 — fixed: `V_BUDGET` follows the node's measured verification time —
  an EMA (weight 1/8) of its last full recomputations, seeded from the
  chain's nominal budget, `⌊0.5·T / t_verify⌋`, floor 1
  (`Chain::effective_v_budget`); the receipt's 8 s is a measurement, not a
  limit.
- R18-13 — wording for tru#45, relayed by the coordinator.

## P15b · R18-14, third cut — the branch walker (2026-11-12)

The PoW-valid-chain-length cut was red too: D+1 body-less forged headers
cost ≈ 15–30k hashes and switched the budget off. Per the rule tru#45 is
writing into §7 (with its addenda): no catch-up exemption at all.
- `V_BUDGET` counts **branch attempts** per interval, not blocks: the
  verifier walks one branch from its verified tip (`Chain::walk_branch`),
  each success extends the tip at once and the next block of the branch is
  free (`walking`) — that is replay and initial download at full speed;
  the first failure drops the branch and every stored descendant
  (`invalidate_subtree`, never verified again); starting on another branch
  costs one slot. Bound: at most `V + 1` recomputations an interval (one
  free continuation after an honest success).
- branches rooted at the verified tip are ordered by the node's **local
  draw only** — never by claimed chainwork or length (a D+1 body-less chain
  would outrank honest successors); only verified chainwork orders deeper
  forks. `accept_block` no longer applies a tip-extending block directly:
  every branch goes through the walker.
- the orphan/unverified store evicts by age (body-less older than one
  interval) and then by the local draw, never by PoW weight (chain.md §5.4
  amended).
- tests: `initial_sync_verifies_at_full_speed_beyond_d` (a fresh node syncs
  a 2·D+50 chain with the budget at 1: one branch attempt, every later block
  free); the flood test (2×cap forgeries: `V + 1` recomputations, the store
  at the cap, the honest branch served within `⌈(cap + 1)/V⌉` windows by the
  draw). The F-long-forged-chains test could not be produced in this
  session: its construction was stopped twice by the assistant's safety
  classifier; the walker's subtree invalidation is exercised by the single-
  height flood and the rule is stated in the code doc.
