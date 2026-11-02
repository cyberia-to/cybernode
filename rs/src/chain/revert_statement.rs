//! The revert statement (chain.md §5.6): the owner's signed instruction to
//! return to a block, the way a release does, without a release.
//!
//! A statement is `version ‖ chain ‖ height ‖ keep ‖ counter ‖ anchor_time ‖
//! sig`, signed by the genesis `checkpoint_key` under its own domain tag
//! (`soft3/chain/revert/v1\0`) and bound by no sunset. A node holds the
//! statements it accepted as a [`StatementSet`] ordered by counter; the set
//! replays into table entries ([`RevertEntry`] with an empty `vetoed` list,
//! `counter > 0`) that the engine enforces like the compiled ones.
//!
//! Pure data and functions plus the small append-only file of accepted
//! statements; the engine (`engine.rs`) applies them.

use std::io::Write;
use std::path::Path;

use super::header::{hash32, BlockId};
use super::params::REVERTS;
use super::revert::{compiled_max_rule_set, RevertEntry};
use super::select::{CheckpointError, Checkpoints};
use super::sigs::{neuron_of, revert_statement};
use super::tx::SIG_BYTES;

/// The statement version this release speaks.
pub const VERSION: u8 = 1;

/// `version[1] ‖ chain[32] ‖ height[8] ‖ keep[32] ‖ counter[8] ‖ anchor_time[8] ‖ sig[102]`.
pub const STATEMENT_BYTES: usize = 1 + 32 + 8 + 32 + 8 + 8 + SIG_BYTES;

/// The accepted statements under the chain's home, encoded back to back.
pub const FILE: &str = "revert-statements";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertStatement {
    pub version: u8,
    pub chain: [u8; 32],
    pub height: u64,
    pub keep: BlockId,
    pub counter: u64,
    pub anchor_time: u64,
    pub sig: Vec<u8>,
}

impl RevertStatement {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(STATEMENT_BYTES);
        out.push(self.version);
        out.extend_from_slice(&self.chain);
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.keep);
        out.extend_from_slice(&self.counter.to_le_bytes());
        out.extend_from_slice(&self.anchor_time.to_le_bytes());
        out.extend_from_slice(&self.sig);
        out
    }

    /// Exactly [`STATEMENT_BYTES`]; the version is judged by [`RevertStatement::check`].
    pub fn decode(b: &[u8]) -> Option<RevertStatement> {
        if b.len() != STATEMENT_BYTES {
            return None;
        }
        Some(RevertStatement {
            version: b[0],
            chain: b[1..33].try_into().ok()?,
            height: u64::from_le_bytes(b[33..41].try_into().ok()?),
            keep: b[41..73].try_into().ok()?,
            counter: u64::from_le_bytes(b[73..81].try_into().ok()?),
            anchor_time: u64::from_le_bytes(b[81..89].try_into().ok()?),
            sig: b[89..].to_vec(),
        })
    }

    /// The statement the signature covers (own domain tag).
    pub fn message(&self) -> [u8; 32] {
        revert_statement(self.version, &self.chain, self.height, &self.keep, self.counter, self.anchor_time)
    }

    /// `hemera(encode())`: the deterministic tie-break between two
    /// statements of one counter (the smaller wins).
    pub fn digest(&self) -> [u8; 32] {
        hash32(&self.encode())
    }

    /// Sign with the owner key (the signer box only; the key is never stored).
    pub fn sign(chain: &[u8; 32], height: u64, keep: BlockId, counter: u64, anchor_time: u64, key: &mudra::SigningKey) -> Result<RevertStatement, mudra::Error> {
        let pubkey = mudra::cosmos::compressed(key.verifying_key());
        let st = revert_statement(VERSION, chain, height, &keep, counter, anchor_time);
        let sig = mudra::neuron::sign(key, neuron_of(&pubkey), st)?;
        Ok(RevertStatement { version: VERSION, chain: *chain, height, keep, counter, anchor_time, sig })
    }

    /// `compiled max rule set + counter`, `None` past `u16`.
    pub fn rule_set(&self) -> Option<u16> {
        u16::try_from(self.counter).ok().and_then(|c| compiled_max_rule_set().checked_add(c))
    }

    /// The table entry the statement stands for: empty `vetoed`, the time
    /// floor and the difficulty anchor do the vetoing (§5.6).
    pub fn entry(&self) -> RevertEntry {
        RevertEntry {
            height: self.height,
            keep: self.keep,
            vetoed: Vec::new(),
            anchor_time: self.anchor_time,
            rule_set: self.rule_set().unwrap_or(u16::MAX),
            rehearsal: false,
            counter: self.counter,
        }
    }

    /// Everything that needs no store: version, chain, the signature under
    /// the checkpoint key, a counter and a height the table can carry.
    pub fn check(&self, chain: &[u8; 32], checkpoints: &Checkpoints) -> Result<(), StatementError> {
        if !checkpoints.enabled() {
            return Err(StatementError::Disabled);
        }
        if self.version != VERSION {
            return Err(StatementError::Version(self.version));
        }
        if self.chain != *chain {
            return Err(StatementError::WrongChain);
        }
        checkpoints.verify_statement(self.message(), &self.sig).map_err(|e| match e {
            CheckpointError::Disabled => StatementError::Disabled,
            _ => StatementError::Signature,
        })?;
        if self.height == 0 {
            return Err(StatementError::Height(0));
        }
        if self.counter == 0 || self.rule_set().is_none() {
            return Err(StatementError::Counter(self.counter));
        }
        if REVERTS.iter().any(|r| self.height <= r.height) {
            return Err(StatementError::Height(self.height));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementError {
    /// no checkpoint key (all-zero) or `--no-checkpoints`
    Disabled,
    Malformed,
    Version(u8),
    WrongChain,
    Signature,
    /// zero, or too large for `compiled max + counter` to be a rule set
    Counter(u64),
    /// zero, or not above the highest compiled revert
    Height(u64),
    /// `--revert` rehearsal entries and statements must not mix
    Rehearsal,
    /// the table with this statement fails its consistency checks
    Inconsistent(String),
    Store(String),
}

impl StatementError {
    /// chain-wire §5 score for the peer that sent it: only a statement that
    /// is not the owner's, or not a statement, is an offence.
    pub fn score(&self) -> u32 {
        match self {
            StatementError::Signature | StatementError::Malformed => 10,
            _ => 0,
        }
    }
}

impl std::fmt::Display for StatementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatementError::Disabled => write!(f, "revert statements are disabled (no checkpoint key, or --no-checkpoints)"),
            StatementError::Malformed => write!(f, "revert statement is not {STATEMENT_BYTES} bytes"),
            StatementError::Version(v) => write!(f, "revert statement version {v}, this release speaks {VERSION}"),
            StatementError::WrongChain => write!(f, "revert statement is for another chain"),
            StatementError::Signature => write!(f, "revert statement signature does not verify under the checkpoint key"),
            StatementError::Counter(c) => write!(f, "revert statement counter {c} is not in 1..={}", u16::MAX - compiled_max_rule_set()),
            StatementError::Height(h) => write!(f, "revert statement height {h} is not above genesis and the compiled reverts"),
            StatementError::Rehearsal => write!(f, "--revert rehearsal entries and revert statements must not mix: a drill never takes an authority's word"),
            StatementError::Inconsistent(e) => write!(f, "revert statement is inconsistent: {e}"),
            StatementError::Store(e) => write!(f, "{FILE}: {e}"),
        }
    }
}

impl std::error::Error for StatementError {}

/// The statements a node holds, ascending by counter, one per counter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatementSet {
    held: Vec<RevertStatement>,
}

impl StatementSet {
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    pub fn statements(&self) -> &[RevertStatement] {
        &self.held
    }

    /// The set with `st` taken in, or `None` when `st` changes nothing: only
    /// a counter above the highest held applies; the same counter with other
    /// content replaces the held statement only when its digest is smaller
    /// (every node then ends on the same one); a duplicate or a lower
    /// counter is ignored. `st` is assumed to have passed
    /// [`RevertStatement::check`].
    pub fn offer(&self, st: &RevertStatement) -> Option<StatementSet> {
        let mut held = self.held.clone();
        if let Some(top) = held.last() {
            if st.counter < top.counter {
                return None;
            }
            if st.counter == top.counter {
                if st == top || st.digest() >= top.digest() {
                    return None;
                }
                held.pop();
            }
        }
        held.push(st.clone());
        Some(StatementSet { held })
    }

    /// The effective statement entries: the statements replayed in counter
    /// order, a statement at `H2` dropping the earlier entries at heights
    /// `>= H2` before it is appended (§5.6). Ascending by height.
    pub fn entries(&self) -> Vec<RevertEntry> {
        let mut out: Vec<RevertEntry> = Vec::new();
        for st in &self.held {
            out.retain(|e| e.height < st.height);
            out.push(st.entry());
        }
        out
    }
}

/// The statements in the file at `path` (absent or empty: none), ascending
/// by counter; a trailing partial record is ignored.
pub fn load(path: &Path) -> Vec<RevertStatement> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let mut out: Vec<RevertStatement> = bytes.chunks_exact(STATEMENT_BYTES).filter_map(RevertStatement::decode).collect();
    out.sort_by_key(|s| s.counter);
    out
}

/// Append a statement to the file at `path` (durable before it returns).
pub fn append(path: &Path, st: &RevertStatement) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(&st.encode())?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::select::Checkpoint;

    fn key(i: u8) -> mudra::SigningKey {
        mudra::SigningKey::from_bytes((&[i; 32]).into()).unwrap()
    }

    fn statement(k: &mudra::SigningKey, height: u64, keep: u8, counter: u64) -> RevertStatement {
        RevertStatement::sign(&[0x44; 32], height, [keep; 32], counter, 5_000, k).unwrap()
    }

    #[test]
    fn it_round_trips_at_a_fixed_size() {
        let st = statement(&key(7), 10, 1, 3);
        assert_eq!(st.encode().len(), STATEMENT_BYTES);
        assert_eq!(STATEMENT_BYTES, 191);
        assert_eq!(RevertStatement::decode(&st.encode()), Some(st.clone()));
        assert_eq!(RevertStatement::decode(&st.encode()[1..]), None);
        let mut long = st.encode();
        long.push(0);
        assert_eq!(RevertStatement::decode(&long), None);
    }

    #[test]
    fn only_the_checkpoint_key_signs_and_only_under_its_own_tag() {
        let (owner, other) = (key(7), key(8));
        let cps = Checkpoints::new(mudra::cosmos::compressed(owner.verifying_key()), true);
        let chain = [0x44; 32];
        assert_eq!(statement(&owner, 10, 1, 1).check(&chain, &cps), Ok(()));
        assert_eq!(statement(&other, 10, 1, 1).check(&chain, &cps), Err(StatementError::Signature));
        // the fields are covered: a changed height does not verify
        let mut moved = statement(&owner, 10, 1, 1);
        moved.height = 11;
        assert_eq!(moved.check(&chain, &cps), Err(StatementError::Signature));
        assert_eq!(statement(&owner, 10, 1, 1).check(&[0x45; 32], &cps), Err(StatementError::WrongChain));
        // a checkpoint signature over the same chain, height and block is not a revert
        let cp = Checkpoint::sign(&chain, 120, [1; 32], &owner).unwrap();
        let as_revert = RevertStatement { version: VERSION, chain, height: 120, keep: [1; 32], counter: 1, anchor_time: 0, sig: cp.sig.clone() };
        assert_eq!(as_revert.check(&chain, &cps), Err(StatementError::Signature));
        // and a revert signature is not a checkpoint
        let st = statement(&owner, 120, 1, 1);
        let as_checkpoint = Checkpoint { height: 120, block_id: [1; 32], sig: st.sig.clone() };
        let mut held = Checkpoints::new(mudra::cosmos::compressed(owner.verifying_key()), true);
        assert_eq!(held.accept(&chain, &as_checkpoint), Err(CheckpointError::Signature));
        // disabled: no key
        assert_eq!(statement(&owner, 10, 1, 1).check(&chain, &Checkpoints::new([0; 33], true)), Err(StatementError::Disabled));
        // counter and height bounds
        assert_eq!(statement(&owner, 10, 1, 0).check(&chain, &cps), Err(StatementError::Counter(0)));
        assert_eq!(statement(&owner, 10, 1, 70_000).check(&chain, &cps), Err(StatementError::Counter(70_000)));
        assert_eq!(statement(&owner, 0, 1, 1).check(&chain, &cps), Err(StatementError::Height(0)));
    }

    #[test]
    fn the_rule_set_is_the_compiled_max_plus_the_counter() {
        let st = statement(&key(7), 10, 1, 3);
        assert_eq!(st.rule_set(), Some(compiled_max_rule_set() + 3));
        let e = st.entry();
        assert!(e.vetoed.is_empty() && !e.rehearsal && e.counter == 3 && e.rule_set == compiled_max_rule_set() + 3);
    }

    #[test]
    fn a_higher_counter_applies_and_a_lower_or_equal_one_does_not() {
        let k = key(7);
        let set = StatementSet::default().offer(&statement(&k, 10, 1, 2)).unwrap();
        assert_eq!(set.offer(&statement(&k, 10, 1, 2)), None, "a duplicate");
        assert_eq!(set.offer(&statement(&k, 5, 9, 1)), None, "a lower counter");
        let up = set.offer(&statement(&k, 12, 3, 3)).unwrap();
        assert_eq!(up.statements().len(), 2);
        // equal counter, other content: the smaller digest wins whichever came first
        let (a, b) = (statement(&k, 10, 1, 4), statement(&k, 10, 2, 4));
        let (small, big) = if a.digest() < b.digest() { (a, b) } else { (b, a) };
        let held_big = StatementSet::default().offer(&big).unwrap();
        assert_eq!(held_big.offer(&small).unwrap().statements(), &[small.clone()]);
        let held_small = StatementSet::default().offer(&small).unwrap();
        assert_eq!(held_small.offer(&big), None);
    }

    #[test]
    fn a_later_statement_at_a_lower_height_drops_the_earlier_entries() {
        let k = key(7);
        let set = StatementSet::default().offer(&statement(&k, 20, 1, 1)).unwrap().offer(&statement(&k, 30, 2, 2)).unwrap();
        assert_eq!(set.entries().iter().map(|e| e.height).collect::<Vec<_>>(), vec![20, 30]);
        let set = set.offer(&statement(&k, 25, 3, 3)).unwrap();
        assert_eq!(set.entries().iter().map(|e| (e.height, e.counter)).collect::<Vec<_>>(), vec![(20, 1), (25, 3)], "the entry at 30 is dropped");
        let set = set.offer(&statement(&k, 10, 4, 4)).unwrap();
        assert_eq!(set.entries().iter().map(|e| (e.height, e.counter)).collect::<Vec<_>>(), vec![(10, 4)]);
    }

    #[test]
    fn the_file_keeps_what_was_appended_in_counter_order() {
        let k = key(7);
        let mut dir = std::env::temp_dir();
        dir.push(format!("soft3-revert-statements-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        assert!(load(&path).is_empty());
        let (a, b) = (statement(&k, 10, 1, 2), statement(&k, 8, 2, 1));
        append(&path, &a).unwrap();
        append(&path, &b).unwrap();
        assert_eq!(load(&path), vec![b, a]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
