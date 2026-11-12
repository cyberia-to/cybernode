//! The settlement ticket — the chain's proof of work (tru `specs/rewards.md`
//! §7 steps 1–3, chain-economy §7.1, chain.md §1.2).
//!
//! The block at height `h` in epoch `E + 1` samples the game of epoch `E`:
//! the players are the positions that changed between the boundary
//! snapshots `G_{E−1}` and `G_E`, one cluster, and the base graph is
//! `A_{E−1}`. The header's first 168 bytes are the template `n`; its digest
//! draws the nonce the ordering takes and the window's first rank, the
//! probes come from the same digest, and the ticket is `W_TICKET` entries —
//! the marginals of `min(N, W_TICKET − 1)` cyclic ranks of `π(n)` and
//! gradient probes for the rest — committed as
//! `sample = hemera(D_SAMPLE ‖ W ‖ m…)` at the header's end. The block id
//! over all 200 bytes below the target is the ticket, the work and the
//! subsidy.
//!
//! The game, the ordering, the probes and the commitment are tru#46's
//! (`tru::focusing::epoch::coalition`): [`EpochGame::ticket`] computes the
//! same entries as `Game::ticket`, bit for bit, with the independent solves
//! spread over the node's cores. Integers only; nothing here reads a clock.

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use tru::focusing::epoch::coalition::{Probe, Ticket, ordering, probes};
use tru::focusing::epoch::{Axon as TruAxon, Change, EpochParams, Game};

use super::economy::settle::Snapshot;
use super::economy::side_of;
use super::economy::weights::{Axon, GenesisWeights};
use super::header::{Header, hash32};
use super::params::{PHI_STEPS, TEMPLATE_BYTES};
use super::state::Id;

/// `D_CLUSTER`: the cluster's identity is the hash of its players
/// (rewards.md §7 "the region"; chain-economy §3.1 amended).
pub const D_CLUSTER: &[u8] = b"soft3/chain/cluster/v1\0";

/// One player of the epoch's game (rewards.md §7 "the game"): a card that
/// changed its stake on one side between two boundaries, `(card, σ, Δs)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    pub card: Id,
    /// `σ ∈ {+1, −1}`
    pub side: i8,
    /// `Δs(c)` on that side, signed: positive for an increase
    pub delta: i128,
    pub p: Id,
    pub q: Id,
    /// the payout owner at the later boundary (the earlier one when the
    /// card left)
    pub owner: Id,
}

impl Player {
    /// The side byte of `card ‖ σ`: `σ` as a little-endian `i8`.
    pub fn side_byte(&self) -> u8 {
        self.side as u8
    }

    /// tru's change on the axon: `Δs · σ`.
    pub fn change(&self) -> Change {
        Change { from: self.p, to: self.q, delta: if self.side < 0 { -self.delta } else { self.delta } }
    }
}

/// The players between two boundaries, ascending by `card ‖ σ` (the bytes;
/// `+1` sorts before `−1` on a card that changed sides): for each card and
/// side with `Δs ≠ 0` one player; a card that changed sides is two.
pub fn players(prev: &Snapshot, cur: &Snapshot) -> Vec<Player> {
    let before: std::collections::BTreeMap<Id, &super::economy::settle::Position> = prev.positions.iter().map(|p| (p.card, p)).collect();
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for pos in &cur.positions {
        seen.insert(pos.card);
        let sigma = side_of(pos.valence);
        let player = |side: i8, delta: i128, owner: Id| Player { card: pos.card, side, delta, p: pos.p, q: pos.q, owner };
        match before.get(&pos.card) {
            Some(old) if side_of(old.valence) == sigma => {
                let d = pos.stake as i128 - old.stake as i128;
                if d != 0 {
                    out.push(player(sigma, d, pos.owner));
                }
            }
            Some(old) => {
                if old.stake > 0 {
                    out.push(player(side_of(old.valence), -(old.stake as i128), pos.owner));
                }
                if pos.stake > 0 {
                    out.push(player(sigma, pos.stake as i128, pos.owner));
                }
            }
            None => {
                if pos.stake > 0 {
                    out.push(player(sigma, pos.stake as i128, pos.owner));
                }
            }
        }
    }
    for old in prev.positions.iter().filter(|p| !seen.contains(&p.card) && p.stake > 0) {
        out.push(Player { card: old.card, side: side_of(old.valence), delta: -(old.stake as i128), p: old.p, q: old.q, owner: old.owner });
    }
    out.sort_by_key(|p| (p.card, p.side_byte()));
    out
}

/// `cluster(E) = hemera(D_CLUSTER ‖ N: u32 ‖ (card ‖ σ)…)` (rewards.md §7).
pub fn cluster_id(players: &[Player]) -> Id {
    let mut buf = Vec::with_capacity(D_CLUSTER.len() + 4 + 33 * players.len());
    buf.extend_from_slice(D_CLUSTER);
    buf.extend_from_slice(&(players.len() as u32).to_le_bytes());
    for p in players {
        buf.extend_from_slice(&p.card);
        buf.push(p.side_byte());
    }
    hash32(&buf)
}

/// The draw of a template (rewards.md §7 step 1): `h = hemera(n)`,
/// `nonce = u64_le(h[0..8])`, the window's first rank from `u64_le(h[8..16])`
/// (taken modulo `N` by the ticket).
pub fn draw(template: &[u8; TEMPLATE_BYTES]) -> (u64, u64) {
    let h = hash32(template);
    (u64::from_le_bytes(h[0..8].try_into().expect("8")), u64::from_le_bytes(h[8..16].try_into().expect("8")))
}

/// `sample = hemera(Ticket::preimage())` under `D_SAMPLE`.
pub fn sample_of(ticket: &Ticket) -> Id {
    hash32(&ticket.preimage())
}

/// The game one epoch's blocks sample, built once per epoch from the
/// settled snapshots and cached (rewards.md §7 "the game"; §16).
pub struct EpochGame {
    /// `E`, the epoch whose game this is (its blocks are those of `E + 1`)
    pub epoch: u64,
    /// `b_E`: the block id of `boundary(E)`
    pub beacon: Id,
    pub cluster: Id,
    pub players: Vec<Player>,
    /// `W_TICKET` of the chain
    pub width: usize,
    /// the base axons `A_{E−1}`, ascending (the probes' index space)
    pub base: Vec<Axon<u128>>,
    game: Game,
}

impl std::fmt::Debug for EpochGame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EpochGame {{ epoch: {}, players: {}, axons: {}, width: {} }}", self.epoch, self.players.len(), self.base.len(), self.width)
    }
}

impl PartialEq for EpochGame {
    fn eq(&self, o: &Self) -> bool {
        self.epoch == o.epoch && self.beacon == o.beacon && self.cluster == o.cluster && self.width == o.width
    }
}

impl Eq for EpochGame {}

/// Why a header's sample does not verify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// the recomputed commitment differs from `header.sample`
    Mismatch { expected: Id, actual: Id },
}

impl std::fmt::Display for SampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SampleError::Mismatch { .. } => write!(f, "the sample does not commit to the ticket of this template"),
        }
    }
}

/// The solve threads of the game: the parallelism of a ticket is across
/// its independent solves, so a solve gets its own threads only when the
/// ticket has fewer solves than the machine has cores (bostrom: three
/// solves of 6 s on twelve threads; pussy: 65 solves of 36 ms on one each),
/// and never more than one per [`AXONS_PER_THREAD`] axons of the base:
/// threads inside a solve lose below that size (tru#46 §3.1).
pub fn solve_threads(width: usize, workers: usize, axons: usize) -> usize {
    (workers / (width + 1)).min(axons / AXONS_PER_THREAD).max(1)
}

/// Axons of the base per solve thread: pussy's 26,805 axons solve fastest
/// on one thread (tru#46 §3.1), bostrom's 2.99 M on twelve and more.
pub const AXONS_PER_THREAD: usize = 1 << 15;

impl EpochGame {
    /// The game over the base `A_{E−1}` and the players, under the
    /// protocol parameters (`PHI_STEPS`, the ground weight). One solve.
    /// `workers` sizes the threads a solve may use ([`solve_threads`]).
    pub fn build(epoch: u64, beacon: Id, base: Vec<Axon<u128>>, players: Vec<Player>, width: usize, workers: usize) -> Result<EpochGame, String> {
        assert!(width >= 1, "W_TICKET is at least 1");
        let params = EpochParams { steps: PHI_STEPS, threads: solve_threads(width, workers, base.len()), ..EpochParams::default() };
        let prev: Vec<TruAxon> = base.iter().map(|(p, q, w)| TruAxon { from: *p, to: *q, weight: *w }).collect();
        let changes: Vec<Change> = players.iter().map(Player::change).collect();
        let game = Game::new(&prev, &changes, &params).map_err(|e| e.to_string())?;
        // the probe modulus is `self.base.len()`; tru's `Game` canonicalises
        // its base (drops self-loops), so the two must agree or a probe would
        // index an axon the game does not have (R18-12). The launch genesis
        // graphs carry no self-loop, so this holds; the assert catches a
        // future graph that does.
        assert_eq!(base.len(), game.base().len(), "the base must be canonical: no self-loop axons");
        Ok(EpochGame { epoch, beacon, cluster: cluster_id(&players), players, width, base, game })
    }

    /// The game of epoch `E ≥ 1` from its two boundary snapshots.
    pub fn from_snapshots(prev: &Snapshot, cur: &Snapshot, weights: &GenesisWeights, width: usize, workers: usize) -> Result<EpochGame, String> {
        EpochGame::build(cur.epoch, cur.block_id, prev.adjacency(weights), players(prev, cur), width, workers)
    }

    /// The empty game of epoch 0 (`G_{−1} := G_0`, `N = 0`): the blocks of
    /// epoch 1 carry probes of the genesis graph.
    pub fn genesis(block0: Id, weights: &GenesisWeights, width: usize, workers: usize) -> Result<EpochGame, String> {
        let base: Vec<Axon<u128>> = weights.axons.iter().map(|(p, q, g)| (*p, *q, *g as u128)).collect();
        EpochGame::build(0, block0, base, Vec::new(), width, workers)
    }

    /// tru's game.
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// `v(N) = J_U(φ*(A_E)) − J_U(φ*(A_{E−1}))`: the value of the grand
    /// coalition, one solve.
    pub fn value_of_all(&self) -> i128 {
        let all: Vec<usize> = (0..self.players.len()).collect();
        self.game.value(&all)
    }

    /// The ticket of a template (rewards.md §7 steps 1–2): the entries of
    /// `Game::ticket` for the template's draw, with the `W' + 1` prefix
    /// solves and the probes run across `workers` threads. Bit-identical to
    /// tru's sequential `Game::ticket` (`ticket_equals_tru_s_sequential_one`).
    pub fn ticket(&self, template: &[u8; TEMPLATE_BYTES], workers: usize) -> Ticket {
        let (nonce, rank) = draw(template);
        self.ticket_at(nonce, rank, workers)
    }

    /// [`EpochGame::ticket`] for an explicit draw.
    pub fn ticket_at(&self, nonce: u64, rank: u64, workers: usize) -> Ticket {
        let n = self.players.len();
        let w = self.width;
        let order = ordering(&self.beacon, &self.cluster, nonce, n);
        let rank = if n == 0 { 0 } else { (rank % n as u64) as usize };
        let w_m = n.min(w - 1);
        // the window's ranks, cyclic from `rank`; the prefix of rank j is order[..=j]
        let ranks: Vec<usize> = (0..w_m).map(|k| (rank + k) % n).collect();
        let mut coalitions: Vec<Vec<usize>> = ranks.iter().map(|&j| order[..=j].to_vec()).collect();
        // the value before the first rank: one more solve unless it is v(∅)
        let prefix = (w_m > 0 && rank > 0).then(|| {
            coalitions.push(order[..rank].to_vec());
            coalitions.len() - 1
        });
        let draws = probes(&self.beacon, &self.cluster, nonce, w - w_m, self.base.len());
        let jobs = coalitions.len() + draws.len();
        let values: Vec<std::sync::Mutex<i128>> = (0..coalitions.len()).map(|_| std::sync::Mutex::new(0)).collect();
        let probed: Vec<std::sync::Mutex<Option<Probe>>> = (0..draws.len()).map(|_| std::sync::Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        let threads = workers.clamp(1, jobs.max(1));
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Relaxed);
                    if i >= jobs {
                        break;
                    }
                    if i < coalitions.len() {
                        *values[i].lock().expect("value slot") = self.game.value(&coalitions[i]);
                    } else {
                        let (a, t) = draws[i - coalitions.len()];
                        *probed[i - coalitions.len()].lock().expect("probe slot") = Some(self.game.probe(a, t));
                    }
                });
            }
        });
        let values: Vec<i128> = values.into_iter().map(|m| m.into_inner().expect("value slot")).collect();
        let probes: Vec<Probe> = probed.into_iter().map(|m| m.into_inner().expect("probe slot").expect("every probe solved")).collect();
        self.assemble(nonce, rank, order, &ranks, &values, prefix, probes)
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(&self, nonce: u64, rank: usize, order: Vec<usize>, ranks: &[usize], values: &[i128], prefix: Option<usize>, probes: Vec<Probe>) -> Ticket {
        let mut by_player: Vec<(usize, i128)> = Vec::with_capacity(ranks.len());
        for (k, &j) in ranks.iter().enumerate() {
            let prev = if j == 0 {
                0
            } else if k == 0 {
                values[prefix.expect("rank > 0 has a prefix solve")]
            } else {
                values[k - 1]
            };
            by_player.push((order[j], values[k] - prev));
        }
        by_player.sort_unstable_by_key(|x| x.0);
        Ticket {
            nonce,
            rank,
            order,
            covered: by_player.iter().map(|x| x.0).collect(),
            marginals: by_player.iter().map(|x| x.1).collect(),
            probes,
        }
    }

    /// The miner's attempt: the ticket of the header's template, its
    /// commitment written into `header.sample`.
    pub fn solve(&self, header: &mut Header, workers: usize) -> Ticket {
        let ticket = self.ticket(&header.template(), workers);
        header.sample = sample_of(&ticket);
        ticket
    }

    /// Recompute the ticket of `header` and compare with its commitment
    /// (chain.md §5.5 step 8).
    pub fn verify(&self, header: &Header, workers: usize) -> Result<Ticket, SampleError> {
        let ticket = self.ticket(&header.template(), workers);
        let expected = sample_of(&ticket);
        if expected != header.sample {
            return Err(SampleError::Mismatch { expected, actual: header.sample });
        }
        Ok(ticket)
    }
}

// ── the game on the wire: `GET /game/{beacon}` ──────────────────────────

/// The game as a miner receives it (chain-wire §7 amended): the base axons
/// and the players, enough to build the same [`EpochGame`] — the same
/// cluster, the same solves — on a machine without the node's state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameWire {
    pub epoch: u64,
    pub beacon: String,
    pub cluster: String,
    pub w_ticket: usize,
    /// `[from, to, weight]`, weight as a decimal string
    pub base: Vec<(String, String, String)>,
    /// `[card, side, delta, p, q, owner]`, delta as a decimal string
    pub players: Vec<(String, i8, String, String, String, String)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex32(s: &str) -> Result<Id, String> {
    if s.len() != 64 {
        return Err(format!("{s}: not 64 hex digits"));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

impl GameWire {
    pub fn of(g: &EpochGame) -> GameWire {
        GameWire {
            epoch: g.epoch,
            beacon: hex(&g.beacon),
            cluster: hex(&g.cluster),
            w_ticket: g.width,
            base: g.base.iter().map(|(p, q, w)| (hex(p), hex(q), w.to_string())).collect(),
            players: g.players.iter().map(|p| (hex(&p.card), p.side, p.delta.to_string(), hex(&p.p), hex(&p.q), hex(&p.owner))).collect(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a game serializes")
    }

    /// The game rebuilt (one solve); its cluster must equal the one served.
    pub fn build(&self, workers: usize) -> Result<EpochGame, String> {
        let base = self.base.iter().map(|(p, q, w)| Ok((unhex32(p)?, unhex32(q)?, w.parse::<u128>().map_err(|e| e.to_string())?))).collect::<Result<Vec<_>, String>>()?;
        let players = self
            .players
            .iter()
            .map(|(card, side, delta, p, q, owner)| {
                Ok(Player { card: unhex32(card)?, side: *side, delta: delta.parse().map_err(|e: std::num::ParseIntError| e.to_string())?, p: unhex32(p)?, q: unhex32(q)?, owner: unhex32(owner)? })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let g = EpochGame::build(self.epoch, unhex32(&self.beacon)?, base, players, self.w_ticket, workers)?;
        if hex(&g.cluster) != self.cluster {
            return Err("the game's cluster differs from the served one".into());
        }
        Ok(g)
    }
}

// ── the ticket as a cache record ───────────────────────────────────────

const MAGIC: &[u8; 8] = b"S3TCKT02";

/// A verified ticket's bytes: everything the commitment and the settlement
/// read (the ordering is implied by the nonce and not stored), with a hemera
/// checksum trailer. The header's `sample` commits only to the marginal and
/// probe values (`Ticket::preimage`), so a cached record's `covered`, `rank`
/// and probe `axon/t/u` are bound by this checksum instead — a flipped bit
/// in any of them is rejected on load rather than paying the wrong player at
/// settlement (R18-7).
pub fn encode_ticket(t: &Ticket) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&t.nonce.to_le_bytes());
    out.extend_from_slice(&(t.rank as u64).to_le_bytes());
    out.extend_from_slice(&(t.covered.len() as u32).to_le_bytes());
    for (c, m) in t.covered.iter().zip(&t.marginals) {
        out.extend_from_slice(&(*c as u32).to_le_bytes());
        out.extend_from_slice(&m.to_le_bytes());
    }
    out.extend_from_slice(&(t.probes.len() as u32).to_le_bytes());
    for p in &t.probes {
        out.extend_from_slice(&(p.axon as u32).to_le_bytes());
        out.extend_from_slice(&p.t.to_le_bytes());
        out.extend_from_slice(&p.u.to_le_bytes());
        out.extend_from_slice(&p.value.to_le_bytes());
    }
    let sum = hash32(&out);
    out.extend_from_slice(&sum);
    out
}

pub fn decode_ticket(b: &[u8]) -> Option<Ticket> {
    // the checksum binds every field (R18-7): split it off and compare
    let body = b.get(..b.len().checked_sub(32)?)?;
    if hash32(body)[..] != b[body.len()..] {
        return None;
    }
    let b = body;
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at + n)?;
        at += n;
        Some(s)
    };
    if take(8)? != MAGIC {
        return None;
    }
    let u64_ = |s: &[u8]| u64::from_le_bytes(s.try_into().expect("8"));
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().expect("4"));
    let i128_ = |s: &[u8]| i128::from_le_bytes(s.try_into().expect("16"));
    let nonce = u64_(take(8)?);
    let rank = u64_(take(8)?) as usize;
    let n = u32_(take(4)?) as usize;
    if n > 1 << 20 {
        return None;
    }
    let mut covered = Vec::with_capacity(n);
    let mut marginals = Vec::with_capacity(n);
    for _ in 0..n {
        covered.push(u32_(take(4)?) as usize);
        marginals.push(i128_(take(16)?));
    }
    let k = u32_(take(4)?) as usize;
    if k > 1 << 20 {
        return None;
    }
    let mut probes = Vec::with_capacity(k);
    for _ in 0..k {
        let axon = u32_(take(4)?) as usize;
        let t = u32_(take(4)?);
        let u = u128::from_le_bytes(take(16)?.try_into().ok()?);
        let value = i128_(take(16)?);
        probes.push(Probe { axon, t, u, value });
    }
    if at != b.len() {
        return None;
    }
    Some(Ticket { nonce, rank, order: Vec::new(), covered, marginals, probes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(i: u8) -> Id {
        [i; 32]
    }

    /// A small base and players of every kind: raised, lowered, a new axon.
    fn game(width: usize) -> EpochGame {
        let base: Vec<Axon<u128>> = vec![(pid(1), pid(2), 1_000_000), (pid(2), pid(3), 2_000_000), (pid(3), pid(1), 500_000), (pid(1), pid(3), 700_000)];
        let players = vec![
            Player { card: [10; 32], side: 1, delta: 300_000, p: pid(1), q: pid(2), owner: [7; 32] },
            Player { card: [11; 32], side: -1, delta: 200_000, p: pid(2), q: pid(3), owner: [7; 32] },
            Player { card: [12; 32], side: 1, delta: 400_000, p: pid(2), q: pid(4), owner: [8; 32] },
            Player { card: [13; 32], side: 1, delta: 100_000, p: pid(3), q: pid(1), owner: [8; 32] },
            Player { card: [14; 32], side: -1, delta: 50_000, p: pid(1), q: pid(3), owner: [9; 32] },
        ];
        EpochGame::build(3, [0xbe; 32], base, players, width, 4).unwrap()
    }

    #[test]
    fn ticket_equals_tru_s_sequential_one() {
        for width in [1usize, 2, 3, 5, 6, 8] {
            let g = game(width);
            for nonce in 0..6u64 {
                for rank in [0u64, 1, 3, 4, 7, u64::MAX] {
                    let ours = g.ticket_at(nonce, rank, 4);
                    let tru = g.game().ticket(&g.beacon, &g.cluster, nonce, (rank % 5) as usize, width);
                    assert_eq!(ours, tru, "width {width} nonce {nonce} rank {rank}");
                    assert_eq!(ours.width(), width);
                    assert_eq!(ours.preimage(), tru.preimage());
                }
            }
        }
    }

    #[test]
    fn ticket_is_the_same_on_one_and_many_workers() {
        let g = game(6);
        let template = [0x5a; TEMPLATE_BYTES];
        let a = g.ticket(&template, 1);
        let b = g.ticket(&template, 8);
        assert_eq!(a, b);
        let (nonce, _) = draw(&template);
        assert_eq!(a.nonce, nonce);
    }

    #[test]
    fn full_window_sums_to_the_coalition_value() {
        // W − 1 ≥ N: the marginals are the whole ordering and sum to v(N) exactly
        let g = game(8);
        let t = g.ticket_at(9, 2, 4);
        assert_eq!(t.covered.len(), 5);
        assert_eq!(t.probes.len(), 3);
        assert_eq!(t.marginals.iter().sum::<i128>(), g.value_of_all());
    }

    #[test]
    fn empty_game_is_all_probes_and_every_template_differs() {
        let base: Vec<Axon<u128>> = vec![(pid(1), pid(2), 1_000_000), (pid(2), pid(3), 2_000_000)];
        let g = EpochGame::build(0, [0; 32], base, Vec::new(), 3, 2).unwrap();
        let mut t1 = [0u8; TEMPLATE_BYTES];
        let t2 = {
            let mut t = t1;
            t[24] = 1;
            t
        };
        let a = g.ticket(&t1, 2);
        let b = g.ticket(&t2, 2);
        assert!(a.covered.is_empty() && a.probes.len() == 3);
        assert_ne!(a.probes.iter().map(|p| (p.axon, p.t)).collect::<Vec<_>>(), b.probes.iter().map(|p| (p.axon, p.t)).collect::<Vec<_>>());
        assert_ne!(sample_of(&a), sample_of(&b));
        t1[0] = 1;
        assert_ne!(draw(&t1), draw(&t2));
    }

    #[test]
    fn solve_then_verify_and_a_corrupted_sample_fails() {
        let g = game(4);
        let mut h = super::super::header::tests::test_header();
        let t = g.solve(&mut h, 3);
        assert_eq!(h.sample, sample_of(&t));
        assert_eq!(g.verify(&h, 3), Ok(t.clone()));
        let mut bad = h;
        bad.sample[0] ^= 1;
        assert!(matches!(g.verify(&bad, 3), Err(SampleError::Mismatch { .. })));
        let mut other = h;
        other.nonce[0] ^= 1;
        assert!(g.verify(&other, 3).is_err(), "a changed template is a new draw");
        assert_eq!(decode_ticket(&encode_ticket(&t)).map(|d| (d.nonce, d.rank, d.covered, d.marginals, d.probes)), Some((t.nonce, t.rank, t.covered.clone(), t.marginals.clone(), t.probes.clone())));
        assert_eq!(hash32(&decode_ticket(&encode_ticket(&t)).unwrap().preimage()), h.sample);
    }

    #[test]
    fn game_round_trips_over_the_wire() {
        let g = game(3);
        let w = GameWire::of(&g);
        let back: GameWire = serde_json::from_str(&w.to_json()).unwrap();
        assert_eq!(back, w);
        let g2 = back.build(2).unwrap();
        assert_eq!(g2, g);
        assert_eq!(g2.ticket_at(5, 1, 2), g.ticket_at(5, 1, 2));
        let mut wrong = w.clone();
        wrong.cluster = "00".repeat(32);
        assert!(wrong.build(1).is_err());
    }

    #[test]
    fn cluster_binds_the_player_set() {
        let g = game(2);
        let mut fewer = g.players.clone();
        fewer.pop();
        assert_ne!(cluster_id(&g.players), cluster_id(&fewer));
        assert_eq!(cluster_id(&[]), hash32(&[D_CLUSTER, &0u32.to_le_bytes()].concat()));
        assert_eq!(solve_threads(64, 96, 26_805), 1, "pussy: one thread per solve, the parallelism across the 65 solves");
        assert_eq!(solve_threads(2, 96, 2_997_569), 32, "bostrom: three solves, 32 threads each");
        assert_eq!(solve_threads(2, 96, 100), 1, "a small graph never spreads a solve");
        assert_eq!(solve_threads(2, 1, 2_997_569), 1);
    }
}
