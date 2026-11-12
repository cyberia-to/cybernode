//! Proof of work and chainwork (§1.2, §5.1). Integer only.

use super::header::{BlockId, Header, decode_bits};
use super::u256::U256;

/// `u256_le(block_id) < target(bits)`; equality fails (§1.2). Genesis is
/// exempt: height 0 is pinned by the genesis file, not mined.
pub fn valid_pow(header: &Header) -> bool {
    if header.height == 0 {
        return true;
    }
    let Ok(target) = decode_bits(header.bits) else {
        return false;
    };
    id_meets_target(&header.id(), target)
}

/// The comparison alone, for a miner that already has the id.
pub fn id_meets_target(id: &BlockId, target: U256) -> bool {
    U256::from_le_bytes(id) < target
}

/// `work(header) = floor(2^256 / (target + 1))` (§5.1).
pub fn work(header: &Header) -> U256 {
    header.target().work()
}

/// `chainwork(B) = chainwork(prev) + work(B)` (§5.1).
pub fn chainwork(prev: U256, header: &Header) -> U256 {
    prev.saturating_add(work(header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::params::GENESIS_BITS;

    fn base() -> Header {
        Header {
            version: 1,
            height: 1,
            time: 1_762_349_000,
            bits: 0x1d10_0000,
            nonce: [0; 16],
            prev: [0x11; 32],
            tx_root: [0x22; 32],
            state_root: [0x33; 32],
            chain: [0x44; 32],
            sample: [0; 32],
        }
    }

    #[test]
    fn test_header_fails_its_own_target() {
        let h = base();
        assert!(!valid_pow(&h));
        let mut genesis = h;
        genesis.height = 0;
        assert!(valid_pow(&genesis), "height 0 is exempt");
    }

    #[test]
    fn work_and_chainwork() {
        // chain-conformance §5 amended 2026-11-04: work in attempts, each a sample
        let mut h = base();
        h.bits = GENESIS_BITS;
        assert_eq!(work(&h), U256::from_u64(127), "2^249: 127 expected attempts");
        let mut acc = U256::ZERO;
        for _ in 0..3 {
            acc = chainwork(acc, &h);
        }
        assert_eq!(acc, U256::from_u64(381));
        assert_eq!(chainwork(U256::MAX, &h), U256::MAX);
        h.bits = super::super::params::POW_LIMIT_BITS;
        assert_eq!(work(&h), U256::from_u64(15), "2^252: 15");
        h.bits = 0x1e01_0000;
        assert_eq!(work(&h), U256::from_u64(16_777_215), "2^232 stays a canonical example");
    }
}
