//! Epoch settlement (chain-economy §2.4, §5; rewards.md §7 "the payout"):
//! the boundary snapshot, the solves and the attribution per position — a
//! pure function of two snapshots and the window's tickets, run off the
//! block path ([`compute`]) — and the settlement step of the block at
//! `settle(E) = boundary(E + 1) + D` ([`apply`]), which only divides the
//! pool's budget by the precomputed weights and writes the result.
//!
//! The payout rule (rewards.md §7): a player with `K_MIN` sampled marginals
//! in the window is paid their mean, one with fewer its exact path gradient;
//! negative shares clip to zero; when the clipped sum and `v(N)` are both
//! positive the shares are rescaled to `v(N)`, floored, the dust to the
//! pool; otherwise nothing is paid and the pool carries.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::params::{DOMAIN_EPOCH, K_MIN, POOL_RELEASE, R_MAX_PPM, RHO_LAUNCH};
use super::super::state::{Id, StateBackend, StateError};
use super::focus::{FocusEngine, FocusGraph};
use super::lottery::{Lottery, PlayerEstimate, Sums};
use super::weights::{GenesisWeights, adjacency, midpoint};
use super::wide::{W, mul_div};
use super::{Mark, decay_mark, focus_bps, focus_key, payout_leaf, payouts_root, phi_digest, side_of, stake_key};

/// One position as the boundary state holds it (§2.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub card: Id,
    /// the axon-particle `hemera(p ‖ q)`
    pub axon: Id,
    pub p: Id,
    pub q: Id,
    pub owner: Id,
    pub valence: i8,
    pub stake: u64,
}

/// The frozen boundary state `G_E` (§2.4): what settlement reads and
/// nothing else. Genesis weights are the chain's constant, not copied here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub height: u64,
    pub block_id: Id,
    pub state_root: Id,
    /// ascending by card id
    pub positions: Vec<Position>,
    /// `H⁺, H⁻` per axon as left by the previous settlement, by axon id,
    /// with the axon's endpoints
    pub marks: BTreeMap<Id, (Id, Id, Mark)>,
    /// Σ stake over cards with `v ≠ 0`
    pub active: u64,
    /// `S0 + boundary(E) · SUBSIDY`
    pub supply: u64,
}

impl Snapshot {
    /// Freeze the state after a boundary block.
    pub fn capture(state: &impl StateBackend, epoch: u64, block_id: Id, state_root: Id, supply: u64) -> Snapshot {
        let mut positions = Vec::new();
        let mut active = 0u64;
        for (card, c) in state.cards() {
            let (p, q) = state.axon_edge(&c.particle).expect("a card's axon is in axon_edges");
            let stake = state.raw_balance(&stake_key(&card));
            if c.valence != 0 {
                active += stake;
            }
            positions.push(Position { card, axon: c.particle, p, q, owner: c.owner, valence: c.valence, stake });
        }
        let marks = state
            .marks()
            .into_iter()
            .map(|(axon, m)| {
                let (p, q) = state.axon_edge(&axon).expect("a marked axon is in axon_edges");
                (axon, (p, q, m))
            })
            .collect();
        Snapshot { epoch, height: state.height(), block_id, state_root, positions, marks, active, supply }
    }

    /// `(S⁺, S⁻)` per axon, by axon id, with the endpoints (§4.2).
    pub fn sides(&self) -> BTreeMap<Id, (Id, Id, u64, u64)> {
        let mut out: BTreeMap<Id, (Id, Id, u64, u64)> = BTreeMap::new();
        for pos in &self.positions {
            let e = out.entry(pos.axon).or_insert((pos.p, pos.q, 0, 0));
            if pos.valence < 0 {
                e.3 += pos.stake;
            } else {
                e.2 += pos.stake;
            }
        }
        out
    }

    /// `A_pq` over every present axon of this snapshot (§4.2), ascending.
    pub fn adjacency(&self, weights: &GenesisWeights) -> Vec<(Id, Id, u128)> {
        let by_pq: BTreeMap<(Id, Id), (u64, u64)> = self.sides().values().map(|(p, q, pos, neg)| ((*p, *q), (*pos, *neg))).collect();
        adjacency(&weights.axons, &by_pq)
    }
}

/// φ*_E as the node keeps it for `/focus` (§11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusOut {
    pub particles: Vec<Id>,
    pub phi: Vec<u64>,
    pub syntropy: i128,
    pub digest: Id,
}

/// One position that earns this epoch (§5.4): its weight `x(c)` and cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Earner {
    pub card: Id,
    pub owner: Id,
    pub x: W,
    pub cap: u64,
}

/// Everything settlement of epoch E needs except the pool balance, which
/// is read at `settle(E)` (§5.5): a pure function of `G_{E−1}`, `G_E`, the
/// genesis weights and the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettleResult {
    pub epoch: u64,
    pub prev_block: Id,
    pub boundary: u64,
    pub beacon: Id,
    pub boundary_root: Id,
    pub particles: u32,
    pub axons: u32,
    pub phi: Id,
    pub syntropy: i128,
    pub prev_syntropy: i128,
    /// `focus_bps(E)`, the split in force for the blocks of epoch E + 1
    pub focus_bps: u64,
    /// ascending by card; `x > 0` only
    pub earners: Vec<Earner>,
    /// the full mark set `H_E`, ascending by axon id
    pub marks: Vec<(Id, Mark)>,
    pub focus: Arc<FocusOut>,
    /// `v(N)`, the game's value in nats at 2^60 (rewards.md §7)
    pub value: i128,
    /// `N`, the players of the game
    pub players: u32,
    /// Σ k_c: the sampled marginals the window delivered
    pub samples: u32,
    /// players paid by their mean (`k_c ≥ K_MIN`)
    pub by_mean: u32,
    /// the lottery's estimate per player, players ascending (served, not
    /// written into the record)
    pub lottery: Vec<PlayerEstimate>,
}

/// The solves and the attribution (§5.1–§5.4), paid by the window's
/// tickets (rewards.md §7). `prev_syntropy` is `J(φ*_{E−1})` when the node
/// still holds it; otherwise it is solved.
pub fn compute(prev: &Snapshot, cur: &Snapshot, weights: &GenesisWeights, engine: &dyn FocusEngine, prev_syntropy: Option<i128>, lottery: &Lottery) -> SettleResult {
    compute_with(prev, cur, weights, engine, prev_syntropy, super::super::params::ATTR_NODES, lottery)
}

/// [`compute`] with `quadrature` gradient nodes on the path from `A_prev`
/// to `A_cur` (1 = the midpoint of chain-economy §5.1).
pub fn compute_with(
    prev: &Snapshot,
    cur: &Snapshot,
    weights: &GenesisWeights,
    engine: &dyn FocusEngine,
    prev_syntropy: Option<i128>,
    quadrature: usize,
    lottery: &Lottery,
) -> SettleResult {
    debug_assert_eq!(lottery.game.beacon, cur.block_id, "the lottery's game is this epoch's");
    let a_prev = prev.adjacency(weights);
    let a_cur = cur.adjacency(weights);
    let a_mid = midpoint(&a_prev, &a_cur);

    // solve 1: φ*_E (published); J(φ*_{E−1}) from the previous epoch or solved
    let g_cur = FocusGraph::build(&a_cur);
    let focus = engine.focus(&g_cur);
    let prev_syntropy = prev_syntropy.unwrap_or_else(|| engine.focus(&FocusGraph::build(&a_prev)).syntropy);
    // solves 2 and 3: the fixed point of A_mid and the adjoint gradient
    // there (one quadrature node); k > 1 nodes integrate the gradient
    // along the straight path, Gauss–Legendre, weights at 2^32
    let path = gradient_path(&a_prev, &a_cur, &a_mid, quadrature, engine);
    let grad_of = |p: &Id, q: &Id| -> i128 {
        match path.binary_search_by(|(x, y, _)| (x, y).cmp(&(p, q))) {
            Ok(i) => path[i].2,
            Err(_) => 0, // absent at every node ([A_mid_pq > 0] fails): no credit
        }
    };

    // the players (§5.2, rewards.md §7 "the game"): one per card and side
    // with Δs ≠ 0, ascending by card ‖ σ — the game's own index space
    let game = &lottery.game;
    let players = &game.players;
    let mut raw_increase: BTreeMap<(Id, i8), u128> = BTreeMap::new(); // R^σ per axon side
    for pl in players {
        if pl.delta > 0 {
            *raw_increase.entry((super::axon_of(&pl.p, &pl.q), pl.side)).or_insert(0) += pl.delta as u128;
        }
    }

    // the new height of each side (§5.3)
    let sides = cur.sides();
    let side_stake = |axon: &Id, sigma: i8| -> u64 {
        sides.get(axon).map(|(_, _, pos, neg)| if sigma < 0 { *neg } else { *pos }).unwrap_or(0)
    };
    let mark_of = |axon: &Id| -> Mark { cur.marks.get(axon).map(|(_, _, m)| *m).unwrap_or_default() };

    // a(c) per player (rewards.md §7 "the payout"): the mean of its sampled
    // marginals when k_c ≥ K_MIN, else the exact path gradient
    // a_exact(c) = Δs · σ · ḡ brought to the game's unit, nats at 2^60
    let sums = Sums::of(game, &lottery.tickets);
    let exact_of = |pl: &super::super::ticket::Player| -> i128 {
        let g = grad_of(&pl.p, &pl.q);
        let signed_g = if pl.side < 0 { -g } else { g };
        tru::focusing::epoch::fixed::mul_wide_floor(pl.delta, signed_g, engine.grad_bits() - super::focus::PHI_BITS)
    };
    let mut estimates: Vec<PlayerEstimate> = players
        .iter()
        .enumerate()
        .map(|(i, pl)| {
            let exact = exact_of(pl);
            let k = sums.k[i];
            let a = if k as usize >= K_MIN { sums.mean(i) } else { exact };
            PlayerEstimate { card: pl.card, side: pl.side, k, mean: sums.mean(i), stderr: sums.stderr(i), exact, a, paid: 0 }
        })
        .collect();
    let by_mean = estimates.iter().filter(|e| e.k as usize >= K_MIN).count() as u32;

    // the clip and the rescale to v(N): a⁺ = max(0, a); â = ⌊a⁺ · v(N) / Σ a⁺⌋
    // when Σ a⁺ > 0 and v(N) > 0, else nothing is paid and the pool carries
    let value = game.value_of_all();
    let mut positive = W::ZERO;
    for e in &estimates {
        positive = positive.checked_add(&W::from_u128(e.a.max(0) as u128)).expect("Σ a⁺ fits 512 bits");
    }
    if value > 0 && !positive.is_zero() {
        let v = W::from_u128(value as u128);
        for e in estimates.iter_mut() {
            if e.a > 0 {
                e.paid = mul_div(&W::from_u128(e.a as u128), &v, &positive).to_u128().expect("â ≤ v(N) fits") as i128;
            }
        }
    }

    let current: BTreeMap<Id, &Position> = cur.positions.iter().map(|p| (p.card, p)).collect();
    let mut earners = Vec::new();
    for (pl, e) in players.iter().zip(&estimates) {
        // gates (§5.4): the position is in G_E on this side, active, increased
        let Some(now) = current.get(&pl.card) else { continue };
        if side_of(now.valence) != pl.side || now.valence == 0 || pl.delta <= 0 {
            continue;
        }
        // only a positive share earns, and only when the epoch created value
        if e.paid <= 0 {
            continue;
        }
        let new_height = side_stake(&now.axon, pl.side).saturating_sub(mark_of(&now.axon).side(pl.side));
        if new_height == 0 {
            continue;
        }
        let r = raw_increase[&(now.axon, pl.side)];
        let x = mul_div(&W::from_u128(e.paid as u128), &W::from_u64(new_height.saturating_mul(RHO_LAUNCH)), &W::from_u128(r));
        if x.is_zero() {
            continue;
        }
        let cap = ((pl.delta as u128 * R_MAX_PPM as u128) / 1_000_000) as u64;
        earners.push(Earner { card: now.card, owner: now.owner, x, cap });
    }
    earners.sort_by(|a, b| a.card.cmp(&b.card));

    // H_E for every axon that ever held a card (§5.3)
    let mut marks = Vec::new();
    let mut axons: std::collections::BTreeSet<Id> = cur.marks.keys().copied().collect();
    axons.extend(sides.keys().copied());
    for axon in axons {
        let (s_pos, s_neg) = (side_stake(&axon, 1), side_stake(&axon, -1));
        let h = mark_of(&axon);
        let next = Mark { pos: decay_mark(h.pos, s_pos), neg: decay_mark(h.neg, s_neg) };
        if s_pos == 0 && s_neg == 0 && next.pos == 0 && next.neg == 0 {
            continue;
        }
        marks.push((axon, next));
    }

    let digest = phi_digest(&focus.phi);
    SettleResult {
        epoch: cur.epoch,
        prev_block: prev.block_id,
        boundary: cur.height,
        beacon: cur.block_id,
        boundary_root: cur.state_root,
        particles: g_cur.n() as u32,
        axons: g_cur.edges() as u32,
        phi: digest,
        syntropy: focus.syntropy,
        prev_syntropy,
        focus_bps: focus_bps(cur.active, cur.supply),
        earners,
        marks,
        focus: Arc::new(FocusOut { particles: g_cur.particles, phi: focus.phi, syntropy: focus.syntropy, digest }),
        value,
        players: players.len() as u32,
        samples: sums.samples(),
        by_mean,
        lottery: estimates,
    }
}

/// Gauss–Legendre nodes `t` and weights `w` on `[0, 1]` at 2^32 (weights
/// sum to 2^32) — the same table as tru's `integrated::gauss_legendre`.
pub fn gauss_legendre(k: usize) -> Vec<(u64, u64)> {
    tru::focusing::epoch::integrated::gauss_legendre(k)
}

/// The path gradient per axon present at some node, ascending by `p ‖ q`:
/// `g(A_mid)` for one node, `Σ_t w_t · g(A_prev + t·ΔA) / 2^32` for k.
fn gradient_path(a_prev: &[(Id, Id, u128)], a_cur: &[(Id, Id, u128)], a_mid: &[(Id, Id, u128)], k: usize, engine: &dyn FocusEngine) -> Vec<(Id, Id, i128)> {
    if k <= 1 {
        let g = engine.gradient(&FocusGraph::build(a_mid));
        return a_mid.iter().zip(g).map(|((p, q, _), g)| (*p, *q, g)).collect();
    }
    // the union of both snapshots with (A_prev, A_cur)
    let mut union: BTreeMap<(Id, Id), (u128, u128)> = BTreeMap::new();
    for (p, q, w) in a_prev {
        union.entry((*p, *q)).or_default().0 = *w;
    }
    for (p, q, w) in a_cur {
        union.entry((*p, *q)).or_default().1 = *w;
    }
    let mut total: BTreeMap<(Id, Id), i128> = BTreeMap::new();
    for (t, w) in gauss_legendre(k) {
        let at: Vec<(Id, Id, u128)> = union
            .iter()
            .map(|((p, q), (a, b))| {
                let d = *b as i128 - *a as i128;
                (*p, *q, (*a as i128 + ((t as i128 * d) >> 32)).max(0) as u128)
            })
            .filter(|x| x.2 > 0)
            .collect();
        let g = engine.gradient(&FocusGraph::build(&at));
        for ((p, q, _), gk) in at.iter().zip(g) {
            // ⌊w·g / 2^32⌋, 256-bit wide (tru/specs/epoch.md §4)
            let wg = tru::focusing::epoch::fixed::mul_wide_floor(w as i128, gk, 32);
            let t = total.entry((*p, *q)).or_insert(0);
            *t = t.saturating_add(wg);
        }
    }
    total.into_iter().map(|((p, q), g)| (p, q, g)).collect()
}

/// The epoch record of §5.6.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub epoch: u64,
    pub boundary: u64,
    pub beacon: Id,
    pub boundary_root: Id,
    pub particles: u32,
    pub axons: u32,
    pub phi: Id,
    pub syntropy: i64,
    pub delta_syntropy: i64,
    pub focus_bps: u16,
    pub budget: u64,
    pub paid: u64,
    pub pool_after: u64,
    pub rewarded: u32,
    pub payouts: Id,
    /// `N`, the players of the epoch's game (rewards.md §7)
    pub players: u32,
    /// Σ k_c, the sampled marginals of the window
    pub samples: u32,
    /// players paid by their mean
    pub by_mean: u32,
    /// `v(N)` at 2^60, saturated to i64
    pub value: i64,
}

/// Record bytes (§5.6 amended: the lottery's four fields after `payouts`).
pub const RECORD_BYTES: usize = 218;

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(RECORD_BYTES);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.boundary.to_le_bytes());
        out.extend_from_slice(&self.beacon);
        out.extend_from_slice(&self.boundary_root);
        out.extend_from_slice(&self.particles.to_le_bytes());
        out.extend_from_slice(&self.axons.to_le_bytes());
        out.extend_from_slice(&self.phi);
        out.extend_from_slice(&self.syntropy.to_le_bytes());
        out.extend_from_slice(&self.delta_syntropy.to_le_bytes());
        out.extend_from_slice(&self.focus_bps.to_le_bytes());
        out.extend_from_slice(&self.budget.to_le_bytes());
        out.extend_from_slice(&self.paid.to_le_bytes());
        out.extend_from_slice(&self.pool_after.to_le_bytes());
        out.extend_from_slice(&self.rewarded.to_le_bytes());
        out.extend_from_slice(&self.payouts);
        out.extend_from_slice(&self.players.to_le_bytes());
        out.extend_from_slice(&self.samples.to_le_bytes());
        out.extend_from_slice(&self.by_mean.to_le_bytes());
        out.extend_from_slice(&self.value.to_le_bytes());
        debug_assert_eq!(out.len(), RECORD_BYTES);
        out
    }

    /// `time[settle(E)] = hemera(D_EPOCH ‖ record)`.
    pub fn digest(&self) -> Id {
        let mut buf = DOMAIN_EPOCH.to_vec();
        buf.extend_from_slice(&self.encode());
        *hemera::hash(&buf).as_bytes()
    }
}

/// i128 syntropy into the record's i64 (saturating: J ≤ ln n nats, which
/// at 2^60 exceeds i64 only past e^8 ≈ 3,000 particles of full concentration).
fn sat64(v: i128) -> i64 {
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// What the settlement step did, for the node's epoch store and `/epoch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub record: Record,
    /// `(card, owner, amount)`, cards ascending, amount > 0
    pub payouts: Vec<(Id, Id, u64)>,
    pub focus: Arc<FocusOut>,
}

/// Floor of `B · x / X` capped per position (§5.5).
pub fn divide(budget: u64, earners: &[Earner]) -> Vec<u64> {
    let mut total = W::ZERO;
    for e in earners {
        total = total.checked_add(&e.x).expect("Σ x fits 512 bits");
    }
    if total.is_zero() {
        return vec![0; earners.len()];
    }
    let b = W::from_u64(budget);
    earners
        .iter()
        .map(|e| {
            let share = mul_div(&b, &e.x, &total).to_u64().expect("share ≤ budget");
            share.min(e.cap)
        })
        .collect()
}

/// The settlement step of the block at `settle(E)` (§5.5–§5.7): after the
/// coinbase, before the transactions. Pays from the pool, rewrites the
/// high-water marks, writes `time[height]`.
pub fn apply<S: StateBackend>(state: &mut S, native: &Id, height: u64, r: &SettleResult) -> Result<Outcome, StateError> {
    let pool = focus_key(native);
    let f = state.raw_balance(&pool);
    let budget = f / POOL_RELEASE;
    let pays = divide(budget, &r.earners);
    let mut payouts = Vec::new();
    let mut paid = 0u64;
    for (e, pay) in r.earners.iter().zip(pays) {
        if pay == 0 {
            continue;
        }
        state.credit(&e.owner, native, pay)?;
        paid += pay;
        payouts.push((e.card, e.owner, pay));
    }
    if paid > budget || budget > f {
        return Err(StateError::Insufficient { have: budget, need: paid });
    }
    state.debit_raw(&pool, native, paid)?;

    let keep: std::collections::BTreeSet<Id> = r.marks.iter().map(|(a, _)| *a).collect();
    for (axon, _) in state.marks() {
        if !keep.contains(&axon) {
            state.set_mark(&axon, None);
        }
    }
    for (axon, m) in &r.marks {
        state.set_mark(axon, Some(*m));
    }

    let leaves: Vec<Id> = payouts.iter().map(|(c, o, a)| payout_leaf(c, o, *a)).collect();
    let record = Record {
        epoch: r.epoch,
        boundary: r.boundary,
        beacon: r.beacon,
        boundary_root: r.boundary_root,
        particles: r.particles,
        axons: r.axons,
        phi: r.phi,
        syntropy: sat64(r.syntropy),
        delta_syntropy: sat64(r.syntropy - r.prev_syntropy),
        focus_bps: r.focus_bps as u16,
        budget,
        paid,
        pool_after: f - paid,
        rewarded: payouts.len() as u32,
        payouts: payouts_root(&leaves),
        players: r.players,
        samples: r.samples,
        by_mean: r.by_mean,
        value: sat64(r.value),
    };
    state.set_time(height, record.digest());
    let mut s = state.scalars();
    s.last_settled = r.epoch;
    state.set_scalars(s);
    Ok(Outcome { record, payouts, focus: r.focus.clone() })
}

/// Σ stake keys over the cards of a state, the §6.3 check against the
/// running total.
pub fn stake_total(state: &impl StateBackend) -> u128 {
    state.cards().iter().map(|(c, _)| state.raw_balance(&stake_key(c)) as u128).sum()
}
