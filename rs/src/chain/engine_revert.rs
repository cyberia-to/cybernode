//! The rollbacks of revert by release and of the owner's revert statement
//! (chain.md §5.6), beside [`super::engine`]: the open-time pass that cuts
//! the stored main chain, the live apply of a held statement, and the
//! statement set behind it. The table itself is [`super::revert`]; the
//! statement format [`super::revert_statement`].

use std::collections::HashSet;
use std::path::Path;

use super::api::{ChainError, Event};
use super::economy::run::Economy;
use super::economy::weights::GenesisWeights;
use super::engine::{Chain, REVERTED_LOG};
use super::header::BlockId;
use super::params::{FINALITY_DEPTH, REVERTS};
use super::revert::{self, RevertTable, Reverted};
use super::revert_statement::{self, RevertStatement, StatementError, StatementSet};
use super::select::Checkpoints;
use super::state::StateBackend;
use super::store::{self, Store};
use super::u256::U256;

/// The statement set with `st` taken in (`None`: it changes nothing), or why
/// not: the signature and bounds ([`RevertStatement::check`]), the supersede
/// rules ([`StatementSet::offer`]), and the table with it passing its static
/// and store checks. A `keep` the store holds at another height, or marked
/// invalid, is refused here — the statement is not held, so no node opens
/// into a table it cannot check.
fn judge(checkpoints: &Checkpoints, chain: &[u8; 32], store: &Store, held: &StatementSet, st: &RevertStatement) -> Result<Option<StatementSet>, StatementError> {
    st.check(chain, checkpoints)?;
    let Some(next) = held.offer(st) else { return Ok(None) };
    let table = RevertTable::new(REVERTS, Vec::new()).with_statements(next.entries());
    table.check_static().map_err(StatementError::Inconsistent)?;
    table.check_store(store).map_err(StatementError::Inconsistent)?;
    if store.get(&st.keep).is_some_and(|e| e.status == store::Status::Invalid) {
        return Err(StatementError::Inconsistent(format!("keep {} is a block this node holds invalid", super::net::codec::hex(&st.keep))));
    }
    Ok(Some(next))
}

impl Chain {
    /// Rollback of revert by release (§5.6): at open before the replay
    /// (`live` false), and on a running chain when a statement arrives
    /// (`live` true).
    ///
    /// The vetoed ids become permanently invalid; every stored block that is
    /// a vetoed id, a descendant of a block already marked, or any block
    /// other than `keep` at height `H` is marked invalid (side branches
    /// included). When that touches the stored main chain, the main chain is
    /// cut below its lowest invalid block. At open the state is not persisted
    /// — it is rebuilt from genesis by `replay()` right after, on the
    /// shortened main chain, with the stored-root checks — so there is
    /// nothing to undo. On a running chain the cut walks the state back block
    /// by block with the undo journals, the way a reorganization does
    /// (`retract`), which costs the depth of the revert instead of a replay
    /// of the whole chain; `true` is returned then, and the caller has no
    /// state to rebuild. Transactions of the retracted blocks (original
    /// order, coinbases skipped) are kept for the mempool, and one `Reverted`
    /// per triggered entry goes to the event stream (and the log) before any
    /// block event can exist.
    pub(super) fn apply_reverts(&mut self, home: &Path, live: bool) -> Result<bool, ChainError> {
        if self.reverts.is_empty() {
            return Ok(false);
        }
        let ids: Vec<BlockId> = self.reverts.vetoed_ids().copied().collect();
        self.revert_vetoed.extend(ids);
        let tip = self.store.tip_height();
        // a statement entry also fires when the main block at `H + 1` is older than its anchor
        let triggered: Vec<(Reverted, bool)> = self
            .reverts
            .entries()
            .iter()
            .filter(|e| {
                e.vetoed.iter().any(|v| self.store.on_main(v))
                    || (tip >= e.height && self.store.main_at(e.height).is_some_and(|id| id != e.keep))
                    || (e.counter > 0
                        && self.store.main_at(e.height.saturating_add(1)).and_then(|id| self.store.get(&id)).is_some_and(|b| b.header.time < e.anchor_time))
            })
            .map(|e| (Reverted { to_height: e.height, keep: e.keep, vetoed: e.vetoed.clone(), rehearsal: e.rehearsal }, e.counter > 0))
            .collect();
        let by_statement = triggered.iter().any(|(_, statement)| *statement);
        let min_height = self.reverts.entries().iter().map(|e| e.height).min().unwrap_or(0);
        let mut candidates: Vec<(u64, BlockId)> =
            self.store.iter().filter(|(_, e)| e.header.height >= min_height).map(|(id, e)| (e.header.height, *id)).collect();
        candidates.sort();
        let mut bad: HashSet<BlockId> = HashSet::new();
        for (height, id) in candidates {
            let Some(e) = self.store.get(&id) else { continue };
            let (prev, status, time) = (e.header.prev, e.status, e.header.time);
            let wrong_at_keep = self.reverts.keep_at(height).is_some_and(|keep| keep != id);
            // a statement vetoes by time: the block at `H + 1` older than the anchor is on the old side
            let below_floor = self.reverts.statement_floor(height).is_some_and(|floor| time < floor);
            if self.revert_vetoed.contains(&id) || bad.contains(&prev) || wrong_at_keep || below_floor {
                bad.insert(id);
                if status != store::Status::Invalid {
                    self.store.set_status(&id, store::Status::Invalid).map_err(ChainError::Store)?;
                }
            }
        }
        self.revert_vetoed.extend(bad.iter().copied());
        let mut undone = false;
        let first_bad = (1..=tip).find(|h| self.store.main_at(*h).is_some_and(|id| bad.contains(&id)));
        if let Some(cut) = first_bad {
            // the owner's statement drops the retracted transactions: the chain they were
            // sent for is not the chain the release returned to (§5.6)
            for h in (cut..=tip).filter(|_| !by_statement) {
                let Some(id) = self.store.main_at(h) else { continue };
                let Some(body) = self.store.body(&id) else { continue };
                if let Ok(raws) = super::tx::split_body(&body) {
                    self.retracted_txs.extend(raws.into_iter().skip(1));
                }
            }
            // on a running chain the state comes back with the undo journals of
            // the blocks the cut drops — every block that was ever extended has
            // one and they are never pruned; at open there is no state to undo
            undone = live && (cut..=tip).all(|h| self.store.main_at(h).is_some_and(|id| self.store.undo(&id).is_some()));
            while self.store.tip_height() >= cut {
                if undone {
                    self.retract()?;
                } else {
                    self.store.pop_main().map_err(ChainError::Store)?;
                }
            }
        }
        self.store.persist().map_err(ChainError::Store)?;
        for (r, _) in triggered {
            revert::append(&home.join(REVERTED_LOG), &r).map_err(|e| ChainError::Store(format!("{REVERTED_LOG}: {e}")))?;
            self.events.push(Event::Reverted { to_height: r.to_height, keep: r.keep, vetoed: r.vetoed.clone(), rehearsal: r.rehearsal });
            self.reverted = Some(r);
        }
        Ok(undone)
    }

    /// Hold a revert statement of the owner key (§5.6) and apply it now.
    /// `Ok(true)` when it is new: persisted, in the table, and the stored
    /// chain rolled back by the same path as at open — at any depth, the
    /// reorg limit does not apply — with the retracted transactions dropped.
    /// `Ok(false)` when it changes nothing (a duplicate, or a counter not
    /// above the highest held).
    pub(super) fn hold_revert_statement(&mut self, st: &RevertStatement) -> Result<bool, StatementError> {
        if self.reverts.any_rehearsal() {
            return Err(StatementError::Rehearsal);
        }
        let Some(next) = judge(&self.checkpoints, &self.rules.chain, &self.store, &self.statements, st)? else { return Ok(false) };
        revert_statement::append(&self.home.join(revert_statement::FILE), st).map_err(|e| StatementError::Store(e.to_string()))?;
        self.statements = next;
        self.reverts = RevertTable::new(REVERTS, Vec::new()).with_statements(self.statements.entries());
        self.reapply_reverts().map_err(|e| StatementError::Store(e.to_string()))?;
        // a block of a rule set this node did not know may be explained now (§7.1)
        if self.upgrade_required.is_some_and(|(_, rule_set)| rule_set <= self.reverts.last_rule_set()) {
            self.upgrade_required = None;
        }
        Ok(true)
    }

    /// The open-time rollback again on the running chain: the table changed.
    /// When the main chain was cut, the state walked back with the undo
    /// journals inside [`Self::apply_reverts`] — the kernel lock is held for
    /// the depth of the revert, not for a replay of the chain. Only when a
    /// journal is missing is the state rebuilt from genesis on the shortened
    /// chain, as `open_with` does (`replay`). Then fork choice takes a
    /// heavier stored chain on the right side of the cut.
    fn reapply_reverts(&mut self) -> Result<(), ChainError> {
        let home = self.home.clone();
        let old_tip = self.tip_id();
        let undone = self.apply_reverts(&home, true)?;
        self.retracted_txs.clear();
        if self.tip_id() != old_tip && !undone {
            let mut state = self.genesis.state();
            let root0 = state.root();
            let block0 = self.genesis.block0(root0);
            let mut economy = Economy::new(GenesisWeights::from_file(&self.genesis.file), self.economy.engine.clone(), self.rules.clock, Some(home.join("epochs")));
            economy.background = self.economy.background;
            economy.quadrature = self.economy.quadrature;
            economy.on_boundary(&state, 0, block0.id(), root0, self.rules.s0, None);
            self.state = state;
            self.economy = economy;
            self.coinbases.clear();
            self.replay()?;
        }
        if self.tip_id() != old_tip {
            self.finalized = self.store.tip_height().saturating_sub(FINALITY_DEPTH);
        }
        self.heaviest = self.store.iter().filter(|(_, e)| e.status != store::Status::Invalid).map(|(_, e)| e.chainwork).max().unwrap_or(U256::ZERO);
        self.after_tip_move()?;
        let _ = self.try_advance();
        Ok(())
    }

    /// The revert statements this node holds, ascending by counter.
    pub fn held_statements(&self) -> &[RevertStatement] {
        self.statements.statements()
    }
}
