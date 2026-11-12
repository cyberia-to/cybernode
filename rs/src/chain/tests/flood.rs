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
    assert_eq!(rejected, 3, "exactly V_BUDGET forged headers were recomputed and rejected");
    assert_eq!(waited, f - 3, "the rest waited for the window without a recomputation");
    // the store is at the cap and no further: the flood reached it and every
    // later forgery evicted an older one (lowest chainwork, oldest first)
    assert_eq!(unverified, UNVERIFIED_CAP, "the unverified store holds exactly the cap under a 2×cap flood");
    assert_eq!(c.tip().id, tip, "no forged header became the tip");
    // the forged headers above the tip are unverified work: they never make
    // the node think it is catching up, so the budget stays on (R18-14)
    let next = c.tip().header.height + 1;
    assert!(!c.catching_up_for_test(next), "a flood at the tip is not a PoW-valid chain longer than D: the budget stays on");
    c.set_budget_on(true);

    // the honest block of the same height is served once the budget renews —
    // a flood delays it, it never starves it (rewards.md §7 "Verification")
    c.renew_verify_budget();
    let honest = block_of(&template(&mut c, miner, 0, &[]));
    assert_eq!(c.accept_block(&honest, NOW), Ok(BlockVerdict::Extended), "the honest block is admitted (an eviction made room) and served after the window renews");
    assert_eq!(c.tip().id, honest.id());
}

/// Initial sync verifies at full speed (R18-14; tru#45 020c020, §7's scope
/// sentence): a fresh node joining a chain of 2·D + 50 blocks receives the
/// PoW-valid header chain first (header-first sync, chain-wire §3.1), then
/// the bodies; every block more than D below the best header is catch-up
/// and spends no `V_BUDGET` slot — with a budget of one and no window
/// renewal, all of them apply. The last D blocks are at the tip's frontier
/// and are budgeted again (the window renews per block here).
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
    assert!(b.catching_up_for_test(1), "the next block is more than D below the best PoW-valid header");
    // bodies: everything strictly more than D below the best header (heights
    // 1 … best − D − 1) applies without a slot
    let beyond = chain.len() - D as usize - 1;
    for blk in &chain[..beyond] {
        let v = b.accept_block(blk, NOW).unwrap_or_else(|e| panic!("height {} throttled during catch-up: {e}", blk.header.height));
        assert!(matches!(v, BlockVerdict::Extended | BlockVerdict::Reorganized { .. }), "{v:?}");
    }
    assert_eq!(b.tip().header.height, beyond as u64);
    assert_eq!(b.flood_state().0, 0, "catch-up spent no verification slot");
    // the frontier: the last D blocks are budgeted again (one per renewed window)
    for blk in &chain[beyond..] {
        assert!(!b.catching_up_for_test(blk.header.height), "within D of the best header: the tip's rule");
        b.renew_verify_budget();
        b.accept_block(blk, NOW).unwrap();
    }
    assert_eq!(b.tip().id, a.tip().id);
}
