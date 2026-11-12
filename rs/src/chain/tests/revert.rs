//! Revert by release (chain.md §5.6) on the engine: the table by rule on two
//! nodes, the old-release node, the consistency refusals, the ASERT
//! re-anchor, and the timing receipt. The compiled table ships empty, so
//! every scenario uses rehearsal entries (`OpenOptions::reverts`).

use super::*;
use crate::chain::api::{BlockVerdict, ChainError, Event};
use crate::chain::daa::{self, Anchor, ReAnchor};
use crate::chain::header::{HeaderError, decode_bits};
use crate::chain::params::MAX_REORG;
use crate::chain::revert::{RevertEntry, Reverted};
use crate::chain::revert_statement::{RevertStatement, StatementError};
use crate::chain::select::{Checkpoint, CheckpointError};
use crate::chain::state::StateBackend;
use crate::chain::store::Status;
use crate::chain::u256::U256;

/// The old branch's length and the height the chain returns to.
const N: usize = 10;
const H: u64 = 6;

/// Mine on the tip one second after the parent: the chain runs ahead of the
/// ASERT schedule, so the targets fall below the limit and a wrong anchor
/// shows in `bits` instead of being hidden by the clamp.
fn mine_fast(chain: &mut Chain, to: Id, txs: &[Vec<u8>]) -> Block {
    let t = chain.tip().header.time + 1;
    mine(chain, to, t, txs)
}

/// The old branch: `N` blocks, a transfer in the block at `H + 1`.
fn grow_old(net: &Net, a: &mut Chain) -> Vec<Block> {
    let (m0, m1) = (net.neurons[1], net.neurons[2]);
    (1..=N)
        .map(|h| {
            let txs = if h as u64 == H + 1 { vec![net.transfer(0, 0, m1, 10, 224)] } else { Vec::new() };
            mine_fast(a, m0, &txs)
        })
        .collect()
}

/// The entry: keep the block at `H`, veto the old block at `H + 1`.
fn entry_for(blocks: &[Block], anchor_time: u64) -> RevertEntry {
    RevertEntry {
        height: H,
        keep: blocks[H as usize - 1].id(),
        vetoed: vec![blocks[H as usize].id()],
        anchor_time,
        rule_set: 0,
        rehearsal: true,
        counter: 0,
    }
}

fn same_tip(a: &Chain, b: &Chain) {
    assert_eq!(a.tip(), b.tip());
    assert_eq!(a.status().state_root, b.status().state_root);
    assert_eq!(a.status().supply, b.status().supply);
    assert!(a.status().conserved && b.status().conserved);
}

#[test]
fn an_empty_table_changes_nothing() {
    let net = Net::new(3);
    let (m0, m1) = (net.neurons[1], net.neurons[2]);
    let mut a = net.open("a");
    let mut b = net.open("b");
    let shared = vec![mine(&mut a, m0, 0, &[net.transfer(0, 0, m1, 10, 224)]), mine(&mut a, m0, 0, &[])];
    assert_eq!(feed(&mut b, &shared), vec![BlockVerdict::Extended, BlockVerdict::Extended]);
    mine_n(&mut a, m0, 2);
    let fork = mine_n(&mut b, m1, 4);
    assert!(matches!(feed(&mut a, &fork)[3], BlockVerdict::Reorganized { depth: 2, .. } | BlockVerdict::Extended));
    same_tip(&a, &b);
    // nothing of the machinery shows
    let s = a.status();
    assert!(s.reverts.is_empty() && s.reverted.is_none() && s.upgrade_required.is_none());
    assert_eq!(s.rule_set, 1);
    assert_eq!(s.next_bits, daa::next_bits(a.genesis().anchor(), &a.tip().header));
    assert!(a.reverts().is_empty() && a.reverts().activations_line() == "1@0");
    assert!(a.take_events().is_empty());
    // a restart with no entry replays as before and queues nothing
    let tip = a.tip();
    drop(a);
    let mut a2 = net.reopen(0);
    assert_eq!(a2.tip(), tip);
    assert!(a2.take_events().is_empty() && a2.take_retracted_txs().is_empty());
}

#[test]
fn two_nodes_revert_by_rule_and_end_on_one_tip_above_h() {
    let net = Net::new(3);
    let (m0, m1) = (net.neurons[1], net.neurons[2]);
    let mut a = net.open("a"); // store 0
    let blocks = grow_old(&net, &mut a);
    let mut b = net.open("b"); // store 1
    assert!(feed(&mut b, &blocks).iter().all(|v| *v == BlockVerdict::Extended));
    // a descendant of the old branch that no store has seen
    let mut c = net.open("c"); // store 2
    feed(&mut c, &blocks);
    let orphan = mine_fast(&mut c, m0, &[]);
    assert_eq!(orphan.header.height, N as u64 + 1);
    same_tip(&a, &b);

    // the release: back to the block at H, the old H + 1 vetoed, the
    // difficulty anchored 300 s after H's time
    let anchor_time = blocks[H as usize - 1].header.time + 300;
    let entry = entry_for(&blocks, anchor_time);
    let old7 = blocks[H as usize].id();
    drop(a);
    drop(b);
    let mut a = net.reopen_with(0, vec![entry.clone()]).unwrap();
    let mut b = net.reopen_with(1, vec![entry.clone()]).unwrap();
    for chain in [&a, &b] {
        assert_eq!(chain.tip().height, H);
        assert_eq!(chain.tip().id, entry.keep);
        assert_eq!(chain.store().get(&old7).unwrap().status, Status::Invalid, "the old branch is invalid");
        assert_eq!(chain.store().get(&blocks[N - 1].id()).unwrap().status, Status::Invalid, "and so is every descendant");
        assert!(chain.status().conserved);
    }
    same_tip(&a, &b);
    assert_eq!(a.status().reverts.len(), 1);
    assert_eq!(a.status().reverts[0].rule_set, 2, "last known + 1");
    assert_eq!(a.reverts().activations_line(), format!("1@0,2@{}", H + 1));

    // Reverted is the first event, before any finalized block
    let reverted = Event::Reverted { to_height: H, keep: entry.keep, vetoed: vec![old7], rehearsal: true };
    assert_eq!(a.take_events(), vec![reverted.clone()]);
    assert_eq!(b.take_events(), vec![reverted]);
    // the retracted transactions come back in block order, coinbases skipped
    let in_block_7: Vec<Vec<u8>> = crate::chain::tx::split_body(&blocks[H as usize].body).unwrap().into_iter().skip(1).collect();
    assert_eq!(in_block_7.len(), 1);
    assert_eq!(a.take_retracted_txs(), in_block_7);
    assert!(a.take_retracted_txs().is_empty(), "drained once");

    // headers and blocks of the old branch, and descendants nobody stored, are refused
    assert!(matches!(a.accept_header(&blocks[H as usize].header, NOW), Err(ChainError::Checkpoint { height, expected }) if height == H + 1 && expected == entry.keep));
    // a child of the old tip, stamped with the new rule set so the parent is what fails
    let child = Block { header: Header { version: 2, ..orphan.header }, body: orphan.body.clone() };
    assert!(matches!(a.accept_header(&child.header, NOW), Err(ChainError::Checkpoint { .. })));
    assert!(matches!(b.accept_block(&child, NOW), Err(ChainError::Checkpoint { .. })));
    // and the child as the old release made it carries a rule set the table no longer allows there
    assert!(matches!(a.accept_header(&orphan.header, NOW), Err(ChainError::Header(HeaderError::RuleSet(1)))));
    assert!(a.status().upgrade_required.is_none(), "an older rule set is not an upgrade");
    assert!(matches!(b.accept_block(&blocks[H as usize], NOW), Err(ChainError::Checkpoint { .. })));
    assert_eq!(b.tip().height, H);

    // a mines the new branch: rule set 2, time not before the anchor
    let new: Vec<Block> = (0..4).map(|_| mine_fast(&mut a, m1, &[])).collect();
    assert_eq!(new[0].header.version & 0xffff, 2);
    assert!(new[0].header.time >= anchor_time);
    assert_eq!(new[0].header.height, H + 1);
    assert!(feed(&mut b, &new).iter().all(|v| *v == BlockVerdict::Extended));
    same_tip(&a, &b);
    assert_eq!(a.tip().height, H + 4);

    // a fresh node that opens with the entry never takes the old branch and syncs the new
    let mut d = net.open_reverts("d", vec![entry.clone()]);
    feed(&mut d, &blocks[..H as usize]);
    assert!(matches!(d.accept_block(&blocks[H as usize], NOW), Err(ChainError::Checkpoint { .. })));
    assert!(feed(&mut d, &new).iter().all(|v| *v == BlockVerdict::Extended));
    same_tip(&a, &d);

    // a block at H that is not `keep` is invalid whatever its work: a sibling of the kept block
    let mut e = net.open("e");
    feed(&mut e, &blocks[..H as usize - 1]);
    let sibling = mine_fast(&mut e, m1, &[]);
    assert_eq!(sibling.header.height, H);
    assert_ne!(sibling.id(), entry.keep);
    assert!(matches!(a.accept_block(&sibling, NOW), Err(ChainError::Checkpoint { height, expected }) if height == H && expected == entry.keep));

    // a restart applies nothing again: same tip, the log still names the last revert
    let tip = a.tip();
    drop(a);
    let mut a2 = net.reopen_with(0, vec![entry.clone()]).unwrap();
    assert_eq!(a2.tip(), tip);
    assert!(a2.take_events().is_empty() && a2.take_retracted_txs().is_empty());
    assert_eq!(a2.status().reverted, Some(Reverted { to_height: H, keep: entry.keep, vetoed: vec![old7], rehearsal: true }));
}

#[test]
fn an_old_release_node_reports_upgrade_required_and_keeps_its_tip() {
    let net = Net::new(3);
    let m1 = net.neurons[2];
    let mut a = net.open("a");
    let blocks = grow_old(&net, &mut a);
    // the old-release node: no entry, the old branch up to H
    let mut old = net.open("old");
    feed(&mut old, &blocks[..H as usize]);
    let tip = old.tip();
    assert!(old.status().upgrade_required.is_none());
    // the new branch, mined by a node that runs the entry
    let entry = entry_for(&blocks, blocks[H as usize - 1].header.time + 300);
    drop(a);
    let mut a = net.reopen_with(0, vec![entry]).unwrap();
    let first = mine_fast(&mut a, m1, &[]);
    assert_eq!(first.header.version & 0xffff, 2);

    // junk with an unknown rule set and no proof of work does not set the flag
    let mut junk = first.header;
    for n in 0u128.. {
        junk.nonce = n.to_le_bytes();
        if !valid_pow(&junk) {
            break;
        }
    }
    assert_eq!(old.accept_header(&junk, NOW), Err(ChainError::Header(HeaderError::RuleSet(2))));
    assert!(old.status().upgrade_required.is_none(), "valid proof of work first");

    // the real block: rejected, not stored, flag set with height and rule set
    assert_eq!(old.accept_block(&first, NOW), Err(ChainError::Header(HeaderError::RuleSet(2))));
    assert_eq!(old.status().upgrade_required, Some((H + 1, 2)));
    assert!(old.header(&first.id()).is_none(), "never stored, so never relayed");
    assert_eq!(old.tip(), tip, "the node keeps its tip");
    // and it still follows its own branch
    let more = mine_fast(&mut old, m1, &[]);
    assert_eq!(more.header.version & 0xffff, 1);
    assert_eq!(old.tip().height, H + 1);
}

#[test]
fn an_inconsistent_table_refuses_to_open() {
    let net = Net::new(3);
    let mut a = net.open("a");
    let blocks = grow_old(&net, &mut a);
    // a side branch from height 4, stored in a (not heavier, so not the tip)
    let mut f = net.open("f");
    feed(&mut f, &blocks[..4]);
    let fork: Vec<Block> = (0..6).map(|_| mine_fast(&mut f, net.neurons[2], &[])).collect(); // heights 5..=10
    feed(&mut a, &fork);
    drop(a);
    let ok = entry_for(&blocks, 1_306);
    let refuses = |entries: Vec<RevertEntry>, needle: &str| match net.reopen_with(0, entries) {
        Err(ChainError::Store(m)) => assert!(m.contains("revert table") && m.contains(needle), "{m}"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("opened with an inconsistent table ({needle})"),
    };
    refuses(vec![RevertEntry { vetoed: vec![], ..ok.clone() }], "vetoed is empty");
    refuses(vec![RevertEntry { rule_set: 1, ..ok.clone() }], "rule set");
    refuses(vec![RevertEntry { keep: blocks[4].id(), ..ok.clone() }], "is at height 5");
    let off_branch = RevertEntry { height: 9, keep: fork[4].id(), vetoed: vec![fork[5].id()], anchor_time: 2_000, rule_set: 0, rehearsal: true, counter: 0 };
    refuses(vec![ok.clone(), off_branch], "does not descend");
    let on_old = RevertEntry { height: 9, keep: blocks[8].id(), vetoed: vec![blocks[9].id()], anchor_time: 2_000, rule_set: 0, rehearsal: true, counter: 0 };
    refuses(vec![ok.clone(), on_old], "keep descends from");
    // the consistent table opens, and the store was not touched by the refusals
    let c = net.reopen_with(0, vec![ok.clone()]).unwrap();
    assert_eq!(c.tip().id, ok.keep);
}

#[test]
fn a_rehearsal_refuses_to_start_on_a_production_chain() {
    let net = Net::named(2, "pussy");
    let mut a = net.open("a");
    let blocks: Vec<Block> = (0..3).map(|_| mine_fast(&mut a, net.neurons[1], &[])).collect();
    drop(a);
    let entry = RevertEntry { height: 2, keep: blocks[1].id(), vetoed: vec![blocks[2].id()], anchor_time: 2_000, rule_set: 0, rehearsal: true, counter: 0 };
    match net.reopen_with(0, vec![entry]) {
        Err(ChainError::Store(m)) => assert!(m.contains("production chain"), "{m}"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("a rehearsal started on a production chain"),
    }
    // without the flag the same store opens as before
    assert_eq!(net.reopen(0).tip().height, 3);
}

fn header_at(height: u64, time: u64) -> Header {
    Header { version: 1, height, time, bits: 0x1d10_0000, nonce: [0; 16], prev: [0; 32], tx_root: [0; 32], state_root: [0; 32], chain: [0; 32], sample: [0; 32] }
}

#[test]
fn asert_reanchors_above_an_entry_in_the_function() {
    let anchor = Anchor { target: U256::ONE << 228, time: 0 };
    let parent_h = header_at(10, 300); // on schedule: bits 0x1d100000 from the genesis anchor
    let ancestor = |h: u64| (h == 10).then_some(parent_h);
    let reanchors = [ReAnchor { height: 10, anchor_time: 10_000 }];
    let first = daa::next_bits(anchor, &parent_h);
    assert_eq!(first, 0x1d10_0000);
    // the block at H + 1 is plain §1.5 from the parent at H
    assert_eq!(daa::next_bits_reverted(anchor, &reanchors, &parent_h, &ancestor), first);
    // the block at H + 2, parent at H + 1 stamped at the anchor: Δt = 0, ideal = 0
    let parent_11 = header_at(11, 10_000);
    assert_eq!(daa::next_bits_reverted(anchor, &reanchors, &parent_11, &ancestor), first);
    // the genesis anchor would see hours of drift and climb 2^16 and more
    // (toward POW_LIMIT = 2^252, which this drift does not reach)
    let drifted = decode_bits(daa::next_bits(anchor, &parent_11)).unwrap();
    assert!(drifted >= U256::ONE << 244 && drifted < U256::ONE << 252, "{drifted:?}");
    // one interval later and on schedule (ideal = 30 s): still the anchor target
    let on_time = header_at(12, 10_030);
    assert_eq!(daa::next_bits_reverted(anchor, &reanchors, &on_time, &ancestor), first);
    // 30 s late against the ideal: the target rises
    let late = header_at(12, 10_060);
    let late_bits = daa::next_bits_reverted(anchor, &reanchors, &late, &ancestor);
    assert!(decode_bits(late_bits).unwrap() > decode_bits(first).unwrap());
    // no entry below the parent, or no entries at all: exactly §1.5
    assert_eq!(daa::next_bits_reverted(anchor, &[], &late, &ancestor), daa::next_bits(anchor, &late));
    assert_eq!(daa::next_bits_reverted(anchor, &reanchors, &header_at(9, 270), &ancestor), daa::next_bits(anchor, &header_at(9, 270)));
    // a second entry anchors on the first one's result
    let second = [ReAnchor { height: 10, anchor_time: 10_000 }, ReAnchor { height: 12, anchor_time: 20_000 }];
    let parent_12 = header_at(12, 10_090);
    let anc2 = |h: u64| match h {
        10 => Some(parent_h),
        12 => Some(parent_12),
        _ => None,
    };
    let base_12 = daa::next_bits_reverted(anchor, &second[..1], &parent_12, &anc2); // bits of block 13 under entry one alone
    let parent_13 = header_at(13, 20_000);
    assert_eq!(daa::next_bits_reverted(anchor, &second, &parent_13, &anc2), base_12);
}

#[test]
fn asert_reanchors_in_the_engine_and_is_unchanged_without_an_entry() {
    let net = Net::new(3);
    let m1 = net.neurons[2];
    let mut a = net.open("a");
    let blocks = grow_old(&net, &mut a);
    // without an entry: plain §1.5 on the tip, whatever the targets did
    let s = a.status();
    assert_eq!(s.next_bits, daa::next_bits(a.genesis().anchor(), &a.tip().header));
    assert!(decode_bits(s.bits).unwrap() < U256::ONE << 255, "the fast chain pulled the target down");
    drop(a);

    let anchor_time = blocks[H as usize - 1].header.time + 300;
    let mut a = net.reopen_with(0, vec![entry_for(&blocks, anchor_time)]).unwrap();
    // H + 1: plain §1.5 from the parent at H, and stamped at the anchor
    let first = mine_fast(&mut a, m1, &[]);
    assert_eq!(first.header.time, anchor_time);
    assert_eq!(first.header.bits, daa::next_bits(a.genesis().anchor(), &blocks[H as usize - 1].header));
    // H + 2: the anchor is the time of release and the target of H + 1, not genesis
    let s = a.status();
    assert_eq!(s.next_bits, first.header.bits);
    assert_ne!(s.next_bits, daa::next_bits(a.genesis().anchor(), &first.header), "genesis would see the gap as drift");
    let second = mine(&mut a, m1, 0, &[]); // on schedule: parent + 30 s
    assert_eq!(second.header.bits, first.header.bits);
    // H + 3 is 30 s after its parent against an ideal of one interval: unchanged again
    assert_eq!(a.status().next_bits, first.header.bits);
    // the template carries the rule set and the floor time of the entry
    let t = template(&mut a, m1, 0, &[]);
    assert_eq!(t.header.version & 0xffff, 2);
    assert_eq!(t.header.bits, first.header.bits);
}

// ── revert statements of the owner key (chain.md §5.6) ─────────────────

/// A statement signed by the test genesis' checkpoint key (the test's own key,
/// never a real secret).
fn statement(net: &Net, height: u64, keep: Id, counter: u64, anchor_time: u64) -> RevertStatement {
    RevertStatement::sign(&net.genesis.chain_id, height, keep, counter, anchor_time, &net.checkpoint_key).unwrap()
}

fn reverted(height: u64, keep: Id) -> Event {
    Event::Reverted { to_height: height, keep, vetoed: Vec::new(), rehearsal: false }
}

#[test]
fn a_statement_of_the_checkpoint_key_rewinds_the_chain_and_no_other_does() {
    let net = Net::new(3);
    let m1 = net.neurons[2];
    let mut a = net.open("a");
    let blocks = grow_old(&net, &mut a);
    let keep = blocks[H as usize - 1].id();
    let anchor = blocks[H as usize - 1].header.time + 300;
    let old7 = blocks[H as usize].id();
    let chain = net.genesis.chain_id;

    // (b) a statement of any other key is refused and changes nothing
    let forged = RevertStatement::sign(&chain, H, keep, 1, anchor, &key(5)).unwrap();
    assert_eq!(a.accept_revert_statement(&forged), Err(StatementError::Signature));
    assert_eq!((a.tip().height, a.reverts().is_empty(), a.take_events().len()), (N as u64, true, 0));
    // (c) a checkpoint signature is not a revert, and a revert signature is not a checkpoint
    let cp = Checkpoint::sign(&chain, H, keep, &net.checkpoint_key).unwrap();
    let as_revert = RevertStatement { version: 1, chain, height: H, keep, counter: 1, anchor_time: anchor, sig: cp.sig };
    assert_eq!(a.accept_revert_statement(&as_revert), Err(StatementError::Signature));
    let st = statement(&net, H, keep, 1, anchor);
    let as_checkpoint = Checkpoint { height: 120, block_id: keep, sig: st.sig.clone() };
    assert_eq!(a.accept_checkpoint(&as_checkpoint), Err(ChainError::CheckpointRejected(CheckpointError::Signature)));
    assert_eq!(a.tip().height, N as u64);
    // a counter of zero is not a statement; nor is one for another chain
    assert_eq!(a.accept_revert_statement(&statement(&net, H, keep, 0, anchor)), Err(StatementError::Counter(0)));
    let other_chain = RevertStatement::sign(&[9; 32], H, keep, 1, anchor, &net.checkpoint_key).unwrap();
    assert_eq!(a.accept_revert_statement(&other_chain), Err(StatementError::WrongChain));

    // (a) the owner's statement: back to `keep`, the empty veto list, the time floor and the rule set do the rest
    assert_eq!(a.accept_revert_statement(&st), Ok(true));
    assert_eq!((a.tip().height, a.tip().id), (H, keep));
    assert_eq!(a.store().get(&old7).unwrap().status, Status::Invalid, "the old block at H + 1 is older than the anchor");
    assert_eq!(a.store().get(&blocks[N - 1].id()).unwrap().status, Status::Invalid);
    assert!(a.status().conserved);
    assert_eq!(a.take_events(), vec![reverted(H, keep)]);
    assert!(a.take_retracted_txs().is_empty(), "the retracted transactions are dropped, not re-admitted");
    let entry = a.status().reverts[0].clone();
    assert_eq!((entry.height, entry.counter, entry.rule_set, entry.vetoed.len(), entry.rehearsal), (H, 1, 2, 0, false));
    assert_eq!(a.reverts().activations_line(), format!("1@0,2@{}", H + 1));
    assert_eq!(a.status().reverted, Some(Reverted { to_height: H, keep, vetoed: Vec::new(), rehearsal: false }));
    // the old branch is refused, the new one is rule set 2 at or after the anchor
    assert!(matches!(a.accept_header(&blocks[H as usize].header, NOW), Err(ChainError::Checkpoint { height, expected }) if height == H + 1 && expected == keep));
    let new: Vec<Block> = (0..3).map(|_| mine_fast(&mut a, m1, &[])).collect();
    assert_eq!(new[0].header.version & 0xffff, 2);
    assert!(new[0].header.time >= anchor);
    // a block at H + 1 on the kept branch that is older than the anchor is refused
    let mut c = net.open("c");
    feed(&mut c, &blocks[..H as usize]);
    let early = mine_fast(&mut c, m1, &[]);
    assert!(early.header.time < anchor);
    let early_v2 = solve_on(&game_of(&mut c, early.header.height), Header { version: 2, ..early.header });
    assert!(matches!(a.accept_header(&early_v2, NOW), Err(ChainError::Time { .. })));
    // (d) the same statement again changes nothing
    assert_eq!(a.accept_revert_statement(&st), Ok(false));
    assert_eq!(a.tip().height, H + 3);
}

#[test]
fn a_statement_goes_back_deeper_than_max_reorg() {
    let net = Net::new(3);
    let m0 = net.neurons[1];
    let mut a = net.open("a");
    let blocks = mine_n(&mut a, m0, MAX_REORG as usize + 11);
    const BACK: u64 = 3;
    let keep = blocks[BACK as usize - 1].id();
    assert!(!crate::chain::select::reorg_allowed(a.tip().height, BACK), "the reorg limit forbids this depth");
    assert_eq!(a.accept_revert_statement(&statement(&net, BACK, keep, 1, blocks[BACK as usize - 1].header.time + 300)), Ok(true));
    assert_eq!((a.tip().height, a.tip().id), (BACK, keep));
    // the state is the state of a node that never saw the rest
    let mut b = net.open("b");
    feed(&mut b, &blocks[..BACK as usize]);
    same_tip(&a, &b);
}

#[test]
fn a_higher_counter_at_a_lower_height_supersedes_and_a_lower_counter_is_ignored() {
    let net = Net::new(3);
    let (m0, m1) = (net.neurons[1], net.neurons[2]);
    let mut a = net.open("a"); // store 0
    let blocks = grow_old(&net, &mut a);
    let (h1, h2) = (8u64, 5u64);
    let st1 = statement(&net, h1, blocks[h1 as usize - 1].id(), 1, blocks[h1 as usize - 1].header.time + 300);
    assert_eq!(a.accept_revert_statement(&st1), Ok(true));
    assert_eq!(a.tip().height, h1);
    let mid: Vec<Block> = (0..3).map(|_| mine_fast(&mut a, m1, &[])).collect();
    assert_eq!(mid[0].header.version & 0xffff, 2);
    assert_eq!(a.tip().height, h1 + 3);

    // counter 2 at H2 < H1 drops the entry of counter 1 and goes further back
    let keep2 = blocks[h2 as usize - 1].id();
    let st2 = statement(&net, h2, keep2, 2, 5_000);
    assert_eq!(a.accept_revert_statement(&st2), Ok(true));
    assert_eq!((a.tip().height, a.tip().id), (h2, keep2));
    let held: Vec<_> = a.status().reverts.iter().map(|e| (e.height, e.counter, e.rule_set)).collect();
    assert_eq!(held, vec![(h2, 2, 3)], "entry 1 is gone; the rule set is the compiled max + the counter");
    assert_eq!(a.reverts().activations_line(), format!("1@0,3@{}", h2 + 1));
    assert!(a.status().conserved);
    // (d) a lower counter, and a duplicate, are ignored
    assert_eq!(a.accept_revert_statement(&st1), Ok(false));
    assert_eq!(a.accept_revert_statement(&st2), Ok(false));
    assert_eq!(a.tip().height, h2);
    let new = mine_fast(&mut a, m0, &[]);
    assert_eq!((new.header.version & 0xffff, new.header.height), (3, h2 + 1));
    assert!(new.header.time >= 5_000);

    // (h) a reopened node keeps the reverted chain: the statements are on disk, nothing is applied again
    let tip = a.tip();
    let state = a.status().state_root;
    drop(a);
    let mut a2 = net.reopen(0);
    assert_eq!(a2.tip(), tip);
    assert_eq!(a2.status().state_root, state);
    let held: Vec<_> = a2.status().reverts.iter().map(|e| (e.height, e.counter)).collect();
    assert_eq!(held, vec![(h2, 2)]);
    assert!(a2.take_events().is_empty() && a2.take_retracted_txs().is_empty());
    assert_eq!(a2.status().reverted, Some(Reverted { to_height: h2, keep: keep2, vetoed: Vec::new(), rehearsal: false }));
    assert_eq!(a2.accept_revert_statement(&st2), Ok(false), "and knows it holds it");
    // a drill and an authority do not mix
    drop(a2);
    let drill = RevertEntry { height: 2, keep: blocks[1].id(), vetoed: vec![blocks[2].id()], anchor_time: 2_000, rule_set: 0, rehearsal: true, counter: 0 };
    match net.reopen_with(0, vec![drill]) {
        Err(ChainError::Store(m)) => assert!(m.contains("must not mix"), "{m}"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("opened with a rehearsal over held statements"),
    }
}

#[test]
fn a_statement_can_come_before_its_keep_and_a_wrong_height_is_refused() {
    let net = Net::new(3);
    let mut a = net.open("a");
    let blocks = grow_old(&net, &mut a);
    let keep = blocks[H as usize - 1].id();
    let anchor = blocks[H as usize - 1].header.time + 300;
    let st = statement(&net, H, keep, 1, anchor);

    // a node at height 3 has neither `keep` nor a tip at H: it holds the statement and rewinds nothing
    let mut e = net.open("e");
    feed(&mut e, &blocks[..3]);
    assert_eq!(e.accept_revert_statement(&st), Ok(true));
    assert_eq!(e.tip().height, 3);
    assert!(e.take_events().is_empty(), "nothing to revert yet");
    feed(&mut e, &blocks[3..H as usize]);
    assert_eq!(e.tip().id, keep);
    // the old block above `keep` has the old rule set: refused; the new branch is taken
    assert!(matches!(e.accept_block(&blocks[H as usize], NOW), Err(ChainError::Header(HeaderError::RuleSet(1)))));
    // a statement whose `keep` is stored at another height is refused and not held
    let wrong = statement(&net, H - 1, keep, 1, anchor);
    match a.accept_revert_statement(&wrong) {
        Err(StatementError::Inconsistent(m)) => assert!(m.contains(&format!("is at height {H}")), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(a.reverts().is_empty());
    assert_eq!(a.tip().height, N as u64);
    // and with a rehearsal in the table, the owner's word is not taken
    let drill = RevertEntry { height: 2, keep: [1; 32], vetoed: vec![[2; 32]], anchor_time: 2_000, rule_set: 0, rehearsal: true, counter: 0 };
    let mut r = net.open_reverts("r", vec![drill]);
    assert_eq!(r.accept_revert_statement(&st), Err(StatementError::Rehearsal));
}

/// The receipt for the spec's "to be timed" cost line (chain.md §5.6): the
/// per-block cost of applying an undo journal and of replaying a block from
/// genesis, on the machine that runs the tests. Prints, does not assert.
#[test]
fn p9b_timing_undo_and_replay_per_block() {
    use std::time::Instant;
    const BLOCKS: usize = 60;
    let net = Net::new(3);
    let (m0, m1) = (net.neurons[1], net.neurons[2]);
    let mut a = net.open("timing"); // store 0
    for k in 0..BLOCKS as u64 {
        mine_fast(&mut a, m0, &[net.transfer(0, k, m1, 1, 224)]);
    }
    assert_eq!(a.tip().height, BLOCKS as u64);
    drop(a);

    // the cost of opening an empty chain (genesis state, store, economy), subtracted from the replay
    let t = Instant::now();
    let empty = net.open("timing-empty"); // store 1
    let baseline = t.elapsed();
    drop(empty);

    let t = Instant::now();
    let mut a = net.reopen(0);
    let replay = t.elapsed().saturating_sub(baseline);
    assert_eq!(a.tip().height, BLOCKS as u64);

    // undo: the journal of each block, newest first, straight onto the state
    let ids: Vec<_> = (1..=BLOCKS as u64).rev().map(|h| a.store().main_at(h).unwrap()).collect();
    let undos: Vec<_> = ids.iter().map(|id| a.store().undo(id).expect("every block's journal is kept")).collect();
    let t = Instant::now();
    for undo in &undos {
        a.state_mut().revert(undo);
    }
    let undo = t.elapsed();

    let n = BLOCKS as u128;
    println!("p9b-timing undo_per_block_us={} replay_per_block_us={} blocks={}", undo.as_micros() / n, replay.as_micros() / n, BLOCKS);
    println!("p9b-timing-detail undo_total_us={} replay_total_us={} open_empty_us={}", undo.as_micros(), replay.as_micros(), baseline.as_micros());
}
