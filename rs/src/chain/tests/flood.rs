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

    // F random-sample forged headers at the tip: each passes the id test and
    // fails the sample. The first V_BUDGET are recomputed and rejected; the
    // rest wait for the window, stored but never verified past the budget.
    let f = 20usize;
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
    // the store stays bounded under the flood: at most UNVERIFIED_CAP
    // unverified headers above the tip (the admission gate in accept_header
    // refuses beyond it; here the flood is below the cap)
    assert!(unverified <= UNVERIFIED_CAP, "the unverified store is bounded: {unverified}");
    assert_eq!(c.tip().id, tip, "no forged header became the tip");

    // the honest block of the same height is served once the budget renews —
    // a flood delays it, it never starves it (rewards.md §7 "Verification")
    c.renew_verify_budget();
    let honest = block_of(&template(&mut c, miner, 0, &[]));
    assert_eq!(c.accept_block(&honest, NOW), Ok(BlockVerdict::Extended), "the honest block is served after the window renews");
    assert_eq!(c.tip().id, honest.id());
}
