//! Steps 1–5 of the validation order (chain.md §5.5): the header alone —
//! syntax, the parent, the median time past and the future drift, the
//! `bits` ASERT expects, the rule set and proof of work — with no state
//! read beyond the store's headers.

use super::api::ChainError;
use super::daa::{median_time_past, next_bits, next_bits_reverted};
use super::engine::Chain;
use super::header::{BlockId, Header, HeaderError, decode_bits};
use super::params::{MAX_FUTURE_DRIFT, MTP_WINDOW, RULE_SET};
use super::pow::{chainwork, valid_pow};
use super::store;
use super::u256::U256;

impl Chain {
    /// Median time past of the parent and its ancestors (§1.4).
    pub(super) fn mtp(&self, parent: &BlockId) -> u64 {
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
}
