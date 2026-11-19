//! Why a transaction or a block is invalid: the two error enums of the
//! validation order (chain.md §5.5 steps 6–8) and their messages. Separate
//! from the rules that raise them so the codes of the conformance corpus
//! (`super::super::corpus::verdict_code`) have one place to read.

use super::super::state::{Id, StateError};
use super::super::tx::DecodeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxError {
    Decode(DecodeError),
    TooLarge(usize),
    CoinbaseInBody,
    Signature,
    Nonce { expected: u64, actual: u64 },
    Fee { min: u64, actual: u64 },
    Insufficient { have: u64, need: u64 },
    Immature { spendable: u64, need: u64 },
    SelfLink,
    /// a staked cyberlink names a token other than the native coin (chain-economy §3.3)
    StakeToken,
    /// the card exists with the other valence: unlock to zero first (§3.3)
    Valence { card: i8, tx: i8 },
    /// kinds 6 and 7 need `card(from, p, q)` to exist (§3.4–§3.5)
    NoCard,
    /// an unlock before `since + MIN_LOCK` (§3.4)
    Locked { until: u64 },
    /// an unlock of 0 or of more than the position's stake (§3.4)
    UnlockAmount { stake: u64, amount: u64 },
    ClaimKey,
    Overflow,
}

impl std::fmt::Display for TxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxError::Decode(e) => write!(f, "{e}"),
            TxError::TooLarge(n) => write!(f, "{n} bytes exceeds MAX_TX_BYTES"),
            TxError::CoinbaseInBody => write!(f, "coinbase outside position 0"),
            TxError::Signature => write!(f, "signature does not verify"),
            TxError::Nonce { expected, actual } => write!(f, "nonce {actual}, expected {expected}"),
            TxError::Fee { min, actual } => write!(f, "fee {actual} below floor {min}"),
            TxError::Insufficient { have, need } => write!(f, "insufficient balance: have {have}, need {need}"),
            TxError::Immature { spendable, need } => write!(f, "coinbase immature: spendable {spendable}, need {need}"),
            TxError::SelfLink => write!(f, "cyberlink p == q"),
            TxError::StakeToken => write!(f, "stake must be the native coin"),
            TxError::Valence { card, tx } => write!(f, "the position holds valence {card}; unlock it to zero before locking {tx}"),
            TxError::NoCard => write!(f, "no position card(from, p, q)"),
            TxError::Locked { until } => write!(f, "stake locked until height {until}"),
            TxError::UnlockAmount { stake, amount } => write!(f, "unlock {amount} of a position holding {stake}"),
            TxError::ClaimKey => write!(f, "claim key does not derive the account id"),
            TxError::Overflow => write!(f, "arithmetic overflow"),
        }
    }
}

impl From<StateError> for TxError {
    fn from(e: StateError) -> Self {
        match e {
            StateError::Insufficient { have, need } => TxError::Insufficient { have, need },
            StateError::Overflow => TxError::Overflow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockError {
    Body(DecodeError),
    Empty,
    TooManyTxs(usize),
    TooLarge(usize),
    TxRoot,
    CoinbasePosition,
    CoinbaseHeight { expected: u64, actual: u64 },
    DuplicateTx(usize),
    CoinbaseAmount { expected: u64, actual: u64 },
    Tx { index: usize, error: TxError },
    StateRoot { expected: Id, actual: Id },
    Conservation { token: Id, sum: u64, supply: u64 },
    /// the settlement of `epoch` is due at this height and the node has no
    /// result for it (chain-economy §2.5: it waits, never guesses)
    Settlement { epoch: u64 },
    /// the header's sample does not commit to the ticket of its template on
    /// the epoch's game (chain.md §5.5 step 8; rewards.md §7): invalid, and
    /// a protocol error of the peer that sent it (chain-wire §5)
    Sample(String),
    /// Σ stake keys differs from the running stake total (§6.3)
    StakeTotal { cards: u128, total: u64 },
    Overflow,
}

impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockError::Body(e) => write!(f, "body: {e}"),
            BlockError::Empty => write!(f, "body has no coinbase"),
            BlockError::TooManyTxs(n) => write!(f, "{n} transactions exceed MAX_BLOCK_TXS"),
            BlockError::TooLarge(n) => write!(f, "{n} bytes exceed MAX_BLOCK_BYTES"),
            BlockError::TxRoot => write!(f, "tx_root mismatch"),
            BlockError::CoinbasePosition => write!(f, "tx[0] is not a coinbase"),
            BlockError::CoinbaseHeight { expected, actual } => write!(f, "coinbase height {actual}, block height {expected}"),
            BlockError::DuplicateTx(i) => write!(f, "tx {i} duplicates an earlier txid"),
            BlockError::CoinbaseAmount { expected, actual } => write!(f, "coinbase amount {actual}, expected {expected}"),
            BlockError::Tx { index, error } => write!(f, "tx {index}: {error}"),
            BlockError::StateRoot { .. } => write!(f, "state_root mismatch"),
            BlockError::Conservation { sum, supply, .. } => write!(f, "conservation: balances sum {sum}, supply {supply}"),
            BlockError::Settlement { epoch } => write!(f, "settlement of epoch {epoch} is not computed"),
            BlockError::Sample(s) => write!(f, "sample: {s}"),
            BlockError::StakeTotal { cards, total } => write!(f, "stake keys sum {cards}, running total {total}"),
            BlockError::Overflow => write!(f, "arithmetic overflow"),
        }
    }
}
