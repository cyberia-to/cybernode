//! Opening a chain (chain.md §4, §5.6): the genesis file against the
//! store, the revert table and the held statements, the rollback a release
//! carries, and the replay of the main chain's bodies onto the state. The
//! [`Chain`] this returns is ready for the pipeline of [`super::engine`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use super::api::{ChainError, Event};
use super::apply::{self, Locked, Rules};
use super::economy::focus::{FocusEngine, Tru};
use super::economy::run::Economy;
use super::economy::weights::GenesisWeights;
use super::engine::{Chain, LAUNCH_CHAINS};
use super::genesis::Genesis;
use super::header::{BlockId, hash32};
use super::params::{BLOCK_TIME, CHAINS, EpochClock, FINALITY_DEPTH, REVERTS, v_budget, w_ticket};
use super::revert::{self, RevertEntry, RevertTable, Reverted};
use super::revert_statement::{self, RevertStatement, StatementError, StatementSet};
use super::select::Checkpoints;
use super::store::{self, Store};
use super::u256::U256;

/// The log of applied reverts under the chain's home (§5.6).
const REVERTED_LOG: &str = "reverted.log";
/// The node's own randomness for its tie draw: the clock and the process,
/// hashed. Local policy, never consensus.
fn local_salt() -> [u8; 32] {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let mut m = Vec::with_capacity(24);
    m.extend_from_slice(&t.as_nanos().to_le_bytes());
    m.extend_from_slice(&std::process::id().to_le_bytes());
    hash32(&m)
}

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
/// How a chain is opened beyond its genesis file.
#[derive(Clone, Default)]
pub struct OpenOptions {
    pub checkpoints: bool,
    /// a shortened epoch clock for a test or dev net; ignored (the spec's
    /// clock is used) for the launch chains
    pub clock: Option<EpochClock>,
    /// the φ* engine; tru's epoch engine when `None`
    pub engine: Option<Arc<dyn FocusEngine>>,
    /// gradient nodes on the epoch path; `ATTR_NODES` (the protocol) when 0
    pub quadrature: usize,
    /// compute settlements inline instead of on worker threads
    pub inline_settlement: bool,
    /// rehearsal revert entries (`--revert`, §5.6), added to the compiled
    /// table; refused on a production chain
    pub reverts: Vec<RevertEntry>,
}

impl Chain {
    /// Open or create the chain under `home` (`home/blocks` is the store).
    /// A store whose block 0 differs from the genesis file refuses to open.
    pub fn open(home: &Path, genesis_bytes: &[u8], checkpoints_enabled: bool) -> Result<Chain, ChainError> {
        let clock = EpochClock::from_env();
        Chain::open_with(home, genesis_bytes, OpenOptions { checkpoints: checkpoints_enabled, clock, ..Default::default() })
    }

    pub fn open_with(home: &Path, genesis_bytes: &[u8], opts: OpenOptions) -> Result<Chain, ChainError> {
        let genesis = Genesis::decode(genesis_bytes).map_err(|e| ChainError::Store(e.to_string()))?;
        let launch = LAUNCH_CHAINS.contains(&genesis.params().name.as_str());
        let clock = match opts.clock {
            Some(c) if !launch && c.valid() => c,
            _ => EpochClock::SPEC,
        };
        let reverts = RevertTable::effective(opts.reverts.clone());
        if reverts.any_rehearsal() && (launch || CHAINS.iter().any(|c| c.pinned() && c.chain_id == genesis.chain_id)) {
            return Err(ChainError::Store(format!(
                "--revert is a rehearsal and this chain ({}) is a production chain: a drill must never feed a real indexer",
                genesis.params().name
            )));
        }
        reverts.check_static().map_err(|e| ChainError::Store(format!("revert table: {e}")))?;
        let name = genesis.params().name.clone();
        let rules = Rules {
            chain: genesis.chain_id,
            native: genesis.native_token(),
            hrp: genesis.params().hrp.clone(),
            subsidy: genesis.subsidy(),
            s0: genesis.s0(),
            clock,
            w_ticket: w_ticket(&name),
            v_budget: v_budget(&name),
        };
        std::fs::create_dir_all(home).map_err(|e| ChainError::Store(e.to_string()))?;
        let mut store = Store::open(&home.join("blocks")).map_err(ChainError::Store)?;
        let mut state = genesis.state();
        let root0 = state.root();
        let block0 = genesis.block0(root0);
        let checkpoints = Checkpoints::new(genesis.params().checkpoint_key, opts.checkpoints);
        if store.is_empty() {
            store.put_header(block0, U256::ZERO, store::Status::Valid).map_err(ChainError::Store)?;
            store.push_main(block0.id()).map_err(ChainError::Store)?;
            store.persist().map_err(ChainError::Store)?;
        } else if store.main_at(0) != Some(block0.id()) {
            return Err(ChainError::Genesis);
        }
        // the owner's revert statements (§5.6), before the table is checked against the store
        let mut statements = StatementSet::default();
        let mut reverts = reverts;
        if checkpoints.enabled() {
            let held = revert_statement::load(&home.join(revert_statement::FILE));
            if !held.is_empty() && reverts.any_rehearsal() {
                return Err(ChainError::Store(format!("{} ({} holds statements): drop --revert or the file", StatementError::Rehearsal, revert_statement::FILE)));
            }
            for st in held {
                match judge(&checkpoints, &genesis.chain_id, &store, &statements, &st) {
                    Ok(Some(next)) => statements = next,
                    Ok(None) => {}
                    Err(e) => eprintln!("chain: statement at {} counter {} in {} ignored: {e}", st.height, st.counter, revert_statement::FILE),
                }
            }
            if !statements.is_empty() {
                reverts = RevertTable::new(REVERTS, Vec::new()).with_statements(statements.entries());
            }
        }
        let engine: Arc<dyn FocusEngine> = opts.engine.unwrap_or_else(|| Arc::new(Tru::for_node()));
        let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let mut economy = Economy::new(GenesisWeights::from_file(&genesis.file), engine, clock, Some(home.to_path_buf()), rules.w_ticket, workers);
        economy.background = !opts.inline_settlement;
        economy.quadrature = if opts.quadrature == 0 { super::params::ATTR_NODES } else { opts.quadrature };
        economy.on_boundary(&state, 0, block0.id(), root0, rules.s0);
        let verify_ema_ms = (BLOCK_TIME as u64 * 500 / rules.v_budget.max(1) as u64).max(1);
        let mut chain = Chain {
            genesis,
            rules,
            store,
            state,
            checkpoints,
            coinbases: VecDeque::new(),
            rejected: HashSet::new(),
            rejected_order: VecDeque::new(),
            vetoed: HashSet::new(),
            heaviest: U256::ZERO,
            finalized: 0,
            events: Vec::new(),
            economy,
            reverts,
            revert_vetoed: HashSet::new(),
            upgrade_required: None,
            statements,
            home: home.to_path_buf(),
            reverted: revert::load_last(&home.join(REVERTED_LOG)),
            retracted_txs: Vec::new(),
            verifications: VecDeque::new(),
            tie_salt: local_salt(),
            budget_spent: false,
            budget_on: true,
            arrivals: HashMap::new(),
            verify_ema_ms,
            v_budget_pinned: None,
            walking: None,
        };
        chain.reverts.check_store(&chain.store).map_err(|e| ChainError::Store(format!("revert table: {e}")))?;
        chain.apply_reverts(home)?;
        chain.replay()?;
        chain.heaviest = chain.store.iter().filter(|(_, e)| e.status != store::Status::Invalid).map(|(_, e)| e.chainwork).max().unwrap_or(U256::ZERO);
        chain.finalized = chain.store.tip_height().saturating_sub(FINALITY_DEPTH);
        Ok(chain)
    }

    /// Open-time rollback of revert by release (§5.6), before the replay.
    ///
    /// The vetoed ids become permanently invalid; every stored block that is
    /// a vetoed id, a descendant of a block already marked, or any block
    /// other than `keep` at height `H` is marked invalid (side branches
    /// included). When that touches the stored main chain, the main chain is
    /// cut below its lowest invalid block. The state is not persisted — it is
    /// rebuilt from genesis by `replay()` right after, on the shortened main
    /// chain, with the stored-root checks — so there is nothing to undo: the
    /// undo journals are not needed and are left in place. Transactions of
    /// the retracted blocks (original order, coinbases skipped) are kept for
    /// the mempool, and one `Reverted` per triggered entry goes to the event
    /// stream (and the log) before any block event can exist.
    fn apply_reverts(&mut self, home: &Path) -> Result<(), ChainError> {
        if self.reverts.is_empty() {
            return Ok(());
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
            while self.store.tip_height() >= cut {
                self.store.pop_main().map_err(ChainError::Store)?;
            }
        }
        self.store.persist().map_err(ChainError::Store)?;
        for (r, _) in triggered {
            revert::append(&home.join(REVERTED_LOG), &r).map_err(|e| ChainError::Store(format!("{REVERTED_LOG}: {e}")))?;
            self.events.push(Event::Reverted { to_height: r.to_height, keep: r.keep, vetoed: r.vetoed.clone(), rehearsal: r.rehearsal });
            self.reverted = Some(r);
        }
        Ok(())
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
    /// When the main chain was cut, the state is rebuilt from genesis on the
    /// shortened chain exactly as `open_with` does (`replay`), then fork
    /// choice takes a heavier stored chain on the right side of the cut.
    fn reapply_reverts(&mut self) -> Result<(), ChainError> {
        let home = self.home.clone();
        let old_tip = self.tip_id();
        self.apply_reverts(&home)?;
        self.retracted_txs.clear();
        if self.tip_id() != old_tip {
            let mut state = self.genesis.state();
            let root0 = state.root();
            let block0 = self.genesis.block0(root0);
            let workers = self.economy.tickets.workers;
            let mut economy = Economy::new(GenesisWeights::from_file(&self.genesis.file), self.economy.engine.clone(), self.rules.clock, Some(home.clone()), self.rules.w_ticket, workers);
            economy.background = self.economy.background;
            economy.quadrature = self.economy.quadrature;
            economy.on_boundary(&state, 0, block0.id(), root0, self.rules.s0);
            self.state = state;
            self.economy = economy;
            self.coinbases.clear();
            self.replay()?;
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

    /// Recovery: replay every main-chain body on the genesis state and
    /// verify the stored roots before readiness (§3.6).
    fn replay(&mut self) -> Result<(), ChainError> {
        let tip = self.store.tip_height();
        // replay from disk is not a competition at the tip: verify every
        // stored ticket at full speed, so a node whose `tickets/` cache is
        // gone still opens (R18-2; rewards.md §7 "Verification").
        let budget_on = std::mem::replace(&mut self.budget_on, false);
        let out = self.replay_inner(tip);
        self.budget_on = budget_on;
        out
    }

    fn replay_inner(&mut self, tip: u64) -> Result<(), ChainError> {
        for h in 1..=tip {
            let id = self.store.main_at(h).ok_or_else(|| ChainError::Store(format!("main chain gap at {h}")))?;
            let header = self.store.get(&id).map(|e| e.header).ok_or_else(|| ChainError::Store("missing header".into()))?;
            let body = self.store.body(&id).ok_or_else(|| ChainError::Store(format!("missing body at {h}")))?;
            let txs = apply::decode_body(&self.rules, &header, &body).map_err(ChainError::Block)?;
            let locked = Locked::at(self.coinbases.make_contiguous(), h);
            self.apply_on_tip(&header, &txs, &locked)?;
            self.push_coinbase(h, &txs);
        }
        self.economy.prune(tip);
        Ok(())
    }
}
