//! The forged-header flood, as an in-process conformance test of this
//! crate's own test chain (the class of the corpus generator's invalid
//! blocks and `tests/chain_adversary.rs`): it opens no socket and spawns no
//! process — the chain is a `Net` scratch store and the forged headers are
//! fed through `ChainApi::accept_block` here. It exercises only a 127.0.0.1
//! dev test chain; it is never pointed at a launch chain.
//!
//! The rule under test (tru rewards.md §7 "Verification"; chain.md §5.5
//! step 8): a header whose id passes the target with any 32 bytes as its
//! sample costs the node one full recomputation to reject. The node spends
//! at most `V_BUDGET` such recomputations per block interval; a zero sample
//! is refused before a slot is spent; the rest wait, stored, until the
//! window renews, and the unverified set above the tip never exceeds
//! `UNVERIFIED_CAP`; no forged header ever becomes the tip, and the honest
//! block of the same height is served once the budget renews.

use super::*;
use crate::chain::api::{BlockVerdict, ChainApi, ChainError};
use crate::chain::apply::BlockError;
use crate::chain::params::UNVERIFIED_CAP;
use crate::chain::pow::valid_pow;

/// A forged header at `tip + 1`: the honest template with `sample` written
/// in and the nonce ground until the id passes the target (the test net's
/// target is easy, so a couple of tries). The body is the template's.
fn forged(c: &mut Chain, miner: Id, sample: [u8; 32]) -> Block {
    let t = template(c, miner, 0, &[]);
    let mut h = t.header;
    h.sample = sample;
    for n in 0u128.. {
        h.nonce = n.to_le_bytes();
        if valid_pow(&h) {
            break;
        }
    }
    Block { header: h, body: t.body }
}

#[test]
fn forged_header_flood_is_bounded_by_v_budget_and_the_store_cap() {
    let net = Net::new(2);
    let mut c = net.open("flood");
    let miner = net.neurons[1];
    mine_n(&mut c, miner, 3); // a tip with a game to sample
    c.set_budget_on(true);
    c.set_v_budget(3);
    c.renew_verify_budget(); // open a fresh window after the honest mining
    let tip = c.tip().id;

    // a zero sample is never an honest commitment: refused before a game is
    // built or a verification slot is spent (R18-5)
    let z = forged(&mut c, miner, [0; 32]);
    assert!(matches!(c.accept_block(&z, NOW), Err(ChainError::Block(BlockError::Sample(_)))));
    assert_eq!(c.flood_state().0, 0, "a zero sample costs no verification");

    // F > 2 × UNVERIFIED_CAP random-sample forged headers at the tip: each
    // passes the id test and fails the sample. The first V_BUDGET are
    // recomputed and rejected; the rest wait for the window, stored; once the
    // set is at the cap every further one evicts the oldest (R18-15), so the
    // store never grows past the cap and nothing is refused.
    let f = 2 * UNVERIFIED_CAP + 16;
    let (mut rejected, mut waited) = (0usize, 0usize);
    for i in 0..f {
        let mut s = [0x9cu8; 32];
        s[..16].copy_from_slice(&(i as u128 + 1).to_le_bytes());
        let block = forged(&mut c, miner, s);
        match c.accept_block(&block, NOW) {
            Err(ChainError::Block(BlockError::Sample(_))) => rejected += 1,
            Err(ChainError::VerifyBudget) => waited += 1,
            other => panic!("a forged header was not rejected: {other:?}"),
        }
    }
    let (spent, unverified) = c.flood_state();
    assert!(spent <= 3, "at most V_BUDGET full recomputations in the window: {spent}");
    // V_BUDGET counts branch attempts (tru#45 §7): the first forged header
    // continues the walk from the honest tip for free, then V cost a slot
    assert_eq!(rejected, 4, "V + 1 recomputations: one free continuation of the honest tip, then V slots");
    assert_eq!(waited, f - 4, "the rest waited for the window without a recomputation");
    // the store is at the cap and no further: the flood reached it and every
    // later forgery evicted another chosen by the node's draw (chain.md §5.4
    // amended: by age, then the local draw — never by claimed work)
    assert_eq!(unverified, UNVERIFIED_CAP, "the unverified store holds exactly the cap under a 2×cap flood");
    assert_eq!(c.tip().id, tip, "no forged header became the tip");
    assert_eq!(c.walking_for_test(), None, "a failed ticket ends the walk: the next attempt costs a slot (R18-14)");

    // the honest block of the same height: admitted (an eviction makes room),
    // it waits for the window, then is served within ⌈(cap + 1) / V⌉ windows
    // — each window the walker attempts V branches in the node's draw order,
    // a failed branch never returns, and no forged branch outranks the honest
    // one by claimed work (rewards.md §7 "Verification"; tru#45 §7 addendum)
    let honest = block_of(&template(&mut c, miner, 0, &[]));
    assert_eq!(c.accept_block(&honest, NOW), Err(ChainError::VerifyBudget), "the window is spent: the honest block is stored and waits");
    let bound = (UNVERIFIED_CAP + 1).div_ceil(3);
    let mut served = None;
    for w in 1..=bound {
        c.renew_verify_budget();
        let _ = c.try_advance();
        if c.tip().id == honest.id() {
            served = Some(w);
            break;
        }
    }
    assert!(served.is_some(), "the honest branch was not served within {bound} windows");
    assert_eq!(c.tip().id, honest.id());
}

/// Initial sync is one branch (R18-14; tru#45 §7): a fresh node joining a
/// chain of 2·D + 50 blocks receives the PoW-valid header chain first
/// (header-first sync, chain-wire §3.1), then the bodies; the first body is
/// one branch attempt and every further block continues the walk for free,
/// so with a budget of one and no window renewal all of them apply — no
/// catch-up exemption exists, none is needed.
#[test]
fn initial_sync_verifies_at_full_speed_beyond_d() {
    use crate::chain::params::FINALITY_DEPTH as D;
    let net = Net::new(2);
    let miner = net.neurons[1];
    let mut a = net.open("sync-a");
    let chain = mine_n(&mut a, miner, (2 * D + 50) as usize);
    let mut b = net.open("sync-b");
    b.set_budget_on(true);
    b.set_v_budget(1);
    b.renew_verify_budget();
    // headers first: PoW-valid, contiguous from the tip
    for blk in &chain {
        b.accept_header(&blk.header, NOW).unwrap();
    }
    // bodies in order: the first is one branch attempt, every later block
    // continues the walk from the block just verified for free (tru#45 §7)
    for blk in &chain {
        let v = b.accept_block(blk, NOW).unwrap_or_else(|e| panic!("height {} throttled during sync: {e}", blk.header.height));
        assert!(matches!(v, BlockVerdict::Extended | BlockVerdict::Reorganized { .. }), "{v:?}");
    }
    assert_eq!(b.tip().id, a.tip().id);
    assert_eq!(b.flood_state().0, 1, "one branch attempt for the whole chain");
}

/// R18-14 (third cut): F forged chains of D + 1 PoW-valid headers
/// above the tip, each root carrying a body so it is a branch attempt. The
/// budget counts branch attempts: at most V + 1 recomputations per interval (a
/// slot each, one free continuation), every attempted chain is dead at its first ticket together
/// with its D descendants, and the honest successor is served once the
/// window renews (≈ V/(V+F) per interval before that). Review evidence
/// (loopback, in-process; opens no socket).
#[test]
fn long_forged_chains_cost_one_branch_attempt_each() {
    use crate::chain::params::FINALITY_DEPTH as D;
    use crate::chain::store::Status;
    let net = Net::new(2);
    let mut c = net.open("longflood");
    let miner = net.neurons[1];
    mine_n(&mut c, miner, 3);
    c.set_budget_on(true);
    c.set_v_budget(1);
    c.renew_verify_budget();
    let tip = c.tip().id;
    let f = 4usize; // 4 × (D + 1) headers, below UNVERIFIED_CAP
    let mut roots = Vec::new();
    let mut tails = Vec::new();
    for i in 0..f {
        let mut s = [0x77u8; 32];
        s[..16].copy_from_slice(&(i as u128 + 1).to_le_bytes());
        let root = forged(&mut c, miner, s);
        let mut parent = root.header;
        c.accept_header(&parent, NOW).unwrap();
        for k in 0..D {
            let mut h = parent;
            h.height += 1;
            h.prev = parent.id();
            h.time = parent.time + 1;
            h.bits = c.bits_after(&parent);
            h.sample = [0x55 ^ (k as u8); 32];
            for n in 0u128.. {
                h.nonce = n.to_le_bytes();
                if valid_pow(&h) {
                    break;
                }
            }
            c.accept_header(&h, NOW).unwrap();
            parent = h;
        }
        roots.push(root);
        tails.push(parent.id());
    }
    // the chains are contiguous, PoW-valid, D + 1 long, and heavier than the tip
    assert!(c.store().iter().filter(|(_, e)| e.status != Status::Invalid).map(|(_, e)| e.header.height).max().unwrap() > c.tip().header.height + D);
    // each root arrives with its body: a branch attempt
    let mut verdicts = Vec::new();
    for r in &roots {
        verdicts.push(format!("{:?}", c.accept_block(r, NOW).map(|v| match v { BlockVerdict::NeedBodies(_) => "NeedBodies".to_string(), o => format!("{o:?}") })));
    }
    let dead: Vec<usize> = (0..f).filter(|i| c.entry(&roots[*i].id()).map(|e| e.status) == Some(Status::Invalid)).collect();
    eprintln!("LONGFLOOD verdicts={verdicts:?} dead={dead:?}");
    assert_eq!(dead.len(), 2, "V_BUDGET + 1 branch attempts were recomputed (the +1 is the free continuation of the walk from the tip), each root dead at its first ticket");
    assert!(c.flood_state().0 <= 1, "at most V_BUDGET slots: {}", c.flood_state().0);
    for i in dead.clone() {
        let (r, t) = (&roots[i], &tails[i]);
        assert_eq!(c.entry(&r.id()).map(|e| e.status), Some(Status::Invalid), "the attempted root is dead");
        assert_eq!(c.entry(t).map(|e| e.status), Some(Status::Invalid), "and its D descendants with it");
    }
    assert_eq!(c.tip().id, tip, "no forged chain became the tip");
    // the honest successor competes with the surviving forged roots by the
    // local draw: served with probability about V/(V + F) in a window, and
    // within F + 1 windows for certain, since every window that does not
    // serve it kills one forged root at its first ticket
    let honest = block_of(&template(&mut c, miner, 0, &[]));
    let mut windows = 0usize;
    loop {
        c.renew_verify_budget();
        windows += 1;
        assert!(windows <= f + 1, "the honest branch was not served within F + 1 windows");
        let v = c.accept_block(&honest, NOW).or_else(|_| c.try_advance());
        if c.tip().id == honest.id() {
            assert!(matches!(v, Ok(BlockVerdict::Extended)), "{v:?}");
            break;
        }
    }
    eprintln!("LONGFLOOD honest served in window {windows} of at most {}", f + 1);
}

/// A failed sample always scores (R18-17): when another root at the tip has
/// no body yet, the forged root that failed its ticket is still reported as
/// `Sample` — never folded into that root's `NeedBodies` — so the sender's
/// +100 and ban are never lost to another branch's state.
#[test]
fn a_failed_sample_is_reported_beside_a_body_less_root() {
    let net = Net::new(2);
    let mut c = net.open("score");
    let miner = net.neurons[1];
    mine_n(&mut c, miner, 2);
    c.set_budget_on(true);
    c.set_v_budget(3);
    c.renew_verify_budget();
    let tip = c.tip().id;
    // a body-less root at the tip (a header only), then a forged root with a body
    let bare = forged(&mut c, miner, [0x41; 32]);
    assert_eq!(c.accept_header(&bare.header, NOW), Ok(crate::chain::api::HeaderVerdict::Accepted));
    let bad = forged(&mut c, miner, [0x42; 32]);
    assert!(matches!(c.accept_block(&bad, NOW), Err(ChainError::Block(BlockError::Sample(_)))), "the failed sample is reported, not NeedBodies");
    assert_eq!(c.accept_header(&bad.header, NOW), Err(ChainError::Rejected(bad.id())), "remembered as rejected");
    assert_eq!(c.tip().id, tip);
    // the body-less root is still wanted by sync
    assert!(matches!(c.try_advance(), Ok(BlockVerdict::NeedBodies(ids)) if ids == vec![bare.id()]));
}
