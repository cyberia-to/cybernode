//! chain.md §5.6 / chain-wire §3.1: a new connection brings the peer's revert
//! statements with its checkpoints. Two in-process nodes on a test genesis
//! with a checkpoint key: A holds the owner's statement (its `keep` is a
//! block nobody has yet — held on the signature, applied when the blocks
//! arrive); B starts afterwards, holding none, and dials A. B must hold the
//! statement without waiting for anyone to sign another.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cybernode::chain::genesis::Genesis;
use cybernode::chain::net::chain_view::ChainView;
use cybernode::chain::net::kernel::KernelChain;
use cybernode::chain::net::mempool::Mempool;
use cybernode::chain::net::{Bootstrap, Config, Net};
use cybernode::chain::revert_statement::RevertStatement;
use cybernode::chain::sigs::token;
use cybernode::genesis::file::{Balance, GenesisFile, Params};

fn temp(label: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("soft3-rs-connect-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_connection_brings_the_peers_revert_statements() {
    let signer = mudra::SigningKey::from_bytes((&[7u8; 32]).into()).unwrap();
    let native = token("rstest");
    let balances = vec![Balance { owner: [1; 32], token: native, amount: 105_120_000_000 }];
    let file = GenesisFile {
        params: Params {
            version: 1,
            name: "rstest".into(),
            hrp: "rstest".into(),
            denom: "rstest".into(),
            time: 1_800_000_000,
            bits: cybernode::chain::params::POW_LIMIT_BITS,
            subsidy: cybernode::chain::params::subsidy(105_120_000_000),
            checkpoint_key: mudra::cosmos::compressed(signer.verifying_key()),
            checkpoint_sunset: cybernode::chain::params::CHECKPOINT_SUNSET,
            pool: cybernode::chain::pool::Pool::genesis(),
        },
        balances,
        links: vec![],
    };
    let genesis = Genesis::from_file(&file).unwrap();
    let start = |label: &str, bootstrap: Vec<Bootstrap>| {
        let dir = temp(label);
        let kernel = Arc::new(KernelChain::open(&genesis.bytes, &dir).unwrap());
        let chain: Arc<dyn ChainView> = kernel.clone();
        let cfg = Config { chain_dir: dir, radio_bind: Some("127.0.0.1:0".parse().unwrap()), relay: false, bootstrap, neuron: [0; 32] };
        (Net::start(cfg, chain, Arc::new(Mutex::new(Mempool::new())), false), kernel)
    };
    let (a, _ka) = {
        let (net, kernel) = start("a", vec![]);
        (net.await.unwrap(), kernel)
    };
    let st = RevertStatement::sign(&genesis.chain_id, 5, [9; 32], 1, 1_900_000_000, &signer).unwrap().encode();
    assert_eq!(a.publish_revert(&st).await, Ok(true), "A holds the statement");
    let forged = RevertStatement::sign(&genesis.chain_id, 5, [8; 32], 2, 1_900_000_000, &mudra::SigningKey::from_bytes((&[9u8; 32]).into()).unwrap()).unwrap().encode();
    assert!(a.publish_revert(&forged).await.is_err(), "another key's statement is refused");
    let (b, kb) = {
        let (net, kernel) = start("b", vec![Bootstrap { name: "a".into(), id: a.id(), addrs: a.bound() }]);
        (net.await.unwrap(), kernel)
    };
    let t0 = Instant::now();
    while kb.revert_statements().is_empty() && t0.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(kb.revert_statements(), vec![st], "B fetched A's revert statement on connecting");
    b.shutdown().await;
    a.shutdown().await;
}
