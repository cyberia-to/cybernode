//! The chains one binary serves (chain.md §4.4): their names, hrps and the
//! pins — the chain id (the genesis file's particle) and the id of block 0 —
//! with the compile-time hex decoder that catches a typo in a pin.

/// An embedded chain: name, hrp, chain id (= genesis file particle) and the
/// id of block 0. Non-zero ids are pinned: the node refuses a genesis file
/// whose particle differs, and a kernel whose block 0 differs. pussy is
/// pinned to the values blessed 2026-10-17 in `conformance/chain-genesis.toml`
/// (and `chain.snap`). Zero ids mean "not yet pinned": bostrom waits until
/// pussy's genesis is covered end to end; until pinned, `--genesis` is
/// mandatory for that chain and any file naming it is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedChain {
    pub name: &'static str,
    pub hrp: &'static str,
    pub chain_id: [u8; 32],
    pub genesis_block: [u8; 32],
    pub genesis_time: u64,
}

impl EmbeddedChain {
    /// True once the chain id is pinned (non-zero).
    pub fn pinned(&self) -> bool {
        self.chain_id != [0; 32]
    }
}

/// Decode 64 hex digits at compile time; a typo in a pin fails the build.
const fn h32(s: &str) -> [u8; 32] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("pin is not lowercase hex"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 64, "pin is not 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nibble(b[2 * i]) << 4) | nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// The two chains one binary serves (§4.4).
pub const CHAINS: &[EmbeddedChain] = &[
    EmbeddedChain {
        name: "pussy",
        hrp: "pussy",
        // re-blessed 2026-11-12: the file carries GENESIS_BITS 0x20020000 (2^249) and the
        // 200-byte block 0 (rewards.md §7, chain.md §0/§1.1 amended); bae85f40… / 73208ee2…
        // were the 0x1e010000 file and its 168-byte block 0 (conformance/chain-genesis.toml)
        chain_id: h32("a94ee10120f2d24009f091eb5f620de0e65f67ef2619073988766d50be566d7f"),
        genesis_block: h32("b5aca6a0446c3fd3080dc8cac4a03c3b0bafc41fe9847b9779f19a6faaeb6e06"),
        genesis_time: 1_793_885_820,
    },
    EmbeddedChain {
        // unpinned on purpose: bostrom waits for pussy's genesis coverage
        name: "bostrom",
        hrp: "bostrom",
        chain_id: [0; 32],
        genesis_block: [0; 32],
        genesis_time: 1_793_884_962,
    },
];

/// Look up an embedded chain by name.
pub fn embedded(name: &str) -> Option<&'static EmbeddedChain> {
    CHAINS.iter().find(|c| c.name == name)
}
