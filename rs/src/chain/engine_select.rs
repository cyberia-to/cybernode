//! Fork choice (chain.md §5.1–§5.3): the candidates above the tip, the
//! advance that walks one branch per verification slot, the reorg under
//! the reorg limit and the held checkpoints, and what the node does once
//! the tip has moved.

use super::api::{BlockVerdict, ChainError, Event};
use super::engine::Chain;
use super::header::{BlockId, hash32};
use super::params::FINALITY_DEPTH;
use super::select;
use super::store;
use super::u256::U256;

impl Chain {
    /// Non-invalid headers heavier than the tip, heaviest first; ties among
    /// verified blocks in first-seen order (§5.1), ties among blocks whose
    /// sample is still to be checked in an order drawn from the node's own
    /// salt — unpredictable to a sender flooding target-passing headers
    /// (rewards.md §7 "Verification").
    fn candidates(&self) -> Vec<BlockId> {
        let tip_work = self.entry(&self.tip_id()).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        let mut v: Vec<(U256, u8, [u8; 32], BlockId)> = self
            .store
            .iter()
            .filter(|(_, e)| e.status != store::Status::Invalid && e.chainwork > tip_work)
            .map(|(id, e)| {
                let verified = e.status == store::Status::Valid;
                let key = if verified {
                    let mut k = [0u8; 32];
                    k[..8].copy_from_slice(&e.seq.to_be_bytes());
                    k
                } else {
                    hash32(&[&self.tie_salt[..], &id[..]].concat())
                };
                (e.chainwork, u8::from(!verified), key, *id)
            })
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        v.into_iter().map(|(_, _, _, id)| id).collect()
    }

    /// The path from the fork point (exclusive) to `id` (inclusive), oldest
    /// first, and the fork height.
    fn path_from_main(&self, id: &BlockId) -> Result<(u64, Vec<BlockId>), ChainError> {
        let mut path = Vec::new();
        let mut cur = *id;
        loop {
            if self.store.on_main(&cur) {
                let h = self.entry(&cur).map(|e| e.header.height).unwrap_or(0);
                path.reverse();
                return Ok((h, path));
            }
            let e = self.entry(&cur).ok_or(ChainError::Store("broken chain".into()))?;
            path.push(cur);
            cur = e.header.prev;
        }
    }

    /// Fork choice after a header or body arrives: move to the heaviest
    /// chain whose bodies are all present (§5.1–§5.3), or say what is
    /// missing. A heavier chain the reorg limit or a checkpoint forbids is
    /// reported and skipped in favour of the next candidate.
    pub fn try_advance(&mut self) -> Result<BlockVerdict, ChainError> {
        self.budget_spent = false;
        let tip = self.tip_id();
        let tip_work = self.entry(&tip).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        if !select::heavier(self.heaviest, tip_work) {
            return Ok(BlockVerdict::Known);
        }
        // 1. the branches rooted at the verified tip, in the node's local
        //    draw order and nothing else (tru#45 §7, chain.md §5.4 amended):
        //    claimed chainwork or length never orders an unverified branch,
        //    or a long body-less forged chain would outrank honest successors.
        //    The walker verifies one branch sequentially, each success
        //    extending the tip at once; the first failure drops the branch.
        let mut roots: Vec<([u8; 32], BlockId)> = self
            .store
            .iter()
            .filter(|(id, e)| e.header.prev == tip && e.status != store::Status::Invalid && !self.store.on_main(id))
            .map(|(id, _)| (hash32(&[&self.tie_salt[..], &id[..]].concat()), *id))
            .collect();
        roots.sort();
        let mut missing = Vec::new();
        let mut last_fault = None;
        for (_, root) in roots {
            match self.walk_branch(root) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {
                    // the root has no body: name every body the branch's best
                    // known path lacks, oldest first, for header-first sync
                    let h = self.entry(&root).map(|e| e.header.height).unwrap_or(0);
                    let best = self
                        .store
                        .iter()
                        .filter(|(id, e)| e.status != store::Status::Invalid && (**id == root || (e.header.height > h && self.store.ancestor_at(id, h) == Some(root))))
                        .max_by_key(|(_, e)| (e.chainwork, std::cmp::Reverse(e.seq)))
                        .map(|(id, _)| *id)
                        .unwrap_or(root);
                    let (_, path) = self.path_from_main(&best)?;
                    missing.extend(path.iter().filter(|id| !self.store.has_body(id)).copied());
                }
                Err(e) if e.is_blocks_fault() => last_fault = Some(e),
                Err(e) => return Err(e),
            }
        }
        // 2. deeper forks: a whole path whose bodies are all present, by
        //    verified chainwork order among applied candidates
        let mut first_error = None;
        for best in self.candidates() {
            let (fork_height, path) = self.path_from_main(&best)?;
            let tip_height = self.store.tip_height();
            if fork_height == tip_height {
                continue; // a branch of the tip: the walker's, above
            }
            if !select::reorg_allowed(tip_height, fork_height) {
                first_error.get_or_insert(ChainError::DeepFork { fork_height, tip_height });
                continue;
            }
            if let Some(floor) = self.checkpoints.floor() {
                // reorganizing below the floor is what a checkpoint forbids;
                // a tip still under it moves up through it (a chain that
                // excludes it is caught by `violation` below)
                if fork_height < floor && tip_height >= floor {
                    let expected = self.checkpoints.get(floor).unwrap_or([0; 32]);
                    first_error.get_or_insert(ChainError::Checkpoint { height: floor, expected });
                    continue;
                }
            }
            if let Some((height, expected)) = self.checkpoints.violation(|h| self.store.ancestor_at(&best, h)) {
                // the chain excludes a held checkpoint: not a candidate, nor its descendants
                self.veto(best);
                first_error.get_or_insert(ChainError::Checkpoint { height, expected });
                continue;
            }
            let missing_here: Vec<BlockId> = path.iter().filter(|id| !self.store.has_body(id)).copied().collect();
            if !missing_here.is_empty() {
                missing.extend(missing_here);
                continue;
            }
            return self.reorg(fork_height, &path);
        }
        // a root whose sample failed is reported before any missing body
        // (R18-17): its sender's +100 and ban must never be lost to another
        // root's state; the bodies still wanted are re-asked by the sync
        // pass through `missing_bodies`
        if let Some(e) = last_fault {
            return Err(e);
        }
        if !missing.is_empty() {
            return Ok(BlockVerdict::NeedBodies(missing));
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(BlockVerdict::Stored),
        }
    }

    /// The heaviest non-invalid header whose chain the node may still move
    /// to — the target of header-first body download (chain-wire §3.2) —
    /// or the tip when there is none. A heavier chain that the reorg limit
    /// or a checkpoint forbids is skipped: its bodies can never be applied,
    /// and following it would starve the bodies of the best chain the node
    /// can take (a released private fork deeper than `MAX_REORG` otherwise
    /// strands every honest node that sits on a stale branch).
    pub fn best_reachable(&self) -> (u64, BlockId, U256) {
        let tip = self.tip_id();
        let (tip_height, tip_work) = self.entry(&tip).map(|e| (e.header.height, e.chainwork)).unwrap_or((0, U256::ZERO));
        if !select::heavier(self.heaviest, tip_work) {
            return (tip_height, tip, tip_work);
        }
        for id in self.candidates() {
            if !self.own_chain_blocked(&id) {
                if let Some(e) = self.entry(&id) {
                    return (e.header.height, id, e.chainwork);
                }
            }
        }
        (tip_height, tip, tip_work)
    }

    /// A held checkpoint contradicts our main chain at `height` (chain.md
    /// §5.3: a chain not containing it is rejected — our own too): retract
    /// below `height`, mark the retracted blocks invalid, and move to the
    /// heaviest chain left (the checkpointed one when its blocks are known;
    /// sync fetches them otherwise). Bounded by the reorg limit like any
    /// reorganization: deeper than `MAX_REORG`, the node logs and keeps its
    /// chain — the operator resyncs (§5.2).
    pub(super) fn reject_main_from(&mut self, height: u64, ours: BlockId) -> Result<(), ChainError> {
        let tip_height = self.store.tip_height();
        let fork = height.saturating_sub(1);
        if !select::reorg_allowed(tip_height, fork) {
            eprintln!("chain: checkpoint at {height} contradicts the main chain {} blocks deep, beyond MAX_REORG: resync from a checkpoint", tip_height - fork);
            return Ok(());
        }
        self.veto(ours);
        while self.store.tip_height() > fork {
            let id = self.retract()?;
            self.veto(id);
        }
        self.heaviest = self.store.iter().filter(|(_, e)| e.status != store::Status::Invalid).map(|(_, e)| e.chainwork).max().unwrap_or(U256::ZERO);
        self.after_tip_move()?;
        let _ = self.try_advance();
        Ok(())
    }

    /// Does the reorg limit or a checkpoint floor forbid switching to `id`?
    pub(super) fn own_chain_blocked(&self, id: &BlockId) -> bool {
        let Ok((fork_height, _)) = self.path_from_main(id) else { return false };
        let tip_height = self.store.tip_height();
        let deep = !select::reorg_allowed(tip_height, fork_height);
        let below_floor = self.checkpoints.floor().is_some_and(|f| fork_height < f && tip_height >= f);
        let vetoed = self.entry(id).is_some_and(|e| e.status == store::Status::Invalid);
        deep || below_floor || vetoed
    }

    fn reorg(&mut self, fork_height: u64, path: &[BlockId]) -> Result<BlockVerdict, ChainError> {
        let old_tip = self.tip_id();
        let depth = self.store.tip_height() - fork_height;
        let mut retracted = Vec::new();
        while self.store.tip_height() > fork_height {
            retracted.push(self.retract()?);
        }
        let mut applied = 0usize;
        let mut failure = None;
        for id in path {
            let (header, txs) = match self.decoded(id) {
                Ok(d) => d,
                Err(e) => {
                    self.invalidate(*id);
                    failure = Some(e);
                    break;
                }
            };
            if let Err(e) = self.extend(&header, &txs) {
                failure = Some(e);
                break;
            }
            applied += 1;
        }
        if let Some(e) = failure {
            // every descendant of the failed block is invalid too — unless
            // the failure was this node's own condition
            if e.is_blocks_fault() {
                for id in &path[applied + 1..] {
                    self.invalidate(*id);
                }
            }
            for _ in 0..applied {
                self.retract()?;
            }
            for id in retracted.iter().rev() {
                let (header, txs) = self.decoded(id)?;
                self.extend(&header, &txs)?;
            }
            self.store.persist().map_err(ChainError::Store)?;
            return Err(e);
        }
        self.after_tip_move()?;
        Ok(BlockVerdict::Reorganized { old_tip, depth })
    }

    pub(super) fn after_tip_move(&mut self) -> Result<(), ChainError> {
        self.store.persist().map_err(ChainError::Store)?;
        self.economy.prune(self.store.tip_height());
        // an arrival is worth keeping only while `evict_unverified` can
        // still reach its header, and that walk stops at the tip's
        // chainwork: once the tip passes an orphan header, or the store
        // forgets it, the entry is dead weight. Dropping them here bounds
        // `arrivals` by the unverified set above the tip instead of by the
        // life of the process.
        let tip_height = self.store.tip_height();
        let store = &self.store;
        self.arrivals.retain(|id, _| store.get(id).is_some_and(|e| e.header.height > tip_height));
        let new_final = self.store.tip_height().saturating_sub(FINALITY_DEPTH);
        while self.finalized < new_final {
            let h = self.finalized + 1;
            if let Some(id) = self.store.main_at(h) {
                if let (Some(e), Some(body)) = (self.entry(&id), self.store.body(&id)) {
                    self.events.push(Event::Block { header: e.header, body, state_root: e.header.state_root });
                }
            }
            self.finalized = h;
        }
        Ok(())
    }
}
