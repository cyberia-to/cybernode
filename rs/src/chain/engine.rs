//! The chain engine: [`Chain`] over the store, the bbg-backed state, the
//! checkpoints and the validation order of §5.5. This module holds the
//! type itself, the settlement a block carries and the block pipeline on
//! the tip (apply, extend, retract); the rest of the engine is one module
//! per stage, each an `impl Chain` beside this one:
//!
//! - [`super::engine_open`] — opening a chain, the reverts and the replay;
//! - [`super::engine_check`] — steps 1–5, the header alone;
//! - [`super::engine_verify`] — step 8, the sample under `V_BUDGET`;
//! - [`super::engine_select`] — fork choice (§5.1–§5.3) and the tip move;
//! - [`super::engine_api`] — the [`ChainApi`](super::api::ChainApi) impl;
//! - [`super::engine_tx`] — mempool and template entry points.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use super::api::{ChainError, Event};
use super::apply::{self, Decoded, Locked, Rules};
use super::bbg_state::BbgBacked;
use super::economy::lottery::Lottery;
use super::economy::run::Economy;
use super::economy::settle::SettleResult;
use super::genesis::Genesis;
use super::header::{BlockId, Header};
use super::params::COINBASE_MATURITY;
use super::revert::{RevertTable, Reverted};
use super::revert_statement::StatementSet;
use super::select::Checkpoints;
use super::state::{Id, StateBackend};
use super::store::{self, Store};
use super::tx::Body;
use super::u256::U256;

pub use super::engine_open::OpenOptions;

const REJECTED_CAPACITY: usize = 4_096;

pub struct Chain {
    pub(super) genesis: Genesis,
    pub(super) rules: Rules,
    pub(super) store: Store,
    pub(super) state: BbgBacked,
    pub(super) checkpoints: Checkpoints,
    /// main-chain coinbases `(height, to, amount)` still inside maturity
    pub(super) coinbases: VecDeque<(u64, Id, u64)>,
    pub(super) rejected: HashSet<BlockId>,
    pub(super) rejected_order: VecDeque<BlockId>,
    /// blocks invalid only for contradicting a held checkpoint (§5.3): a
    /// peer relaying them or their children is on the other branch, not
    /// misbehaving — refused as `Checkpoint`, never scored
    pub(super) vetoed: HashSet<BlockId>,
    /// the most work any non-invalid header carries; when the tip has it,
    /// there is nothing to consider
    pub(super) heaviest: U256,
    pub(super) finalized: u64,
    pub(super) events: Vec<Event>,
    /// the focus economy's snapshots, settlement workers and outcomes
    pub(super) economy: Economy,
    /// the effective revert table (§5.6): compiled plus rehearsal entries
    pub(super) reverts: RevertTable,
    /// ids invalid by the revert table: the vetoed ids, the blocks marked at
    /// open (a wrong block at `H`, descendants of a vetoed id). Unlike
    /// `vetoed` it is never cleared, and a header that is, or descends from,
    /// one is refused without a score (chain-wire §5)
    pub(super) revert_vetoed: HashSet<BlockId>,
    /// `(height, rule_set)` of the last block with valid proof of work whose
    /// rule set this node does not know (§7.1)
    pub(super) upgrade_required: Option<(u64, u16)>,
    /// the revert statements of the owner key this node holds (§5.6),
    /// persisted in `home/revert-statements`; their entries are in `reverts`
    pub(super) statements: StatementSet,
    /// the chain's home: where the logs and the statements live
    pub(super) home: PathBuf,
    /// the last revert applied (persisted in `home/reverted.log`)
    pub(super) reverted: Option<Reverted>,
    /// transactions of the main-chain blocks a revert retracted at open, in
    /// original block order, coinbases skipped; taken by the mempool
    pub(super) retracted_txs: Vec<Vec<u8>>,
    /// the instants of the full sample verifications of the last block
    /// interval: `V_BUDGET` of them per interval (chain.md §5.5 step 8)
    pub(super) verifications: VecDeque<Instant>,
    /// the node's own draw for ties among unverified candidates of equal
    /// chainwork (rewards.md §7 "Verification"): local, never consensus
    pub(super) tie_salt: [u8; 32],
    /// a block waits here when the budget is spent; retried by `try_advance`
    pub(super) budget_spent: bool,
    /// `V_BUDGET` governs only gossip-delivered headers competing at the
    /// tip (rewards.md §7 "Verification"); replay from disk and initial
    /// block download verify at full speed, so a node without `tickets/`
    /// opens and a node days behind catches up. `false` during replay and
    /// while the net reports it is catching up (chain-wire §3).
    pub(super) budget_on: bool,
    /// when each unverified header arrived (R18-15: a header without a body
    /// for longer than one interval is evicted); dropped on verify or evict
    pub(super) arrivals: HashMap<BlockId, Instant>,
    /// the node's measured verification time, an EMA in milliseconds of
    /// its last full recomputations (R18-4): `V_BUDGET = ⌊0.5·T / t_verify⌋`
    /// follows it, floor 1; seeded from the chain's nominal budget
    pub(super) verify_ema_ms: u64,
    /// a pinned budget (tests only); `None` follows the EMA
    pub(super) v_budget_pinned: Option<usize>,
    /// the block the verifier last verified: its child continues the branch
    /// for free; `None` after a failure (the next attempt costs a slot)
    pub(super) walking: Option<BlockId>,
}

/// The launch chains: they run the spec's epoch clock and nothing else.
pub const LAUNCH_CHAINS: &[&str] = &["pussy", "bostrom"];

impl Chain {
    /// The three boundary ids the settlement of epoch `e` reads on the
    /// main chain: `boundary(e − 1)`, `boundary(e)`, `boundary(e + 1)`.
    fn settlement_key(&self, e: u64) -> Option<(BlockId, BlockId, BlockId)> {
        let c = self.rules.clock;
        Some((self.store.main_at(c.boundary(e - 1))?, self.store.main_at(c.boundary(e))?, self.store.main_at(c.boundary(e + 1))?))
    }

    /// The lottery of epoch `e` on the main chain: its game and the tickets
    /// of the window `(boundary(e), boundary(e + 1)]`, each verified or
    /// recomputed (a replay without the ticket cache solves them again).
    fn lottery_of(&mut self, e: u64, prev: &BlockId, cur: &BlockId) -> Option<Lottery> {
        let game = self.economy.game(prev, cur)?.ok()?;
        let c = self.rules.clock;
        let mut tickets = Vec::with_capacity(c.length as usize);
        for h in c.boundary(e) + 1..=c.boundary(e + 1) {
            let id = self.store.main_at(h)?;
            let header = self.entry(&id)?.header;
            tickets.push(self.economy.tickets.verify(&header, &game).ok()?);
        }
        Some(Lottery { game, tickets })
    }

    /// The lottery of epoch `e` whose window closes with the block
    /// `(end_id, end_header)` now being applied at `boundary(e + 1)` — that
    /// block is not on the main table yet, so it is read from the arguments
    /// and every earlier window block from the store (R18-3: the settlement
    /// job starts at the window's close, the D-block head start of
    /// chain-economy §2.5).
    fn lottery_closing(&mut self, e: u64, prev: &BlockId, cur: &BlockId, end_id: &BlockId, end_header: &Header) -> Option<Lottery> {
        let game = self.economy.game(prev, cur)?.ok()?;
        let c = self.rules.clock;
        let last = c.boundary(e + 1);
        let mut tickets = Vec::with_capacity(c.length as usize);
        for h in c.boundary(e) + 1..=last {
            let header = if h == last { *end_header } else { self.entry(&self.store.main_at(h)?)?.header };
            tickets.push(self.economy.tickets.verify(&header, &game).ok()?);
        }
        let _ = end_id;
        Some(Lottery { game, tickets })
    }

    /// The settlement the block at `height` (on the main tip) must carry,
    /// if one is due: the result reading the main chain's boundaries of
    /// epochs E − 1, E and E + 1 (chain-economy §2.5 amended).
    pub(super) fn settlement_for(&mut self, height: u64) -> Option<Arc<SettleResult>> {
        let e = self.rules.clock.settles_at(height)?;
        let (prev, cur, end) = self.settlement_key(e)?;
        if let Some(r) = self.economy.lottery_of(&prev, &cur, &end) {
            return Some(r);
        }
        let lottery = self.lottery_of(e, &prev, &cur);
        self.economy.result(&prev, &cur, &end, move || lottery)
    }

    /// Steps 7–8 for a block whose parent is the main tip, with the
    /// economy's side effects: the sample verified, the settlement result
    /// in, the outcome kept, the boundary snapshot taken and the closed
    /// epoch's settlement started.
    pub(super) fn apply_on_tip(&mut self, header: &Header, txs: &[Decoded], locked: &Locked) -> Result<super::state::Undo, ChainError> {
        self.verify_sample(header)?;
        let settle = self.settlement_for(header.height);
        let applied = apply::apply_block(&self.rules, header, txs, &mut self.state, locked, settle.as_deref()).map_err(ChainError::Block)?;
        let id = header.id();
        if let Some(outcome) = applied.settled {
            self.economy.record_outcome(id, outcome);
        }
        let clock = self.rules.clock;
        if clock.is_boundary(header.height) {
            let epoch = clock.epoch(header.height);
            let supply = self.rules.supply_at(header.height);
            self.economy.on_boundary(&self.state, epoch, id, header.state_root, supply);
            // the window of epoch E = epoch − 1 closes with this boundary
            // block: start its settlement now, D blocks before settle(E), on
            // a worker thread (R18-3, chain-economy §2.5). This block is not
            // on the main table yet, so it is `end` and the window's last
            // ticket; `prev` and `cur` are earlier boundaries on main.
            if epoch >= 2 {
                let c = self.rules.clock;
                let e = epoch - 1;
                if let (Some(prev), Some(cur)) = (self.store.main_at(c.boundary(e - 1)), self.store.main_at(c.boundary(e))) {
                    if let Some(lottery) = self.lottery_closing(e, &prev, &cur, &id, header) {
                        self.economy.start(prev, cur, id, lottery);
                    }
                }
            }
        }
        Ok(applied.undo)
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn state(&self) -> &BbgBacked {
        &self.state
    }

    pub(super) fn entry(&self, id: &BlockId) -> Option<&store::Entry> {
        self.store.get(id)
    }

    pub(super) fn tip_id(&self) -> BlockId {
        self.store.tip().expect("genesis is always present").1
    }

    pub(super) fn push_coinbase(&mut self, height: u64, txs: &[Decoded]) {
        if let Some(Body::Coinbase { to, amount, .. }) = txs.first().map(|d| &d.tx.body) {
            self.coinbases.push_back((height, *to, *amount));
        }
        while self.coinbases.front().is_some_and(|(h, _, _)| h.saturating_add(COINBASE_MATURITY) <= height.saturating_add(1)) {
            self.coinbases.pop_front();
        }
    }

    pub(super) fn locked_at(&mut self, height: u64) -> Locked {
        Locked::at(self.coinbases.make_contiguous(), height)
    }

    fn remember_rejected(&mut self, id: BlockId) {
        if self.rejected.insert(id) {
            self.rejected_order.push_back(id);
            if self.rejected_order.len() > REJECTED_CAPACITY {
                if let Some(old) = self.rejected_order.pop_front() {
                    self.rejected.remove(&old);
                }
            }
        }
    }

    pub(super) fn invalidate(&mut self, id: BlockId) {
        let _ = self.store.set_status(&id, store::Status::Invalid);
        self.arrivals.remove(&id);
        self.remember_rejected(id);
    }

    /// Invalid for contradicting a held checkpoint (see `vetoed`).
    pub(super) fn veto(&mut self, id: BlockId) {
        self.invalidate(id);
        if self.vetoed.len() >= REJECTED_CAPACITY {
            self.vetoed.clear();
        }
        self.vetoed.insert(id);
    }

    /// The error for a header that is, or descends from, a vetoed block.
    pub(super) fn vetoed_error(&self, height: u64) -> ChainError {
        let held = self.checkpoints.held().filter(|(h, _)| **h <= height).last().map(|(_, id)| *id);
        // a revert's `keep` is a compiled checkpoint (§5.6)
        let expected = held.or_else(|| self.reverts.latest_at_or_below(height).map(|e| e.keep)).unwrap_or([0; 32]);
        ChainError::Checkpoint { height, expected }
    }

    /// Apply a block whose parent is the tip (steps 7–8), then extend.
    pub(super) fn extend(&mut self, header: &Header, txs: &[Decoded]) -> Result<(), ChainError> {
        let id = header.id();
        let locked = self.locked_at(header.height);
        let undo = match self.apply_on_tip(header, txs, &locked) {
            Ok(undo) => undo,
            // a settlement or a game this node cannot compute, or a spent
            // verification budget, is its own condition, never the block's
            // fault (chain-economy §2.5: it waits)
            Err(e) if !e.is_blocks_fault() => return Err(e),
            Err(e) => {
                // the branch is dropped at its first failed ticket, with every
                // stored descendant (R18-14): none of them is verified again
                self.invalidate_subtree(id);
                return Err(e);
            }
        };
        self.store.put_undo(&id, &undo).map_err(ChainError::Store)?;
        self.store.set_status(&id, store::Status::Valid).map_err(ChainError::Store)?;
        self.arrivals.remove(&id);
        self.store.push_main(id).map_err(ChainError::Store)?;
        self.push_coinbase(header.height, txs);
        Ok(())
    }

    /// Undo the tip block (its journal is in the store).
    pub(super) fn retract(&mut self) -> Result<BlockId, ChainError> {
        let (height, id) = self.store.tip().ok_or(ChainError::Store("empty chain".into()))?;
        if height == 0 {
            return Err(ChainError::Store("cannot retract genesis".into()));
        }
        let undo = self.store.undo(&id).ok_or(ChainError::Store("missing undo".into()))?;
        self.state.revert(&undo);
        self.store.pop_main().map_err(ChainError::Store)?;
        self.rebuild_coinbases();
        Ok(id)
    }

    fn rebuild_coinbases(&mut self) {
        self.coinbases.clear();
        let tip = self.store.tip_height();
        let from = tip.saturating_sub(COINBASE_MATURITY).max(1);
        for h in from..=tip {
            let Some(id) = self.store.main_at(h) else { continue };
            let Some(body) = self.store.body(&id) else { continue };
            if let Ok(raws) = super::tx::split_body(&body) {
                if let Some(Ok(tx)) = raws.first().map(|r| super::tx::Tx::decode(r)) {
                    if let Body::Coinbase { to, amount, .. } = tx.body {
                        self.coinbases.push_back((h, to, amount));
                    }
                }
            }
        }
    }

    pub(super) fn decoded(&self, id: &BlockId) -> Result<(Header, Vec<Decoded>), ChainError> {
        let header = self.entry(id).map(|e| e.header).ok_or(ChainError::Store("missing header".into()))?;
        let body = self.store.body(id).ok_or(ChainError::Store("missing body".into()))?;
        let txs = apply::decode_body(&self.rules, &header, &body).map_err(ChainError::Block)?;
        Ok((header, txs))
    }


}

impl Chain {
    pub(super) fn state_mut(&mut self) -> &mut BbgBacked {
        &mut self.state
    }

    pub(super) fn mtp_of_tip(&self) -> u64 {
        self.mtp(&self.tip_id())
    }

    pub(super) fn tip_header(&self) -> Header {
        let id = self.tip_id();
        self.entry(&id).expect("tip stored").header
    }

    /// Drop the finalized block events nobody takes (the wire has no use for
    /// them, §7.3) and keep the `Reverted` ones for the indexers.
    pub fn discard_block_events(&mut self) {
        self.events.retain(|e| !matches!(e, Event::Block { .. }));
    }

    /// Transactions of the blocks a revert retracted at open, original block
    /// order, coinbases skipped (§5.6); drained by the first caller, which
    /// re-admits them to its mempool.
    pub fn take_retracted_txs(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.retracted_txs)
    }

    pub fn reverts(&self) -> &RevertTable {
        &self.reverts
    }

    pub fn reverted(&self) -> Option<&Reverted> {
        self.reverted.as_ref()
    }

    pub fn upgrade_required(&self) -> Option<(u64, u16)> {
        self.upgrade_required
    }
}
