//! The lottery's state on a node (rewards.md §7): the game of each epoch,
//! built once from the settled snapshots and cached; the verified ticket of
//! every block, kept in memory and under `$home/tickets/` so a replay does
//! not solve again; and the per-player statistics the settlement pays by.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tru::focusing::epoch::coalition::Ticket;

use super::super::header::Header;
use super::super::state::Id;
use super::super::ticket::{EpochGame, SampleError, decode_ticket, encode_ticket, sample_of};
use super::settle::Snapshot;
use super::weights::GenesisWeights;

/// Every ticket of one epoch's window and the game they sample: the
/// settlement's input beside the two snapshots.
#[derive(Clone)]
pub struct Lottery {
    pub game: Arc<EpochGame>,
    /// the window's tickets, by height ascending
    pub tickets: Vec<Arc<Ticket>>,
}

/// What the lottery says about one player at settlement (chain-economy §11
/// `GET /epoch/{E}/lottery`): the count, the mean, its standard error, the
/// exact path gradient and the share paid, all in nats at 2^60.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerEstimate {
    pub card: Id,
    pub side: i8,
    pub k: u32,
    /// the integer quotient toward zero of the sum by `k`; 0 when `k = 0`
    pub mean: i128,
    /// the realized standard error of the mean (informational, never
    /// consensus): `sqrt(Σ(m − mean)² / (k(k − 1)))` at 2^60, 0 below two samples
    pub stderr: i128,
    pub exact: i128,
    /// `a(c)`: the mean when `k ≥ K_MIN`, else `exact`
    pub a: i128,
    /// `â(c)`: the clipped share after the rescale to `v(N)`; 0 when unpaid
    pub paid: i128,
}

/// Per-player sums over the window's tickets, indexed like the game's players.
pub struct Sums {
    pub k: Vec<u32>,
    pub sum: Vec<i128>,
    /// Σ (m >> 30)² per player, for the standard error only
    sq: Vec<i128>,
}

impl Sums {
    pub fn of(game: &EpochGame, tickets: &[Arc<Ticket>]) -> Sums {
        let n = game.players.len();
        let mut s = Sums { k: vec![0; n], sum: vec![0; n], sq: vec![0; n] };
        for t in tickets {
            for (&c, &m) in t.covered.iter().zip(&t.marginals) {
                if c < n {
                    s.k[c] += 1;
                    // `saturating_add` is unreachable here (a marginal is a J
                    // difference under 2^124 and an epoch holds L·(W−1) ≤ 15,120
                    // of them, the sum far below i128::MAX ≈ 2^127); it guards a
                    // malformed cached ticket rather than an honest overflow (R18-12)
                    s.sum[c] = s.sum[c].saturating_add(m);
                    let r = m >> 30;
                    s.sq[c] = s.sq[c].saturating_add(r.saturating_mul(r));
                }
            }
        }
        s
    }

    /// The mean of player `c`: the integer quotient toward zero (rewards.md §7).
    pub fn mean(&self, c: usize) -> i128 {
        if self.k[c] == 0 { 0 } else { self.sum[c] / self.k[c] as i128 }
    }

    /// The realized standard error of the mean, at 2^60 (informational).
    pub fn stderr(&self, c: usize) -> i128 {
        let k = self.k[c] as i128;
        if k < 2 {
            return 0;
        }
        let mean = self.mean(c) >> 30;
        // Σ(r − mean)² = Σr² − k·mean², at 2^60 after the 2^30 scaling
        let var = (self.sq[c] - k * mean * mean).max(0) / (k * (k - 1));
        isqrt_i128(var) << 30
    }

    pub fn samples(&self) -> u32 {
        self.k.iter().sum()
    }
}

fn isqrt_i128(n: i128) -> i128 {
    if n <= 0 { 0 } else { super::isqrt(n as u128) as i128 }
}

/// The games and tickets a node holds.
pub struct Tickets {
    /// the game of epoch E by the id of `boundary(E)`
    games: HashMap<Id, Arc<EpochGame>>,
    /// the served `GET /game/{beacon}` JSON, built once per game (R18-8)
    json: HashMap<Id, Arc<str>>,
    /// the verified ticket of a block by its id, with the block's height
    tickets: HashMap<Id, (u64, Arc<Ticket>)>,
    dir: Option<PathBuf>,
    /// the solve threads a verification may use
    pub workers: usize,
}

impl Tickets {
    pub fn new(dir: Option<PathBuf>, workers: usize) -> Tickets {
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
        }
        Tickets { games: HashMap::new(), json: HashMap::new(), tickets: HashMap::new(), dir, workers: workers.max(1) }
    }

    /// `GET /game/{beacon}` (R18-8): the game's JSON, built once and cached,
    /// served only for a game of epoch `≥ min_epoch` (the current and the
    /// previous, so an unauthenticated caller cannot make the node rebuild
    /// an arbitrary old game — bostrom's is about half a gigabyte).
    pub fn game_json(&mut self, beacon: &Id, min_epoch: u64) -> Option<Arc<str>> {
        let g = self.games.get(beacon)?.clone();
        if g.epoch < min_epoch {
            return None;
        }
        if let Some(j) = self.json.get(beacon) {
            return Some(j.clone());
        }
        let j: Arc<str> = Arc::from(super::super::ticket::GameWire::of(&g).to_json().as_str());
        self.json.insert(*beacon, j.clone());
        Some(j)
    }

    pub fn game(&self, boundary: &Id) -> Option<Arc<EpochGame>> {
        self.games.get(boundary).cloned()
    }

    /// The game of epoch `cur.epoch` from its two snapshots, built once.
    pub fn game_of(&mut self, prev: &Snapshot, cur: &Snapshot, weights: &GenesisWeights, width: usize) -> Result<Arc<EpochGame>, String> {
        if let Some(g) = self.games.get(&cur.block_id) {
            return Ok(g.clone());
        }
        let g = Arc::new(if cur.epoch == 0 {
            EpochGame::genesis(cur.block_id, weights, width, self.workers)?
        } else {
            EpochGame::from_snapshots(prev, cur, weights, width, self.workers)?
        });
        self.games.insert(cur.block_id, g.clone());
        Ok(g)
    }

    fn path(&self, block: &Id) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}.ticket", hex(block))))
    }

    /// The verified ticket of `block`, if this node holds it.
    pub fn get(&self, block: &Id) -> Option<Arc<Ticket>> {
        self.tickets.get(block).map(|(_, t)| t.clone())
    }

    /// A cached ticket whose commitment is the header's: no solve.
    fn cached(&mut self, header: &Header) -> Option<Arc<Ticket>> {
        let id = header.id();
        if let Some((_, t)) = self.tickets.get(&id) {
            return Some(t.clone());
        }
        let bytes = std::fs::read(self.path(&id)?).ok()?;
        let t = decode_ticket(&bytes)?;
        if sample_of(&t) != header.sample {
            return None;
        }
        let t = Arc::new(t);
        self.tickets.insert(id, (header.height, t.clone()));
        Some(t)
    }

    /// Is the header's ticket held without a solve?
    pub fn is_cached(&mut self, header: &Header) -> bool {
        self.cached(header).is_some()
    }

    /// The header's ticket: cached, or recomputed on `game` and compared
    /// with the header's commitment (chain.md §5.5 step 8). `Err` is a
    /// mismatch: the block is invalid.
    pub fn verify(&mut self, header: &Header, game: &EpochGame) -> Result<Arc<Ticket>, SampleError> {
        if let Some(t) = self.cached(header) {
            return Ok(t);
        }
        let t = Arc::new(game.verify(header, self.workers)?);
        self.put(header.id(), header.height, t.clone());
        Ok(t)
    }

    /// Keep a ticket this node computed itself.
    pub fn put(&mut self, block: Id, height: u64, t: Arc<Ticket>) {
        if let Some(p) = self.path(&block) {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, encode_ticket(&t)).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
        self.tickets.insert(block, (height, t));
    }

    /// Drop the games of boundaries no longer held and the tickets of blocks
    /// below `floor` (the disk keeps them for a replay).
    pub fn prune(&mut self, keep_games: impl Fn(&Id) -> bool, floor: u64) {
        self.games.retain(|id, _| keep_games(id));
        self.json.retain(|id, _| keep_games(id));
        self.tickets.retain(|_, (h, _)| *h >= floor);
    }

    pub fn held(&self) -> (usize, usize) {
        (self.games.len(), self.tickets.len())
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tru::focusing::epoch::coalition::Probe;

    #[test]
    fn sums_mean_and_stderr() {
        let base = vec![([1; 32], [2; 32], 1_000_000u128), ([2; 32], [3; 32], 2_000_000)];
        let players = vec![
            super::super::super::ticket::Player { card: [10; 32], side: 1, delta: 10, p: [1; 32], q: [2; 32], owner: [7; 32] },
            super::super::super::ticket::Player { card: [11; 32], side: 1, delta: 10, p: [2; 32], q: [3; 32], owner: [7; 32] },
        ];
        let g = EpochGame::build(1, [0; 32], base, players, 2, 1).unwrap();
        let t = |c: usize, m: i128| Arc::new(Ticket { nonce: 0, rank: 0, order: vec![], covered: vec![c], marginals: vec![m], probes: vec![Probe { axon: 0, t: 0, u: 1, value: 0 }] });
        let s = Sums::of(&g, &[t(0, 5 << 30), t(0, 7 << 30), t(1, -9), t(0, 9 << 30)]);
        assert_eq!((s.k[0], s.k[1]), (3, 1));
        assert_eq!(s.mean(0), 7 << 30);
        assert_eq!(s.mean(1), -9);
        assert_eq!(s.stderr(1), 0);
        // samples 5, 7, 9: variance of the mean = 4/3 → sqrt ≈ 1.15 at the 2^30 grain
        assert_eq!(s.stderr(0), 1 << 30);
        assert_eq!(s.samples(), 4);
        assert_eq!(Sums::of(&g, &[]).mean(0), 0);
    }

    #[test]
    fn tickets_cache_on_disk_and_refuse_a_foreign_commitment() {
        let dir = std::env::temp_dir().join(format!("soft3-tickets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = vec![([1; 32], [2; 32], 1_000_000u128), ([2; 32], [3; 32], 2_000_000)];
        let g = EpochGame::build(0, [0; 32], base, Vec::new(), 2, 2).unwrap();
        let mut h = super::super::super::header::tests::test_header();
        let t = g.solve(&mut h, 2);
        let mut store = Tickets::new(Some(dir.clone()), 2);
        assert!(!store.is_cached(&h));
        let v = store.verify(&h, &g).unwrap();
        assert_eq!(*v, t);
        let mut again = Tickets::new(Some(dir.clone()), 2);
        assert!(again.is_cached(&h), "the disk holds it");
        let mut other = h;
        other.sample[3] ^= 0x80;
        assert!(!again.is_cached(&other));
        assert!(again.verify(&other, &g).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
