//! Engine tests on a private test genesis with a simulated miner
//! (chain-conformance §8–§9). The harness here; scenarios in the siblings.

mod corpus;
mod economy;
mod flood;
mod forks;
mod revert;
mod rules;
mod simulate;

use std::path::PathBuf;

use crate::genesis::file::{Balance, GenesisFile, Link};

use super::api::{Block, ChainApi, ChainError, Template};
use super::engine::{Chain, OpenOptions};
use super::params::EpochClock;
use super::genesis::{Genesis, Params};
use super::header::Header;
use super::params::test_pow;
use super::revert::RevertEntry;
use super::pow::valid_pow;
use super::sigs::{self, legacy_owner, neuron_of, token};
use super::state::Id;
use super::tx::{Body, Tx};

/// A clock far enough ahead that the future-drift rule never fires unless
/// a test wants it to.
pub const NOW: u64 = 1 << 40;
pub const GENESIS_TIME: u64 = 1_000;
pub const FUNDS: u64 = 1_000_000_000;

pub struct Net {
    pub genesis: Genesis,
    pub keys: Vec<mudra::SigningKey>,
    pub neurons: Vec<Id>,
    pub checkpoint_key: mudra::SigningKey,
    pub native: Id,
    pub dormant_key: usize,
    /// a short epoch clock for economy tests; `None` = the spec's clock
    pub clock: Option<EpochClock>,
    pub engine: Option<std::sync::Arc<dyn super::economy::focus::FocusEngine>>,
    pub inline_settlement: bool,
    dirs: std::cell::RefCell<Vec<PathBuf>>,
}

pub fn key(i: u8) -> mudra::SigningKey {
    mudra::SigningKey::from_bytes((&[i + 1; 32]).into()).expect("non-zero scalar")
}

pub fn pubkey(k: &mudra::SigningKey) -> [u8; 33] {
    mudra::cosmos::compressed(k.verifying_key())
}

impl Net {
    /// `accounts` funded neurons, one dormant legacy owner (claimable by
    /// key `accounts`), checkpoints signed by key 99, target 2^255.
    pub fn new(accounts: u8) -> Net {
        Net::with_funds(accounts, FUNDS)
    }

    /// As [`Net::new`] with `funds` per account.
    pub fn with_funds(accounts: u8, funds: u64) -> Net {
        Net::with_funds_named(accounts, funds, "test")
    }

    /// As [`Net::new`] with the genesis naming chain `name` (a launch
    /// chain's name makes the engine treat it as a production chain).
    pub fn named(accounts: u8, name: &str) -> Net {
        Net::with_funds_named(accounts, FUNDS, name)
    }

    fn with_funds_named(accounts: u8, funds: u64, name: &str) -> Net {
        Net::with_funds_links(accounts, funds, name, &[([1; 32], [2; 32])])
    }

    /// As [`Net::with_funds_named`] with a chosen genesis edge set (author
    /// `neurons[0]`) — the base graph the economy settles over. A chain
    /// a → b → c (particles 1, 2, 3) gives a high-φ* sink (3) a lock can
    /// concentrate onto, so an epoch's `v(N) > 0` (rewards.md §7 step 4); the
    /// default is one edge 1 → 2.
    pub fn with_funds_links(accounts: u8, funds: u64, name: &str, edges: &[(Id, Id)]) -> Net {
        test_pow::relax();
        let keys: Vec<_> = (0..accounts).map(key).collect();
        let neurons: Vec<Id> = keys.iter().map(|k| neuron_of(&pubkey(k))).collect();
        let checkpoint_key = key(99);
        let native = token("pussy");
        let dormant = legacy_owner("pussy", &mudra::cosmos::account_id(&pubkey(&key(accounts))));
        let mut balances: Vec<Balance> = neurons.iter().map(|n| Balance { owner: *n, token: native, amount: funds }).collect();
        balances.push(Balance { owner: dormant, token: native, amount: 10_000 });
        balances.push(Balance { owner: dormant, token: token("other"), amount: 5 });
        balances.sort();
        let s0: u64 = balances.iter().filter(|b| b.token == native).map(|b| b.amount).sum();
        let file = GenesisFile {
            params: Params {
                version: 1,
                name: name.into(),
                hrp: "pussy".into(),
                denom: "pussy".into(),
                time: GENESIS_TIME,
                bits: test_pow::EASY_BITS,
                subsidy: super::params::subsidy(s0),
                checkpoint_key: pubkey(&checkpoint_key),
                checkpoint_sunset: super::params::CHECKPOINT_SUNSET,
                pool: super::pool::Pool::genesis(),
            },
            balances,
            links: edges.iter().map(|(p, q)| Link { neuron: neurons[0], p: *p, q: *q, height: 7 }).collect(),
        };
        Net {
            genesis: Genesis::from_file(&file).unwrap(),
            keys,
            neurons,
            checkpoint_key,
            native,
            dormant_key: accounts as usize,
            clock: None,
            engine: None,
            inline_settlement: false,
            dirs: Default::default(),
        }
    }

    pub fn subsidy(&self) -> u64 {
        self.genesis.subsidy()
    }

    /// A fresh chain store in a temp dir.
    pub fn open(&self, label: &str) -> Chain {
        self.open_with(label, true)
    }

    pub fn open_with(&self, label: &str, checkpoints: bool) -> Chain {
        self.open_dir(label, checkpoints, Vec::new())
    }

    /// A fresh chain store opened with rehearsal revert entries (§5.6).
    pub fn open_reverts(&self, label: &str, reverts: Vec<RevertEntry>) -> Chain {
        self.open_dir(label, true, reverts)
    }

    fn open_dir(&self, label: &str, checkpoints: bool, reverts: Vec<RevertEntry>) -> Chain {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut dir = std::env::temp_dir();
        dir.push(format!("soft3-chain-test-{label}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        self.dirs.borrow_mut().push(dir.clone());
        let mut opts = self.options(checkpoints);
        opts.reverts = reverts;
        Chain::open_with(&dir, &self.genesis.bytes, opts).unwrap()
    }

    fn options(&self, checkpoints: bool) -> OpenOptions {
        OpenOptions { checkpoints, clock: self.clock, engine: self.engine.clone(), inline_settlement: self.inline_settlement, quadrature: 0, reverts: Vec::new() }
    }

    /// Reopen the store of chain `index` (recovery path).
    pub fn reopen(&self, index: usize) -> Chain {
        let dir = self.dirs.borrow()[index].clone();
        Chain::open_with(&dir, &self.genesis.bytes, self.options(true)).unwrap()
    }

    /// Reopen the store of chain `index` with rehearsal revert entries; the
    /// previous handle on that store must be dropped first.
    pub fn reopen_with(&self, index: usize, reverts: Vec<RevertEntry>) -> Result<Chain, ChainError> {
        let dir = self.dirs.borrow()[index].clone();
        let mut opts = self.options(true);
        opts.reverts = reverts;
        Chain::open_with(&dir, &self.genesis.bytes, opts)
    }

    pub fn unlock(&self, from: usize, nonce: u64, p: Id, q: Id, amount: u64, fee: u64) -> Vec<u8> {
        let mut tx = Tx { body: Body::Unlock { from: self.neurons[from], nonce, p, q, amount, fee }, sig: vec![] };
        sigs::sign(&self.genesis.chain_id, &mut tx, &self.keys[from]).unwrap();
        tx.encode()
    }

    pub fn payout(&self, from: usize, nonce: u64, p: Id, q: Id, payout: Id, fee: u64) -> Vec<u8> {
        let mut tx = Tx { body: Body::Payout { from: self.neurons[from], nonce, p, q, payout, fee }, sig: vec![] };
        sigs::sign(&self.genesis.chain_id, &mut tx, &self.keys[from]).unwrap();
        tx.encode()
    }

    /// A cyberlink naming another token (a staked one is invalid).
    pub fn link_token(&self, from: usize, nonce: u64, token: Id, p: Id, q: Id, amount: u64, valence: i8, fee: u64) -> Vec<u8> {
        let mut tx = Tx { body: Body::Cyberlink { from: self.neurons[from], nonce, token, amount, p, q, valence, fee }, sig: vec![] };
        sigs::sign(&self.genesis.chain_id, &mut tx, &self.keys[from]).unwrap();
        tx.encode()
    }

    pub fn transfer(&self, from: usize, nonce: u64, to: Id, amount: u64, fee: u64) -> Vec<u8> {
        let mut tx = Tx {
            body: Body::Transfer { from: self.neurons[from], nonce, token: self.native, amount, to, fee },
            sig: vec![],
        };
        sigs::sign(&self.genesis.chain_id, &mut tx, &self.keys[from]).unwrap();
        tx.encode()
    }

    pub fn link(&self, from: usize, nonce: u64, p: Id, q: Id, amount: u64, valence: i8, fee: u64) -> Vec<u8> {
        let mut tx = Tx {
            body: Body::Cyberlink { from: self.neurons[from], nonce, token: self.native, amount, p, q, valence, fee },
            sig: vec![],
        };
        sigs::sign(&self.genesis.chain_id, &mut tx, &self.keys[from]).unwrap();
        tx.encode()
    }

    pub fn claim(&self, key_index: u8, nonce: u64, fee: u64) -> Vec<u8> {
        let k = key(key_index);
        let mut tx = Tx { body: Body::Claim { account_id: mudra::cosmos::account_id(&pubkey(&k)), nonce, fee }, sig: vec![] };
        sigs::sign(&self.genesis.chain_id, &mut tx, &k).unwrap();
        tx.encode()
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        for d in self.dirs.borrow().iter() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Nonce search from 0 at the template's target (about two attempts), each
/// attempt the ticket of the template on `game` (rewards.md §7).
pub fn solve_on(game: &super::ticket::EpochGame, mut header: Header) -> Header {
    for n in 0u128.. {
        header.nonce = n.to_le_bytes();
        game.solve(&mut header, 2);
        if valid_pow(&header) {
            return header;
        }
    }
    unreachable!()
}

/// [`solve_on`] with the template's own game.
pub fn solve(t: &Template, header: Header) -> Header {
    solve_on(&t.game, header)
}

/// The game the next block on `chain`'s tip samples.
pub fn game_of(chain: &mut Chain, height: u64) -> std::sync::Arc<super::ticket::EpochGame> {
    let parent = chain.header_at(height - 1).expect("a parent").id();
    chain.game_for(&parent, height).expect("the game of the next block")
}

pub fn block_of(t: &Template) -> Block {
    Block { header: solve(t, t.header), body: t.body.clone() }
}

/// Mine on the tip: template at `time` (0 = parent + 30) with `txs`, solve, accept.
pub fn mine(chain: &mut Chain, to: Id, time: u64, txs: &[Vec<u8>]) -> Block {
    let t = template(chain, to, time, txs);
    let block = block_of(&t);
    let verdict = chain.accept_block(&block, NOW).unwrap();
    assert!(matches!(verdict, super::api::BlockVerdict::Extended), "{verdict:?}");
    block
}

pub fn template(chain: &mut Chain, to: Id, time: u64, txs: &[Vec<u8>]) -> Template {
    let time = if time == 0 { chain.tip().header.time + 30 } else { time };
    chain.header_template(to, time, txs).unwrap()
}

/// Mine `n` empty blocks paying `to`.
pub fn mine_n(chain: &mut Chain, to: Id, n: usize) -> Vec<Block> {
    (0..n).map(|_| mine(chain, to, 0, &[])).collect()
}

/// Feed blocks to another chain in order.
pub fn feed(chain: &mut Chain, blocks: &[Block]) -> Vec<super::api::BlockVerdict> {
    blocks.iter().map(|b| chain.accept_block(b, NOW).unwrap()).collect()
}
