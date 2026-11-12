//! The chain API: the surface the network layer (`chain/net/`), the miner
//! (`chain/mine/`) and the HTTP routes code against. Stable; additions are
//! fine, changes to existing signatures are a coordinated change.
//!
//! Clock discipline: nothing here reads the wall clock. Every entry point
//! that applies the future-drift rule takes `now` (unix seconds) so tests
//! and the net layer's hold-and-retry are deterministic.

use super::apply::{BlockError, TxError};
use super::genesis::Genesis;
use super::header::{BlockId, Header, HeaderError};
use super::params::HEADER_BYTES;
use super::revert::{RevertEntry, Reverted};
use super::revert_statement::{RevertStatement, StatementError};
use super::select::{Checkpoint, CheckpointError};
use super::state::Id;
use super::tx::Tx;
use super::u256::U256;

/// The canonical tip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    pub height: u64,
    pub id: BlockId,
    pub chainwork: U256,
    pub header: Header,
}

/// A header with its body (`count ‖ tx…`, §1.6). Block 0 has an empty body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub body: Vec<u8>,
}

impl Block {
    pub fn id(&self) -> BlockId {
        self.header.id()
    }

    /// `header[200] ‖ body` — the `B` frame payload of chain-wire §3.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.header.encode().to_vec();
        out.extend_from_slice(&self.body);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Block, HeaderError> {
        let header = Header::decode_raw(b.get(..HEADER_BYTES).ok_or(HeaderError::Length(b.len()))?)?;
        Ok(Block { header, body: b[HEADER_BYTES..].to_vec() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderVerdict {
    /// passed steps 1–5, stored; relay it
    Accepted,
    /// already in the store
    Known,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockVerdict {
    /// the block extended the tip
    Extended,
    /// the tip moved to this block's chain, undoing `depth` blocks from `old_tip`
    Reorganized { old_tip: BlockId, depth: u64 },
    /// valid as far as checked (steps 1–6) and stored; not heavier than the
    /// tip, so steps 7–8 run if it ever becomes the heaviest chain
    Stored,
    /// heavier than the tip, but these ancestors have no body yet; fetch
    /// them (chain-wire §3.1) and resubmit any one of the chain
    NeedBodies(Vec<BlockId>),
    /// already applied
    Known,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// step 1
    Header(HeaderError),
    /// step 2: `prev` not in the store (the net layer's orphan pool takes it)
    UnknownParent(BlockId),
    /// step 2: the parent is known and invalid
    InvalidParent(BlockId),
    /// step 2
    Height { expected: u64, actual: u64 },
    /// step 3: `time <= MTP(prev)`
    Time { mtp: u64, actual: u64 },
    /// step 3: ahead of the clock; hold and re-evaluate, never score
    FutureTime { time: u64, now: u64 },
    /// step 4
    Bits { expected: u32, actual: u32 },
    /// step 5
    Pow,
    /// §7.1: the activation table ends before this height
    RuleSetUnknown(u64),
    /// §5.2: the fork point is at or below `tip − FINALITY_DEPTH`
    DeepFork { fork_height: u64, tip_height: u64 },
    /// §5.3: the chain excludes a held checkpoint
    Checkpoint { height: u64, expected: BlockId },
    /// §5.3
    CheckpointRejected(CheckpointError),
    /// steps 6–8; the header is now marked invalid with every descendant
    Block(BlockError),
    /// an id in the rejected set (§5.4)
    Rejected(BlockId),
    /// genesis block mismatch on a block claiming height 0
    Genesis,
    /// the store
    Store(String),
    /// step 8: the node's `V_BUDGET` of sample verifications for this block
    /// interval is spent; the block waits in the store and is retried
    /// (chain.md §5.5 amended) — never the block's fault
    VerifyBudget,
    /// step 8: the game the block samples cannot be built on this node
    /// yet (a boundary snapshot it does not hold) — never the block's fault
    Game(String),
    /// the node's store of unverified headers above the tip is at
    /// `UNVERIFIED_CAP`; this header is not admitted (R18-5, the forged-
    /// header flood) and is re-offered when the set drains — never the
    /// block's fault, never scored
    Unverified,
}

impl ChainError {
    /// Is the block (and every descendant) invalid for it, or is it this
    /// node's own condition — a settlement or a game not computed, a spent
    /// verification budget — after which the same block may be accepted?
    pub fn is_blocks_fault(&self) -> bool {
        !matches!(self, ChainError::VerifyBudget | ChainError::Game(_) | ChainError::Unverified | ChainError::Block(BlockError::Settlement { .. }))
    }
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainError::Header(e) => write!(f, "header: {e}"),
            ChainError::UnknownParent(_) => write!(f, "unknown parent"),
            ChainError::InvalidParent(_) => write!(f, "parent is invalid"),
            ChainError::Height { expected, actual } => write!(f, "height {actual}, expected {expected}"),
            ChainError::Time { mtp, actual } => write!(f, "time {actual} is not above the median time past {mtp}"),
            ChainError::FutureTime { time, now } => write!(f, "time {time} is more than the drift ahead of {now}"),
            ChainError::Bits { expected, actual } => write!(f, "bits {actual:#010x}, ASERT expects {expected:#010x}"),
            ChainError::Pow => write!(f, "block id does not meet the target"),
            ChainError::RuleSetUnknown(h) => write!(f, "no rule set is known for height {h}; this release stops here"),
            ChainError::DeepFork { fork_height, tip_height } => write!(f, "fork at {fork_height} is beyond MAX_REORG from the tip {tip_height}"),
            ChainError::Checkpoint { height, .. } => write!(f, "chain excludes the checkpoint at {height}"),
            ChainError::CheckpointRejected(e) => write!(f, "checkpoint: {e}"),
            ChainError::Block(e) => write!(f, "block: {e}"),
            ChainError::Rejected(_) => write!(f, "previously rejected block"),
            ChainError::Genesis => write!(f, "genesis block mismatch"),
            ChainError::Store(e) => write!(f, "store: {e}"),
            ChainError::VerifyBudget => write!(f, "the sample verification budget of this interval is spent; retry"),
            ChainError::Game(s) => write!(f, "the epoch's game is not available: {s}"),
            ChainError::Unverified => write!(f, "the unverified-header store is full; the header is re-offered later"),
        }
    }
}

impl std::error::Error for ChainError {}

/// What a mempool learns from a validated transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxInfo {
    pub txid: Id,
    pub tx: Tx,
    pub size: usize,
    pub fee: u64,
    /// the account whose nonce the tx consumes (the claim's key-derived neuron)
    pub account: Id,
    pub nonce: u64,
}

/// A mining template (chain-wire §7 `/template`): the header with zero
/// nonce and the ASERT target, plus the body it commits to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub header: Header,
    pub target: U256,
    pub body: Vec<u8>,
    pub txids: Vec<Id>,
    pub fees: u64,
    /// the game the block samples (rewards.md §7): the miner's input
    pub game: std::sync::Arc<super::ticket::EpochGame>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Template {
    /// The `GET /template` answer of chain-wire §7, exactly.
    pub fn json(&self, chain_name: &str, expires: u64) -> String {
        let g = &self.game;
        format!(
            "{{\"chain\":{},\"height\":{},\"prev\":\"{}\",\"header\":\"{}\",\"bits\":\"{:08x}\",\"target\":\"{}\",\"body\":\"{}\",\"expires\":{},\"game\":{{\"epoch\":{},\"beacon\":\"{}\",\"cluster\":\"{}\",\"players\":{},\"axons\":{},\"w_ticket\":{}}}}}",
            serde_json::to_string(chain_name).unwrap_or_else(|_| "\"\"".into()),
            self.header.height,
            hex(&self.header.prev),
            hex(&self.header.encode()),
            self.header.bits,
            hex(&self.target.to_le_bytes()),
            hex(&self.body),
            expires,
            g.epoch,
            hex(&g.beacon),
            hex(&g.cluster),
            g.players.len(),
            g.base.len(),
            g.width
        )
    }
}

impl Tip {
    /// The `GET /tip` answer of chain-wire §7, exactly.
    pub fn json(&self) -> String {
        format!("{{\"height\":{},\"id\":\"{}\",\"chainwork\":\"{}\"}}", self.height, hex(&self.id), self.chainwork.to_decimal())
    }
}

/// Finalized block stream for the shadow indexers (§7.3): emitted once a
/// block is `FINALITY_DEPTH` deep, never reorged — except by `Reverted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Block { header: Header, body: Vec<u8>, state_root: Id },
    /// A revert by release (§5.6) applied at open: the chain returned to
    /// `keep` at `to_height`; blocks above it that were delivered earlier no
    /// longer exist. Delivered before any finalized block above `to_height`.
    Reverted { to_height: u64, keep: BlockId, vetoed: Vec<BlockId>, rehearsal: bool },
}

/// `/status` fields the chain owns (chain-wire §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub chain: Id,
    pub name: String,
    pub rule_set: u16,
    pub height: u64,
    pub tip: BlockId,
    pub chainwork: U256,
    pub bits: u32,
    pub next_bits: u32,
    pub genesis_block: BlockId,
    pub genesis_time: u64,
    pub state_root: Id,
    pub supply: u64,
    pub conserved: bool,
    pub subsidy: u64,
    pub neurons: u64,
    pub particles: u64,
    pub axons: u64,
    pub checkpoint: Option<u64>,
    pub finalized: u64,
    /// the effective revert table (§5.6), compiled plus rehearsal entries
    pub reverts: Vec<RevertEntry>,
    /// the last revert this node applied (§5.6)
    pub reverted: Option<Reverted>,
    /// a block of valid proof of work named a rule set this node does not
    /// know: `(height, rule_set)` last seen (§7.1)
    pub upgrade_required: Option<(u64, u16)>,
}

/// The chain as the other layers see it.
pub trait ChainApi {
    fn chain_id(&self) -> Id;
    fn genesis(&self) -> &Genesis;
    fn tip(&self) -> Tip;
    fn status(&self) -> Status;

    // ── reads ───────────────────────────────────────────────────────
    fn header(&self, id: &BlockId) -> Option<Header>;
    /// main-chain header at a height
    fn header_at(&self, height: u64) -> Option<Header>;
    /// main-chain headers `[from, from + count)`, ascending, truncated at the tip
    fn headers(&self, from: u64, count: usize) -> Vec<Header>;
    fn block(&self, id: &BlockId) -> Option<Block>;
    /// main-chain blocks `[from, from + count)`, ascending
    fn blocks(&self, from: u64, count: usize) -> Vec<Block>;
    fn chainwork(&self, id: &BlockId) -> Option<U256>;
    /// main-chain ids for a get-headers locator (chain-wire §3)
    fn locator(&self) -> Vec<BlockId>;
    /// the main-chain id at `height` on the branch ending at `id`
    fn ancestor_at(&self, id: &BlockId, height: u64) -> Option<BlockId>;
    fn balance(&self, owner: &Id, token: &Id) -> u64;
    fn nonce(&self, neuron: &Id) -> u64;
    fn supply(&self, token: &Id) -> u64;
    fn finalized_height(&self) -> u64;

    // ── writes ──────────────────────────────────────────────────────
    /// steps 1–5; stores a valid header
    fn accept_header(&mut self, header: &Header, now: u64) -> Result<HeaderVerdict, ChainError>;
    /// steps 1–8 and fork choice; the header is accepted first
    fn accept_block(&mut self, block: &Block, now: u64) -> Result<BlockVerdict, ChainError>;
    /// §5.3; `Ok(true)` when new
    fn accept_checkpoint(&mut self, cp: &Checkpoint) -> Result<bool, ChainError>;
    fn checkpoints(&self) -> Vec<(u64, BlockId)>;
    /// §5.6: hold a revert statement of the owner key and apply it by the
    /// open-time rollback, at any depth; `Ok(true)` when it is new,
    /// `Ok(false)` when it changes nothing (duplicate, counter not above
    /// the highest held). The transactions of the retracted blocks are dropped.
    fn accept_revert_statement(&mut self, st: &RevertStatement) -> Result<bool, StatementError>;
    /// the held revert statements, ascending by counter
    fn revert_statements(&self) -> Vec<RevertStatement>;
    /// mempool admission (chain-wire §4): decode, signature, nonce within
    /// `NONCE_WINDOW` given `pending` (the mempool's txs of the same
    /// account, applied first in nonce order), fee floor, balances and
    /// maturity on the tip state. Never mutates the chain.
    fn validate_tx(&mut self, tx_bytes: &[u8], pending: &[Tx]) -> Result<TxInfo, TxError>;
    /// a template on the tip for a miner paying `to`, at `time` (raised to
    /// MTP + 1 if needed), from candidate transactions already admitted
    /// (chain-wire §4: fee per byte descending, nonce order kept, budgets)
    fn header_template(&mut self, to: Id, time: u64, mempool: &[Vec<u8>]) -> Result<Template, ChainError>;
    /// drain the finalized block events
    fn take_events(&mut self) -> Vec<Event>;
}
