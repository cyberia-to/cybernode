//! cybernode — rung 7 of the soft3 ladder: the node. moved from the `soft3` crate on 2026-10-27 (soft3/proposals/cybernode.md); the code is unchanged, the wire identifiers (`soft3/chain/…/v1`) and the home directories keep their names.
//!
//! - [[network]] — spacepussy-test product network (soft3 chaosnet)
//! - [[node]] — the node itself over cybergraph + bbg
//! - [[genesis]] — burial snapshot → genesis manifest → bbg root (specs/genesis.md)
//! - [[wallet]] — sign and submit phase-1 root chain transactions (specs/chain.md)
//! - [[chain]] — the phase-1 proof-of-work root chain kernel (specs/chain.md);
//!   `chain::mine` is the reference miner with CPU, Metal and WebGPU backends
//! - CLI: `cybernode sync`, `cybernode run` (`node`), `cybernode network`, `cybernode genesis`, `cybernode mine`
//!
//! Default network: **spacepussy-test** at `https://cyb.ai/spacepussy-test`
//! (cybernode). Not the cosmos-sdk chain `space-pussy`.

pub mod chain;
pub mod cli;
pub mod genesis;
pub mod network;
pub mod node;
pub mod wallet;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn identity() -> &'static str {
    "cybernode"
}

pub fn default_network() -> network::Network {
    network::Network::DEFAULT
}

pub fn manifesto() -> &'static [&'static str] {
    &[
        "one mind — shared, provable, self-improving",
        "many languages — write, compute, mean",
        "open world — no schema, no gatekeeper",
        "engine: cybergraph + bbg on spacepussy-test",
        "default network: spacepussy-test (soft3 chaosnet)",
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_network_spacepussy_test() {
        assert_eq!(super::default_network().chain_id(), "spacepussy-test");
    }
}
