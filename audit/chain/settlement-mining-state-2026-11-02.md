---
tags: cybernode, audit, chain, mining, settlement
crystal-type: measure
crystal-domain: cyber
date: 2026-11-02
---

# settlement mining — the real state on 2026-11-02

asked by the owner on 2026-11-02 against `specs/chain-economy.md` §7.1 (translated): "I would never agree — explain the tradeoffs; we have to supply complete mining rewards at genesis — check all the associated repositories and report first on the real state." read-only survey of tru, foculus, zheng, cybernode (and soft3 `release/phase1`, which carries the newest node code), cyb, simulation, warriors and `cyber/launch.md`, all fetched on 2026-11-02. references are `repo@branch:path:line`; `rp1` is `origin/release/phase1`.

## verdict

no repository implements settlement mining for the root chain. foculus carries the lottery as a library: tickets on its `release/phase1`, but the fold lottery and per-cluster difficulty are not on its settle path and no ticket subsidy exists; its sound ticket proofs disclose the witness, so checking a ticket costs as much as recomputing it; and it computes the marginal with tru's legacy kernel, a different function from the integer epoch engine consensus runs. the node rejects ticket kinds 8–10 and pays the subsidy to header grinding.

## 1. the pipeline and its owners

| step | spec |
|---|---|
| ε-support cluster | `tru@master:specs/rewards.md:157-163`; `cybernode@rp1:specs/chain-economy.md:761-763`; launch row 35 |
| one Shapley sample per nonce, π(n) = H(b_E ‖ cluster ‖ n) | rewards.md:173-174 |
| win test commit(m(n)) < target; the ticket is the subsidy | rewards.md:175-177, 217; chain-economy.md:764-775 (kind 8) |
| a zheng proof per ticket | rewards.md:181, 185; `foculus@rp1:specs/fold-mining.md` (status draft, :5) |
| cluster fold, O(1) decider | rewards.md:185-195; fold-mining.md:34-100 |
| deadline, k_min, decide | rewards.md:345 ("unspecified"); chain-economy.md:769-772 |
| mint and conservation clip | rewards.md:89 (tok); chain-economy.md §5.5 |

the specs disagree on genesis: rewards.md §16 (:362, :375), chain-economy.md §7.1 (:707-723) and the decision of 2026-10-17 (`cyber@master:launch.md:83, :99`) defer the lottery; launch.md:19 (phase row 1) still reads "with settlement mining, fold".

## 2. each step

| step | code | merged | tests | measured |
|---|---|---|---|---|
| cluster | `partition.rs:85`, called by nothing on rp1; an epoch is one cluster (master `epoch.rs:88-94`) | rp1, unused | — | no |
| sample | `settlement.rs` `value` = `tru::impulse(...).directed`; one attempt is a whole ordering (n marginals) | rp1, master | 5 | 672.8 → 2.1 samples/s at ε-support 8 → 4,096, M4 Max only (`audit/hardware-profile.md:117`) |
| ticket | `tickets.rs` `try_settlement_ticket`, `grind_settlement`, `verify_settlement_ticket` | both | 18 (master) | — |
| proof | `ticket_proof.rs`: R1CS over m[π_j] = v_j − v_{j−1}, the v_j checked natively (:17-23) | rp1 via foculus#88/#89 (P6); master on the legacy fold (`ticket_proof.rs:15-16`); #61, #63, #66 open | 8 | 45 B / 0.26 ms (n = 1) to 549 B / 810 ms (n = 64) (`audit/sound-settlement-proofs-2026-10.md:120-125`) |
| fold | `cluster::verify_cluster` checks every ticket; `cluster_accumulated` behind an off feature | rp1 | 5 + 4 | 512 tickets at n = 8: 14.4 s, 51,712 B native; accumulated 2.36 MB, 33.6 s (`audit/accumulated-settlement-2026-10.md:64-83`) |
| settle | `settle_tickets.rs:55`; one end-to-end test, `easy_target`, 2 contributors, 4 samples, stops before the mint | rp1 | 4 | — |
| difficulty | `banded_target` arithmetic only; live paths use `easy_target()` (`live.rs:243`, `epoch_cert.rs:211`) | — | `progress_freedom` `#[ignore]` (:68) | banded ratio 1.96–2.45 vs flat 15.7–19.6 (`progress-freedom-banding.md:67-68`) |
| mint | tok `mint.rs:55`, called only by foculus; `pow_subsidy_shares`, `mint_receipt_to_ledger` have no callers | — | — | — |
| chain | kinds 8–10 rejected (`cybernode@rp1:rs/src/chain/tx.rs:182`); the miner grinds hemera headers (`mine/mod.rs:1-13`) | — | — | — |

simulation has no ticket scenario (`simulation/audit/network-2026-10.md:75-78`); cyb's body tickets are SpMV or recomputation, not Shapley samples (`cyb/shell/src/worlds/body/prover.rs:27`, cyb#1407/#1408 open); warriors measure uhash only. every ticket number comes from one M4 Max; no ticket measurement exists at pussy's size. the pussy-scale figure is consensus recompute: 0.12 s on one thread for 26,805 axons / 16,270 particles (`cybernode@rp1:audit/chain/economy-2026-10-17.md:96`, one-node midpoint; the spec now uses four nodes).

## 3. the proofs

- the ticket relation on rp1 is sound and not succinct: the v3 public certificate discloses the witness (sound-settlement audit :66-73); the impulse has no constraint system (`ticket_proof.rs:20-23`), so verifying is recomputing; profile 1 refused, relation not closed (`device-profile-choice-2026-10.md:69-70, 92`).
- retired as unsound: the legacy fold/decide (`zheng@master:specs/soundness.md:165`); launch row 3's 18.7 / 9.2 ms are of that fold.
- the φ* relation: one pussy solve is 11–14 G machine rows, ≈ 2.7 years on the CI runner, "not viable" (`tru@rp1:audit/phi-relation-2026-11.md:58-61`); D18 chose a direct CCS step plus IVC, after genesis.
- fold is linear: accumulation grows ≈ 96 KB per segment, 512 statements 8.9 MB, verify 1.17 s (zheng `audit/accumulation-2026-10.md`); constant size exists only as IVC (profile 5) or wrap (profile 6, ≤ 64,213 B, 37–97 s, 14 GB), used by no ticket; rewards.md:343's "O(1) accumulator" is not backed by code.

## 4. the economics

- launch row 3 (ticket reward): unpriced; no code computes a ticket subsidy.
- row 4 (difficulty per cluster): deferred; the banding exponent open.
- row 7 (withholding): bound holds on a synthetic 400-ticket pool; forfeit pricing and role separation open.
- the split: rewards.md pays the PoW share B(1 − θ^α) for proven settlement work (:252, 270-272); chain-economy.md pays the header miner SUBSIDY − cut + fees (:618), stakers from the pool by attribution, nobody for the settlement compute (:714-717); §7.3 moves the PoW share to the ticket while "the header keeps a hash target for ordering" (:773-775) and does not say who pays header grinding then.

## 5. tradeoffs, lottery at genesis against recomputation at genesis

measured: recompute 0.12 s (pussy) and 75 s (bostrom) against a 3,600 s window (chain-economy.md:737); four-node attribution median error ≤ 5 % against exact Shapley (`tru@rp1:audit/epoch-settlement-2026-10.md` §3, §6); cluster verification linear, 28 ms per ticket at n = 8.

inferred: the work becomes useful only when nodes stop recomputing, which needs a constraint system for impulse/φ* (after genesis by D18) — until then the lottery pays miners to sample what every node computes exactly; giving tickets a share of the subsidy takes it from the header budget that orders the chain, with few miners on pussy; a ticket costs as much to verify as to make (rewards.md:185), so without an O(1) fold the bound is a per-block cap plus the target; foculus's marginals (legacy kernel, per contributor batch) and consensus's (epoch engine, per card) are different functions.

unknown: proof cost against reward; throughput on x86, phones, GPUs; ticket verification cost at pussy's size; tickets across nodes; time to implement.

## 6. the shortest path for pussy

1. owner specs (D6): m(n) defined on the consensus epoch engine, per card; the header/ticket subsidy split; payouts as the ticket mean or the exact recompute (§7.3 is a sketch, :754-782).
2. tru: a coalition-marginal entry point on the epoch engine (only the whole-epoch `path_gradient` exists, `epoch/mod.rs:9-13`).
3. cybernode: kind 8 format, codec, validity, acceptance window, per-block cap, the mint in `apply.rs`, conformance vectors — none exists.
4. ticket verification in the node (foculus's sits on the other kernel).
5. difficulty: retarget toward a sample count per epoch; the k_min deferral (:772).
6. withholding: forfeit or role separation.
7. behaviour: a fleet run, a multi-machine rehearsal, a simulation scenario.

options at pussy's size, for the owner, not recommendations: tickets verified by recomputation with no zheng proof (≈ 0.07 s per marginal, estimated); no fold, a cap on tickets per epoch; one cluster per epoch (foculus's live path already does it, rows 35 and 4 moot); tickets distribute the PoW subsidy while the mint to stakers stays the exact recompute; a header share of the subsidy kept for ordering security.

not verified: whether chain-economy §10's two-machine 24-hour determinism gate ran (it names deimos, retired); P7's x86 and macOS-CPU legs; whether cyb master builds.
