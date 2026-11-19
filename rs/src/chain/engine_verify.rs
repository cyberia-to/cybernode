//! Validation step 8, the sample (chain.md §5.5 amended; rewards.md §7
//! "Verification"): the game a header samples, the recomputation of its
//! ticket under the node's budget of `V_BUDGET` full verifications per
//! block interval, the branch walk that makes a verified chain free to
//! follow, and the eviction that bounds the unverified set a flood of
//! target-passing headers can store.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::api::{BlockVerdict, ChainError};
use super::apply;
use super::engine::Chain;
use super::header::{BlockId, Header, hash32};
use super::params::BLOCK_TIME;
use super::store;
use super::ticket::EpochGame;
use super::u256::U256;

impl Chain {
    /// `V_BUDGET` on or off (replay switches it off itself; tests pin it on).
    #[cfg(test)]
    pub(crate) fn set_budget_on(&mut self, on: bool) {
        self.budget_on = on;
    }

    /// Set `V_BUDGET` (tests: the flood conformance test pins a small one).
    #[cfg(test)]
    pub(crate) fn set_v_budget(&mut self, b: usize) {
        self.v_budget_pinned = Some(b.max(1));
    }

    /// The branch the verifier is walking (the last block it verified).
    #[cfg(test)]
    pub(crate) fn walking_for_test(&self) -> Option<BlockId> {
        self.walking
    }

    /// Verifications spent in the open window, and the unverified set above
    /// the tip (tests: the flood conformance test).
    #[cfg(test)]
    pub(crate) fn flood_state(&self) -> (usize, usize) {
        (self.verifications.len(), self.unverified_above_tip())
    }

    /// Open a fresh verification window (tests: the flood conformance test
    /// renews the budget the next interval would).
    #[cfg(test)]
    pub(crate) fn renew_verify_budget(&mut self) {
        self.verifications.clear();
        self.budget_spent = false;
    }

    /// Headers heavier than the tip whose sample this node has not verified
    /// (`Status::Header` or `Status::Body`, not on the main chain): the set
    /// `UNVERIFIED_CAP` bounds against a forged-header flood (R18-5).
    pub(super) fn unverified_above_tip(&self) -> usize {
        let tip_work = self.entry(&self.tip_id()).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        self.store
            .iter()
            .filter(|(id, e)| e.chainwork > tip_work && matches!(e.status, store::Status::Header | store::Status::Body) && !self.store.on_main(id))
            .count()
    }

    /// Make room in the unverified set (R18-15; chain.md §5.4 amended): forget
    /// every header that has waited for its body longer than one interval,
    /// then, while the set is at `UNVERIFIED_CAP`, forget an entry chosen by
    /// the node's local draw — never by claimed chainwork or length, which
    /// would let a forged heavy tail outlive honest headers. A forgotten
    /// header is not remembered as rejected: re-offering it costs its sender
    /// the work again, and the store never grows past the cap.
    pub(super) fn evict_unverified(&mut self) {
        let now = Instant::now();
        let interval = Duration::from_secs(BLOCK_TIME as u64);
        let tip_work = self.entry(&self.tip_id()).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        let unverified = |id: &BlockId, e: &store::Entry, on_main: bool| e.chainwork > tip_work && matches!(e.status, store::Status::Header | store::Status::Body) && !on_main && id != &[0u8; 32];
        let stale: Vec<BlockId> = self
            .store
            .iter()
            .filter(|(id, e)| unverified(id, e, self.store.on_main(id)) && e.status == store::Status::Header && self.arrivals.get(*id).is_some_and(|t| now.duration_since(*t) >= interval))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.forget_unverified(&id);
        }
        while self.unverified_above_tip() >= super::params::UNVERIFIED_CAP {
            let victim = self
                .store
                .iter()
                .filter(|(id, e)| unverified(id, e, self.store.on_main(id)))
                .min_by_key(|(id, _)| hash32(&[&self.tie_salt[..], &id[..]].concat()))
                .map(|(id, _)| *id);
            match victim {
                Some(id) => self.forget_unverified(&id),
                None => break,
            }
        }
    }

    fn forget_unverified(&mut self, id: &BlockId) {
        if self.store.forget(id).is_ok() {
            self.arrivals.remove(id);
            self.heaviest = self.store.iter().filter(|(_, e)| e.status != store::Status::Invalid).map(|(_, e)| e.chainwork).max().unwrap_or(U256::ZERO);
        }
    }

    /// The budget in force: the pinned one (tests) or `⌊0.5·T / t_verify⌋`
    /// from the node's measured verification time, floor 1 (R18-4).
    pub(super) fn effective_v_budget(&self) -> usize {
        self.v_budget_pinned.unwrap_or_else(|| ((BLOCK_TIME as u64 * 500) / self.verify_ema_ms.max(1)).max(1) as usize)
    }

    /// The game the block at `height` samples (rewards.md §7): the game of
    /// `epoch(height) − 1`, from the boundaries on the parent's branch.
    /// `Err` names what is missing — a snapshot this node never took is its
    /// own condition, never the block's fault.
    pub(super) fn game_for(&mut self, parent: &BlockId, height: u64) -> Result<Arc<EpochGame>, ChainError> {
        let c = self.rules.clock;
        let e = c.game_of(height);
        let (prev, cur) = if e == 0 {
            let g = self.store.main_at(0).ok_or(ChainError::Store("no genesis".into()))?;
            (g, g)
        } else {
            let at = |h: u64| self.store.ancestor_at(parent, h).ok_or_else(|| ChainError::Game(format!("no ancestor at {h}")));
            (at(c.boundary(e - 1))?, at(c.boundary(e))?)
        };
        match self.economy.game(&prev, &cur) {
            Some(Ok(g)) => Ok(g),
            Some(Err(e)) => Err(ChainError::Game(e)),
            None => Err(ChainError::Game(format!("the boundary snapshots of epoch {e} are not held on this node"))),
        }
    }

    /// Chain.md §5.5 step 8, the sample: the header's ticket recomputed on
    /// its game and compared with its commitment, under the node-wide
    /// budget of `V_BUDGET` full verifications per block interval. A cached
    /// ticket costs nothing and is outside the budget.
    pub(super) fn verify_sample(&mut self, header: &Header) -> Result<(), ChainError> {
        // a zero sample is never an honest commitment (R18-5): refuse it
        // before a game is built or a verification slot is spent
        if header.sample == [0u8; 32] {
            return Err(ChainError::Block(apply::BlockError::Sample("the sample is zero".into())));
        }
        let game = self.game_for(&header.prev, header.height)?;
        // `V_BUDGET` counts branch attempts per interval, not blocks (R18-14,
        // tru#45 §7): the verifier walks one branch from its verified tip and
        // each success extends the tip at once — the next block of the same
        // branch continues for free, which is replay and initial download at
        // full speed; the first failure drops the branch and its descendants;
        // starting on another branch (a block whose parent is not the block
        // just verified, or any block after a failure) costs one slot. A
        // flood of forged headers at the tip is one branch attempt each, one
        // free after an honest success, so at most `V_BUDGET + 1`
        // recomputations per interval, and no chain length or claimed work
        // buys an exemption.
        let continuing = self.walking == Some(header.prev);
        if self.budget_on && !continuing && !self.economy.tickets.is_cached(header) {
            let now = Instant::now();
            let interval = Duration::from_secs(BLOCK_TIME as u64);
            while self.verifications.front().is_some_and(|t| now.duration_since(*t) >= interval) {
                self.verifications.pop_front();
            }
            if self.verifications.len() >= self.effective_v_budget() {
                self.budget_spent = true;
                return Err(ChainError::VerifyBudget);
            }
            self.verifications.push_back(now);
        }
        let cached = self.economy.tickets.is_cached(header);
        let t0 = Instant::now();
        let out = self.economy.tickets.verify(header, &game).map(|_| ()).map_err(|e| ChainError::Block(apply::BlockError::Sample(e.to_string())));
        if !cached {
            // the measured recomputation feeds the budget (R18-4): an EMA with
            // weight 1/8 on the latest, so a slow node spends fewer slots and a
            // fast one more, floor 1 either way
            let ms = (t0.elapsed().as_millis() as u64).max(1);
            self.verify_ema_ms = (self.verify_ema_ms * 7 + ms) / 8;
        }
        self.walking = if out.is_ok() { Some(header.id()) } else { None };
        out
    }

    /// Invalidate `id` and every stored descendant: a branch whose ticket
    /// failed is dead to its end, and a heavy body-less tail above a dead
    /// root never counts again (R18-14).
    pub(super) fn invalidate_subtree(&mut self, id: BlockId) {
        let Some(h) = self.entry(&id).map(|e| e.header.height) else { return };
        let below: Vec<BlockId> = self
            .store
            .iter()
            .filter(|(_, e)| e.status != store::Status::Invalid && e.header.height > h)
            .filter(|(d, _)| self.store.ancestor_at(d, h) == Some(id))
            .map(|(d, _)| *d)
            .collect();
        self.invalidate(id);
        for d in below {
            self.invalidate(d);
        }
        self.heaviest = self.store.iter().filter(|(_, e)| e.status != store::Status::Invalid).map(|(_, e)| e.chainwork).max().unwrap_or(U256::ZERO);
    }

    /// Walk one branch from the verified tip (R18-14, tru#45 §7): apply the
    /// root and then, while a child with a body exists (chosen by the local
    /// draw among siblings), the next block — each success extends the tip
    /// at once. `Ok(None)`: the root has no body yet. `Ok(Some(_))`: at
    /// least one block applied. `Err`: the root failed (a sample or body
    /// fault drops the whole branch; a spent budget leaves it waiting).
    pub(super) fn walk_branch(&mut self, root: BlockId) -> Result<Option<BlockVerdict>, ChainError> {
        let mut cur = root;
        let mut applied = false;
        loop {
            if !self.store.has_body(&cur) {
                return Ok(applied.then_some(BlockVerdict::Extended));
            }
            let (header, txs) = match self.decoded(&cur) {
                Ok(d) => d,
                Err(e) => {
                    if e.is_blocks_fault() {
                        self.invalidate_subtree(cur);
                    }
                    return if applied { Ok(Some(BlockVerdict::Extended)) } else { Err(e) };
                }
            };
            match self.extend(&header, &txs) {
                Ok(()) => {
                    applied = true;
                    self.after_tip_move()?;
                }
                Err(e) => return if applied { Ok(Some(BlockVerdict::Extended)) } else { Err(e) },
            }
            let next = self
                .store
                .iter()
                .filter(|(id, e)| e.header.prev == cur && e.status != store::Status::Invalid && self.store.has_body(id))
                .map(|(id, _)| (hash32(&[&self.tie_salt[..], &id[..]].concat()), *id))
                .min()
                .map(|(_, id)| id);
            match next {
                Some(id) => cur = id,
                None => return Ok(Some(BlockVerdict::Extended)),
            }
        }
    }
}
