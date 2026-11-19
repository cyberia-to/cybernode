//! The invalid-block generator of the corpus: one valid block and its
//! game in, every mutation a conformance run must reject out — the body
//! and transaction mutations re-ground so each gets an id of its own, and
//! the forged tickets (a wrong marginal, a wrong probe, a wrong order, a
//! garbage sample, a stale base) that validation step 8 must refuse.

use std::collections::HashSet;

use super::super::api::{Block, ChainApi};
use super::super::header::{self, BlockId, Header};
use super::super::pow::valid_pow;
use super::super::state::Id;
use super::super::ticket::{EpochGame, sample_of};
use super::super::tree;
use super::super::tx::{self, Body, Tx};
use super::GRIND_LIMIT;

/// One mutated block and the verdict code it must draw.
#[derive(Clone, Debug)]
pub struct Invalid {
    pub name: &'static str,
    pub block: Block,
    pub expected: String,
}

/// First nonce (from 0) whose header — its sample the ticket of its
/// template on `game`, or `corrupt`'s rewrite of it — does (`want = true`)
/// or does not meet its target and whose id is not in `skip`; bounded, so a
/// hard target yields `None` for `want = true`. `skip` holds the valid
/// block's id and the ids of the mutations already generated: each mutation
/// must get an id of its own, because a body mutation that reused the valid
/// header would put the valid id in the rejected set (§5.4) and every later
/// one would draw `rejected` instead of its own code.
fn grind(game: &EpochGame, mut h: Header, want: bool, skip: &HashSet<BlockId>, corrupt: &dyn Fn(&EpochGame, &Header) -> [u8; 32]) -> Option<Header> {
    for n in 0..GRIND_LIMIT {
        h.nonce = n.to_le_bytes();
        h.sample = corrupt(game, &h);
        if valid_pow(&h) == want && !skip.contains(&h.id()) {
            return Some(h);
        }
    }
    None
}

/// The honest sample of a template (rewards.md §7).
fn honest(game: &EpochGame, h: &Header) -> [u8; 32] {
    sample_of(&game.ticket(&h.template(), 2))
}

/// The invalid tickets (rewards.md §7 "Verification"; chain-conformance §5
/// amended): a header whose id passes the target with a sample that does
/// not commit to the ticket of its template.
fn wrong_marginal(game: &EpochGame, h: &Header) -> [u8; 32] {
    let mut t = game.ticket(&h.template(), 2);
    // one marginal off by one unit of 2^-60 nats; the honest sample when the
    // game has no player (the mutation is then not produced)
    if let Some(m) = t.marginals.first_mut() {
        *m += 1;
    }
    sample_of(&t)
}

fn wrong_probe(game: &EpochGame, h: &Header) -> [u8; 32] {
    let mut t = game.ticket(&h.template(), 2);
    // the first probe's value off by one unit; the honest sample when the
    // base has no axon (the mutation is then not produced)
    if let Some(p) = t.probes.first_mut() {
        p.value += 1;
    }
    sample_of(&t)
}

/// The marginals in rank order rather than ascending by player: differs
/// only when the window covers two players out of index order.
fn wrong_order(game: &EpochGame, h: &Header) -> [u8; 32] {
    let mut t = game.ticket(&h.template(), 2);
    t.marginals.reverse();
    t.covered.reverse();
    sample_of(&t)
}

/// The mutations so far and every block id taken (the valid block's included).
struct Mutations<'a> {
    out: Vec<Invalid>,
    used: HashSet<BlockId>,
    game: &'a EpochGame,
}

impl Mutations<'_> {
    fn add(&mut self, name: &'static str, header: Header, body: &[u8], expected: &str) {
        self.used.insert(header.id());
        self.out.push(Invalid { name, block: Block { header, body: body.to_vec() }, expected: expected.into() });
    }

    /// Re-grind the nonce so proof of work passes with the honest sample
    /// and the id is unused.
    fn sealed(&mut self, name: &'static str, header: Header, body: Vec<u8>, expected: &str) {
        if let Some(g) = grind(self.game, header, true, &self.used, &honest) {
            self.add(name, g, &body, expected);
        }
    }

    /// Re-grind the nonce so the id passes the target with a sample
    /// `corrupt` rewrites: an invalid ticket (step 8).
    fn forged(&mut self, name: &'static str, header: Header, body: &[u8], corrupt: &dyn Fn(&EpochGame, &Header) -> [u8; 32]) {
        if let Some(g) = grind(self.game, header, true, &self.used, corrupt) {
            if g.sample != honest(self.game, &g) {
                self.add(name, g, body, "reject:block:sample");
            }
        }
    }
}

fn coinbase_bytes(height: u64, to: Id, amount: u64, extra: [u8; 32]) -> Vec<u8> {
    Tx { body: Body::Coinbase { height, to, amount, extra }, sig: vec![] }.encode()
}

/// For the valid next block of `chain`'s tip, one mutated block per
/// validation step (chain.md §5.5) that can be reached without a key. Each
/// fails at exactly its step: those targeting steps 6–8 get their nonce
/// re-ground so proof of work passes, and recompute the tx root unless that
/// is what they break. Every mutation's block id is distinct from `valid`'s
/// and from every other mutation's (a body mutation reusing the valid header
/// would put the valid id in the rejected set, §5.4, and every later one would
/// draw `rejected`); the nonce search skips ids already used. Deterministic.
/// A mutation whose nonce search runs out (hard target) is left out. Mutations needing a transaction (`bad_signature`,
/// `duplicate_tx`, `coinbase_position`) appear when `valid` carries one;
/// `replayed_tx` when the last 64 blocks of the main chain carry one.
pub fn invalid_blocks<C: ChainApi + ?Sized>(chain: &C, valid: &Block, game: &EpochGame) -> Vec<Invalid> {
    let h = valid.header;
    let mut m = Mutations { out: Vec::new(), used: HashSet::from([valid.id()]), game };

    // steps 1–5: header only (everything before PoW fails before PoW is read)
    m.add("wrong_chain", Header { chain: [0xEE; 32], ..h }, &valid.body, "reject:header:wrong_chain");
    m.add("rule_set", Header { version: (h.version & !0xffff) | 2, ..h }, &valid.body, "reject:header:rule_set");
    m.add("bits_zero", Header { bits: 0, ..h }, &valid.body, "reject:header:bits");
    m.add("second_genesis", Header { height: 0, ..h }, &valid.body, "reject:genesis");
    m.add("unknown_parent", Header { prev: [0xAB; 32], ..h }, &valid.body, "reject:unknown_parent");
    m.add("wrong_height", Header { height: h.height + 1, ..h }, &valid.body, "reject:height");
    m.add("time_not_after_mtp", Header { time: 0, ..h }, &valid.body, "reject:time");
    if let Ok(target) = header::decode_bits(h.bits) {
        let harder = target >> 1;
        if !harder.is_zero() {
            m.add("wrong_bits", Header { bits: header::encode_bits(harder), ..h }, &valid.body, "reject:bits");
        }
    }
    if let Some(bad) = grind(game, h, false, &m.used, &honest) {
        m.add("bad_pow", bad, &valid.body, "reject:pow");
    }

    // step 8, the sample (rewards.md §7): the id passes, the ticket does not
    m.forged("wrong_marginal", h, &valid.body, &wrong_marginal);
    m.forged("wrong_probe", h, &valid.body, &wrong_probe);
    m.forged("wrong_preimage_order", h, &valid.body, &wrong_order);
    m.forged("garbage_sample", h, &valid.body, &|_, h| {
        let mut s = h.template();
        s[0] ^= 0xff;
        header::hash32(&s)
    });

    // steps 6–8: body and state rules, the nonce re-ground
    let rooted = |raws: &[Vec<u8>]| Header { tx_root: tree::tx_root(raws), ..h };

    m.sealed("tx_root", Header { tx_root: flip(h.tx_root), ..h }, valid.body.clone(), "reject:block:tx_root");
    m.sealed("state_root", Header { state_root: flip(h.state_root), ..h }, valid.body.clone(), "reject:block:state_root");
    m.sealed("empty_body", h, tx::join_body(&[]), "reject:block:empty");
    if let Some(cut) = valid.body.len().checked_sub(1) {
        m.sealed("truncated_body", h, valid.body[..cut].to_vec(), "reject:block:body");
    }

    let raws = tx::split_body(&valid.body).unwrap_or_default();
    let cb = raws.first().and_then(|r| Tx::decode(r).ok()).and_then(|t| match t.body {
        Body::Coinbase { height, to, amount, extra } => Some((height, to, amount, extra)),
        _ => None,
    });
    if let Some((height, to, amount, extra)) = cb {
        let cb_raw = &raws[0];
        let with = |first: Vec<u8>| -> Vec<Vec<u8>> {
            let mut v = raws.clone();
            v[0] = first;
            v
        };
        let r = with(coinbase_bytes(height + 1, to, amount, extra));
        m.sealed("coinbase_height", rooted(&r), tx::join_body(&r), "reject:block:coinbase_height");
        let r = with(coinbase_bytes(height, to, amount + 1, extra));
        m.sealed("coinbase_amount", rooted(&r), tx::join_body(&r), "reject:block:coinbase_amount");
        let r = vec![cb_raw.clone(), cb_raw.clone()];
        m.sealed("coinbase_in_body", rooted(&r), tx::join_body(&r), "reject:block:tx:coinbase_in_body");

        if let Some(t) = raws.get(1) {
            let r = vec![cb_raw.clone(), t.clone(), t.clone()];
            m.sealed("duplicate_tx", rooted(&r), tx::join_body(&r), "reject:block:duplicate_tx");
            let mut bad = t.clone();
            let last = bad.len() - 1;
            bad[last] ^= 1;
            let r = with_tx(&raws, 1, bad);
            m.sealed("bad_signature", rooted(&r), tx::join_body(&r), "reject:block:tx:signature");
            let r = vec![t.clone(), cb_raw.clone()];
            m.sealed("coinbase_position", rooted(&r), tx::join_body(&r), "reject:block:coinbase_position");
        }

        // a transaction an earlier block spent: its nonce is behind the account's
        let tip = chain.tip().height;
        let old = (1..=tip).rev().take(64).find_map(|n| {
            let b = chain.blocks(n, 1).into_iter().next()?;
            tx::split_body(&b.body).ok()?.into_iter().skip(1).last()
        });
        let fees = raws.iter().skip(1).try_fold(0u64, |a, r| a.checked_add(Tx::decode(r).ok()?.fee()));
        if let (Some(old), Some(fees)) = (old, fees) {
            let old_fee = Tx::decode(&old).map(|t| t.fee()).unwrap_or(0);
            if let Some(amount) = amount.checked_sub(fees).and_then(|share| share.checked_add(old_fee)) {
                let r = vec![coinbase_bytes(height, to, amount, extra), old];
                m.sealed("replayed_tx", rooted(&r), tx::join_body(&r), "reject:block:tx:nonce");
            }
        }
    }
    m.out
}

/// A block whose ticket was computed on another game (`stale`: the previous
/// epoch's base), the id passing the target: `reject:block:sample`.
pub fn stale_base<C: ChainApi + ?Sized>(_chain: &C, valid: &Block, game: &EpochGame, stale: &EpochGame, used: &HashSet<BlockId>) -> Option<Invalid> {
    if stale.cluster == game.cluster && stale.beacon == game.beacon {
        return None;
    }
    let g = grind(game, valid.header, true, used, &|_, h| honest(stale, h))?;
    (g.sample != honest(game, &g)).then(|| Invalid { name: "stale_base", block: Block { header: g, body: valid.body.clone() }, expected: "reject:block:sample".into() })
}

fn with_tx(raws: &[Vec<u8>], at: usize, replacement: Vec<u8>) -> Vec<Vec<u8>> {
    let mut v = raws.to_vec();
    v[at] = replacement;
    v
}

fn flip(mut b: [u8; 32]) -> [u8; 32] {
    b[0] ^= 1;
    b
}
