//! The conformance corpus writer (proposals/proven-present.md row S0): every
//! block a node is given, in arrival order, with its native verdict and `R`
//! (the bbg state root after the call), plus a generator of invalid blocks.
//!
//! Format: JSON Lines, `<dir>/blocks.jsonl`, one record per line, append-only,
//! flushed per record. Fields:
//!
//! - `height`   the block's header height
//! - `id`       hex of the block id
//! - `block`    hex of [`Block::encode`] (`header[200] ‖ body`, the `B` frame payload)
//! - `verdict`  [`verdict_code`], or `"genesis"` for record 0
//! - `root`     hex of `R` (the tip's `state_root`) after the call when the verdict
//!              accepts; `null` for a rejection
//! - `mutation` `null` for a block as received/mined, else the generator's name
//!
//! Record 0 is block 0 with verdict `"genesis"` and `R(G)`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::api::{Block, BlockVerdict, ChainApi, ChainError};
use super::apply::{BlockError, TxError};
use super::header::HeaderError;
use super::state::Id;

/// The corpus file inside the corpus directory.
pub const FILE: &str = "blocks.jsonl";

/// Longest nonce search the generator runs for one block.
const GRIND_LIMIT: u128 = 1 << 20;

// ── verdict codes ───────────────────────────────────────────────────────

/// Stable snake_case code of an `accept_block` result. Every `match` here is
/// exhaustive on purpose: a new variant must fail the build, not be mis-coded.
pub fn verdict_code(r: &Result<BlockVerdict, ChainError>) -> String {
    match r {
        Ok(BlockVerdict::Extended) => "extended".into(),
        Ok(BlockVerdict::Reorganized { .. }) => "reorganized".into(),
        Ok(BlockVerdict::Stored) => "stored".into(),
        Ok(BlockVerdict::NeedBodies(_)) => "need_bodies".into(),
        Ok(BlockVerdict::Known) => "known".into(),
        Err(e) => format!("reject:{}", chain_error_code(e)),
    }
}

fn chain_error_code(e: &ChainError) -> String {
    match e {
        ChainError::Header(h) => format!("header:{}", header_error_code(h)),
        ChainError::UnknownParent(_) => "unknown_parent".into(),
        ChainError::InvalidParent(_) => "invalid_parent".into(),
        ChainError::Height { .. } => "height".into(),
        ChainError::Time { .. } => "time".into(),
        ChainError::FutureTime { .. } => "future_time".into(),
        ChainError::Bits { .. } => "bits".into(),
        ChainError::Pow => "pow".into(),
        ChainError::RuleSetUnknown(_) => "rule_set_unknown".into(),
        ChainError::DeepFork { .. } => "deep_fork".into(),
        ChainError::Checkpoint { .. } => "checkpoint".into(),
        ChainError::CheckpointRejected(_) => "checkpoint_rejected".into(),
        ChainError::Block(b) => format!("block:{}", block_error_code(b)),
        ChainError::Rejected(_) => "rejected".into(),
        ChainError::Genesis => "genesis".into(),
        ChainError::Store(_) => "store".into(),
        ChainError::VerifyBudget => "verify_budget".into(),
        ChainError::Game(_) => "game".into(),
    }
}

fn header_error_code(e: &HeaderError) -> &'static str {
    match e {
        HeaderError::Length(_) => "length",
        HeaderError::RuleSet(_) => "rule_set",
        HeaderError::WrongChain => "wrong_chain",
        HeaderError::Bits(_) => "bits",
    }
}

fn block_error_code(e: &BlockError) -> String {
    match e {
        BlockError::Body(_) => "body".into(),
        BlockError::Empty => "empty".into(),
        BlockError::TooManyTxs(_) => "too_many_txs".into(),
        BlockError::TooLarge(_) => "too_large".into(),
        BlockError::TxRoot => "tx_root".into(),
        BlockError::CoinbasePosition => "coinbase_position".into(),
        BlockError::CoinbaseHeight { .. } => "coinbase_height".into(),
        BlockError::DuplicateTx(_) => "duplicate_tx".into(),
        BlockError::CoinbaseAmount { .. } => "coinbase_amount".into(),
        BlockError::Tx { error, .. } => format!("tx:{}", tx_error_code(error)),
        BlockError::StateRoot { .. } => "state_root".into(),
        BlockError::Conservation { .. } => "conservation".into(),
        BlockError::Settlement { .. } => "settlement".into(),
        BlockError::Sample(_) => "sample".into(),
        BlockError::StakeTotal { .. } => "stake_total".into(),
        BlockError::Overflow => "overflow".into(),
    }
}

fn tx_error_code(e: &TxError) -> &'static str {
    match e {
        TxError::Decode(_) => "decode",
        TxError::TooLarge(_) => "too_large",
        TxError::CoinbaseInBody => "coinbase_in_body",
        TxError::Signature => "signature",
        TxError::Nonce { .. } => "nonce",
        TxError::Fee { .. } => "fee",
        TxError::Insufficient { .. } => "insufficient",
        TxError::Immature { .. } => "immature",
        TxError::SelfLink => "self_link",
        TxError::StakeToken => "stake_token",
        TxError::Valence { .. } => "valence",
        TxError::NoCard => "no_card",
        TxError::Locked { .. } => "locked",
        TxError::UnlockAmount { .. } => "unlock_amount",
        TxError::ClaimKey => "claim_key",
        TxError::Overflow => "overflow",
    }
}

pub mod invalid;

pub use invalid::{Invalid, invalid_blocks, stale_base};

// ── the file ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Record {
    height: u64,
    id: String,
    block: String,
    verdict: String,
    root: Option<String>,
    mutation: Option<String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

/// Append-only writer of `<dir>/blocks.jsonl`.
pub struct CorpusWriter {
    file: File,
    path: PathBuf,
    records: usize,
}

impl CorpusWriter {
    /// Open (creating the directory and file if missing) for append.
    pub fn open(dir: &Path) -> io::Result<CorpusWriter> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        let records = match std::fs::read_to_string(&path) {
            Ok(text) => text.lines().filter(|l| !l.trim().is_empty()).count(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(CorpusWriter { file, path, records })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records in the file, including ones from earlier runs.
    pub fn len(&self) -> usize {
        self.records
    }

    pub fn is_empty(&self) -> bool {
        self.records == 0
    }

    fn write(&mut self, rec: &Record) -> io::Result<()> {
        let mut line = serde_json::to_string(rec).map_err(io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        self.records += 1;
        Ok(())
    }

    /// Record 0: block 0 with verdict `"genesis"` and `R(G)`. Does nothing
    /// when the file already has records.
    pub fn record_genesis(&mut self, chain: &impl ChainApi) -> io::Result<()> {
        if !self.is_empty() {
            return Ok(());
        }
        let g = chain.header_at(0).and_then(|h| chain.block(&h.id())).ok_or_else(|| io::Error::other("chain has no block 0"))?;
        self.write(&Record {
            height: 0,
            id: hex(&g.id()),
            block: hex(&g.encode()),
            verdict: "genesis".into(),
            root: Some(hex(&g.header.state_root)),
            mutation: None,
        })
    }

    /// One record: `root` is `R` after the call; it is written only when the
    /// result accepts.
    pub fn record(&mut self, block: &Block, result: &Result<BlockVerdict, ChainError>, root: &Id, mutation: Option<&str>) -> io::Result<()> {
        self.write(&Record {
            height: block.header.height,
            id: hex(&block.id()),
            block: hex(&block.encode()),
            verdict: verdict_code(result),
            root: result.is_ok().then(|| hex(root)),
            mutation: mutation.map(str::to_string),
        })
    }
}

/// `accept_block`, then the record. Writes record 0 first if the file is
/// empty. The outer error is the file's; the inner is the chain's verdict.
pub fn record_accept<C: ChainApi>(
    chain: &mut C,
    writer: &mut CorpusWriter,
    block: &Block,
    now: u64,
    mutation: Option<&str>,
) -> io::Result<Result<BlockVerdict, ChainError>> {
    writer.record_genesis(&*chain)?;
    let result = chain.accept_block(block, now);
    let root = chain.tip().header.state_root;
    writer.record(block, &result, &root, mutation)?;
    Ok(result)
}

// ── replay ──────────────────────────────────────────────────────────────

/// The first record whose replay differs from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayMismatch {
    /// 1-based line in `blocks.jsonl` (0: the file could not be read)
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for ReplayMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{FILE}:{}: {}", self.line, self.reason)
    }
}

impl std::error::Error for ReplayMismatch {}

/// Replay `<dir>/blocks.jsonl` on a fresh `chain`: record 0 must be the
/// chain's genesis and `R(G)`; every later block goes through `accept_block`
/// in file order and must reproduce the recorded verdict code and root.
/// Returns the number of records after genesis.
pub fn replay<C: ChainApi>(dir: &Path, chain: &mut C, now: u64) -> Result<usize, ReplayMismatch> {
    let text = std::fs::read_to_string(dir.join(FILE)).map_err(|e| ReplayMismatch { line: 0, reason: e.to_string() })?;
    let mut count = 0usize;
    let mut seen_genesis = false;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let bad = |reason: String| ReplayMismatch { line, reason };
        let rec: Record = serde_json::from_str(raw).map_err(|e| bad(format!("not a record: {e}")))?;
        let bytes = unhex(&rec.block).ok_or_else(|| bad("block is not hex".into()))?;
        let block = Block::decode(&bytes).map_err(|e| bad(format!("block does not decode: {e}")))?;
        if hex(&block.id()) != rec.id || block.header.height != rec.height {
            return Err(bad("id/height do not match the block bytes".into()));
        }
        if !seen_genesis {
            seen_genesis = true;
            let g = chain.header_at(0).and_then(|h| chain.block(&h.id())).ok_or_else(|| bad("chain has no block 0".into()))?;
            if rec.verdict != "genesis" || rec.height != 0 {
                return Err(bad(format!("record 0 must be the genesis, found {:?} at height {}", rec.verdict, rec.height)));
            }
            if g.encode() != bytes {
                return Err(bad(format!("genesis block differs: file {}, chain {}", rec.id, hex(&g.id()))));
            }
            let want = hex(&g.header.state_root);
            if rec.root.as_deref() != Some(want.as_str()) {
                return Err(bad(format!("R(G) is {want}, file has {:?}", rec.root)));
            }
            continue;
        }
        let result = chain.accept_block(&block, now);
        let code = verdict_code(&result);
        if code != rec.verdict {
            return Err(bad(format!("height {}: verdict {code}, file has {}", rec.height, rec.verdict)));
        }
        let root = result.is_ok().then(|| hex(&chain.tip().header.state_root));
        if root != rec.root {
            return Err(bad(format!("height {}: R is {root:?}, file has {:?}", rec.height, rec.root)));
        }
        count += 1;
    }
    if !seen_genesis {
        return Err(ReplayMismatch { line: 0, reason: "empty corpus".into() });
    }
    Ok(count)
}
