//! The chain engine: [`Chain`] implements [`ChainApi`] over the store, the
//! bbg-backed state, the checkpoints and the validation order of §5.5.
//! Headers are validated in steps 1–5 and stored; bodies run steps 6–8 on
//! the parent state; fork choice (§5.1–§5.3) moves the tip with the undo
//! journals. The [`ChainApi`](super::api::ChainApi) impl is in
//! [`super::engine_api`]; mempool and template entry points in
//! [`super::engine_tx`].

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::api::{BlockVerdict, ChainError, Event};
use super::apply::{self, Decoded, Locked, Rules};
use super::bbg_state::BbgBacked;
use super::daa::{median_time_past, next_bits, next_bits_reverted};
use super::economy::focus::{FocusEngine, Tru};
use super::economy::run::Economy;
use super::economy::settle::SettleResult;
use super::economy::weights::GenesisWeights;
use super::genesis::Genesis;
use super::header::{BlockId, Header, HeaderError, decode_bits};
use super::params::{CHAINS, COINBASE_MATURITY, EpochClock, FINALITY_DEPTH, MAX_FUTURE_DRIFT, MTP_WINDOW, REVERTS, RULE_SET};
use super::pow::{chainwork, valid_pow};
use super::revert::{self, RevertEntry, RevertTable, Reverted};
use super::revert_statement::{self, RevertStatement, StatementError, StatementSet};
use super::select::{self, Checkpoints};
use super::state::{Id, StateBackend};
use super::store::{self, Store};
use super::tx::Body;
use super::u256::U256;

const REJECTED_CAPACITY: usize = 4_096;

/// The log of applied reverts under the chain's home (§5.6).
const REVERTED_LOG: &str = "reverted.log";

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
}

/// The launch chains: they run the spec's epoch clock and nothing else.
pub const LAUNCH_CHAINS: &[&str] = &["pussy", "bostrom"];

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
        let rules = Rules {
            chain: genesis.chain_id,
            native: genesis.native_token(),
            hrp: genesis.params().hrp.clone(),
            subsidy: genesis.subsidy(),
            s0: genesis.s0(),
            clock,
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
        let mut economy = Economy::new(GenesisWeights::from_file(&genesis.file), engine, clock, Some(home.join("epochs")));
        economy.background = !opts.inline_settlement;
        economy.quadrature = if opts.quadrature == 0 { super::params::ATTR_NODES } else { opts.quadrature };
        economy.on_boundary(&state, 0, block0.id(), root0, rules.s0, None);
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
            let mut economy = Economy::new(GenesisWeights::from_file(&self.genesis.file), self.economy.engine.clone(), self.rules.clock, Some(home.join("epochs")));
            economy.background = self.economy.background;
            economy.quadrature = self.economy.quadrature;
            economy.on_boundary(&state, 0, block0.id(), root0, self.rules.s0, None);
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
        for h in 1..=tip {
            let id = self.store.main_at(h).ok_or_else(|| ChainError::Store(format!("main chain gap at {h}")))?;
            let header = self.store.get(&id).map(|e| e.header).ok_or_else(|| ChainError::Store("missing header".into()))?;
            let body = self.store.body(&id).ok_or_else(|| ChainError::Store(format!("missing body at {h}")))?;
            let txs = apply::decode_body(&self.rules, &header, &body).map_err(ChainError::Block)?;
            let locked = Locked::at(self.coinbases.make_contiguous(), h);
            self.apply_on_tip(&header, &txs, &locked).map_err(ChainError::Block)?;
            self.push_coinbase(h, &txs);
        }
        self.economy.prune(tip);
        Ok(())
    }

    /// The settlement the block at `height` (on the main tip) must carry,
    /// if one is due: the result reading the main chain's boundaries of
    /// epochs E − 1 and E (chain-economy §2.5).
    pub(super) fn settlement_for(&mut self, height: u64) -> Option<Arc<SettleResult>> {
        let e = self.rules.clock.settles_at(height)?;
        let prev = self.store.main_at(self.rules.clock.boundary(e - 1))?;
        let cur = self.store.main_at(self.rules.clock.boundary(e))?;
        self.economy.result(&prev, &cur)
    }

    /// Steps 7–8 for a block whose parent is the main tip, with the
    /// economy's side effects: the settlement result in, the outcome kept,
    /// the boundary snapshot taken and its settlement started.
    fn apply_on_tip(&mut self, header: &Header, txs: &[Decoded], locked: &Locked) -> Result<super::state::Undo, apply::BlockError> {
        let settle = self.settlement_for(header.height);
        let applied = apply::apply_block(&self.rules, header, txs, &mut self.state, locked, settle.as_deref())?;
        let id = header.id();
        if let Some(outcome) = applied.settled {
            self.economy.record_outcome(id, outcome);
        }
        let clock = self.rules.clock;
        if clock.is_boundary(header.height) {
            let epoch = clock.epoch(header.height);
            let prev = self.store.main_at(clock.boundary(epoch - 1));
            let supply = self.rules.supply_at(header.height);
            self.economy.on_boundary(&self.state, epoch, id, header.state_root, supply, prev);
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

    fn push_coinbase(&mut self, height: u64, txs: &[Decoded]) {
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

    /// Non-invalid headers heavier than the tip, heaviest first, ties in
    /// first-seen order (§5.1).
    fn candidates(&self) -> Vec<BlockId> {
        let tip_work = self.entry(&self.tip_id()).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        let mut v: Vec<(U256, u64, BlockId)> = self
            .store
            .iter()
            .filter(|(_, e)| e.status != store::Status::Invalid && e.chainwork > tip_work)
            .map(|(id, e)| (e.chainwork, e.seq, *id))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Median time past of the parent and its ancestors (§1.4).
    fn mtp(&self, parent: &BlockId) -> u64 {
        let mut times = Vec::with_capacity(MTP_WINDOW);
        let mut cur = *parent;
        while times.len() < MTP_WINDOW {
            let Some(e) = self.entry(&cur) else { break };
            times.push(e.header.time);
            if e.header.height == 0 {
                break;
            }
            cur = e.header.prev;
        }
        median_time_past(&times)
    }

    /// Steps 1–5 without storing.
    pub(super) fn check_header(&self, header: &Header, now: u64) -> Result<U256, ChainError> {
        // §7.1 extended by the revert entries (§5.6): the rule set the height requires
        let Some(rule_set) = self.reverts.rule_set_at(header.height) else {
            return Err(ChainError::RuleSetUnknown(header.height));
        };
        header.check_syntax_for(&self.rules.chain, rule_set).map_err(ChainError::Header)?;
        if header.height == 0 {
            return Err(ChainError::Genesis);
        }
        let parent = self.entry(&header.prev).ok_or(ChainError::UnknownParent(header.prev))?;
        if parent.status == store::Status::Invalid {
            if self.vetoed.contains(&header.prev) || self.revert_vetoed.contains(&header.prev) {
                return Err(self.vetoed_error(header.height));
            }
            return Err(ChainError::InvalidParent(header.prev));
        }
        if header.height != parent.header.height + 1 {
            return Err(ChainError::Height { expected: parent.header.height + 1, actual: header.height });
        }
        let mtp = self.mtp(&header.prev);
        if header.time <= mtp {
            return Err(ChainError::Time { mtp, actual: header.time });
        }
        if header.time > now.saturating_add(MAX_FUTURE_DRIFT) {
            return Err(ChainError::FutureTime { time: header.time, now });
        }
        // §5.6 rule 3: the first block above a revert is not older than the release
        if let Some(floor) = self.reverts.time_floor(header.height) {
            if header.time < floor {
                return Err(ChainError::Time { mtp: floor.saturating_sub(1), actual: header.time });
            }
        }
        let expected = self.bits_after(&parent.header);
        if header.bits != expected {
            return Err(ChainError::Bits { expected, actual: header.bits });
        }
        if !valid_pow(header) {
            return Err(ChainError::Pow);
        }
        // §5.3: a chain not containing a held checkpoint is rejected — at the
        // checkpointed height itself, whatever path the block arrives by
        if let Some(expected) = self.checkpoints.get(header.height) {
            if header.id() != expected {
                return Err(ChainError::Checkpoint { height: header.height, expected });
            }
        }
        // §5.6 rule 1: `keep` is a compiled checkpoint, without key or sunset
        if let Some(keep) = self.reverts.keep_at(header.height) {
            if header.id() != keep {
                return Err(ChainError::Checkpoint { height: header.height, expected: keep });
            }
        }
        Ok(chainwork(parent.chainwork, header))
    }

    /// Expected `bits` of the block after `parent` (§1.5), re-anchored above
    /// each revert entry (§5.6). `parent` must be stored (the ancestors are
    /// read from the store); with an empty table it is plain ASERT.
    pub(super) fn bits_after(&self, parent: &Header) -> u32 {
        if self.reverts.is_empty() {
            return next_bits(self.genesis.anchor(), parent);
        }
        let parent_id = parent.id();
        let reanchors = self.reverts.reanchors();
        next_bits_reverted(self.genesis.anchor(), &reanchors, parent, &|h| {
            self.store.ancestor_at(&parent_id, h).and_then(|id| self.store.get(&id)).map(|e| e.header)
        })
    }

    /// The rule set a block at `height` must carry (§7.1, §5.6): the header
    /// `version & 0xffff` of a template.
    pub(super) fn rule_set_for(&self, height: u64) -> u16 {
        self.reverts.rule_set_at(height).unwrap_or(RULE_SET)
    }

    /// The earliest `time` a block at `height` may carry (§5.6 rule 3); 0 if none.
    pub(super) fn time_floor(&self, height: u64) -> u64 {
        self.reverts.time_floor(height).unwrap_or(0)
    }

    /// Record a header of valid proof of work whose rule set this node does
    /// not know: `upgrade-required` (§7.1). Anything else is ignored; the
    /// header is never stored and never relayed.
    pub(super) fn note_unknown_rule_set(&mut self, header: &Header, error: &ChainError) {
        let ChainError::Header(HeaderError::RuleSet(rule_set)) = error else { return };
        if *rule_set <= self.reverts.last_rule_set() || header.chain != self.rules.chain || header.height == 0 {
            return;
        }
        if decode_bits(header.bits).is_ok() && valid_pow(header) {
            self.upgrade_required = Some((header.height, *rule_set));
        }
    }

    /// Apply a block whose parent is the tip (steps 7–8), then extend.
    pub(super) fn extend(&mut self, header: &Header, txs: &[Decoded]) -> Result<(), ChainError> {
        let id = header.id();
        let locked = self.locked_at(header.height);
        let undo = match self.apply_on_tip(header, txs, &locked) {
            Ok(undo) => undo,
            // a settlement this node cannot compute is its own condition,
            // never the block's fault (chain-economy §2.5: it waits)
            Err(e @ apply::BlockError::Settlement { .. }) => return Err(ChainError::Block(e)),
            Err(e) => {
                self.invalidate(id);
                return Err(ChainError::Block(e));
            }
        };
        self.store.put_undo(&id, &undo).map_err(ChainError::Store)?;
        self.store.set_status(&id, store::Status::Valid).map_err(ChainError::Store)?;
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

    fn decoded(&self, id: &BlockId) -> Result<(Header, Vec<Decoded>), ChainError> {
        let header = self.entry(id).map(|e| e.header).ok_or(ChainError::Store("missing header".into()))?;
        let body = self.store.body(id).ok_or(ChainError::Store("missing body".into()))?;
        let txs = apply::decode_body(&self.rules, &header, &body).map_err(ChainError::Block)?;
        Ok((header, txs))
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
        let tip = self.tip_id();
        let tip_work = self.entry(&tip).map(|e| e.chainwork).unwrap_or(U256::ZERO);
        if !select::heavier(self.heaviest, tip_work) {
            return Ok(BlockVerdict::Known);
        }
        let mut first_error = None;
        for best in self.candidates() {
            let (fork_height, path) = self.path_from_main(&best)?;
            let tip_height = self.store.tip_height();
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
            let missing: Vec<BlockId> = path.iter().filter(|id| !self.store.has_body(id)).copied().collect();
            if !missing.is_empty() {
                return Ok(BlockVerdict::NeedBodies(missing));
            }
            return self.reorg(fork_height, &path);
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
            // every descendant of the failed block is invalid too
            for id in &path[applied + 1..] {
                self.invalidate(*id);
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
